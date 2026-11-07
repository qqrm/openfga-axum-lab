#[path = "support/stack.rs"]
pub mod stack;

use reqwest::Client;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use stack::{Stack, TestResult};
use std::{path::PathBuf, time::Instant};
use tokio::task::JoinSet;

#[derive(Serialize)]
struct Sample {
    index: usize,
    latency_ns: u64,
    status: Option<u16>,
    error: Option<String>,
}

#[derive(Serialize)]
struct Arm {
    elapsed_ns: u64,
    p50_ns: Option<f64>,
    error_count: usize,
    warmup: Vec<Sample>,
    raw: Vec<Sample>,
}

fn median(values: &[u64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let middle = sorted.len() / 2;
    Some(if sorted.len().is_multiple_of(2) {
        (sorted[middle - 1] as f64 + sorted[middle] as f64) / 2.0
    } else {
        sorted[middle] as f64
    })
}

fn percentile(values: &[u64], numerator: usize, denominator: usize) -> Option<u64> {
    assert!(denominator > 0 && numerator <= denominator);
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted
        .len()
        .checked_sub(1)
        .map(|last| sorted[last * numerator / denominator])
}

async fn one(client: &Client, url: &str, token: &str, index: usize) -> Sample {
    let request = client
        .get(format!("{url}/pets/fox"))
        .bearer_auth(token)
        .header(reqwest::header::CONNECTION, "close");
    let start = Instant::now();
    let mut status = None;
    let outcome = async {
        let response = request.send().await?;
        status = Some(response.status().as_u16());
        let body: Value = response.json().await?;
        if status != Some(200) || body["id"] != "fox" {
            return Err("unexpected response".into());
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    }
    .await;
    Sample {
        index,
        latency_ns: start.elapsed().as_nanos().try_into().unwrap_or(u64::MAX),
        status,
        error: outcome.err().map(|error| error.to_string()),
    }
}

async fn requests(
    client: &Client,
    url: &str,
    token: &str,
    count: usize,
    concurrency: usize,
) -> TestResult<Vec<Sample>> {
    assert!(concurrency > 0);
    let mut workers = JoinSet::new();
    for worker in 0..concurrency.min(count) {
        let client = client.clone();
        let url = url.to_owned();
        let token = token.to_owned();
        workers.spawn(async move {
            let mut samples = Vec::new();
            for index in (worker..count).step_by(concurrency) {
                samples.push(one(&client, &url, &token, index).await);
            }
            samples
        });
    }
    let mut samples = Vec::with_capacity(count);
    while let Some(result) = workers.join_next().await {
        samples.extend(result?);
    }
    samples.sort_unstable_by_key(|sample| sample.index);
    assert_eq!(samples.len(), count, "missing measurement observations");
    assert!(
        samples
            .iter()
            .enumerate()
            .all(|(index, sample)| index == sample.index)
    );
    Ok(samples)
}

async fn arm(
    client: &Client,
    url: &str,
    token: &str,
    paired: bool,
    concurrency: usize,
) -> TestResult<Arm> {
    let (warmup_count, count, warmup_concurrency) = if paired {
        (20, 120, concurrency)
    } else {
        (50, 400, 1)
    };
    let warmup = requests(client, url, token, warmup_count, warmup_concurrency).await?;
    let start = Instant::now();
    let raw = requests(client, url, token, count, concurrency).await?;
    let elapsed_ns = start.elapsed().as_nanos().try_into()?;
    let error_count = warmup
        .iter()
        .chain(&raw)
        .filter(|sample| sample.error.is_some())
        .count();
    let values: Vec<_> = raw
        .iter()
        .filter(|sample| sample.error.is_none())
        .map(|sample| sample.latency_ns)
        .collect();
    Ok(Arm {
        elapsed_ns,
        p50_ns: median(&values),
        error_count,
        warmup,
        raw,
    })
}

fn environment() -> TestResult<Value> {
    let binary = std::fs::read(env!("CARGO_BIN_EXE_openfga-axum-lab"))?;
    let rustc = std::process::Command::new("rustc")
        .arg("--version")
        .output()?;
    if !rustc.status.success() {
        return Err("could not record Rust toolchain".into());
    }
    Ok(json!({
        "timestamp_unix_ns": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos(),
        "commit": std::env::var("GITHUB_SHA").unwrap_or_else(|_| "unrecorded local working tree".into()),
        "runner": std::env::var("RUNNER_NAME").unwrap_or_else(|_| "local".into()),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "cpu_count": std::thread::available_parallelism()?.get(),
        "rustc": String::from_utf8(rustc.stdout)?.trim(),
        "build": if cfg!(debug_assertions) { "debug" } else { "release" },
        "clock": {"implementation": "std::time::Instant", "monotonic": true,
            "reported_unit": "nanoseconds", "hardware_resolution": "not measured"},
        "client_process_cpu_ns": null,
        "cpu_measurement": "not collected by the portable Rust client",
        "app_sha256": Sha256::digest(&binary).iter().map(|b| format!("{:02x}", b)).collect::<String>(),
        "benchmark_sha256": Sha256::digest(include_bytes!("performance.rs")).iter().map(|b| format!("{:02x}", b)).collect::<String>(),
        "load_model": "closed-loop fixed concurrency, one HTTP connection per request",
        "operation": "GET /pets/fox",
        "timeout_seconds": 5,
        "retries": 0
    }))
}

fn artifacts() -> TestResult<PathBuf> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("artifacts");
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

fn measurement_client() -> TestResult<Client> {
    Ok(Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .pool_max_idle_per_host(0)
        .timeout(std::time::Duration::from_secs(5))
        .build()?)
}

#[tokio::test]
#[ignore = "requires a local Docker engine; report-only latency diagnostic"]
async fn diagnostic_latency() -> TestResult {
    let client = measurement_client()?;
    let stack = Stack::start(&client).await?;
    let app = stack.app(&stack.fga_url).await?;
    let token = stack.login(&client, &app, "alice").await?;
    let mut lanes = Vec::new();
    let mut errors = 0;
    for concurrency in [1, 8] {
        let result = arm(&client, &app.url, &token, false, concurrency).await?;
        errors += result.error_count;
        let values: Vec<_> = result
            .raw
            .iter()
            .filter(|s| s.error.is_none())
            .map(|s| s.latency_ns)
            .collect();
        lanes.push(json!({
            "concurrency": concurrency, "warmup_requests": 50, "requested": 400,
            "completed": result.raw.len(), "successful": values.len(),
            "error_count_including_warmup": result.error_count,
            "elapsed_ns": result.elapsed_ns,
            "completed_per_second": result.raw.len() as f64 * 1e9 / result.elapsed_ns as f64,
            "latency_ns": {"p50": result.p50_ns, "p95": percentile(&values, 95, 100),
                "p99": percentile(&values, 99, 100)},
            "raw_warmup_requests": result.warmup, "raw_requests": result.raw
        }));
    }
    let report = json!({"purpose": "report-only, not a regression or capacity claim",
        "environment": environment()?, "lanes": lanes});
    let directory = artifacts()?;
    std::fs::write(
        directory.join("benchmark.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    stack
        .diagnostics(&directory.join("benchmark-environment"))
        .await?;
    assert_eq!(
        errors, 0,
        "invalid measurement: inspect artifacts/benchmark.json"
    );
    println!("Recorded 800 measured requests and 100 warmups; no performance verdict.");
    Ok(())
}

// Seeded SplitMix64 is used only for experimental ordering/resampling, never credentials.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
        value ^ (value >> 31)
    }

    fn index(&mut self, length: usize) -> usize {
        assert!(length > 0);
        let bound = length as u64;
        let limit = u64::MAX - u64::MAX % bound;
        loop {
            let value = self.next();
            if value < limit {
                return (value % bound) as usize;
            }
        }
    }

    fn order(&mut self) -> [&'static str; 6] {
        let mut order = ["AB", "AB", "AB", "BA", "BA", "BA"];
        for index in (1..order.len()).rev() {
            let other = self.index(index + 1);
            order.swap(index, other);
        }
        order
    }
}

fn geometric_mean(values: &[f64]) -> f64 {
    assert!(!values.is_empty() && values.iter().all(|value| *value > 0.0 && value.is_finite()));
    (values.iter().map(|value| value.ln()).sum::<f64>() / values.len() as f64).exp()
}

fn bootstrap_interval(ratios: &[f64], rng: &mut Rng) -> [f64; 2] {
    let mut bootstrap = Vec::with_capacity(5000);
    for _ in 0..5000 {
        let sample: Vec<_> = (0..ratios.len())
            .map(|_| ratios[rng.index(ratios.len())])
            .collect();
        bootstrap.push(geometric_mean(&sample));
    }
    bootstrap.sort_by(f64::total_cmp);
    [bootstrap[125], bootstrap[4874]]
}

#[tokio::test]
#[ignore = "requires Docker; run only through the manual performance workflow"]
async fn paired_aa_calibration() -> TestResult {
    let client = measurement_client()?;
    let stack = Stack::start(&client).await?;
    let app = stack.app(&stack.fga_url).await?;
    let token = stack.login(&client, &app, "alice").await?;
    let mut rng = Rng(2606);
    let mut blocks = Vec::new();
    let mut ratios = Vec::new();
    let mut errors = 0;
    for (index, order) in rng.order().iter().enumerate() {
        let mut arms = serde_json::Map::new();
        let mut medians = [None, None];
        for label in order.chars() {
            let result = arm(&client, &app.url, &token, true, 4).await?;
            medians[usize::from(label == 'B')] = result.p50_ns;
            errors += result.error_count;
            arms.insert(label.to_string(), serde_json::to_value(result)?);
        }
        if let [Some(a), Some(b)] = medians
            && a > 0.0
            && b > 0.0
        {
            ratios.push(b / a);
        }
        blocks.push(json!({"index": index, "order": order, "arms": arms}));
    }
    let valid = errors == 0 && ratios.len() == 6;
    let statistics = if valid {
        json!({"ratio_b_over_a": geometric_mean(&ratios),
            "same_run_bootstrap_95pct": bootstrap_interval(&ratios, &mut rng)})
    } else {
        Value::Null
    };
    let report = json!({
        "purpose": "A/A calibration only; both labels call the same deployed binary",
        "decision": "no performance verdict; six blocks in one VM do not estimate between-VM variability",
        "environment": environment()?, "seed": 2606, "rng": "SplitMix64",
        "load": {"concurrency": 4, "samples_per_arm": 120, "warmup_per_arm": 20},
        "blocks": blocks, "error_count": errors, "statistics": statistics
    });
    let directory = artifacts()?;
    std::fs::write(
        directory.join("paired-aa.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    stack
        .diagnostics(&directory.join("paired-aa-environment"))
        .await?;
    assert!(
        valid,
        "invalid measurement: inspect artifacts/paired-aa.json"
    );
    println!("Recorded six balanced A/A blocks; no regression verdict.");
    Ok(())
}

#[test]
fn summary_statistics_handle_empty_even_and_odd_samples() {
    assert_eq!(median(&[]), None);
    assert_eq!(median(&[3, 1, 2]), Some(2.0));
    assert_eq!(median(&[4, 1, 3, 2]), Some(2.5));
    assert_eq!(percentile(&[], 95, 100), None);
    assert_eq!(percentile(&[40, 10, 30, 20], 95, 100), Some(30));
}

#[test]
fn ordering_is_balanced_and_reproducible() {
    let first = Rng(2606).order();
    assert_eq!(first, Rng(2606).order());
    assert_eq!(first.iter().filter(|value| **value == "AB").count(), 3);
    assert_eq!(first.iter().filter(|value| **value == "BA").count(), 3);
}

#[test]
fn paired_statistics_preserve_an_identical_ratio() {
    assert_eq!(geometric_mean(&[1.0; 6]), 1.0);
    assert_eq!(bootstrap_interval(&[1.0; 6], &mut Rng(2606)), [1.0, 1.0]);
}
