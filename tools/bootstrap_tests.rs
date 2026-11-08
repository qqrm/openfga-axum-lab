use super::*;
use axum::{
    Router,
    body::{Body, Bytes},
    extract::State,
    http::{Method, Uri},
    response::Response,
};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::Arc,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::{Mutex, oneshot},
    task::JoinHandle,
};

const STORE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const MODEL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const OTHER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
const MODEL_BYTES: &[u8] = include_bytes!("../fga/model.json");

fn config() -> Config {
    Config {
        store_id: STORE.into(),
        model_id: MODEL.into(),
        model_sha256: digest(MODEL_BYTES),
    }
}
fn model_path(directory: &Path) -> PathBuf {
    let path = directory.join("model.json");
    fs::write(&path, MODEL_BYTES).unwrap();
    path
}
fn model_response() -> Value {
    // Match v1.10.3's grpc-gateway EmitUnpopulated protobuf JSON, which
    // includes default fields absent from the unmodified source model.
    fn defaults(value: &mut Value) {
        match value {
            Value::Array(children) => children.iter_mut().for_each(defaults),
            Value::Object(fields) => {
                for (name, child) in fields {
                    if matches!(name.as_str(), "computedUserset" | "tupleset") {
                        child
                            .as_object_mut()
                            .unwrap()
                            .entry("object")
                            .or_insert(json!(""));
                    }
                    defaults(child);
                }
            }
            _ => {}
        }
    }
    let mut model: Value = serde_json::from_slice(MODEL_BYTES).unwrap();
    defaults(&mut model);
    for definition in model["type_definitions"].as_array_mut().unwrap() {
        definition
            .as_object_mut()
            .unwrap()
            .entry("relations")
            .or_insert(json!({}));
        let metadata = definition["metadata"].as_object_mut().unwrap();
        let relations = metadata
            .entry("relations")
            .or_insert(json!({}))
            .as_object_mut()
            .unwrap();
        for metadata in relations.values_mut() {
            let fields = metadata.as_object_mut().unwrap();
            fields.entry("module").or_insert(json!(""));
            fields.entry("source_info").or_insert(Value::Null);
            let subjects = fields
                .entry("directly_related_user_types")
                .or_insert(json!([]))
                .as_array_mut()
                .unwrap();
            for subject in subjects {
                subject
                    .as_object_mut()
                    .unwrap()
                    .entry("condition")
                    .or_insert(json!(""));
            }
        }
    }
    model["id"] = json!(MODEL);
    model["conditions"] = json!({});
    json!({"authorization_model": model})
}

