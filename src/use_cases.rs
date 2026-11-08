//! Tenant-scoped catalog operations using Lakekeeper's unmodified OpenFGA model.
//! Transactional project and object locks are deliberately visible here: they
//! serialize app-owned security mutations and protect the authorization boundary.
use crate::application::{App, CatalogTable, TableInput, TableMetadata, TablePage, Tuple};
use crate::domain::{
    self, Event, Lifecycle, NamespaceId, Object, Privilege, ProjectId, RoleId, Scope, Subject,
    TableAction, TableId, UserId,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction, postgres::PgRow};
use tokio::task::JoinSet;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum Error {
    #[error("invalid request")]
    Invalid,
    #[error("not found")]
    NotFound,
    #[error("conflict")]
    Conflict,
    #[error("gone")]
    Gone,
    #[error("service unavailable")]
    Unavailable,
    #[error("forbidden")]
    Forbidden,
    #[error("unauthenticated")]
    Unauthenticated,
    #[error("too many requests")]
    Throttled,
}
type Result<T> = std::result::Result<T, Error>;

struct RelationshipUpdate<'a> {
    operation: &'static str,
    owner: &'a str,
    namespace: &'a str,
    writes: &'a [Tuple],
    deletes: &'a [Tuple],
}

fn storage(error: sqlx::Error) -> Error {
    let category = match error {
        sqlx::Error::PoolTimedOut => "pool_timeout",
        sqlx::Error::Database(_) => "database",
        sqlx::Error::Io(_) => "transport",
        _ => "storage",
    };
    tracing::warn!(category, "catalog database operation failed");
    Error::Unavailable
}
fn authorization(_: crate::application::AuthError) -> Error {
    tracing::warn!("catalog authorization operation failed closed");
    Error::Unavailable
}
fn table_from_row(row: &PgRow) -> Result<CatalogTable> {
    Ok(CatalogTable {
        id: row.try_get("id").map_err(storage)?,
        project_id: row.try_get("project_id").map_err(storage)?,
        warehouse_id: row.try_get("warehouse_id").map_err(storage)?,
        namespace_id: row.try_get("namespace_id").map_err(storage)?,
        owner_id: row.try_get("owner_id").map_err(storage)?,
        name: row.try_get("name").map_err(storage)?,
        description: row.try_get("description").map_err(storage)?,
    })
}
fn table_object(scope: &Scope, id: &TableId) -> Object {
    Object::Table {
        warehouse: scope.warehouse.clone(),
        id: id.clone(),
    }
}
fn tuple(user: impl Into<String>, relation: &str, object: impl Into<String>) -> Tuple {
    Tuple {
        user: user.into(),
        relation: relation.to_owned(),
        object: object.into(),
    }
}
pub(crate) fn provisioning_tuples(
    warehouse: &str,
    id: &str,
    namespace: &str,
    owner: &str,
) -> Vec<Tuple> {
    let object = format!("lakekeeper_table:{warehouse}/{id}");
    vec![
        tuple(format!("user:{owner}"), "ownership", object.clone()),
        tuple(format!("namespace:{namespace}"), "parent", object.clone()),
        tuple(object, "child", format!("namespace:{namespace}")),
    ]
}

