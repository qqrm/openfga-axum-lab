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
    CheckRequest, CheckResponse, ConsistencyPreference, GetStoreRequest, GetStoreResponse,
    ReadRequest, ReadResponse, Tuple as FgaTuple, TupleKey, WriteRequest, WriteResponse,
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
    let bytes = match uri.path().rsplit('/').next().unwrap_or("") {
        "Check" => frame(CheckResponse {
            allowed: true,
            ..Default::default()
        }),
        "GetStore" => frame(GetStoreResponse {
            id: "store".to_owned(),
            ..Default::default()
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
        trailers.insert("grpc-status", HeaderValue::from_static("0"));
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
            relation: "owner".to_owned(),
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
    assert!(auth.check("alice", "can_view", "pet:cat").await?);
    let tuples = [Tuple {
        user: "user:alice".to_owned(),
        relation: "owner".to_owned(),
        object: "pet:cat".to_owned(),
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
            tuples: vec![fga_tuple("pet:cat")],
            continuation_token: "cursor-before-deletion".to_owned(),
        },
        ReadResponse::default(),
    ];
    let stub = Stub::start(pages, false).await?;
    let auth = OpenFgaGrpc::connect(stub.url.clone(), "store", "model").await?;
    auth.delete_object("pet:cat").await?;
    let reads = stub.requests::<ReadRequest>("Read").await?;
    assert_eq!(reads.len(), 2);
    for read in reads {
        assert_eq!(read.consistency(), ConsistencyPreference::HigherConsistency);
        assert_eq!(read.page_size, Some(100));
        assert!(read.continuation_token.is_empty());
        assert_eq!(read.tuple_key.as_ref().unwrap().object, "pet:cat");
    }
    let writes = stub.requests::<WriteRequest>("Write").await?;
    assert_eq!(writes.len(), 1);
    let deletes = writes[0].deletes.as_ref().unwrap();
    assert_eq!(deletes.on_missing, "ignore");
    assert_eq!(deletes.tuple_keys.len(), 1);
    assert_eq!(deletes.tuple_keys[0].object, "pet:cat");
    Ok(())
}

#[tokio::test]
async fn deletion_rejects_malformed_or_unrelated_tuples_before_writing() -> TestResult {
    for invalid in [FgaTuple::default(), fga_tuple("pet:unrelated")] {
        let stub = Stub::start(
            vec![ReadResponse {
                tuples: vec![fga_tuple("pet:cat"), invalid],
                ..Default::default()
            }],
            false,
        )
        .await?;
        let auth = OpenFgaGrpc::connect(stub.url.clone(), "store", "model").await?;
        assert!(auth.delete_object("pet:cat").await.is_err());
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
        auth.check("alice", "can_view", "pet:cat"),
    )
    .await
    .expect("adapter must bound the full response, not only the headers");
    assert!(response.is_err(), "missing gRPC trailers must fail closed");
    Ok(())
}
