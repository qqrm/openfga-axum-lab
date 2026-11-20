mod api;
mod bootstrap;
mod bridge;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::Path, time::Duration};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const MODEL: &[u8] = include_bytes!("../../../browser-concept/model.json");
const FIXTURE: &[u8] = include_bytes!("../../../browser-concept/fixtures.json");
const OBJECT: &str = "document:demo";
const BODY_LIMIT: usize = 24 * 1024;
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Document {
    id: String,
    title: String,
    body: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    actors: Vec<String>,
    owner: String,
    document: Document,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Health {
    protocol: u8,
    document: String,
    owner: String,
    store_id: String,
    model_id: String,
    model_sha256: String,
    title: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Credentials {
    alice: String,
    bob: String,
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn valid_token(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_ulid(value: &str) -> bool {
    value.len() == 26
        && value
            .bytes()
            .all(|byte| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&byte))
}

fn fixture() -> Result<Fixture> {
    let fixture: Fixture = serde_json::from_slice(FIXTURE)?;
    if fixture.actors.len() != 2
        || fixture.actors[0] != "alice"
        || fixture.actors[1] != "bob"
        || fixture.owner != "alice"
        || fixture.document.id != "demo"
        || fixture.document.title.is_empty()
        || fixture.document.title.len() > 120
        || fixture.document.body.is_empty()
        || fixture.document.body.len() > 4096
    {
        return Err("concept fixture differs from the independent two-actor scenario".into());
    }
    Ok(fixture)
}

fn credentials_path() -> Result<String> {
    std::env::var("CONCEPT_CREDENTIALS_FILE")
        .map_err(|_| "CONCEPT_CREDENTIALS_FILE is required".into())
}

fn read_credentials(path: &Path) -> Result<Credentials> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err("concept credentials must be a regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != 0o600 {
            return Err("concept credentials must have mode 0600".into());
        }
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?.take(1025).read_to_end(&mut bytes)?;
    if bytes.len() > 1024 {
        return Err("concept credentials exceed the size limit".into());
    }
    let credentials: Credentials = serde_json::from_slice(&bytes)?;
    if !valid_token(&credentials.alice)
        || !valid_token(&credentials.bob)
        || credentials.alice == credentials.bob
    {
        return Err("concept credentials are invalid".into());
    }
    Ok(credentials)
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(HTTP_TIMEOUT)
        .timeout(HTTP_TIMEOUT)
        .build()?)
}

pub async fn run() -> Result {
    tracing_subscriber::fmt().with_target(false).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [command] if command == "serve" => api::serve().await,
        [command] if command == "bridge" => bridge::run().await,
        _ => Err("usage: browser-concept serve|bridge".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_reject_duplicates_unknown_fields_and_malformed_tokens() {
        assert!(
            serde_json::from_str::<Credentials>(r#"{"alice":"x","alice":"y","bob":"z"}"#).is_err()
        );
        assert!(
            serde_json::from_str::<Credentials>(r#"{"alice":"x","bob":"y","actor":"alice"}"#)
                .is_err()
        );
        assert!(!valid_token(&"x".repeat(64)));
        assert!(!valid_token(&"a".repeat(63)));
        assert!(valid_token(&"a".repeat(64)));
    }
}
