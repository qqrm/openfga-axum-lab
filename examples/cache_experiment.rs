//! Orchestration and correctness gates; all inferential benchmark statistics come from Criterion.
#[path = "../benches/support/mod.rs"]
mod support;

use reqwest::Method;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::{Path, PathBuf}, process::Command, sync::Arc, time::Instant};
use support::{BenchResult, Fixture, delta, validate_counters, write_json};

const CASES: [&str; 2] = ["allow_hot", "deny_hot"];
const RELATION: &str = "/pets/fox/relations/caregiver/dave";

fn compose(args: &[&str]) -> BenchResult {
    let project = std::env::var("COMPOSE_PROJECT_NAME")?;
    if !project.starts_with("authz-criterion-") {
        return Err("use a disposable COMPOSE_PROJECT_NAME starting with authz-criterion-".into());
    }
    let status = Command::new("docker").args(["compose", "-f", "compose.yaml", "-f", "compose.cache-ab.yaml"])
        .args(args).status()?;
    if !status.success() { return Err(format!("docker compose {args:?} failed").into()); }
    Ok(())
}

fn permutation(length: usize, seed: u64, domain: &str) -> Vec<usize> {
    let mut indices: Vec<_> = (0..length).collect();
    indices.sort_by_cached_key(|index| Sha256::digest(format!("{seed}:{domain}:{index}")).to_vec());
    indices
}

fn host() -> Value {
    json!({"cpu_stat":std::fs::read_to_string("/proc/stat").ok(),
        "cpu_pressure":std::fs::read_to_string("/proc/pressure/cpu").ok(),
        "loadavg":std::fs::read_to_string("/proc/loadavg").ok()})
}

fn read_json(path: &Path) -> BenchResult<Value> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

fn criterion_arm(directory: &Path, mode: &str, label: &str, fixture: &Fixture) -> BenchResult {
    let argument = if label == "first" { "--save-baseline" } else { "--baseline" };
    let output = Command::new("cargo")
        .args(["bench", "--locked", "--bench", "authz_cache", "--", argument, "reference", "--noplot"])
        .env("AUTHZ_BENCH_MODE", mode)
        .env("AUTHZ_BENCH_LABEL", label)
        .env("AUTHZ_BENCH_PAIR_DIR", directory)
        .env("BENCH_ALICE_TOKEN", &fixture.alice)
        .env("BENCH_DAVE_TOKEN", &fixture.dave)
        .output()?;
    std::fs::write(directory.join(format!("{label}-stdout.log")), &output.stdout)?;
    std::fs::write(directory.join(format!("{label}-stderr.log")), &output.stderr)?;
    if !output.status.success() {
        return Err(format!("Criterion {label}/{mode} failed; inspect {}", directory.display()).into());
    }
    Ok(())
}

fn comparison(directory: &Path, case: &str, first_mode: &str, second_mode: &str) -> BenchResult<Value> {
    let root = directory.join("criterion/authz_http").join(case);
    let first = read_json(&root.join("reference/estimates.json"))?;
    let second = read_json(&root.join("new/estimates.json"))?;
    let change = read_json(&root.join("change/estimates.json"))?;
    let number = |pointer: &str| -> BenchResult<f64> {
        let n = change.pointer(pointer).and_then(Value::as_f64).ok_or("missing Criterion change estimate")?;
        if !n.is_finite() || n <= -1.0 { return Err("invalid Criterion relative estimate".into()); }
        Ok(1.0 + n)
    };
    let point = number("/mean/point_estimate")?;
    let lower = number("/mean/confidence_interval/lower_bound")?;
    let upper = number("/mean/confidence_interval/upper_bound")?;
    let (point, lower, upper) = if first_mode == "on" && second_mode == "off" {
        (1.0 / point, 1.0 / upper, 1.0 / lower)
    } else { (point, lower, upper) };
    Ok(json!({"case":case, "first_mode":first_mode,"second_mode":second_mode,
        "first_estimates":first,"second_estimates":second,"criterion_relative_change":change,
        "ratio_definition":if first_mode == second_mode {"second/first (A/A)"} else {"on/off (A/B)"},
        "mean_ratio":{"point_estimate":point,"confidence_interval_95":[lower,upper]},
        "interval_scope":"Criterion samples within this pair, not independent hosts",
        "first_counters":read_json(&directory.join(format!("first-{case}-counters.json")))?,
        "second_counters":read_json(&directory.join(format!("second-{case}-counters.json")))?}))
}

