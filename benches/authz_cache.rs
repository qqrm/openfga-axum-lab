mod support;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use serde_json::json;
use std::{path::PathBuf, time::Duration};
use support::{Fixture, delta, validate_counters, write_json};

fn config() -> Criterion {
    let directory =
        std::env::var("AUTHZ_BENCH_PAIR_DIR").unwrap_or_else(|_| "target/criterion-authz".into());
    Criterion::default()
        .sample_size(40)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3))
        .nresamples(10_000)
        .confidence_level(0.95)
        .significance_level(0.05)
        .noise_threshold(0.05)
        .output_directory(&PathBuf::from(directory).join("criterion"))
}

fn authorization(c: &mut Criterion) {
    let mode = std::env::var("AUTHZ_BENCH_MODE").expect("set AUTHZ_BENCH_MODE=on or off");
    let directory = PathBuf::from(
        std::env::var("AUTHZ_BENCH_PAIR_DIR").unwrap_or_else(|_| "target/criterion-authz".into()),
    );
    let label = std::env::var("AUTHZ_BENCH_LABEL").unwrap_or_else(|_| mode.clone());
    assert!(matches!(label.as_str(), "first" | "second" | "on" | "off"));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("Tokio runtime");
    let fixture = runtime
        .block_on(Fixture::connect(&mode))
        .expect("real isolated stand");
    let mut group = c.benchmark_group("authz_http");
    group.throughput(Throughput::Elements(1));
    for case in ["allow_hot", "deny_hot"] {
        let before = runtime
            .block_on(fixture.snapshot())
            .expect("before counters");
        // Criterion owns the timer, warm-up, sampling, bootstrap and baseline comparison.
        // The measured operation includes request creation, full body consumption and validation.
        group.bench_function(case, |b| {
            b.to_async(&runtime).iter(|| async {
                fixture
                    .read(case)
                    .await
                    .expect("authorization response must remain correct");
            });
        });
        let after = runtime
            .block_on(fixture.snapshot())
            .expect("after counters");
        let difference = delta(&before, &after).expect("monotonic counters");
        let count = validate_counters(&difference, &mode).expect("observed cache mode");
        write_json(
            &directory.join(format!("{label}-{case}-counters.json")),
            &json!({
                "mode":mode, "case":case, "successful_decisions":count,
                "window":"all Criterion warm-up and measurement iterations for this case",
                "delta":difference, "before":before, "after":after,
                "load":"one in-flight HTTP/1.1 request; reused reqwest connection pool",
                "criterion":"0.8.2", "sample_size":40, "warmup_seconds":1,
                "measurement_seconds":3, "resamples":10000,
                "confidence_level":0.95, "noise_threshold":0.05,
                "arguments":std::env::args().skip(1).collect::<Vec<_>>()
            }),
        )
        .expect("write counter evidence");
    }
    group.finish();
}

criterion_group! { name = benches; config = config(); targets = authorization }
criterion_main!(benches);
