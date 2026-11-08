//! Application use cases. HTTP conversion stays in the delivery adapter.
//! PostgreSQL is deliberately a concrete dependency here: transactional row
//! locks are part of the consistency contract rather than a generic CRUD port.
use crate::application::{App, Pet, PetInput, Tuple};
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
type Result<T> = std::result::Result<T, Error>;

fn pet_from_row(row: sqlx::postgres::PgRow) -> Pet {
    Pet {
        id: row.get("id"),
        name: row.get("name"),
        species: row.get("species"),
        biography: row.get("biography"),
    }
}

impl App {
    pub(crate) async fn authorize_shelter_creation(&self, user: &str) -> Result<()> {
        if self
            .auth
            .check(user, "can_create", "shelter:central")
            .await
            .map_err(|_| Error::Unavailable)?
        {
            Ok(())
        } else {
            Err(Error::Forbidden)
        }
    }
    pub(crate) async fn authorize_pet(
        &self,
        user: &str,
        id: &str,
        relation: &str,
        deletion_retry: bool,
    ) -> Result<()> {
        if !domain::valid_id(id) {
            return Err(Error::Invalid);
        }
        let row = sqlx::query("SELECT status,delete_actor FROM pets WHERE id=$1")
            .bind(id)
            .fetch_optional(&self.db)
            .await
            .map_err(|_| Error::Unavailable)?
            .ok_or(Error::NotFound)?;
        let status: String = row.get("status");
        let actor: Option<String> = row.get("delete_actor");
        let state = Lifecycle::from_storage(&status).ok_or(Error::Unavailable)?;
        if !domain::is_visible(state) {
            if deletion_retry
                && matches!(state, Lifecycle::Deleting | Lifecycle::Deleted)
                && actor.as_deref() == Some(user)
            {
                return Ok(());
            }
            return Err(Error::NotFound);
        }
        if self
            .auth
            .check(user, relation, &format!("pet:{id}"))
            .await
            .map_err(|_| Error::Unavailable)?
        {
            Ok(())
        } else {
            Err(Error::Forbidden)
        }
    }
    pub(crate) async fn visible_pets(&self, user: &str) -> Result<Vec<Pet>> {
        let rows = sqlx::query(
            "SELECT id,name,species,biography FROM pets WHERE status='active' ORDER BY id",
        )
        .fetch_all(&self.db)
        .await
        .map_err(|_| Error::Unavailable)?;
        let mut pets = Vec::new();
        for row in rows {
            let pet = pet_from_row(row);
            if self
                .auth
                .check(user, "can_view", &format!("pet:{}", pet.id))
                .await
                .map_err(|_| Error::Unavailable)?
            {
                pets.push(pet);
            }
        }
        Ok(pets)
    }
    pub(crate) async fn get_pet(&self, id: &str) -> Result<Pet> {
        sqlx::query("SELECT id,name,species,biography FROM pets WHERE id=$1 AND status='active'")
            .bind(id)
            .fetch_optional(&self.db)
            .await
            .map_err(|_| Error::Unavailable)?
            .map(pet_from_row)
            .ok_or(Error::NotFound)
    }
    pub(crate) async fn create_pet(
        &self,
        owner: &str,
        key: &str,
        input: &PetInput,
    ) -> Result<String> {
        if !domain::valid_pet(&input.name, &input.species, &input.biography)
            || !(8..=128).contains(&key.len())
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(Error::Invalid);
        }
        let hash = Sha256::digest(serde_json::to_vec(input).map_err(|_| Error::Invalid)?).to_vec();
        let candidate = uuid::Uuid::new_v4().to_string();
        let inserted: Option<String> = sqlx::query_scalar("INSERT INTO pets (id,name,species,biography,owner_id,status,idempotency_key,request_hash) VALUES ($1,$2,$3,$4,$5,'pending',$6,$7) ON CONFLICT (owner_id,idempotency_key) WHERE idempotency_key IS NOT NULL DO NOTHING RETURNING id")
            .bind(&candidate).bind(&input.name).bind(&input.species).bind(&input.biography).bind(owner)
            .bind(key).bind(&hash).fetch_optional(&self.db).await.map_err(|_| Error::Unavailable)?;
        let id = match inserted {
            Some(id) => id,
            None => {
                let row = sqlx::query(
                    "SELECT id,request_hash FROM pets WHERE owner_id=$1 AND idempotency_key=$2",
                )
                .bind(owner)
                .bind(key)
                .fetch_optional(&self.db)
                .await
                .map_err(|_| Error::Unavailable)?
                .ok_or(Error::Unavailable)?;
                let stored: Vec<u8> = row.get("request_hash");
                if stored != hash {
                    return Err(Error::Conflict);
                }
                row.get("id")
            }
        };
        let mut tx = self.db.begin().await.map_err(|_| Error::Unavailable)?;
        let status: Option<String> =
            sqlx::query_scalar("SELECT status FROM pets WHERE id=$1 FOR UPDATE")
                .bind(&id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|_| Error::Unavailable)?;
        let state = status
            .as_deref()
            .and_then(Lifecycle::from_storage)
            .ok_or(Error::Unavailable)?;
        if state == Lifecycle::Active {
            return Ok(id);
        }
        if domain::transition(state, Event::Provisioned) != Some(Lifecycle::Active) {
            return Err(Error::Gone);
        }
        let object = format!("pet:{id}");
        let tuples = [
            Tuple {
                user: format!("user:{owner}"),
                relation: "owner".into(),
                object: object.clone(),
            },
            Tuple {
                user: "shelter:central".into(),
                relation: "shelter".into(),
                object,
            },
        ];
        self.auth
            .write(&tuples, true)
            .await
            .map_err(|_| Error::Unavailable)?;
        sqlx::query("UPDATE pets SET status='active' WHERE id=$1 AND status='pending'")
            .bind(&id)
            .execute(&mut *tx)
            .await
            .map_err(|_| Error::Unavailable)?;
        tx.commit().await.map_err(|_| Error::Unavailable)?;
        Ok(id)
    }
    pub(crate) async fn edit_pet(&self, actor: &str, id: &str, input: &PetInput) -> Result<()> {
        if !domain::valid_pet(&input.name, &input.species, &input.biography) {
            return Err(Error::Invalid);
        }
        let mut tx = self.db.begin().await.map_err(|_| Error::Unavailable)?;
        let state: Option<String> =
            sqlx::query_scalar("SELECT status FROM pets WHERE id=$1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|_| Error::Unavailable)?;
        if !state
            .as_deref()
            .and_then(Lifecycle::from_storage)
            .is_some_and(domain::is_visible)
        {
            return Err(Error::NotFound);
        }
        if !self
            .auth
            .check(actor, "can_edit", &format!("pet:{id}"))
            .await
            .map_err(|_| Error::Unavailable)?
        {
            return Err(Error::Forbidden);
        }
        let changed = sqlx::query(
            "UPDATE pets SET name=$2,species=$3,biography=$4 WHERE id=$1 AND status='active'",
        )
        .bind(id)
        .bind(&input.name)
        .bind(&input.species)
        .bind(&input.biography)
        .execute(&mut *tx)
        .await
        .map_err(|_| Error::Unavailable)?
        .rows_affected();
        if changed == 1 {
            tx.commit().await.map_err(|_| Error::Unavailable)
        } else {
            Err(Error::NotFound)
        }
    }
    pub(crate) async fn delete_pet(&self, id: &str, actor: &str) -> Result<()> {
        let mut tx = self.db.begin().await.map_err(|_| Error::Unavailable)?;
        let row = sqlx::query("SELECT status,delete_actor FROM pets WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| Error::Unavailable)?
            .ok_or(Error::NotFound)?;
        let status: String = row.get("status");
        let delete_actor: Option<String> = row.get("delete_actor");
        let state = Lifecycle::from_storage(&status).ok_or(Error::Unavailable)?;
        if state == Lifecycle::Active {
            if !self
                .auth
                .check(actor, "can_manage", &format!("pet:{id}"))
                .await
                .map_err(|_| Error::Unavailable)?
            {
                return Err(Error::Forbidden);
            }
            if domain::transition(state, Event::BeginDeletion) != Some(Lifecycle::Deleting) {
                return Err(Error::Unavailable);
            }
            sqlx::query(
                "UPDATE pets SET status='deleting',delete_actor=$2 WHERE id=$1 AND status='active'",
            )
            .bind(id)
            .bind(actor)
            .execute(&mut *tx)
            .await
            .map_err(|_| Error::Unavailable)?;
        } else if delete_actor.as_deref() != Some(actor) {
            return Err(Error::NotFound);
        }
        tx.commit().await.map_err(|_| Error::Unavailable)?;
        let state = if state == Lifecycle::Active {
            Lifecycle::Deleting
        } else {
            state
        };
        if state == Lifecycle::Deleted {
            return Ok(());
        }
        if domain::transition(state, Event::TuplesRemoved) != Some(Lifecycle::Deleted) {
            return Err(Error::NotFound);
        }
        self.auth
            .delete_object(&format!("pet:{id}"))
            .await
            .map_err(|_| Error::Unavailable)?;
        sqlx::query("UPDATE pets SET status='deleted' WHERE id=$1 AND status='deleting' AND delete_actor=$2")
            .bind(id).bind(actor).execute(&self.db).await.map_err(|_| Error::Unavailable)?;
        Ok(())
    }
    pub(crate) async fn set_relation(
        &self,
        actor: &str,
        id: &str,
        relation: &str,
        subject: &str,
        add: bool,
    ) -> Result<()> {
        if !domain::valid_id(subject) || !matches!(relation, "viewer" | "caregiver") {
            return Err(Error::Invalid);
        }
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id=$1)")
            .bind(subject)
            .fetch_one(&self.db)
            .await
            .map_err(|_| Error::Unavailable)?;
        if !exists {
            return Err(Error::NotFound);
        }
        let mut tx = self.db.begin().await.map_err(|_| Error::Unavailable)?;
        let status: Option<String> =
            sqlx::query_scalar("SELECT status FROM pets WHERE id=$1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|_| Error::Unavailable)?;
        if !status
            .as_deref()
            .and_then(Lifecycle::from_storage)
            .is_some_and(domain::is_visible)
        {
            return Err(Error::NotFound);
        }
        if !self
            .auth
            .check(actor, "can_manage", &format!("pet:{id}"))
            .await
            .map_err(|_| Error::Unavailable)?
        {
            return Err(Error::Forbidden);
        }
        let tuple = Tuple {
            user: format!("user:{subject}"),
            relation: relation.into(),
            object: format!("pet:{id}"),
        };
        self.auth
            .write(&[tuple], add)
            .await
            .map_err(|_| Error::Unavailable)?;
        tx.commit().await.map_err(|_| Error::Unavailable)?;
        Ok(())
    }
}