async fn revocations(fixtures: &[Arc<Fixture>; 2]) -> BenchResult<Value> {
    let mut records = Vec::new();
    for (writer_index, writer) in fixtures.iter().enumerate() {
        let carol = writer.login("carol").await?;
        writer.expect(&carol, Method::PUT, RELATION, 204).await?;
        let result = async {
            let before = fixtures[1].snapshot().await?;
            for fixture in fixtures {
                for _ in 0..3 { fixture.expect(&fixture.dave, Method::GET, "/pets/fox", 200).await?; }
            }
            let after = fixtures[1].snapshot().await?;
            let warmed = delta(&before, &after)?;
            if warmed.counters.get("authz_cache_events_total{event=\"hit_allow\"}").copied().unwrap_or(0) < 1 {
                return Err("revocation did not warm the cache".into());
            }
            let start = Instant::now();
            writer.expect(&carol, Method::DELETE, RELATION, 204).await?;
            let latency = start.elapsed().as_nanos();
            let acknowledged = Instant::now();
            let mut denials = Vec::new();
            for (mode, fixture) in ["off", "on"].into_iter().zip(fixtures) {
                for _ in 0..10 {
                    let started = acknowledged.elapsed().as_nanos();
                    fixture.expect(&fixture.dave, Method::GET, "/pets/fox", 403).await?;
                    denials.push(json!({"mode":mode,"status":403,"started_after_ack_ns":started}));
                }
            }
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(json!({
                "writer":["off","on"][writer_index],"revoke_latency_ns":latency,"post_ack_denials":denials}))
        }.await;
        // Restore the original fixture even when the check fails.
        let cleanup = writer.expect(&carol, Method::DELETE, RELATION, 204).await;
        records.push(result?);
        cleanup?;
    }
    Ok(json!(records))
}

async fn fail_closed(fixtures: &[Arc<Fixture>; 2]) -> BenchResult<Value> {
    for fixture in fixtures { for _ in 0..3 { fixture.read("allow_hot").await?; } }
    compose(&["stop", "-t", "1", "openfga"])?;
    let result = async {
        for fixture in fixtures { fixture.expect(&fixture.alice, Method::GET, "/pets/fox", 503).await?; }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(json!({"off":503,"on":503}))
    }.await;
    let restart = compose(&["start", "openfga"]);
    let value = result?;
    restart?;
    Ok(value)
}

fn percentile(sorted: &[u64], numerator: usize) -> u64 {
    sorted[(sorted.len() * numerator).div_ceil(100).saturating_sub(1)]
}

async fn diagnostic(fixture: Arc<Fixture>, mode: &str, case: &'static str, path: &Path) -> BenchResult<Value> {
    const WORKERS: usize = 4;
    const PER_WORKER: usize = 500;
    for _ in 0..200 { fixture.read(case).await?; }
    let before = fixture.snapshot().await?;
    let barrier = Arc::new(tokio::sync::Barrier::new(WORKERS + 1));
    let mut tasks = tokio::task::JoinSet::new();
    for worker in 0..WORKERS {
        let fixture = fixture.clone();
        let barrier = barrier.clone();
        tasks.spawn(async move {
            barrier.wait().await;
            let mut raw = Vec::with_capacity(PER_WORKER);
            for index in 0..PER_WORKER {
                let start = Instant::now();
                let result = fixture.read(case).await;
                raw.push(json!({"index":worker * PER_WORKER + index,
                    "latency_ns":start.elapsed().as_nanos(),
                    "error":result.err().map(|error| error.to_string())}));
            }
            raw
        });
    }
    let start = Instant::now();
    barrier.wait().await;
    let mut raw = Vec::with_capacity(WORKERS * PER_WORKER);
    while let Some(result) = tasks.join_next().await { raw.extend(result?); }
    let elapsed = start.elapsed();
    let after = fixture.snapshot().await?;
    let difference = delta(&before, &after)?;
    let errors = raw.iter().filter(|sample| !sample["error"].is_null()).count();
    let mut values: Vec<_> = raw.iter().filter_map(|sample| sample["latency_ns"].as_u64()).collect();
    values.sort_unstable();
    let report = json!({"mode":mode,"case":case,"kind":"separate Rust HTTP diagnostic, NOT Criterion sample quantiles",
        "workers":WORKERS,"warmup_requests":200,"requests":raw.len(),"errors":errors,
        "elapsed_ns":elapsed.as_nanos(),"throughput_rps":(raw.len()-errors) as f64 / elapsed.as_secs_f64(),
        "latency_ns":{"p50":percentile(&values,50),"p95":percentile(&values,95),"p99":percentile(&values,99)},
        "quantile_method":"nearest rank; invalid if any request errors",
        "before":before,"after":after,"delta":difference,"raw":raw});
    write_json(path, &report)?;
    if errors != 0 || validate_counters(&difference, mode)? != (WORKERS * PER_WORKER) as u64 {
        return Err("invalid HTTP diagnostic; raw observations saved".into());
    }
    Ok(json!({"mode":mode,"case":case,"path":path,"requests":WORKERS*PER_WORKER,"errors":errors,
        "throughput_rps":report["throughput_rps"],"latency_ns":report["latency_ns"],"delta":report["delta"]}))
}

