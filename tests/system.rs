#[path = "support/api.rs"]
pub mod api;
#[path = "cache_support/proxy.rs"]
mod proxy;
#[path = "support/stack.rs"]
pub mod stack;

use api::{Api, expect};
use reqwest::{Method as M, StatusCode as S};
use serde_json::{Value, json};
use stack::{Stack, TestResult, client};
use std::{collections::BTreeSet, sync::Arc};
use tokio::{sync::Barrier, task::JoinSet};
use uuid::Uuid;

fn subject(id: &str) -> Value {
    json!({"subject":{"type":"user","id":id}})
}

fn ids(value: &Value) -> BTreeSet<&str> {
    value["items"]
        .as_array()
        .expect("table page")
        .iter()
        .map(|table| table["id"].as_str().expect("table id"))
        .collect()
}

fn metrics_have_no_credentials(metrics: &str, tokens: &[&str]) -> bool {
    let lower = metrics.to_ascii_lowercase();
    !metrics.contains("demo-")
        && !lower.contains("authorization:")
        && !lower.contains("bearer ")
        && tokens
            .iter()
            .all(|token| !token.is_empty() && !metrics.contains(token))
}

#[test]
fn metrics_guard_allows_help_but_rejects_credentials() {
    let help = "# HELP authz_cache_events_total Authorization decision cache events\n";
    assert!(metrics_have_no_credentials(help, &["synthetic-session"]));
    for leaked in [
        "metric{token=\"synthetic-session\"} 1",
        "# HELP metric synthetic-session",
        "Authorization: synthetic-value",
        "aUtHoRiZaTiOn: synthetic-value",
        "Bearer synthetic-value",
        "metric{value=\"demo-value\"} 1",
    ] {
        assert!(!metrics_have_no_credentials(leaked, &["synthetic-session"]));
    }
    assert!(!metrics_have_no_credentials(help, &[""]));
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn authorization_sessions_and_metrics() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    for username in ["alice", "nonexistent"] {
        expect(
            api.request(M::POST, "/session")
                .json(&json!({"username": username, "password": "wrong-password"})),
            S::UNAUTHORIZED,
        )
        .await?;
    }
    let alice = stack.login(&client, &app, "alice").await?;
    let bob = stack.login(&client, &app, "bob").await?;
    let carol = stack.login(&client, &app, "carol").await?;
    let dave = stack.login(&client, &app, "dave").await?;
    expect(
        api.request(M::GET, "/projects/acme/warehouses/analytics/tables"),
        S::UNAUTHORIZED,
    )
    .await?;
    expect(
        api.call(
            M::GET,
            "/projects/acme/warehouses/analytics/tables",
            "invalid-token",
        ),
        S::UNAUTHORIZED,
    )
    .await?;
    let all = expect(
        api.call(M::GET, "/projects/acme/warehouses/analytics/tables", &alice),
        S::OK,
    )
    .await?;
    assert!(ids(&all).is_superset(&BTreeSet::from(["requests", "latency", "errors"])));
    let owned = expect(
        api.call(M::GET, "/projects/acme/warehouses/analytics/tables", &carol),
        S::OK,
    )
    .await?;
    assert_eq!(ids(&owned), BTreeSet::from(["requests"]));
    let visible = expect(
        api.call(M::GET, "/projects/acme/warehouses/analytics/tables", &dave),
        S::OK,
    )
    .await?;
    assert_eq!(ids(&visible), BTreeSet::from(["latency"]));
    expect(
        api.call(
            M::GET,
            "/projects/acme/warehouses/analytics/tables/requests",
            &dave,
        ),
        S::FORBIDDEN,
    )
    .await?;
    expect(
        api.call(
            M::GET,
            "/projects/acme/warehouses/analytics/tables/requests",
            &bob,
        ),
        S::OK,
    )
    .await?;
    let metrics = api
        .request(M::GET, "/metrics")
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    assert!(metrics.contains("http_requests_total"));
    assert!(metrics.contains("route=\"/projects/{project}/warehouses/{warehouse}/tables/{id}\""));
    assert!(!metrics.contains("route=\"/projects/acme/warehouses/analytics/tables/requests\""));
    assert!(
        metrics_have_no_credentials(&metrics, &[&alice, &bob, &carol, &dave]),
        "metrics leaked credential data"
    );
    let edit = json!({"name": "HTTP requests", "description": "Service request observations."});
    expect(
        api.request(
            M::DELETE,
            "/projects/acme/warehouses/analytics/tables/requests",
        ),
        S::UNAUTHORIZED,
    )
    .await?;
    expect(
        api.call(
            M::DELETE,
            "/projects/acme/warehouses/analytics/tables/requests",
            &dave,
        ),
        S::FORBIDDEN,
    )
    .await?;
    expect(
        api.call(
            M::DELETE,
            "/projects/acme/warehouses/analytics/tables/missing",
            &carol,
        ),
        S::NOT_FOUND,
    )
    .await?;
    expect(
        api.call(
            M::DELETE,
            "/projects/acme/warehouses/analytics/tables/invalid.id",
            &carol,
        ),
        S::BAD_REQUEST,
    )
    .await?;
    // A caller without manage permission cannot discover whether a subject exists.
    for subject_id in ["dave", "nonexistent"] {
        for method in [M::PUT, M::DELETE] {
            expect(
                api.call(
                    method,
                    "/projects/acme/warehouses/analytics/tables/requests/grants/select",
                    &dave,
                )
                .json(&subject(subject_id)),
                S::FORBIDDEN,
            )
            .await?;
        }
    }
    expect(
        api.call(
            M::PUT,
            "/projects/acme/warehouses/analytics/tables/requests/grants/select",
            &carol,
        )
        .json(&subject("nonexistent")),
        S::NOT_FOUND,
    )
    .await?;
    for token in [&dave, &bob] {
        expect(
            api.call(
                M::PATCH,
                "/projects/acme/warehouses/analytics/tables/requests",
                token,
            )
            .json(&edit),
            S::FORBIDDEN,
        )
        .await?;
    }
    let relation = "/projects/acme/warehouses/analytics/tables/requests/grants/modify";
    expect(
        api.call(M::PUT, relation, &bob).json(&subject("dave")),
        S::FORBIDDEN,
    )
    .await?;
    for _ in 0..2 {
        expect(
            api.call(M::PUT, relation, &carol).json(&subject("dave")),
            S::NO_CONTENT,
        )
        .await?;
    }
    expect(
        api.call(
            M::PATCH,
            "/projects/acme/warehouses/analytics/tables/requests",
            &dave,
        )
        .json(&edit),
        S::NO_CONTENT,
    )
    .await?;
    let table = expect(
        api.call(
            M::GET,
            "/projects/acme/warehouses/analytics/tables/requests",
            &carol,
        ),
        S::OK,
    )
    .await?;
    assert_eq!(table["description"], edit["description"]);
    for _ in 0..2 {
        expect(
            api.call(M::DELETE, relation, &carol).json(&subject("dave")),
            S::NO_CONTENT,
        )
        .await?;
    }
    expect(
        api.call(
            M::GET,
            "/projects/acme/warehouses/analytics/tables/requests",
            &dave,
        ),
        S::FORBIDDEN,
    )
    .await?;
    expect(
        api.call(M::POST, "/projects/acme/warehouses/analytics/tables", &dave)
            .header("Idempotency-Key", Uuid::new_v4().to_string())
            .json(&json!({"namespace_id":"metrics","name":"Unauthorized","description":""})),
        S::FORBIDDEN,
    )
    .await?;
    expect(
        api.call(
            M::POST,
            "/projects/acme/warehouses/analytics/tables",
            &alice,
        )
        .header("Idempotency-Key", Uuid::new_v4().to_string())
        .json(&json!({"namespace_id": "metrics", "name": "", "description": ""})),
        S::BAD_REQUEST,
    )
    .await?;
    expect(api.call(M::DELETE, "/session", &dave), S::NO_CONTENT).await?;
    expect(
        api.call(M::GET, "/projects/acme/warehouses/analytics/tables", &dave),
        S::UNAUTHORIZED,
    )
    .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn concurrent_idempotency_and_deletion_leave_no_tuples() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    let alice = stack.login(&client, &app, "alice").await?;
    let bob = stack.login(&client, &app, "bob").await?;
    let carol = stack.login(&client, &app, "carol").await?;
    let payload = json!({"namespace_id": "metrics", "name": "Service events", "description": "Structured event observations."});
    expect(
        api.call(M::POST, "/projects/acme/warehouses/analytics/tables", &bob)
            .json(&payload),
        S::BAD_REQUEST,
    )
    .await?;
    let key = Uuid::new_v4().to_string();
    let barrier = Arc::new(Barrier::new(6));
    let mut tasks = JoinSet::new();
    for _ in 0..6 {
        let request = api
            .call(M::POST, "/projects/acme/warehouses/analytics/tables", &bob)
            .header("Idempotency-Key", &key)
            .json(&payload);
        let barrier = barrier.clone();
        tasks.spawn(async move {
            barrier.wait().await;
            expect(request, S::CREATED).await
        });
    }
    let mut results = Vec::new();
    while let Some(result) = tasks.join_next().await {
        results.push(result??);
    }
    assert_eq!(results.len(), 6);
    let created = &results[0];
    assert!(results.iter().all(|value| value == created));
    let repeated = expect(
        api.call(M::POST, "/projects/acme/warehouses/analytics/tables", &bob)
            .header("Idempotency-Key", &key)
            .json(&payload),
        S::CREATED,
    )
    .await?;
    assert_eq!(&repeated, created);
    let mut conflicting = payload.clone();
    conflicting["name"] = json!("Different");
    expect(
        api.call(M::POST, "/projects/acme/warehouses/analytics/tables", &bob)
            .header("Idempotency-Key", &key)
            .json(&conflicting),
        S::CONFLICT,
    )
    .await?;
    let id = created["id"].as_str().ok_or("missing table id")?;
    let path = format!("/projects/acme/warehouses/analytics/tables/{id}");
    let relation = format!("{path}/grants/select");
    expect(api.call(M::GET, &path, &bob), S::OK).await?;
    expect(api.call(M::GET, &path, &carol), S::FORBIDDEN).await?;
    expect(
        api.call(M::PUT, &relation, &bob).json(&subject("carol")),
        S::NO_CONTENT,
    )
    .await?;
    expect(api.call(M::GET, &path, &carol), S::OK).await?;
    expect(
        api.call(M::DELETE, &relation, &bob).json(&subject("carol")),
        S::NO_CONTENT,
    )
    .await?;
    expect(api.call(M::GET, &path, &carol), S::FORBIDDEN).await?;
    for _ in 0..2 {
        expect(api.call(M::DELETE, &path, &bob), S::NO_CONTENT).await?;
    }
    expect(api.call(M::GET, &path, &alice), S::NOT_FOUND).await?;
    expect(
        api.call(M::POST, "/projects/acme/warehouses/analytics/tables", &bob)
            .header("Idempotency-Key", &key)
            .json(&payload),
        S::GONE,
    )
    .await?;
    stack.assert_no_tuples(&client, id).await?;
    let race = expect(
        api.call(M::POST, "/projects/acme/warehouses/analytics/tables", &bob)
            .header("Idempotency-Key", Uuid::new_v4().to_string())
            .json(&payload),
        S::CREATED,
    )
    .await?;
    let id = race["id"].as_str().ok_or("missing table id")?;
    let path = format!("/projects/acme/warehouses/analytics/tables/{id}");
    let relation = format!("{path}/grants/select");
    let barrier = Arc::new(Barrier::new(8));
    let mut tasks: JoinSet<TestResult<(usize, S)>> = JoinSet::new();
    for index in 0..8 {
        let request = if index == 0 {
            api.call(M::DELETE, &path, &bob)
        } else {
            api.call(M::PUT, &relation, &bob).json(&subject("carol"))
        };
        let barrier = barrier.clone();
        tasks.spawn(async move {
            barrier.wait().await;
            Ok((index, request.send().await?.status()))
        });
    }
    let mut completed = 0;
    while let Some(result) = tasks.join_next().await {
        let (index, status) = result??;
        if index == 0 {
            assert_eq!(status, S::NO_CONTENT, "delete request {index}");
        } else {
            assert!(
                matches!(status, S::NO_CONTENT | S::NOT_FOUND),
                "relation grant request {index} returned {status}"
            );
        }
        completed += 1;
    }
    assert_eq!(completed, 8);
    stack.assert_no_tuples(&client, id).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn mutations_authorize_while_holding_table_lifecycle_lock() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let proxy = proxy::Proxy::start(client.clone(), stack.fga_url.clone()).await?;
    let app = stack.app(&proxy.url).await?;
    let api = Api::new(client.clone(), &app);
    let bob = stack.login(&client, &app, "bob").await?;
    let payload = json!({"namespace_id": "metrics", "name": "Service events", "description": "Structured event observations."});
    for (method, suffix) in [
        (M::PATCH, ""),
        (M::DELETE, ""),
        (M::PUT, "/grants/select"),
        (M::DELETE, "/grants/select"),
    ] {
        // Pause the authoritative Check and inspect the lock from another connection.
        let table = expect(
            api.call(M::POST, "/projects/acme/warehouses/analytics/tables", &bob)
                .header("Idempotency-Key", Uuid::new_v4().to_string())
                .json(&payload),
            S::CREATED,
        )
        .await?;
        let id = table["id"].as_str().ok_or("missing table id")?;
        let path = format!("/projects/acme/warehouses/analytics/tables/{id}{suffix}");
        let mut pause = proxy.pause("check", false).await;
        let (mutation, locked) =
            tokio::join!(
            expect(
                api.call(method.clone(), &path, &bob).json(&if suffix.is_empty() {
                    json!({"name":"Updated service events", "description":"Metadata update"})
                } else { subject("carol") }),
                S::NO_CONTENT
            ),
            async {
                let captured = pause.captured().await;
                let locked = stack.table_row_is_locked(id).await;
                drop(pause);
                let decision: Value = serde_json::from_slice(&captured?)?;
                assert_eq!(decision["allowed"], true);
                assert!(
                    locked?,
                    "{method} {path} authorized without the table lifecycle lock"
                );
                Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
            }
        );
        locked?;
        mutation?;
        expect(
            api.call(
                M::DELETE,
                &format!("/projects/acme/warehouses/analytics/tables/{id}"),
                &bob,
            ),
            S::NO_CONTENT,
        )
        .await?;
        stack.assert_no_tuples(&client, id).await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn data_relations_and_sessions_survive_full_restart() -> TestResult {
    let client = client()?;
    let mut stack = Stack::start(&client).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    let carol = stack.login(&client, &app, "carol").await?;
    let dave = stack.login(&client, &app, "dave").await?;
    let edit = json!({"name": "HTTP requests", "description": "Persistent across restart"});
    let relation = "/projects/acme/warehouses/analytics/tables/requests/grants/select";
    expect(
        api.call(
            M::PATCH,
            "/projects/acme/warehouses/analytics/tables/requests",
            &carol,
        )
        .json(&edit),
        S::NO_CONTENT,
    )
    .await?;
    expect(
        api.call(M::PUT, relation, &carol).json(&subject("dave")),
        S::NO_CONTENT,
    )
    .await?;
    expect(
        api.call(
            M::GET,
            "/projects/acme/warehouses/analytics/tables/requests",
            &dave,
        ),
        S::OK,
    )
    .await?;
    let config_before = std::fs::read(stack.config_path())?;
    app.stop().await?;
    stack.restart_dependencies(&client).await?;
    stack.bootstrap().await?;
    assert_eq!(std::fs::read(stack.config_path())?, config_before);
    let restarted = stack.app(&stack.fga_url).await?;
    let api = Api::new(client, &restarted);
    // Reuse pre-restart sessions: logging in again would not test session persistence.
    let table = expect(
        api.call(
            M::GET,
            "/projects/acme/warehouses/analytics/tables/requests",
            &dave,
        ),
        S::OK,
    )
    .await?;
    assert_eq!(table["description"], edit["description"]);
    expect(
        api.call(M::DELETE, relation, &carol).json(&subject("dave")),
        S::NO_CONTENT,
    )
    .await?;
    expect(
        api.call(
            M::GET,
            "/projects/acme/warehouses/analytics/tables/requests",
            &dave,
        ),
        S::FORBIDDEN,
    )
    .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn pending_creation_recovers_after_restart() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let app = stack.app(&stack.fga_url).await?;
    let carol = stack.login(&client, &app, "carol").await?;
    app.stop().await?;
    stack
        .sql(include_str!("../scripts/recovery-fixture.sql"))
        .await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    let path = "/projects/acme/warehouses/analytics/tables/recovery-fixture";
    let table = expect(api.call(M::GET, path, &carol), S::OK).await?;
    assert_eq!(table["id"], "recovery-fixture");
    expect(api.call(M::DELETE, path, &carol), S::NO_CONTENT).await?;
    stack.assert_no_tuples(&client, "recovery-fixture").await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn pending_deletion_recovers_and_removes_all_tuples() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let app = stack.app(&stack.fga_url).await?;
    let bob = stack.login(&client, &app, "bob").await?;
    app.stop().await?;
    stack
        .sql(include_str!("../scripts/deletion-fixture.sql"))
        .await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    expect(
        api.call(
            M::GET,
            "/projects/acme/warehouses/analytics/tables/errors",
            &bob,
        ),
        S::NOT_FOUND,
    )
    .await?;
    stack.assert_no_tuples(&client, "errors").await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn openfga_outage_fails_closed_even_with_warm_allows() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let proxy = proxy::Proxy::start(client.clone(), stack.fga_url.clone()).await?;
    let app = stack.app(&proxy.url).await?;
    let api = Api::new(client.clone(), &app);
    expect(api.request(M::GET, "/readyz"), S::OK).await?;
    let alice = stack.login(&client, &app, "alice").await?;
    for _ in 0..2 {
        for path in [
            "/projects/acme/warehouses/analytics/tables/requests",
            "/projects/acme/warehouses/analytics/tables",
        ] {
            expect(api.call(M::GET, path, &alice), S::OK).await?;
        }
    }
    // The real backend is healthy, but the application's selected transport fails.
    // Readiness must see that failure instead of using a warm authorization result.
    let mut pause = proxy.pause("healthz", true).await;
    let (ready, upstream) = tokio::join!(
        expect(api.request(M::GET, "/readyz"), S::SERVICE_UNAVAILABLE),
        async {
            pause.captured().await?;
            expect(client.get(format!("{}/healthz", stack.fga_http_url)), S::OK).await?;
            drop(pause);
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        }
    );
    upstream?;
    ready?;
    expect(api.request(M::GET, "/readyz"), S::OK).await?;
    stack.stop_fga().await?;
    expect(api.request(M::GET, "/readyz"), S::SERVICE_UNAVAILABLE).await?;
    for path in [
        "/projects/acme/warehouses/analytics/tables/requests",
        "/projects/acme/warehouses/analytics/tables",
    ] {
        expect(api.call(M::GET, path, &alice), S::SERVICE_UNAVAILABLE).await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn bootstrap_reuses_state_and_refuses_implicit_resets() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let original = std::fs::read(stack.config_path())?;
    stack.bootstrap().await?;
    assert_eq!(std::fs::read(stack.config_path())?, original);
    let mut changed = stack.config()?;
    changed["model_sha256"] = json!("0".repeat(64));
    let changed = serde_json::to_vec(&changed)?;
    std::fs::write(stack.config_path(), &changed)?;
    let output = stack.bootstrap_command().output().await?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("explicit migration"));
    assert_eq!(std::fs::read(stack.config_path())?, changed);
    std::fs::write(stack.config_path(), &original)?;
    let config = stack.config()?;
    let store = config["store_id"].as_str().ok_or("missing store id")?;
    client
        .delete(format!("{}/stores/{store}", stack.fga_http_url))
        .send()
        .await?
        .error_for_status()?;
    let output = stack.bootstrap_command().output().await?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("refusing silent reset"));
    assert_eq!(std::fs::read(stack.config_path())?, original);
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn deny_mode_blocks_existing_permissions_during_outage_and_unknown_mode_refuses_startup()
-> TestResult {
    const TABLES: &str = "/projects/acme/warehouses/analytics/tables";
    const TABLE: &str = "/projects/acme/warehouses/analytics/tables/requests";
    let client = client()?;
    let mut stack = Stack::start(&client).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    let alice = stack.login(&client, &app, "alice").await?;
    expect(api.call(M::GET, TABLE, &alice), S::OK).await?;
    app.stop().await?;

    let mut unknown = stack.app_command(&stack.fga_url);
    unknown.env("AUTHZ_MODE", "permissive-typo");
    let output =
        tokio::time::timeout(std::time::Duration::from_secs(60), unknown.output()).await??;
    assert!(
        !output.status.success(),
        "unknown mode must not silently select a provider"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("AUTHZ_MODE must be openfga or deny"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Listening on "));

    // The explicit deny provider must start and deny access even when the real
    // OpenFGA service is down, rather than falling back to warmed permissions.
    stack.stop_fga().await?;
    let app = stack.app_in_mode(&stack.fga_url, "deny").await?;
    let api = Api::new(client.clone(), &app);
    let carol = stack.login(&client, &app, "carol").await?;
    expect(api.request(M::GET, TABLE), S::UNAUTHORIZED).await?;
    for token in [&alice, &carol] {
        expect(api.call(M::GET, TABLE, token), S::FORBIDDEN).await?;
        let page = expect(api.call(M::GET, TABLES, token), S::OK).await?;
        assert!(ids(&page).is_empty());
        assert!(page["next_cursor"].is_null());
        let catalog = expect(api.call(M::GET, "/catalog", token), S::OK).await?;
        assert!(
            catalog["scopes"]
                .as_array()
                .ok_or("missing scopes")?
                .is_empty()
        );
        let permission = expect(
            api.call(M::POST, &format!("{TABLE}/check"), token)
                .json(&json!({"action":"can_get_metadata"})),
            S::OK,
        )
        .await?;
        assert_eq!(permission["allowed"], false);
    }
    expect(api.request(M::GET, "/readyz"), S::SERVICE_UNAVAILABLE).await?;
    expect(
        api.call(M::POST, TABLES, &alice)
            .header("Idempotency-Key", Uuid::new_v4().to_string())
            .json(&json!({"namespace_id":"metrics","name":"Denied creation","description":""})),
        S::FORBIDDEN,
    )
    .await?;
    expect(
        api.call(M::PATCH, TABLE, &alice)
            .json(&json!({"name":"Changed","description":"Denied edit"})),
        S::FORBIDDEN,
    )
    .await?;
    expect(api.call(M::DELETE, TABLE, &alice), S::FORBIDDEN).await?;
    for method in [M::PUT, M::DELETE] {
        expect(
            api.call(method.clone(), &format!("{TABLE}/grants/select"), &carol)
                .json(&subject("dave")),
            S::FORBIDDEN,
        )
        .await?;
        expect(
            api.call(
                method,
                "/projects/acme/roles/analysts/assignees/bob",
                &alice,
            ),
            S::FORBIDDEN,
        )
        .await?;
    }
    expect(
        api.call(M::PUT, &format!("{TABLE}/owner"), &carol)
            .json(&json!({"owner_id":"dave"})),
        S::FORBIDDEN,
    )
    .await?;
    expect(
        api.call(M::PUT, &format!("{TABLE}/namespace"), &alice)
            .json(&json!({"namespace_id":"restricted"})),
        S::FORBIDDEN,
    )
    .await?;
    let pool = stack.pool().await?;
    let state: (String, String, String) = sqlx::query_as("SELECT status,owner_id,namespace_id FROM catalog_tables WHERE warehouse_id='analytics' AND id='requests'")
        .fetch_one(&pool).await?;
    assert_eq!(state, ("active".into(), "carol".into(), "metrics".into()));
    let pending: i64 = sqlx::query_scalar("SELECT pending FROM authz_cache_state WHERE singleton")
        .fetch_one(&pool)
        .await?;
    assert_eq!(
        pending, 0,
        "denied commands must never begin a remote mutation"
    );
    pool.close().await;
    app.stop().await?;
    stack.restart_dependencies(&client).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client, &app);
    expect(api.call(M::GET, TABLE, &alice), S::OK).await?;
    let table = expect(api.call(M::GET, TABLE, &carol), S::OK).await?;
    assert_eq!(table["owner_id"], "carol");
    assert_eq!(table["namespace_id"], "metrics");
    app.stop().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn paginated_lists_bind_cursors_to_actor_scope_and_expiry_and_recheck_live_permissions()
-> TestResult {
    const TABLES: &str = "/projects/acme/warehouses/analytics/tables";
    const FOREIGN: &str = "/projects/other/warehouses/external/tables";
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    let alice = stack.login(&client, &app, "alice").await?;
    let carol = stack.login(&client, &app, "carol").await?;
    let dave = stack.login(&client, &app, "dave").await?;
    let eve = stack.login(&client, &app, "eve").await?;
    let first = expect(
        api.call(M::GET, TABLES, &alice).query(&[("limit", "1")]),
        S::OK,
    )
    .await?;
    assert_eq!(ids(&first), BTreeSet::from(["errors"]));
    let cursor = first["next_cursor"]
        .as_str()
        .ok_or("missing first-page cursor")?;
    Uuid::parse_str(cursor)?;
    let second = expect(
        api.call(M::GET, TABLES, &alice)
            .query(&[("limit", "1"), ("cursor", cursor)]),
        S::OK,
    )
    .await?;
    assert_eq!(ids(&second), BTreeSet::from(["latency"]));
    let next = second["next_cursor"]
        .as_str()
        .ok_or("missing second-page cursor")?;
    let repeat = expect(
        api.call(M::GET, TABLES, &alice)
            .query(&[("limit", "1"), ("cursor", cursor)]),
        S::OK,
    )
    .await?;
    assert_eq!(
        ids(&repeat),
        ids(&second),
        "same actor/scope may reuse an unexpired cursor"
    );
    let last = expect(
        api.call(M::GET, TABLES, &alice)
            .query(&[("limit", "1"), ("cursor", next)]),
        S::OK,
    )
    .await?;
    assert_eq!(ids(&last), BTreeSet::from(["requests"]));
    assert!(last["next_cursor"].is_null());
    expect(
        api.call(M::GET, TABLES, &dave).query(&[("cursor", cursor)]),
        S::BAD_REQUEST,
    )
    .await?;
    expect(
        api.call(M::GET, FOREIGN, &eve).query(&[("cursor", cursor)]),
        S::BAD_REQUEST,
    )
    .await?;
    expect(
        api.call(M::GET, TABLES, &eve).query(&[("cursor", cursor)]),
        S::NOT_FOUND,
    )
    .await?;
    let pool = stack.pool().await?;
    // A user may belong to two projects. Even then a valid cursor for one
    // scope cannot be used to traverse another scope with the same identity.
    sqlx::query("INSERT INTO catalog_memberships(project_id,user_id) VALUES('other','alice')")
        .execute(&pool)
        .await?;
    expect(
        api.call(M::GET, FOREIGN, &alice)
            .query(&[("cursor", cursor)]),
        S::BAD_REQUEST,
    )
    .await?;
    for limit in ["0", "101"] {
        expect(
            api.call(M::GET, TABLES, &alice).query(&[("limit", limit)]),
            S::BAD_REQUEST,
        )
        .await?;
    }
    for invalid in ["not-a-uuid".to_owned(), Uuid::new_v4().to_string()] {
        expect(
            api.call(M::GET, TABLES, &alice)
                .query(&[("cursor", invalid)]),
            S::BAD_REQUEST,
        )
        .await?;
    }
    // Advance the durable expiration deterministically instead of sleeping for
    // the production TTL. No cleanup request runs before this negative check.
    sqlx::query("UPDATE catalog_list_cursors SET expires_at=now()-interval '1 second' WHERE id=$1")
        .bind(cursor)
        .execute(&pool)
        .await?;
    expect(
        api.call(M::GET, TABLES, &alice)
            .query(&[("cursor", cursor)]),
        S::BAD_REQUEST,
    )
    .await?;
    expect(
        api.call(M::GET, TABLES, &alice).query(&[("cursor", next)]),
        S::OK,
    )
    .await?;

    let visible = expect(
        api.call(M::GET, TABLES, &dave).query(&[("limit", "1")]),
        S::OK,
    )
    .await?;
    assert_eq!(ids(&visible), BTreeSet::from(["latency"]));
    let dave_cursor = visible["next_cursor"]
        .as_str()
        .ok_or("missing filtered-page cursor")?;
    let empty = expect(
        api.call(M::GET, TABLES, &dave)
            .query(&[("cursor", dave_cursor)]),
        S::OK,
    )
    .await?;
    assert!(ids(&empty).is_empty());
    let grant = format!("{TABLES}/requests/grants/select");
    expect(
        api.call(M::PUT, &grant, &carol).json(&subject("dave")),
        S::NO_CONTENT,
    )
    .await?;
    let granted = expect(
        api.call(M::GET, TABLES, &dave)
            .query(&[("cursor", dave_cursor)]),
        S::OK,
    )
    .await?;
    assert_eq!(ids(&granted), BTreeSet::from(["requests"]));
    expect(
        api.call(M::DELETE, &grant, &carol).json(&subject("dave")),
        S::NO_CONTENT,
    )
    .await?;
    let revoked = expect(
        api.call(M::GET, TABLES, &dave)
            .query(&[("cursor", dave_cursor)]),
        S::OK,
    )
    .await?;
    assert!(
        ids(&revoked).is_empty(),
        "a previously valid cursor must not retain revoked visibility"
    );
    assert!(revoked["next_cursor"].is_null());
    pool.close().await;
    app.stop().await?;
    Ok(())
}
