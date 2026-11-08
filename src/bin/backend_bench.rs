//! Raw OpenFGA diagnostics; these measurements do not exercise task API authorization.
use openfga_axum_lab::application::Authorizer;
#[cfg(feature = "grpc")]
use openfga_axum_lab::infrastructure::OpenFgaGrpc;
#[cfg(feature = "http")]
use openfga_axum_lab::infrastructure::OpenFgaHttp;
use std::{collections::BTreeSet, env, error::Error, str::FromStr, sync::Arc, time::Instant};
use tokio::task::JoinSet;

type BenchResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

enum Backend {
    #[cfg(feature = "grpc")]
    Grpc(OpenFgaGrpc),
    #[cfg(feature = "http")]
    Http(OpenFgaHttp),
}

impl Backend {
    fn authorizer(&self) -> &dyn Authorizer {
        match self {
            #[cfg(feature = "grpc")]
            Self::Grpc(auth) => auth,
            #[cfg(feature = "http")]
            Self::Http(auth) => auth,
        }
    }

    async fn list(
        &self,
        user: &str,
        relation: &str,
        object_type: &str,
    ) -> BenchResult<Vec<String>> {
        Ok(match self {
            #[cfg(feature = "grpc")]
            Self::Grpc(auth) => auth.list_objects(user, relation, object_type).await?,
            #[cfg(feature = "http")]
            Self::Http(auth) => auth.list_objects(user, relation, object_type).await?,
        })
    }
}

enum Operation {
    Check {
        object: String,
        expected: bool,
    },
    List {
        object_type: String,
        expected: BTreeSet<String>,
    },
}

struct Workload {
    user: String,
    relation: String,
    operation: Operation,
}

impl Workload {
    async fn execute(&self, backend: &Backend) -> BenchResult {
        match &self.operation {
            Operation::Check { object, expected } => {
                let actual = backend
                    .authorizer()
                    .check(&self.user, &self.relation, object)
                    .await?;
                if actual != *expected {
                    return Err("unexpected Check decision; benchmark is invalid".into());
                }
            }
            Operation::List {
                object_type,
                expected,
            } => {
                let objects = backend
                    .list(&self.user, &self.relation, object_type)
                    .await?;
                let count = objects.len();
                let actual: BTreeSet<_> = objects.into_iter().collect();
                if actual.len() != count || actual != *expected {
                    return Err("unexpected ListObjects set; benchmark is invalid".into());
                }
            }
        }
        Ok(())
    }
}

fn setting<T: FromStr>(name: &str, default: T) -> BenchResult<T>
where
    T::Err: std::fmt::Display,
{
    match env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|error| format!("{name}: {error}").into()),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(error.into()),
    }
}

#[tokio::main]
async fn main() -> BenchResult {
    let backend_name = setting(
        "BACKEND",
        if cfg!(feature = "http") {
            "http"
        } else {
            "grpc"
        }
        .to_owned(),
    )?;
    let mode = setting("MODE", "check".to_owned())?;
    let iters = setting("ITERS", 20usize)?;
    let requested_conc = setting("CONC", 1usize)?;
    let delay_ms = setting("DELAY_MS", 0u64)?;
    if iters == 0 || requested_conc == 0 {
        return Err("ITERS and CONC must be positive".into());
    }
    let workers = requested_conc.min(iters);
    let operation = match mode.as_str() {
        "check" => Operation::Check {
            object: setting("BENCH_OBJECT", "task:access-review".to_owned())?,
            expected: setting("BENCH_EXPECT_ALLOWED", true)?,
        },
        "list" => {
            let objects: Vec<String> = serde_json::from_str(
                &env::var("BENCH_EXPECTED_OBJECTS")
                    .map_err(|_| "MODE=list requires BENCH_EXPECTED_OBJECTS as a JSON array")?,
            )?;
            let count = objects.len();
            let expected: BTreeSet<_> = objects.into_iter().collect();
            if expected.len() != count {
                return Err("BENCH_EXPECTED_OBJECTS must not contain duplicates".into());
            }
            Operation::List {
                object_type: setting("BENCH_TYPE", "task".to_owned())?,
                expected,
            }
        }
        _ => return Err("MODE must be check or list".into()),
    };
    let workload = Arc::new(Workload {
        user: setting("BENCH_USER", "alice".to_owned())?,
        relation: setting("BENCH_RELATION", "can_view".to_owned())?,
        operation,
    });
    let url = env::var("FGA_URL")?;
    let store = env::var("FGA_STORE")?;
    let model = env::var("FGA_MODEL")?;
    let backend = Arc::new(match backend_name.as_str() {
        #[cfg(feature = "grpc")]
        "grpc" => Backend::Grpc(OpenFgaGrpc::connect(url, &store, &model).await?),
        #[cfg(feature = "http")]
        "http" => Backend::Http(OpenFgaHttp {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(2))
                .build()?,
            url,
            store,
            model,
        }),
        _ => return Err("unsupported backend for the compiled features".into()),
    });
    backend.authorizer().ensure_ready().await?;
    for _ in 0..3 {
        workload.execute(&backend).await?;
    }

    let mut tasks = JoinSet::new();
    let started = Instant::now();
    for worker in 0..workers {
        let backend = backend.clone();
        let workload = workload.clone();
        let count = iters / workers + usize::from(worker < iters % workers);
        tasks.spawn(async move {
            let mut samples = Vec::with_capacity(count);
            for _ in 0..count {
                // Pacing affects throughput, but is not request latency.
                if delay_ms > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                }
                let request_started = Instant::now();
                workload.execute(&backend).await?;
                samples.push(request_started.elapsed().as_secs_f64() * 1000.0);
            }
            Ok::<_, Box<dyn Error + Send + Sync>>(samples)
        });
    }
    let mut samples = Vec::with_capacity(iters);
    while let Some(result) = tasks.join_next().await {
        samples.extend(result??);
    }
    let total = started.elapsed().as_secs_f64();
    if samples.len() != iters {
        return Err("incomplete measurement; benchmark is invalid".into());
    }
    samples.sort_by(f64::total_cmp);
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    let p50 = samples[samples.len() / 2];
    let p99 = samples[(samples.len() as f64 * 0.99) as usize];
    println!(
        "backend={} mode={} requested_conc={} workers={} iters={} delay_ms={} total_ms={:.1} request_mean_ms={:.3} p50_ms={:.3} p99_ms={:.3} throughput_rps={:.1}",
        backend_name,
        mode,
        requested_conc,
        workers,
        samples.len(),
        delay_ms,
        total * 1000.0,
        mean,
        p50,
        p99,
        samples.len() as f64 / total
    );
    Ok(())
}
