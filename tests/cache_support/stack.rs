use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use std::{error::Error, io::Write, net::SocketAddr, process::Stdio, time::Duration};
use tempfile::NamedTempFile;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
    time::{sleep, timeout},
};
use uuid::Uuid;

pub type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

pub struct Stack {
    _compose: Compose,
    config: NamedTempFile,
    database: String,
    password: String,
    pub fga_url: String,
}

struct Compose {
    project: String,
    password: String,
}

impl Compose {
    fn command(&self) -> std::process::Command {
        let mut command = std::process::Command::new("docker");
        command
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("CACHE_TEST_PASSWORD", &self.password)
            .args([
                "compose",
                "-p",
                &self.project,
                "-f",
                "compose.yaml",
                "-f",
                "compose.cache-test.yaml",
            ]);
        command
    }

    async fn run(&self, args: &[&str]) -> TestResult<String> {
        let output = Command::from(self.command())
            .args(args)
            .kill_on_drop(true)
            .output()
            .await?;
        if !output.status.success() {
            return Err(format!(
                "docker compose {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        Ok(String::from_utf8(output.stdout)?)
    }

    async fn address(&self, service: &str, port: &str) -> TestResult<SocketAddr> {
        let address: SocketAddr = self.run(&["port", service, port]).await?.trim().parse()?;
        if !address.ip().is_loopback() {
            return Err("cache test service is not bound to loopback".into());
        }
        Ok(address)
    }
}

impl Drop for Compose {
    fn drop(&mut self) {
        let cleanup = self.command().args(["down", "-v", "-t", "2"]).output();
        if !cleanup.is_ok_and(|output| output.status.success()) {
            eprintln!(
                "Could not remove disposable Compose project {}",
                self.project
            );
        }
    }
}

impl Stack {
    pub async fn start(client: &Client) -> TestResult<Self> {
        let compose = Compose {
            project: format!("petlab-cache-{}", Uuid::new_v4()),
            password: Uuid::new_v4().to_string(),
        };
        timeout(
            Duration::from_secs(120),
            compose.run(&["up", "-d", "openfga"]),
        )
        .await??;
        let postgres = compose.address("postgres", "5432").await?;
        let database = format!(
            "postgres://petlab:{}@{postgres}/petlab?sslmode=disable",
            compose.password
        );
        let fga_url = format!("http://{}", compose.address("openfga", "8080").await?);
        timeout(Duration::from_secs(30), async {
            while !matches!(
                client.get(format!("{fga_url}/healthz")).send().await,
                Ok(response) if response.status() == StatusCode::OK
            ) {
                sleep(Duration::from_millis(100)).await;
            }
        })
        .await?;
        let config = fga_config(client, &fga_url).await?;
        Ok(Self {
            _compose: compose,
            config,
            database,
            password: Uuid::new_v4().to_string(),
            fga_url,
        })
    }

    pub async fn app(&self, fga_url: &str) -> TestResult<App> {
        let mut child = Command::new(env!("CARGO_BIN_EXE_openfga-axum-lab"))
            .env("DATABASE_URL", &self.database)
            .env(
                "MIGRATIONS_DIR",
                concat!(env!("CARGO_MANIFEST_DIR"), "/migrations"),
            )
            .env("FGA_URL", fga_url)
            .env("FGA_CONFIG", self.config.path())
            .env("AUTHZ_MODE", "openfga")
            .env("DEMO_SEED", "true")
            .env("DEMO_PASSWORD", &self.password)
            .env("LISTEN_ADDR", "127.0.0.1:0")
            .env("RUST_LOG", "off")
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout")).lines();
        let address = timeout(Duration::from_secs(30), async {
            while let Some(line) = stdout.next_line().await? {
                if let Some(address) = line.strip_prefix("Listening on ") {
                    return Ok(address.parse::<SocketAddr>()?);
                }
            }
            Err::<_, Box<dyn Error + Send + Sync>>("Axum exited before listening".into())
        })
        .await??;
        tokio::spawn(async move { while matches!(stdout.next_line().await, Ok(Some(_))) {} });
        Ok(App {
            url: format!("http://{address}"),
            child,
        })
    }

    pub async fn login(&self, client: &Client, app: &App, user: &str) -> TestResult<String> {
        let response: Value = client
            .post(format!("{}/session", app.url))
            .json(&json!({"username": user, "password": self.password}))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(response["token"]
            .as_str()
            .ok_or("missing session token")?
            .to_owned())
    }
}

pub struct App {
    pub url: String,
    child: Child,
}

impl App {
    pub async fn stop(mut self) -> TestResult {
        self.child.kill().await?;
        Ok(())
    }
}

async fn fga_config(client: &Client, url: &str) -> TestResult<NamedTempFile> {
    let store: Value = client
        .post(format!("{url}/stores"))
        .json(&json!({"name": "cache-test"}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let store = store["id"].as_str().ok_or("missing store id")?;
    let model: Value = client
        .post(format!("{url}/stores/{store}/authorization-models"))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(include_str!("../../fga/model.json"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let model = model["authorization_model_id"]
        .as_str()
        .ok_or("missing model id")?;
    let tuples: Value = serde_json::from_str(include_str!("../../fga/legacy.json"))?;
    client
        .post(format!("{url}/stores/{store}/write"))
        .json(&json!({"authorization_model_id": model, "writes": {"tuple_keys": tuples}}))
        .send()
        .await?
        .error_for_status()?;
    let mut config = NamedTempFile::new()?;
    serde_json::to_writer(&mut config, &json!({"store_id": store, "model_id": model}))?;
    config.flush()?;
    Ok(config)
}
