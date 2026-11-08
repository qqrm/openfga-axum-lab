//! Task use cases. PostgreSQL row locks serialize mutations; reads validate a
//! durable organization revision around remote checks without blocking revokes.
use crate::application::{App, CreateTask, Relation, Task, TaskInput};
use crate::authorization::{
    self, Projection, creation_tuples, lock_organization, lock_organization_state, require_user,
    stage, tuple,
};
use crate::domain::{self, Event, Lifecycle};
use sha2::{Digest, Sha256};
use sqlx::Row;

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
pub(crate) type Result<T> = std::result::Result<T, Error>;

impl App {
    pub(crate) async fn visible_tasks(&self, user: &str) -> Result<Vec<Task>> {
        for _ in 0..3 {
            let before = self.organization_snapshot().await?;
            let objects = self
                .auth
                .list_objects(user, "can_view", "task")
                .await
                .map_err(|_| Error::Unavailable)?;
            let visible_ids: Vec<&str> = objects
                .iter()
                .map(|object| {
                    object
                        .strip_prefix("task:")
                        .filter(|id| domain::valid_id(id))
                        .ok_or(Error::Unavailable)
                })
                .collect::<Result<_>>()?;
            let ids: Vec<String> = sqlx::query_scalar(
                "SELECT id FROM tasks WHERE status='active' AND id = ANY($1) ORDER BY id",
            )
            .bind(&visible_ids)
            .fetch_all(&self.db)
            .await
            .map_err(|_| Error::Unavailable)?;
            let mut tasks = Vec::new();
            for id in ids {
                match self.get_task(user, &id).await {
                    Ok(task) => tasks.push(task),
                    Err(Error::Forbidden | Error::NotFound) => {}
                    Err(error) => return Err(error),
                }
            }
            if before == self.organization_snapshot().await? {
                return Ok(tasks);
            }
        }
        Err(Error::Unavailable)
    }

    pub(crate) async fn get_task(&self, user: &str, id: &str) -> Result<Task> {
        if !domain::valid_id(id) {
            return Err(Error::Invalid);
        }
        for _ in 0..3 {
            let row = sqlx::query("SELECT t.*,o.authorization_version,o.authz_pending,o.authz_uncertain FROM tasks t JOIN organizations o ON o.id=t.organization_id WHERE t.id=$1")
                .bind(id).fetch_optional(&self.db).await.map_err(|_| Error::Unavailable)?
                .ok_or(Error::NotFound)?;
            if row.get::<bool, _>("authz_pending") {
                return Err(Error::Unavailable);
            }
            let state = Lifecycle::from_storage(row.get("status")).ok_or(Error::Unavailable)?;
            if !domain::is_visible(state) {
                return Err(Error::NotFound);
            }
            let organization: String = row.get("organization_id");
            let version: i64 = row.get("authorization_version");
            let object = format!("task:{id}");
            let view = self.allowed(user, "can_view", &object).await?;
            let recovery_required: bool = row.get("authz_uncertain");
            let mut task = Task {
                id: id.into(),
                organization_id: organization.clone(),
                title: row.get("title"),
                description: row.get("description"),
                status: row.get("task_status"),
                assigner_id: row.get("assigner_id"),
                can_edit: false,
                can_delete: false,
                recovery_required,
                relations: None,
            };
            if view {
                task.can_edit =
                    !recovery_required && self.allowed(user, "can_edit", &object).await?;
                task.can_delete =
                    !recovery_required && self.allowed(user, "can_delete", &object).await?;
                if task.can_delete {
                    let rows = sqlx::query("SELECT relation,user_id FROM task_relations WHERE task_id=$1 ORDER BY relation,user_id")
                        .bind(id).fetch_all(&self.db).await.map_err(|_| Error::Unavailable)?;
                    let mut relations = vec![Relation {
                        relation: "assigner".into(),
                        user_id: task.assigner_id.clone(),
                    }];
                    relations.extend(rows.into_iter().map(|row| Relation {
                        relation: row.get("relation"),
                        user_id: row.get("user_id"),
                    }));
                    task.relations = Some(relations);
                }
            }
            if !self.stable_organization(&organization, version).await? {
                continue;
            }
            return if view {
                Ok(task)
            } else {
                Err(Error::Forbidden)
            };
        }
        Err(Error::Unavailable)
    }

