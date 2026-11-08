use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use std::{net::SocketAddr, time::Duration};
use tokio::{
    process::Command,
    time::{sleep, timeout},
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct Deployment {
    project: String,
    password: String,
}

impl Deployment {
    fn command(&self) -> std::process::Command {
        let mut command = std::process::Command::new("docker");
        command
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("APP_PORT", "0")
            .env("DEMO_PASSWORD", &self.password)
            .env("POSTGRES_PASSWORD", &self.password)
            .args(["compose", "-p", &self.project, "-f", "compose.yaml"]);
        command
    }
}

impl Drop for Deployment {
    fn drop(&mut self) {
        if !self
            .command()
            .args(["down", "-v", "-t", "2"])
            .output()
            .is_ok_and(|output| output.status.success())
        {
            eprintln!("Could not clean up Compose deployment {}", self.project);
        }
    }
}

#[tokio::test]
#[ignore = "requires Docker and builds the production images"]
async fn compose_runtime_bootstraps_and_serves_real_api() -> TestResult {
    let deployment = Deployment {
        project: format!("petlab-deployment-{}", uuid::Uuid::new_v4()),
        password: uuid::Uuid::new_v4().to_string(),
    };
    let output = timeout(
        Duration::from_secs(900),
        Command::from(deployment.command())
            .args(["up", "-d", "--build", "app"])
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    if !output.status.success() {
        return Err(format!(
            "Compose build/start failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let output = Command::from(deployment.command())
        .args(["port", "app", "3000"])
        .output()
        .await?;
    if !output.status.success() {
        return Err("could not resolve Compose API port".into());
    }
    let address: SocketAddr = String::from_utf8(output.stdout)?.trim().parse()?;
    assert!(address.ip().is_loopback());
    let base = format!("http://{address}");
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()?;
    timeout(Duration::from_secs(60), async {
        loop {
            if matches!(client.get(format!("{base}/healthz")).send().await,
                Ok(response) if response.status() == StatusCode::OK)
            {
                break;
            }
            sleep(Duration::from_millis(250)).await;
        }
    })
    .await?;
    let session: Value = client
        .post(format!("{base}/session"))
        .json(&json!({"username": "alice", "password": deployment.password}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let token = session["token"].as_str().ok_or("missing session token")?;
    let pets: Vec<Value> = client
        .get(format!("{base}/pets"))
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(pets.iter().any(|pet| pet["id"] == "fox"));
    Ok(())
}
