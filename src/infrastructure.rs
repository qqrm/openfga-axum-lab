use crate::application::{AuthError, Authorizer, Tuple};
use argon2::{
    Argon2, PasswordHasher,
    password_hash::{SaltString, rand_core::OsRng},
};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};

pub(crate) fn hash_token(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}
pub(crate) fn hash_password(password: &str) -> Result<String, Box<dyn std::error::Error>> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| "password hashing failed".into())
}

// Emergency kill switch. It never grants access or silently falls back.
pub(crate) struct DenyAll;
#[async_trait]
impl Authorizer for DenyAll {
    async fn ensure_available(&self) -> Result<(), AuthError> {
        Err(AuthError)
    }
    async fn check(&self, _: &str, _: &str, _: &str) -> Result<bool, AuthError> {
        Ok(false)
    }
    async fn write(&self, _: &[Tuple], _: bool) -> Result<(), AuthError> {
        Err(AuthError)
    }
    async fn delete_object(&self, _: &str) -> Result<(), AuthError> {
        Err(AuthError)
    }
}
pub(crate) struct OpenFga {
    pub(crate) client: openfga_client::client::OpenFgaClient<openfga_client::tonic::transport::Channel>,
}

#[async_trait]
impl Authorizer for OpenFga {
    #[tracing::instrument(skip(self), name = "openfga.ensure_available")]
    async fn ensure_available(&self) -> Result<(), AuthError> {
        // A cheap check that exercises the gRPC connection and store.
        self.client
            .check_simple(openfga_client::client::CheckRequestTupleKey {
                user: "user:health".to_string(),
                relation: "can_view".to_string(),
                object: "pet:health".to_string(),
            })
            .await
            .map(|_| ())
            .map_err(|_| AuthError)
    }

    #[tracing::instrument(skip(self, user, object), fields(relation = %relation), name = "openfga.check")]
    async fn check(&self, user: &str, relation: &str, object: &str) -> Result<bool, AuthError> {
        self.client
            .check_simple(openfga_client::client::CheckRequestTupleKey {
                user: format!("user:{user}"),
                relation: relation.to_string(),
                object: object.to_string(),
            })
            .await
            .map_err(|_| AuthError)
    }

    #[tracing::instrument(skip(self, tuples), fields(count = tuples.len(), add), name = "openfga.write")]
    async fn write(&self, tuples: &[Tuple], add: bool) -> Result<(), AuthError> {
        if add {
            let writes: Vec<openfga_client::client::TupleKey> = tuples
                .iter()
                .map(|tuple| openfga_client::client::TupleKey {
                    user: tuple.user.clone(),
                    relation: tuple.relation.clone(),
                    object: tuple.object.clone(),
                    condition: None,
                })
                .collect();
            self.client
                .write(Some(writes), None)
                .await
                .map(|_| ())
                .map_err(|_| AuthError)
        } else {
            let deletes: Vec<openfga_client::client::TupleKeyWithoutCondition> = tuples
                .iter()
                .map(|tuple| openfga_client::client::TupleKeyWithoutCondition {
                    user: tuple.user.clone(),
                    relation: tuple.relation.clone(),
                    object: tuple.object.clone(),
                })
                .collect();
            self.client
                .write(None, Some(deletes))
                .await
                .map(|_| ())
                .map_err(|_| AuthError)
        }
    }

    #[tracing::instrument(skip(self), fields(object = %object), name = "openfga.delete_object")]
    async fn delete_object(&self, object: &str) -> Result<(), AuthError> {
        self.client
            .delete_relations_to_object(object)
            .await
            .map(|_| ())
            .map_err(|_| AuthError)
    }
}

pub(crate) async fn seed(db: &PgPool, password: &str) -> Result<(), Box<dyn std::error::Error>> {
    for (id, name) in [
        ("alice", "Alice"),
        ("bob", "Bob"),
        ("carol", "Carol"),
        ("dave", "Dave"),
    ] {
        let hash = hash_password(password)?;
        sqlx::query("INSERT INTO users (id,display_name,password_hash) VALUES ($1,$2,$3) ON CONFLICT (id) DO UPDATE SET password_hash=EXCLUDED.password_hash WHERE users.password_hash IS NULL")
            .bind(id).bind(name).bind(hash).execute(db).await?;
    }
    for (id, name, species, biography, owner) in [
        (
            "fox",
            "Mika",
            "fox",
            "A curious fox who likes quiet afternoons.",
            "carol",
        ),
        (
            "cat",
            "Pixel",
            "cat",
            "A sleepy cat with a loud purr.",
            "alice",
        ),
        (
            "dog",
            "Rex",
            "dog",
            "A friendly dog who loves walks.",
            "bob",
        ),
    ] {
        sqlx::query("INSERT INTO pets (id,name,species,biography,owner_id,status) VALUES ($1,$2,$3,$4,$5,'active') ON CONFLICT (id) DO NOTHING")
            .bind(id).bind(name).bind(species).bind(biography).bind(owner).execute(db).await?;
    }
    Ok(())
}
pub(crate) async fn recover_pending(
    db: &PgPool,
    auth: &dyn Authorizer,
) -> Result<(), Box<dyn std::error::Error>> {
    let ids: Vec<String> =
        sqlx::query_scalar("SELECT id FROM pets WHERE status='pending' ORDER BY id")
            .fetch_all(db)
            .await?;
    for id in ids {
        let mut transaction = db.begin().await?;
        let row =
            sqlx::query("SELECT owner_id FROM pets WHERE id=$1 AND status='pending' FOR UPDATE")
                .bind(&id)
                .fetch_optional(&mut *transaction)
                .await?;
        let Some(row) = row else { continue };
        let owner: String = row.get("owner_id");
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
        auth.write(&tuples, true)
            .await
            .map_err(|_| "pending authorization recovery failed")?;
        sqlx::query("UPDATE pets SET status='active' WHERE id=$1 AND status='pending'")
            .bind(id)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
    }
    let deleting: Vec<String> =
        sqlx::query_scalar("SELECT id FROM pets WHERE status='deleting' ORDER BY id")
            .fetch_all(db)
            .await?;
    for id in deleting {
        auth.delete_object(&format!("pet:{id}"))
            .await
            .map_err(|_| "pending deletion recovery failed")?;
        sqlx::query("UPDATE pets SET status='deleted' WHERE id=$1 AND status='deleting'")
            .bind(id)
            .execute(db)
            .await?;
    }
    Ok(())
}
