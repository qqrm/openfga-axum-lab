//! Offline negative controls for public-entry-point policy and lifecycle configuration.
use std::process::{Command, Output};

fn script(args: &[&str], email: &str, password: &str, minutes: &str) -> Output {
    Command::new("bash")
        .arg("scripts/live-lab.sh")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("LIVE_LAB_ALLOWED_EMAIL", email)
        .env("LIVE_LAB_PASSWORD", password)
        .env("LIVE_LAB_MINUTES", minutes)
        .output()
        .expect("run infrastructure policy without Docker or network")
}

#[test]
fn invitation_and_duration_fail_closed_without_disclosing_values() {
    let password = uuid::Uuid::new_v4().to_string();
    let emails = [
        "",
        "*@example.org",
        "a@example.org,b@example.org",
        "a@example.org\n",
        "--help",
    ];
    for email in emails {
        let result = script(&["validate"], email, &password, "30");
        assert!(!result.status.success());
        assert!(!String::from_utf8_lossy(&result.stderr).contains(&password));
    }
    for duration in ["0", "360", "999999", "30;exit", "-1"] {
        let result = script(&["validate"], "tester@example.invalid", &password, duration);
        assert!(!result.status.success());
    }
    let empty = script(&["validate"], "tester@example.invalid", "", "30");
    let valid = script(&["validate"], "tester@example.invalid", &password, "30");
    assert!(!empty.status.success());
    assert!(valid.status.success());
}

#[test]
fn only_the_actual_email_login_redirect_counts_as_protection() {
    for (status, location) in [
        ("200", "https://login.trycloudflare.com/"),
        ("403", "https://login.trycloudflare.com/"),
        ("502", "https://login.trycloudflare.com/"),
        ("302", "https://login.trycloudflare.com.attacker.invalid/"),
        ("302", "https://login.trycloudflare.com@attacker.invalid/"),
        ("302", "http://login.trycloudflare.com/"),
    ] {
        let result = script(&["classify-gate", status, location], "", "", "");
        assert!(!result.status.success());
    }
    let args = ["classify-gate", "302", "https://login.trycloudflare.com/"];
    assert!(script(&args, "", "", "").status.success());
}

#[test]
fn codespace_does_not_forward_database_or_openfga_ports() {
    let source = include_str!("../.devcontainer/devcontainer.json");
    let config: serde_json::Value = serde_json::from_str(source).unwrap();
    assert_eq!(config["forwardPorts"], serde_json::json!([3000]));
    assert_eq!(config["otherPortsAttributes"]["onAutoForward"], "ignore");
    assert_eq!(
        config["postAttachCommand"],
        "bash scripts/codespace-lab.sh private"
    );
    assert!(include_str!("../scripts/codespace-lab.sh").contains("3000:private"));
    let workflow = include_str!("../.github/workflows/live-lab.yml");
    assert!(workflow.contains("workflow_dispatch:"));
    assert!(!workflow.contains("schedule:"));
    assert!(!workflow.contains("pull_request:"));
    assert!(workflow.contains("github.ref == 'refs/heads/main'"));
    assert!(workflow.contains("timeout-minutes: 85"));
    assert!(workflow.contains("if: always()"));
    assert!(include_str!("../scripts/live-lab.sh").contains("--allowed-mail"));
}
