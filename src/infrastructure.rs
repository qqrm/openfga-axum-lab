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
pub(crate) fn hash_password(
    password: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
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

pub(crate) async fn seed(
    db: &PgPool,
    password: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut tx = db.begin().await?;
    // Serialize competing demo starters. Do not lock the journal before source
    // tables: migration and statement triggers acquire those locks first.
    // A race with import is rejected by the source-freeze triggers.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('openlane-demo-seed', 0))")
        .execute(&mut *tx)
        .await?;
    let migration: String =
        sqlx::query_scalar("SELECT status FROM openlane_authorization_migration WHERE singleton")
            .fetch_one(&mut *tx)
            .await?;
    let catalog_source: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM catalog_projects) OR EXISTS(SELECT 1 FROM legacy_resource_grants) OR EXISTS(SELECT 1 FROM authorization_migration WHERE source_sha256 IS NOT NULL OR status <> 'pending')",
    )
    .fetch_one(&mut *tx)
    .await?;
    if catalog_source {
        return Err("Openlane demo setup requires an isolated source; existing Lakekeeper data or authorization binding needs an explicit cutover".into());
    }
    let existing_source: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM organizations)")
        .fetch_one(&mut *tx)
        .await?;
    if migration != "pending" || existing_source {
        tx.rollback().await?;
        return Ok(());
    }
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
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    crate::authorization::recover(db, coordinator, auth, after_backend_drain).await?;
    Ok(())
}

#[cfg(feature = "http")]
mod openfga_http {
    use super::*;
    use reqwest::{RequestBuilder, StatusCode, header::CONTENT_TYPE};
    use serde::de::DeserializeOwned;
    use std::time::Duration;

    const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
    const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

    pub struct OpenFgaHttp {
        /// Use a client with redirects disabled. Response URLs are also checked
        /// so a followed redirect cannot be mistaken for backend confirmation.
        pub client: reqwest::Client,
        pub url: String,
        pub store: String,
        pub model: String,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Check {
        allowed: bool,
        #[serde(default, rename = "resolution")]
        _resolution: Option<String>,
    }
    #[derive(Deserialize)]
    struct Health {
        status: String,
    }
    #[derive(Deserialize)]
    struct Store {
        id: String,
    }
    #[derive(Deserialize)]
    struct ModelResponse {
        authorization_model: Store,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct WriteResponse {}
    #[derive(Deserialize)]
    struct ReadTuple {
        key: Tuple,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ReadResponse {
        #[serde(default)]
        tuples: Vec<ReadTuple>,
        #[serde(default)]
        continuation_token: String,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ListObjectsResponse {
        #[serde(default)]
        objects: Vec<String>,
    }

    fn protocol_error(operation: &'static str, reason: &'static str) -> AuthError {
        tracing::warn!(operation, reason, "invalid OpenFGA HTTP response");
        AuthError
    }

    // Validate the complete response. reqwest::error_for_status accepts 3xx,
    // and receiving headers alone does not confirm a completed write or Check.
    // Bound bodies even if the caller supplies a client without a timeout.
    async fn send_json<T: DeserializeOwned>(
        builder: RequestBuilder,
        operation: &'static str,
    ) -> Result<T, AuthError> {
        tokio::time::timeout(REQUEST_TIMEOUT, async {
            let (client, request) = builder.build_split();
            let request = request.map_err(|_| protocol_error(operation, "request construction"))?;
            let expected_url = request.url().clone();
            let mut response = client
                .execute(request)
                .await
                .map_err(|_| protocol_error(operation, "transport failure"))?;
            if response.status() != StatusCode::OK || response.url() != &expected_url {
                tracing::warn!(operation, status = %response.status(), "OpenFGA HTTP response did not confirm success");
                return Err(AuthError);
            }
            let json_content = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(';').next())
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"));
            if !json_content {
                return Err(protocol_error(operation, "expected JSON content type"));
            }
            if response
                .content_length()
                .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
            {
                return Err(protocol_error(operation, "response body too large"));
            }
            let mut body = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| protocol_error(operation, "incomplete response body"))?
            {
                if chunk.len() > MAX_RESPONSE_BYTES - body.len() {
                    return Err(protocol_error(operation, "response body too large"));
                }
                body.extend_from_slice(&chunk);
            }
            // serde can deserialize a struct from a positional JSON array.
            // OpenFGA response messages must be objects: in particular `[]`
            // must never count as a confirmed empty Write response.
            if body.iter().copied().find(|byte| !byte.is_ascii_whitespace()) != Some(b'{') {
                return Err(protocol_error(operation, "expected JSON object response"));
            }
            serde_json::from_slice(&body)
                .map_err(|_| protocol_error(operation, "invalid JSON response schema"))
        })
        .await
        .map_err(|_| protocol_error(operation, "response deadline exceeded"))?
    }

