#![cfg(any(feature = "grpc", feature = "http"))]

//! Independent model workloads against real OpenFGA, without Docker/PostgreSQL.
//! Each workload installs its explicit model in a fresh temporary store.
//! The ignored tests require explicitly supplied official native binaries.

use openfga_axum_lab::application::{Authorizer, Tuple};
#[cfg(feature = "grpc")]
use openfga_axum_lab::infrastructure::OpenFgaGrpc;
#[cfg(all(feature = "http", not(feature = "grpc")))]
use openfga_axum_lab::infrastructure::OpenFgaHttp;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{net::TcpListener, process::Stdio, time::Duration};
use tokio::{process::Child, process::Command, time::timeout};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const OPENLANE_MODEL: &str = include_str!("../scenarios/openlane/fga/model.json");
const LAKEKEEPER_MODEL: &str = include_str!("../fga/model.json");

#[cfg(feature = "grpc")]
const TRANSPORT: &str = "gRPC";
#[cfg(all(feature = "http", not(feature = "grpc")))]
const TRANSPORT: &str = "HTTP";

#[derive(Deserialize)]
struct Cases {
    description: String,
    source_revision: String,
    operations: Vec<Operation>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Operation {
    Check {
        name: String,
        user: String,
        relation: String,
        object: String,
        expected: bool,
    },
    Change {
        name: String,
        writes: Vec<Tuple>,
        deletes: Vec<Tuple>,
        status: u16,
    },
}

struct NativeServer {
    child: Child,
    client: Client,
    http_url: String,
    #[cfg(feature = "grpc")]
    grpc_url: String,
    _log: tempfile::NamedTempFile,
}

impl NativeServer {
    async fn start() -> TestResult<Self> {
        let binary = std::env::var("OPENFGA_TEST_BINARY")
            .map_err(|_| "set OPENFGA_TEST_BINARY to the official OpenFGA 1.10.3 binary")?;
        let version = Command::new(&binary).arg("version").output().await?;
        let version_text = format!(
            "{}{}",
            String::from_utf8_lossy(&version.stdout),
            String::from_utf8_lossy(&version.stderr)
        );
        if !version.status.success() || !version_text.contains("`v1.10.3`") {
            return Err(
                format!("native test requires pinned OpenFGA 1.10.3: {version_text}").into(),
            );
        }
        // Reserve distinct local ports, then release them immediately before
        // spawning the child. No shared/default service ports or user data.
        let http = TcpListener::bind("127.0.0.1:0")?;
        let grpc = TcpListener::bind("127.0.0.1:0")?;
        let http_address = http.local_addr()?;
        let grpc_address = grpc.local_addr()?;
        drop((http, grpc));
        let log = tempfile::NamedTempFile::new()?;
        let child = Command::new(binary)
            .args([
                "run",
                "--datastore-engine",
                "memory",
                "--http-addr",
                &http_address.to_string(),
                "--grpc-addr",
                &grpc_address.to_string(),
                "--playground-enabled=false",
                "--metrics-enabled=false",
                "--log-level",
                "error",
            ])
            .stdout(Stdio::from(log.reopen()?))
            .stderr(Stdio::from(log.reopen()?))
            .kill_on_drop(true)
            .spawn()?;
        let server = Self {
            child,
            client: Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(2))
                .build()?,
            http_url: format!("http://{http_address}"),
            #[cfg(feature = "grpc")]
            grpc_url: format!("http://{grpc_address}"),
            _log: log,
        };
        timeout(Duration::from_secs(8), async {
            loop {
                if let Ok(response) = server
                    .client
                    .get(format!("{}/healthz", server.http_url))
                    .send()
                    .await
                    && response.status() == StatusCode::OK
                    && let Ok(body) = response.json::<Value>().await
                    && body["status"] == "SERVING"
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .map_err(|_| {
            format!(
                "native OpenFGA did not become ready: {}",
                std::fs::read_to_string(server._log.path()).unwrap_or_default()
            )
        })?;
        Ok(server)
    }

    async fn post(&self, path: &str, value: &Value) -> TestResult<Value> {
        Ok(self
            .client
            .post(format!("{}{path}", self.http_url))
            .json(value)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    async fn create_model(&self, name: &str, model_json: &str) -> TestResult<(String, String)> {
        let store = self.post("/stores", &json!({"name": name})).await?["id"]
            .as_str()
            .ok_or("missing native store id")?
            .to_owned();
        let model: Value = serde_json::from_str(model_json)?;
        let model_id = self
            .post(&format!("/stores/{store}/authorization-models"), &model)
            .await?["authorization_model_id"]
            .as_str()
            .ok_or("missing native authorization model id")?
            .to_owned();
        Ok((store, model_id))
    }
}

#[tokio::test]
#[ignore = "requires OPENFGA_TEST_BINARY pointing to official OpenFGA 1.10.3"]
async fn native_openlane_model_and_adapter_matrix() -> TestResult {
    let mut server = NativeServer::start().await?;
    let (store, model_id) = server
        .create_model("openlane-native-model-test", OPENLANE_MODEL)
        .await?;
    #[cfg(feature = "grpc")]
    let auth: Box<dyn Authorizer> =
        Box::new(OpenFgaGrpc::connect(server.grpc_url.clone(), &store, &model_id).await?);
    #[cfg(all(feature = "http", not(feature = "grpc")))]
    let auth: Box<dyn Authorizer> = Box::new(OpenFgaHttp {
        client: server.client.clone(),
        url: server.http_url.clone(),
        store: store.clone(),
        model: model_id.clone(),
    });
    auth.ensure_ready().await?;
    let seed: Vec<Tuple> =
        serde_json::from_str(include_str!("../scenarios/openlane/fga/legacy.json"))?;
    auth.apply(&seed, &[]).await?;
    // Exact seed lists cover inherited ownership, the explicit administrator
    // read grant, assignment, cross-organization sharing and an empty result.
    let expected_seed_lists: &[(&str, &[&str])] = &[
        (
            "alice",
            &[
                "task:access-review",
                "task:incident-response",
                "task:vendor-review",
            ],
        ),
        (
            "bob",
            &[
                "task:access-review",
                "task:incident-response",
                "task:vendor-review",
            ],
        ),
        ("carol", &["task:access-review", "task:incident-response"]),
        ("dave", &["task:vendor-review"]),
        ("erin", &["task:other-review"]),
        ("unrelated", &[]),
    ];
    for (user, expected) in expected_seed_lists {
        let mut listed = auth.list_objects(user, "can_view", "task").await?;
        listed.sort();
        assert_eq!(&listed, expected, "seed list for {user}");
    }
    let cases: Cases = serde_json::from_str(include_str!(
        "../scenarios/openlane/tests/fixtures/openlane-model-cases.json"
    ))?;
    assert_eq!(
        cases.source_revision,
        "1c38df0a53530c8c39eb0784862b06eaa3e26f58"
    );
    println!("{}", cases.description);
    let (mut checks, mut mutations, mut rejected, mut list_checks) = (0, 0, 0, 0);
    for operation in cases.operations {
        match operation {
            Operation::Check {
                name,
                user,
                relation,
                object,
                expected,
            } => {
                let actual = auth.check(&user, &relation, &object).await?;
                assert_eq!(actual, expected, "{name}: {user} {relation} {object}");
                if relation == "can_view" && object.starts_with("task:") {
                    let listed = auth.list_objects(&user, "can_view", "task").await?;
                    assert_eq!(
                        listed.contains(&object),
                        expected,
                        "{name}: ListObjects for {user} must agree with expected visibility of {object}"
                    );
                    list_checks += 1;
                }
                let reference = server
                    .post(
                        &format!("/stores/{store}/check"),
                        &json!({
                            "authorization_model_id": model_id,
                            "consistency": "HIGHER_CONSISTENCY",
                            "tuple_key": {
                                "user": format!("user:{user}"),
                                "relation": relation,
                                "object": object
                            }
                        }),
                    )
                    .await?;
                assert_eq!(reference["allowed"], json!(actual), "{name}: HTTP parity");
                checks += 1;
            }
            Operation::Change {
                name,
                writes,
                deletes,
                status,
            } => {
                let result = auth.apply(&writes, &deletes).await;
                if status == 200 {
                    result.map_err(|error| format!("{name}: {error}"))?;
                } else {
                    assert!(result.is_err(), "{name}: invalid mutation must fail");
                    let mut body = json!({"authorization_model_id": model_id});
                    if !writes.is_empty() {
                        body["writes"] = json!({"tuple_keys": writes, "on_duplicate": "ignore"});
                    }
                    if !deletes.is_empty() {
                        body["deletes"] = json!({"tuple_keys": deletes, "on_missing": "ignore"});
                    }
                    let response = server
                        .client
                        .post(format!("{}/stores/{store}/write", server.http_url))
                        .json(&body)
                        .send()
                        .await?;
                    assert_eq!(response.status().as_u16(), status, "{name}");
                    rejected += 1;
                }
                mutations += 1;
            }
        }
    }
    server.child.kill().await?;
    server.child.wait().await?;
    println!(
        "OpenFGA 1.10.3 memory datastore, {TRANSPORT} adapter: {checks} model checks with HTTP parity; 6 exact seed lists and {list_checks} task-list membership checks across mutations; {mutations} changes plus initial seed; {rejected} invalid changes rejected. PostgreSQL/Axum E2E is a separate gate."
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires OPENFGA_TEST_BINARY pointing to official OpenFGA 1.10.3"]
async fn native_idempotency_does_not_order_late_writes() -> TestResult {
    let mut server = NativeServer::start().await?;
    // The same backend ordering boundary applies to the independently loaded
    // Openlane subset and to the exact default Lakekeeper model. These stores
    // never share tuples, model IDs or the application's configured store.
    let scenarios = [
        (
            "openlane-native-ordering",
            OPENLANE_MODEL,
            "owner",
            "organization:acme",
            ("can_view", "task:ownership-proof"),
            vec![
                json!({"user": "organization:acme", "relation": "parent_context", "object": "task:ownership-proof"}),
                json!({"user": "user:dave", "relation": "assigner", "object": "task:ownership-proof"}),
            ],
        ),
        (
            "lakekeeper-native-ordering",
            LAKEKEEPER_MODEL,
            "ownership",
            "lakekeeper_table:analytics/ordering-proof",
            ("can_read_data", "lakekeeper_table:analytics/ordering-proof"),
            vec![],
        ),
    ];
    for (name, model, owner_relation, owner_object, derived_access, mut seed) in scenarios {
        let (store, model_id) = server.create_model(name, model).await?;
        let path = format!("/stores/{store}/write");
        let owner = |user: &str| json!({"user": format!("user:{user}"), "relation": owner_relation, "object": owner_object});
        seed.push(owner("alice"));
        server
            .post(
                &path,
                &json!({
                    "authorization_model_id": model_id,
                    "writes": {"tuple_keys": seed}
                }),
            )
            .await?;
        let transfer = |old: &str, next: &str| {
            json!({
                "authorization_model_id": model_id,
                "writes": {"tuple_keys": [owner(next)], "on_duplicate": "ignore"},
                "deletes": {"tuple_keys": [owner(old)], "on_missing": "ignore"}
            })
        };
        let original = transfer("alice", "bob");
        // Deliver a recovery replay while an identical original request is
        // delayed outside the server, then deliver a newer ownership transfer.
        // This controls delivery order; it does not cancel a PostgreSQL
        // transaction or exercise the application's durable recovery fence.
        server.post(&path, &original).await?;
        server.post(&path, &transfer("bob", "carol")).await?;
        for late in [false, true] {
            if late {
                server.post(&path, &original).await?;
            }
            for (user, expected) in [("alice", false), ("bob", late), ("carol", true)] {
                for (relation, object) in [(owner_relation, owner_object), derived_access] {
                    let result = server
                        .post(
                            &format!("/stores/{store}/check"),
                            &json!({
                                "authorization_model_id": model_id,
                                "consistency": "HIGHER_CONSISTENCY",
                                "tuple_key": {"user": format!("user:{user}"), "relation": relation, "object": object}
                            }),
                        )
                        .await?;
                    assert_eq!(
                        result["allowed"],
                        json!(expected),
                        "{name}: {user} {relation}, late={late}"
                    );
                }
            }
        }
        println!(
            "{name}: real OpenFGA accepts the old late write and restores Bob alongside Carol; neither atomic Write nor an acknowledged replay provides request ordering."
        );
    }
    server.child.kill().await?;
    server.child.wait().await?;
    Ok(())
}

fn normalize_rewrite(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            fields.retain(|key, value| !(key == "object" && value == ""));
            for value in fields.values_mut() {
                normalize_rewrite(value);
            }
        }
        Value::Array(values) => values.iter_mut().for_each(normalize_rewrite),
        _ => {}
    }
}

fn model_semantics(model: &Value) -> Value {
    let mut types = serde_json::Map::new();
    for definition in model["type_definitions"].as_array().unwrap() {
        let mut relations = definition.get("relations").cloned().unwrap_or(json!({}));
        normalize_rewrite(&mut relations);
        let mut direct = serde_json::Map::new();
        if let Some(metadata) = definition["metadata"]["relations"].as_object() {
            for (relation, details) in metadata {
                if let Some(allowed) = details["directly_related_user_types"].as_array()
                    && !allowed.is_empty()
                {
                    direct.insert(relation.to_owned(), json!(allowed));
                }
            }
        }
        types.insert(
            definition["type"].as_str().unwrap().to_owned(),
            json!({"relations": relations, "direct_types": direct}),
        );
    }
    json!({"schema_version": model["schema_version"], "types": types})
}

#[tokio::test]
#[ignore = "requires FGA_CLI_TEST_BINARY pointing to official fga CLI"]
async fn readable_openlane_model_matches_scenario_json() -> TestResult {
    let binary = std::env::var("FGA_CLI_TEST_BINARY")
        .map_err(|_| "set FGA_CLI_TEST_BINARY to the official fga CLI")?;
    let output = Command::new(binary)
        .args([
            "model",
            "transform",
            concat!(
                "--file=",
                env!("CARGO_MANIFEST_DIR"),
                "/scenarios/openlane/fga/model.fga"
            ),
            "--output-format=json",
        ])
        .output()
        .await?;
    assert!(
        output.status.success(),
        "official DSL transform failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let transformed: Value = serde_json::from_slice(&output.stdout)?;
    let scenario_model: Value = serde_json::from_str(OPENLANE_MODEL)?;
    assert_eq!(
        model_semantics(&transformed),
        model_semantics(&scenario_model)
    );
    Ok(())
}
