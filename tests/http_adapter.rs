#![cfg(feature = "http")]

use axum::{
    Router,
    body::{Body, Bytes},
    extract::State,
    http::{StatusCode, Uri},
    response::Response,
};
use openfga_axum_lab::{
    application::{Authorizer, Tuple},
    infrastructure::OpenFgaHttp,
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::{Mutex, oneshot},
    task::JoinHandle,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct Reply {
    status: StatusCode,
    content_type: Option<&'static str>,
    body: Vec<u8>,
    location: Option<&'static str>,
}

impl Reply {
    fn json(value: Value) -> Self {
        Self::raw(serde_json::to_vec(&value).unwrap())
    }

    fn raw(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: StatusCode::OK,
            content_type: Some("application/json; charset=utf-8"),
            body: body.into(),
            location: None,
        }
    }
}

#[derive(Default)]
struct WireState {
    requests: Mutex<Vec<(String, Value)>>,
    replies: Mutex<HashMap<String, VecDeque<Reply>>>,
}

struct Stub {
    url: String,
    state: Arc<WireState>,
    task: JoinHandle<std::io::Result<()>>,
}

impl Stub {
    async fn start() -> TestResult<Self> {
        let state = Arc::new(WireState::default());
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let router = Router::new().fallback(reply).with_state(state.clone());
        let task = tokio::spawn(async move { axum::serve(listener, router).await });
        Ok(Self { url, state, task })
    }

    async fn enqueue(&self, operation: &str, reply: Reply) {
        self.state
            .replies
            .lock()
            .await
            .entry(operation.to_owned())
            .or_default()
            .push_back(reply);
    }

    async fn requests(&self, operation: &str) -> Vec<Value> {
        self.state
            .requests
            .lock()
            .await
            .iter()
            .filter(|(path, _)| path.rsplit('/').next() == Some(operation))
            .map(|(_, body)| body.clone())
            .collect()
    }

