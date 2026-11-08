//! Fault boundary around the real HTTP migration backend. A detached bounded
//! worker can retain a request after its client has died, including before the
//! upstream commit; idle means every captured worker actually finished. A
//! per-pause receipt separately proves a successful real backend acknowledgement.
use super::stack::TestResult;
use axum::{
    Router,
    body::Bytes,
    extract::{OriginalUri, State},
    http::{Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
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

#[derive(Default)]
enum Fault {
    #[default]
    None,
    RedirectWrite,
    ErrorObjectWrite,
    WrongCheck,
    PauseWrite {
        remaining: usize,
        before: bool,
        captured: oneshot::Sender<()>,
        release: oneshot::Receiver<()>,
        acknowledged: oneshot::Sender<()>,
    },
}
#[derive(Clone)]
struct Upstream {
    url: String,
    client: Client,
    fault: Arc<Mutex<Fault>>,
    active: Arc<AtomicUsize>,
    idle: Arc<Notify>,
}
pub struct Proxy {
    pub url: String,
    upstream: Upstream,
    task: JoinHandle<std::io::Result<()>>,
}
pub struct Pause {
    captured: oneshot::Receiver<()>,
    release: Option<oneshot::Sender<()>>,
    acknowledged: oneshot::Receiver<()>,
}
impl Pause {
    pub async fn captured(&mut self) -> TestResult {
        Ok(timeout(Duration::from_secs(30), &mut self.captured).await??)
    }
    pub async fn release_and_wait_for_ack(mut self) -> TestResult {
        self.release
            .take()
            .ok_or("paused migration batch was already released")?
            .send(())
            .map_err(|_| "paused migration batch expired before its explicit release")?;
        timeout(Duration::from_secs(30), &mut self.acknowledged)
            .await
            .map_err(|_| "paused migration batch received no backend acknowledgement in time")?
            .map_err(|_| "paused migration batch did not receive a successful OpenFGA Write acknowledgement")?;
        Ok(())
    }
}
impl Drop for Pause {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }
}
impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}
struct Active(Upstream);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
        self.0.idle.notify_one();
    }
}
impl Proxy {
    pub async fn start(client: Client, url: String) -> TestResult<Self> {
        let upstream = Upstream {
            url,
            client,
            fault: Arc::new(Mutex::new(Fault::None)),
            active: Arc::new(AtomicUsize::new(0)),
            idle: Arc::new(Notify::new()),
        };
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let router = Router::new().fallback(forward).with_state(upstream.clone());
        let task = tokio::spawn(async move { axum::serve(listener, router).await });
        Ok(Self {
            url,
            upstream,
            task,
        })
    }
    pub async fn redirect_write(&self) {
        *self.upstream.fault.lock().await = Fault::RedirectWrite;
    }
    pub async fn error_object_write(&self) {
        *self.upstream.fault.lock().await = Fault::ErrorObjectWrite;
    }
    pub async fn wrong_check(&self) {
        *self.upstream.fault.lock().await = Fault::WrongCheck;
    }
    pub async fn pause_write(&self, nth: usize) -> Pause {
        self.pause(nth, false).await
    }
    pub async fn pause_before_write(&self, nth: usize) -> Pause {
        self.pause(nth, true).await
    }
    async fn pause(&self, nth: usize, before: bool) -> Pause {
        assert!(nth > 0);
        let (capture_tx, captured) = oneshot::channel();
        let (release, release_rx) = oneshot::channel();
        let (acknowledged_tx, acknowledged) = oneshot::channel();
        *self.upstream.fault.lock().await = Fault::PauseWrite {
            remaining: nth,
            before,
            captured: capture_tx,
            release: release_rx,
            acknowledged: acknowledged_tx,
        };
        Pause {
            captured,
            release: Some(release),
            acknowledged,
        }
    }
    pub async fn wait_idle(&self) -> TestResult {
        timeout(Duration::from_secs(30), async {
            while self.upstream.active.load(Ordering::SeqCst) != 0 {
                self.upstream.idle.notified().await;
            }
        })
        .await?;
        Ok(())
    }
}
async fn pause_write(fault: Fault) -> Result<oneshot::Sender<()>, StatusCode> {
    let Fault::PauseWrite {
        before,
        captured,
        release,
        acknowledged,
        ..
    } = fault
    else {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    };
    let _ = captured.send(());
    // Before-forward tests run the full bounded equivalence matrix (180s) and
    // restart an app (60s) while this worker remains held. Keep a finite margin
    // for SQL/HTTP assertions; expiry is an explicit test failure, never an ACK.
    let hold = if before { 300 } else { 60 };
    timeout(Duration::from_secs(hold), release)
        .await
        .map_err(|_| StatusCode::GATEWAY_TIMEOUT)?
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    Ok(acknowledged)
}
async fn forward(
    State(upstream): State<Upstream>,
    OriginalUri(uri): OriginalUri,
    method: Method,
    body: Bytes,
) -> Result<Response, StatusCode> {
    upstream.active.fetch_add(1, Ordering::SeqCst);
    let (send, receive) = oneshot::channel();
    tokio::spawn(async move {
        let _active = Active(upstream.clone());
        let result = forward_worker(upstream, uri, method, body).await;
        let _ = send.send(result);
    });
    receive.await.map_err(|_| StatusCode::BAD_GATEWAY)?
}
async fn forward_worker(
    upstream: Upstream,
    uri: Uri,
    method: Method,
    body: Bytes,
) -> Result<Response, StatusCode> {
    let operation = uri.path().rsplit('/').next().unwrap_or_default();
    let mut paused = None;
    if operation == "write" {
        let mut fault = upstream.fault.lock().await;
        match &mut *fault {
            Fault::RedirectWrite => {
                *fault = Fault::None;
                return Ok((
                    StatusCode::TEMPORARY_REDIRECT,
                    [(header::LOCATION, "/blocked")],
                    "redirect",
                )
                    .into_response());
            }
            Fault::ErrorObjectWrite => {
                *fault = Fault::None;
                return Ok((
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "application/json")],
                    r#"{"error":"not-written"}"#,
                )
                    .into_response());
            }
            Fault::PauseWrite { remaining, .. } => {
                *remaining -= 1;
                if *remaining == 0 {
                    paused = Some(std::mem::take(&mut *fault));
                }
            }
            _ => {}
        }
    }
    let mut acknowledged = None;
    if matches!(paused, Some(Fault::PauseWrite { before: true, .. })) {
        acknowledged = Some(pause_write(paused.take().unwrap()).await?);
    }
    let response = upstream
        .client
        .request(method, format!("{}{}", upstream.url, uri))
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    let status = response.status();
    let content_type = response.headers().get(header::CONTENT_TYPE).cloned();
    let mut bytes = response
        .bytes()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    if status == StatusCode::OK && operation == "check" {
        let mut fault = upstream.fault.lock().await;
        if matches!(*fault, Fault::WrongCheck) {
            *fault = Fault::None;
            let mut value: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|_| StatusCode::BAD_GATEWAY)?;
            value["allowed"] = serde_json::Value::Bool(
                !value["allowed"].as_bool().ok_or(StatusCode::BAD_GATEWAY)?,
            );
            bytes = Bytes::from(serde_json::to_vec(&value).map_err(|_| StatusCode::BAD_GATEWAY)?);
        }
    }
    if let Some(pause) = paused {
        acknowledged = Some(pause_write(pause).await?);
    }
    if let Some(acknowledged) = acknowledged {
        // This receipt belongs only to the selected held write. Idle alone
        // cannot distinguish delivery from a dropped/timed-out worker.
        let json = content_type
            .as_ref()
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(str::trim)
            == Some("application/json");
        let empty_object = matches!(
            serde_json::from_slice::<serde_json::Value>(&bytes),
            Ok(serde_json::Value::Object(value)) if value.is_empty()
        );
        if status == StatusCode::OK && json && empty_object {
            let _ = acknowledged.send(());
        }
    }
    let mut result = (status, bytes).into_response();
    if let Some(content_type) = content_type {
        result
            .headers_mut()
            .insert(header::CONTENT_TYPE, content_type);
    }
    Ok(result)
}