struct Reply {
    status: StatusCode,
    content_type: Option<&'static str>,
    body: Vec<u8>,
}
impl Reply {
    fn json(status: StatusCode, value: Value) -> Self {
        Self {
            status,
            content_type: Some("application/json"),
            body: serde_json::to_vec(&value).unwrap(),
        }
    }
}
#[derive(Default)]
struct WireState {
    requests: Mutex<Vec<(Method, String, Value)>>,
    replies: Mutex<HashMap<&'static str, VecDeque<Reply>>>,
}
struct Stub {
    url: String,
    state: Arc<WireState>,
    task: JoinHandle<std::io::Result<()>>,
}
impl Stub {
    async fn start() -> ToolResult<Self> {
        let state = Arc::new(WireState::default());
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let router = Router::new().fallback(reply).with_state(state.clone());
        let task = tokio::spawn(async move { axum::serve(listener, router).await });
        Ok(Self { url, state, task })
    }
    async fn enqueue(&self, operation: &'static str, reply: Reply) {
        self.state
            .replies
            .lock()
            .await
            .entry(operation)
            .or_default()
            .push_back(reply);
    }
}
impl Drop for Stub {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn reply(
    State(state): State<Arc<WireState>>,
    method: Method,
    uri: Uri,
    body: Bytes,
) -> Response {
    let body = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap()
    };
    state
        .requests
        .lock()
        .await
        .push((method.clone(), uri.path().to_owned(), body));
    let operation = if uri.path() == "/healthz" {
        "health"
    } else if method == Method::POST && uri.path() == "/stores" {
        "create_store"
    } else if method == Method::POST && uri.path().ends_with("/authorization-models") {
        "create_model"
    } else if method == Method::GET && uri.path() == format!("/stores/{STORE}") {
        "get_store"
    } else if method == Method::GET && uri.path().contains("/authorization-models/") {
        "get_model"
    } else if method == Method::POST && uri.path().ends_with("/read") {
        "read"
    } else {
        panic!("bootstrap unexpectedly called {method} {}", uri.path());
    };
    let configured = state
        .replies
        .lock()
        .await
        .get_mut(operation)
        .and_then(VecDeque::pop_front);
    let reply = configured.unwrap_or_else(|| match operation {
        "health" => Reply::json(StatusCode::OK, json!({"status": "SERVING"})),
        "create_store" => Reply::json(StatusCode::CREATED, json!({"id": STORE})),
        "create_model" => Reply::json(
            StatusCode::CREATED,
            json!({"authorization_model_id": MODEL}),
        ),
        "get_store" => Reply::json(StatusCode::OK, json!({"id": STORE})),
        "get_model" => Reply::json(StatusCode::OK, model_response()),
        "read" => Reply::json(StatusCode::OK, json!({})),
        _ => unreachable!(),
    });
    let mut response = Response::builder().status(reply.status);
    if let Some(content_type) = reply.content_type {
        response = response.header("content-type", content_type);
    }
    response.body(Body::from(reply.body)).unwrap()
}

#[test]
fn unchanged_pinned_model_is_reused() {
    assert!(validate_existing(&config(), MODEL_BYTES).is_ok());
}
#[test]
fn model_comparison_preserves_grants_and_subject_restrictions() -> ToolResult {
    let expected: Value = serde_json::from_slice(MODEL_BYTES)?;
    let wire = model_response()["authorization_model"].clone();
    assert_eq!(policy_model(&wire)?, policy_model(&expected)?);
    let index = wire["type_definitions"]
        .as_array()
        .unwrap()
        .iter()
        .position(|definition| definition["type"] == "task")
        .unwrap();
    let mut changed = wire.clone();
    changed["type_definitions"][index]["relations"]["can_view"] = json!({"this":{}});
    assert_ne!(policy_model(&changed)?, policy_model(&expected)?);
    let mut changed = wire.clone();
    changed["type_definitions"][index]["metadata"]["relations"]["assigner"]["directly_related_user_types"] =
        json!([{"type":"user","wildcard":{}}]);
    assert_ne!(policy_model(&changed)?, policy_model(&expected)?);
    let mut changed = wire;
    changed["type_definitions"][index]["metadata"]["relations"]["assigner"]["directly_related_user_types"]
        [0]["condition"] = json!("unexpected-condition");
    assert_ne!(policy_model(&changed)?, policy_model(&expected)?);
    Ok(())
}

#[test]
fn changed_model_requires_explicit_migration() {
    assert!(validate_existing(&config(), b"different").is_err());
}
#[test]
fn missing_or_invalid_identifiers_fail_closed() {
    for invalid in [
        "",
        "../another-store",
        "01arZ3NDEKTSV4RRFFQ69G5FAV",
        "01IRZ3NDEKTSV4RRFFQ69G5FAV",
    ] {
        let mut value = config();
        value.store_id = invalid.into();
        assert!(validate_existing(&value, MODEL_BYTES).is_err());
        let mut value = config();
        value.model_id = invalid.into();
        assert!(validate_existing(&value, MODEL_BYTES).is_err());
    }
}
#[test]
fn config_is_published_atomically_and_never_overwritten() -> ToolResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("config.json");
    write_config(&path, &config())?;
    let original = fs::read(&path)?;
    let mut replacement = config();
    replacement.store_id = OTHER.into();
    assert!(write_config(&path, &replacement).is_err());
    assert_eq!(fs::read(&path)?, original);
    assert_eq!(fs::read_dir(directory.path())?.count(), 1);
    Ok(())
}

