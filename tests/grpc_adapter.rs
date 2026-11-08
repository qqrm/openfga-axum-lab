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
    GetStoreResponse, ReadAuthorizationModelRequest, ReadAuthorizationModelResponse, ReadRequest,
    ReadResponse, Tuple as FgaTuple, TupleKey, WriteRequest, WriteResponse,
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
        "GetStore" => frame(GetStoreResponse {
            id: "store".to_owned(),
            ..Default::default()
        }),
        "ReadAuthorizationModel" => frame(ReadAuthorizationModelResponse {
            authorization_model: Some(AuthorizationModel {
                id: "model".to_owned(),
                ..Default::default()
            }),
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
            relation: "ownership".to_owned(),
            object: object.to_owned(),
            condition: None,
        }),
        timestamp: None,
    }
}

#[tokio::test]
async fn grpc_requests_keep_strong_consistency_and_idempotency() -> TestResult {
    let stub = Stub::start(Vec::new(), false).await?;
    let auth = OpenFgaGrpc::connect(stub.url.clone(), "store", "model").await?;
    auth.ensure_available().await?;
    assert!(
        auth.check(
            "alice",
            "can_read_data",
            "lakekeeper_table:analytics/example"
        )
        .await?
    );
    let tuples = [Tuple {
        user: "user:alice".to_owned(),
        relation: "ownership".to_owned(),
        object: "lakekeeper_table:analytics/example".to_owned(),
    }];
    auth.write(&tuples, true).await?;
    auth.write(&tuples, false).await?;
    auth.apply(
        &[Tuple {
            user: "user:bob".into(),
            ..tuples[0].clone()
        }],
        &tuples,
    )
    .await?;
    auth.apply(&[], &[]).await?;

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
    assert_eq!(writes.len(), 3, "mixed changes require one atomic Write");
    assert_eq!(writes[0].writes.as_ref().unwrap().on_duplicate, "ignore");
    assert!(writes[0].deletes.is_none());
    assert_eq!(writes[1].deletes.as_ref().unwrap().on_missing, "ignore");
    assert!(writes[1].writes.is_none());
    let replacement = &writes[2];
    let granted = replacement.writes.as_ref().unwrap();
    let revoked = replacement.deletes.as_ref().unwrap();
    assert_eq!(granted.on_duplicate, "ignore");
    assert_eq!(revoked.on_missing, "ignore");
    assert_eq!(granted.tuple_keys.len(), 1);
    assert_eq!(revoked.tuple_keys.len(), 1);
    assert_eq!(granted.tuple_keys[0].user, "user:bob");
    assert_eq!(revoked.tuple_keys[0].user, "user:alice");
    assert_eq!(granted.tuple_keys[0].object, revoked.tuple_keys[0].object);
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
            tuples: vec![fga_tuple("lakekeeper_table:analytics/example")],
            continuation_token: "cursor-before-deletion".to_owned(),
        },
        ReadResponse::default(),
    ];
    let stub = Stub::start(pages, false).await?;
    let auth = OpenFgaGrpc::connect(stub.url.clone(), "store", "model").await?;
    auth.delete_object("lakekeeper_table:analytics/example")
        .await?;
    let reads = stub.requests::<ReadRequest>("Read").await?;
    assert_eq!(reads.len(), 2);
    for read in reads {
        assert_eq!(read.consistency(), ConsistencyPreference::HigherConsistency);
        assert_eq!(read.page_size, Some(100));
        assert!(read.continuation_token.is_empty());
        assert_eq!(
            read.tuple_key.as_ref().unwrap().object,
            "lakekeeper_table:analytics/example"
        );
    }
    let writes = stub.requests::<WriteRequest>("Write").await?;
    assert_eq!(writes.len(), 1);
    let deletes = writes[0].deletes.as_ref().unwrap();
    assert_eq!(deletes.on_missing, "ignore");
    assert_eq!(deletes.tuple_keys.len(), 1);
    assert_eq!(
        deletes.tuple_keys[0].object,
        "lakekeeper_table:analytics/example"
    );
    Ok(())
}

#[tokio::test]
async fn deletion_rejects_malformed_or_unrelated_tuples_before_writing() -> TestResult {
    for invalid in [
        FgaTuple::default(),
        fga_tuple("lakekeeper_table:analytics/unrelated"),
    ] {
        let stub = Stub::start(
            vec![ReadResponse {
                tuples: vec![fga_tuple("lakekeeper_table:analytics/example"), invalid],
                ..Default::default()
            }],
            false,
        )
        .await?;
        let auth = OpenFgaGrpc::connect(stub.url.clone(), "store", "model").await?;
        assert!(
            auth.delete_object("lakekeeper_table:analytics/example")
                .await
                .is_err()
        );
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
        auth.check(
            "alice",
            "can_read_data",
            "lakekeeper_table:analytics/example",
        ),
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
