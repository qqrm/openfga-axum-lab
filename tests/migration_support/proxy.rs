//! HTTP migration fault boundary: every successful operation still reaches the
//! real OpenFGA instance, except the explicitly requested pre-write rejection.
use super::stack::TestResult;
use axum::{
    Router,
    body::Bytes,
    extract::{OriginalUri, State},
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use reqwest::Client;
use std::{sync::Arc, time::Duration};
use tokio::{
    net::TcpListener,
    sync::{Mutex, oneshot},
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
        captured: oneshot::Sender<()>,
        release: oneshot::Receiver<()>,
    },
}

#[derive(Clone)]
struct Upstream {
    url: String,
    client: Client,
    fault: Arc<Mutex<Fault>>,
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

impl Proxy {
    pub async fn start(client: Client, url: String) -> TestResult<Self> {
        let upstream = Upstream {
            url,
            client,
            fault: Arc::new(Mutex::new(Fault::None)),
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
        assert!(nth > 0);
        let (capture_tx, captured) = oneshot::channel();
        let (release, release_rx) = oneshot::channel();
        *self.upstream.fault.lock().await = Fault::PauseWrite {
            remaining: nth,
            captured: capture_tx,
            release: release_rx,
        };
        Pause {
            captured,
            release: Some(release),
        }
    }
}

async fn forward(
    State(upstream): State<Upstream>,
    OriginalUri(uri): OriginalUri,
    method: Method,
    body: Bytes,
) -> Result<Response, StatusCode> {
    let operation = uri.path().rsplit('/').next().unwrap_or_default();
    if operation == "write" {
        let mut fault = upstream.fault.lock().await;
        match *fault {
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
            _ => {}
        }
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
    let captured = {
        let mut fault = upstream.fault.lock().await;
        if status == StatusCode::OK && operation == "check" && matches!(*fault, Fault::WrongCheck) {
            *fault = Fault::None;
            let mut body: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|_| StatusCode::BAD_GATEWAY)?;
            body["allowed"] =
                serde_json::Value::Bool(!body["allowed"].as_bool().ok_or(StatusCode::BAD_GATEWAY)?);
            bytes = Bytes::from(serde_json::to_vec(&body).map_err(|_| StatusCode::BAD_GATEWAY)?);
            None
        } else if status == StatusCode::OK && operation == "write" {
            if let Fault::PauseWrite { remaining, .. } = &mut *fault {
                *remaining -= 1;
                if *remaining == 0 {
                    Some(std::mem::take(&mut *fault))
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    };
    if let Some(Fault::PauseWrite {
        captured, release, ..
    }) = captured
    {
        let _ = captured.send(());
        let _ = timeout(Duration::from_secs(30), release).await;
    }
    let mut result = (status, bytes).into_response();
    if let Some(content_type) = content_type {
        result
            .headers_mut()
            .insert(header::CONTENT_TYPE, content_type);
    }
    Ok(result)
}
