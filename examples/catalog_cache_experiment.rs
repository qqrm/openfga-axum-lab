//! Isolated Lakekeeper Criterion driver. No CI scheduling or automatic verdict.
#[path = "../benches/lakekeeper_support/mod.rs"]
#[allow(dead_code)] // Shared fixture also serves the Criterion bench.
mod support;

use reqwest::Method;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};
use support::{BenchResult, Case, Fixture, Mode, TABLE, write_json};

const CASES: [Case; 2] = [Case::AllowFresh, Case::DenyHot];

fn sha256(path: &Path) -> BenchResult<String> {
    let digest = Sha256::digest(std::fs::read(path)?);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}
fn permutation(length: usize, seed: u64, domain: &str) -> Vec<usize> {
    let mut order: Vec<usize> = (0..length).collect();
    order.sort_by_cached_key(|index| Sha256::digest(format!("{seed}:{domain}:{index}")).to_vec());
    order
}
fn compose(args: &[&str]) -> BenchResult<String> {
    let name = std::env::var("COMPOSE_PROJECT_NAME")?;
    if !name.starts_with("authz-criterion-") || name.len() < "authz-criterion-x".len() {
        return Err("set a disposable COMPOSE_PROJECT_NAME beginning authz-criterion-".into());
    }
    let output = Command::new("docker")
        .args([
            "compose",
            "-f",
            "compose.yaml",
            "-f",
            "compose.cache-lakekeeper.yaml",
        ])
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "docker compose {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}
// Read-only SQL witness: every benchmark arm must see the same catalog and
// generation, with no outstanding authorization mutation.
fn dataset_fingerprint() -> BenchResult<Value> {
    let sql = "SELECT generation::text || ':' || pending::text FROM authz_cache_state WHERE singleton; \
        SELECT md5(COALESCE(string_agg(row_to_json(t)::text, ';' \
        ORDER BY t.project_id,t.warehouse_id,t.id),'')) \
        FROM (SELECT project_id,warehouse_id,id,namespace_id,owner_id,status,name,description \
        FROM catalog_tables) AS t;";
    let output = compose(&[
        "exec",
        "-T",
        "postgres",
        "psql",
        "-XAt",
        "-v",
        "ON_ERROR_STOP=1",
        "-U",
        "authzlab",
        "-d",
        "authzlab",
        "-c",
        sql,
    ])?;
    let lines: Vec<_> = output.lines().collect();
    if lines.len() != 2 || !lines[0].ends_with(":0") || lines[1].len() != 32 {
        return Err(format!("unfenced SQL witness missing or invalid: {output}").into());
    }
    Ok(json!({"authorization_generation_pending":lines[0], "catalog_data_md5":lines[1]}))
}

fn assert_identical_server_binaries() -> BenchResult<Value> {
    let on = compose(&["exec", "-T", "app", "sha256sum", "/usr/local/bin/app"])?;
    let off = compose(&[
        "exec",
        "-T",
        "app-uncached",
        "sha256sum",
        "/usr/local/bin/app",
    ])?;
    if on != off || on.is_empty() {
        return Err("server binary hashes differ between cache arms".into());
    }
    let images = compose(&["images", "-q", "app", "app-uncached"])?;
    let mut ids: Vec<_> = images.lines().collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.len() != 1 || ids[0].is_empty() {
        return Err("cache arms do not use the same container image".into());
    }
    Ok(json!({"server_sha256":on,"container_image_id":ids[0]}))
}

fn read_json(path: &Path) -> BenchResult<Value> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}
fn benchmark_cli_args(label: &str) -> BenchResult<[&'static str; 4]> {
    let baseline = match label {
        "first" => "--save-baseline",
        "second" => "--baseline",
        _ => return Err("benchmark arm label must be first or second".into()),
    };
    Ok(["--bench", baseline, "reference", "--noplot"])
}
fn bench_arm(bin: &Path, pair: &Path, label: &str, mode: Mode, fixture: &Fixture) -> BenchResult {
    // Criterion defaults to smoke-test mode when invoked without Cargo.
    // An explicit --bench is mandatory for measured samples and estimates.
    let args = benchmark_cli_args(label)?;
    let output = Command::new(bin)
        .args(args)
        .env("AUTHZ_BENCH_PAIR_DIR", pair)
        .env("AUTHZ_BENCH_LABEL", label)
        .env("AUTHZ_BENCH_MODE", mode.name())
        .env("BENCH_ALICE_TOKEN", &fixture.alice)
        .env("BENCH_CAROL_TOKEN", &fixture.carol)
        .env("BENCH_DAVE_TOKEN", &fixture.dave)
        .output()?;
    std::fs::write(pair.join(format!("{label}-stdout.log")), &output.stdout)?;
    std::fs::write(pair.join(format!("{label}-stderr.log")), &output.stderr)?;
    if !output.status.success() {
        return Err(format!(
            "Criterion failed for {label}/{}; see pair logs",
            mode.name()
        )
        .into());
    }
    // Smoke tests return success without any measured data. Reject them.
    let estimate = if label == "first" { "reference" } else { "new" };
    for case in CASES {
        let measured = pair
            .join("criterion/lakekeeper_catalog_http")
            .join(case.name())
            .join(estimate)
            .join("estimates.json");
        if !measured.is_file() {
            return Err(format!(
                "Criterion emitted no estimates at {}; benchmark mode required",
                measured.display()
            )
            .into());
        }
    }
    Ok(())
}
fn comparison(pair: &Path, case: Case, first: Mode, second: Mode) -> BenchResult<Value> {
    let base = pair
        .join("criterion/lakekeeper_catalog_http")
        .join(case.name());
    let a = read_json(&base.join("reference/estimates.json"))?;
    let b = read_json(&base.join("new/estimates.json"))?;
    let change = read_json(&base.join("change/estimates.json"))?;
    let number = |path: &str| -> BenchResult<f64> {
        let relative = change
            .pointer(path)
            .and_then(Value::as_f64)
            .ok_or("missing Criterion estimate")?;
        if !relative.is_finite() || relative <= -1.0 {
            return Err("invalid Criterion relative change".into());
        }
        Ok(relative + 1.0)
    };
    let point = number("/mean/point_estimate")?;
    let lower = number("/mean/confidence_interval/lower_bound")?;
    let upper = number("/mean/confidence_interval/upper_bound")?;
    let (point, lower, upper) = if first == Mode::On && second == Mode::Off {
        (1.0 / point, 1.0 / upper, 1.0 / lower)
    } else {
        (point, lower, upper)
    };
    if lower > upper {
        return Err("inverted Criterion interval".into());
    }
    Ok(json!({
        "case":case.name(), "first":first.name(), "second":second.name(),
        "ratio_definition":if first == second { "second/first (A/A)" } else { "on/off (A/B)" },
        "mean_ratio":{"point_estimate":point,"confidence_interval_95":[lower,upper]},
        "estimate_scope":"within this Criterion pair, not across hosts",
        "first_estimates":a,"second_estimates":b,"change_estimates":change,
        "first_counters":read_json(&pair.join(format!("first-{}-counters.json", case.name())))?,
        "second_counters":read_json(&pair.join(format!("second-{}-counters.json", case.name())))?,
    }))
}

