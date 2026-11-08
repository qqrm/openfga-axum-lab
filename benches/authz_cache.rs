//! Live adapter diagnostics. This target measures OpenFGA calls, not cache hits.
use criterion::Criterion;
use openfga_axum_lab::application::Authorizer;
#[cfg(feature = "grpc")]
use openfga_axum_lab::infrastructure::OpenFgaGrpc;
#[cfg(feature = "http")]
use openfga_axum_lab::infrastructure::OpenFgaHttp;
use std::env;
#[cfg(feature = "http")]
use std::time::Duration;
use tokio::runtime::Runtime;

async fn checked_request(
    auth: &dyn Authorizer,
    user: &str,
    relation: &str,
    object: &str,
    expected: bool,
) -> bool {
    let allowed = auth
        .check(user, relation, object)
        .await
        .expect("OpenFGA check failed; benchmark is invalid");
    assert_eq!(
        allowed, expected,
        "OpenFGA returned an unexpected decision; benchmark is invalid"
    );
    allowed
}

fn benchmark(c: &mut Criterion, runtime: &Runtime, name: &str, auth: &dyn Authorizer) {
    let user = env::var("BENCH_USER").unwrap_or_else(|_| "alice".into());
    let relation = env::var("BENCH_RELATION").unwrap_or_else(|_| "can_view".into());
    let object = env::var("BENCH_OBJECT").unwrap_or_else(|_| "task:access-review".into());
    let expected: bool = env::var("BENCH_EXPECT_ALLOWED")
        .unwrap_or_else(|_| "true".into())
        .parse()
        .expect("BENCH_EXPECT_ALLOWED must be true or false");
    runtime
        .block_on(auth.ensure_ready())
        .expect("OpenFGA store/model is unavailable; benchmark is invalid");
    runtime.block_on(checked_request(auth, &user, &relation, &object, expected));
    c.bench_function(name, |b| {
        b.to_async(runtime)
            .iter(|| checked_request(auth, &user, &relation, &object, expected));
    });
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = env::args_os().skip(1).collect();
    // Cargo test --all-targets invokes this custom harness without --bench.
    // Compile it normally, but require an explicit benchmark run for network I/O.
    if !arguments.iter().any(|arg| arg == "--bench") || arguments.iter().any(|arg| arg == "--test")
    {
        eprintln!(
            "Live OpenFGA benchmarks skipped in test mode; run cargo bench --bench authz_cache with FGA_STORE and FGA_MODEL configured."
        );
        return Ok(());
    }

    let mut criterion = Criterion::default().configure_from_args();
    let store = env::var("FGA_STORE").map_err(|_| "FGA_STORE must identify the seeded store")?;
    let model = env::var("FGA_MODEL")
        .map_err(|_| "FGA_MODEL must identify the seeded authorization model")?;
    // The gRPC channel's background tasks must stay on a live runtime for the
    // complete measurement. Do not connect inside a temporary runtime.
    let runtime = Runtime::new()?;
    #[cfg(feature = "grpc")]
    {
        let url = env::var("FGA_URL_GRPC").unwrap_or_else(|_| "http://openfga:8081".into());
        let auth = runtime.block_on(OpenFgaGrpc::connect(url, &store, &model))?;
        benchmark(&mut criterion, &runtime, "grpc_check", &auth);
    }
    #[cfg(feature = "http")]
    {
        let auth = OpenFgaHttp {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(2))
                .build()?,
            url: env::var("FGA_URL_HTTP").unwrap_or_else(|_| "http://openfga:8080".into()),
            store,
            model,
        };
        benchmark(&mut criterion, &runtime, "http_check", &auth);
    }
    criterion.final_summary();
    Ok(())
}
