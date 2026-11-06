use crate::ToolResult;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, io::Write, path::Path, time::Duration};
use tokio::time::{sleep, timeout};

#[derive(Debug, Deserialize, Serialize)]
struct Config {
    store_id: String,
    model_id: String,
    model_sha256: String,
}

fn validate_existing(config: &Config, model: &[u8]) -> ToolResult {
    if config.store_id.is_empty() || config.model_id.is_empty() {
        return Err("existing store/model identifiers are missing".into());
    }
    if config.model_sha256 != format!("{:x}", Sha256::digest(model)) {
        return Err("model changed; explicit migration is required".into());
    }
    Ok(())
}

async fn wait_ready(client: &Client, base: &str) -> ToolResult {
    timeout(Duration::from_secs(60), async {
        loop {
            if let Ok(response) = client.get(format!("{base}/healthz")).send().await
                && response.status() == StatusCode::OK
                && let Ok(body) = response.json::<Value>().await
                && body["status"] == "SERVING"
            {
                break;
            }
            sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .map_err(|_| "OpenFGA unavailable")?;
    Ok(())
}

fn write_config(path: &Path, config: &Config) -> ToolResult {
    if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
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
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub async fn run() -> ToolResult {
    let base = std::env::var("FGA_URL").unwrap_or_else(|_| "http://openfga:8080".into());
    let path = std::env::var("FGA_CONFIG").unwrap_or_else(|_| "/data/config.json".into());
    let model_path = std::env::var("FGA_MODEL").unwrap_or_else(|_| "/app/fga/model.json".into());
    let tuples_path = std::env::var("FGA_TUPLES").unwrap_or_else(|_| "/app/fga/legacy.json".into());
    let model_bytes = fs::read(model_path)?;
    let model: Value = serde_json::from_slice(&model_bytes)?;
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()?;
    match fs::read(&path) {
        Ok(bytes) => {
            let config: Config = serde_json::from_slice(&bytes)?;
            validate_existing(&config, &model_bytes)?;
            wait_ready(&client, &base).await?;
            // A model can remain readable after its store is soft-deleted.
            // Check both resources before accepting a persisted configuration.
            for endpoint in [
                format!("{base}/stores/{}", config.store_id),
                format!(
                    "{base}/stores/{}/authorization-models/{}",
                    config.store_id, config.model_id
                ),
            ] {
                let response = client.get(endpoint).send().await?;
                if response.status() == StatusCode::NOT_FOUND {
                    return Err("existing store/model missing; refusing silent reset".into());
                }
                response.error_for_status()?;
            }
            println!("Using existing store and model");
            return Ok(());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let tuples: Value = serde_json::from_slice(&fs::read(tuples_path)?)?;
    wait_ready(&client, &base).await?;
    // Never retry a write with an ambiguous outcome: readiness retries are GET-only.
    let store: Value = client
        .post(format!("{base}/stores"))
        .json(&json!({"name": "axum-lab"}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let store_id = store["id"].as_str().ok_or("missing store id")?;
    let response: Value = client
        .post(format!("{base}/stores/{store_id}/authorization-models"))
        .json(&model)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let model_id = response["authorization_model_id"]
        .as_str()
        .ok_or("missing model id")?;
    client
        .post(format!("{base}/stores/{store_id}/write"))
        .json(&json!({"authorization_model_id": model_id, "writes": {"tuple_keys": tuples}}))
        .send()
        .await?
        .error_for_status()?;
    write_config(
        Path::new(&path),
        &Config {
            store_id: store_id.into(),
            model_id: model_id.into(),
            model_sha256: format!("{:x}", Sha256::digest(&model_bytes)),
        },
    )?;
    println!("Demo migration complete");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Config, validate_existing, write_config};
    use sha2::{Digest, Sha256};

    fn config() -> Config {
        Config {
            store_id: "store".into(),
            model_id: "model".into(),
            model_sha256: format!("{:x}", Sha256::digest(b"model")),
        }
    }

    #[test]
    fn unchanged_model_is_reused() {
        assert!(validate_existing(&config(), b"model").is_ok());
    }

    #[test]
    fn changed_model_requires_explicit_migration() {
        assert!(validate_existing(&config(), b"different").is_err());
    }

    #[test]
    fn missing_identifiers_fail_closed() {
        let mut value = config();
        value.store_id.clear();
        assert!(validate_existing(&value, b"model").is_err());
        let mut value = config();
        value.model_id.clear();
        assert!(validate_existing(&value, b"model").is_err());
    }

    #[test]
    fn config_is_published_without_a_partial_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        write_config(&path, &config()).unwrap();
        let actual: Config = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(actual.model_sha256, config().model_sha256);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
