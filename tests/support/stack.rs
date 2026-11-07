use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use std::{error::Error, net::SocketAddr, path::PathBuf, process::Stdio, time::Duration};
use tempfile::TempDir;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
    time::{sleep, timeout},
};
use uuid::Uuid;

pub type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;
type CoverageProfile = Option<(PathBuf, String)>;

pub fn client() -> TestResult<Client> {
    Ok(Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()?)
}

pub struct Stack {
    compose: Compose,
    config_dir: TempDir,
    database: String,
    password: String,
    pub fga_url: String,
}

struct Compose {
    project: String,
    password: String,
}

#[derive(serde::Deserialize)]
struct TuplePage {
    #[serde(default)]
    tuples: Vec<Value>,
    #[serde(default)]
    continuation_token: String,
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
        let output = timeout(
            Duration::from_secs(120),
            Command::from(self.command())
                .args(args)
                .kill_on_drop(true)
                .output(),
        )
        .await??;
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
            return Err("test service is not bound to loopback".into());
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
            project: format!("petlab-test-{}", Uuid::new_v4()),
            password: Uuid::new_v4().to_string(),
        };
        compose.run(&["up", "-d", "openfga"]).await?;
        let postgres = compose.address("postgres", "5432").await?;
        let database = format!(
            "postgres://petlab:{}@{postgres}/petlab?sslmode=disable",
            compose.password
        );
        let fga_url = format!("http://{}", compose.address("openfga", "8080").await?);
        let stack = Self {
            compose,
            config_dir: tempfile::tempdir()?,
            database,
            password: Uuid::new_v4().to_string(),
            fga_url,
        };
        stack.wait_ready(client).await?;
        stack.bootstrap().await?;
        Ok(stack)
    }

    async fn wait_ready(&self, client: &Client) -> TestResult {
        timeout(Duration::from_secs(60), async {
            loop {
                if let Ok(response) = client.get(format!("{}/healthz", self.fga_url)).send().await
                    && response.status() == StatusCode::OK
                    && let Ok(body) = response.json::<Value>().await
                    && body["status"] == "SERVING"
                {
                    break;
                }
                sleep(Duration::from_millis(100)).await;
            }
        })
        .await?;
        Ok(())
    }

    pub fn config_path(&self) -> PathBuf {
        self.config_dir.path().join("config.json")
    }

    pub fn database_url(&self) -> &str {
        &self.database
    }

    pub fn config(&self) -> TestResult<Value> {
        Ok(serde_json::from_slice(&std::fs::read(self.config_path())?)?)
    }

    pub fn bootstrap_command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lab-tool"));
        command
            .arg("bootstrap")
            .env("FGA_URL", &self.fga_url)
            .env("FGA_CONFIG", self.config_path())
            .env(
                "FGA_MODEL",
                concat!(env!("CARGO_MANIFEST_DIR"), "/fga/model.json"),
            )
            .env(
                "FGA_TUPLES",
                concat!(env!("CARGO_MANIFEST_DIR"), "/fga/legacy.json"),
            )
            .kill_on_drop(true);
        command
    }

    pub async fn bootstrap(&self) -> TestResult {
        let output = timeout(Duration::from_secs(75), self.bootstrap_command().output()).await??;
        if !output.status.success() {
            return Err(format!(
                "Rust bootstrap failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        Ok(())
    }

    fn app_command(&self, fga_url: &str, mode: &str) -> (Command, CoverageProfile) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_openfga-axum-lab"));
        command
            .env("DATABASE_URL", &self.database)
            .env(
                "MIGRATIONS_DIR",
                concat!(env!("CARGO_MANIFEST_DIR"), "/migrations"),
            )
            .env("FGA_URL", fga_url)
            .env("FGA_CONFIG", self.config_path())
            .env("AUTHZ_MODE", mode)
            .env("DEMO_SEED", "true")
            .env("DEMO_PASSWORD", &self.password)
            .env("LISTEN_ADDR", "127.0.0.1:0")
            .env("RUST_LOG", "off")
            .stdout(Stdio::piped())
            .kill_on_drop(true);
        // Keep profiles in cargo-llvm-cov's directory, but give this child a
        // distinct prefix so a test-client profile cannot satisfy its check.
        let coverage_profile = std::env::var_os("LLVM_PROFILE_FILE").map(|pattern| {
            let prefix = format!("axum-{}", Uuid::new_v4());
            let path = PathBuf::from(pattern).with_file_name(format!("{prefix}-%p-%m.profraw"));
            command.env("LLVM_PROFILE_FILE", &path);
            (path, prefix)
        });
        (command, coverage_profile)
    }

    pub async fn app(&self, fga_url: &str) -> TestResult<App> {
        self.app_with_mode(fga_url, "openfga").await
    }

    pub async fn app_with_mode(&self, fga_url: &str, mode: &str) -> TestResult<App> {
        let (mut command, coverage_profile) = self.app_command(fga_url, mode);
        let mut child = command.spawn()?;
        let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout")).lines();
        let address = timeout(Duration::from_secs(60), async {
            while let Some(line) = stdout.next_line().await? {
                if let Some(address) = line.strip_prefix("Listening on ") {
                    return Ok(address.parse::<SocketAddr>()?);
                }
            }
            Err::<_, Box<dyn Error + Send + Sync>>("Axum exited before listening".into())
        })
        .await??;
        if !address.ip().is_loopback() {
            return Err("test API is not bound to loopback".into());
        }
        tokio::spawn(async move { while matches!(stdout.next_line().await, Ok(Some(_))) {} });
        Ok(App {
            url: format!("http://{address}"),
            child,
            coverage_profile,
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

    pub async fn sql(&self, statement: &str) -> TestResult {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&self.database)
            .await?;
        let result = sqlx::query(statement).execute(&pool).await;
        pool.close().await;
        result?;
        Ok(())
    }

    pub async fn restart_dependencies(&mut self, client: &Client) -> TestResult {
        self.compose
            .run(&["stop", "-t", "2", "openfga", "postgres"])
            .await?;
        self.compose.run(&["up", "-d", "openfga"]).await?;
        // Docker may assign different ephemeral host ports after a stop/start.
        let postgres = self.compose.address("postgres", "5432").await?;
        self.database = format!(
            "postgres://petlab:{}@{postgres}/petlab?sslmode=disable",
            self.compose.password
        );
        self.fga_url = format!("http://{}", self.compose.address("openfga", "8080").await?);
        self.wait_ready(client).await
    }

    pub async fn stop_fga(&self) -> TestResult {
        self.compose.run(&["stop", "-t", "2", "openfga"]).await?;
        Ok(())
    }

    pub async fn assert_no_tuples(&self, client: &Client, pet: &str) -> TestResult {
        let config = self.config()?;
        let store = config["store_id"].as_str().ok_or("missing store id")?;
        let response: TuplePage = client
            .post(format!("{}/stores/{store}/read", self.fga_url))
            .json(&json!({"tuple_key": {"object": format!("pet:{pet}")},
                "consistency": "HIGHER_CONSISTENCY"}))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert!(
            response.tuples.is_empty() && response.continuation_token.is_empty(),
            "deleted pet still has authorization tuples"
        );
        Ok(())
    }

    pub async fn diagnostics(&self, directory: &std::path::Path) -> TestResult {
        std::fs::create_dir_all(directory)?;
        std::fs::write(
            directory.join("images.txt"),
            self.compose.run(&["images"]).await?,
        )?;
        std::fs::write(
            directory.join("containers.jsonl"),
            self.compose
                .run(&["ps", "--all", "--format", "json"])
                .await?,
        )?;
        let ids = self.compose.run(&["ps", "-q"]).await?;
        if ids.trim().is_empty() {
            return Err("no benchmark containers to measure".into());
        }
        let stats = timeout(
            Duration::from_secs(20),
            Command::new("docker")
                .args(["stats", "--no-stream", "--format", "{{json .}}", "--"])
                .args(ids.lines())
                .kill_on_drop(true)
                .output(),
        )
        .await??;
        if !stats.status.success() {
            return Err("could not capture container resource statistics".into());
        }
        std::fs::write(directory.join("docker-stats.jsonl"), stats.stdout)?;
        Ok(())
    }
}

pub struct App {
    pub url: String,
    child: Child,
    coverage_profile: CoverageProfile,
}

impl App {
    pub async fn stop(mut self) -> TestResult {
        let pid = self
            .child
            .id()
            .ok_or("Axum child has already been reaped")?;
        let stopped = timeout(Duration::from_secs(15), async {
            if let Some(status) = self.child.try_wait()? {
                return Err(format!("Axum child {pid} exited before shutdown: {status}").into());
            }
            // These real-stack tests run on Linux, where SIGTERM lets Axum
            // drain requests and LLVM flush profiles on normal process exit.
            let signal = Command::new("kill")
                .args(["-TERM", "--", &pid.to_string()])
                .kill_on_drop(true)
                .status()
                .await?;
            if !signal.success() {
                return Err(format!("could not signal Axum child {pid}: {signal}").into());
            }
            let status = self.child.wait().await?;
            if !status.success() {
                return Err(format!("Axum child {pid} did not exit cleanly: {status}").into());
            }
            Ok::<(), Box<dyn Error + Send + Sync>>(())
        })
        .await;
        let stopped = match stopped {
            Ok(result) => result,
            Err(error) => Err(format!("Axum child {pid} shutdown timed out: {error}").into()),
        };
        if let Err(error) = stopped {
            // Force-kill/reap is failure cleanup, never a successful teardown.
            let _ = timeout(Duration::from_secs(5), self.child.kill()).await;
            return Err(error);
        }
        if let Some((pattern, prefix)) = &self.coverage_profile {
            let directory = pattern
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
                .unwrap_or_else(|| std::path::Path::new("."));
            let prefix = format!("{prefix}-{pid}-");
            let mut found = false;
            for entry in std::fs::read_dir(directory)? {
                let entry = entry?;
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with(&prefix) && name.ends_with(".profraw") {
                    let metadata = entry.metadata()?;
                    if !metadata.is_file() || metadata.len() == 0 {
                        return Err(
                            format!("empty Axum profile: {}", entry.path().display()).into()
                        );
                    }
                    println!(
                        "Axum child {pid} exited successfully; profile {} ({} bytes)",
                        entry.path().display(),
                        metadata.len()
                    );
                    found = true;
                }
            }
            if !found {
                return Err(format!("Axum child {pid} did not save its LLVM profile").into());
            }
        }
        Ok(())
    }
}
