//! Fault boundary around the real HTTP migration backend. A detached bounded
//! worker can retain a request after its client has died, including before the
//! upstream commit; idle means every captured worker actually finished.
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
}
impl Pause {
    pub async fn captured(&mut self) -> TestResult {
        Ok(timeout(Duration::from_secs(30), &mut self.captured).await??)
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
        *self.upstream.fault.lock().await = Fault::PauseWrite {
            remaining: nth,
            before,
            captured: capture_tx,
            release: release_rx,
        };
        Pause {
            captured,
            release: Some(release),
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
async fn pause_write(fault: Fault) -> Result<(), StatusCode> {
    if let Fault::PauseWrite {
        captured, release, ..
    } = fault
    {
        let _ = captured.send(());
        timeout(Duration::from_secs(60), release)
            .await
            .map_err(|_| StatusCode::GATEWAY_TIMEOUT)?
            .map_err(|_| StatusCode::BAD_GATEWAY)?;
    }
    Ok(())
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
    if matches!(paused, Some(Fault::PauseWrite { before: true, .. })) {
        pause_write(paused.take().unwrap()).await?;
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
        pause_write(pause).await?;
    }
    let mut result = (status, bytes).into_response();
    if let Some(content_type) = content_type {
        result
            .headers_mut()
            .insert(header::CONTENT_TYPE, content_type);
    }
    Ok(result)
}