    pub(crate) async fn create_task(
        &self,
        actor: &str,
        key: &str,
        input: &CreateTask,
    ) -> Result<String> {
        if !domain::valid_id(&input.organization_id) {
            return Err(Error::Invalid);
        }
        let hash = Sha256::digest(serde_json::to_vec(input).map_err(|_| Error::Invalid)?).to_vec();
        let mut tx = self.db.begin().await.map_err(|_| Error::Unavailable)?;
        let organization = lock_organization_state(&mut tx, &input.organization_id).await?;
        if organization.get::<bool, _>("authz_uncertain") {
            return Err(Error::Unavailable);
        }
        if organization.get::<bool, _>("authz_pending") {
            // Only the exact, already-authorized durable creation may resume
            // through a fence. No fresh request is authorized against mixed state.
            let pending = sqlx::query("SELECT t.id,t.request_hash FROM tasks t JOIN authorization_jobs j ON j.organization_id=t.organization_id AND j.activate_task_id=t.id WHERE t.organization_id=$1 AND t.assigner_id=$2 AND t.idempotency_key=$3 AND t.status='pending'")
                .bind(&input.organization_id).bind(actor).bind(key).fetch_optional(&mut *tx).await.map_err(|_| Error::Unavailable)?
                .ok_or(Error::Unavailable)?;
            if pending.get::<Vec<u8>, _>("request_hash") != hash {
                return Err(Error::Conflict);
            }
            let id: String = pending.get("id");
            tx.rollback().await.map_err(|_| Error::Unavailable)?;
            authorization::apply(
                &self.db,
                &self.authz_coordinator,
                self.auth.as_ref(),
                &input.organization_id,
            )
            .await?;
            let status: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id=$1")
                .bind(&id)
                .fetch_one(&self.db)
                .await
                .map_err(|_| Error::Unavailable)?;
            return if status == "active" {
                Ok(id)
            } else {
                Err(Error::Gone)
            };
        }
        self.require(
            actor,
            "can_create_task",
            &format!("organization:{}", input.organization_id),
        )
        .await?;
        if !domain::valid_task(&input.title, &input.description, &input.status)
            || !domain::valid_idempotency_key(key)
        {
            return Err(Error::Invalid);
        }
        if let Some(row) = sqlx::query("SELECT id,request_hash,status FROM tasks WHERE organization_id=$1 AND assigner_id=$2 AND idempotency_key=$3 FOR UPDATE")
            .bind(&input.organization_id).bind(actor).bind(key).fetch_optional(&mut *tx).await.map_err(|_| Error::Unavailable)? {
            if row.get::<Vec<u8>, _>("request_hash") != hash { return Err(Error::Conflict); }
            return match row.get::<&str, _>("status") {
                "active" => Ok(row.get("id")),
                "pending" => Err(Error::Unavailable),
                _ => Err(Error::Gone),
            };
        }
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO tasks (id,organization_id,title,description,task_status,assigner_id,status,idempotency_key,request_hash) VALUES ($1,$2,$3,$4,$5,$6,'pending',$7,$8)")
            .bind(&id).bind(&input.organization_id).bind(&input.title).bind(&input.description)
            .bind(&input.status).bind(actor).bind(key).bind(hash).execute(&mut *tx).await.map_err(|_| Error::Unavailable)?;
        stage(
            &mut tx,
            &input.organization_id,
            Projection {
                writes: creation_tuples(&id, &input.organization_id, actor),
                activate_task_id: Some(id.clone()),
                ..Default::default()
            },
        )
        .await?;
        tx.commit().await.map_err(|_| Error::Unavailable)?;
        authorization::apply(
            &self.db,
            &self.authz_coordinator,
            self.auth.as_ref(),
            &input.organization_id,
        )
        .await?;
        Ok(id)
    }

    /// Parent IDs are immutable. All writers take organization then task locks
    /// so an inherited role revocation cannot interleave after authorization.
    async fn task_organization(&self, id: &str) -> Result<String> {
        if !domain::valid_id(id) {
            return Err(Error::Invalid);
        }
        sqlx::query_scalar("SELECT organization_id FROM tasks WHERE id=$1")
            .bind(id)
            .fetch_optional(&self.db)
            .await
            .map_err(|_| Error::Unavailable)?
            .ok_or(Error::NotFound)
    }

