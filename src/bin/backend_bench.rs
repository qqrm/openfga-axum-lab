use openfga_axum_lab::application::Authorizer;
use std::{
    env,
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::task::JoinSet;

#[cfg(feature = "grpc")]
use openfga_axum_lab::infrastructure::OpenFgaGrpc;
#[cfg(feature = "http")]
use openfga_axum_lab::infrastructure::OpenFgaHttp;

type BenchResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Copy)]
enum Mode {
    Check,
    List,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::List => "list",
        }
    }
}

struct Workload {
    mode: Mode,
    user: String,
    relation: String,
    object_type: String,
    object: String,
}

impl Workload {
    async fn request(&self, auth: &dyn Authorizer) -> BenchResult {
        let valid = match self.mode {
            Mode::Check => auth.check(&self.user, &self.relation, &self.object).await?,
            Mode::List => auth
                .list_objects(&self.user, &self.relation, &self.object_type)
                .await?
                .contains(&self.object),
        };
        if !valid {
            return Err(
                "BENCH_OBJECT was denied or absent from ListObjects; benchmark is invalid".into(),
            );
        }
        Ok(())
    }
}

fn setting<T: FromStr>(name: &str, default: T) -> BenchResult<T> {
    match env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|_| format!("{name} must be an unsigned integer").into()),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(error.into()),
    }
}

async fn measure(
    auth: Arc<dyn Authorizer>,
    workload: Arc<Workload>,
    iters: usize,
    concurrency: usize,
    delay: Duration,
) -> BenchResult<(Vec<f64>, Duration)> {
    if iters == 0 || concurrency == 0 {
        return Err("ITERS and CONC must be positive".into());
    }
    let mut workers = JoinSet::new();
    let started = Instant::now();
    for worker in 0..concurrency.min(iters) {
        let auth = auth.clone();
        let workload = workload.clone();
        workers.spawn(async move {
            let mut samples = Vec::new();
            for _ in (worker..iters).step_by(concurrency) {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                let started = Instant::now();
                workload.request(auth.as_ref()).await?;
                samples.push(started.elapsed().as_secs_f64() * 1000.0);
            }
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(samples)
        });
    }
    let mut samples = Vec::with_capacity(iters);
    while let Some(result) = workers.join_next().await {
        samples.extend(result??);
    }
    let elapsed = started.elapsed();
    if samples.len() != iters {
        return Err("measurement count differs from ITERS; benchmark is invalid".into());
    }
    Ok((samples, elapsed))
}

