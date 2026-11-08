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
use std::{sync::Arc, time::Duration};
use tokio::{
    net::TcpListener,
    sync::{Mutex, oneshot},
    task::JoinHandle,
    time::timeout,
};

struct Gate {
    operation: &'static str,
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
        let (capture_tx, captured) = oneshot::channel();
        let (release, release_rx) = oneshot::channel();
        let mut gate = self.upstream.gate.lock().await;
        assert!(gate.is_none(), "Previous proxy gate was not captured");
        *gate = Some(Gate {
            operation,
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

async fn gate_response(
    upstream: &Upstream,
    operation: &str,
    succeeded: bool,
    body: Bytes,
) -> Result<(), StatusCode> {
    let captured = {
        let mut gate = upstream.gate.lock().await;
        if succeeded
            && gate
                .as_ref()
                .is_some_and(|gate| operation == gate.operation)
        {
            gate.take()
        } else {
            None
        }
    };
    if let Some(gate) = captured {
        // The real operation completed before the test is told it can proceed.
        let _ = gate.captured.send(body);
        if !matches!(
            timeout(Duration::from_secs(8), gate.release).await,
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
    gate_response(
        &upstream,
        uri.path().rsplit('/').next().unwrap_or_default(),
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
    mut request: Request,
) -> Result<Response, StatusCode> {
    let operation = match request.uri().path() {
        "/openfga.v1.OpenFGAService/GetStore" => "healthz",
        "/openfga.v1.OpenFGAService/Check" => "check",
        "/openfga.v1.OpenFGAService/Write" => "write",
        _ => "other",
    };
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
    gate_response(&upstream, operation, succeeded, captured).await?;
    // Replay the exact protobuf bytes and gRPC trailers, including failures.
    Ok(Response::from_parts(parts, grpc_body(bytes, trailers)))
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
                object: "pet:fox".into(),
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