    fn auth(&self) -> TestResult<OpenFgaHttp> {
        Ok(OpenFgaHttp {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            url: self.url.clone(),
            store: "store".into(),
            model: "model".into(),
        })
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn reply(State(state): State<Arc<WireState>>, uri: Uri, body: Bytes) -> Response {
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap()
    };
    state
        .requests
        .lock()
        .await
        .push((uri.path().to_owned(), value));
    let operation = uri.path().rsplit('/').next().unwrap_or("");
    let configured = state
        .replies
        .lock()
        .await
        .get_mut(operation)
        .and_then(VecDeque::pop_front);
    let response = configured.unwrap_or_else(|| {
        Reply::json(match operation {
            "healthz" => json!({"status": "SERVING"}),
            "store" => json!({"id": "store"}),
            "model" => json!({"authorization_model": {"id": "model"}}),
            "check" | "redirected" => json!({"allowed": true, "resolution": ""}),
            "write" | "read" => json!({}),
            "list-objects" => json!({"objects":["task:access-review", "task:vendor-review"]}),
            _ => panic!("unexpected HTTP operation {}", uri.path()),
        })
    });
    let mut builder = Response::builder().status(response.status);
    if let Some(content_type) = response.content_type {
        builder = builder.header("content-type", content_type);
    }
    if let Some(location) = response.location {
        builder = builder.header("location", location);
    }
    builder.body(Body::from(response.body)).unwrap()
}

fn tuple(user: &str) -> Tuple {
    Tuple {
        user: format!("user:{user}"),
        relation: "owner".into(),
        object: "organization:acme".into(),
    }
}

#[tokio::test]
async fn http_preserves_model_consistency_and_atomic_idempotent_writes() -> TestResult {
    let stub = Stub::start().await?;
    let auth = stub.auth()?;
    auth.ensure_ready().await?;
    assert!(auth.check("alice", "can_view", "organization:acme").await?);
    auth.write(&[tuple("alice")], true).await?;
    auth.write(&[tuple("alice")], false).await?;
    auth.apply(&[tuple("bob")], &[tuple("alice")]).await?;
    auth.apply(&[], &[]).await?;
    auth.write(&[], true).await?;
    auth.write(&[], false).await?;
    let checks = stub.requests("check").await;
    assert_eq!(checks.len(), 1);
    assert_eq!(
        checks[0],
        json!({
            "authorization_model_id": "model", "consistency": "HIGHER_CONSISTENCY",
            "tuple_key": {"user": "user:alice", "relation": "can_view", "object": "organization:acme"}
        })
    );
    let writes = stub.requests("write").await;
    assert_eq!(writes.len(), 3, "mixed changes must use one atomic Write");
    for write in &writes {
        assert_eq!(write["authorization_model_id"], "model");
    }
    assert_eq!(writes[0]["writes"]["on_duplicate"], "ignore");
    assert!(writes[0].get("deletes").is_none());
    assert_eq!(writes[1]["deletes"]["on_missing"], "ignore");
    assert!(writes[1].get("writes").is_none());
    assert_eq!(writes[2]["writes"]["tuple_keys"][0]["user"], "user:bob");
    assert_eq!(writes[2]["deletes"]["tuple_keys"][0]["user"], "user:alice");
    assert_eq!(writes[2]["writes"]["on_duplicate"], "ignore");
    assert_eq!(writes[2]["deletes"]["on_missing"], "ignore");
    Ok(())
}

#[tokio::test]
async fn http_never_accepts_unconfirmed_status_as_allow_or_write_success() -> TestResult {
    let stub = Stub::start().await?;
    let auth = stub.auth()?;
    for status in [202, 204, 301, 302, 307, 308, 401, 429, 500, 503] {
        for operation in ["check", "write"] {
            let mut response = Reply::json(if operation == "check" {
                json!({"allowed": true})
            } else {
                json!({})
            });
            response.status = StatusCode::from_u16(status)?;
            response.location = Some("/redirected");
            stub.enqueue(operation, response).await;
            let failed = if operation == "check" {
                auth.check("alice", "can_view", "organization:acme")
                    .await
                    .is_err()
            } else {
                auth.write(&[tuple("alice")], false).await.is_err()
            };
            assert!(failed, "{operation} must reject HTTP {status}");
        }
    }
    assert!(stub.requests("redirected").await.is_empty());
    assert_eq!(
        stub.requests("check").await.len(),
        10,
        "failed Checks must not be retried"
    );
    assert_eq!(
        stub.requests("write").await.len(),
        10,
        "failed writes must not be retried"
    );
    // A caller-supplied client following a redirect must not turn its final
    // 200 response into confirmation from the intended Check endpoint.
    let mut redirected = Reply::json(json!({"allowed": true}));
    redirected.status = StatusCode::TEMPORARY_REDIRECT;
    redirected.location = Some("/redirected");
    stub.enqueue("check", redirected).await;
    let auth = OpenFgaHttp {
        client: reqwest::Client::builder().no_proxy().build()?,
        ..auth
    };
    assert!(
        auth.check("alice", "can_view", "organization:acme")
            .await
            .is_err()
    );
    assert_eq!(stub.requests("redirected").await.len(), 1);
    Ok(())
}

#[tokio::test]
async fn http_rejects_malformed_allow_and_write_envelopes() -> TestResult {
    let stub = Stub::start().await?;
    let auth = stub.auth()?;
    for body in [
        "{}",
        "null",
        "[]",
        "[true]",
        "[true, null]",
        "{\"allowed\":null}",
        "{\"allowed\":\"true\"}",
        "{\"allowed\":true,\"allowed\":false}",
        "{\"allowed\":true,\"error\":\"unavailable\"}",
        "{\"allowed\":true,\"resolution\":42}",
        "{\"allowed\":true}garbage",
        "{\"allowed\":true",
    ] {
        stub.enqueue("check", Reply::raw(body)).await;
        assert!(
            auth.check("alice", "can_view", "organization:acme")
                .await
                .is_err(),
            "accepted {body}"
        );
    }
    for body in [
        "",
        "null",
        "[]",
        "{\"code\":\"error\"}",
        "{\"allowed\":true}",
    ] {
        stub.enqueue("write", Reply::raw(body)).await;
        assert!(
            auth.write(&[tuple("alice")], false).await.is_err(),
            "accepted {body}"
        );
    }
    for content_type in [None, Some("text/html"), Some("text/plain")] {
        let mut response = Reply::json(json!({"allowed": true}));
        response.content_type = content_type;
        stub.enqueue("check", response).await;
        assert!(
            auth.check("alice", "can_view", "organization:acme")
                .await
                .is_err()
        );
    }
    let mut oversized = br#"{"allowed":true,"resolution":""#.to_vec();
    oversized.extend(std::iter::repeat_n(b'x', 1024 * 1024));
    oversized.extend_from_slice(br#""}"#);
    stub.enqueue("check", Reply::raw(oversized)).await;
    assert!(
        auth.check("alice", "can_view", "organization:acme")
            .await
            .is_err()
    );
    stub.enqueue("check", Reply::json(json!({"allowed": false})))
        .await;
    assert!(!auth.check("alice", "can_view", "organization:acme").await?);
    assert!(auth.check("alice", "can_view", "organization:acme").await?);
    Ok(())
}

#[tokio::test]
async fn http_readiness_requires_expected_store_and_model() -> TestResult {
    let stub = Stub::start().await?;
    let auth = stub.auth()?;
    for (operation, body) in [
        ("healthz", json!({"status": "NOT_SERVING"})),
        ("store", json!({})),
        ("store", json!({"id": "another-store"})),
        ("model", json!({})),
        (
            "model",
            json!({"authorization_model": {"id": "another-model"}}),
        ),
    ] {
        stub.enqueue(operation, Reply::json(body)).await;
        assert!(
            auth.ensure_ready().await.is_err(),
            "accepted invalid {operation}"
        );
    }
    auth.ensure_ready().await?;
    Ok(())
}

#[tokio::test]
async fn http_delete_rejects_nonterminal_empty_pages_and_invalid_keys() -> TestResult {
    let stub = Stub::start().await?;
    let auth = stub.auth()?;
    for page in [
        json!([]),
        json!({"tuples": [], "continuation_token": "more"}),
        json!({"continuation_token": "more"}),
        json!({"tuples": [{"key": {"user": "user:alice", "relation": "owner", "object": "organization:other"}}]}),
        json!({"tuples": [{"key": {"user": "", "relation": "owner", "object": "organization:acme"}}]}),
        json!({"tuples": [{"key": {"user": "user:alice", "relation": "", "object": "organization:acme"}}]}),
        json!({"tuples": [{}]}),
    ] {
        stub.enqueue("read", Reply::json(page)).await;
        assert!(auth.delete_object("organization:acme").await.is_err());
    }
    assert!(
        stub.requests("write").await.is_empty(),
        "invalid read pages must not cause writes"
    );
    Ok(())
}

#[tokio::test]
async fn http_delete_drains_first_page_until_confirmed_empty() -> TestResult {
    let stub = Stub::start().await?;
    let auth = stub.auth()?;
    for user in ["alice", "bob"] {
        stub.enqueue("read", Reply::json(json!({
            "tuples": [{"key": {"user": format!("user:{user}"), "relation": "owner", "object": "organization:acme"}}],
            "continuation_token": "unstable-cursor"
        }))).await;
    }
    // Proto JSON may omit both empty fields. It still denotes a terminal page.
    stub.enqueue("read", Reply::json(json!({}))).await;
    auth.delete_object("organization:acme").await?;
    let reads = stub.requests("read").await;
    assert_eq!(reads.len(), 3);
    for read in reads {
        assert_eq!(read["tuple_key"]["object"], "organization:acme");
        assert_eq!(read["consistency"], "HIGHER_CONSISTENCY");
        assert!(read.get("continuation_token").is_none());
    }
    assert_eq!(stub.requests("write").await.len(), 2);
    Ok(())
}

#[tokio::test]
async fn http_deadline_covers_the_complete_response_without_client_timeouts() -> TestResult {
    for operation in ["check", "write", "list-objects"] {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let (release, wait) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let mut first_byte = [0];
            socket.read_exact(&mut first_byte).await?;
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{").await?;
            let _ = wait.await;
            Ok::<_, std::io::Error>(())
        });
        let auth = OpenFgaHttp {
            client: reqwest::Client::builder().no_proxy().build()?,
            url,
            store: "store".into(),
            model: "model".into(),
        };
        let result = tokio::time::timeout(Duration::from_secs(5), async {
            if operation == "check" {
                auth.check("alice", "can_view", "organization:acme")
                    .await
                    .map(|_| ())
            } else if operation == "list-objects" {
                auth.list_objects("alice", "can_view", "task")
                    .await
                    .map(|_| ())
            } else {
                auth.write(&[tuple("alice")], false).await
            }
        })
        .await?;
        assert!(result.is_err(), "{operation} accepted unfinished response");
        let _ = release.send(());
        server.await??;
    }
    Ok(())
}

#[tokio::test]
async fn list_objects_pins_model_and_strong_consistency() -> TestResult {
    let stub = Stub::start().await?;
    assert_eq!(
        stub.auth()?
            .list_objects("alice", "can_view", "task")
            .await?,
        ["task:access-review", "task:vendor-review"]
    );
    let requests = stub.requests("list-objects").await;
    assert_eq!(
        requests,
        [json!({
            "authorization_model_id": "model",
            "consistency": "HIGHER_CONSISTENCY",
            "user": "user:alice",
            "relation": "can_view",
            "type": "task"
        })]
    );
    Ok(())
}

#[tokio::test]
async fn list_objects_rejects_errors_and_malformed_envelopes() -> TestResult {
    let stub = Stub::start().await?;
    let auth = stub.auth()?;
    for status in [202, 204, 302, 503] {
        let mut reply = Reply::json(json!({"objects":["task:access-review"]}));
        reply.status = StatusCode::from_u16(status)?;
        stub.enqueue("list-objects", reply).await;
        assert!(
            auth.list_objects("alice", "can_view", "task")
                .await
                .is_err()
        );
    }
    for body in [
        "[]",
        "null",
        "{\"objects\":true}",
        "{\"objects\":[null]}",
        "{\"objects\":[],\"error\":\"unavailable\"}",
    ] {
        stub.enqueue("list-objects", Reply::raw(body)).await;
        assert!(
            auth.list_objects("alice", "can_view", "task")
                .await
                .is_err(),
            "accepted {body}"
        );
    }
    assert_eq!(
        stub.requests("list-objects").await.len(),
        9,
        "failed lists must not be retried"
    );
    // Protobuf may omit an empty repeated field, but not its object envelope.
    stub.enqueue("list-objects", Reply::json(json!({}))).await;
    assert!(
        auth.list_objects("alice", "can_view", "task")
            .await?
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn healthy_server_with_missing_store_is_unavailable() -> TestResult {
    let stub = Stub::start().await?;
    let mut missing = Reply::json(json!({}));
    missing.status = StatusCode::NOT_FOUND;
    stub.enqueue("store", missing).await;
    assert!(stub.auth()?.ensure_available().await.is_err());
    let requests = stub.state.requests.lock().await;
    let paths: Vec<_> = requests.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(paths, ["/healthz", "/stores/store"]);
    Ok(())
}

#[tokio::test]
async fn readiness_probes_health_store_and_model_once_per_attempt() -> TestResult {
    let stub = Stub::start().await?;
    let auth = stub.auth()?;
    auth.ensure_ready().await?;
    auth.ensure_ready().await?;
    let requests = stub.state.requests.lock().await;
    let paths: Vec<_> = requests.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "/healthz",
            "/stores/store",
            "/stores/store/authorization-models/model"
        ]
        .repeat(2)
    );
    Ok(())
}

#[tokio::test]
async fn positive_check_does_not_replace_live_store_availability() -> TestResult {
    let stub = Stub::start().await?;
    let auth = stub.auth()?;
    auth.ensure_available().await?;
    assert!(
        auth.check(
            "alice",
            "can_get_metadata",
            "lakekeeper_table:warehouse/table"
        )
        .await?
    );
    // OpenFGA may still return a positive Check after soft-deleting the store.
    // CachedAuthorizer must also use this independent, uncached store probe.
    let mut missing = Reply::json(json!({}));
    missing.status = StatusCode::NOT_FOUND;
    stub.enqueue("store", missing).await;
    assert!(
        auth.check(
            "alice",
            "can_get_metadata",
            "lakekeeper_table:warehouse/table"
        )
        .await?
    );
    assert!(auth.ensure_available().await.is_err());
    assert_eq!(stub.requests("check").await.len(), 2);
    assert_eq!(stub.requests("store").await.len(), 2);
    Ok(())
}
