use crate::ToolResult;
use openfga_axum_lab::model_binding::policy_model;
use reqwest::{Client, RequestBuilder, StatusCode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{ffi::OsStr, fs, io::Write, path::Path, time::Duration};
use tokio::time::{sleep, timeout};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_MODEL_REQUEST_TIMEOUT_SECONDS: u64 = 30;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
// Lakekeeper authorization schema v4.15, also enforced by the SQL migration.
const PINNED_MODEL_SHA256: &str =
    "a894f61fb40563467d0a16409981acb14224c619073f39c42dca233609a78e6d";

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Config {
    store_id: String,
    model_id: String,
    model_sha256: String,
}

#[derive(Deserialize)]
struct Health {
    status: String,
}
#[derive(Deserialize)]
struct Store {
    id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreatedModel {
    authorization_model_id: String,
}
#[derive(Deserialize)]
struct ModelResponse {
    authorization_model: Model,
}
#[derive(Deserialize, Serialize)]
struct Model {
    id: String,
    schema_version: String,
    type_definitions: Value,
    #[serde(default)]
    conditions: Option<Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadPage {
    #[serde(default)]
    tuples: Vec<Value>,
    #[serde(default)]
    continuation_token: String,
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn valid_id(value: &str) -> bool {
    value.len() == 26
        && value
            .bytes()
            .all(|byte| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&byte))
}

fn validate_existing(config: &Config, model: &[u8]) -> ToolResult {
    if !valid_id(&config.store_id) || !valid_id(&config.model_id) {
        return Err("existing store/model identifiers are invalid".into());
    }
    if config.model_sha256 != digest(model) || config.model_sha256 != PINNED_MODEL_SHA256 {
        return Err(
            "model differs from the pinned catalog model; explicit migration required".into(),
        );
    }
    Ok(())
}

fn model_request_timeout(value: Option<&OsStr>) -> ToolResult<Duration> {
    let Some(value) = value else {
        return Ok(REQUEST_TIMEOUT);
    };
    let error = "BROWSER_BOOTSTRAP_MODEL_TIMEOUT_SECONDS must be an integer from 5 to 30";
    let value = value.to_str().ok_or(error)?;
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(error.into());
    }
    let seconds = value.parse::<u64>().map_err(|_| error)?;
    if !(REQUEST_TIMEOUT.as_secs()..=MAX_MODEL_REQUEST_TIMEOUT_SECONDS).contains(&seconds) {
        return Err(error.into());
    }
    Ok(Duration::from_secs(seconds))
}

async fn send_json<T: DeserializeOwned>(
    builder: RequestBuilder,
    expected: StatusCode,
) -> ToolResult<T> {
    send_json_with_timeout(builder, expected, REQUEST_TIMEOUT).await
}

async fn send_json_with_timeout<T: DeserializeOwned>(
    builder: RequestBuilder,
    expected: StatusCode,
    request_timeout: Duration,
) -> ToolResult<T> {
    timeout(request_timeout, async {
        let (client, request) = builder.timeout(request_timeout).build_split();
        let request = request?;
        let requested_url = request.url().clone();
        let mut response = client.execute(request).await?;
        if response.status() != expected || response.url() != &requested_url {
            return Err(format!(
                "OpenFGA did not confirm this operation: HTTP {}; expected {expected}",
                response.status()
            )
            .into());
        }
        if !response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|header| header.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
        {
            return Err("OpenFGA returned a non-JSON response".into());
        }
        if response
            .content_length()
            .is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
        {
            return Err("OpenFGA response is too large".into());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if chunk.len() > MAX_RESPONSE_BYTES - bytes.len() {
                return Err("OpenFGA response is too large".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        // Serde also accepts positional arrays for structs. Protocol messages
        // must be JSON objects, including messages containing only default fields.
        if bytes
            .iter()
            .copied()
            .find(|byte| !byte.is_ascii_whitespace())
            != Some(b'{')
        {
            return Err("OpenFGA response must be a JSON object".into());
        }
        Ok(serde_json::from_slice(&bytes)?)
    })
    .await?
}

async fn wait_ready(client: &Client, base: &str) -> ToolResult {
    timeout(Duration::from_secs(60), async {
        loop {
            if let Ok(health) =
                send_json::<Health>(client.get(format!("{base}/healthz")), StatusCode::OK).await
                && health.status == "SERVING"
            {
                break;
            }
            // Only this read-only health probe is retried. A create operation
            // with an uncertain outcome is never retried automatically.
            sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .map_err(|_| "OpenFGA unavailable")?;
    Ok(())
}

async fn verify_target(
    client: &Client,
    base: &str,
    config: &Config,
    model: &Value,
    model_timeout: Duration,
) -> ToolResult {
    let store: Store = send_json(
        client.get(format!("{base}/stores/{}", config.store_id)),
        StatusCode::OK,
    )
    .await
    .map_err(|error| {
        format!("OpenFGA target cannot be verified; refusing silent reset: {error}")
    })?;
    if store.id != config.store_id {
        return Err("persisted OpenFGA store identity differs; refusing silent reset".into());
    }
    let actual: ModelResponse = send_json_with_timeout(
        client.get(format!(
            "{base}/stores/{}/authorization-models/{}",
            config.store_id, config.model_id
        )),
        StatusCode::OK,
        model_timeout,
    )
    .await
    .map_err(|error| {
        format!("OpenFGA target cannot be verified; refusing silent reset: {error}")
    })?;
    let actual = actual.authorization_model;
    if actual.id != config.model_id
        || policy_model(&serde_json::to_value(&actual)?)? != policy_model(model)?
    {
        return Err(
            "persisted OpenFGA model differs from the pinned model; refusing silent reset".into(),
        );
    }
    Ok(())
}

fn write_config(path: &Path, config: &Config) -> ToolResult {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(directory)?;
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> ToolResult {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        serde_json::to_writer(&mut file, config)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        // Both names are on the same filesystem. Publishing a hard link is
        // atomic and fails if a concurrent bootstrap already bound this path;
        // unlike rename it cannot overwrite an existing store/model binding.
        fs::hard_link(&temporary, path)?;
        fs::remove_file(&temporary)?;
        #[cfg(unix)]
        fs::File::open(directory)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

async fn provision(
    base: &str,
    path: &Path,
    model_path: &Path,
    model_timeout: Duration,
) -> ToolResult {
    let model_bytes = fs::read(model_path)?;
    let model_hash = digest(&model_bytes);
    if model_hash != PINNED_MODEL_SHA256 {
        return Err("bootstrap requires the exact pinned Lakekeeper model".into());
    }
    let model: Value = serde_json::from_slice(&model_bytes)?;
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(REQUEST_TIMEOUT)
        .build()?;
    let base = base.trim_end_matches('/');
    match fs::read(path) {
        Ok(bytes) => {
            let config: Config = serde_json::from_slice(&bytes)?;
            validate_existing(&config, &model_bytes)?;
            wait_ready(&client, base).await?;
            verify_target(&client, base, &config, &model, model_timeout).await?;
            println!("Using existing store and pinned model");
            return Ok(());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    wait_ready(&client, base).await?;
    // CreateStore and WriteAuthorizationModel return 201 in the OpenFGA API;
    // GET and Read return 200. Never treat arbitrary 2xx/3xx as confirmation.
    let store: Store = send_json(
        client
            .post(format!("{base}/stores"))
            .json(&json!({"name": "axum-catalog-lab"})),
        StatusCode::CREATED,
    )
    .await?;
    if !valid_id(&store.id) {
        return Err("OpenFGA returned an invalid store identifier".into());
    }
    let created: CreatedModel = send_json_with_timeout(
        client
            .post(format!("{base}/stores/{}/authorization-models", store.id))
            .json(&model),
        StatusCode::CREATED,
        model_timeout,
    )
    .await?;
    if !valid_id(&created.authorization_model_id) {
        return Err("OpenFGA returned an invalid model identifier".into());
    }
    let config = Config {
        store_id: store.id,
        model_id: created.authorization_model_id,
        model_sha256: model_hash,
    };
    verify_target(&client, base, &config, &model, model_timeout).await?;
    let page: ReadPage = send_json(
        client
            .post(format!("{base}/stores/{}/read", config.store_id))
            .json(&json!({"page_size": 1, "consistency": "HIGHER_CONSISTENCY"})),
        StatusCode::OK,
    )
    .await?;
    if !page.tuples.is_empty() || !page.continuation_token.is_empty() {
        return Err(
            "new authorization store is not confirmed empty; configuration was not published"
                .into(),
        );
    }
    write_config(path, &config)?;
    println!(
        "Empty store and pinned model provisioned; SQL authorization migration remains required"
    );
    Ok(())
}

pub async fn run() -> ToolResult {
    // Opt in only the two administrative model operations. Health, store
    // identity, tuple reads and all other CLI commands retain their deadlines.
    let configured_timeout = std::env::var_os("BROWSER_BOOTSTRAP_MODEL_TIMEOUT_SECONDS");
    let model_timeout = model_request_timeout(configured_timeout.as_deref())?;
    let base = std::env::var("FGA_URL").unwrap_or_else(|_| "http://openfga:8080".into());
    let path = std::env::var("FGA_CONFIG").unwrap_or_else(|_| "/data/config.json".into());
    let model = std::env::var("FGA_MODEL").unwrap_or_else(|_| "/app/fga/model.json".into());
    provision(&base, Path::new(&path), Path::new(&model), model_timeout).await
}

#[cfg(test)]
#[path = "bootstrap_tests.rs"]
mod tests;
