use crate::application::{AuthError, Authorizer, Tuple};
use argon2::{
    Argon2, PasswordHasher,
    password_hash::{SaltString, rand_core::OsRng},
};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use sqlx::PgPool;

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
    async fn ensure_ready(&self) -> Result<(), AuthError> {
        Err(AuthError)
    }
    async fn check(&self, _: &str, _: &str, _: &str) -> Result<bool, AuthError> {
        Ok(false)
    }
    async fn list_objects(&self, _: &str, _: &str, _: &str) -> Result<Vec<String>, AuthError> {
        Ok(Vec::new())
    }
    async fn write(&self, _: &[Tuple], _: bool) -> Result<(), AuthError> {
        Err(AuthError)
    }
    async fn change(&self, _: &[Tuple], _: &[Tuple]) -> Result<(), AuthError> {
        Err(AuthError)
    }
    async fn delete_object(&self, _: &str) -> Result<(), AuthError> {
        Err(AuthError)
    }
}
#[cfg(feature = "grpc")]
mod openfga_grpc;
#[cfg(feature = "grpc")]
pub use openfga_grpc::OpenFgaGrpc;
#[cfg(feature = "http")]
use serde::Deserialize;
#[cfg(feature = "http")]
use serde_json::json;

pub(crate) async fn seed(db: &PgPool, password: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut tx = db.begin().await?;
    for (id, name) in [
        ("alice", "Alice"),
        ("bob", "Bob"),
        ("carol", "Carol"),
        ("dave", "Dave"),
        ("erin", "Erin"),
    ] {
        let hash = hash_password(password)?;
        sqlx::query("INSERT INTO users (id,display_name,password_hash) VALUES ($1,$2,$3) ON CONFLICT (id) DO UPDATE SET password_hash=EXCLUDED.password_hash WHERE users.password_hash IS NULL")
            .bind(id).bind(name).bind(hash).execute(&mut *tx).await?;
    }
    for (id, name, owner, members) in [
        (
            "acme",
            "Acme Compliance",
            "alice",
            vec![("alice", "member"), ("bob", "admin"), ("carol", "member")],
        ),
        (
            "other",
            "Other Organization",
            "erin",
            vec![("erin", "member"), ("dave", "member")],
        ),
    ] {
        let inserted = sqlx::query(
            "INSERT INTO organizations (id,name,owner_id) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING",
        )
        .bind(id)
        .bind(name)
        .bind(owner)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if inserted == 1 {
            for (user, role) in members {
                sqlx::query("INSERT INTO organization_members (organization_id,user_id,role) VALUES ($1,$2,$3)")
                    .bind(id).bind(user).bind(role).execute(&mut *tx).await?;
            }
            if id == "acme" {
                sqlx::query("INSERT INTO organization_permissions (organization_id,user_id,relation) VALUES ('acme','bob','can_view_task')")
                    .execute(&mut *tx).await?;
            }
        }
    }
    for (id, org, title, description, assigner) in [
        (
            "access-review",
            "acme",
            "Quarterly access review",
            "Collect evidence and review privileged accounts.",
            "carol",
        ),
        (
            "vendor-review",
            "acme",
            "Vendor security assessment",
            "Review vendor controls and record outstanding evidence.",
            "alice",
        ),
        (
            "incident-response",
            "acme",
            "Incident response exercise",
            "Exercise the response procedure and capture follow-up actions.",
            "bob",
        ),
        (
            "other-review",
            "other",
            "Independent organization review",
            "This task demonstrates organization isolation.",
            "erin",
        ),
    ] {
        let fenced: bool = sqlx::query_scalar(
            "SELECT authz_pending OR authz_uncertain FROM organizations WHERE id=$1 FOR UPDATE",
        )
        .bind(org)
        .fetch_one(&mut *tx)
        .await?;
        if fenced {
            continue;
        }
        let inserted = sqlx::query("INSERT INTO tasks (id,organization_id,title,description,task_status,assigner_id,status) VALUES ($1,$2,$3,$4,'todo',$5,'active') ON CONFLICT DO NOTHING")
            .bind(id).bind(org).bind(title).bind(description).bind(assigner).execute(&mut *tx).await?.rows_affected();
        if inserted == 1 {
            let grant = match id {
                "vendor-review" => Some(("dave", "viewer")),
                "incident-response" => Some(("carol", "assignee")),
                _ => None,
            };
            if let Some((user, relation)) = grant {
                sqlx::query(
                    "INSERT INTO task_relations (task_id,user_id,relation) VALUES ($1,$2,$3)",
                )
                .bind(id)
                .bind(user)
                .bind(relation)
                .execute(&mut *tx)
                .await?;
            }
        }
    }
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn recover_pending(
    db: &PgPool,
    coordinator: &PgPool,
    auth: &dyn Authorizer,
    after_backend_drain: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    crate::authorization::recover(db, coordinator, auth, after_backend_drain).await?;
    Ok(())
}

#[cfg(feature = "http")]
mod openfga_http {
    use super::*;
    pub struct OpenFgaHttp {
        pub client: reqwest::Client,
        pub url: String,
        pub store: String,
        pub model: String,
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
    #[derive(Deserialize)]
    struct ListObjectsResponse {
        #[serde(default)]
        objects: Vec<String>,
    }
    #[async_trait]
    impl Authorizer for OpenFgaHttp {
        #[tracing::instrument(skip(self), name = "openfga.ensure_available")]
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
            if health.status != "SERVING" {
                return Err(AuthError);
            }
            // A healthy server may host a soft-deleted store. Cached allows
            // must probe the configured store, matching the gRPC adapter.
            self.client
                .get(format!("{}/stores/{}", self.url, self.store))
                .send()
                .await
                .map_err(|_| AuthError)?
                .error_for_status()
                .map_err(|_| AuthError)?
                .bytes()
                .await
                .map_err(|_| AuthError)?;
            Ok(())
        }
        async fn ensure_ready(&self) -> Result<(), AuthError> {
            self.ensure_available().await?;
            self.client
                .get(format!(
                    "{}/stores/{}/authorization-models/{}",
                    self.url, self.store, self.model
                ))
                .send()
                .await
                .map_err(|_| AuthError)?
                .error_for_status()
                .map_err(|_| AuthError)?
                .bytes()
                .await
                .map_err(|_| AuthError)?;
            Ok(())
        }
        #[tracing::instrument(skip(self, user, object), fields(relation = %relation), name = "openfga.check")]
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
        #[tracing::instrument(skip(self, user), fields(relation = %relation, object_type = %object_type), name = "openfga.list_objects")]
        async fn list_objects(
            &self,
            user: &str,
            relation: &str,
            object_type: &str,
        ) -> Result<Vec<String>, AuthError> {
            let response: ListObjectsResponse = self
                .client
                .post(format!("{}/stores/{}/list-objects", self.url, self.store))
                .json(&json!({
                    "authorization_model_id": self.model,
                    "consistency": "HIGHER_CONSISTENCY",
                    "user": format!("user:{user}"),
                    "relation": relation,
                    "type": object_type
                }))
                .send()
                .await
                .map_err(|_| AuthError)?
                .error_for_status()
                .map_err(|_| AuthError)?
                .json()
                .await
                .map_err(|_| AuthError)?;
            Ok(response.objects)
        }
        #[tracing::instrument(skip(self, tuples), fields(count = tuples.len(), add), name = "openfga.write")]
        async fn write(&self, tuples: &[Tuple], add: bool) -> Result<(), AuthError> {
            if add {
                self.change(tuples, &[]).await
            } else {
                self.change(&[], tuples).await
            }
        }
        async fn change(&self, writes: &[Tuple], deletes: &[Tuple]) -> Result<(), AuthError> {
            if writes.is_empty() && deletes.is_empty() {
                return Ok(());
            }
            let mut data = json!({"authorization_model_id":self.model});
            if !writes.is_empty() {
                data["writes"] = json!({"tuple_keys":writes,"on_duplicate":"ignore"});
            }
            if !deletes.is_empty() {
                data["deletes"] = json!({"tuple_keys":deletes,"on_missing":"ignore"});
            }
            self.client
                .post(format!("{}/stores/{}/write", self.url, self.store))
                .json(&data)
                .send()
                .await
                .map_err(|_| AuthError)?
                .error_for_status()
                .map_err(|_| AuthError)?
                .bytes()
                .await
                .map_err(|_| AuthError)?;
            Ok(())
        }
        #[tracing::instrument(skip(self), fields(object = %object), name = "openfga.delete_object")]
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
}

#[cfg(feature = "http")]
pub use openfga_http::OpenFgaHttp;
