use crate::application::{AuthError, Authorizer, Tuple};
use argon2::{
    Argon2, PasswordHasher,
    password_hash::{SaltString, rand_core::OsRng},
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
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
    pub(crate) client: reqwest::Client,
    pub(crate) url: String,
    pub(crate) store: String,
    pub(crate) model: String,
}
#[derive(Deserialize)]
struct Check {
    allowed: bool,
}
#[derive(Deserialize)]
struct Health {
    status: String,
}
#[derive(Deserialize)]
struct ReadTuple {
    key: Tuple,
}
#[derive(Deserialize)]
struct ReadResponse {
    #[serde(default)]
    tuples: Vec<ReadTuple>,
}
#[async_trait]
impl Authorizer for OpenFga {
    async fn ensure_available(&self) -> Result<(), AuthError> {
        let health = self
            .client
            .get(format!("{}/healthz", self.url))
            .send()
            .await
            .map_err(|_| AuthError)?
            .error_for_status()
            .map_err(|_| AuthError)?
            .json::<Health>()
            .await
            .map_err(|_| AuthError)?;
        if health.status == "SERVING" {
            Ok(())
        } else {
            Err(AuthError)
        }
    }
    async fn check(&self, user: &str, relation: &str, object: &str) -> Result<bool, AuthError> {
        let response = self
            .client
            .post(format!("{}/stores/{}/check", self.url, self.store))
            .json(
                &json!({"authorization_model_id":self.model,"consistency":"HIGHER_CONSISTENCY",
                "tuple_key":{"user":format!("user:{user}"),"relation":relation,"object":object}}),
            )
            .send()
            .await
            .map_err(|_| AuthError)?
            .error_for_status()
            .map_err(|_| AuthError)?
            .json::<Check>()
            .await
            .map_err(|_| AuthError)?;
        Ok(response.allowed)
    }
    async fn write(&self, tuples: &[Tuple], add: bool) -> Result<(), AuthError> {
        let mut data = json!({"authorization_model_id":self.model});
        data[if add { "writes" } else { "deletes" }] = if add {
            json!({"tuple_keys":tuples, "on_duplicate":"ignore"})
        } else {
            json!({"tuple_keys":tuples, "on_missing":"ignore"})
        };
        self.client
            .post(format!("{}/stores/{}/write", self.url, self.store))
            .json(&data)
            .send()
            .await
            .map_err(|_| AuthError)?
            .error_for_status()
            .map_err(|_| AuthError)?;
        Ok(())
    }
    async fn delete_object(&self, object: &str) -> Result<(), AuthError> {
        // Re-read the first page after every deletion: continuation tokens do
        // not give a stable snapshot while the underlying tuples are removed.
        for _ in 0..1000 {
            let page = self
                .client
                .post(format!("{}/stores/{}/read", self.url, self.store))
                .json(&json!({"tuple_key":{"object":object}, "page_size":100,
                    "consistency":"HIGHER_CONSISTENCY"}))
                .send()
                .await
                .map_err(|_| AuthError)?
                .error_for_status()
                .map_err(|_| AuthError)?
                .json::<ReadResponse>()
                .await
                .map_err(|_| AuthError)?;
            if page.tuples.is_empty() {
                return Ok(());
            }
            let tuples: Vec<_> = page.tuples.into_iter().map(|entry| entry.key).collect();
            if tuples.iter().any(|tuple| tuple.object != object) {
                return Err(AuthError);
            }
            self.write(&tuples, false).await?;
        }
        Err(AuthError)
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

#[cfg(test)]
mod tests {
    use super::{Authorizer, DenyAll, Tuple};

    #[tokio::test]
    async fn deny_all_never_grants_or_mutates_authorization() {
        let auth: &dyn Authorizer = &DenyAll;
        assert!(auth.ensure_available().await.is_err());
        for (user, relation, object) in [
            ("alice", "can_create", "shelter:central"),
            ("carol", "can_view", "pet:fox"),
            ("carol", "can_edit", "pet:fox"),
            ("carol", "can_manage", "pet:fox"),
        ] {
            assert!(!auth.check(user, relation, object).await.unwrap());
        }
        let tuples = [Tuple {
            user: "user:dave".into(),
            relation: "viewer".into(),
            object: "pet:fox".into(),
        }];
        for add in [true, false] {
            assert!(auth.write(&tuples, add).await.is_err());
        }
        assert!(auth.delete_object("pet:fox").await.is_err());
    }
}
