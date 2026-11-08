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
    async fn write(&self, _: &[Tuple], _: bool) -> Result<(), AuthError> {
        Err(AuthError)
    }
    async fn apply(&self, _: &[Tuple], _: &[Tuple]) -> Result<(), AuthError> {
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
    for (id, name) in [
        ("alice", "Alice"),
        ("bob", "Bob"),
        ("carol", "Carol"),
        ("dave", "Dave"),
        ("eve", "Eve"),
    ] {
        let hash = hash_password(password)?;
        sqlx::query("INSERT INTO users (id,display_name,password_hash) VALUES ($1,$2,$3) ON CONFLICT (id) DO UPDATE SET password_hash=EXCLUDED.password_hash WHERE users.password_hash IS NULL")
            .bind(id).bind(name).bind(hash).execute(db).await?;
    }
    crate::authorization_migration::seed(db).await?;
    Ok(())
}

pub(crate) async fn recover_pending(
    db: &PgPool,
    auth: &dyn Authorizer,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Startup calls this only after validating the bound migration gate. Every
    // replay uses durable catalog state and the same project lock as requests.
    // A failure leaves the resource hidden and aborts startup.
    let rows = sqlx::query("SELECT project_id,warehouse_id,id FROM catalog_tables WHERE status IN ('pending','updating','deleting') ORDER BY project_id,warehouse_id,id")
        .fetch_all(db).await?;
    for row in rows {
        let project: String = row.try_get("project_id")?;
        let warehouse: String = row.try_get("warehouse_id")?;
        let id: String = row.try_get("id")?;
        let mut tx = db.begin().await?;
        sqlx::query("SELECT id FROM catalog_projects WHERE id=$1 FOR UPDATE")
            .bind(&project)
            .fetch_one(&mut *tx)
            .await?;
        let row = sqlx::query("SELECT owner_id,namespace_id,status FROM catalog_tables WHERE project_id=$1 AND warehouse_id=$2 AND id=$3 FOR UPDATE")
            .bind(&project).bind(&warehouse).bind(&id).fetch_one(&mut *tx).await?;
        let owner: String = row.try_get("owner_id")?;
        let namespace: String = row.try_get("namespace_id")?;
        let status: String = row.try_get("status")?;
        let object = format!("lakekeeper_table:{warehouse}/{id}");
        match status.as_str() {
            "pending" => {
                auth.write(
                    &crate::use_cases::provisioning_tuples(&warehouse, &id, &namespace, &owner),
                    true,
                )
                .await
                .map_err(|_| "pending catalog provisioning failed")?;
                sqlx::query("UPDATE catalog_tables SET status='active' WHERE project_id=$1 AND warehouse_id=$2 AND id=$3 AND status='pending'")
                    .bind(&project).bind(&warehouse).bind(&id).execute(&mut *tx).await?;
            }
            "updating" => {
                let intent = sqlx::query("SELECT owner_id,namespace_id,writes::text,deletes::text FROM catalog_pending_updates WHERE project_id=$1 AND warehouse_id=$2 AND table_id=$3 FOR UPDATE")
                    .bind(&project).bind(&warehouse).bind(&id).fetch_one(&mut *tx).await?;
                let writes: Vec<Tuple> =
                    serde_json::from_str(&intent.try_get::<String, _>("writes")?)?;
                let deletes: Vec<Tuple> =
                    serde_json::from_str(&intent.try_get::<String, _>("deletes")?)?;
                auth.apply(&writes, &deletes)
                    .await
                    .map_err(|_| "pending catalog relationship replacement failed")?;
                let next_owner: String = intent.try_get("owner_id")?;
                let next_namespace: String = intent.try_get("namespace_id")?;
                sqlx::query("UPDATE catalog_tables SET owner_id=$4,namespace_id=$5,status='active' WHERE project_id=$1 AND warehouse_id=$2 AND id=$3 AND status='updating'")
                    .bind(&project).bind(&warehouse).bind(&id).bind(&next_owner).bind(&next_namespace).execute(&mut *tx).await?;
                sqlx::query("DELETE FROM catalog_pending_updates WHERE project_id=$1 AND warehouse_id=$2 AND table_id=$3")
                    .bind(&project).bind(&warehouse).bind(&id).execute(&mut *tx).await?;
            }
            "deleting" => {
                auth.delete_object(&object)
                    .await
                    .map_err(|_| "pending catalog deletion failed")?;
                auth.write(
                    &[Tuple {
                        user: object.clone(),
                        relation: "child".into(),
                        object: format!("namespace:{namespace}"),
                    }],
                    false,
                )
                .await
                .map_err(|_| "pending catalog hierarchy cleanup failed")?;
                sqlx::query("UPDATE catalog_tables SET status='deleted' WHERE project_id=$1 AND warehouse_id=$2 AND id=$3 AND status='deleting'")
                    .bind(&project).bind(&warehouse).bind(&id).execute(&mut *tx).await?;
            }
            "active" | "deleted" => continue,
            _ => return Err("invalid catalog lifecycle state".into()),
        }
        tx.commit().await?;
        tracing::info!(
            event = "catalog_recovered",
            project,
            object,
            previous_status = status,
            "catalog audit"
        );
    }
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
            if health.status == "SERVING" {
                Ok(())
            } else {
                Err(protocol_error("health", "backend is not serving"))
            }
        }

        async fn ensure_ready(&self) -> Result<(), AuthError> {
            self.ensure_available().await?;
            // A model can remain readable after a soft-deleted store. Check
            // both resources and their identities, not just an HTTP status.
            let store: Store = send_json(
                self.client
                    .get(format!("{}/stores/{}", self.url, self.store)),
                "get_store",
            )
            .await?;
            if store.id != self.store {
                return Err(protocol_error("get_store", "store identity mismatch"));
            }
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

        #[tracing::instrument(skip(self, tuples), fields(count = tuples.len(), add), name = "openfga.write")]
        async fn write(&self, tuples: &[Tuple], add: bool) -> Result<(), AuthError> {
            if add {
                self.apply(tuples, &[]).await
            } else {
                self.apply(&[], tuples).await
            }
        }

        #[tracing::instrument(skip(self, writes, deletes), fields(writes = writes.len(), deletes = deletes.len()), name = "openfga.apply")]
        async fn apply(&self, writes: &[Tuple], deletes: &[Tuple]) -> Result<(), AuthError> {
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