fn nearest_rank(sorted: &[u64], percentile: usize) -> u64 {
    assert!(!sorted.is_empty() && (1..=100).contains(&percentile));
    sorted[(sorted.len() * percentile).div_ceil(100) - 1]
}

async fn concurrent_profile(
    fixture: &Fixture,
    mode: Mode,
    case: Case,
    workers: usize,
    requests: usize,
) -> BenchResult<Value> {
    if workers == 0 || requests < workers {
        return Err("invalid fixed-concurrency workload".into());
    }
    // Ensure the hot path is primed outside the timed request window.
    fixture.read(case).await?;
    let before = fixture.snapshot().await?;
    let started = Instant::now();
    let mut tasks = tokio::task::JoinSet::new();
    for worker in 0..workers {
        let fixture = (*fixture).clone(); // Shared reqwest connection pool, separate workers.
        let count = requests / workers + usize::from(worker < requests % workers);
        tasks.spawn(async move {
            let mut latencies = Vec::with_capacity(count);
            for _ in 0..count {
                let started = Instant::now();
                fixture.read(case).await?;
                latencies.push(started.elapsed().as_nanos() as u64);
            }
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(latencies)
        });
    }
    let mut samples = Vec::with_capacity(requests);
    while let Some(result) = tasks.join_next().await {
        samples.extend(result??);
    }
    let wall_seconds = started.elapsed().as_secs_f64();
    if samples.len() != requests || wall_seconds <= 0.0 {
        return Err("incorrect request profile sample count".into());
    }
    let after = fixture.snapshot().await?;
    let delta = after.subtract(&before)?;
    if delta.validate(mode, case)? != requests as u64 {
        return Err("HTTP decisions differ from per-request timing samples".into());
    }
    samples.sort_unstable();
    let mean_ns = samples.iter().map(|&n| n as f64).sum::<f64>() / requests as f64;
    Ok(json!({
        "mode":mode.name(), "case":case.name(),
        "concurrency":workers, "requests":requests,
        "load_type":"closed-loop; fixed completed requests, not fixed arrival rate",
        "client":"one shared pooled HTTP/1.1 reqwest Client; response status and body validated",
        "timing":"each complete HTTP request including JSON decoding and response validation",
        "warmup":"one validated request excluded from timed samples and counter deltas",
        "elapsed_seconds":wall_seconds, "throughput_rps":requests as f64 / wall_seconds,
        "latency_ns":{
            "mean":mean_ns,
            "p50":nearest_rank(&samples,50),
            "p95":nearest_rank(&samples,95),
            "p99":nearest_rank(&samples,99),
            "min":samples[0], "max":samples[requests - 1]
        },
        "cache_hits":delta.event("hit_deny"),
        "cache_misses":delta.event("miss"),
        "uncached_checks":delta.event("uncached"),
        "hit_ratio":delta.event("hit_deny") as f64 / requests as f64,
        "counters":delta,
        "raw_samples_ns_sorted":samples
    }))
}

