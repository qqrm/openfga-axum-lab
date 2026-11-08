use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::sync::Arc;

// Deserialization is at the delivery boundary; the pure domain remains usable
// without serde by the bounded formal checks and the small WASM visualizer.
macro_rules! identifier_serde {
    ($($name:ident),+ $(,)?) => {$(
        impl<'de> Deserialize<'de> for crate::domain::$name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                Self::try_from(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
            }
        }
        impl Serialize for crate::domain::$name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }
    )+};
}
identifier_serde!(UserId, ProjectId, WarehouseId, NamespaceId, TableId, RoleId);

#[derive(Debug, thiserror::Error)]
#[error("authorization backend unavailable")]
pub struct AuthError;
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
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
    /// Every allow requires a fresh successful authoritative Check. Unresolved
    /// mutations must return an error; a cached denial may conservatively deny.
    async fn check(&self, user: &str, relation: &str, object: &str) -> Result<bool, AuthError>;
    async fn write(&self, tuples: &[Tuple], add: bool) -> Result<(), AuthError>;
    /// Apply both sides of a relationship replacement in one OpenFGA transaction.
    /// Never split a mixed change into separate grant/revoke operations.
    async fn apply(&self, writes: &[Tuple], deletes: &[Tuple]) -> Result<(), AuthError>;
    async fn delete_object(&self, object: &str) -> Result<(), AuthError>;
}
#[derive(Clone)]
pub(crate) struct AuthorizationTarget {
    pub(crate) store: String,
    pub(crate) model: String,
    pub(crate) model_sha256: String,
}
#[derive(Clone)]
pub(crate) struct App {
    pub(crate) db: PgPool,
    pub(crate) auth: Arc<dyn Authorizer>,
    pub(crate) login_slots: Arc<tokio::sync::Semaphore>,
    pub(crate) dummy_password_hash: Arc<str>,
    pub(crate) metrics: crate::metrics::Metrics,
    pub(crate) authorization_target: AuthorizationTarget,
}
#[derive(Clone, derive_more::Deref)]
pub(crate) struct Identity(pub(crate) crate::domain::UserId);
#[derive(Clone, Serialize)]
pub(crate) struct CatalogTable {
    pub(crate) id: String,
    pub(crate) project_id: String,
    pub(crate) warehouse_id: String,
    pub(crate) namespace_id: String,
    pub(crate) owner_id: String,
    pub(crate) name: String,
    pub(crate) description: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TableInput {
    pub(crate) namespace_id: crate::domain::NamespaceId,
    pub(crate) name: String,
    pub(crate) description: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TableMetadata {
    pub(crate) name: String,
    pub(crate) description: String,
}
#[derive(Serialize)]
pub(crate) struct TablePage {
    pub(crate) items: Vec<CatalogTable>,
    pub(crate) next_cursor: Option<String>,
}
