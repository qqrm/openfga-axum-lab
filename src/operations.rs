//! Operational probes use the raw adapter, outside the authorization cache path.
use crate::application::Authorizer;
use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};

#[derive(Clone)]
pub(crate) struct Readiness {
    db: PgPool,
    backend: Arc<dyn Authorizer>,
}
impl Readiness {
    pub(crate) fn new(db: PgPool, backend: Arc<dyn Authorizer>) -> Self {
        Self { db, backend }
    }
}
async fn ready(State(state): State<Readiness>) -> (StatusCode, Json<Value>) {
    let result = tokio::time::timeout(Duration::from_secs(3), async {
        let (db, fga) = tokio::join!(
            sqlx::query("SELECT 1").execute(&state.db),
            state.backend.ensure_ready()
        );
        db.is_ok() && fga.is_ok()
    })
    .await
    .unwrap_or(false);
    (
        if result {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(json!({"ready":result})),
    )
}
async fn version() -> Json<Value> {
    Json(json!({"version":env!("CARGO_PKG_VERSION"),
        "revision":option_env!("APP_REVISION").unwrap_or("unknown")}))
}
pub(crate) fn router(state: Readiness) -> Router {
    Router::new()
        .route("/readyz", get(ready))
        .route("/version", get(version))
        .with_state(state)
}