async fn capture_concurrency_profiles(
    fixtures: &[Fixture; 2],
    output: &Path,
    study_dataset: &Value,
    report: &mut Value,
) -> BenchResult {
    report["concurrency_profiles"] = json!([]);
    // The second block reverses both the mode and case execution order.
    // These are closed-loop client-concurrency profiles, not Criterion CIs.
    for (workers, requests) in [(1, 128), (8, 512), (32, 1024)] {
        for block in 0..2 {
            let modes = if block == 0 {
                [Mode::Off, Mode::On]
            } else {
                [Mode::On, Mode::Off]
            };
            let cases = if block == 0 {
                [Case::AllowFresh, Case::DenyHot]
            } else {
                [Case::DenyHot, Case::AllowFresh]
            };
            for case in cases {
                for mode in modes {
                    if dataset_fingerprint()? != *study_dataset {
                        return Err("authorization state changed during concurrency profile".into());
                    }
                    let fixture = &fixtures[usize::from(mode == Mode::On)];
                    let mut sample =
                        concurrent_profile(fixture, mode, case, workers, requests).await?;
                    sample["block"] = json!(block);
                    report["concurrency_profiles"]
                        .as_array_mut()
                        .ok_or("concurrency profile array missing")?
                        .push(sample);
                    write_json(&output.join("experiment.json"), report)?;
                }
            }
        }
    }
    if dataset_fingerprint()? != *study_dataset {
        return Err("authorization state changed after concurrency profiles".into());
    }
    Ok(())
}

async fn check_fresh_allows(fixtures: &[Fixture; 2]) -> BenchResult<Value> {
    let mut results = Vec::new();
    for (mode, fixture) in [Mode::Off, Mode::On].into_iter().zip(fixtures) {
        let before = fixture.snapshot().await?;
        fixture.read(Case::AllowFresh).await?;
        fixture.read(Case::AllowFresh).await?;
        let after = fixture.snapshot().await?;
        let delta = after.subtract(&before)?;
        if delta.validate(mode, Case::AllowFresh)? != 2 {
            return Err("a successful read did not perform a new Check".into());
        }
        results.push(json!({"mode":mode.name(),"decisions":2,"delta":delta}));
    }
    Ok(json!(results))
}

