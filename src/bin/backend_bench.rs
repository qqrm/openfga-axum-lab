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
    let iters: usize = env::var("ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    let conc: usize = env::var("CONC")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let delay_ms: u64 = env::var("DELAY_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let user = env::var("BENCH_USER").unwrap_or_else(|_| "alice".into());
    let relation = env::var("BENCH_RELATION").unwrap_or_else(|_| "reader".into());
    let otype = env::var("BENCH_TYPE").unwrap_or_else(|_| "repo".into());

    let auth: Arc<dyn Authorizer> = match backend.as_str() {
        #[cfg(feature = "grpc")]
        "grpc" => {
            let service_client = openfga_client::client::OpenFgaServiceClient::connect(url).await?;
            let client = openfga_client::client::OpenFgaClient::new(service_client, &store, &model);
            Arc::new(OpenFgaGrpc { client })
        }
        #[cfg(feature = "http")]
        "http" => Arc::new(OpenFgaHttp {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()?,
            url,
            store,
            model,
        }),
        _ => return Err("unsupported backend".into()),
    };

    // Warm up.
    for _ in 0..3 {
        if mode == "list" {
            let _ = auth.list_objects(&user, &relation, &otype).await;
        } else {
            let _ = auth.check(&user, &relation, &format!("{}:1", otype)).await;
        }
    }

    let per_task = iters / conc;
    let started = std::time::Instant::now();
    let mut handles = Vec::new();
    for _ in 0..conc {
        let auth = auth.clone();
        let user = user.clone();
        let relation = relation.clone();
        let otype = otype.clone();
        let mode = mode.clone();
        handles.push(tokio::spawn(async move {
            for _ in 0..per_task {
                if delay_ms > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                }
                if mode == "list" {
                    let _ = auth.list_objects(&user, &relation, &otype).await;
                } else {
                    let _ = auth.check(&user, &relation, &format!("{}:1", otype)).await;
                }
            }
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    let total = started.elapsed().as_secs_f64();
    println!(
        "backend={} mode={} conc={} iters={} total_ms={:.1} avg_ms={:.3} rps={:.1}",
        backend,
        mode,
        conc,
        iters,
        total * 1000.0,
        total * 1000.0 / iters as f64,
        iters as f64 / total
    );
    Ok(())
}
