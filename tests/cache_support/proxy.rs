use super::stack::TestResult;
use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{StatusCode, header},
    response::Response,
};
#[cfg(feature = "grpc")]
use axum::{body::Body, extract::Request, http::HeaderMap};
#[cfg(not(feature = "grpc"))]
use axum::{
    extract::OriginalUri,
    http::Method,
    response::IntoResponse,
    routing::{get, post},
};
#[cfg(feature = "grpc")]
use http_body_util::{BodyExt, Full};
#[cfg(feature = "grpc")]
use hyper_util::{
    client::legacy::{Client as Http2Client, connect::HttpConnector},
    rt::TokioExecutor,
};
#[cfg(feature = "grpc")]
use openfga_client::client::CheckResponse;
#[cfg(feature = "grpc")]
use prost::Message;
use reqwest::Client;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::{Mutex, Notify, oneshot},
    task::JoinHandle,
    time::timeout,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    BeforeRequest,
    AfterResponse,
}

struct Gate {
    operation: &'static str,
    stage: Stage,
    captured: oneshot::Sender<Bytes>,
    release: oneshot::Receiver<()>,
    fail: bool,
}

pub struct Pause {
    captured: oneshot::Receiver<Bytes>,
    release: Option<oneshot::Sender<()>>,
}

impl Pause {
    pub async fn captured(&mut self) -> TestResult<Bytes> {
        Ok(timeout(Duration::from_secs(4), &mut self.captured).await??)
    }
}

impl Drop for Pause {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }
}

#[derive(Clone)]
struct Upstream {
    url: String,
    #[cfg(not(feature = "grpc"))]
    client: Client,
    #[cfg(feature = "grpc")]
    client: Http2Client<HttpConnector, Body>,
    gate: Arc<Mutex<Option<Gate>>>,
    active: Arc<AtomicUsize>,
    idle: Arc<Notify>,
    unacknowledged_writes: Arc<AtomicUsize>,
    acknowledged_writes: Arc<AtomicUsize>,
}

struct InFlight(Upstream);

impl InFlight {
    fn new(upstream: &Upstream) -> Self {
        upstream.active.fetch_add(1, Ordering::SeqCst);
        Self(upstream.clone())
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        if self.0.active.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.0.idle.notify_one();
        }
    }
}

pub struct Proxy {
    pub url: String,
    upstream: Upstream,
    task: JoinHandle<std::io::Result<()>>,
}

