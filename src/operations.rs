//! Readiness includes the bound migration gate and durable mutation fence.
use crate::application::{AuthorizationTarget, Authorizer};
use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};

#[derive(Clone)]
pub(crate) struct Readiness {
    db: PgPool,
    backend: Arc<dyn Authorizer>,
    target: Option<AuthorizationTarget>,
}
impl Readiness {
    pub(crate) fn new(
        db: PgPool,
        backend: Arc<dyn Authorizer>,
        target: Option<AuthorizationTarget>,
    ) -> Self {
        Self {
            db,
            backend,
            target,
        }
    }
}
async fn ready(State(state): State<Readiness>) -> (StatusCode, Json<Value>) {
    let Some(target) = state.target.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"ready":false})),
        );
    };
    let result = tokio::time::timeout(Duration::from_secs(3), async {
        let (db, fga) = tokio::join!(
            crate::authorization_migration::ensure_active(
                &state.db,
                &target.store,
                &target.model,
                &target.model_sha256
            ),
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