async fn run(root: &Path, report: &mut Value) -> BenchResult {
    let fixtures = [Arc::new(Fixture::connect("off").await?), Arc::new(Fixture::connect("on").await?)];
    report["revocation_before"] = revocations(&fixtures).await?;
    write_json(&root.join("experiment.json"), report)?;
    let seed: u64 = std::env::var("AB_SEED").unwrap_or_else(|_| "2606".into()).parse()?;
    let definitions = [("AA-off", "off", "off"), ("AA-on", "on", "on"), ("AB", "off", "on")];
    for stage in permutation(definitions.len(), seed, "stages") {
        let (name, a, b) = definitions[stage];
        for (block, index) in permutation(4, seed, name).into_iter().enumerate() {
            let (first, second) = if index < 2 { (a, b) } else { (b, a) };
            let directory = root.join(format!("{name}-{block}"));
            std::fs::create_dir_all(&directory)?;
            let before = host();
            println!("Criterion {name} block {block}: {first} -> {second}");
            for (label, mode) in [("first",first),("second",second)] {
                criterion_arm(&directory, mode, label, &fixtures[usize::from(mode == "on")])?;
            }
            let comparisons: Vec<_> = CASES.iter().map(|case| comparison(&directory,case,first,second))
                .collect::<BenchResult<_>>()?;
            report["pairs"].as_array_mut().ok_or("missing pairs")?.push(json!({
                "stage":name,"block":block,"order":if index < 2 {"AB"} else {"BA"},
                "first_mode":first,"second_mode":second,"comparisons":comparisons,
                "host_before":before,"host_after":host()}));
            write_json(&root.join("experiment.json"), report)?;
        }
    }
    for (mode, fixture) in ["off","on"].into_iter().zip(&fixtures) {
        for case in CASES {
            let result = diagnostic(fixture.clone(), mode, case, &root.join(format!("diagnostic-{mode}-{case}.json"))).await?;
            report["diagnostics"].as_array_mut().ok_or("missing diagnostics")?.push(result);
        }
    }
    report["revocation_after"] = revocations(&fixtures).await?;
    report["fail_closed"] = fail_closed(&fixtures).await?;
    Ok(())
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> BenchResult {
    let root = PathBuf::from("artifacts/criterion-experiment");
    if root.join("experiment.json").exists() {
        return Err("refusing to reuse an existing experiment directory; archive it first".into());
    }
    let commit = Command::new("git").args(["rev-parse","HEAD"]).output()?;
    if !commit.status.success() { return Err("cannot establish measured commit".into()); }
    let mut report = json!({"valid":false,"criterion_version":"0.8.2",
        "commit":String::from_utf8(commit.stdout)?.trim(),
        "cargo_lock_sha256":format!("{:x}",Sha256::digest(std::fs::read("Cargo.lock")?)),
        "replica":std::env::var("BENCH_REPLICA").unwrap_or_else(|_| "local".into()),
        "seed":std::env::var("AB_SEED").unwrap_or_else(|_| "2606".into()),
        "plan":{"blocks_per_stage":4,"cases":CASES,"criterion_clients":1,
            "diagnostic_clients":4,"practical_threshold":0.05},
        "pairs":[],"diagnostics":[],
        "limitations":["Criterion interval is within-pair, not a population-of-hosts interval",
            "Criterion iteration mean/median is not an HTTP request p50/p95/p99",
            "Separate diagnostic retains individual HTTP observations and fixed-window counters",
            "Tiny hot-key workload, pooled HTTP/1.1, shared VM; not production capacity",
            "No automatic merge/regression decision; inspect A/A and independent runners"]});
    write_json(&root.join("experiment.json"), &report)?;
    match run(&root, &mut report).await {
        Ok(()) => { report["valid"] = json!(true); }
        Err(error) => {
            report["failure"] = json!(error.to_string());
            write_json(&root.join("experiment.json"), &report)?;
            return Err(error);
        }
    }
    write_json(&root.join("experiment.json"), &report)?;
    println!("Verified Criterion experiment: {}", root.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_are_balanced_and_reproducible() {
        let order = permutation(4, 2606, "AB");
        assert_eq!(order, permutation(4, 2606, "AB"));
        assert_eq!(order.iter().filter(|&&index| index < 2).count(), 2);
        let mut sorted = order;
        sorted.sort_unstable();
        assert_eq!(sorted, vec![0,1,2,3]);
    }

    #[test]
    fn nearest_rank_tails_are_not_means() {
        let samples: Vec<_> = (1..=2000).collect();
        assert_eq!(percentile(&samples,50),1000);
        assert_eq!(percentile(&samples,95),1900);
        assert_eq!(percentile(&samples,99),1980);
    }
}