impl Proxy {
    pub async fn start(client: Client, url: String) -> TestResult<Self> {
        #[cfg(feature = "grpc")]
        let client = {
            // reqwest does not expose the response trailers needed by gRPC.
            drop(client);
            Http2Client::builder(TokioExecutor::new())
                .http2_only(true)
                .build_http()
        };
        let upstream = Upstream {
            url,
            client,
            gate: Arc::new(Mutex::new(None)),
            active: Arc::new(AtomicUsize::new(0)),
            idle: Arc::new(Notify::new()),
            unacknowledged_writes: Arc::new(AtomicUsize::new(0)),
            acknowledged_writes: Arc::new(AtomicUsize::new(0)),
        };
        #[cfg(not(feature = "grpc"))]
        let router = Router::new()
            .route("/healthz", get(forward))
            .route("/stores/{store}", get(forward))
            .route("/stores/{store}/authorization-models/{model}", get(forward))
            .route("/stores/{store}/{operation}", post(forward))
            .with_state(upstream.clone());
        #[cfg(feature = "grpc")]
        let router = Router::new()
            .fallback(forward_grpc)
            .with_state(upstream.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let task = tokio::spawn(async move { axum::serve(listener, router).await });
        Ok(Self {
            url,
            upstream,
            task,
        })
    }

    pub async fn pause(&self, operation: &'static str, fail: bool) -> Pause {
        self.pause_at(operation, Stage::AfterResponse, fail).await
    }

    /// Retain the request before forwarding it, even after the caller times out.
    /// This models a buffered request rather than only a lost committed response.
    pub async fn pause_before_request(&self, operation: &'static str) -> Pause {
        self.pause_at(operation, Stage::BeforeRequest, false).await
    }

    pub async fn wait_idle(&self) -> TestResult {
        timeout(Duration::from_secs(10), async {
            while self.upstream.active.load(Ordering::SeqCst) != 0 {
                self.upstream.idle.notified().await;
            }
        })
        .await?;
        if self.upstream.unacknowledged_writes.load(Ordering::SeqCst) != 0 {
            return Err("test proxy has writes without a complete upstream acknowledgement".into());
        }
        Ok(())
    }

    pub fn acknowledged_writes(&self) -> usize {
        self.upstream.acknowledged_writes.load(Ordering::SeqCst)
    }

    async fn pause_at(&self, operation: &'static str, stage: Stage, fail: bool) -> Pause {
        let (capture_tx, captured) = oneshot::channel();
        let (release, release_rx) = oneshot::channel();
        let mut gate = self.upstream.gate.lock().await;
        assert!(gate.is_none(), "Previous proxy gate was not captured");
        *gate = Some(Gate {
            operation,
            stage,
            captured: capture_tx,
            release: release_rx,
            fail,
        });
        Pause {
            captured,
            release: Some(release),
        }
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn gate_operation(
    upstream: &Upstream,
    operation: &str,
    stage: Stage,
    succeeded: bool,
    body: Bytes,
) -> Result<(), StatusCode> {
    let captured = {
        let mut gate = upstream.gate.lock().await;
        if succeeded
            && gate
                .as_ref()
                .is_some_and(|gate| operation == gate.operation && stage == gate.stage)
        {
            gate.take()
        } else {
            None
        }
    };
    if let Some(gate) = captured {
        // The selected boundary is explicit: either no upstream call has been
        // made yet, or the real operation already completed.
        let _ = gate.captured.send(body);
        if !matches!(
            timeout(Duration::from_secs(15), gate.release).await,
            Ok(Ok(()))
        ) || gate.fail
        {
            return Err(StatusCode::BAD_GATEWAY);
        }
    }
    Ok(())
}

#[cfg(not(feature = "grpc"))]
async fn forward(
    State(upstream): State<Upstream>,
    OriginalUri(uri): OriginalUri,
    method: Method,
    body: Bytes,
) -> Result<Response, StatusCode> {
    let active = InFlight::new(&upstream);
    // Detach forwarding from the caller's connection lifetime. A network
    // intermediary can retain a complete request after cancellation upstream.
    tokio::spawn(async move {
        let _active = active;
        forward_request(upstream, uri, method, body).await
    })
    .await
    .map_err(|_| StatusCode::BAD_GATEWAY)?
}

#[cfg(not(feature = "grpc"))]
async fn forward_request(
    upstream: Upstream,
    uri: axum::http::Uri,
    method: Method,
    body: Bytes,
) -> Result<Response, StatusCode> {
    let operation = uri.path().rsplit('/').next().unwrap_or_default();
    gate_operation(
        &upstream,
        operation,
        Stage::BeforeRequest,
        true,
        body.clone(),
    )
    .await?;
    if operation == "write" {
        upstream
            .unacknowledged_writes
            .fetch_add(1, Ordering::SeqCst);
    }
    let response = upstream
        .client
        .request(method, format!("{}{}", upstream.url, uri.path()))
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    if operation == "write" && status.is_success() {
        upstream
            .unacknowledged_writes
            .fetch_sub(1, Ordering::SeqCst);
        upstream.acknowledged_writes.fetch_add(1, Ordering::SeqCst);
    }
    gate_operation(
        &upstream,
        operation,
        Stage::AfterResponse,
        status.is_success(),
        body.clone(),
    )
    .await?;
    Ok((status, [(header::CONTENT_TYPE, "application/json")], body).into_response())
}

#[cfg(feature = "grpc")]
fn grpc_body(bytes: Bytes, trailers: Option<HeaderMap>) -> Body {
    Body::new(
        Full::new(bytes)
            .with_trailers(async move { trailers.map(Ok::<_, std::convert::Infallible>) }),
    )
}

#[cfg(feature = "grpc")]
fn check_capture(body: &Bytes) -> Result<Bytes, StatusCode> {
    // A unary, uncompressed protobuf message has a five-byte gRPC envelope.
    let Some(header) = body.get(..5) else {
        return Err(StatusCode::BAD_GATEWAY);
    };
    let length = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
    if header[0] != 0 || body.len() - 5 != length {
        return Err(StatusCode::BAD_GATEWAY);
    }
    let response = CheckResponse::decode(&body[5..]).map_err(|_| StatusCode::BAD_GATEWAY)?;
    serde_json::to_vec(&serde_json::json!({"allowed": response.allowed}))
        .map(Bytes::from)
        .map_err(|_| StatusCode::BAD_GATEWAY)
}

#[cfg(feature = "grpc")]
async fn forward_grpc(
    State(upstream): State<Upstream>,
    request: Request,
) -> Result<Response, StatusCode> {
    let active = InFlight::new(&upstream);
    tokio::spawn(async move {
        let _active = active;
        forward_grpc_request(upstream, request).await
    })
    .await
    .map_err(|_| StatusCode::BAD_GATEWAY)?
}

#[cfg(feature = "grpc")]
async fn forward_grpc_request(
    upstream: Upstream,
    mut request: Request,
) -> Result<Response, StatusCode> {
    let operation = match request.uri().path() {
        "/openfga.v1.OpenFGAService/GetStore" => "healthz",
        "/openfga.v1.OpenFGAService/Check" => "check",
        "/openfga.v1.OpenFGAService/Write" => "write",
        _ => "other",
    };
    let (parts, body) = request.into_parts();
    let body = body.collect().await.map_err(|_| StatusCode::BAD_GATEWAY)?;
    let trailers = body.trailers().cloned();
    let bytes = body.to_bytes();
    gate_operation(
        &upstream,
        operation,
        Stage::BeforeRequest,
        true,
        bytes.clone(),
    )
    .await?;
    if operation == "write" {
        upstream
            .unacknowledged_writes
            .fetch_add(1, Ordering::SeqCst);
    }
    request = Request::from_parts(parts, grpc_body(bytes, trailers));
    let path = request.uri().path_and_query().map_or("/", |p| p.as_str());
    *request.uri_mut() = format!("{}{path}", upstream.url)
        .parse()
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    request.headers_mut().remove(header::HOST);
    let (parts, body) = timeout(Duration::from_secs(10), async {
        let response = upstream
            .client
            .request(request)
            .await
            .map_err(|_| StatusCode::BAD_GATEWAY)?;
        let (parts, body) = response.into_parts();
        let body = body.collect().await.map_err(|_| StatusCode::BAD_GATEWAY)?;
        Ok::<_, StatusCode>((parts, body))
    })
    .await
    .map_err(|_| StatusCode::BAD_GATEWAY)??;
    let trailers = body.trailers().cloned();
    let bytes = body.to_bytes();
    // HTTP 200 alone does not mean that the gRPC operation succeeded.
    let succeeded = parts.status.is_success()
        && trailers
            .as_ref()
            .and_then(|headers| headers.get("grpc-status"))
            .or_else(|| parts.headers.get("grpc-status"))
            .is_some_and(|status| status == "0");
    let captured = if succeeded && operation == "check" {
        check_capture(&bytes)?
    } else {
        bytes.clone()
    };
    if operation == "write" && succeeded {
        upstream
            .unacknowledged_writes
            .fetch_sub(1, Ordering::SeqCst);
        upstream.acknowledged_writes.fetch_add(1, Ordering::SeqCst);
    }
    gate_operation(
        &upstream,
        operation,
        Stage::AfterResponse,
        succeeded,
        captured,
    )
    .await?;
    // Replay the exact protobuf bytes and gRPC trailers, including failures.
    Ok(Response::from_parts(parts, grpc_body(bytes, trailers)))
}

#[cfg(test)]
mod buffering_tests {
    use super::*;
    use openfga_axum_lab::application::Authorizer;

    async fn upstream_check(State(calls): State<Arc<AtomicUsize>>) -> Response {
        calls.fetch_add(1, Ordering::SeqCst);
        #[cfg(feature = "grpc")]
        {
            let message = CheckResponse {
                allowed: true,
                resolution: String::new(),
            }
            .encode_to_vec();
            let mut bytes = vec![0];
            bytes.extend(u32::try_from(message.len()).unwrap().to_be_bytes());
            bytes.extend(message);
            let mut trailers = HeaderMap::new();
            trailers.insert("grpc-status", "0".parse().unwrap());
            Response::builder()
                .header(header::CONTENT_TYPE, "application/grpc")
                .body(grpc_body(Bytes::from(bytes), Some(trailers)))
                .unwrap()
        }
        #[cfg(not(feature = "grpc"))]
        axum::Json(serde_json::json!({"allowed":true})).into_response()
    }

    #[tokio::test]
    async fn buffered_request_outlives_caller_cancellation_and_has_observable_drain() -> TestResult
    {
        let calls = Arc::new(AtomicUsize::new(0));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let router = Router::new()
            .fallback(upstream_check)
            .with_state(calls.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await });
        let client = Client::builder().no_proxy().build()?;
        let proxy = Proxy::start(client.clone(), url).await?;
        #[cfg(feature = "grpc")]
        let auth = openfga_axum_lab::infrastructure::OpenFgaGrpc::connect(
            proxy.url.clone(),
            "store",
            "model",
        )
        .await?;
        #[cfg(not(feature = "grpc"))]
        let auth = openfga_axum_lab::infrastructure::OpenFgaHttp {
            client,
            url: proxy.url.clone(),
            store: "store".into(),
            model: "model".into(),
        };
        let mut pause = proxy.pause_before_request("check").await;
        let check =
            tokio::spawn(
                async move { auth.check("alice", "can_view", "task:access-review").await },
            );
        pause.captured().await?;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "request escaped the pre-forward gate"
        );
        check.abort();
        assert!(check.await.unwrap_err().is_cancelled());
        drop(pause);
        proxy.wait_idle().await?;
        assert_eq!(proxy.acknowledged_writes(), 0);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "the buffered request must still reach the real upstream after cancellation"
        );
        server.abort();
        Ok(())
    }
}

