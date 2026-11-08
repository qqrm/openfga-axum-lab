#![cfg(feature = "grpc")]

use axum::{
    Router,
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, HeaderValue, Uri},
    response::Response,
};
use http_body_util::{BodyExt, Full};
use openfga_axum_lab::{
    application::{Authorizer, Tuple},
    infrastructure::OpenFgaGrpc,
};
use openfga_client::client::{
    AuthorizationModel, CheckRequest, CheckResponse, ConsistencyPreference, GetStoreRequest,
    GetStoreResponse, ListObjectsRequest, ListObjectsResponse, ReadAuthorizationModelRequest,
    ReadAuthorizationModelResponse, ReadRequest, ReadResponse, Tuple as FgaTuple, TupleKey,
    WriteRequest, WriteResponse,
};
use prost::Message;
use std::{collections::VecDeque, convert::Infallible, sync::Arc, time::Duration};
use tokio::{net::TcpListener, sync::Mutex, task::JoinHandle};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Default)]
struct WireState {
    requests: Mutex<Vec<(String, Bytes)>>,
    reads: Mutex<VecDeque<ReadResponse>>,
    stall_trailers: bool,
    fail_operation: Mutex<Option<&'static str>>,
    store_response: Mutex<Option<GetStoreResponse>>,
    model_response: Mutex<Option<ReadAuthorizationModelResponse>>,
}

struct Stub {
    url: String,
    state: Arc<WireState>,
    task: JoinHandle<std::io::Result<()>>,
}

impl Stub {
    async fn start(reads: Vec<ReadResponse>, stall_trailers: bool) -> TestResult<Self> {
        let state = Arc::new(WireState {
            reads: Mutex::new(reads.into()),
            stall_trailers,
            ..Default::default()
        });
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let router = Router::new().fallback(reply).with_state(state.clone());
        let task = tokio::spawn(async move { axum::serve(listener, router).await });
        Ok(Self { url, state, task })
    }

