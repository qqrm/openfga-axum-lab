#[path = "support/api.rs"]
pub mod api;
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
        .expect("pet list")
        .iter()
        .map(|pet| pet["id"].as_str().expect("pet id"))
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
    expect(api.request(M::GET, "/pets"), S::UNAUTHORIZED).await?;
    expect(api.call(M::GET, "/pets", "invalid-token"), S::UNAUTHORIZED).await?;
    let all = expect(api.call(M::GET, "/pets", &alice), S::OK).await?;
    assert!(ids(&all).is_superset(&BTreeSet::from(["fox", "cat", "dog"])));
    let owned = expect(api.call(M::GET, "/pets", &carol), S::OK).await?;
    assert_eq!(ids(&owned), BTreeSet::from(["fox"]));
    let visible = expect(api.call(M::GET, "/pets", &dave), S::OK).await?;
    assert_eq!(ids(&visible), BTreeSet::from(["cat"]));
    expect(api.call(M::GET, "/pets/fox", &dave), S::FORBIDDEN).await?;
    expect(api.call(M::GET, "/pets/fox", &bob), S::OK).await?;
    let metrics = api
        .request(M::GET, "/metrics")
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    assert!(metrics.contains("http_requests_total"));
    assert!(metrics.contains("route=\"/pets/{id}\""));
    assert!(!metrics.contains("route=\"/pets/fox\""));
    assert!(
        metrics_have_no_credentials(&metrics, &[&alice, &bob, &carol, &dave]),
        "metrics leaked credential data"
    );
    let edit = json!({"name": "Mika", "species": "fox", "biography": "Mika likes apples."});
    for token in [&dave, &bob] {
        expect(
            api.call(M::PATCH, "/pets/fox", token).json(&edit),
            S::FORBIDDEN,
        )
        .await?;
    }
    let relation = "/pets/fox/relations/caregiver/dave";
    expect(api.call(M::PUT, relation, &bob), S::FORBIDDEN).await?;
    for _ in 0..2 {
        expect(api.call(M::PUT, relation, &carol), S::NO_CONTENT).await?;
    }
    expect(
        api.call(M::PATCH, "/pets/fox", &dave).json(&edit),
        S::NO_CONTENT,
    )
    .await?;
    let pet = expect(api.call(M::GET, "/pets/fox", &carol), S::OK).await?;
    assert_eq!(pet["biography"], edit["biography"]);
    for _ in 0..2 {
        expect(api.call(M::DELETE, relation, &carol), S::NO_CONTENT).await?;
    }
    expect(api.call(M::GET, "/pets/fox", &dave), S::FORBIDDEN).await?;
    expect(api.call(M::POST, "/pets", &dave).json(&edit), S::FORBIDDEN).await?;
    expect(
        api.call(M::POST, "/pets", &alice)
            .header("Idempotency-Key", Uuid::new_v4().to_string())
            .json(&json!({"name": "", "species": "fox", "biography": ""})),
        S::BAD_REQUEST,
    )
    .await?;
    expect(api.call(M::DELETE, "/session", &dave), S::NO_CONTENT).await?;
    expect(api.call(M::GET, "/pets", &dave), S::UNAUTHORIZED).await?;
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
    let payload = json!({"name": "Biscuit", "species": "rabbit", "biography": "Likes carrots."});
    expect(
        api.call(M::POST, "/pets", &bob).json(&payload),
        S::BAD_REQUEST,
    )
    .await?;
    let key = Uuid::new_v4().to_string();
    let barrier = Arc::new(Barrier::new(6));
    let mut tasks = JoinSet::new();
    for _ in 0..6 {
        let request = api
            .call(M::POST, "/pets", &bob)
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
        api.call(M::POST, "/pets", &bob)
            .header("Idempotency-Key", &key)
            .json(&payload),
        S::CREATED,
    )
    .await?;
    assert_eq!(&repeated, created);
    let mut conflicting = payload.clone();
    conflicting["name"] = json!("Different");
    expect(
        api.call(M::POST, "/pets", &bob)
            .header("Idempotency-Key", &key)
            .json(&conflicting),
        S::CONFLICT,
    )
    .await?;
    let id = created["id"].as_str().ok_or("missing pet id")?;
    let path = format!("/pets/{id}");
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
        api.call(M::POST, "/pets", &bob)
            .header("Idempotency-Key", &key)
            .json(&payload),
        S::GONE,
    )
    .await?;
    stack.assert_no_tuples(&client, id).await?;
    let race = expect(
        api.call(M::POST, "/pets", &bob)
            .header("Idempotency-Key", Uuid::new_v4().to_string())
            .json(&payload),
        S::CREATED,
    )
    .await?;
    let id = race["id"].as_str().ok_or("missing pet id")?;
    let path = format!("/pets/{id}");
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
    while let Some(result) = tasks.join_next().await {
        let (index, status) = result??;
        if index == 0 {
            assert_eq!(status, S::NO_CONTENT);
        } else {
            assert!(matches!(status, S::NO_CONTENT | S::NOT_FOUND));
        }
        completed += 1;
    }
    assert_eq!(completed, 8);
    stack.assert_no_tuples(&client, id).await?;
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
    let edit = json!({"name": "Mika", "species": "fox", "biography": "Persistent across restart"});
    let relation = "/pets/fox/relations/viewer/dave";
    expect(
        api.call(M::PATCH, "/pets/fox", &carol).json(&edit),
        S::NO_CONTENT,
    )
    .await?;
    expect(api.call(M::PUT, relation, &carol), S::NO_CONTENT).await?;
    expect(api.call(M::GET, "/pets/fox", &dave), S::OK).await?;
    let config_before = std::fs::read(stack.config_path())?;
    app.stop().await?;
    stack.restart_dependencies(&client).await?;
    stack.bootstrap().await?;
    assert_eq!(std::fs::read(stack.config_path())?, config_before);
    let restarted = stack.app(&stack.fga_url).await?;
    let api = Api::new(client, &restarted);
    // Reuse pre-restart sessions: logging in again would not test session persistence.
    let pet = expect(api.call(M::GET, "/pets/fox", &dave), S::OK).await?;
    assert_eq!(pet["biography"], edit["biography"]);
    expect(api.call(M::DELETE, relation, &carol), S::NO_CONTENT).await?;
    expect(api.call(M::GET, "/pets/fox", &dave), S::FORBIDDEN).await?;
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
    let path = "/pets/recovery-fixture";
    let pet = expect(api.call(M::GET, path, &carol), S::OK).await?;
    assert_eq!(pet["id"], "recovery-fixture");
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
    expect(api.call(M::GET, "/pets/dog", &bob), S::NOT_FOUND).await?;
    stack.assert_no_tuples(&client, "dog").await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn openfga_outage_fails_closed_even_with_warm_allows() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    let alice = stack.login(&client, &app, "alice").await?;
    for _ in 0..2 {
        for path in ["/pets/fox", "/pets"] {
            expect(api.call(M::GET, path, &alice), S::OK).await?;
        }
    }
    stack.stop_fga().await?;
    for path in ["/pets/fox", "/pets"] {
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
