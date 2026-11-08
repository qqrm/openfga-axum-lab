//! Raw adapter Check timing, outside the catalog's SQL and mutation-fence boundary.
use criterion::{Criterion, criterion_group, criterion_main};
use openfga_axum_lab::application::Authorizer;
#[cfg(feature = "grpc")]
use openfga_axum_lab::infrastructure::OpenFgaGrpc;
#[cfg(feature = "http")]
use openfga_axum_lab::infrastructure::OpenFgaHttp;
use std::env;
use tokio::runtime::Runtime;

fn setting(name: &str, default: &str) -> String {
    match env::var(name) {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => default.to_owned(),
        Err(error) => panic!("{name}: {error}"),
    }
}

fn measure(c: &mut Criterion, name: &str, runtime: &Runtime, auth: &dyn Authorizer) {
    let user = setting("BENCH_USER", "alice");
    let relation = setting("BENCH_RELATION", "can_get_metadata");
    let object = setting("BENCH_OBJECT", "lakekeeper_table:analytics/requests");
    let expected: bool = setting("BENCH_EXPECT_ALLOWED", "true")
        .parse()
        .expect("BENCH_EXPECT_ALLOWED must be true or false");
    runtime
        .block_on(auth.ensure_ready())
        .expect("backend must be ready");
    c.bench_function(name, |b| {
        b.to_async(runtime).iter(|| async {
            let actual = auth
                .check(&user, &relation, &object)
                .await
                .expect("failed Check invalidates the benchmark");
            assert_eq!(
                actual, expected,
                "unexpected Check decision invalidates the benchmark"
            );
        })
    });
}

#[cfg(feature = "grpc")]
fn bench_grpc(c: &mut Criterion) {
    // The same runtime owns the connection and remains alive throughout timing.
    let runtime = Runtime::new().expect("benchmark runtime");
    let auth = runtime
        .block_on(OpenFgaGrpc::connect(
            setting("FGA_URL_GRPC", "http://openfga:8081"),
            &env::var("FGA_STORE").expect("FGA_STORE"),
            &env::var("FGA_MODEL").expect("FGA_MODEL"),
        ))
        .expect("connect to benchmark OpenFGA");
    measure(c, "grpc_check", &runtime, &auth);
}

#[cfg(feature = "http")]
fn bench_http(c: &mut Criterion) {
    let runtime = Runtime::new().expect("benchmark runtime");
    let auth = OpenFgaHttp {
        client: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .expect("HTTP client"),
        url: setting("FGA_URL_HTTP", "http://openfga:8080"),
        store: env::var("FGA_STORE").expect("FGA_STORE"),
        model: env::var("FGA_MODEL").expect("FGA_MODEL"),
    };
    measure(c, "http_check", &runtime, &auth);
}

#[cfg(feature = "grpc")]
criterion_group!(grpc_benches, bench_grpc);
#[cfg(feature = "http")]
criterion_group!(http_benches, bench_http);

#[cfg(all(feature = "grpc", feature = "http"))]
criterion_main!(grpc_benches, http_benches);
#[cfg(all(feature = "grpc", not(feature = "http")))]
criterion_main!(grpc_benches);
#[cfg(all(feature = "http", not(feature = "grpc")))]
criterion_main!(http_benches);