async fn revocation_and_cold_misses(fixtures: &[Fixture; 2], rounds: usize) -> BenchResult<Value> {
    let off = &fixtures[0];
    let on = &fixtures[1];
    let mut measurements = Vec::new();
    for round in 0..rounds {
        on.change_grant(true).await?;
        let mut grant_live = true;
        let attempt = async {
            // A fresh allow must not be satisfiable from a positive cache on either replica.
            for fixture in fixtures {
                let (status, body) = fixture
                    .request(Method::GET, TABLE, &fixture.dave, None)
                    .await?;
                if status != 200 || body["id"] != "requests" {
                    return Err("new grant not visible to both replicas".into());
                }
            }
            let started = Instant::now();
            on.change_grant(false).await?;
            grant_live = false;
            let revoke_ns = started.elapsed().as_nanos();
            let mut requests = Vec::new();
            for (mode, fixture) in [(Mode::Off, off), (Mode::On, on)] {
                let before = fixture.snapshot().await?;
                let started = Instant::now();
                fixture.read(Case::DenyHot).await?;
                let cold_ns = started.elapsed().as_nanos();
                fixture.read(Case::DenyHot).await?;
                let delta = fixture.snapshot().await?.subtract(&before)?;
                let (expected_miss, expected_hit, expected_uncached) = if mode == Mode::On {
                    (1, 1, 0)
                } else {
                    (0, 0, 2)
                };
                if (
                    delta.event("miss"),
                    delta.event("hit_deny"),
                    delta.event("uncached"),
                ) != (expected_miss, expected_hit, expected_uncached)
                    || delta.http(403) != 2
                    || delta.event("hit_allow") != 0
                    || delta.event("blocked_pending") != 0
                {
                    return Err(
                        format!("cold miss/hit/revoke gate failed: {mode:?}: {delta:?}").into(),
                    );
                }
                requests.push(json!({"mode":mode.name(),"first_denial_latency_ns":cold_ns,
                    "first_denial":"miss if cache on; uncached Check if cache off",
                    "second_denial":"hit_deny if cache on; uncached Check if cache off",
                    "delta":delta}));
            }
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(
                json!({"round":round,"revoke_ns":revoke_ns,"observations":requests}),
            )
        }
        .await;
        // Attempt clean-up only if the revoke was not acknowledged; surface uncertainty.
        if grant_live && let Err(cleanup) = on.change_grant(false).await {
            return Err(format!(
                "experimental grant cleanup failed: {cleanup}; investigate the durable fence"
            )
            .into());
        }
        measurements.push(attempt?);
    }
    Ok(json!(measurements))
}

