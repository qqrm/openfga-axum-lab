#![cfg(feature = "http")]

use openfga_axum_lab::{
    application::{Authorizer, Tuple},
    infrastructure::OpenFgaHttp,
};
use serde_json::{Value, json};
use std::{io, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::Mutex,
    task::{JoinHandle, JoinSet},
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Copy)]
enum Reply {
    Success,
    Unavailable,
    StalledBody,
    Ready,
    MissingStore,
    ListedTasks,
    StalledList,
    MalformedList,
}

struct Stub {
    url: String,
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    task: JoinHandle<()>,
}

impl Stub {
    async fn start(reply: Reply) -> TestResult<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            while let Ok((stream, _)) = listener.accept().await {
                let captured = captured.clone();
                connections.spawn(async move { serve(stream, captured, reply).await });
            }
        });
        Ok(Self {
            url,
            requests,
            task,
        })
    }

    fn authorizer(&self) -> TestResult<OpenFgaHttp> {
        Ok(OpenFgaHttp {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(2))
                .build()?,
            url: self.url.clone(),
            store: "store".to_owned(),
            model: "model".to_owned(),
        })
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.task.abort();
    }
}

// A raw HTTP stub can acknowledge headers but withhold part of the declared
// response body. This catches adapters that report mutation success too early.
async fn serve(
    stream: TcpStream,
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    reply: Reply,
) -> io::Result<()> {
    let mut stream = BufReader::new(stream);
    let mut line = String::new();
    stream.read_line(&mut line).await?;
    let request_line = line.trim_end().to_owned();
    let mut length = None;
    loop {
        line.clear();
        if stream.read_line(&mut line).await? == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "HTTP headers"));
        }
        if line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = Some(value.trim().parse::<usize>().map_err(io::Error::other)?);
        }
    }
    let length = match length {
        Some(length) => length,
        None if request_line.starts_with("GET ") => 0,
        None => return Err(io::Error::other("missing request length")),
    };
    if length > 16_384 {
        return Err(io::Error::other("unexpectedly large test request"));
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes).await?;
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).map_err(io::Error::other)?
    };
    let target = request_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| io::Error::other("missing request target"))?
        .to_owned();
    requests.lock().await.push((request_line, body));
    let (status, response_body) = match reply {
        Reply::Success | Reply::StalledBody => ("200 OK", "{}"),
        Reply::Unavailable => ("503 Service Unavailable", "{}"),
        Reply::ListedTasks | Reply::StalledList => (
            "200 OK",
            r#"{"objects":["task:access-review","task:vendor-review"]}"#,
        ),
        Reply::MalformedList => ("200 OK", r#"{"objects":true}"#),
        Reply::Ready | Reply::MissingStore => match target.as_str() {
            "/healthz" => ("200 OK", r#"{"status":"SERVING"}"#),
            "/stores/store" if matches!(reply, Reply::MissingStore) => ("404 Not Found", "{}"),
            "/stores/store" => ("200 OK", r#"{"id":"store"}"#),
            "/stores/store/authorization-models/model" => {
                ("200 OK", r#"{"authorization_model":{"id":"model"}}"#)
            }
            _ => ("404 Not Found", "{}"),
        },
    };
    let response_length =
        response_body.len() + usize::from(matches!(reply, Reply::StalledBody | Reply::StalledList));
    stream
        .get_mut()
        .write_all(
            format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {response_length}\r\nConnection: close\r\n\r\n{response_body}"
            )
            .as_bytes(),
        )
        .await?;
    if matches!(reply, Reply::StalledBody | Reply::StalledList) {
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
    Ok(())
}

fn owner(user: &str) -> Tuple {
    Tuple {
        user: format!("user:{user}"),
        relation: "owner".to_owned(),
        object: "organization:acme".to_owned(),
    }
}

fn transfer() -> Value {
    json!({
        "authorization_model_id": "model",
        "writes": {"tuple_keys": [owner("carol")], "on_duplicate": "ignore"},
        "deletes": {"tuple_keys": [owner("alice")], "on_missing": "ignore"}
    })
}

#[tokio::test]
async fn ownership_transfer_is_one_atomic_idempotent_write() -> TestResult {
    let stub = Stub::start(Reply::Success).await?;
    stub.authorizer()?
        .change(&[owner("carol")], &[owner("alice")])
        .await?;
    let requests = stub.requests.lock().await;
    assert_eq!(requests.len(), 1, "transfer must not split into two writes");
    assert_eq!(requests[0].0, "POST /stores/store/write HTTP/1.1");
    assert_eq!(requests[0].1, transfer());
    Ok(())
}

#[tokio::test]
async fn one_sided_writes_omit_the_empty_side_and_empty_changes_are_noops() -> TestResult {
    let stub = Stub::start(Reply::Success).await?;
    let auth = stub.authorizer()?;
    auth.write(&[owner("carol")], true).await?;
    auth.write(&[owner("alice")], false).await?;
    auth.change(&[], &[]).await?;
    auth.write(&[], true).await?;
    auth.write(&[], false).await?;
    let requests = stub.requests.lock().await;
    assert_eq!(requests.len(), 2);
    let mut only_writes = transfer();
    only_writes.as_object_mut().unwrap().remove("deletes");
    let mut only_deletes = transfer();
    only_deletes.as_object_mut().unwrap().remove("writes");
    assert_eq!(requests[0].1, only_writes);
    assert_eq!(requests[1].1, only_deletes);
    Ok(())
}

#[tokio::test]
async fn ownership_transfer_rejects_unavailable_server_without_retrying() -> TestResult {
    let stub = Stub::start(Reply::Unavailable).await?;
    assert!(
        stub.authorizer()?
            .change(&[owner("carol")], &[owner("alice")])
            .await
            .is_err()
    );
    let requests = stub.requests.lock().await;
    assert_eq!(requests.len(), 1, "failed mutations must not be retried");
    assert_eq!(requests[0].1, transfer());
    Ok(())
}

#[tokio::test]
async fn ownership_transfer_waits_for_the_entire_acknowledgement() -> TestResult {
    let stub = Stub::start(Reply::StalledBody).await?;
    let auth = stub.authorizer()?;
    let writes = [owner("carol")];
    let deletes = [owner("alice")];
    let response = tokio::time::timeout(Duration::from_secs(3), auth.change(&writes, &deletes))
        .await
        .expect("HTTP mutation must have a bounded full-response deadline");
    assert!(
        response.is_err(),
        "headers alone cannot acknowledge a write"
    );
    let requests = stub.requests.lock().await;
    assert_eq!(requests.len(), 1, "ambiguous mutations must not be retried");
    assert_eq!(requests[0].1, transfer());
    Ok(())
}

#[tokio::test]
async fn healthy_server_with_missing_store_is_unavailable() -> TestResult {
    let stub = Stub::start(Reply::MissingStore).await?;
    assert!(stub.authorizer()?.ensure_available().await.is_err());
    let requests = stub.requests.lock().await;
    let lines: Vec<_> = requests.iter().map(|(line, _)| line.as_str()).collect();
    assert_eq!(
        lines,
        ["GET /healthz HTTP/1.1", "GET /stores/store HTTP/1.1"]
    );
    Ok(())
}

#[tokio::test]
async fn readiness_probes_health_store_and_model_once_per_attempt() -> TestResult {
    let stub = Stub::start(Reply::Ready).await?;
    let auth = stub.authorizer()?;
    auth.ensure_ready().await?;
    auth.ensure_ready().await?;
    let requests = stub.requests.lock().await;
    let lines: Vec<_> = requests.iter().map(|(line, _)| line.as_str()).collect();
    let one_attempt = [
        "GET /healthz HTTP/1.1",
        "GET /stores/store HTTP/1.1",
        "GET /stores/store/authorization-models/model HTTP/1.1",
    ];
    assert_eq!(lines, one_attempt.repeat(2));
    Ok(())
}

#[tokio::test]
async fn list_objects_pins_model_and_strong_consistency() -> TestResult {
    let stub = Stub::start(Reply::ListedTasks).await?;
    assert_eq!(
        stub.authorizer()?
            .list_objects("alice", "can_view", "task")
            .await?,
        ["task:access-review", "task:vendor-review"]
    );
    let requests = stub.requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, "POST /stores/store/list-objects HTTP/1.1");
    assert_eq!(
        requests[0].1,
        json!({
            "authorization_model_id": "model",
            "consistency": "HIGHER_CONSISTENCY",
            "user": "user:alice",
            "relation": "can_view",
            "type": "task"
        })
    );
    Ok(())
}

#[tokio::test]
async fn list_objects_rejects_errors_or_malformed_responses() -> TestResult {
    for reply in [Reply::Unavailable, Reply::MalformedList] {
        let stub = Stub::start(reply).await?;
        assert!(
            stub.authorizer()?
                .list_objects("alice", "can_view", "task")
                .await
                .is_err()
        );
        assert_eq!(
            stub.requests.lock().await.len(),
            1,
            "failed lists must not be retried"
        );
    }
    Ok(())
}

#[tokio::test]
async fn list_objects_does_not_return_partial_response_bodies() -> TestResult {
    let stub = Stub::start(Reply::StalledList).await?;
    let auth = stub.authorizer()?;
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        auth.list_objects("alice", "can_view", "task"),
    )
    .await
    .expect("HTTP list must have a bounded full-response deadline");
    assert!(
        result.is_err(),
        "the declared response body must be fully received"
    );
    assert_eq!(stub.requests.lock().await.len(), 1);
    Ok(())
}