    #[async_trait]
    impl Authorizer for OpenFgaHttp {
        #[tracing::instrument(skip(self), name = "openfga.ensure_available")]
        async fn ensure_available(&self) -> Result<(), AuthError> {
            let health: Health =
                send_json(self.client.get(format!("{}/healthz", self.url)), "health").await?;
            if health.status != "SERVING" {
                return Err(protocol_error("health", "backend is not serving"));
            }
            // A healthy service may still have a deleted or different store.
            let store: Store = send_json(
                self.client
                    .get(format!("{}/stores/{}", self.url, self.store)),
                "get_store",
            )
            .await?;
            if store.id != self.store {
                return Err(protocol_error("get_store", "store identity mismatch"));
            }
            Ok(())
        }

        async fn ensure_ready(&self) -> Result<(), AuthError> {
            self.ensure_available().await?;
            let model: ModelResponse = send_json(
                self.client.get(format!(
                    "{}/stores/{}/authorization-models/{}",
                    self.url, self.store, self.model
                )),
                "get_model",
            )
            .await?;
            if model.authorization_model.id != self.model {
                return Err(protocol_error("get_model", "model identity mismatch"));
            }
            Ok(())
        }

        #[tracing::instrument(skip(self, user, object), fields(relation = %relation), name = "openfga.check")]
        async fn check(&self, user: &str, relation: &str, object: &str) -> Result<bool, AuthError> {
            let response: Check = send_json(
                self.client
                    .post(format!("{}/stores/{}/check", self.url, self.store))
                    .json(&json!({
                        "authorization_model_id": self.model,
                        "consistency": "HIGHER_CONSISTENCY",
                        "tuple_key": {
                            "user": format!("user:{user}"),
                            "relation": relation,
                            "object": object,
                        },
                    })),
                "check",
            )
            .await?;
            Ok(response.allowed)
        }

        #[tracing::instrument(skip(self, user), fields(relation = %relation, object_type = %object_type), name = "openfga.list_objects")]
        async fn list_objects(
            &self,
            user: &str,
            relation: &str,
            object_type: &str,
        ) -> Result<Vec<String>, AuthError> {
            let response: ListObjectsResponse = send_json(
                self.client
                    .post(format!("{}/stores/{}/list-objects", self.url, self.store))
                    .json(&json!({
                        "authorization_model_id": self.model,
                        "consistency": "HIGHER_CONSISTENCY",
                        "user": format!("user:{user}"),
                        "relation": relation,
                        "type": object_type,
                    })),
                "list_objects",
            )
            .await?;
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

        #[tracing::instrument(skip(self, writes, deletes), fields(writes = writes.len(), deletes = deletes.len()), name = "openfga.change")]
        async fn change(&self, writes: &[Tuple], deletes: &[Tuple]) -> Result<(), AuthError> {
            if writes.is_empty() && deletes.is_empty() {
                return Ok(());
            }
            let mut data = json!({"authorization_model_id": self.model});
            if !writes.is_empty() {
                data["writes"] = json!({"tuple_keys": writes, "on_duplicate": "ignore"});
            }
            if !deletes.is_empty() {
                data["deletes"] = json!({"tuple_keys": deletes, "on_missing": "ignore"});
            }
            let _: WriteResponse = send_json(
                self.client
                    .post(format!("{}/stores/{}/write", self.url, self.store))
                    .json(&data),
                "write",
            )
            .await?;
            Ok(())
        }

        #[tracing::instrument(skip(self), fields(object = %object), name = "openfga.delete_object")]
        async fn delete_object(&self, object: &str) -> Result<(), AuthError> {
            // Re-read the first page after every deletion: continuation tokens
            // are not a snapshot while the underlying tuples are removed.
            for _ in 0..1000 {
                let page: ReadResponse = send_json(
                    self.client
                        .post(format!("{}/stores/{}/read", self.url, self.store))
                        .json(&json!({
                            "tuple_key": {"object": object},
                            "page_size": 100,
                            "consistency": "HIGHER_CONSISTENCY",
                        })),
                    "read",
                )
                .await?;
                if page.tuples.is_empty() {
                    return if page.continuation_token.is_empty() {
                        Ok(())
                    } else {
                        Err(protocol_error("read", "empty page is not terminal"))
                    };
                }
                let tuples: Vec<_> = page.tuples.into_iter().map(|entry| entry.key).collect();
                if tuples.iter().any(|tuple| {
                    tuple.object != object || tuple.user.is_empty() || tuple.relation.is_empty()
                }) {
                    return Err(protocol_error("read", "unexpected tuple key"));
                }
                self.write(&tuples, false).await?;
            }
            Err(protocol_error("read", "deletion page limit exceeded"))
        }
    }
}

#[cfg(feature = "http")]
pub use openfga_http::OpenFgaHttp;
