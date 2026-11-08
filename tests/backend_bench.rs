#![cfg(feature = "http")]

use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{StatusCode, Uri},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{net::TcpListener, process::Command, sync::Mutex, task::JoinHandle, time::timeout};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct Wire {
    calls: AtomicUsize,
    response: Mutex<(StatusCode, Value)>,
}

struct Stub {
    url: String,
    wire: Arc<Wire>,
    task: JoinHandle<std::io::Result<()>>,
}

impl Stub {
    async fn start() -> TestResult<Self> {
        let wire = Arc::new(Wire {
            calls: AtomicUsize::new(0),
            response: Mutex::new((StatusCode::OK, json!({"allowed": true}))),
        });
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let app = Router::new().fallback(reply).with_state(wire.clone());
        let task = tokio::spawn(async move { axum::serve(listener, app).await });
        Ok(Self { url, wire, task })
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_backend_bench"));
        for name in [
            "BACKEND",
            "MODE",
            "BENCH_USER",
            "BENCH_RELATION",
            "BENCH_OBJECT",
            "BENCH_TYPE",
            "BENCH_EXPECT_ALLOWED",
            "BENCH_EXPECTED_OBJECTS",
        ] {
            command.env_remove(name);
        }
        command
            .env("FGA_URL", &self.url)
            .env("FGA_STORE", "store")
            .env("FGA_MODEL", "model")
            .env("ITERS", "5")
            .env("CONC", "2")
            .env("DELAY_MS", "0")
            .kill_on_drop(true);
        command
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn reply(State(wire): State<Arc<Wire>>, uri: Uri, body: Bytes) -> Response {
    let value = match uri.path().rsplit('/').next().unwrap_or("") {
        "healthz" => json!({"status": "SERVING"}),
        "store" => json!({"id": "store"}),
        "model" => json!({"authorization_model": {"id": "model"}}),
        "check" | "list-objects" => {
            let request: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(request["consistency"], "HIGHER_CONSISTENCY");
            assert_eq!(request["authorization_model_id"], "model");
            wire.calls.fetch_add(1, Ordering::SeqCst);
            let (status, value) = wire.response.lock().await.clone();
            return (status, Json(value)).into_response();
        }
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    Json(value).into_response()
}

#[tokio::test]
async fn diagnostic_counts_remainders_and_batches_smaller_than_concurrency() -> TestResult {
    let stub = Stub::start().await?;
    for (iters, concurrency, workers) in [(5, 2, 2), (2, 5, 2)] {
        stub.wire.calls.store(0, Ordering::SeqCst);
        let output = timeout(
            Duration::from_secs(10),
            stub.command()
                .env("ITERS", iters.to_string())
                .env("CONC", concurrency.to_string())
                .output(),
        )
        .await??;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(stub.wire.calls.load(Ordering::SeqCst), iters + 3);
        let report = String::from_utf8(output.stdout)?;
        assert!(report.contains("backend=http mode=check"));
        assert!(report.contains(&format!("workers={workers} iters={iters}")));
        assert!(report.contains("request_mean_ms="));
        assert!(report.contains("throughput_rps="));
        assert!(!report.contains("avg_ms="));
    }
    Ok(())
}

#[tokio::test]
async fn invalid_workloads_and_backend_results_never_emit_timings() -> TestResult {
    let stub = Stub::start().await?;
    for (name, value) in [
        ("ITERS", "0"),
        ("CONC", "0"),
        ("ITERS", "invalid"),
        ("CONC", "invalid"),
        ("DELAY_MS", "-1"),
        ("MODE", "invalid"),
    ] {
        let output = timeout(
            Duration::from_secs(10),
            stub.command().env(name, value).output(),
        )
        .await??;
        assert!(!output.status.success(), "{name}={value} must fail");
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains(name));
    }
    assert_eq!(stub.wire.calls.load(Ordering::SeqCst), 0);
    for (status, response) in [
        (StatusCode::OK, json!({"allowed": false})),
        (StatusCode::SERVICE_UNAVAILABLE, json!({"allowed": true})),
    ] {
        *stub.wire.response.lock().await = (status, response);
        let output = timeout(Duration::from_secs(10), stub.command().output()).await??;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
    *stub.wire.response.lock().await = (
        StatusCode::OK,
        json!({"objects": ["lakekeeper_table:analytics/requests"]}),
    );
    for expected in ["[]", "[\"lakekeeper_table:analytics/requests\"]"] {
        let output = timeout(
            Duration::from_secs(10),
            stub.command()
                .env("MODE", "list")
                .env("BENCH_EXPECTED_OBJECTS", expected)
                .output(),
        )
        .await??;
        assert_eq!(output.status.success(), expected != "[]");
        assert_eq!(output.stdout.is_empty(), expected == "[]");
    }
    Ok(())
}