#[tokio::main]
async fn main() -> BenchResult {
    let backend = env::var("BACKEND").unwrap_or_else(|_| {
        if cfg!(feature = "grpc") {
            "grpc"
        } else {
            "http"
        }
        .into()
    });
    let mode = match env::var("MODE").as_deref().unwrap_or("check") {
        "check" => Mode::Check,
        "list" => Mode::List,
        _ => return Err("MODE must be check or list".into()),
    };
    let iters = setting("ITERS", 20usize)?;
    let concurrency = setting("CONC", 1usize)?;
    if iters == 0 || concurrency == 0 {
        return Err("ITERS and CONC must be positive".into());
    }
    let delay = Duration::from_millis(setting("DELAY_MS", 0u64)?);
    let object_type = env::var("BENCH_TYPE").unwrap_or_else(|_| "task".into());
    let object =
        env::var("BENCH_OBJECT").unwrap_or_else(|_| format!("{object_type}:access-review"));
    if !object
        .strip_prefix(&format!("{object_type}:"))
        .is_some_and(|id| !id.is_empty())
    {
        return Err("BENCH_OBJECT must include the BENCH_TYPE prefix and an object ID".into());
    }
    let workload = Arc::new(Workload {
        mode,
        user: env::var("BENCH_USER").unwrap_or_else(|_| "alice".into()),
        relation: env::var("BENCH_RELATION").unwrap_or_else(|_| "can_view".into()),
        object_type,
        object,
    });
    let url = env::var("FGA_URL")?;
    let store = env::var("FGA_STORE")?;
    let model = env::var("FGA_MODEL")?;
    let auth: Arc<dyn Authorizer> = match backend.as_str() {
        #[cfg(feature = "grpc")]
        "grpc" => Arc::new(OpenFgaGrpc::connect(url, &store, &model).await?),
        #[cfg(feature = "http")]
        "http" => Arc::new(OpenFgaHttp {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(2))
                .build()?,
            url,
            store,
            model,
        }),
        _ => return Err("BACKEND is unsupported by the enabled Cargo features".into()),
    };

    auth.ensure_ready().await?;
    for _ in 0..3 {
        workload.request(auth.as_ref()).await?;
    }
    let (mut samples, elapsed) = measure(auth, workload, iters, concurrency, delay).await?;
    samples.sort_by(f64::total_cmp);
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    let p50 = samples[(samples.len() - 1) / 2];
    let p99 = samples[(samples.len() - 1) * 99 / 100];
    let total = elapsed.as_secs_f64();
    println!(
        "backend={} mode={} conc={} workers={} iters={} total_ms={:.1} mean_ms={:.3} p50_ms={:.3} p99_ms={:.3} rps={:.1}",
        backend,
        mode.name(),
        concurrency,
        concurrency.min(iters),
        iters,
        total * 1000.0,
        mean,
        p50,
        p99,
        iters as f64 / total
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use openfga_axum_lab::application::{AuthError, Tuple};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct ObservedBackend {
        calls: AtomicUsize,
        outcome: Result<bool, AuthError>,
    }

    #[async_trait]
    impl Authorizer for ObservedBackend {
        async fn ensure_available(&self) -> Result<(), AuthError> {
            Ok(())
        }
        async fn ensure_ready(&self) -> Result<(), AuthError> {
            Ok(())
        }
        async fn check(&self, _: &str, _: &str, _: &str) -> Result<bool, AuthError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.outcome.as_ref().copied().map_err(|_| AuthError)
        }
        async fn list_objects(&self, _: &str, _: &str, _: &str) -> Result<Vec<String>, AuthError> {
            self.check("", "", "").await.map(|allowed| {
                if allowed {
                    vec!["task:access-review".into()]
                } else {
                    Vec::new()
                }
            })
        }
        async fn write(&self, _: &[Tuple], _: bool) -> Result<(), AuthError> {
            Err(AuthError)
        }
        async fn change(&self, _: &[Tuple], _: &[Tuple]) -> Result<(), AuthError> {
            Err(AuthError)
        }
        async fn delete_object(&self, _: &str) -> Result<(), AuthError> {
            Err(AuthError)
        }
    }

    fn workload(mode: Mode) -> Arc<Workload> {
        Arc::new(Workload {
            mode,
            user: "alice".into(),
            relation: "can_view".into(),
            object_type: "task".into(),
            object: "task:access-review".into(),
        })
    }

    #[tokio::test]
    async fn records_every_request_for_uneven_and_excess_concurrency() -> BenchResult {
        for mode in [Mode::Check, Mode::List] {
            for (iterations, concurrency) in [(17, 4), (3, 8)] {
                let auth = Arc::new(ObservedBackend {
                    calls: AtomicUsize::new(0),
                    outcome: Ok(true),
                });
                let (samples, _) = measure(
                    auth.clone(),
                    workload(mode),
                    iterations,
                    concurrency,
                    Duration::ZERO,
                )
                .await?;
                assert_eq!(samples.len(), iterations);
                assert_eq!(auth.calls.load(Ordering::SeqCst), iterations);
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn backend_errors_denials_and_missing_objects_invalidate_measurement() {
        for mode in [Mode::Check, Mode::List] {
            for outcome in [Ok(false), Err(AuthError)] {
                let auth = Arc::new(ObservedBackend {
                    calls: AtomicUsize::new(0),
                    outcome,
                });
                assert!(
                    measure(auth, workload(mode), 3, 2, Duration::ZERO)
                        .await
                        .is_err()
                );
            }
        }
    }
}
