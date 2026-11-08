//! A denial cache coordinated by the shared PostgreSQL primary.
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
    denials: Cache<Key, ()>,
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
            denials: Cache::builder()
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

    async fn unfenced_revision(&self) -> Result<Revision, AuthError> {
        let revision = self.revision().await?;
        if revision.pending != 0 {
            self.events.with_label_values(&["blocked_pending"]).inc();
            tracing::warn!("authorization mutation is unresolved; access blocked");
            return Err(AuthError);
        }
        Ok(revision)
    }

    async fn mutate(
        &self,
        operation: impl Future<Output = Result<(), AuthError>> + Send,
    ) -> Result<(), AuthError> {
        // These statements commit independently of the caller's transaction.
        // Persist the fence before polling the remote operation. Cancellation
        // or an ambiguous response leaves all replicas blocked until explicit
        // reconciliation. A later successful write only clears its own marker.
        sqlx::query_scalar::<_, i64>(
            "UPDATE authz_cache_state SET generation = generation + 1, pending = pending + 1 WHERE singleton RETURNING generation",
        )
        .fetch_one(&self.coordinator)
        .await
        .map_err(|_| AuthError)?;
        if operation.await.is_err() {
            tracing::warn!("authorization mutation outcome uncertain; access remains blocked");
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
        self.unfenced_revision().await?;
        self.inner.ensure_available().await?;
        self.unfenced_revision().await?;
        Ok(())
    }

    async fn ensure_ready(&self) -> Result<(), AuthError> {
        self.unfenced_revision().await?;
        self.inner.ensure_ready().await?;
        self.unfenced_revision().await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, user), fields(relation = %relation, object_type = %object_type), name = "authz_cache.list_objects")]
    async fn list_objects(
        &self,
        user: &str,
        relation: &str,
        object_type: &str,
    ) -> Result<Vec<String>, AuthError> {
        for _ in 0..3 {
            let before = self.unfenced_revision().await?;
            let objects = self.inner.list_objects(user, relation, object_type).await?;
            // Even an empty enumeration must not cross a pending write or an
            // acknowledged grant/revoke. Catalog endpoints still enforce SQL
            // scope and lifecycle using their locked per-object Checks.
            if before == self.unfenced_revision().await? {
                return Ok(objects);
            }
            self.events.with_label_values(&["retry"]).inc();
        }
        Err(AuthError)
    }

    #[tracing::instrument(skip(self, user, object), fields(relation = %relation), name = "authz_cache.check")]
    async fn check(&self, user: &str, relation: &str, object: &str) -> Result<bool, AuthError> {
        for _ in 0..3 {
            let before = self.unfenced_revision().await?;
            let key = Key {
                generation: before.generation,
                user: user.into(),
                relation: relation.into(),
                object: object.into(),
            };
            let hit = self.denials.get(&key).await.is_some();
            // A healthy probe is not evidence that Check succeeded. Never cache
            // permission to access an object: every allow needs a live Check.
            let allowed = if hit {
                false
            } else {
                self.inner.check(user, relation, object).await?
            };
            if allowed {
                // OpenFGA can still answer Check after a soft-deleted store.
                // An allow also requires a live configured store; a health
                // probe alone or a prior successful Check cannot replace it.
                self.inner.ensure_available().await?;
            }
            // This read is the decision's linearization point. A late denial
            // fill can only populate the old generation's key. Pending writes
            // block immediately, including writes with an ambiguous outcome.
            if before != self.unfenced_revision().await? {
                self.events.with_label_values(&["retry"]).inc();
                continue;
            }
            let event = if hit {
                "hit_deny"
            } else {
                if !allowed {
                    self.denials.insert(key, ()).await;
                }
                "miss"
            };
            self.events.with_label_values(&[event]).inc();
            return Ok(allowed);
        }
        Err(AuthError)
    }

    #[tracing::instrument(skip(self, tuples), fields(count = tuples.len(), add), name = "authz_cache.write")]
    async fn write(&self, tuples: &[Tuple], add: bool) -> Result<(), AuthError> {
        if add {
            self.apply(tuples, &[]).await
        } else {
            self.apply(&[], tuples).await
        }
    }

    #[tracing::instrument(skip(self, writes, deletes), fields(writes = writes.len(), deletes = deletes.len()), name = "authz_cache.apply")]
    async fn apply(&self, writes: &[Tuple], deletes: &[Tuple]) -> Result<(), AuthError> {
        if writes.is_empty() && deletes.is_empty() {
            return Ok(());
        }
        self.mutate(self.inner.apply(writes, deletes)).await
    }

    #[tracing::instrument(skip(self), fields(object = %object), name = "authz_cache.delete_object")]
    async fn delete_object(&self, object: &str) -> Result<(), AuthError> {
        self.mutate(self.inner.delete_object(object)).await
    }
}