#[tokio::test]
async fn provisions_only_empty_store_and_pinned_model_then_reuses_binding() -> ToolResult {
    let stub = Stub::start().await?;
    let directory = tempfile::tempdir()?;
    let model = model_path(directory.path());
    let path = directory.path().join("config.json");
    provision(&stub.url, &path, &model).await?;
    let published = fs::read(&path)?;
    let bound: Config = serde_json::from_slice(&published)?;
    assert_eq!(bound.store_id, STORE);
    assert_eq!(bound.model_id, MODEL);
    assert_eq!(bound.model_sha256, PINNED_MODEL_SHA256);
    provision(&stub.url, &path, &model).await?;
    assert_eq!(fs::read(&path)?, published);
    let requests = stub.state.requests.lock().await;
    let creates: Vec<_> = requests
        .iter()
        .filter(|(method, _, _)| *method == Method::POST)
        .collect();
    assert_eq!(
        creates.len(),
        3,
        "only create store, create model, and read empty store"
    );
    assert_eq!(creates[0].1, "/stores");
    assert!(creates[1].1.ends_with("/authorization-models"));
    assert_eq!(creates[1].2, serde_json::from_slice::<Value>(MODEL_BYTES)?);
    assert!(creates[2].1.ends_with("/read"));
    assert_eq!(creates[2].2["consistency"], "HIGHER_CONSISTENCY");
    assert!(!requests.iter().any(|(_, path, _)| path.ends_with("/write")));
    Ok(())
}