    async fn requests<T: Message + Default>(&self, operation: &str) -> TestResult<Vec<T>> {
        self.state
            .requests
            .lock()
            .await
            .iter()
            .filter(|(path, _)| path.rsplit('/').next() == Some(operation))
            .map(|(_, body)| {
                assert_eq!(body[0], 0, "test requests must not be compressed");
                let length = u32::from_be_bytes(body[1..5].try_into()?) as usize;
                assert_eq!(body.len(), 5 + length, "expected one unary message");
                Ok(T::decode(&body[5..])?)
            })
            .collect()
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn frame(message: impl Message) -> Bytes {
    let bytes = message.encode_to_vec();
    let mut frame = Vec::with_capacity(5 + bytes.len());
    frame.push(0);
    frame.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    frame.extend_from_slice(&bytes);
    frame.into()
}

async fn reply(State(state): State<Arc<WireState>>, uri: Uri, body: Bytes) -> Response {
    state
        .requests
        .lock()
        .await
        .push((uri.path().to_owned(), body));
    let operation = uri.path().rsplit('/').next().unwrap_or("");
    let fail = *state.fail_operation.lock().await == Some(operation);
    let bytes = match operation {
        "Check" => frame(CheckResponse {
            allowed: true,
            ..Default::default()
        }),
        "GetStore" => frame(
            state
                .store_response
                .lock()
                .await
                .clone()
                .unwrap_or_else(|| GetStoreResponse {
                    id: "store".to_owned(),
                    ..Default::default()
                }),
        ),
        "ReadAuthorizationModel" => frame(
            state
                .model_response
                .lock()
                .await
                .clone()
                .unwrap_or_else(|| ReadAuthorizationModelResponse {
                    authorization_model: Some(AuthorizationModel {
                        id: "model".to_owned(),
                        ..Default::default()
                    }),
                }),
        ),
        "ListObjects" => frame(ListObjectsResponse {
            objects: vec![
                "task:access-review".to_owned(),
                "task:vendor-review".to_owned(),
            ],
        }),
        "Write" => frame(WriteResponse {}),
        "Read" => frame(state.reads.lock().await.pop_front().unwrap_or_default()),
        _ => panic!("unexpected RPC {}", uri.path()),
    };
    let body = Full::new(bytes).with_trailers(async move {
        if state.stall_trailers {
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
        let mut trailers = HeaderMap::new();
        trailers.insert(
            "grpc-status",
            HeaderValue::from_static(if fail { "5" } else { "0" }),
        );
        Some(Ok::<_, Infallible>(trailers))
    });
    Response::builder()
        .header("content-type", "application/grpc")
        .body(Body::new(body))
        .unwrap()
}

fn fga_tuple(object: &str) -> FgaTuple {
    FgaTuple {
        key: Some(TupleKey {
            user: "user:alice".to_owned(),
            relation: "assigner".to_owned(),
            object: object.to_owned(),
            condition: None,
        }),
        timestamp: None,
    }
}

fn owner(user: &str) -> Tuple {
    Tuple {
        user: format!("user:{user}"),
        relation: "owner".to_owned(),
        object: "organization:acme".to_owned(),
    }
}

fn assert_owner_transfer(request: &WriteRequest) {
    assert_eq!(request.store_id, "store");
    assert_eq!(request.authorization_model_id, "model");
    let writes = request.writes.as_ref().expect("new owner in same Write");
    let deletes = request.deletes.as_ref().expect("old owner in same Write");
    assert_eq!(writes.on_duplicate, "ignore");
    assert_eq!(deletes.on_missing, "ignore");
    assert_eq!(writes.tuple_keys.len(), 1);
    assert_eq!(deletes.tuple_keys.len(), 1);
    assert_eq!(writes.tuple_keys[0].user, "user:carol");
    assert_eq!(deletes.tuple_keys[0].user, "user:alice");
    assert_eq!(writes.tuple_keys[0].relation, "owner");
    assert_eq!(deletes.tuple_keys[0].relation, "owner");
    assert_eq!(writes.tuple_keys[0].object, "organization:acme");
    assert_eq!(deletes.tuple_keys[0].object, "organization:acme");
    assert!(writes.tuple_keys[0].condition.is_none());
}

#[tokio::test]
async fn ownership_transfer_is_one_atomic_idempotent_write() -> TestResult {
    let stub = Stub::start(Vec::new(), false).await?;
    let auth = OpenFgaGrpc::connect(stub.url.clone(), "store", "model").await?;
    auth.apply(&[owner("carol")], &[owner("alice")]).await?;
    let requests = stub.requests::<WriteRequest>("Write").await?;
    assert_eq!(requests.len(), 1, "ownership transfer must be one RPC");
    assert_owner_transfer(&requests[0]);
    assert_eq!(stub.state.requests.lock().await.len(), 1);
    Ok(())
}

#[tokio::test]
async fn empty_changes_do_not_send_an_invalid_write() -> TestResult {
    let stub = Stub::start(Vec::new(), false).await?;
    let auth = OpenFgaGrpc::connect(stub.url.clone(), "store", "model").await?;
    auth.apply(&[], &[]).await?;
    auth.write(&[], true).await?;
    auth.write(&[], false).await?;
    assert!(stub.state.requests.lock().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn ownership_transfer_rejects_rpc_failure_without_retrying() -> TestResult {
    let stub = Stub::start(Vec::new(), false).await?;
    *stub.state.fail_operation.lock().await = Some("Write");
    let auth = OpenFgaGrpc::connect(stub.url.clone(), "store", "model").await?;
    assert!(
        auth.apply(&[owner("carol")], &[owner("alice")])
            .await
            .is_err()
    );
    let requests = stub.requests::<WriteRequest>("Write").await?;
    assert_eq!(requests.len(), 1, "failed mutations must not be retried");
    assert_owner_transfer(&requests[0]);
    Ok(())
}

#[tokio::test]
async fn ownership_transfer_rejects_stalled_trailers_without_retrying() -> TestResult {
    let stub = Stub::start(Vec::new(), true).await?;
    let auth = OpenFgaGrpc::connect(stub.url.clone(), "store", "model").await?;
    let writes = [owner("carol")];
    let deletes = [owner("alice")];
    let result = tokio::time::timeout(Duration::from_secs(3), auth.apply(&writes, &deletes))
        .await
        .expect("mutation deadline must include response trailers");
    assert!(result.is_err(), "unacknowledged transfer must not succeed");
    let requests = stub.requests::<WriteRequest>("Write").await?;
    assert_eq!(requests.len(), 1, "ambiguous mutations must not be retried");
    assert_owner_transfer(&requests[0]);
    Ok(())
}

#[tokio::test]
async fn grpc_requests_keep_strong_consistency_and_idempotency() -> TestResult {
    let stub = Stub::start(Vec::new(), false).await?;
    let auth = OpenFgaGrpc::connect(stub.url.clone(), "store", "model").await?;
    auth.ensure_available().await?;
    assert!(auth.check("alice", "can_view", "organization:acme").await?);
    let tuples = [Tuple {
        user: "user:alice".to_owned(),
        relation: "owner".to_owned(),
        object: "organization:acme".to_owned(),
    }];
    auth.write(&tuples, true).await?;
    auth.write(&tuples, false).await?;

    let health = stub.requests::<GetStoreRequest>("GetStore").await?;
    assert_eq!(health.len(), 1);
    assert_eq!(health[0].store_id, "store");
    let checks = stub.requests::<CheckRequest>("Check").await?;
    assert_eq!(checks.len(), 1);
    assert_eq!(
        checks[0].consistency(),
        ConsistencyPreference::HigherConsistency
    );
    assert_eq!(checks[0].store_id, "store");
    assert_eq!(checks[0].authorization_model_id, "model");
    assert_eq!(checks[0].tuple_key.as_ref().unwrap().user, "user:alice");
    let writes = stub.requests::<WriteRequest>("Write").await?;
    assert_eq!(writes.len(), 2);
    assert_eq!(writes[0].writes.as_ref().unwrap().on_duplicate, "ignore");
    assert!(writes[0].deletes.is_none());
    assert_eq!(writes[1].deletes.as_ref().unwrap().on_missing, "ignore");
    assert!(writes[1].writes.is_none());
    for write in writes {
        assert_eq!(write.store_id, "store");
        assert_eq!(write.authorization_model_id, "model");
    }
    Ok(())
}

#[tokio::test]
async fn deletion_restarts_first_page_after_each_write() -> TestResult {
    let pages = vec![
        ReadResponse {
            tuples: vec![fga_tuple("task:vendor-review")],
            continuation_token: "cursor-before-deletion".to_owned(),
        },
        ReadResponse::default(),
    ];
    let stub = Stub::start(pages, false).await?;
    let auth = OpenFgaGrpc::connect(stub.url.clone(), "store", "model").await?;
    auth.delete_object("task:vendor-review").await?;
    let reads = stub.requests::<ReadRequest>("Read").await?;
    assert_eq!(reads.len(), 2);
    for read in reads {
        assert_eq!(read.consistency(), ConsistencyPreference::HigherConsistency);
        assert_eq!(read.page_size, Some(100));
        assert!(read.continuation_token.is_empty());
        assert_eq!(
            read.tuple_key.as_ref().unwrap().object,
            "task:vendor-review"
        );
    }
    let writes = stub.requests::<WriteRequest>("Write").await?;
    assert_eq!(writes.len(), 1);
    let deletes = writes[0].deletes.as_ref().unwrap();
    assert_eq!(deletes.on_missing, "ignore");
    assert_eq!(deletes.tuple_keys.len(), 1);
    assert_eq!(deletes.tuple_keys[0].object, "task:vendor-review");
    Ok(())
}

#[tokio::test]
async fn deletion_rejects_malformed_or_unrelated_tuples_before_writing() -> TestResult {
    let mut empty_user = fga_tuple("task:vendor-review");
    empty_user.key.as_mut().unwrap().user.clear();
    let mut empty_relation = fga_tuple("task:vendor-review");
    empty_relation.key.as_mut().unwrap().relation.clear();
    for invalid in [
        FgaTuple::default(),
        fga_tuple("task:unrelated"),
        empty_user,
        empty_relation,
    ] {
        let stub = Stub::start(
            vec![ReadResponse {
                tuples: vec![fga_tuple("task:vendor-review"), invalid],
                ..Default::default()
            }],
            false,
        )
        .await?;
        let auth = OpenFgaGrpc::connect(stub.url.clone(), "store", "model").await?;
        assert!(auth.delete_object("task:vendor-review").await.is_err());
        assert!(stub.requests::<WriteRequest>("Write").await?.is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn grpc_timeout_covers_stalled_response_trailers() -> TestResult {
    let stub = Stub::start(Vec::new(), true).await?;
    let auth = OpenFgaGrpc::connect(stub.url.clone(), "store", "model").await?;
    let response = tokio::time::timeout(
        Duration::from_secs(3),
        auth.check("alice", "can_view", "task:vendor-review"),
    )
    .await
    .expect("adapter must bound the full response, not only the headers");
    assert!(response.is_err(), "missing gRPC trailers must fail closed");
    Ok(())
}

#[tokio::test]
async fn readiness_probes_store_and_model_without_cached_success() -> TestResult {
    let stub = Stub::start(Vec::new(), false).await?;
    let auth = OpenFgaGrpc::connect(stub.url.clone(), "store", "model").await?;
    auth.ensure_ready().await?;
    let stores = stub.requests::<GetStoreRequest>("GetStore").await?;
    assert_eq!(stores.len(), 1);
    assert_eq!(stores[0].store_id, "store");
    let models = stub
        .requests::<ReadAuthorizationModelRequest>("ReadAuthorizationModel")
        .await?;
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].store_id, "store");
    assert_eq!(models[0].id, "model");

    *stub.state.fail_operation.lock().await = Some("ReadAuthorizationModel");
    assert!(auth.ensure_ready().await.is_err());
    assert_eq!(stub.requests::<GetStoreRequest>("GetStore").await?.len(), 2);
    assert_eq!(
        stub.requests::<ReadAuthorizationModelRequest>("ReadAuthorizationModel")
            .await?
            .len(),
        2
    );

    *stub.state.fail_operation.lock().await = Some("GetStore");
    assert!(auth.ensure_ready().await.is_err());
    assert_eq!(stub.requests::<GetStoreRequest>("GetStore").await?.len(), 3);
    assert_eq!(
        stub.requests::<ReadAuthorizationModelRequest>("ReadAuthorizationModel")
            .await?
            .len(),
        2,
        "an unavailable store must fail even if its model remains readable"
    );
    assert!(stub.requests::<CheckRequest>("Check").await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn list_objects_pins_model_and_strong_consistency() -> TestResult {
    let stub = Stub::start(Vec::new(), false).await?;
    let auth = OpenFgaGrpc::connect(stub.url.clone(), "store", "model").await?;
    assert_eq!(
        auth.list_objects("alice", "can_view", "task").await?,
        ["task:access-review", "task:vendor-review"]
    );
    let requests = stub.requests::<ListObjectsRequest>("ListObjects").await?;
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.store_id, "store");
    assert_eq!(request.authorization_model_id, "model");
    assert_eq!(request.user, "user:alice");
    assert_eq!(request.relation, "can_view");
    assert_eq!(request.r#type, "task");
    assert_eq!(
        request.consistency,
        ConsistencyPreference::HigherConsistency as i32
    );
    assert!(request.contextual_tuples.is_none());
    assert!(request.context.is_none());
    Ok(())
}

#[tokio::test]
async fn list_objects_rejects_rpc_failure_without_retrying() -> TestResult {
    let stub = Stub::start(Vec::new(), false).await?;
    *stub.state.fail_operation.lock().await = Some("ListObjects");
    let auth = OpenFgaGrpc::connect(stub.url.clone(), "store", "model").await?;
    assert!(
        auth.list_objects("alice", "can_view", "task")
            .await
            .is_err()
    );
    assert_eq!(
        stub.requests::<ListObjectsRequest>("ListObjects")
            .await?
            .len(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn list_objects_does_not_return_objects_before_successful_trailers() -> TestResult {
    let stub = Stub::start(Vec::new(), true).await?;
    let auth = OpenFgaGrpc::connect(stub.url.clone(), "store", "model").await?;
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        auth.list_objects("alice", "can_view", "task"),
    )
    .await
    .expect("list deadline must include response trailers");
    assert!(
        result.is_err(),
        "an object payload alone is not a completed RPC"
    );
    assert_eq!(
        stub.requests::<ListObjectsRequest>("ListObjects")
            .await?
            .len(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn readiness_rejects_store_or_model_identity_mismatches() -> TestResult {
    let stub = Stub::start(Vec::new(), false).await?;
    let auth = OpenFgaGrpc::connect(stub.url.clone(), "store", "model").await?;
    *stub.state.store_response.lock().await = Some(GetStoreResponse {
        id: "another-store".into(),
        ..Default::default()
    });
    assert!(auth.ensure_available().await.is_err());
    assert!(auth.ensure_ready().await.is_err());
    *stub.state.store_response.lock().await = None;
    for model in [
        None,
        Some(AuthorizationModel {
            id: "another-model".into(),
            ..Default::default()
        }),
    ] {
        *stub.state.model_response.lock().await = Some(ReadAuthorizationModelResponse {
            authorization_model: model,
        });
        assert!(auth.ensure_ready().await.is_err());
    }
    *stub.state.model_response.lock().await = None;
    auth.ensure_ready().await?;
    Ok(())
}

#[tokio::test]
async fn positive_check_does_not_replace_live_store_availability() -> TestResult {
    let stub = Stub::start(Vec::new(), false).await?;
    let auth = OpenFgaGrpc::connect(stub.url.clone(), "store", "model").await?;
    auth.ensure_available().await?;
    assert!(
        auth.check(
            "alice",
            "can_get_metadata",
            "lakekeeper_table:warehouse/table"
        )
        .await?
    );
    // The server still returns an allow payload, while GetStore ends in
    // NOT_FOUND trailers. Earlier successful probes must never mask this.
    *stub.state.fail_operation.lock().await = Some("GetStore");
    assert!(
        auth.check(
            "alice",
            "can_get_metadata",
            "lakekeeper_table:warehouse/table"
        )
        .await?
    );
    assert!(auth.ensure_available().await.is_err());
    assert_eq!(stub.requests::<CheckRequest>("Check").await?.len(), 2);
    assert_eq!(stub.requests::<GetStoreRequest>("GetStore").await?.len(), 2);
    assert!(
        stub.requests::<ReadAuthorizationModelRequest>("ReadAuthorizationModel")
            .await?
            .is_empty()
    );
    Ok(())
}
