use std::{path::Path, process::Command};

#[test]
fn repository_test_sources_and_workflows_do_not_require_python() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let files = Command::new("git")
        .current_dir(root)
        .args(["ls-files", "-z"])
        .output()
        .expect("list tracked repository files");
    assert!(files.status.success());
    let files = String::from_utf8(files.stdout).expect("UTF-8 repository paths");
    for path in files.split('\0').filter(|path| !path.is_empty()) {
        assert!(
            !path.ends_with(".py") && !path.ends_with(".pyw"),
            "unexpected Python source: {path}"
        );
        if path.starts_with(".github/workflows/") || path == "compose.yaml" || path == "Dockerfile"
        {
            let text = std::fs::read_to_string(root.join(path)).expect("workflow source");
            assert!(
                !text.to_ascii_lowercase().contains("python"),
                "Python runtime reference: {path}"
            );
        }
    }
}

#[test]
fn manual_ci_keeps_all_real_stack_suites_enabled() {
    let ci = include_str!("../.github/workflows/ci.yml");
    assert!(ci.contains("workflow_dispatch"));
    assert!(ci.contains("cargo nextest run --locked --release --profile ci"));
    assert!(ci.contains("--run-ignored only"));
    for suite in [
        "system",
        "cache_replicas",
        "compose_deployment",
        "performance",
    ] {
        assert!(
            ci.contains(&format!("--test {suite}")),
            "missing CI suite: {suite}"
        );
    }
    assert!(ci.contains("needs: [rust, integration, shell, formal, browser-demo, browser-e2e]"));
    assert!(ci.contains("BROWSER_E2E_RESULT: ${{ needs.browser-e2e.result }}"));
    assert!(ci.contains("test \"$BROWSER_E2E_RESULT\" = success"));
    for browser_step in [
        "transport: grpc",
        "port: 8081",
        "transport: http",
        "port: 8080",
        "FGA_APP_URL: http://openfga:${{ matrix.port }}",
        "docker compose up -d --build app",
        "npm ci",
        "run: npm test",
    ] {
        assert!(
            ci.contains(browser_step),
            "missing real browser gate: {browser_step}"
        );
    }
    assert!(!ci.contains("continue-on-error:"));
    let performance = include_str!("../.github/workflows/performance.yml");
    assert!(performance.contains("on: workflow_dispatch"));
    assert!(performance.contains("test(paired_aa_calibration)"));
    assert!(!performance.contains("schedule:"));
}
