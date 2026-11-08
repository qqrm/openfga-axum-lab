use crate::application::{AuthError, Authorizer, Tuple};
use async_trait::async_trait;
use openfga_client::{
    client::{
        CheckRequestTupleKey, ConsistencyPreference, GetStoreRequest, OpenFgaClient,
        OpenFgaServiceClient, ReadAuthorizationModelRequest, ReadRequestTupleKey, TupleKey,
        TupleKeyWithoutCondition, WriteOptions,
    },
    tonic::transport::{Channel, Endpoint, Error},
};
use std::{future::Future, time::Duration};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// Native gRPC adapter with the same consistency and retry contract as HTTP.
pub struct OpenFgaGrpc {
    client: OpenFgaClient<Channel>,
}

impl OpenFgaGrpc {
    pub async fn connect(url: String, store: &str, model: &str) -> Result<Self, Error> {
        let channel = Endpoint::from_shared(url)?
            .connect_timeout(REQUEST_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .connect()
            .await?;
        let client = OpenFgaClient::new(OpenFgaServiceClient::new(channel), store, model)
            .set_consistency(ConsistencyPreference::HigherConsistency);
        Ok(Self { client })
    }

    /// Raw backend enumeration for diagnostics, outside the catalog contract.
    /// This does not enforce SQL scope, migration activation or mutation fences.
    /// Catalog responses must continue to use their locked per-object Check.
    pub async fn list_objects(
        &self,
        user: &str,
        relation: &str,
        object_type: &str,
    ) -> Result<Vec<String>, AuthError> {
        let response = bounded(self.client.list_objects(
            object_type.to_owned(),
            relation.to_owned(),
            format!("user:{user}"),
            None,
            None,
        ))
        .await?
        .into_inner();
        Ok(response.objects)
    }
}

// Bound the complete unary response, including its body and gRPC trailers.
// The transport's request timeout alone does not cover a stalled response body.
async fn bounded<T, E>(request: impl Future<Output = Result<T, E>>) -> Result<T, AuthError> {
    tokio::time::timeout(REQUEST_TIMEOUT, request)
        .await
        .map_err(|_| AuthError)?
        .map_err(|_| AuthError)
}

#[async_trait]
impl Authorizer for OpenFgaGrpc {
    #[tracing::instrument(skip(self), name = "openfga.ensure_available")]
    async fn ensure_available(&self) -> Result<(), AuthError> {
        // Operational health must prove that the configured store responds.
        bounded(self.client.client().get_store(GetStoreRequest {
            store_id: self.client.store_id().to_owned(),
        }))
        .await?;
        Ok(())
    }

    async fn ensure_ready(&self) -> Result<(), AuthError> {
        self.ensure_available().await?;
        bounded(
            self.client
                .client()
                .read_authorization_model(ReadAuthorizationModelRequest {
                    store_id: self.client.store_id().to_owned(),
                    id: self.client.authorization_model_id().to_owned(),
                }),
        )
        .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, user, object), fields(relation = %relation), name = "openfga.check")]
    async fn check(&self, user: &str, relation: &str, object: &str) -> Result<bool, AuthError> {
        bounded(self.client.check_simple(CheckRequestTupleKey {
            user: format!("user:{user}"),
            relation: relation.to_owned(),
            object: object.to_owned(),
        }))
        .await
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
        let writes = (!writes.is_empty()).then(|| {
            writes
                .iter()
                .map(|tuple| TupleKey {
                    user: tuple.user.clone(),
                    relation: tuple.relation.clone(),
                    object: tuple.object.clone(),
                    condition: None,
                })
                .collect()
        });
        let deletes = (!deletes.is_empty()).then(|| {
            deletes
                .iter()
                .map(|tuple| TupleKeyWithoutCondition {
                    user: tuple.user.clone(),
                    relation: tuple.relation.clone(),
                    object: tuple.object.clone(),
                })
                .collect()
        });
        // write_with_options sends one unary Write and rejects an oversized
        // transaction; it never chunks a replacement into separate requests.
        bounded(
            self.client
                .write_with_options(writes, deletes, WriteOptions::new_idempotent()),
        )
        .await
    }

    #[tracing::instrument(skip(self), fields(object = %object), name = "openfga.delete_object")]
    async fn delete_object(&self, object: &str) -> Result<(), AuthError> {
        // Re-read the first page after each deletion, just like the HTTP adapter.
        // Pagination is not a snapshot while tuples are concurrently removed.
        for _ in 0..1000 {
            let page = bounded(self.client.read(
                100,
                ReadRequestTupleKey {
                    object: object.to_owned(),
                    ..Default::default()
                },
                None::<String>,
            ))
            .await?
            .into_inner();
            if page.tuples.is_empty() {
                return if page.continuation_token.is_empty() {
                    Ok(())
                } else {
                    Err(AuthError)
                };
            }
            let tuples: Result<Vec<_>, _> = page
                .tuples
                .into_iter()
                .map(|tuple| {
                    let key = tuple.key.ok_or(AuthError)?;
                    if key.object != object {
                        return Err(AuthError);
                    }
                    Ok(Tuple {
                        user: key.user,
                        relation: key.relation,
                        object: key.object,
                    })
                })
                .collect();
            self.write(&tuples?, false).await?;
        }
        Err(AuthError)
    }
}
