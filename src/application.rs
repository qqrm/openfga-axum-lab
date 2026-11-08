use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
#[error("authorization backend unavailable")]
pub struct AuthError;
#[derive(Clone, Debug, Deserialize, Serialize)]
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
    /// Apply both sides in one atomic OpenFGA Write. Never split replacements.
    async fn change(&self, writes: &[Tuple], deletes: &[Tuple]) -> Result<(), AuthError>;
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
    pub(crate) authz_coordinator: PgPool,
    pub(crate) auth: Arc<dyn Authorizer>,
    pub(crate) login_slots: Arc<tokio::sync::Semaphore>,
    pub(crate) dummy_password_hash: Arc<str>,
    pub(crate) metrics: crate::metrics::Metrics,
    pub(crate) authorization_target: Option<AuthorizationTarget>,
}
#[derive(Clone, derive_more::Deref)]
pub(crate) struct Identity(pub(crate) String);
#[derive(Serialize)]
pub(crate) struct Task {
    pub(crate) id: String,
    pub(crate) organization_id: String,
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) status: String,
    pub(crate) assigner_id: String,
    pub(crate) can_edit: bool,
    pub(crate) can_delete: bool,
    pub(crate) recovery_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) relations: Option<Vec<Relation>>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TaskInput {
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) status: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateTask {
    pub(crate) organization_id: String,
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) status: String,
}
#[derive(Serialize)]
pub(crate) struct Relation {
    pub(crate) relation: String,
    pub(crate) user_id: String,
}
#[derive(Serialize)]
pub(crate) struct Organization {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) owner_id: String,
    pub(crate) can_manage: bool,
    pub(crate) can_create_task: bool,
    pub(crate) recovery_required: bool,
}
#[derive(Serialize)]
pub(crate) struct Member {
    pub(crate) user_id: String,
    pub(crate) display_name: String,
    pub(crate) role: String,
    pub(crate) is_owner: bool,
}
