use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
#[error("authorization backend unavailable")]
pub struct AuthError;
#[derive(Clone, Deserialize, Serialize)]
pub struct Tuple {
    pub user: String,
    pub relation: String,
    pub object: String,
}
#[async_trait]
pub trait Authorizer: Send + Sync {
    async fn ensure_available(&self) -> Result<(), AuthError>;
    /// Probe the live transport, store and configured model for operational readiness.
    /// This must not use cached authorization decisions.
    async fn ensure_ready(&self) -> Result<(), AuthError>;
    async fn check(&self, user: &str, relation: &str, object: &str) -> Result<bool, AuthError>;
    async fn list_objects(
        &self,
        user: &str,
        relation: &str,
        object_type: &str,
    ) -> Result<Vec<String>, AuthError>;
    async fn write(&self, tuples: &[Tuple], add: bool) -> Result<(), AuthError>;
    async fn delete_object(&self, object: &str) -> Result<(), AuthError>;
}
#[derive(Clone)]
pub(crate) struct App {
    pub(crate) db: PgPool,
    pub(crate) auth: Arc<dyn Authorizer>,
    pub(crate) login_slots: Arc<tokio::sync::Semaphore>,
    pub(crate) dummy_password_hash: Arc<str>,
    pub(crate) metrics: crate::metrics::Metrics,
}
#[derive(Clone, derive_more::Deref)]
pub(crate) struct Identity(pub(crate) String);
#[derive(Serialize)]
pub(crate) struct Pet {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) species: String,
    pub(crate) biography: String,
}
#[derive(Deserialize, Serialize)]
pub(crate) struct PetInput {
    pub(crate) name: String,
    pub(crate) species: String,
    pub(crate) biography: String,
}
