//! Exercises the image-only distribution, not a separately compiled server.
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use std::{net::SocketAddr, path::PathBuf, process::Stdio, time::Duration};
use tempfile::TempDir;
use tokio::{
    io::AsyncWriteExt,
    process::Command,
    time::{sleep, timeout},
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
struct Stand {
    project: String,
    directory: TempDir,
    environment: PathBuf,
    overlay: PathBuf,
}

impl Stand {
    async fn new(image: &str) -> TestResult<Self> {
        let directory = tempfile::tempdir()?;
        let state = directory.path().join("state");
        let output = Command::new(env!("CARGO_BIN_EXE_lab-tool"))
            .args(["init-env", state.to_str().ok_or("path")?, image])
            .output()
            .await?;
        assert!(
            output.status.success(),
            "init-env failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let overlay = directory.path().join("test.yaml");
        std::fs::write(
            &overlay,
            "services:\n  openfga:\n    ports: [\"127.0.0.1::8080\"]\n",
        )?;
        Ok(Self {
            project: format!("petlab-delivery-{}", uuid::Uuid::new_v4()),
            environment: state.join("environment.env"),
            directory,
            overlay,
        })
    }
    fn command(&self) -> Command {
        let mut c = Command::new("docker");
        c.current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("APP_PORT", "0")
            .args(["compose", "-p", &self.project, "--env-file"])
            .arg(&self.environment)
            .args(["-f", "compose.release.yaml", "-f"])
            .arg(&self.overlay)
            .kill_on_drop(true);
        c
    }
    async fn run(&self, args: &[&str]) -> TestResult<String> {
        let out = timeout(Duration::from_secs(180), self.command().args(args).output())
            .await
            .map_err(|_| format!("compose {} exceeded 180 seconds", args.join(" ")))??;
        if !out.status.success() {
            return Err(format!(
                "compose {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr)
            )
            .into());
        }
        Ok(String::from_utf8(out.stdout)?)
    }
    async fn address(&self, service: &str, port: &str) -> TestResult<String> {
        let address: SocketAddr = self.run(&["port", service, port]).await?.trim().parse()?;
        assert!(address.ip().is_loopback());
        Ok(format!("http://{address}"))
    }
    async fn start(&self) -> TestResult {
        self.run(&["up", "-d", "--wait", "--wait-timeout", "120", "app"])
            .await?;
        Ok(())
    }
    async fn acceptance(&self) -> TestResult<Value> {
        Ok(serde_json::from_str(
            self.run(&["run", "--rm", "--no-deps", "acceptance"])
                .await?
                .trim(),
        )?)
    }
    async fn fga_config(&self) -> TestResult<Value> {
        Ok(serde_json::from_str(
            &self
                .run(&["exec", "-T", "app", "cat", "/data/config.json"])
                .await?,
        )?)
    }
    fn password(&self) -> TestResult<String> {
        Ok(std::fs::read_to_string(
            self.directory.path().join("state/secrets/demo-password"),
        )?)
    }
    async fn restore(&self, database: &str, dump: &str) -> TestResult {
        let mut child = self
            .command()
            .args([
                "exec",
                "-T",
                "postgres",
                "psql",
                "-U",
                "petlab",
                "-d",
                database,
                "-v",
                "ON_ERROR_STOP=1",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut input = child.stdin.take().ok_or("missing stdin")?;
        input.write_all(dump.as_bytes()).await?;
        drop(input);
        let output = timeout(Duration::from_secs(60), child.wait_with_output())
            .await
            .map_err(|_| format!("restore of {database} timed out"))??;
        if !output.status.success() {
            return Err("restore failed; disposable database only".into());
        }
        Ok(())
    }
}
impl Drop for Stand {
    fn drop(&mut self) {
        // Only the unique test-owned project can be removed.
        let mut c = std::process::Command::new("docker");
        let result = c
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("APP_PORT", "0")
            .args(["compose", "-p", &self.project, "--env-file"])
            .arg(&self.environment)
            .args(["-f", "compose.release.yaml", "-f"])
            .arg(&self.overlay)
            .args(["down", "-v", "-t", "2"])
            .output();
        if !result.is_ok_and(|r| r.status.success()) {
            eprintln!("cleanup needed for {}", self.project);
        }
    }
}
async fn status(client: &Client, base: &str, path: &str, expected: StatusCode) -> TestResult {
    assert_eq!(
        client.get(format!("{base}{path}")).send().await?.status(),
        expected,
        "{path}"
    );
    Ok(())
}
async fn wait_ready(client: &Client, base: &str) -> TestResult {
    timeout(Duration::from_secs(60), async {
        loop {
            if client
                .get(format!("{base}/readyz"))
                .send()
                .await
                .is_ok_and(|r| r.status() == StatusCode::OK)
            {
                break;
            }
            sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .map_err(|_| format!("readiness timed out at {base}; verify current container port"))?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker; image-only two-environment acceptance and restore test"]
async fn immutable_delivery_isolated_stands_and_recovery() -> TestResult {
    let image = std::env::var("DELIVERY_IMAGE").map_err(|_| "build/load a candidate and set DELIVERY_IMAGE to its immutable image ID or registry digest")?;
    let stage = Stand::new(&image).await?;
    let stable = Stand::new(&image).await?;
    assert_ne!(stage.password()?, stable.password()?);
    println!("delivery: starting isolated stage and stable");
    stage.start().await?;
    stable.start().await?;
    let client = Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(6))
        .build()?;
    let stage_url = stage.address("app", "3000").await?;
    let stable_url = stable.address("app", "3000").await?;
    let stage_config = stage.fga_config().await?;
    let stable_config = stable.fga_config().await?;
    assert_ne!(stage_config["store_id"], stable_config["store_id"]);
    let stage_topology: Value =
        serde_json::from_str(&stage.run(&["config", "--format", "json"]).await?)?;
    let stable_topology: Value =
        serde_json::from_str(&stable.run(&["config", "--format", "json"]).await?)?;
    for volume in ["database", "fga-state"] {
        assert_ne!(
            stage_topology["volumes"][volume]["name"],
            stable_topology["volumes"][volume]["name"]
        );
    }
    assert_ne!(
        stage_topology["networks"]["lab"]["name"],
        stable_topology["networks"]["lab"]["name"]
    );
    println!("delivery: verifying real authorization and session isolation");
    let stage_acceptance = stage.acceptance().await?;
    let stable_acceptance = stable.acceptance().await?;
    assert_eq!(stage_acceptance["version"], stable_acceptance["version"]);
    if let Ok(sha) = std::env::var("GITHUB_SHA") {
        assert_eq!(stage_acceptance["version"]["revision"], sha);
    }
    let session: Value = client
        .post(format!("{stage_url}/session"))
        .json(&json!({"username":"alice","password":stage.password()?}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let token = session["token"].as_str().ok_or("token")?;
    assert_eq!(
        client
            .get(format!("{stable_url}/pets/fox"))
            .bearer_auth(token)
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let original: Value = client
        .get(format!("{stage_url}/pets/fox"))
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    println!("delivery: restarting app and resolving its ephemeral port again");
    stage.run(&["restart", "app"]).await?;
    let stage_url = stage.address("app", "3000").await?;
    wait_ready(&client, &stage_url).await?;
    assert_eq!(stage.fga_config().await?, stage_config);
    status(&client, &stage_url, "/healthz", StatusCode::OK).await?;
    println!("delivery: quiesced backup and restore of both databases");
    // Quiesce both systems before backing up. Dumps and session hashes stay in memory.
    stage.run(&["stop", "-t", "2", "app", "openfga"]).await?;
    let mut dumps = Vec::new();
    for database in ["petlab", "openfga"] {
        dumps.push((
            database,
            stage
                .run(&[
                    "exec",
                    "-T",
                    "postgres",
                    "pg_dump",
                    "-U",
                    "petlab",
                    "-d",
                    database,
                    "--clean",
                    "--if-exists",
                    "--no-owner",
                    "--no-privileges",
                ])
                .await?,
        ));
    }
    stage
        .run(&[
            "exec",
            "-T",
            "postgres",
            "psql",
            "-U",
            "petlab",
            "-d",
            "petlab",
            "-v",
            "ON_ERROR_STOP=1",
            "-c",
            "UPDATE pets SET biography='restore sentinel' WHERE id='fox'",
        ])
        .await?;
    for (database, dump) in dumps {
        stage.restore(database, &dump).await?;
    }
    stage.start().await?;
    let stage_url = stage.address("app", "3000").await?;
    let restored: Value = client
        .get(format!("{stage_url}/pets/fox"))
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(original, restored);
    assert_eq!(stage.fga_config().await?, stage_config);
    stage.acceptance().await?;
    println!("delivery: dependency outage must not affect the other stand");
    stage.run(&["stop", "-t", "1", "openfga"]).await?;
    status(
        &client,
        &stage_url,
        "/readyz",
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await?;
    status(&client, &stage_url, "/healthz", StatusCode::OK).await?;
    status(&client, &stable_url, "/readyz", StatusCode::OK).await?;
    assert_eq!(
        client
            .get(format!("{stage_url}/pets/fox"))
            .bearer_auth(token)
            .send()
            .await?
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    stage.run(&["start", "openfga"]).await?;
    wait_ready(&client, &stage_url).await?;
    println!("delivery: missing store must fail readiness while liveness remains healthy");
    let fga = stage.address("openfga", "8080").await?;
    client
        .delete(format!(
            "{fga}/stores/{}",
            stage_config["store_id"].as_str().ok_or("store")?
        ))
        .send()
        .await?
        .error_for_status()?;
    status(
        &client,
        &stage_url,
        "/readyz",
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await?;
    status(&client, &stage_url, "/healthz", StatusCode::OK).await?;
    status(&client, &stable_url, "/readyz", StatusCode::OK).await?;
    stable.acceptance().await?;
    std::fs::create_dir_all("artifacts")?;
    std::fs::write(
        "artifacts/delivery-evidence.json",
        serde_json::to_vec_pretty(&json!({
        "valid":true,"image":image,"stage":stage_acceptance,"stable":stable_acceptance,
        "isolated_secrets_stores_volumes_networks":true,"sessions_survive_restore":true,
        "quiesced_two_database_restore":true,"readiness_outage_and_deleted_store":true,
        "scope":"two disposable CI stands, not persistent remote deployment"}))?,
    )?;
    Ok(())
}
