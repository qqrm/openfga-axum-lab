use openfga_axum_lab::application::Authorizer;
use std::env;
use std::time::Instant;

#[cfg(feature = "grpc")]
use openfga_axum_lab::infrastructure::OpenFgaGrpc;
#[cfg(feature = "http")]
use openfga_axum_lab::infrastructure::OpenFgaHttp;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let backend = env::var("BACKEND").unwrap_or_else(|_| {
        if cfg!(feature = "grpc") {
            "grpc"
        } else {
            "http"
        }
        .into()
    });
    let url = env::var("FGA_URL")?;
    let store = env::var("FGA_STORE")?;
    let model = env::var("FGA_MODEL")?;
    let iters: usize = env::var("ITERS")
        .ok()
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(1000);
    if iters == 0 {
        return Err("ITERS must be positive".into());
    }

    let auth: Box<dyn Authorizer> = match backend.as_str() {
        #[cfg(feature = "grpc")]
        "grpc" => Box::new(OpenFgaGrpc::connect(url, &store, &model).await?),
        #[cfg(feature = "http")]
        "http" => Box::new(OpenFgaHttp {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(2))
                .build()?,
            url,
            store,
            model,
        }),
        _ => return Err("unsupported backend".into()),
    };

    // Warm up.
    for _ in 0..100 {
        if !auth.check("alice", "can_view", "pet:cat").await? {
            return Err("warmup authorization denied; benchmark is invalid".into());
        }
    }

    let mut samples = Vec::with_capacity(iters);
    let started = Instant::now();
    for _ in 0..iters {
        let t = Instant::now();
        let allowed = auth.check("alice", "can_view", "pet:cat").await?;
        samples.push(t.elapsed().as_secs_f64() * 1000.0);
        if !allowed {
            return Err("measured authorization denied; benchmark is invalid".into());
        }
    }
    let total = started.elapsed().as_secs_f64();
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = samples[iters / 2];
    let p99 = samples[(iters as f64 * 0.99) as usize];
    let mean = samples.iter().sum::<f64>() / iters as f64;
    println!(
        "backend={} iters={} mean_ms={:.3} p50_ms={:.3} p99_ms={:.3} rps={:.1}",
        backend,
        iters,
        mean,
        p50,
        p99,
        iters as f64 / total
    );
    Ok(())
}
