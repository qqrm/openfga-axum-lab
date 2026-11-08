use criterion::{Criterion, criterion_group, criterion_main};
use openfga_axum_lab::application::Authorizer;
#[cfg(feature = "grpc")]
use openfga_axum_lab::infrastructure::OpenFgaGrpc;
#[cfg(feature = "http")]
use openfga_axum_lab::infrastructure::OpenFgaHttp;
use std::env;
use std::time::Duration;
use tokio::runtime::Runtime;

fn rt() -> Runtime {
    Runtime::new().unwrap()
}

#[cfg(feature = "grpc")]
fn make_grpc() -> Box<dyn Authorizer> {
    let url = env::var("FGA_URL_GRPC").unwrap_or_else(|_| "http://openfga:8081".into());
    let store = env::var("FGA_STORE").expect("FGA_STORE");
    let model = env::var("FGA_MODEL").expect("FGA_MODEL");
    let rt = rt();
    rt.block_on(async {
        let sc = openfga_client::client::OpenFgaServiceClient::connect(url).await.unwrap();
        let client = openfga_client::client::OpenFgaClient::new(sc, &store, &model);
        Box::new(OpenFgaGrpc { client }) as Box<dyn Authorizer>
    })
}

#[cfg(feature = "http")]
fn make_http() -> Box<dyn Authorizer> {
    let url = env::var("FGA_URL_HTTP").unwrap_or_else(|_| "http://openfga:8080".into());
    let store = env::var("FGA_STORE").expect("FGA_STORE");
    let model = env::var("FGA_MODEL").expect("FGA_MODEL");
    Box::new(OpenFgaHttp {
        client: reqwest::Client::builder().timeout(Duration::from_secs(2)).build().unwrap(),
        url,
        store,
        model,
    }) as Box<dyn Authorizer>
}

#[cfg(feature = "grpc")]
fn bench_grpc(c: &mut Criterion) {
    let auth = make_grpc();
    let rt = rt();
    c.bench_function("grpc_check", |b| {
        b.to_async(&rt).iter(|| async {
            let _ = auth.check("alice", "can_view", "pet:cat").await;
        })
    });
}

#[cfg(feature = "http")]
fn bench_http(c: &mut Criterion) {
    let auth = make_http();
    let rt = rt();
    c.bench_function("http_check", |b| {
        b.to_async(&rt).iter(|| async {
            let _ = auth.check("alice", "can_view", "pet:cat").await;
        })
    });
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