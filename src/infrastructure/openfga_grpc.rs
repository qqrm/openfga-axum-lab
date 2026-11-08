use crate::application::{AuthError, Authorizer, Tuple};
use async_trait::async_trait;
use openfga_client::{
    client::{
        CheckRequestTupleKey, ConsistencyPreference, GetStoreRequest, OpenFgaClient,
        OpenFgaServiceClient, ReadRequestTupleKey, TupleKey, TupleKeyWithoutCondition,
        WriteOptions,
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
        // Cache hits must still prove that the real authorization store responds.
        bounded(self.client.client().get_store(GetStoreRequest {
            store_id: self.client.store_id().to_owned(),
        }))
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
        let writes = add.then(|| {
            tuples
                .iter()
                .map(|tuple| TupleKey {
                    user: tuple.user.clone(),
                    relation: tuple.relation.clone(),
                    object: tuple.object.clone(),
                    condition: None,
                })
                .collect()
        });
        let deletes = (!add).then(|| {
            tuples
                .iter()
                .map(|tuple| TupleKeyWithoutCondition {
                    user: tuple.user.clone(),
                    relation: tuple.relation.clone(),
                    object: tuple.object.clone(),
                })
                .collect()
        });
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