#[tokio::test]
async fn creation_requires_exact_201_and_never_publishes_uncertain_binding() -> ToolResult {
    for operation in ["create_store", "create_model"] {
        for status in [200, 202, 204, 307, 500] {
            let stub = Stub::start().await?;
            let directory = tempfile::tempdir()?;
            let model = model_path(directory.path());
            let path = directory.path().join("config.json");
            let body = if operation == "create_store" {
                json!({"id": STORE})
            } else {
                json!({"authorization_model_id": MODEL})
            };
            stub.enqueue(operation, Reply::json(StatusCode::from_u16(status)?, body))
                .await;
            assert!(
                provision(&stub.url, &path, &model).await.is_err(),
                "{operation} accepted HTTP {status}"
            );
            assert!(
                !path.exists(),
                "uncertain creation must not publish a target"
            );
            let attempts = stub
                .state
                .requests
                .lock()
                .await
                .iter()
                .filter(|(method, path, _)| {
                    *method == Method::POST
                        && if operation == "create_store" {
                            path == "/stores"
                        } else {
                            path.ends_with("/authorization-models")
                        }
                })
                .count();
            assert_eq!(
                attempts, 1,
                "uncertain create operations must not be retried"
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn persisted_target_failures_preserve_config_and_never_recreate() -> ToolResult {
    let mut wrong_model_id = model_response();
    wrong_model_id["authorization_model"]["id"] = json!(OTHER);
    let mut wrong_model_body = model_response();
    wrong_model_body["authorization_model"]["type_definitions"] = json!([]);
    for (operation, response) in [
        ("get_store", Reply::json(StatusCode::NOT_FOUND, json!({}))),
        (
            "get_store",
            Reply::json(StatusCode::TEMPORARY_REDIRECT, json!({"id": STORE})),
        ),
        (
            "get_store",
            Reply::json(StatusCode::OK, json!({"id": OTHER})),
        ),
        ("get_model", Reply::json(StatusCode::NOT_FOUND, json!({}))),
        ("get_model", Reply::json(StatusCode::OK, wrong_model_id)),
        ("get_model", Reply::json(StatusCode::OK, wrong_model_body)),
    ] {
        let stub = Stub::start().await?;
        let directory = tempfile::tempdir()?;
        let model = model_path(directory.path());
        let path = directory.path().join("config.json");
        write_config(&path, &config())?;
        let original = fs::read(&path)?;
        stub.enqueue(operation, response).await;
        assert!(provision(&stub.url, &path, &model).await.is_err());
        assert_eq!(fs::read(&path)?, original);
        assert!(
            stub.state
                .requests
                .lock()
                .await
                .iter()
                .all(|(method, _, _)| *method == Method::GET)
        );
    }
    Ok(())
}

#[tokio::test]
async fn new_target_must_match_model_and_be_confirmed_empty_before_publish() -> ToolResult {
    for (operation, body) in [
        ("create_store", json!({"id": "../../other"})),
        ("create_model", json!({"authorization_model_id": "invalid"})),
        (
            "get_model",
            json!({"authorization_model": {"id": MODEL, "schema_version": "1.1", "type_definitions": []}}),
        ),
        ("read", json!({"tuples": [{"key": {"user": "user:alice"}}]})),
        ("read", json!({"tuples": [], "continuation_token": "more"})),
        ("read", json!([])),
    ] {
        let stub = Stub::start().await?;
        let directory = tempfile::tempdir()?;
        let model = model_path(directory.path());
        let path = directory.path().join("config.json");
        let status = if operation.starts_with("create_") {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        };
        stub.enqueue(operation, Reply::json(status, body)).await;
        assert!(provision(&stub.url, &path, &model).await.is_err());
        assert!(!path.exists());
    }
    Ok(())
}

#[tokio::test]
async fn unexpected_model_file_is_rejected_before_any_request() -> ToolResult {
    let stub = Stub::start().await?;
    let directory = tempfile::tempdir()?;
    let model = model_path(directory.path());
    fs::write(&model, b"{}")?;
    let path = directory.path().join("config.json");
    assert!(provision(&stub.url, &path, &model).await.is_err());
    assert!(stub.state.requests.lock().await.is_empty());
    assert!(!path.exists());
    Ok(())
}

#[tokio::test]
async fn json_protocol_rejects_wrong_media_types_arrays_duplicates_and_oversized_bodies()
-> ToolResult {
    let stub = Stub::start().await?;
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let mut responses: Vec<_> = [None, Some("text/html")]
        .into_iter()
        .map(|content_type| {
            let mut response = Reply::json(StatusCode::OK, json!({"status": "SERVING"}));
            response.content_type = content_type;
            response
        })
        .collect();
    for body in [
        r#"["SERVING"]"#,
        r#"{"status":"NOT_SERVING","status":"SERVING"}"#,
        r#"{"status":"SERVING""#,
    ] {
        responses.push(Reply {
            status: StatusCode::OK,
            content_type: Some("application/json"),
            body: body.as_bytes().to_vec(),
        });
    }
    responses.push(Reply {
        status: StatusCode::OK,
        content_type: Some("application/json"),
        body: vec![b' '; MAX_RESPONSE_BYTES + 1],
    });
    for response in responses {
        stub.enqueue("health", response).await;
        assert!(
            send_json::<Health>(client.get(format!("{}/healthz", stub.url)), StatusCode::OK)
                .await
                .is_err()
        );
    }
    Ok(())
}

#[tokio::test]
async fn response_deadline_includes_unfinished_body() -> ToolResult {
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
    // No reqwest timeout: the helper itself must bound the entire operation.
    let client = Client::builder().no_proxy().build()?;
    let result = timeout(
        REQUEST_TIMEOUT + Duration::from_secs(2),
        send_json::<Health>(client.get(url), StatusCode::OK),
    )
    .await?;
    assert!(result.is_err());
    let _ = release.send(());
    server.await??;
    Ok(())
}
