//! Durable PostgreSQL -> OpenFGA projection. A pending organization is fenced
//! from public access until the complete remote write has been acknowledged.
use crate::application::{App, Authorizer, Tuple};
use crate::use_cases::{Error, Result};
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};

#[derive(Default)]
pub(crate) struct Projection {
    pub(crate) writes: Vec<Tuple>,
    pub(crate) deletes: Vec<Tuple>,
    pub(crate) delete_object: Option<String>,
    pub(crate) activate_task_id: Option<String>,
    pub(crate) delete_task_id: Option<String>,
}

pub(crate) fn tuple(user: &str, relation: &str, object: &str) -> Tuple {
    Tuple {
        user: user.into(),
        relation: relation.into(),
        object: object.into(),
    }
}

pub(crate) fn creation_tuples(id: &str, organization: &str, assigner: &str) -> Vec<Tuple> {
    let object = format!("task:{id}");
    vec![
        tuple(&format!("user:{assigner}"), "assigner", &object),
        tuple(
            &format!("organization:{organization}"),
            "parent_context",
            &object,
        ),
    ]
}

pub(crate) async fn lock_organization(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
) -> Result<PgRow> {
    let row = lock_organization_state(tx, id).await?;
    if row.get::<bool, _>("authz_pending") || row.get::<bool, _>("authz_uncertain") {
        return Err(Error::Unavailable);
    }
    Ok(row)
}

pub(crate) async fn lock_organization_state(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
) -> Result<PgRow> {
    let row = sqlx::query(
        "SELECT owner_id,authz_pending,authz_uncertain FROM organizations WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| Error::Unavailable)?
    .ok_or(Error::NotFound)?;
    Ok(row)
}

pub(crate) async fn require_user(tx: &mut Transaction<'_, Postgres>, id: &str) -> Result<()> {
    if sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM users WHERE id=$1)")
        .bind(id)
        .fetch_one(&mut **tx)
        .await
        .map_err(|_| Error::Unavailable)?
    {
        Ok(())
    } else {
        Err(Error::NotFound)
    }
}

/// Caller holds the organization row lock and stages desired domain rows in
/// this transaction. A committed journal can be replayed after any cancellation.
pub(crate) async fn stage(
    tx: &mut Transaction<'_, Postgres>,
    organization: &str,
    projection: Projection,
) -> Result<()> {
    sqlx::query("INSERT INTO authorization_jobs (organization_id,writes,deletes,delete_object,activate_task_id,delete_task_id) VALUES ($1,$2,$3,$4,$5,$6)")
        .bind(organization)
        .bind(serde_json::to_string(&projection.writes).map_err(|_| Error::Unavailable)?)
        .bind(serde_json::to_string(&projection.deletes).map_err(|_| Error::Unavailable)?)
        .bind(projection.delete_object).bind(projection.activate_task_id).bind(projection.delete_task_id)
        .execute(&mut **tx).await.map_err(|_| Error::Unavailable)?;
    sqlx::query("UPDATE organizations SET authz_pending=true,authorization_version=authorization_version+1 WHERE id=$1")
        .bind(organization).execute(&mut **tx).await.map_err(|_| Error::Unavailable)?;
    Ok(())
}

pub(crate) async fn apply(
    db: &PgPool,
    coordinator: &PgPool,
    auth: &dyn Authorizer,
    organization: &str,
) -> Result<()> {
    let mut tx = db.begin().await.map_err(|_| Error::Unavailable)?;
    let pending: bool =
        sqlx::query_scalar("SELECT authz_pending FROM organizations WHERE id=$1 FOR UPDATE")
            .bind(organization)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| Error::Unavailable)?;
    if !pending {
        return Ok(());
    } // Another replica already acknowledged it.
    let job = sqlx::query("SELECT attempted,writes,deletes,delete_object,activate_task_id,delete_task_id FROM authorization_jobs WHERE organization_id=$1")
        .bind(organization).fetch_optional(&mut *tx).await.map_err(|_| Error::Unavailable)?
        .ok_or(Error::Unavailable)?;
    let writes: Vec<Tuple> =
        serde_json::from_str(job.get::<&str, _>("writes")).map_err(|_| Error::Unavailable)?;
    let deletes: Vec<Tuple> =
        serde_json::from_str(job.get::<&str, _>("deletes")).map_err(|_| Error::Unavailable)?;
    let replayed = job.get::<bool, _>("attempted");
    // Commit before the first remote poll, outside the transaction holding the
    // organization lock. A separate pool prevents pool-exhaustion deadlocks.
    // No retry/ack proves that an older timed-out request has stopped running.
    let marked =
        sqlx::query("UPDATE authorization_jobs SET attempted=true WHERE organization_id=$1")
            .bind(organization)
            .execute(coordinator)
            .await
            .map_err(|_| Error::Unavailable)?
            .rows_affected();
    if marked != 1 {
        return Err(Error::Unavailable);
    }
    if let Some(object) = job.get::<Option<String>, _>("delete_object") {
        auth.delete_object(&object)
            .await
            .map_err(|_| Error::Unavailable)?;
    }
    auth.change(&writes, &deletes)
        .await
        .map_err(|_| Error::Unavailable)?;
    if let Some(id) = job.get::<Option<String>, _>("activate_task_id") {
        let changed = sqlx::query("UPDATE tasks SET status='active' WHERE id=$1 AND organization_id=$2 AND status='pending'")
            .bind(&id).bind(organization).execute(&mut *tx).await.map_err(|_| Error::Unavailable)?.rows_affected();
        if changed != 1 {
            return Err(Error::Unavailable);
        }
    }
    if let Some(id) = job.get::<Option<String>, _>("delete_task_id") {
        let changed = sqlx::query("UPDATE tasks SET status='deleted' WHERE id=$1 AND organization_id=$2 AND status='deleting'")
            .bind(&id).bind(organization).execute(&mut *tx).await.map_err(|_| Error::Unavailable)?.rows_affected();
        if changed != 1 {
            return Err(Error::Unavailable);
        }
        sqlx::query("DELETE FROM task_relations WHERE task_id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|_| Error::Unavailable)?;
    }
    sqlx::query("DELETE FROM authorization_jobs WHERE organization_id=$1")
        .bind(organization)
        .execute(&mut *tx)
        .await
        .map_err(|_| Error::Unavailable)?;
    sqlx::query("UPDATE organizations SET authz_pending=false,authz_uncertain=authz_uncertain OR $2,authorization_version=authorization_version+1 WHERE id=$1")
        .bind(organization).bind(replayed).execute(&mut *tx).await.map_err(|_| Error::Unavailable)?;
    tx.commit().await.map_err(|_| Error::Unavailable)
}