#[cfg(all(test, feature = "grpc"))]
mod tests {
    use super::*;
    use openfga_client::client::{CheckRequest, CheckRequestTupleKey, OpenFgaServiceClient};
    use std::sync::atomic::{AtomicBool, Ordering};

    async fn check_upstream(State(fail): State<Arc<AtomicBool>>) -> Response {
        let mut trailers = HeaderMap::new();
        let bytes = if fail.load(Ordering::SeqCst) {
            trailers.insert("grpc-status", "7".parse().unwrap());
            Bytes::new()
        } else {
            trailers.insert("grpc-status", "0".parse().unwrap());
            let message = CheckResponse {
                allowed: true,
                resolution: String::new(),
            }
            .encode_to_vec();
            let mut bytes = vec![0];
            bytes.extend(u32::try_from(message.len()).unwrap().to_be_bytes());
            bytes.extend(message);
            Bytes::from(bytes)
        };
        Response::builder()
            .header(header::CONTENT_TYPE, "application/grpc")
            .body(grpc_body(bytes, Some(trailers)))
            .unwrap()
    }

    fn request() -> CheckRequest {
        CheckRequest {
            store_id: "test-store".into(),
            tuple_key: Some(CheckRequestTupleKey {
                user: "user:dave".into(),
                relation: "can_view".into(),
                object: "task:access-review".into(),
            }),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn grpc_proxy_gates_completed_responses_and_preserves_status_trailers() -> TestResult {
        let fail = Arc::new(AtomicBool::new(false));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let upstream_url = format!("http://{}", listener.local_addr()?);
        let router = Router::new()
            .route(
                "/openfga.v1.OpenFGAService/Check",
                axum::routing::post(check_upstream),
            )
            .with_state(fail.clone());
        let upstream_task = tokio::spawn(async move { axum::serve(listener, router).await });
        let proxy = Proxy::start(Client::new(), upstream_url).await?;
        let client = OpenFgaServiceClient::connect(proxy.url.clone()).await?;
        let mut pause = proxy.pause("check", false).await;
        let mut paused_client = client.clone();
        let check = tokio::spawn(async move { paused_client.check(request()).await });
        let captured: serde_json::Value = serde_json::from_slice(&pause.captured().await?)?;
        assert_eq!(captured["allowed"], true);
        assert!(
            !check.is_finished(),
            "response escaped before the gate opened"
        );
        drop(pause);
        assert!(
            timeout(Duration::from_secs(2), check)
                .await???
                .into_inner()
                .allowed
        );
        fail.store(true, Ordering::SeqCst);
        let error = client.clone().check(request()).await.unwrap_err();
        assert_eq!(error.code(), openfga_client::tonic::Code::PermissionDenied);
        upstream_task.abort();
        Ok(())
    }
}
