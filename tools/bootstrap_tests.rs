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
    delays: Mutex<HashMap<&'static str, Duration>>,
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
    async fn delay(&self, operation: &'static str, duration: Duration) {
        self.state.delays.lock().await.insert(operation, duration);
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
    } else if method == Method::POST && uri.path().ends_with("/check") {
        "check"
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
        "check" => Reply::json(StatusCode::OK, json!({"allowed": false, "resolution": ""})),
        _ => unreachable!(),
    });
    let delay = state.delays.lock().await.get(operation).copied();
    if let Some(delay) = delay {
        sleep(delay).await;
    }
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
        .position(|definition| definition["type"] == "lakekeeper_table")
        .unwrap();
    let mut changed = wire.clone();
    changed["type_definitions"][index]["relations"]["can_read_data"] = json!({"this":{}});
    assert_ne!(policy_model(&changed)?, policy_model(&expected)?);
    let mut changed = wire.clone();
    changed["type_definitions"][index]["metadata"]["relations"]["ownership"]["directly_related_user_types"] =
        json!([{"type":"user","wildcard":{}}]);
    assert_ne!(policy_model(&changed)?, policy_model(&expected)?);
    let mut changed = wire;
    changed["type_definitions"][index]["metadata"]["relations"]["ownership"]["directly_related_user_types"]
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
    provision(&stub.url, &path, &model, REQUEST_TIMEOUT, false).await?;
    let published = fs::read(&path)?;
    let bound: Config = serde_json::from_slice(&published)?;
    assert_eq!(bound.store_id, STORE);
    assert_eq!(bound.model_id, MODEL);
    assert_eq!(bound.model_sha256, PINNED_MODEL_SHA256);
    provision(&stub.url, &path, &model, REQUEST_TIMEOUT, false).await?;
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
                provision(&stub.url, &path, &model, REQUEST_TIMEOUT, false)
                    .await
                    .is_err(),
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
        assert!(
            provision(&stub.url, &path, &model, REQUEST_TIMEOUT, false)
                .await
                .is_err()
        );
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
        assert!(
            provision(&stub.url, &path, &model, REQUEST_TIMEOUT, false)
                .await
                .is_err()
        );
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
    assert!(
        provision(&stub.url, &path, &model, REQUEST_TIMEOUT, false)
            .await
            .is_err()
    );
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
    // No client timeout: the helper's request and outer deadlines must still
    // include consumption of an unfinished response body.
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

#[test]
fn model_request_timeout_is_opt_in_and_bounded() -> ToolResult {
    assert_eq!(model_request_timeout(None)?, REQUEST_TIMEOUT);
    for seconds in ["5", "6", "30"] {
        assert_eq!(
            model_request_timeout(Some(OsStr::new(seconds)))?,
            Duration::from_secs(seconds.parse()?)
        );
    }
    for value in [
        "",
        "0",
        "4",
        "31",
        "60",
        "-1",
        "+30",
        " 30",
        "30 ",
        "3.0",
        "NaN",
        "18446744073709551616",
        "３０",
    ] {
        assert!(model_request_timeout(Some(OsStr::new(value))).is_err());
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        assert!(model_request_timeout(Some(OsStr::from_bytes(&[0xff]))).is_err());
    }
    Ok(())
}