async fn fail_closed(fixtures: &[Fixture; 2]) -> BenchResult<Value> {
    for f in fixtures {
        f.read(Case::AllowFresh).await?;
    }
    compose(&["stop", "-t", "1", "openfga"])?;
    let observations = async {
        let mut rows = Vec::new();
        for (mode, fixture) in [Mode::Off, Mode::On].into_iter().zip(fixtures) {
            let (allow, _) = fixture
                .request(Method::GET, TABLE, &fixture.alice, None)
                .await?;
            if allow != 503 {
                return Err(
                    "previously allowed access did not fail closed during OpenFGA outage".into(),
                );
            }
            let (deny, _) = fixture
                .request(Method::GET, TABLE, &fixture.dave, None)
                .await?;
            // Cached denial may return 403 without a live Check. Neither arm may grant access.
            if deny != 403 && deny != 503 {
                return Err("access granted during OpenFGA outage".into());
            }
            rows.push(
                json!({"mode":mode.name(),"allowed_user_status":allow,"denied_user_status":deny}),
            );
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(json!(rows))
    }
    .await;
    let restarted = compose(&["start", "openfga"]);
    let result = observations?;
    restarted?;
    for fixture in fixtures {
        fixture.wait_ready().await?;
        fixture.read(Case::AllowFresh).await?;
        fixture.read(Case::DenyHot).await?;
    }
    Ok(result)
}

fn host_environment() -> BenchResult<Value> {
    let rustc = Command::new("rustc").arg("--version").output()?;
    if !rustc.status.success() {
        return Err("rustc --version failed".into());
    }
    Ok(json!({
        "os":std::env::consts::OS,"arch":std::env::consts::ARCH,
        "cpus":std::thread::available_parallelism()?.get(),
        "rustc":String::from_utf8(rustc.stdout)?.trim(),
        "runner":std::env::var("RUNNER_NAME").unwrap_or_else(|_| "local".into()),
        "clock":"std::time::Instant; hardware resolution not established"
    }))
}

async fn run(output: &Path, bin: &Path, seed: u64, report: &mut Value) -> BenchResult {
    let fixtures = [
        Fixture::connect(Mode::Off).await?,
        Fixture::connect(Mode::On).await?,
    ];
    report["fresh_allows"] = check_fresh_allows(&fixtures).await?;
    report["revocations_and_cold_misses_before"] = revocation_and_cold_misses(&fixtures, 8).await?;
    report["backend_outage_before"] = fail_closed(&fixtures).await?;
    write_json(&output.join("experiment.json"), report)?;

    let study_dataset = dataset_fingerprint()?;
    report["study_dataset_fingerprint"] = study_dataset.clone();
    let stages = [
        ("AA-off", Mode::Off, Mode::Off),
        ("AA-on", Mode::On, Mode::On),
        ("AB", Mode::Off, Mode::On),
    ];
    for stage in permutation(stages.len(), seed, "stages") {
        let (name, a, b) = stages[stage];
        for (block, index) in permutation(4, seed, name).into_iter().enumerate() {
            let (first, second) = if index < 2 { (a, b) } else { (b, a) };
            let pair = output.join(format!("{name}-{block}"));
            std::fs::create_dir(&pair)?;
            let before = dataset_fingerprint()?;
            if before != study_dataset {
                return Err(
                    "catalog data or authorization generation changed between pairs".into(),
                );
            }
            bench_arm(
                bin,
                &pair,
                "first",
                first,
                &fixtures[usize::from(first == Mode::On)],
            )?;
            let between = dataset_fingerprint()?;
            bench_arm(
                bin,
                &pair,
                "second",
                second,
                &fixtures[usize::from(second == Mode::On)],
            )?;
            let after = dataset_fingerprint()?;
            if before != between || before != after {
                return Err(format!(
                    "catalog dataset or authorization generation changed within {name}/{block}"
                )
                .into());
            }
            let comparisons: Vec<_> = CASES
                .iter()
                .copied()
                .map(|case| comparison(&pair, case, first, second))
                .collect::<BenchResult<_>>()?;
            report["pairs"]
                .as_array_mut()
                .ok_or("missing report pairs")?
                .push(json!({
                    "stage":name,"block":block,"first":first.name(),"second":second.name(),
                    "order":if index < 2 { "AB" } else { "BA" },
                    "dataset_fingerprint":before,
                    "comparisons":comparisons,
                }));
            write_json(&output.join("experiment.json"), report)?;
        }
    }
    if std::env::var("AUTHZ_BENCH_LOAD_PROFILES").as_deref() == Ok("true") {
        capture_concurrency_profiles(&fixtures, output, &study_dataset, report).await?;
    }
    report["revocations_and_cold_misses_after"] = revocation_and_cold_misses(&fixtures, 8).await?;
    report["backend_outage_after"] = fail_closed(&fixtures).await?;
    report["fresh_allows_after"] = check_fresh_allows(&fixtures).await?;
    if report["server_binary"] != assert_identical_server_binaries()?
        || report["bench_sha256"].as_str() != Some(sha256(bin)?.as_str())
    {
        return Err("application or benchmark binary changed during the experiment".into());
    }
    Ok(())
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> BenchResult {
    let root = PathBuf::from(
        std::env::var("AUTHZ_EXPERIMENT_OUT")
            .unwrap_or_else(|_| "artifacts/criterion-lakekeeper".into()),
    );
    if root.exists() {
        return Err("refuse to reuse an existing experiment output directory".into());
    }
    let bin = PathBuf::from(
        std::env::var("AUTHZ_BENCH_BINARY")
            .map_err(|_| "set AUTHZ_BENCH_BINARY to the precompiled catalog_cache_ab executable")?,
    );
    let bin = bin.canonicalize()?;
    let load_profiles = match std::env::var("AUTHZ_BENCH_LOAD_PROFILES").as_deref() {
        Ok("true") => true,
        Ok("false") | Err(std::env::VarError::NotPresent) => false,
        _ => return Err("AUTHZ_BENCH_LOAD_PROFILES must be true or false".into()),
    };
    let swap_ports = Mode::swapped_ports()?;
    let seed: u64 = std::env::var("AB_SEED")
        .unwrap_or_else(|_| "2606".into())
        .parse()?;
    // Before making an artifact, ensure we are operating on disposable services and *identical* binaries.
    let binary_evidence = assert_identical_server_binaries()?;
    std::fs::create_dir_all(&root)?;
    let sha = Command::new("git").args(["rev-parse", "HEAD"]).output()?;
    if !sha.status.success() {
        return Err("cannot identify benchmark revision".into());
    }
    let mut report = json!({
        "valid":false,"model":"Lakekeeper primary; Openlane excluded",
        "commit":String::from_utf8(sha.stdout)?.trim(),"seed":seed,
        "swap_ports":swap_ports,
        "load_profiles_enabled":load_profiles,
        "bench_sha256":sha256(&bin)?,"server_binary":binary_evidence,
        "cargo_lock_sha256":sha256(Path::new("Cargo.lock"))?,
        "fga_model_sha256":sha256(Path::new("fga/model.json"))?,
        "criterion_source_sha256":sha256(Path::new("benches/catalog_cache_ab.rs"))?,
        "support_source_sha256":sha256(Path::new("benches/lakekeeper_support/mod.rs"))?,
        "environment":host_environment()?,
        "comparisons":["AA-off","AA-on","AB"],"balanced_blocks_per_stage":4,
        "cases":CASES.iter().map(|c| c.name()).collect::<Vec<_>>(),
        "pairs":[],"limitations":[
            "No prior positive cache; allowed reads must always Check live backend",
            "Criterion group measures complete catalog HTTP GET including SQL and OpenFGA",
            "Deny-miss timing is a separate single-request diagnostic after mutation; not Criterion inference",
            "Warmup and measurement both contribute to cache counter deltas",
            "Within-pair Criterion CIs are not between-host confidence intervals",
            "One hot key; fixed client concurrency 1; generator and services may share host",
            "No automated performance verdict; predeclare practical effect threshold before production claim",
            "Closed-loop load profiles report per-request p50/p95/p99, never Criterion batch percentiles",
            "Fixed port-to-mode assignment remains a confound within a run; compare swapped deployments to cross it over"
        ]
    });
    write_json(&root.join("experiment.json"), &report)?;
    match run(&root, &bin, seed, &mut report).await {
        Ok(()) => report["valid"] = json!(true),
        Err(error) => {
            report["failure"] = json!(error.to_string());
            write_json(&root.join("experiment.json"), &report)?;
            return Err(error);
        }
    }
    write_json(&root.join("experiment.json"), &report)?;
    println!("Lakekeeper Criterion experiment: {}", root.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nearest_rank_latency_percentiles() {
        let sorted = [10, 20, 30, 40, 50, 60, 70, 80, 90, 100];
        assert_eq!(nearest_rank(&sorted, 50), 50);
        assert_eq!(nearest_rank(&sorted, 95), 100);
        assert_eq!(nearest_rank(&sorted, 99), 100);
        assert_eq!(nearest_rank(&[42], 99), 42);
    }
    #[test]
    fn deterministic_balanced_order() {
        let one = permutation(4, 2606, "AB");
        assert_eq!(one, permutation(4, 2606, "AB"));
        assert_eq!(one.iter().filter(|&&i| i < 2).count(), 2);
        let mut sorted = one;
        sorted.sort_unstable();
        assert_eq!(sorted, vec![0, 1, 2, 3]);
    }
    #[test]
    fn direct_criterion_invocation_requires_measured_mode() {
        assert_eq!(
            benchmark_cli_args("first").unwrap(),
            ["--bench", "--save-baseline", "reference", "--noplot"]
        );
        assert_eq!(
            benchmark_cli_args("second").unwrap(),
            ["--bench", "--baseline", "reference", "--noplot"]
        );
        assert!(benchmark_cli_args("unexpected").is_err());
    }
    #[test]
    fn ratio_inversion_preserves_interval_order() {
        let (lower, upper) = (0.9_f64, 1.2_f64);
        assert!(1.0 / upper < 1.0 / lower);
    }
}
