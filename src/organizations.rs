//! Organization ownership, membership and explicit inherited task permissions.
//! Owner transfer is an application invariant; OpenFGA itself allows many owners.
use crate::application::{App, Member, Organization};
use crate::authorization::{self, Projection, lock_organization, require_user, stage, tuple};
use crate::domain;
use crate::use_cases::{Error, MAX_ENUMERATED_OBJECTS, Result};
use sqlx::Row;

impl App {
    pub(crate) async fn visible_organizations(&self, actor: &str) -> Result<Vec<Organization>> {
        for _ in 0..3 {
            let before = self.organization_snapshot().await?;
            let rows =
                sqlx::query("SELECT id,name,owner_id,authz_pending,authz_uncertain FROM organizations ORDER BY id LIMIT $1")
                    .bind(MAX_ENUMERATED_OBJECTS + 1)
                    .fetch_all(&self.db)
                    .await
                    .map_err(|_| Error::Unavailable)?;
            if rows.len() > MAX_ENUMERATED_OBJECTS as usize {
                return Err(Error::Unavailable);
            }
            let mut organizations = Vec::new();
            for row in rows {
                if row.get::<bool, _>("authz_pending") {
                    return Err(Error::Unavailable);
                }
                let id: String = row.get("id");
                let object = format!("organization:{id}");
                let recovery_required: bool = row.get("authz_uncertain");
                if self.allowed(actor, "can_view", &object).await? {
                    organizations.push(Organization {
                        id,
                        name: row.get("name"),
                        owner_id: row.get("owner_id"),
                        can_manage: !recovery_required
                            && self.allowed(actor, "owner", &object).await?,
                        can_create_task: !recovery_required
                            && self.allowed(actor, "can_create_task", &object).await?,
                        recovery_required,
                    });
                }
            }
            if before == self.organization_snapshot().await? {
                return Ok(organizations);
            }
        }
        Err(Error::Unavailable)
    }

    pub(crate) async fn organization_members(&self, actor: &str, id: &str) -> Result<Vec<Member>> {
        if !domain::valid_id(id) {
            return Err(Error::Invalid);
        }
        for _ in 0..3 {
            let row = sqlx::query("SELECT owner_id,authz_pending,authorization_version FROM organizations WHERE id=$1")
                .bind(id).fetch_optional(&self.db).await.map_err(|_| Error::Unavailable)?.ok_or(Error::NotFound)?;
            if row.get::<bool, _>("authz_pending") {
                return Err(Error::Unavailable);
            }
            let allowed = self
                .allowed(actor, "can_view", &format!("organization:{id}"))
                .await?;
            let owner: String = row.get("owner_id");
            let version: i64 = row.get("authorization_version");
            let rows = if allowed {
                sqlx::query("SELECT m.user_id,u.display_name,m.role FROM organization_members m JOIN users u ON u.id=m.user_id WHERE m.organization_id=$1 ORDER BY m.user_id LIMIT $2")
                    .bind(id).bind(MAX_ENUMERATED_OBJECTS + 1).fetch_all(&self.db).await.map_err(|_| Error::Unavailable)?
            } else {
                Vec::new()
            };
            if !self.stable_organization(id, version).await? {
                continue;
            }
            if !allowed {
                return Err(Error::Forbidden);
            }
            if rows.len() > MAX_ENUMERATED_OBJECTS as usize {
                return Err(Error::Unavailable);
            }
            return Ok(rows
                .into_iter()
                .map(|row| {
                    let user_id: String = row.get("user_id");
                    Member {
                        is_owner: user_id == owner,
                        user_id,
                        display_name: row.get("display_name"),
                        role: row.get("role"),
                    }
                })
                .collect());
        }
        Err(Error::Unavailable)
    }

