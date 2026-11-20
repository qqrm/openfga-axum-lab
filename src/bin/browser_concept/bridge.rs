//! The UART bridge chooses a provisioned demo session and calls the real API.
//! It never evaluates permissions or substitutes an HTTP status for a failure.
use super::{
    BODY_LIMIT, Credentials, HTTP_TIMEOUT, Health, MODEL, Result, client, credentials_path, digest,
    fixture, read_credentials, valid_ulid,
};
use reqwest::{Client, Method};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, Read, Write},
    path::Path,
    time::Duration,
};

const MAX_FRAME_BYTES: usize = 4096;
const MAX_COMMAND_ID: u64 = 9_007_199_254_740_991;
const READY_TIMEOUT: Duration = Duration::from_secs(900);

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum Actor {
    Alice,
    Bob,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum Operation {
    Permissions,
    Read,
    Grant,
    Revoke,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Command {
    id: u64,
    operation: Operation,
    actor: Actor,
}

#[derive(Serialize)]
struct Observation {
    id: u64,
    operation: Operation,
    actor: Actor,
    http_status: Option<u16>,
    body: Value,
    elapsed_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'static str>,
}

fn command(frame: &[u8], previous_id: u64) -> Result<Command> {
    if frame.len() > MAX_FRAME_BYTES {
        return Err("command frame exceeds the limit".into());
    }
    let line = std::str::from_utf8(frame)?.trim_end_matches(['\r', '\n']);
    let input = line
        .strip_prefix("CONCEPT|command|")
        .ok_or("invalid command prefix")?;
    let command: Command = serde_json::from_str(input)?;
    if command.id <= previous_id || command.id > MAX_COMMAND_ID {
        return Err("command identifier must increase within the safe integer range".into());
    }
    Ok(command)
}

fn emit(event: &str, value: &impl Serialize) -> Result {
    let mut output = io::stdout().lock();
    writeln!(output, "CONCEPT|{event}|{}", serde_json::to_string(value)?)?;
    output.flush()?;
    Ok(())
}

async fn bounded_json(mut response: reqwest::Response) -> Result<Value> {
    if response
        .content_length()
        .is_some_and(|length| length > BODY_LIMIT as u64)
    {
        return Err("HTTP response exceeds the body limit".into());
    }
    let status = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > BODY_LIMIT - bytes.len() {
            return Err("HTTP response exceeds the body limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    if status == reqwest::StatusCode::NO_CONTENT {
        if !bytes.is_empty() {
            return Err("HTTP 204 response contained a body".into());
        }
        return Ok(Value::Null);
    }
    let body: Value = serde_json::from_slice(&bytes)?;
    if !body.is_object() {
        return Err("HTTP response must be a JSON object".into());
    }
    Ok(body)
}

async fn wait_ready(client: &Client, base: &str) -> Result<Health> {
    tokio::time::timeout(READY_TIMEOUT, async {
        loop {
            if let Ok(response) = client.get(format!("{base}/concept/health")).send().await
                && response.status() == reqwest::StatusCode::OK
            {
                let health: Health = serde_json::from_value(bounded_json(response).await?)?;
                if health.protocol != 1
                    || health.document != "demo"
                    || health.owner != "alice"
                    || health.model_sha256 != digest(MODEL)
                    || health.title != fixture()?.document.title
                    || !valid_ulid(&health.store_id)
                    || !valid_ulid(&health.model_id)
                {
                    return Err(
                        "concept health differs from this binary's model and fixture".into(),
                    );
                }
                return Ok(health);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    })
    .await?
}

async fn execute(
    client: &Client,
    base: &str,
    credentials: &Credentials,
    command: Command,
) -> Observation {
    let started = std::time::Instant::now();
    let token = match command.actor {
        Actor::Alice => &credentials.alice,
        Actor::Bob => &credentials.bob,
    };
    let (method, path) = match command.operation {
        Operation::Permissions => (Method::GET, "/concept/permissions"),
        Operation::Read => (Method::GET, "/concept/document"),
        Operation::Grant => (Method::PUT, "/concept/document/viewer"),
        Operation::Revoke => (Method::DELETE, "/concept/document/viewer"),
    };
    let mut request = client
        .request(method, format!("{base}{path}"))
        .bearer_auth(token);
    if matches!(command.operation, Operation::Grant | Operation::Revoke) {
        request = request.json(&json!({"user":"bob"}));
    }
    let mut http_status = None;
    let mut body = Value::Null;
    // One request, one deadline, no retry. A timeout may leave a mutation's
    // outcome unknown; the next permission read observes authoritative state.
    let outcome: std::result::Result<Result, _> = tokio::time::timeout(HTTP_TIMEOUT, async {
        let response = request.send().await?;
        http_status = Some(response.status().as_u16());
        body = bounded_json(response).await?;
        Ok(())
    })
    .await;
    let error = match outcome {
        Ok(Ok(())) => None,
        Ok(Err(_)) => Some("http_transport_or_body_failure"),
        Err(_) => Some("http_deadline_exceeded"),
    };
    Observation {
        id: command.id,
        operation: command.operation,
        actor: command.actor,
        http_status,
        body,
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        error,
    }
}

pub(super) async fn run() -> Result {
    let client = client()?;
    let base = std::env::var("CONCEPT_BASE_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:3000".into())
        .trim_end_matches('/')
        .to_owned();
    let health = wait_ready(&client, &base).await?;
    let credentials = read_credentials(Path::new(&credentials_path()?))?;
    emit("ready", &health)?;
    let mut input = io::stdin().lock();
    let mut previous_id = 0;
    loop {
        let mut frame = Vec::new();
        let count = (&mut input)
            .take((MAX_FRAME_BYTES + 1) as u64)
            .read_until(b'\n', &mut frame)?;
        if count == 0 {
            return Ok(());
        }
        if frame.len() > MAX_FRAME_BYTES || !frame.ends_with(b"\n") {
            return Err("unterminated or oversized concept command".into());
        }
        if frame.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match command(&frame, previous_id) {
            Ok(command) => {
                previous_id = command.id;
                emit(
                    "response",
                    &execute(&client, &base, &credentials, command).await,
                )?;
            }
            Err(_) => emit("error", &json!({"error":"invalid_command_or_sequence"}))?,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_identity_and_operation_are_bounded_and_unambiguous() {
        let valid = br#"CONCEPT|command|{"id":1,"operation":"grant","actor":"alice"}"#;
        assert!(command(valid, 0).is_ok());
        assert!(command(valid, 1).is_err());
        for input in [
            r#"{"id":0,"operation":"read","actor":"bob"}"#,
            r#"{"id":9007199254740992,"operation":"read","actor":"bob"}"#,
            r#"{"id":1,"operation":"write","actor":"alice"}"#,
            r#"{"id":1,"operation":"read","actor":"eve"}"#,
            r#"{"id":1,"operation":"read","actor":"bob","token":"forged"}"#,
            r#"{"id":1,"operation":"read","actor":"bob","actor":"alice"}"#,
        ] {
            assert!(command(format!("CONCEPT|command|{input}").as_bytes(), 0).is_err());
        }
        assert!(command(&vec![b' '; MAX_FRAME_BYTES + 1], 0).is_err());
    }
}
