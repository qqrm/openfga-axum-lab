//! Criterion A/A–A/B: full Lakekeeper catalog read, not a raw OpenFGA adapter Check.
#[path = "lakekeeper_support/mod.rs"]
#[allow(dead_code)] // Shared fixture also serves the experiment driver.
mod support;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use serde_json::json;
use std::{path::PathBuf, time::Duration};
use support::{Case, Fixture, Mode, write_json};

fn configuration() -> Criterion {
    let pair =
        PathBuf::from(std::env::var("AUTHZ_BENCH_PAIR_DIR").expect("set isolated pair directory"));
    Criterion::default()
        .sample_size(40)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3))
        .nresamples(10_000)
        .confidence_level(0.95)
        .noise_threshold(0.05)
        .output_directory(&pair.join("criterion"))
}

fn catalog(c: &mut Criterion) {
    let mode = Mode::parse(&std::env::var("AUTHZ_BENCH_MODE").expect("set arm mode"))
        .expect("mode is on or off");
    let label = std::env::var("AUTHZ_BENCH_LABEL").expect("set first or second");
    assert!(label == "first" || label == "second");
    let pair = PathBuf::from(std::env::var("AUTHZ_BENCH_PAIR_DIR").expect("pair directory"));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("Tokio benchmark runtime");
    let fixture = runtime
        .block_on(Fixture::connect(mode))
        .expect("isolated Lakekeeper fixture");
    let mut group = c.benchmark_group("lakekeeper_catalog_http");
    group.throughput(Throughput::Elements(1));
    for case in [Case::AllowFresh, Case::DenyHot] {
        let before = runtime
            .block_on(fixture.snapshot())
            .expect("before counters");
        group.bench_function(case.name(), |b| {
            b.to_async(&runtime).iter(|| async {
                fixture
                    .read(case)
                    .await
                    .expect("incorrect authorization invalidates measurement");
            });
        });
        let after = runtime
            .block_on(fixture.snapshot())
            .expect("after counters");
        let delta = after.subtract(&before).expect("monotonic counters");
        let count = delta
            .validate(mode, case)
            .expect("cache-mode/fresh-Check gate");
        write_json(
            &pair.join(format!("{label}-{}-counters.json", case.name())),
            &json!({
                "mode":mode.name(), "case":case.name(), "decisions":count,
                "window":"Criterion warmup and measurement, not only timed samples",
                "before":before, "after":after, "delta":delta,
                "client":"single in-flight reused HTTP/1.1 pool; response body validated each time",
                "criterion_version":"0.5.x as locked in Cargo.lock",
                "sample_size":40, "warmup_seconds":1, "measurement_seconds":3,
                "nresamples":10_000,"confidence":0.95,
                "arguments":std::env::args().skip(1).collect::<Vec<_>>()
            }),
        )
        .expect("persist counter evidence");
    }
    group.finish();
}

criterion_group! { name = benches; config = configuration(); targets = catalog }
criterion_main!(benches);