    pub(crate) async fn transfer_ownership(
        &self,
        actor: &str,
        id: &str,
        expected: &str,
        next: &str,
    ) -> Result<()> {
        if !domain::valid_id(id) || !domain::valid_id(expected) || !domain::valid_id(next) {
            return Err(Error::Invalid);
        }
        let mut tx = self.db.begin().await.map_err(|_| Error::Unavailable)?;
        let row = lock_organization(&mut tx, id).await?;
        self.require(actor, "owner", &format!("organization:{id}"))
            .await?;
        let owner: String = row.get("owner_id");
        // Check both sources, including optimistic concurrency. No admin or
        // generic relation endpoint can grant/revoke ownership.
        if actor != owner {
            return Err(Error::Forbidden);
        }
        if owner != expected || owner == next {
            return Err(Error::Conflict);
        }
        require_user(&mut tx, next).await?;
        let member: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM organization_members WHERE organization_id=$1 AND user_id=$2)")
            .bind(id).bind(next).fetch_one(&mut *tx).await.map_err(|_| Error::Unavailable)?;
        if !member {
            return Err(Error::Invalid);
        }
        sqlx::query("UPDATE organizations SET owner_id=$2 WHERE id=$1")
            .bind(id)
            .bind(next)
            .execute(&mut *tx)
            .await
            .map_err(|_| Error::Unavailable)?;
        let object = format!("organization:{id}");
        stage(
            &mut tx,
            id,
            Projection {
                writes: vec![tuple(&format!("user:{next}"), "owner", &object)],
                deletes: vec![tuple(&format!("user:{owner}"), "owner", &object)],
                ..Default::default()
            },
        )
        .await?;
        tx.commit().await.map_err(|_| Error::Unavailable)?;
        authorization::apply(&self.db, &self.authz_coordinator, self.auth.as_ref(), id).await
    }

    pub(crate) async fn set_member(
        &self,
        actor: &str,
        id: &str,
        subject: &str,
        role: Option<&str>,
    ) -> Result<()> {
        if !domain::valid_id(id)
            || !domain::valid_id(subject)
            || role.is_some_and(|role| !matches!(role, "member" | "admin"))
        {
            return Err(Error::Invalid);
        }
        let mut tx = self.db.begin().await.map_err(|_| Error::Unavailable)?;
        let row = lock_organization(&mut tx, id).await?;
        self.require(actor, "owner", &format!("organization:{id}"))
            .await?;
        let owner: String = row.get("owner_id");
        if owner != actor {
            return Err(Error::Forbidden);
        }
        if role.is_none() && owner == subject {
            return Err(Error::Conflict);
        }
        require_user(&mut tx, subject).await?;
        let previous: Option<String> = sqlx::query_scalar(
            "SELECT role FROM organization_members WHERE organization_id=$1 AND user_id=$2",
        )
        .bind(id)
        .bind(subject)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| Error::Unavailable)?;
        if previous.as_deref() == role {
            return Ok(());
        }
        if let Some(role) = role {
            sqlx::query("INSERT INTO organization_members (organization_id,user_id,role) VALUES ($1,$2,$3) ON CONFLICT (organization_id,user_id) DO UPDATE SET role=EXCLUDED.role")
                .bind(id).bind(subject).bind(role).execute(&mut *tx).await.map_err(|_| Error::Unavailable)?;
        } else {
            sqlx::query("DELETE FROM organization_members WHERE organization_id=$1 AND user_id=$2")
                .bind(id)
                .bind(subject)
                .execute(&mut *tx)
                .await
                .map_err(|_| Error::Unavailable)?;
        }
        let object = format!("organization:{id}");
        let user = format!("user:{subject}");
        stage(
            &mut tx,
            id,
            Projection {
                writes: role.map(|r| tuple(&user, r, &object)).into_iter().collect(),
                deletes: previous
                    .map(|r| tuple(&user, &r, &object))
                    .into_iter()
                    .collect(),
                ..Default::default()
            },
        )
        .await?;
        tx.commit().await.map_err(|_| Error::Unavailable)?;
        authorization::apply(&self.db, &self.authz_coordinator, self.auth.as_ref(), id).await
    }

    pub(crate) async fn set_organization_permission(
        &self,
        actor: &str,
        id: &str,
        relation: &str,
        subject: &str,
        add: bool,
    ) -> Result<()> {
        if !domain::valid_id(id)
            || !domain::valid_id(subject)
            || !domain::valid_organization_permission(relation)
        {
            return Err(Error::Invalid);
        }
        let mut tx = self.db.begin().await.map_err(|_| Error::Unavailable)?;
        let row = lock_organization(&mut tx, id).await?;
        self.require(actor, "owner", &format!("organization:{id}"))
            .await?;
        if row.get::<String, _>("owner_id") != actor {
            return Err(Error::Forbidden);
        }
        require_user(&mut tx, subject).await?;
        let query = if add {
            "INSERT INTO organization_permissions (organization_id,user_id,relation) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING"
        } else {
            "DELETE FROM organization_permissions WHERE organization_id=$1 AND user_id=$2 AND relation=$3"
        };
        sqlx::query(query)
            .bind(id)
            .bind(subject)
            .bind(relation)
            .execute(&mut *tx)
            .await
            .map_err(|_| Error::Unavailable)?;
        let relation_tuple = tuple(
            &format!("user:{subject}"),
            relation,
            &format!("organization:{id}"),
        );
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
        stage(&mut tx, id, projection).await?;
        tx.commit().await.map_err(|_| Error::Unavailable)?;
        authorization::apply(&self.db, &self.authz_coordinator, self.auth.as_ref(), id).await
    }
}
