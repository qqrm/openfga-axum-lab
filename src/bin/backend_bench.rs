use openfga_axum_lab::application::Authorizer;
use std::env;
use std::time::Instant;

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
    let iters: usize = env::var("ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(1000);

    let auth: Box<dyn Authorizer> = match backend.as_str() {
        #[cfg(feature = "grpc")]
        "grpc" => {
            let service_client = openfga_client::client::OpenFgaServiceClient::connect(url).await?;
            let client = openfga_client::client::OpenFgaClient::new(service_client, &store, &model);
            Box::new(OpenFgaGrpc { client })
        }
        #[cfg(feature = "http")]
        "http" => Box::new(OpenFgaHttp {
            client: reqwest::Client::builder().timeout(std::time::Duration::from_secs(2)).build()?,
            url,
            store,
            model,
        }),
        _ => return Err("unsupported backend".into()),
    };

    // Warm up.
    for _ in 0..100 {
        let _ = auth.check("alice", "can_view", "pet:cat").await;
    }

    let mut samples = Vec::with_capacity(iters);
    let started = Instant::now();
    for _ in 0..iters {
        let t = Instant::now();
        let allowed = auth.check("alice", "can_view", "pet:cat").await.unwrap_or(false);
        samples.push(t.elapsed().as_secs_f64() * 1000.0);
        if !allowed {
            eprintln!("unexpected deny");
        }
    }
    let total = started.elapsed().as_secs_f64();
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = samples[iters / 2];
    let p99 = samples[(iters as f64 * 0.99) as usize];
    let mean = samples.iter().sum::<f64>() / iters as f64;
    println!(
        "backend={} iters={} mean_ms={:.3} p50_ms={:.3} p99_ms={:.3} rps={:.1}",
        backend, iters, mean, p50, p99, iters as f64 / total
    );
    Ok(())
}