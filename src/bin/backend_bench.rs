use openfga_axum_lab::application::Authorizer;
use std::env;
use std::sync::Arc;
use std::time::Instant;

#[cfg(feature = "grpc")]
use openfga_axum_lab::infrastructure::OpenFgaGrpc;
#[cfg(feature = "http")]
use openfga_axum_lab::infrastructure::OpenFgaHttp;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let backend = env::var("BACKEND").unwrap_or_else(|_| "grpc".into());
    let mode = env::var("MODE").unwrap_or_else(|_| "check".into());
    let url = env::var("FGA_URL").expect("FGA_URL");
    let store = env::var("FGA_STORE").expect("FGA_STORE");
    let model = env::var("FGA_MODEL").expect("FGA_MODEL");
    let iters: usize = env::var("ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(100);

    let auth: Arc<dyn Authorizer> = match backend.as_str() {
        #[cfg(feature = "grpc")]
        "grpc" => {
            let service_client = openfga_client::client::OpenFgaServiceClient::connect(url).await?;
            let client = openfga_client::client::OpenFgaClient::new(service_client, &store, &model);
            Arc::new(OpenFgaGrpc { client })
        }
        #[cfg(feature = "http")]
        "http" => Arc::new(OpenFgaHttp {
            client: reqwest::Client::builder().timeout(std::time::Duration::from_secs(10)).build()?,
            url,
            store,
            model,
        }),
        _ => return Err("unsupported backend".into()),
    };

    // Warm up.
    for _ in 0..3 {
        if mode == "list" {
            let _ = auth.list_objects("alice", "can_view", "pet").await;
        } else {
            let _ = auth.check("alice", "can_view", "pet:cat").await;
        }
    }

    let started = Instant::now();
    for _ in 0..iters {
        if mode == "list" {
            let res = auth.list_objects("alice", "can_view", "pet").await.unwrap_or_default();
            if res.is_empty() {
                eprintln!("empty list");
            }
        } else {
            let _ = auth.check("alice", "can_view", "pet:cat").await;
        }
    }
    let total = started.elapsed().as_secs_f64();
    println!(
        "backend={} mode={} iters={} total_ms={:.1} avg_ms={:.3} rps={:.1}",
        backend, mode, iters, total * 1000.0, total * 1000.0 / iters as f64, iters as f64 / total
    );
    Ok(())
}