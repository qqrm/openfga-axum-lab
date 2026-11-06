//! Operational probes are not part of the authorization decision/cache path.
use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use reqwest::Client;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::time::Duration;

#[derive(Clone)]
pub(crate) struct Readiness {
    db: PgPool,
    client: Client,
    url: String,
    store: String,
    model: String,
    enabled: bool,
}
impl Readiness {
    pub(crate) fn new(db: PgPool, config: &Value) -> Result<Self, Box<dyn std::error::Error>> {
        let field = |name: &str| config[name].as_str().filter(|v| !v.is_empty())
            .map(str::to_owned).ok_or_else(|| format!("missing {name}"));
        Ok(Self {
            db,
            client: Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(2)).build()?,
            url: std::env::var("FGA_URL").unwrap_or_else(|_| "http://openfga:8080".into()),
            store: field("store_id")?, model: field("model_id")?,
            enabled: std::env::var("AUTHZ_MODE").as_deref() != Ok("deny"),
        })
    }
    async fn backend(&self) -> Result<(), reqwest::Error> {
        // A model may remain readable after a soft-deleted store. Check both.
        for path in ["healthz".to_owned(), format!("stores/{}", self.store),
            format!("stores/{}/authorization-models/{}", self.store, self.model)] {
            self.client.get(format!("{}/{}", self.url, path)).send().await?
                .error_for_status()?.bytes().await?;
        }
        Ok(())
    }
}
async fn ready(State(state): State<Readiness>) -> (StatusCode, Json<Value>) {
    let result = tokio::time::timeout(Duration::from_secs(3), async {
        let (db, fga) = tokio::join!(sqlx::query("SELECT 1").execute(&state.db), state.backend());
        state.enabled && db.is_ok() && fga.is_ok()
    }).await.unwrap_or(false);
    (if result { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE },
        Json(json!({"ready":result})))
}
async fn version() -> Json<Value> {
    Json(json!({"version":env!("CARGO_PKG_VERSION"),
        "revision":option_env!("APP_REVISION").unwrap_or("unknown")}))
}
pub(crate) fn router(state: Readiness) -> Router {
    Router::new().route("/readyz", get(ready)).route("/version", get(version)).with_state(state)
}