#[tokio::test]
async fn model_request_override_replaces_only_the_request_client_timeout() -> ToolResult {
    let stub = Stub::start().await?;
    stub.delay("get_model", Duration::from_millis(250)).await;
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_millis(100))
        .build()?;
    let actual: ModelResponse = send_json_with_timeout(
        client.get(format!(
            "{}/stores/{STORE}/authorization-models/{MODEL}",
            stub.url
        )),
        StatusCode::OK,
        Duration::from_secs(2),
    )
    .await?;
    assert_eq!(actual.authorization_model.id, MODEL);
    // The per-request override must not alter the shared client.
    stub.delay("get_store", Duration::from_millis(250)).await;
    assert!(
        client
            .get(format!("{}/stores/{STORE}", stub.url))
            .send()
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn model_budget_does_not_apply_to_health_store_or_empty_tuple_reads() -> ToolResult {
    let stub = Stub::start().await?;
    // Short private test durations make request scope observable. Production
    // accepts only the bounded 5..=30 second environment setting.
    let model_timeout = Duration::from_millis(500);
    for operation in ["health", "create_store", "get_store", "read"] {
        stub.delay(operation, Duration::from_millis(650)).await;
    }
    let directory = tempfile::tempdir()?;
    let model = model_path(directory.path());
    let path = directory.path().join("config.json");
    provision(&stub.url, &path, &model, model_timeout, false).await?;
    let bound: Config = serde_json::from_slice(&fs::read(path)?)?;
    validate_existing(&bound, MODEL_BYTES)?;
    let requests = stub.state.requests.lock().await;
    assert_eq!(requests.len(), 6);
    assert_eq!(requests.last().unwrap().1, format!("/stores/{STORE}/read"));
    Ok(())
}

#[tokio::test]
async fn model_request_deadline_never_retries_or_publishes_uncertain_binding() -> ToolResult {
    for operation in ["create_model", "get_model"] {
        let stub = Stub::start().await?;
        stub.delay(operation, Duration::from_millis(200)).await;
        let directory = tempfile::tempdir()?;
        let model = model_path(directory.path());
        let path = directory.path().join("config.json");
        assert!(
            provision(&stub.url, &path, &model, Duration::from_millis(50), false)
                .await
                .is_err()
        );
        assert!(!path.exists());
        let requests = stub.state.requests.lock().await;
        assert_eq!(
            requests
                .iter()
                .filter(|(_, path, _)| path.ends_with("/authorization-models"))
                .count(),
            1,
            "an uncertain model write must never be retried"
        );
        assert!(requests.iter().all(|(_, path, _)| !path.ends_with("/read")));
        let expected_requests = match operation {
            "create_model" => 3,
            _ => 5,
        };
        assert_eq!(requests.len(), expected_requests);
    }
    Ok(())
}

#[tokio::test]
async fn existing_binding_model_deadline_preserves_configuration_without_reset() -> ToolResult {
    let stub = Stub::start().await?;
    stub.delay("get_model", Duration::from_millis(200)).await;
    let directory = tempfile::tempdir()?;
    let model = model_path(directory.path());
    let path = directory.path().join("config.json");
    write_config(&path, &config())?;
    let before = fs::read(&path)?;
    assert!(
        provision(&stub.url, &path, &model, Duration::from_millis(50), false)
            .await
            .is_err()
    );
    assert_eq!(fs::read(&path)?, before);
    let requests = stub.state.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert!(
        requests
            .iter()
            .all(|(method, _, _)| method.as_str() == "GET")
    );
    Ok(())
}

#[tokio::test]
async fn model_override_keeps_confirmation_and_empty_store_requirements() -> ToolResult {
    for operation in ["create_model", "get_model", "read"] {
        let stub = Stub::start().await?;
        let reply = match operation {
            "create_model" => Reply::json(StatusCode::OK, json!({"authorization_model_id": MODEL})),
            "get_model" => {
                let mut actual = model_response();
                actual["authorization_model"]["id"] = json!(OTHER);
                Reply::json(StatusCode::OK, actual)
            }
            "read" => Reply::json(StatusCode::OK, json!({"tuples": [{}]})),
            _ => unreachable!(),
        };
        stub.enqueue(operation, reply).await;
        let directory = tempfile::tempdir()?;
        let model = model_path(directory.path());
        let path = directory.path().join("config.json");
        assert!(
            provision(&stub.url, &path, &model, Duration::from_secs(30), false)
                .await
                .is_err()
        );
        assert!(!path.exists());
    }
    Ok(())
}

#[tokio::test]
async fn model_request_deadline_includes_unfinished_response_body() -> ToolResult {
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
    let client = Client::builder()
        .no_proxy()
        .timeout(REQUEST_TIMEOUT)
        .build()?;
    let result = timeout(
        Duration::from_secs(2),
        send_json_with_timeout::<ModelResponse>(
            client.get(url),
            StatusCode::OK,
            Duration::from_millis(50),
        ),
    )
    .await?;
    assert!(result.is_err());
    let _ = release.send(());
    server.await??;
    Ok(())
}

#[test]
fn empty_store_check_flag_is_strict_and_default_off() -> ToolResult {
    assert!(!empty_store_check_enabled(None)?);
    assert!(empty_store_check_enabled(Some(OsStr::new("yes")))?);
    for value in ["", "no", "true", "1", "YES", " yes", "yes "] {
        assert!(empty_store_check_enabled(Some(OsStr::new(value))).is_err());
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        assert!(empty_store_check_enabled(Some(OsStr::from_bytes(&[0xff]))).is_err());
    }
    Ok(())
}

#[tokio::test]
async fn empty_store_check_requires_deny_after_model_and_empty_read() -> ToolResult {
    for body in [
        json!({"allowed": false}),
        json!({"allowed": false, "resolution": ""}),
    ] {
        let stub = Stub::start().await?;
        stub.enqueue("check", Reply::json(StatusCode::OK, body)).await;
        let directory = tempfile::tempdir()?;
        let model = model_path(directory.path());
        let path = directory.path().join("config.json");
        provision(&stub.url, &path, &model, REQUEST_TIMEOUT, true).await?;
        let published = fs::read(&path)?;
        validate_existing(&serde_json::from_slice(&published)?, MODEL_BYTES)?;
        {
            let requests = stub.state.requests.lock().await;
            let paths: Vec<_> = requests
                .iter()
                .map(|(_, path, _)| path.as_str())
                .collect();
            assert_eq!(
                paths,
                [
                    "/healthz".to_owned(),
                    "/stores".to_owned(),
                    format!("/stores/{STORE}/authorization-models"),
                    format!("/stores/{STORE}"),
                    format!("/stores/{STORE}/authorization-models/{MODEL}"),
                    format!("/stores/{STORE}/read"),
                    format!("/stores/{STORE}/check"),
                ]
            );
            assert_eq!(requests[6].0, Method::POST);
            assert_eq!(
                requests[6].2,
                json!({
                    "authorization_model_id": MODEL,
                    "consistency": "HIGHER_CONSISTENCY",
                    "tuple_key": {
                        "user": "user:browser-bootstrap-probe",
                        "relation": "ownership",
                        "object": "lakekeeper_table:browser-bootstrap-probe"
                    }
                })
            );
        }
        // Existing bindings are verified, never treated as new empty stores.
        provision(&stub.url, &path, &model, REQUEST_TIMEOUT, true).await?;
        assert_eq!(fs::read(&path)?, published);
        let requests = stub.state.requests.lock().await;
        assert_eq!(requests.len(), 10);
        assert!(
            requests[7..]
                .iter()
                .all(|(method, _, _)| *method == Method::GET)
        );
        assert_eq!(
            requests
                .iter()
                .filter(|(_, path, _)| path.ends_with("/check"))
                .count(),
            1
        );
    }
    Ok(())
}

#[tokio::test]
async fn empty_store_check_failures_never_publish_or_retry() -> ToolResult {
    let replies = [
        Reply::json(StatusCode::OK, json!({"allowed": true})),
        Reply::json(StatusCode::OK, json!({})),
        Reply::json(StatusCode::OK, json!({"allowed": "false"})),
        Reply::json(StatusCode::OK, json!({"allowed": null})),
        Reply::json(StatusCode::OK, json!({"allowed": false, "resolution": 42})),
        Reply::json(StatusCode::OK, json!({"allowed": false, "unexpected": true})),
        Reply::json(StatusCode::CREATED, json!({"allowed": false})),
        Reply::json(StatusCode::TEMPORARY_REDIRECT, json!({"allowed": false})),
        Reply::json(StatusCode::INTERNAL_SERVER_ERROR, json!({"allowed": false})),
        Reply {
            status: StatusCode::OK,
            content_type: Some("application/json"),
            body: br#"{"allowed":false"#.to_vec(),
        },
        Reply {
            status: StatusCode::OK,
            content_type: Some("application/json"),
            body: br#"{"allowed":false,"allowed":true}"#.to_vec(),
        },
    ];
    for reply in replies {
        let stub = Stub::start().await?;
        stub.enqueue("check", reply).await;
        let directory = tempfile::tempdir()?;
        let model = model_path(directory.path());
        let path = directory.path().join("config.json");
        assert!(
            provision(&stub.url, &path, &model, Duration::from_secs(30), true)
                .await
                .is_err()
        );
        assert!(!path.exists());
        let requests = stub.state.requests.lock().await;
        assert_eq!(requests.len(), 7);
        assert_eq!(
            requests
                .iter()
                .filter(|(_, path, _)| path.ends_with("/check"))
                .count(),
            1
        );
        assert!(!requests.iter().any(|(_, path, _)| path.ends_with("/write")));
    }
    Ok(())
}

#[tokio::test]
async fn empty_store_check_requires_prior_confirmation() -> ToolResult {
    for operation in ["get_model", "read", "continuation"] {
        let stub = Stub::start().await?;
        let (route, reply) = match operation {
            "get_model" => {
                let mut actual = model_response();
                actual["authorization_model"]["id"] = json!(OTHER);
                ("get_model", Reply::json(StatusCode::OK, actual))
            }
            "read" => ("read", Reply::json(StatusCode::OK, json!({"tuples": [{}]}))),
            _ => (
                "read",
                Reply::json(StatusCode::OK, json!({"continuation_token": "more"})),
            ),
        };
        stub.enqueue(route, reply).await;
        let directory = tempfile::tempdir()?;
        let model = model_path(directory.path());
        let path = directory.path().join("config.json");
        assert!(
            provision(&stub.url, &path, &model, Duration::from_secs(30), true)
                .await
                .is_err()
        );
        assert!(!path.exists());
        assert!(
            stub.state
                .requests
                .lock()
                .await
                .iter()
                .all(|(_, path, _)| !path.ends_with("/check"))
        );
    }
    Ok(())
}

#[tokio::test]
async fn empty_store_check_keeps_five_seconds_with_model_override() -> ToolResult {
    let stub = Stub::start().await?;
    stub.delay("check", Duration::from_secs(6)).await;
    let directory = tempfile::tempdir()?;
    let model = model_path(directory.path());
    let path = directory.path().join("config.json");
    let start = std::time::Instant::now();
    let result = provision(&stub.url, &path, &model, Duration::from_secs(30), true).await;
    assert!(result.is_err());
    assert!(start.elapsed() >= Duration::from_millis(4500));
    assert!(start.elapsed() < Duration::from_secs(8));
    assert!(!path.exists());
    let requests = stub.state.requests.lock().await;
    assert_eq!(requests.len(), 7);
    assert_eq!(
        requests
            .iter()
            .filter(|(_, path, _)| path.ends_with("/check"))
            .count(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn short_model_budget_does_not_shorten_empty_store_check() -> ToolResult {
    let stub = Stub::start().await?;
    stub.delay("check", Duration::from_millis(250)).await;
    let directory = tempfile::tempdir()?;
    let model = model_path(directory.path());
    let path = directory.path().join("config.json");
    provision(&stub.url, &path, &model, Duration::from_millis(100), true).await?;
    validate_existing(&serde_json::from_slice(&fs::read(path)?)?, MODEL_BYTES)?;
    Ok(())
}
