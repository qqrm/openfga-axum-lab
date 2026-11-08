//! A local decision cache coordinated by the shared PostgreSQL primary.
use crate::application::{AuthError, Authorizer, Tuple};
use async_trait::async_trait;
use moka::future::Cache;
use prometheus::IntCounterVec;
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};

#[derive(Clone, Hash, PartialEq, Eq)]
struct Key {
    generation: i64,
    user: Box<str>,
    relation: Box<str>,
    object: Box<str>,
}

#[derive(PartialEq, Eq)]
struct Revision {
    generation: i64,
    pending: i64,
}

pub(crate) struct CachedAuthorizer {
    inner: Arc<dyn Authorizer>,
    coordinator: PgPool,
    decisions: Cache<Key, bool>,
    events: IntCounterVec,
}

impl CachedAuthorizer {
    pub(crate) fn new(
        inner: Arc<dyn Authorizer>,
        coordinator: PgPool,
        events: IntCounterVec,
    ) -> Self {
        Self {
            inner,
            coordinator,
            decisions: Cache::builder()
                .max_capacity(10_000)
                .time_to_live(Duration::from_secs(2))
                .build(),
            events,
        }
    }

    async fn revision(&self) -> Result<Revision, AuthError> {
        let (generation, pending) =
            sqlx::query_as("SELECT generation, pending FROM authz_cache_state WHERE singleton")
                .fetch_one(&self.coordinator)
                .await
                .map_err(|_| AuthError)?;
        Ok(Revision {
            generation,
            pending,
        })
    }

    async fn mutate(
        &self,
        operation: impl Future<Output = Result<(), AuthError>> + Send,
    ) -> Result<(), AuthError> {
        // These statements commit independently of the caller's domain transaction.
        // Persist the fence before polling the remote operation. Cancellation
        // or an ambiguous response leaves it in place on every replica.
        sqlx::query_scalar::<_, i64>(
            "UPDATE authz_cache_state SET generation = generation + 1, pending = pending + 1 WHERE singleton RETURNING generation",
        )
        .fetch_one(&self.coordinator)
        .await
        .map_err(|_| AuthError)?;
        if operation.await.is_err() {
            tracing::warn!("authorization mutation outcome uncertain; decision cache bypassed");
            return Err(AuthError);
        }
        sqlx::query_scalar::<_, i64>(
            "UPDATE authz_cache_state SET generation = generation + 1, pending = pending - 1 WHERE singleton AND pending > 0 RETURNING generation",
        )
        .fetch_one(&self.coordinator)
        .await
        .map_err(|_| AuthError)?;
        Ok(())
    }
}

#[async_trait]
impl Authorizer for CachedAuthorizer {
    #[tracing::instrument(skip(self), name = "authz_cache.ensure_available")]
    async fn ensure_available(&self) -> Result<(), AuthError> {
        self.inner.ensure_available().await
    }

    async fn ensure_ready(&self) -> Result<(), AuthError> {
        self.inner.ensure_ready().await
    }

    #[tracing::instrument(skip(self, user), fields(relation = %relation, object_type = %object_type), name = "authz_cache.list_objects")]
    async fn list_objects(
        &self,
        user: &str,
        relation: &str,
        object_type: &str,
    ) -> Result<Vec<String>, AuthError> {
        self.inner.list_objects(user, relation, object_type).await
    }

    async fn check(&self, user: &str, relation: &str, object: &str) -> Result<bool, AuthError> {
        for _ in 0..3 {
            let before = self.revision().await?;
            let key = Key {
                generation: before.generation,
                user: user.into(),
                relation: relation.into(),
                object: object.into(),
            };
            let hit = if before.pending == 0 {
                self.decisions.get(&key).await
            } else {
                None
            };
            let allowed = match hit {
                // A cached allow still requires live backend readiness. An
                // unreachable OpenFGA must not turn a warm cache into a bypass.
                Some(true) => {
                    self.ensure_available().await?;
                    true
                }
                Some(false) => false,
                None => self.inner.check(user, relation, object).await?,
            };
            // This read is the decision's linearization point. A late fill
            // after it can only populate the old generation's key.
            if before != self.revision().await? {
                self.events.with_label_values(&["retry"]).inc();
                continue;
            }
            let event = match (hit, before.pending) {
                (Some(true), _) => "hit_allow",
                (Some(false), _) => "hit_deny",
                (None, 0) => {
                    self.decisions.insert(key, allowed).await;
                    "miss"
                }
                (None, _) => "bypass_pending",
            };
            self.events.with_label_values(&[event]).inc();
            return Ok(allowed);
        }
        Err(AuthError)
    }

    #[tracing::instrument(skip(self, tuples), fields(count = tuples.len(), add), name = "authz_cache.write")]
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
        self.mutate(self.inner.change(writes, deletes)).await
    }

    #[tracing::instrument(skip(self), fields(object = %object), name = "authz_cache.delete_object")]
    async fn delete_object(&self, object: &str) -> Result<(), AuthError> {
        self.mutate(self.inner.delete_object(object)).await
    }
}
