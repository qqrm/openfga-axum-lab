use super::stack::TestResult;
use axum::{
    Router,
    body::Bytes,
    extract::{OriginalUri, State},
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
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
    client: Client,
    gate: Arc<Mutex<Option<Gate>>>,
}

pub struct Proxy {
    pub url: String,
    upstream: Upstream,
    task: JoinHandle<std::io::Result<()>>,
}

impl Proxy {
    pub async fn start(client: Client, url: String) -> TestResult<Self> {
        let upstream = Upstream {
            url,
            client,
            gate: Arc::new(Mutex::new(None)),
        };
        let router = Router::new()
            .route("/healthz", get(forward))
            .route("/stores/{store}/{operation}", post(forward))
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
    let captured = {
        let mut gate = upstream.gate.lock().await;
        if status.is_success()
            && gate
                .as_ref()
                .is_some_and(|gate| uri.path().rsplit('/').next() == Some(gate.operation))
        {
            gate.take()
        } else {
            None
        }
    };
    if let Some(gate) = captured {
        // The real operation completed before the test is told it can proceed.
        let _ = gate.captured.send(body.clone());
        if !matches!(
            timeout(Duration::from_secs(8), gate.release).await,
            Ok(Ok(()))
        ) || gate.fail
        {
            return Err(StatusCode::BAD_GATEWAY);
        }
    }
    Ok((status, [(header::CONTENT_TYPE, "application/json")], body).into_response())
}