pub(crate) async fn recover(
    db: &PgPool,
    coordinator: &PgPool,
    auth: &dyn Authorizer,
    after_backend_drain: bool,
) -> Result<()> {
    let organizations: Vec<String> =
        sqlx::query_scalar("SELECT id FROM organizations WHERE authz_pending ORDER BY id")
            .fetch_all(db)
            .await
            .map_err(|_| Error::Unavailable)?;
    for id in organizations {
        apply(db, coordinator, auth, &id).await?;
    }
    // Also recover durable lifecycle rows from the previous recovery protocol.
    let tasks: Vec<(String, String)> = sqlx::query_as("SELECT id,organization_id FROM tasks WHERE status IN ('pending','deleting') ORDER BY organization_id,id")
        .fetch_all(db).await.map_err(|_| Error::Unavailable)?;
    for (id, organization) in tasks {
        let mut tx = db.begin().await.map_err(|_| Error::Unavailable)?;
        if after_backend_drain {
            let row = lock_organization_state(&mut tx, &organization).await?;
            if row.get::<bool, _>("authz_pending") {
                return Err(Error::Unavailable);
            }
        } else {
            lock_organization(&mut tx, &organization).await?;
        }
        let row = sqlx::query("SELECT assigner_id,status FROM tasks WHERE id=$1 FOR UPDATE")
            .bind(&id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| Error::Unavailable)?;
        let projection = match row.get::<&str, _>("status") {
            "pending" => Projection {
                writes: creation_tuples(&id, &organization, row.get("assigner_id")),
                activate_task_id: Some(id),
                ..Default::default()
            },
            "deleting" => Projection {
                delete_object: Some(format!("task:{id}")),
                delete_task_id: Some(id),
                ..Default::default()
            },
            _ => continue,
        };
        stage(&mut tx, &organization, projection).await?;
        // A lifecycle row without a journal has no durable proof that its
        // earlier remote operation was never attempted. Recover conservatively.
        sqlx::query("UPDATE authorization_jobs SET attempted=true WHERE organization_id=$1")
            .bind(&organization)
            .execute(&mut *tx)
            .await
            .map_err(|_| Error::Unavailable)?;
        tx.commit().await.map_err(|_| Error::Unavailable)?;
        apply(db, coordinator, auth, &organization).await?;
    }
    if after_backend_drain {
        // This one-shot operational mode requires all writers stopped and all
        // backend/proxy requests drained. It is never enabled by normal startup.
        let mut tx = db.begin().await.map_err(|_| Error::Unavailable)?;
        sqlx::query("SELECT id FROM organizations ORDER BY id FOR UPDATE")
            .fetch_all(&mut *tx)
            .await
            .map_err(|_| Error::Unavailable)?;
        let pending: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM authorization_jobs) OR EXISTS(SELECT 1 FROM organizations WHERE authz_pending)")
            .fetch_one(&mut *tx).await.map_err(|_| Error::Unavailable)?;
        if pending {
            return Err(Error::Unavailable);
        }
        sqlx::query("UPDATE organizations SET authz_uncertain=false,authorization_version=authorization_version+1 WHERE authz_uncertain")
            .execute(&mut *tx).await.map_err(|_| Error::Unavailable)?;
        sqlx::query(
            "UPDATE authz_cache_state SET pending=0,generation=generation+1 WHERE singleton",
        )
        .execute(&mut *tx)
        .await
        .map_err(|_| Error::Unavailable)?;
        tx.commit().await.map_err(|_| Error::Unavailable)?;
    }
    Ok(())
}

impl App {
    pub(crate) async fn require(&self, user: &str, relation: &str, object: &str) -> Result<()> {
        if self.allowed(user, relation, object).await? {
            Ok(())
        } else {
            Err(Error::Forbidden)
        }
    }
    pub(crate) async fn allowed(&self, user: &str, relation: &str, object: &str) -> Result<bool> {
        self.auth
            .check(user, relation, object)
            .await
            .map_err(|_| Error::Unavailable)
    }
    pub(crate) async fn organization_snapshot(&self) -> Result<Vec<(String, i64, bool)>> {
        sqlx::query_as(
            "SELECT id,authorization_version,authz_pending FROM organizations ORDER BY id",
        )
        .fetch_all(&self.db)
        .await
        .map_err(|_| Error::Unavailable)
    }
    pub(crate) async fn stable_organization(&self, id: &str, version: i64) -> Result<bool> {
        let (current, pending): (i64, bool) = sqlx::query_as(
            "SELECT authorization_version,authz_pending FROM organizations WHERE id=$1",
        )
        .bind(id)
        .fetch_one(&self.db)
        .await
        .map_err(|_| Error::Unavailable)?;
        if pending {
            return Err(Error::Unavailable);
        }
        Ok(current == version)
    }
}