impl App {
    async fn migration_active(&self) -> Result<()> {
        let Some(target) = &self.authorization_target else {
            // The emergency deny provider never grants access and does not
            // need a configured or available authorization store.
            return Ok(());
        };
        crate::authorization_migration::ensure_active(
            &self.db,
            &target.store,
            &target.model,
            &target.model_sha256,
        )
        .await
        .map_err(|_| {
            tracing::warn!("catalog request refused before authorization migration activation");
            Error::Unavailable
        })
    }
    async fn project_transaction(
        &self,
        actor: &UserId,
        project: &ProjectId,
        write: bool,
    ) -> Result<Transaction<'_, Postgres>> {
        self.migration_active().await?;
        let mut tx = self.db.begin().await.map_err(storage)?;
        // All security mutations through this application take this same lock,
        // including role memberships that can affect many descendant objects.
        let query = if write {
            "SELECT p.id FROM catalog_projects p JOIN catalog_memberships m ON m.project_id=p.id WHERE p.id=$1 AND m.user_id=$2 FOR UPDATE OF p"
        } else {
            "SELECT p.id FROM catalog_projects p JOIN catalog_memberships m ON m.project_id=p.id WHERE p.id=$1 AND m.user_id=$2 FOR SHARE OF p"
        };
        let found: Option<String> = sqlx::query_scalar(query)
            .bind(project.as_str())
            .bind(actor.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
        if found.is_none() {
            return Err(Error::NotFound);
        }
        if let Some(target) = &self.authorization_target {
            let active: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM authorization_migration WHERE singleton AND status='active' AND store_id=$1 AND model_id=$2 AND model_sha256=$3)")
                .bind(&target.store).bind(&target.model).bind(&target.model_sha256)
                .fetch_one(&mut *tx).await.map_err(storage)?;
            if !active {
                return Err(Error::Unavailable);
            }
        }
        Ok(tx)
    }
    async fn scoped_transaction(
        &self,
        actor: &UserId,
        scope: &Scope,
        write: bool,
    ) -> Result<Transaction<'_, Postgres>> {
        let mut tx = self
            .project_transaction(actor, &scope.project, write)
            .await?;
        let found: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM catalog_warehouses WHERE project_id=$1 AND id=$2)",
        )
        .bind(scope.project.as_str())
        .bind(scope.warehouse.as_str())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage)?;
        if !found {
            return Err(Error::NotFound);
        }
        Ok(tx)
    }
    async fn require(&self, actor: &UserId, action: &str, object: &Object) -> Result<()> {
        if self
            .auth
            .check(actor.as_str(), action, &object.to_string())
            .await
            .map_err(authorization)?
        {
            Ok(())
        } else {
            tracing::info!(event="authorization_denied",action,actor=%actor,object=%object,"catalog audit");
            Err(Error::Forbidden)
        }
    }
    async fn active_table(
        tx: &mut Transaction<'_, Postgres>,
        scope: &Scope,
        id: &TableId,
        write: bool,
    ) -> Result<CatalogTable> {
        let query = if write {
            "SELECT id,project_id,warehouse_id,namespace_id,owner_id,name,description,status FROM catalog_tables WHERE project_id=$1 AND warehouse_id=$2 AND id=$3 FOR UPDATE"
        } else {
            "SELECT id,project_id,warehouse_id,namespace_id,owner_id,name,description,status FROM catalog_tables WHERE project_id=$1 AND warehouse_id=$2 AND id=$3"
        };
        let row = sqlx::query(query)
            .bind(scope.project.as_str())
            .bind(scope.warehouse.as_str())
            .bind(id.as_str())
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage)?
            .ok_or(Error::NotFound)?;
        let state = Lifecycle::from_storage(&row.try_get::<String, _>("status").map_err(storage)?)
            .ok_or(Error::Unavailable)?;
        if !domain::is_visible(state) {
            return Err(Error::NotFound);
        }
        table_from_row(&row)
    }
    async fn namespace_exists(
        tx: &mut Transaction<'_, Postgres>,
        scope: &Scope,
        namespace: &NamespaceId,
    ) -> Result<()> {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM catalog_namespaces WHERE project_id=$1 AND warehouse_id=$2 AND id=$3)")
            .bind(scope.project.as_str()).bind(scope.warehouse.as_str()).bind(namespace.as_str())
            .fetch_one(&mut **tx).await.map_err(storage)?;
        if exists { Ok(()) } else { Err(Error::NotFound) }
    }
    async fn subject_exists(
        tx: &mut Transaction<'_, Postgres>,
        project: &ProjectId,
        subject: &Subject,
    ) -> Result<()> {
        let (query, id) = match subject {
            Subject::User(id) => (
                "SELECT EXISTS(SELECT 1 FROM catalog_memberships WHERE project_id=$1 AND user_id=$2)",
                id.as_str(),
            ),
            Subject::Role(id) => (
                "SELECT EXISTS(SELECT 1 FROM catalog_roles WHERE project_id=$1 AND id=$2)",
                id.as_str(),
            ),
        };
        let exists: bool = sqlx::query_scalar(query)
            .bind(project.as_str())
            .bind(id)
            .fetch_one(&mut **tx)
            .await
            .map_err(storage)?;
        if exists { Ok(()) } else { Err(Error::NotFound) }
    }
    pub(crate) async fn catalog(&self, actor: &UserId) -> Result<Value> {
        self.migration_active().await?;
        let rows = sqlx::query("SELECT w.id AS warehouse_id,w.name AS warehouse_name,p.id AS project_id,p.name AS project_name FROM catalog_warehouses w JOIN catalog_projects p ON p.id=w.project_id JOIN catalog_memberships m ON m.project_id=p.id WHERE m.user_id=$1 ORDER BY p.id,w.id LIMIT 201")
            .bind(actor.as_str()).fetch_all(&self.db).await.map_err(storage)?;
        if rows.len() > 200 {
            return Err(Error::Unavailable);
        }
        let mut scopes = Vec::new();
        for row in rows {
            let project: String = row.try_get("project_id").map_err(storage)?;
            let warehouse: String = row.try_get("warehouse_id").map_err(storage)?;
            let scope = Scope {
                project: ProjectId::try_from(project.as_str()).map_err(|_| Error::Unavailable)?,
                warehouse: crate::domain::WarehouseId::try_from(warehouse.as_str())
                    .map_err(|_| Error::Unavailable)?,
            };
            let mut tx = self.scoped_transaction(actor, &scope, false).await?;
            if !self
                .auth
                .check(
                    actor.as_str(),
                    "can_include_in_list",
                    &format!("warehouse:{warehouse}"),
                )
                .await
                .map_err(authorization)?
            {
                continue;
            }
            let namespace_rows = sqlx::query("SELECT id,name FROM catalog_namespaces WHERE project_id=$1 AND warehouse_id=$2 ORDER BY id LIMIT 201")
                .bind(&project).bind(&warehouse).fetch_all(&mut *tx).await.map_err(storage)?;
            let role_rows = sqlx::query(
                "SELECT id,name FROM catalog_roles WHERE project_id=$1 ORDER BY id LIMIT 201",
            )
            .bind(&project)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            if namespace_rows.len() > 200 || role_rows.len() > 200 {
                return Err(Error::Unavailable);
            }
            let mut namespaces = Vec::new();
            let mut roles = Vec::new();
            for row in namespace_rows {
                let id: String = row.try_get("id").map_err(storage)?;
                if self
                    .auth
                    .check(
                        actor.as_str(),
                        "can_include_in_list",
                        &format!("namespace:{id}"),
                    )
                    .await
                    .map_err(authorization)?
                {
                    namespaces.push(
                        json!({"id":id,"name":row.try_get::<String,_>("name").map_err(storage)?}),
                    );
                }
            }
            for row in role_rows {
                let id: String = row.try_get("id").map_err(storage)?;
                if self
                    .auth
                    .check(actor.as_str(), "can_read", &format!("role:{id}"))
                    .await
                    .map_err(authorization)?
                {
                    roles.push(
                        json!({"id":id,"name":row.try_get::<String,_>("name").map_err(storage)?}),
                    );
                }
            }
            let members: Vec<String> = if self
                .auth
                .check(
                    actor.as_str(),
                    "can_read_assignments",
                    &format!("project:{project}"),
                )
                .await
                .map_err(authorization)?
            {
                let members = sqlx::query_scalar("SELECT user_id FROM catalog_memberships WHERE project_id=$1 ORDER BY user_id LIMIT 201")
                    .bind(&project).fetch_all(&mut *tx).await.map_err(storage)?;
                if members.len() > 200 {
                    return Err(Error::Unavailable);
                }
                members
            } else {
                Vec::new()
            };
            tx.commit().await.map_err(storage)?;
            scopes.push(json!({
                "project_id":project,"project_name":row.try_get::<String,_>("project_name").map_err(storage)?,
                "warehouse_id":warehouse,"warehouse_name":row.try_get::<String,_>("warehouse_name").map_err(storage)?,
                "namespaces":namespaces,"roles":roles,"members":members,
            }));
        }
        Ok(json!({"scopes":scopes}))
    }
    pub(crate) async fn visible_tables(
        &self,
        actor: &UserId,
        scope: &Scope,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<TablePage> {
        if !(1..=100).contains(&limit) {
            return Err(Error::Invalid);
        }
        let mut tx = self.scoped_transaction(actor, scope, false).await?;
        let mut last = match cursor {
            None => String::new(),
            Some(cursor) => {
                uuid::Uuid::parse_str(cursor).map_err(|_| Error::Invalid)?;
                sqlx::query_scalar::<_,String>("SELECT last_id FROM catalog_list_cursors WHERE id=$1 AND actor_id=$2 AND project_id=$3 AND warehouse_id=$4 AND expires_at>now()")
                    .bind(cursor).bind(actor.as_str()).bind(scope.project.as_str()).bind(scope.warehouse.as_str())
                    .fetch_optional(&mut *tx).await.map_err(storage)?.ok_or(Error::Invalid)?
            }
        };
        let mut items = Vec::new();
        let mut scanned = 0usize;
        while items.len() < limit as usize && scanned < 500 {
            let rows = sqlx::query("SELECT id,project_id,warehouse_id,namespace_id,owner_id,name,description FROM catalog_tables WHERE project_id=$1 AND warehouse_id=$2 AND status='active' AND id>$3 ORDER BY id LIMIT 100")
                .bind(scope.project.as_str()).bind(scope.warehouse.as_str()).bind(&last)
                .fetch_all(&mut *tx).await.map_err(storage)?;
            if rows.is_empty() {
                break;
            }
            let candidates: Vec<_> = rows.iter().map(table_from_row).collect::<Result<_>>()?;
            for batch in candidates.chunks(8) {
                let mut tasks = JoinSet::new();
                for (index, table) in batch.iter().enumerate() {
                    let auth = self.auth.clone();
                    let actor = actor.to_string();
                    let object = format!("lakekeeper_table:{}/{}", table.warehouse_id, table.id);
                    tasks.spawn(async move {
                        auth.check(&actor, TableAction::Metadata.relation(), &object)
                            .await
                            .map(|allow| (index, allow))
                    });
                }
                let mut allowed = vec![false; batch.len()];
                while let Some(result) = tasks.join_next().await {
                    let (index, allow) = result
                        .map_err(|_| Error::Unavailable)?
                        .map_err(authorization)?;
                    allowed[index] = allow;
                }
                for (table, allow) in batch.iter().zip(allowed) {
                    last.clone_from(&table.id);
                    scanned += 1;
                    if allow {
                        items.push(table.clone());
                    }
                    if items.len() == limit as usize || scanned == 500 {
                        break;
                    }
                }
                if items.len() == limit as usize || scanned == 500 {
                    break;
                }
            }
        }
        let more: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM catalog_tables WHERE project_id=$1 AND warehouse_id=$2 AND status='active' AND id>$3)")
            .bind(scope.project.as_str()).bind(scope.warehouse.as_str()).bind(&last).fetch_one(&mut *tx).await.map_err(storage)?;
        let next_cursor = if more {
            let next = uuid::Uuid::new_v4().to_string();
            sqlx::query("DELETE FROM catalog_list_cursors WHERE expires_at<=now()")
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            sqlx::query("INSERT INTO catalog_list_cursors(id,project_id,warehouse_id,actor_id,last_id,expires_at) VALUES($1,$2,$3,$4,$5,now()+interval '15 minutes')")
                .bind(&next).bind(scope.project.as_str()).bind(scope.warehouse.as_str()).bind(actor.as_str()).bind(&last)
                .execute(&mut *tx).await.map_err(storage)?;
            Some(next)
        } else {
            None
        };
        tx.commit().await.map_err(storage)?;
        Ok(TablePage { items, next_cursor })
    }
    pub(crate) async fn get_table(
        &self,
        actor: &UserId,
        scope: &Scope,
        id: &TableId,
    ) -> Result<CatalogTable> {
        let mut tx = self.scoped_transaction(actor, scope, false).await?;
        let table = Self::active_table(&mut tx, scope, id, false).await?;
        self.require(
            actor,
            TableAction::Metadata.relation(),
            &table_object(scope, id),
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        Ok(table)
    }
    pub(crate) async fn check_table(
        &self,
        actor: &UserId,
        scope: &Scope,
        id: &TableId,
        action: TableAction,
    ) -> Result<Value> {
        let mut tx = self.scoped_transaction(actor, scope, false).await?;
        Self::active_table(&mut tx, scope, id, false).await?;
        let object = table_object(scope, id).to_string();
        let allowed = self
            .auth
            .check(actor.as_str(), action.relation(), &object)
            .await
            .map_err(authorization)?;
        tx.commit().await.map_err(storage)?;
        Ok(json!({"allowed":allowed,"action":action.relation(),"object":object}))
    }
    pub(crate) async fn create_table(
        &self,
        actor: &UserId,
        scope: &Scope,
        key: &str,
        input: &TableInput,
    ) -> Result<String> {
        if !domain::valid_table(&input.name, &input.description)
            || !(8..=128).contains(&key.len())
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(Error::Invalid);
        }
        let mut tx = self.scoped_transaction(actor, scope, true).await?;
        Self::namespace_exists(&mut tx, scope, &input.namespace_id).await?;
        self.require(
            actor,
            "can_create_table",
            &Object::Namespace(input.namespace_id.clone()),
        )
        .await?;
        let hash = Sha256::digest(serde_json::to_vec(input).map_err(|_| Error::Invalid)?).to_vec();
        let previous = sqlx::query("SELECT id,request_hash,status FROM catalog_tables WHERE project_id=$1 AND warehouse_id=$2 AND creator_id=$3 AND idempotency_key=$4 FOR UPDATE")
            .bind(scope.project.as_str()).bind(scope.warehouse.as_str()).bind(actor.as_str()).bind(key)
            .fetch_optional(&mut *tx).await.map_err(storage)?;
        let id = if let Some(row) = previous {
            if row.try_get::<Vec<u8>, _>("request_hash").map_err(storage)? != hash {
                return Err(Error::Conflict);
            }
            let id: String = row.try_get("id").map_err(storage)?;
            match row
                .try_get::<String, _>("status")
                .map_err(storage)?
                .as_str()
            {
                "active" => {
                    tx.commit().await.map_err(storage)?;
                    return Ok(id);
                }
                "pending" => id,
                "updating" => return Err(Error::Unavailable),
                _ => return Err(Error::Gone),
            }
        } else {
            let id = uuid::Uuid::new_v4().to_string();
            sqlx::query("INSERT INTO catalog_tables(project_id,warehouse_id,id,namespace_id,name,description,owner_id,creator_id,status,idempotency_key,request_hash) VALUES($1,$2,$3,$4,$5,$6,$7,$7,'pending',$8,$9)")
                .bind(scope.project.as_str()).bind(scope.warehouse.as_str()).bind(&id).bind(input.namespace_id.as_str())
                .bind(&input.name).bind(&input.description).bind(actor.as_str()).bind(key).bind(hash)
                .execute(&mut *tx).await.map_err(storage)?;
            id
        };
        tx.commit().await.map_err(storage)?;
        self.finish_creation(
            actor,
            scope,
            &TableId::try_from(id.as_str()).map_err(|_| Error::Unavailable)?,
        )
        .await?;
        tracing::info!(event="table_created",actor=%actor,project=%scope.project,warehouse=%scope.warehouse,table=%id,"catalog audit");
        Ok(id)
    }
    async fn finish_creation(&self, actor: &UserId, scope: &Scope, id: &TableId) -> Result<()> {
        let mut tx = self.scoped_transaction(actor, scope, true).await?;
        let row = sqlx::query("SELECT namespace_id,owner_id,status FROM catalog_tables WHERE project_id=$1 AND warehouse_id=$2 AND id=$3 FOR UPDATE")
            .bind(scope.project.as_str()).bind(scope.warehouse.as_str()).bind(id.as_str()).fetch_optional(&mut *tx).await.map_err(storage)?.ok_or(Error::NotFound)?;
        let state = Lifecycle::from_storage(&row.try_get::<String, _>("status").map_err(storage)?)
            .ok_or(Error::Unavailable)?;
        if state == Lifecycle::Active {
            return Ok(());
        }
        if domain::transition(state, Event::Provisioned) != Some(Lifecycle::Active) {
            return Err(Error::Gone);
        }
        let namespace: String = row.try_get("namespace_id").map_err(storage)?;
        let owner: String = row.try_get("owner_id").map_err(storage)?;
        self.auth
            .write(
                &provisioning_tuples(scope.warehouse.as_str(), id.as_str(), &namespace, &owner),
                true,
            )
            .await
            .map_err(authorization)?;
        sqlx::query("UPDATE catalog_tables SET status='active' WHERE project_id=$1 AND warehouse_id=$2 AND id=$3 AND status='pending'")
            .bind(scope.project.as_str()).bind(scope.warehouse.as_str()).bind(id.as_str()).execute(&mut *tx).await.map_err(storage)?;
        tx.commit().await.map_err(storage)
    }
    pub(crate) async fn edit_table(
        &self,
        actor: &UserId,
        scope: &Scope,
        id: &TableId,
        input: &TableMetadata,
    ) -> Result<()> {
        if !domain::valid_table(&input.name, &input.description) {
            return Err(Error::Invalid);
        }
        let mut tx = self.scoped_transaction(actor, scope, true).await?;
        let table = Self::active_table(&mut tx, scope, id, true).await?;
        self.require(
            actor,
            TableAction::Commit.relation(),
            &table_object(scope, id),
        )
        .await?;
        if input.name != table.name {
            self.require(actor, "can_rename", &table_object(scope, id))
                .await?;
        }
        sqlx::query("UPDATE catalog_tables SET name=$4,description=$5 WHERE project_id=$1 AND warehouse_id=$2 AND id=$3 AND status='active'")
            .bind(scope.project.as_str()).bind(scope.warehouse.as_str()).bind(id.as_str()).bind(&input.name).bind(&input.description)
            .execute(&mut *tx).await.map_err(storage)?;
        tx.commit().await.map_err(storage)?;
        tracing::info!(event="table_metadata_updated",actor=%actor,object=%table_object(scope,id),"catalog audit");
        Ok(())
    }
    pub(crate) async fn set_grant(
        &self,
        actor: &UserId,
        scope: &Scope,
        id: &TableId,
        privilege: Privilege,
        subject: &Subject,
        add: bool,
    ) -> Result<()> {
        let mut tx = self.scoped_transaction(actor, scope, true).await?;
        Self::active_table(&mut tx, scope, id, true).await?;
        let object = table_object(scope, id);
        let action = if add {
            privilege.grant_action()
        } else {
            privilege.revoke_action()
        };
        self.require(actor, action, &object).await?;
        Self::subject_exists(&mut tx, &scope.project, subject).await?;
        tracing::info!(event="grant_requested",actor=%actor,project=%scope.project,object=%object,subject=%subject,relation=privilege.relation(),add,"catalog audit");
        self.auth
            .write(
                &[tuple(
                    subject.to_string(),
                    privilege.relation(),
                    object.to_string(),
                )],
                add,
            )
            .await
            .map_err(authorization)?;
        tx.commit().await.map_err(storage)?;
        tracing::info!(event=if add {"grant_added"} else {"grant_revoked"},actor=%actor,project=%scope.project,object=%object,subject=%subject,relation=privilege.relation(),"catalog audit");
        Ok(())
    }
    pub(crate) async fn set_role_assignee(
        &self,
        actor: &UserId,
        project: &ProjectId,
        role: &RoleId,
        user: &UserId,
        add: bool,
    ) -> Result<()> {
        let mut tx = self.project_transaction(actor, project, true).await?;
        Self::subject_exists(&mut tx, project, &Subject::Role(role.clone())).await?;
        let object = Object::Role(role.clone());
        self.require(actor, "can_grant_assignee", &object).await?;
        Self::subject_exists(&mut tx, project, &Subject::User(user.clone())).await?;
        tracing::info!(event="role_assignment_requested",actor=%actor,project=%project,role=%role,subject=%user,add,"catalog audit");
        self.auth
            .write(
                &[tuple(
                    Subject::User(user.clone()).to_string(),
                    "assignee",
                    object.to_string(),
                )],
                add,
            )
            .await
            .map_err(authorization)?;
        tx.commit().await.map_err(storage)?;
        tracing::info!(event=if add {"role_assignee_added"} else {"role_assignee_removed"},actor=%actor,project=%project,role=%role,subject=%user,"catalog audit");
        Ok(())
    }
    pub(crate) async fn transfer_owner(
        &self,
        actor: &UserId,
        scope: &Scope,
        id: &TableId,
        owner: &UserId,
    ) -> Result<()> {
        let mut tx = self.scoped_transaction(actor, scope, true).await?;
        let table = Self::active_table(&mut tx, scope, id, true).await?;
        let object = table_object(scope, id);
        self.require(actor, TableAction::ChangeOwnership.relation(), &object)
            .await?;
        Self::subject_exists(&mut tx, &scope.project, &Subject::User(owner.clone())).await?;
        if table.owner_id == owner.as_str() {
            return Ok(());
        }
        let writes = vec![tuple(
            format!("user:{owner}"),
            "ownership",
            object.to_string(),
        )];
        let deletes = vec![tuple(
            format!("user:{}", table.owner_id),
            "ownership",
            object.to_string(),
        )];
        self.stage_update(
            &mut tx,
            actor,
            scope,
            id,
            RelationshipUpdate {
                operation: "owner",
                owner: owner.as_str(),
                namespace: &table.namespace_id,
                writes: &writes,
                deletes: &deletes,
            },
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        tracing::info!(event="owner_change_prepared",actor=%actor,project=%scope.project,object=%object,owner=%owner,"catalog audit");
        self.finish_update(actor, scope, id).await
    }
    pub(crate) async fn move_table(
        &self,
        actor: &UserId,
        scope: &Scope,
        id: &TableId,
        namespace: &NamespaceId,
    ) -> Result<()> {
        let mut tx = self.scoped_transaction(actor, scope, true).await?;
        let table = Self::active_table(&mut tx, scope, id, true).await?;
        let object = table_object(scope, id);
        self.require(actor, TableAction::Move.relation(), &object)
            .await?;
        Self::namespace_exists(&mut tx, scope, namespace).await?;
        self.require(
            actor,
            "can_accept_moved_tabular",
            &Object::Namespace(namespace.clone()),
        )
        .await?;
        if table.namespace_id == namespace.as_str() {
            return Ok(());
        }
        let writes = vec![
            tuple(
                format!("namespace:{namespace}"),
                "parent",
                object.to_string(),
            ),
            tuple(
                object.to_string(),
                "child",
                format!("namespace:{namespace}"),
            ),
        ];
        let deletes = vec![
            tuple(
                format!("namespace:{}", table.namespace_id),
                "parent",
                object.to_string(),
            ),
            tuple(
                object.to_string(),
                "child",
                format!("namespace:{}", table.namespace_id),
            ),
        ];
        self.stage_update(
            &mut tx,
            actor,
            scope,
            id,
            RelationshipUpdate {
                operation: "namespace",
                owner: &table.owner_id,
                namespace: namespace.as_str(),
                writes: &writes,
                deletes: &deletes,
            },
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        tracing::info!(event="namespace_change_prepared",actor=%actor,project=%scope.project,object=%object,namespace=%namespace,"catalog audit");
        self.finish_update(actor, scope, id).await
    }
    async fn stage_update(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        actor: &UserId,
        scope: &Scope,
        id: &TableId,
        change: RelationshipUpdate<'_>,
    ) -> Result<()> {
        let writes = serde_json::to_string(change.writes).map_err(|_| Error::Unavailable)?;
        let deletes = serde_json::to_string(change.deletes).map_err(|_| Error::Unavailable)?;
        sqlx::query("INSERT INTO catalog_pending_updates(project_id,warehouse_id,table_id,actor_id,operation,owner_id,namespace_id,writes,deletes) VALUES($1,$2,$3,$4,$5,$6,$7,$8::jsonb,$9::jsonb)")
            .bind(scope.project.as_str()).bind(scope.warehouse.as_str()).bind(id.as_str()).bind(actor.as_str()).bind(change.operation).bind(change.owner).bind(change.namespace).bind(writes).bind(deletes)
            .execute(&mut **tx).await.map_err(storage)?;
        sqlx::query("UPDATE catalog_tables SET status='updating' WHERE project_id=$1 AND warehouse_id=$2 AND id=$3 AND status='active'")
            .bind(scope.project.as_str()).bind(scope.warehouse.as_str()).bind(id.as_str()).execute(&mut **tx).await.map_err(storage)?;
        Ok(())
    }
    async fn finish_update(&self, actor: &UserId, scope: &Scope, id: &TableId) -> Result<()> {
        let mut tx = self.scoped_transaction(actor, scope, true).await?;
        let row = sqlx::query("SELECT u.actor_id,u.operation,u.owner_id,u.namespace_id,u.writes::text,u.deletes::text,t.status FROM catalog_pending_updates u JOIN catalog_tables t ON t.project_id=u.project_id AND t.warehouse_id=u.warehouse_id AND t.id=u.table_id WHERE u.project_id=$1 AND u.warehouse_id=$2 AND u.table_id=$3 FOR UPDATE OF u,t")
            .bind(scope.project.as_str()).bind(scope.warehouse.as_str()).bind(id.as_str()).fetch_optional(&mut *tx).await.map_err(storage)?.ok_or(Error::Unavailable)?;
        let state = Lifecycle::from_storage(&row.try_get::<String, _>("status").map_err(storage)?)
            .ok_or(Error::Unavailable)?;
        if domain::transition(state, Event::Updated) != Some(Lifecycle::Active) {
            return Err(Error::Unavailable);
        }
        let writes: Vec<Tuple> =
            serde_json::from_str(&row.try_get::<String, _>("writes").map_err(storage)?)
                .map_err(|_| Error::Unavailable)?;
        let deletes: Vec<Tuple> =
            serde_json::from_str(&row.try_get::<String, _>("deletes").map_err(storage)?)
                .map_err(|_| Error::Unavailable)?;
        self.auth
            .apply(&writes, &deletes)
            .await
            .map_err(authorization)?;
        let owner: String = row.try_get("owner_id").map_err(storage)?;
        let namespace: String = row.try_get("namespace_id").map_err(storage)?;
        sqlx::query("UPDATE catalog_tables SET owner_id=$4,namespace_id=$5,status='active' WHERE project_id=$1 AND warehouse_id=$2 AND id=$3 AND status='updating'")
            .bind(scope.project.as_str()).bind(scope.warehouse.as_str()).bind(id.as_str()).bind(&owner).bind(&namespace).execute(&mut *tx).await.map_err(storage)?;
        sqlx::query("DELETE FROM catalog_pending_updates WHERE project_id=$1 AND warehouse_id=$2 AND table_id=$3")
            .bind(scope.project.as_str()).bind(scope.warehouse.as_str()).bind(id.as_str()).execute(&mut *tx).await.map_err(storage)?;
        let operation: String = row.try_get("operation").map_err(storage)?;
        tx.commit().await.map_err(storage)?;
        tracing::info!(event="table_relationship_replaced",operation,actor=%actor,project=%scope.project,object=%table_object(scope,id),owner,namespace,"catalog audit");
        Ok(())
    }
    pub(crate) async fn delete_table(
        &self,
        actor: &UserId,
        scope: &Scope,
        id: &TableId,
    ) -> Result<()> {
        let mut tx = self.scoped_transaction(actor, scope, true).await?;
        let row = sqlx::query("SELECT status,delete_actor,namespace_id FROM catalog_tables WHERE project_id=$1 AND warehouse_id=$2 AND id=$3 FOR UPDATE")
            .bind(scope.project.as_str()).bind(scope.warehouse.as_str()).bind(id.as_str()).fetch_optional(&mut *tx).await.map_err(storage)?.ok_or(Error::NotFound)?;
        let state = Lifecycle::from_storage(&row.try_get::<String, _>("status").map_err(storage)?)
            .ok_or(Error::Unavailable)?;
        let namespace: String = row.try_get("namespace_id").map_err(storage)?;
        let delete_actor: Option<String> = row.try_get("delete_actor").map_err(storage)?;
        let object = table_object(scope, id);
        if state == Lifecycle::Active {
            self.require(actor, TableAction::Drop.relation(), &object)
                .await?;
            if domain::transition(state, Event::BeginDeletion) != Some(Lifecycle::Deleting) {
                return Err(Error::Unavailable);
            }
            sqlx::query("UPDATE catalog_tables SET status='deleting',delete_actor=$4 WHERE project_id=$1 AND warehouse_id=$2 AND id=$3 AND status='active'")
                .bind(scope.project.as_str()).bind(scope.warehouse.as_str()).bind(id.as_str()).bind(actor.as_str()).execute(&mut *tx).await.map_err(storage)?;
        } else if delete_actor.as_deref() != Some(actor.as_str()) {
            return Err(Error::NotFound);
        }
        tx.commit().await.map_err(storage)?;
        if matches!(state, Lifecycle::Deleting | Lifecycle::Deleted) {
            // A recorded delete actor may retry cleanup or its final receipt,
            // but both must respect deny mode, backend loss and unresolved debt.
            // Normal active deletion already passed its authoritative Check.
            self.auth.ensure_available().await.map_err(authorization)?;
        }
        if state == Lifecycle::Deleted {
            return Ok(());
        }
        let deleting = if state == Lifecycle::Active {
            Lifecycle::Deleting
        } else {
            state
        };
        if domain::transition(deleting, Event::TuplesRemoved) != Some(Lifecycle::Deleted) {
            return Err(Error::NotFound);
        }
        self.auth
            .delete_object(&object.to_string())
            .await
            .map_err(authorization)?;
        self.auth
            .write(
                &[tuple(
                    object.to_string(),
                    "child",
                    format!("namespace:{namespace}"),
                )],
                false,
            )
            .await
            .map_err(authorization)?;
        sqlx::query("UPDATE catalog_tables SET status='deleted' WHERE project_id=$1 AND warehouse_id=$2 AND id=$3 AND status='deleting'")
            .bind(scope.project.as_str()).bind(scope.warehouse.as_str()).bind(id.as_str()).execute(&self.db).await.map_err(storage)?;
        tracing::info!(event="table_deleted",actor=%actor,project=%scope.project,object=%object,"catalog audit");
        Ok(())
    }
}