    pub(crate) async fn edit_task(&self, actor: &str, id: &str, input: &TaskInput) -> Result<()> {
        let organization = self.task_organization(id).await?;
        let mut tx = self.db.begin().await.map_err(|_| Error::Unavailable)?;
        lock_organization(&mut tx, &organization).await?;
        let status: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| Error::Unavailable)?;
        if status != "active" {
            return Err(Error::NotFound);
        }
        self.require(actor, "can_edit", &format!("task:{id}"))
            .await?;
        if !domain::valid_task(&input.title, &input.description, &input.status) {
            return Err(Error::Invalid);
        }
        sqlx::query("UPDATE tasks SET title=$2,description=$3,task_status=$4 WHERE id=$1 AND status='active'")
            .bind(id).bind(&input.title).bind(&input.description).bind(&input.status)
            .execute(&mut *tx).await.map_err(|_| Error::Unavailable)?;
        tx.commit().await.map_err(|_| Error::Unavailable)
    }

    pub(crate) async fn delete_task(&self, actor: &str, id: &str) -> Result<()> {
        let organization = self.task_organization(id).await?;
        let mut tx = self.db.begin().await.map_err(|_| Error::Unavailable)?;
        let org = lock_organization_state(&mut tx, &organization).await?;
        let row = sqlx::query("SELECT status,delete_actor FROM tasks WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| Error::Unavailable)?;
        let state = Lifecycle::from_storage(row.get("status")).ok_or(Error::Unavailable)?;
        if org.get::<bool, _>("authz_uncertain") {
            return Err(Error::Unavailable);
        }
        if org.get::<bool, _>("authz_pending") {
            let pending: Option<String> = sqlx::query_scalar(
                "SELECT delete_task_id FROM authorization_jobs WHERE organization_id=$1",
            )
            .bind(&organization)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| Error::Unavailable)?;
            if state != Lifecycle::Deleting
                || pending.as_deref() != Some(id)
                || row.get::<Option<String>, _>("delete_actor").as_deref() != Some(actor)
            {
                return Err(Error::Unavailable);
            }
            tx.rollback().await.map_err(|_| Error::Unavailable)?;
            return authorization::apply(
                &self.db,
                &self.authz_coordinator,
                self.auth.as_ref(),
                &organization,
            )
            .await;
        }
        if state == Lifecycle::Deleted
            && row.get::<Option<String>, _>("delete_actor").as_deref() == Some(actor)
        {
            return Ok(());
        }
        if domain::transition(state, Event::BeginDeletion) != Some(Lifecycle::Deleting) {
            return Err(Error::NotFound);
        }
        self.require(actor, "can_delete", &format!("task:{id}"))
            .await?;
        sqlx::query("UPDATE tasks SET status='deleting',delete_actor=$2 WHERE id=$1")
            .bind(id)
            .bind(actor)
            .execute(&mut *tx)
            .await
            .map_err(|_| Error::Unavailable)?;
        stage(
            &mut tx,
            &organization,
            Projection {
                delete_object: Some(format!("task:{id}")),
                delete_task_id: Some(id.into()),
                ..Default::default()
            },
        )
        .await?;
        tx.commit().await.map_err(|_| Error::Unavailable)?;
        authorization::apply(
            &self.db,
            &self.authz_coordinator,
            self.auth.as_ref(),
            &organization,
        )
        .await
    }

    pub(crate) async fn set_task_relation(
        &self,
        actor: &str,
        id: &str,
        relation: &str,
        subject: &str,
        add: bool,
    ) -> Result<()> {
        if !domain::valid_id(subject) || !domain::valid_task_relation(relation) {
            return Err(Error::Invalid);
        }
        let organization = self.task_organization(id).await?;
        let mut tx = self.db.begin().await.map_err(|_| Error::Unavailable)?;
        lock_organization(&mut tx, &organization).await?;
        let status: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| Error::Unavailable)?;
        if status != "active" {
            return Err(Error::NotFound);
        }
        // Local API policy: assigners and inherited deleters may manage grants;
        // assignees and direct editors may not delegate their access.
        self.require(actor, "can_delete", &format!("task:{id}"))
            .await?;
        require_user(&mut tx, subject).await?;
        let query = if add {
            "INSERT INTO task_relations (task_id,user_id,relation) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING"
        } else {
            "DELETE FROM task_relations WHERE task_id=$1 AND user_id=$2 AND relation=$3"
        };
        sqlx::query(query)
            .bind(id)
            .bind(subject)
            .bind(relation)
            .execute(&mut *tx)
            .await
            .map_err(|_| Error::Unavailable)?;
        let relation_tuple = tuple(&format!("user:{subject}"), relation, &format!("task:{id}"));
        let projection = if add {
            Projection {
                writes: vec![relation_tuple],
                ..Default::default()
            }
        } else {
            Projection {
                deletes: vec![relation_tuple],
                ..Default::default()
            }
        };
        stage(&mut tx, &organization, projection).await?;
        tx.commit().await.map_err(|_| Error::Unavailable)?;
        authorization::apply(
            &self.db,
            &self.authz_coordinator,
            self.auth.as_ref(),
            &organization,
        )
        .await
    }
}
