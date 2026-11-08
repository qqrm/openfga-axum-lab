use openfga_axum_lab::application::Authorizer;
use std::env;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;

#[cfg(feature = "grpc")]
use openfga_axum_lab::infrastructure::OpenFgaGrpc;
#[cfg(feature = "http")]
use openfga_axum_lab::infrastructure::OpenFgaHttp;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let backend = env::var("BACKEND").unwrap_or_else(|_| "grpc".into());
    let url = env::var("FGA_URL").expect("FGA_URL");
    let store = env::var("FGA_STORE").expect("FGA_STORE");
    let model = env::var("FGA_MODEL").expect("FGA_MODEL");
    let iters: usize = env::var("ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(2000);
    let conc: usize = env::var("CONC").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let delay_ms: u64 = env::var("DELAY_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);

    let auth: Arc<dyn Authorizer> = match backend.as_str() {
        #[cfg(feature = "grpc")]
        "grpc" => {
            let service_client = openfga_client::client::OpenFgaServiceClient::connect(url).await?;
            let client = openfga_client::client::OpenFgaClient::new(service_client, &store, &model);
            Arc::new(OpenFgaGrpc { client })
        }
        #[cfg(feature = "http")]
        "http" => Arc::new(OpenFgaHttp {
            client: reqwest::Client::builder().timeout(Duration::from_secs(10)).build()?,
            url,
            store,
            model,
        }),
        _ => return Err("unsupported backend".into()),
    };

    for _ in 0..50 {
        let _ = auth.check("alice", "can_view", "pet:cat").await;
    }

    let per_task = iters / conc;
    let started = Instant::now();
    let mut handles = Vec::new();
    for _ in 0..conc {
        let auth = auth.clone();
        handles.push(tokio::spawn(async move {
            for _ in 0..per_task {
                if delay_ms > 0 {
                    sleep(Duration::from_millis(delay_ms)).await;
                }
                let _ = auth.check("alice", "can_view", "pet:cat").await;
            }
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    let total = started.elapsed().as_secs_f64();
    println!(
        "backend={} conc={} delay_ms={} iters={} total_ms={:.1} rps={:.1}",
        backend, conc, delay_ms, iters, total * 1000.0, iters as f64 / total
    );
    Ok(())
}