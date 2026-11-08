#[path = "support/api.rs"]
pub mod api;
#[path = "scenarios/openlane.rs"]
mod openlane;
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

fn ids(value: &Value) -> BTreeSet<&str> {
    value
        .as_array()
        .expect("task list")
        .iter()
        .map(|task| task["id"].as_str().expect("task id"))
        .collect()
}

fn task_input() -> Value {
    json!({
        "title": "Review access evidence",
        "description": "Verify that each production account has a current approval.",
        "status": "todo"
    })
}

fn create_input(organization: &str) -> Value {
    let mut input = task_input();
    input["organization_id"] = json!(organization);
    input
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
    expect(api.request(M::GET, "/tasks"), S::UNAUTHORIZED).await?;
    expect(api.call(M::GET, "/tasks", "invalid-token"), S::UNAUTHORIZED).await?;
    let all = expect(api.call(M::GET, "/tasks", &alice), S::OK).await?;
    assert!(ids(&all).is_superset(&BTreeSet::from([
        "access-review",
        "vendor-review",
        "incident-response"
    ])));
    let owned = expect(api.call(M::GET, "/tasks", &carol), S::OK).await?;
    assert_eq!(
        ids(&owned),
        BTreeSet::from(["access-review", "incident-response"])
    );
    let visible = expect(api.call(M::GET, "/tasks", &dave), S::OK).await?;
    assert_eq!(ids(&visible), BTreeSet::from(["vendor-review"]));
    expect(
        api.call(M::GET, "/tasks/access-review", &dave),
        S::FORBIDDEN,
    )
    .await?;
    expect(api.call(M::GET, "/tasks/access-review", &bob), S::OK).await?;
    let metrics = api
        .request(M::GET, "/metrics")
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    assert!(metrics.contains("http_requests_total"));
    assert!(metrics.contains("route=\"/tasks/{id}\""));
    assert!(!metrics.contains("route=\"/tasks/access-review\""));
    assert!(
        metrics_have_no_credentials(&metrics, &[&alice, &bob, &carol, &dave]),
        "metrics leaked credential data"
    );
    let edit = task_input();
    expect(
        api.request(M::DELETE, "/tasks/access-review"),
        S::UNAUTHORIZED,
    )
    .await?;
    expect(
        api.call(M::DELETE, "/tasks/access-review", &dave),
        S::FORBIDDEN,
    )
    .await?;
    expect(api.call(M::DELETE, "/tasks/missing", &carol), S::NOT_FOUND).await?;
    expect(
        api.call(M::DELETE, "/tasks/invalid.id", &carol),
        S::BAD_REQUEST,
    )
    .await?;
    // A caller without manage permission cannot discover whether a subject exists.
    for subject in ["dave", "nonexistent"] {
        for method in [M::PUT, M::DELETE] {
            expect(
                api.call(
                    method,
                    &format!("/tasks/access-review/relations/viewer/{subject}"),
                    &dave,
                ),
                S::FORBIDDEN,
            )
            .await?;
        }
    }
    expect(
        api.call(
            M::PUT,
            "/tasks/access-review/relations/viewer/nonexistent",
            &carol,
        ),
        S::NOT_FOUND,
    )
    .await?;
    for token in [&dave, &bob] {
        expect(
            api.call(M::PATCH, "/tasks/access-review", token)
                .json(&edit),
            S::FORBIDDEN,
        )
        .await?;
    }
    let relation = "/tasks/access-review/relations/assignee/dave";
    expect(api.call(M::PUT, relation, &bob), S::FORBIDDEN).await?;
    for _ in 0..2 {
        expect(api.call(M::PUT, relation, &carol), S::NO_CONTENT).await?;
    }
    expect(
        api.call(M::PATCH, "/tasks/access-review", &dave)
            .json(&edit),
        S::NO_CONTENT,
    )
    .await?;
    let task = expect(api.call(M::GET, "/tasks/access-review", &carol), S::OK).await?;
    assert_eq!(task["description"], edit["description"]);
    for _ in 0..2 {
        expect(api.call(M::DELETE, relation, &carol), S::NO_CONTENT).await?;
    }
    expect(
        api.call(M::GET, "/tasks/access-review", &dave),
        S::FORBIDDEN,
    )
    .await?;
    expect(
        api.call(M::POST, "/tasks", &dave)
            .header("Idempotency-Key", Uuid::new_v4().to_string())
            .json(&create_input("acme")),
        S::FORBIDDEN,
    )
    .await?;
    let mut invalid = create_input("acme");
    invalid["title"] = json!("");
    expect(
        api.call(M::POST, "/tasks", &alice)
            .header("Idempotency-Key", Uuid::new_v4().to_string())
            .json(&invalid),
        S::BAD_REQUEST,
    )
    .await?;
    expect(api.call(M::DELETE, "/session", &dave), S::NO_CONTENT).await?;
    expect(api.call(M::GET, "/tasks", &dave), S::UNAUTHORIZED).await?;
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
    let payload = create_input("acme");
    expect(
        api.call(M::POST, "/tasks", &bob).json(&payload),
        S::BAD_REQUEST,
    )
    .await?;
    let key = Uuid::new_v4().to_string();
    let barrier = Arc::new(Barrier::new(6));
    let mut tasks = JoinSet::new();
    for _ in 0..6 {
        let request = api
            .call(M::POST, "/tasks", &bob)
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
        api.call(M::POST, "/tasks", &bob)
            .header("Idempotency-Key", &key)
            .json(&payload),
        S::CREATED,
    )
    .await?;
    assert_eq!(&repeated, created);
    let mut conflicting = payload.clone();
    conflicting["title"] = json!("Different");
    expect(
        api.call(M::POST, "/tasks", &bob)
            .header("Idempotency-Key", &key)
            .json(&conflicting),
        S::CONFLICT,
    )
    .await?;
    let id = created["id"].as_str().ok_or("missing task id")?;
    let path = format!("/tasks/{id}");
    let relation = format!("{path}/relations/viewer/carol");
    expect(api.call(M::GET, &path, &bob), S::OK).await?;
    expect(api.call(M::GET, &path, &carol), S::FORBIDDEN).await?;
    expect(api.call(M::PUT, &relation, &bob), S::NO_CONTENT).await?;
    expect(api.call(M::GET, &path, &carol), S::OK).await?;
    expect(api.call(M::DELETE, &relation, &bob), S::NO_CONTENT).await?;
    expect(api.call(M::GET, &path, &carol), S::FORBIDDEN).await?;
    for _ in 0..2 {
        expect(api.call(M::DELETE, &path, &bob), S::NO_CONTENT).await?;
    }
    expect(api.call(M::GET, &path, &alice), S::NOT_FOUND).await?;
    expect(
        api.call(M::POST, "/tasks", &bob)
            .header("Idempotency-Key", &key)
            .json(&payload),
        S::GONE,
    )
    .await?;
    stack.assert_no_tuples(&client, id).await?;
    let race = expect(
        api.call(M::POST, "/tasks", &bob)
            .header("Idempotency-Key", Uuid::new_v4().to_string())
            .json(&payload),
        S::CREATED,
    )
    .await?;
    let id = race["id"].as_str().ok_or("missing task id")?;
    let path = format!("/tasks/{id}");
    let relation = format!("{path}/relations/viewer/carol");
    let barrier = Arc::new(Barrier::new(8));
    let mut tasks: JoinSet<TestResult<(usize, S)>> = JoinSet::new();
    for index in 0..8 {
        let request = if index == 0 {
            api.call(M::DELETE, &path, &bob)
        } else {
            api.call(M::PUT, &relation, &bob)
        };
        let barrier = barrier.clone();
        tasks.spawn(async move {
            barrier.wait().await;
            Ok((index, request.send().await?.status()))
        });
    }
    let mut completed = 0;
    let mut delete_was_fenced = false;
    let mut fenced_grants = 0;
    while let Some(result) = tasks.join_next().await {
        let (index, status) = result??;
        if index == 0 {
            assert!(
                matches!(status, S::NO_CONTENT | S::SERVICE_UNAVAILABLE),
                "delete request {index} returned {status}"
            );
            delete_was_fenced = status == S::SERVICE_UNAVAILABLE;
        } else {
            assert!(
                matches!(
                    status,
                    S::NO_CONTENT | S::NOT_FOUND | S::SERVICE_UNAVAILABLE
                ),
                "relation grant request {index} returned {status}"
            );
            if status == S::SERVICE_UNAVAILABLE {
                fenced_grants += 1;
            }
        }
        completed += 1;
    }
    assert_eq!(completed, 8);
    // A committed authorization intent fences concurrent mutations with 503.
    // Once its winner completes, retries must preserve deletion semantics;
    // no refused grant may resurrect authorization tuples for a deleted task.
    if delete_was_fenced {
        expect(api.call(M::DELETE, &path, &bob), S::NO_CONTENT).await?;
    }
    for _ in 0..fenced_grants {
        expect(api.call(M::PUT, &relation, &bob), S::NOT_FOUND).await?;
    }
    stack.assert_no_tuples(&client, id).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn mutations_authorize_while_holding_task_lifecycle_lock() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let proxy = proxy::Proxy::start(client.clone(), stack.fga_url.clone()).await?;
    let app = stack.app(&proxy.url).await?;
    let api = Api::new(client.clone(), &app);
    let bob = stack.login(&client, &app, "bob").await?;
    let payload = create_input("acme");
    for (method, suffix) in [
        (M::PATCH, ""),
        (M::DELETE, ""),
        (M::PUT, "/relations/viewer/carol"),
        (M::DELETE, "/relations/viewer/carol"),
    ] {
        // A new object guarantees a real Check, not a warm decision-cache hit.
        let task = expect(
            api.call(M::POST, "/tasks", &bob)
                .header("Idempotency-Key", Uuid::new_v4().to_string())
                .json(&payload),
            S::CREATED,
        )
        .await?;
        let id = task["id"].as_str().ok_or("missing task id")?;
        let path = format!("/tasks/{id}{suffix}");
        let mut pause = proxy.pause("check", false).await;
        let (mutation, locked) = tokio::join!(
            expect(
                api.call(method.clone(), &path, &bob).json(&task_input()),
                S::NO_CONTENT
            ),
            async {
                let captured = pause.captured().await;
                let locked = stack.task_row_is_locked(id).await;
                let organization_locked = stack.organization_row_is_locked("acme").await;
                drop(pause);
                let decision: Value = serde_json::from_slice(&captured?)?;
                assert_eq!(decision["allowed"], true);
                assert!(
                    locked?,
                    "{method} {path} authorized without the task lifecycle lock"
                );
                assert!(
                    organization_locked?,
                    "{method} {path} authorized without the inherited-permission lock"
                );
                Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
            }
        );
        locked?;
        mutation?;
        expect(
            api.call(M::DELETE, &format!("/tasks/{id}"), &bob),
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
    let mut edit = task_input();
    edit["description"] = json!("Persistent across restart");
    let relation = "/tasks/access-review/relations/viewer/dave";
    expect(
        api.call(M::PATCH, "/tasks/access-review", &carol)
            .json(&edit),
        S::NO_CONTENT,
    )
    .await?;
    expect(api.call(M::PUT, relation, &carol), S::NO_CONTENT).await?;
    expect(api.call(M::GET, "/tasks/access-review", &dave), S::OK).await?;
    let config_before = std::fs::read(stack.config_path())?;
    app.stop().await?;
    stack.restart_dependencies(&client).await?;
    stack.bootstrap().await?;
    assert_eq!(std::fs::read(stack.config_path())?, config_before);
    let restarted = stack.app(&stack.fga_url).await?;
    let api = Api::new(client, &restarted);
    // Reuse pre-restart sessions: logging in again would not test session persistence.
    let task = expect(api.call(M::GET, "/tasks/access-review", &dave), S::OK).await?;
    assert_eq!(task["description"], edit["description"]);
    expect(api.call(M::DELETE, relation, &carol), S::NO_CONTENT).await?;
    expect(
        api.call(M::GET, "/tasks/access-review", &dave),
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
    let path = "/tasks/recovery-fixture";
    let task = expect(api.call(M::GET, path, &carol), S::OK).await?;
    assert_eq!(task["id"], "recovery-fixture");
    assert_eq!(task["recovery_required"], true);
    assert_eq!(task["can_delete"], false);
    expect(api.call(M::DELETE, path, &carol), S::SERVICE_UNAVAILABLE).await?;
    app.stop().await?;
    // The fixture was inserted with all apps stopped, and the recovery write
    // completed before startup. No real earlier request exists in this fixture.
    stack.recover_after_backend_drain(&stack.fga_url).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
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
        api.call(M::GET, "/tasks/incident-response", &bob),
        S::NOT_FOUND,
    )
    .await?;
    stack.assert_no_tuples(&client, "incident-response").await?;
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
        for path in ["/tasks/access-review", "/tasks"] {
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
    for path in ["/tasks/access-review", "/tasks"] {
        expect(api.call(M::GET, path, &alice), S::SERVICE_UNAVAILABLE).await?;
    }
    expect(
        api.call(M::POST, "/tasks", &alice)
            .header("Idempotency-Key", Uuid::new_v4().to_string())
            .json(&create_input("acme")),
        S::SERVICE_UNAVAILABLE,
    )
    .await?;
    expect(
        api.call(M::PATCH, "/tasks/access-review", &alice)
            .json(&task_input()),
        S::SERVICE_UNAVAILABLE,
    )
    .await?;
    for (method, path) in [
        (M::DELETE, "/tasks/access-review"),
        (M::PUT, "/tasks/access-review/relations/viewer/dave"),
        (M::DELETE, "/tasks/access-review/relations/viewer/dave"),
    ] {
        expect(api.call(method, path, &alice), S::SERVICE_UNAVAILABLE).await?;
    }
    expect(
        api.call(M::PUT, "/organizations/acme/owner", &alice)
            .json(&json!({"expected_owner":"alice","new_owner":"bob"})),
        S::SERVICE_UNAVAILABLE,
    )
    .await?;
    assert!(
        !stack.authorization_pending("acme").await?,
        "an authorization failure must not enqueue a privileged mutation"
    );
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
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    let alice = stack.login(&client, &app, "alice").await?;
    // Deleting a store leaves the process health endpoint serving. Both
    // transports must reject even cached allows when this store disappears.
    for _ in 0..2 {
        expect(api.call(M::GET, "/tasks/access-review", &alice), S::OK).await?;
    }
    let metrics = api
        .request(M::GET, "/metrics")
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let hits = metrics
        .lines()
        .find_map(|line| line.strip_prefix("authz_cache_events_total{event=\"hit_allow\"} "))
        .unwrap_or("0")
        .parse::<u64>()?;
    assert!(
        hits > 0,
        "store-deletion regression requires a warm allow cache"
    );
    let config = stack.config()?;
    let store = config["store_id"].as_str().ok_or("missing store id")?;
    client
        .delete(format!("{}/stores/{store}", stack.fga_http_url))
        .send()
        .await?
        .error_for_status()?;
    expect(
        api.call(M::GET, "/tasks/access-review", &alice),
        S::SERVICE_UNAVAILABLE,
    )
    .await?;
    expect(api.request(M::GET, "/readyz"), S::SERVICE_UNAVAILABLE).await?;
    let output = stack.bootstrap_command().output().await?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("refusing silent reset"));
    assert_eq!(std::fs::read(stack.config_path())?, original);
    Ok(())
}
