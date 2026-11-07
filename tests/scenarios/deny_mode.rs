//! Exercise the emergency switch through a real child, including startup while
//! OpenFGA is stopped. Sessions remain usable, but existing grants and pending
//! recovery work must not turn into access or authorization mutations.

use crate::{
    api::{Api, expect},
    stack::{Stack, TestResult, client},
};
use reqwest::{Client, Method as M, StatusCode as S};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::time::Duration;

async fn pet_rows(db: &PgPool) -> TestResult<Vec<String>> {
    Ok(sqlx::query_scalar("SELECT row_to_json(pets)::text FROM pets ORDER BY id")
        .fetch_all(db)
        .await?)
}

async fn cache_revision(db: &PgPool) -> TestResult<(i64, i64)> {
    Ok(sqlx::query_as("SELECT generation,pending FROM authz_cache_state WHERE singleton")
        .fetch_one(db)
        .await?)
}

async fn tuple_keys(client: &Client, stack: &Stack) -> TestResult<Vec<Value>> {
    let config = stack.config()?;
    let store = config["store_id"].as_str().ok_or("missing store id")?;
    let response: Value = client
        .post(format!("{}/stores/{store}/read", stack.fga_url))
        .json(&json!({"page_size": 100, "consistency": "HIGHER_CONSISTENCY"}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(
        response["continuation_token"].as_str().unwrap_or("").is_empty(),
        "small fixture snapshot must include every tuple"
    );
    let mut tuples: Vec<_> = response["tuples"]
        .as_array()
        .ok_or("missing tuples")?
        .iter()
        .map(|tuple| tuple["key"].clone())
        .collect();
    tuples.sort_unstable_by_key(Value::to_string);
    Ok(tuples)
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn deny_mode_blocks_access_and_recovery_with_openfga_offline() -> TestResult {
    let client = client()?;
    let mut stack = Stack::start(&client).await?;
    let normal = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &normal);
    let alice = stack.login(&client, &normal, "alice").await?;
    let bob = stack.login(&client, &normal, "bob").await?;
    let carol = stack.login(&client, &normal, "carol").await?;
    let dave = stack.login(&client, &normal, "dave").await?;
    let viewer = "/pets/fox/relations/viewer/dave";
    expect(api.call(M::PUT, viewer, &carol), S::NO_CONTENT).await?;
    for token in [&alice, &bob, &carol, &dave] {
        expect(api.call(M::GET, "/pets/fox", token), S::OK).await?;
    }
    let input = json!({"name": "Nori", "species": "cat", "biography": "A quiet companion."});
    let created = expect(
        api.call(M::POST, "/pets", &alice)
            .header("Idempotency-Key", "deny-mode-deleted-fixture")
            .json(&input),
        S::CREATED,
    )
    .await?;
    let id = created["id"].as_str().ok_or("missing created pet id")?;
    let deleted = format!("/pets/{id}");
    expect(api.call(M::DELETE, &deleted, &alice), S::NO_CONTENT).await?;
    stack.assert_no_tuples(&client, id).await?;
    normal.stop().await?;

    // Deny mode must leave interrupted lifecycle work untouched. Normal mode
    // will recover these fixtures only after the switch is disabled again.
    stack
        .sql(include_str!("../../scripts/recovery-fixture.sql"))
        .await?;
    stack
        .sql(include_str!("../../scripts/deletion-fixture.sql"))
        .await?;
    let db = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .connect(stack.database_url())
        .await?;
    let before_rows = pet_rows(&db).await?;
    let before_revision = cache_revision(&db).await?;
    assert_eq!(before_revision.1, 0, "fixture must start without pending fences");
    let before_tuples = tuple_keys(&client, &stack).await?;
    assert!(!before_tuples.is_empty());
    stack.stop_fga().await?;
    assert!(
        client
            .get(format!("{}/healthz", stack.fga_url))
            .send()
            .await
            .is_err(),
        "OpenFGA must be unreachable for the deny-mode startup"
    );

    let denied = stack.app_with_mode(&stack.fga_url, "deny").await?;
    let api = Api::new(client.clone(), &denied);
    let health = api
        .request(M::GET, "/healthz")
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    assert_eq!(health, "ok");
    // This new login, plus existing sessions from the first process, must work
    // without OpenFGA. Authorization failure must not masquerade as bad login.
    let fresh = stack.login(&client, &denied, "carol").await?;
    expect(api.request(M::GET, "/pets"), S::UNAUTHORIZED).await?;
    for token in [&alice, &bob, &carol, &dave, &fresh] {
        let visible = expect(api.call(M::GET, "/pets", token), S::OK).await?;
        assert_eq!(visible, json!([]));
        expect(api.call(M::GET, "/pets/fox", token), S::FORBIDDEN).await?;
    }
    expect(
        api.call(M::POST, "/pets", &alice)
            .header("Idempotency-Key", "deny-mode-must-not-create")
            .json(&input),
        S::FORBIDDEN,
    )
    .await?;
    expect(
        api.call(M::PATCH, "/pets/fox", &carol).json(&input),
        S::FORBIDDEN,
    )
    .await?;
    expect(api.call(M::DELETE, "/pets/fox", &carol), S::FORBIDDEN).await?;
    expect(
        api.call(M::PUT, "/pets/fox/relations/caregiver/dave", &carol),
        S::FORBIDDEN,
    )
    .await?;
    expect(api.call(M::DELETE, viewer, &carol), S::FORBIDDEN).await?;
    for path in ["/pets/recovery-fixture", "/pets/dog"] {
        expect(api.call(M::GET, path, &alice), S::NOT_FOUND).await?;
    }
    // A deletion retry by its recorded actor passes the lifecycle guard. An
    // unfinished deletion still needs the authorizer and must fail closed;
    // an already completed deletion is a successful no-op under either mode.
    expect(
        api.call(M::DELETE, "/pets/dog", &bob),
        S::SERVICE_UNAVAILABLE,
    )
    .await?;
    expect(api.call(M::DELETE, &deleted, &alice), S::NO_CONTENT).await?;
    expect(api.call(M::DELETE, &deleted, &carol), S::NOT_FOUND).await?;
    expect(api.call(M::DELETE, "/session", &fresh), S::NO_CONTENT).await?;
    expect(api.call(M::GET, "/pets", &fresh), S::UNAUTHORIZED).await?;
    assert_eq!(pet_rows(&db).await?, before_rows, "deny mode changed pet data");
    assert_eq!(
        cache_revision(&db).await?,
        before_revision,
        "deny mode changed the authorization fence"
    );
    denied.stop().await?;
    db.close().await;

    // Read the actual stored tuples after the backend returns, before normal
    // startup can legitimately replay pending recovery and change them.
    stack.restart_dependencies(&client).await?;
    assert_eq!(tuple_keys(&client, &stack).await?, before_tuples);
    let restored = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &restored);
    for token in [&alice, &bob, &carol, &dave] {
        expect(api.call(M::GET, "/pets/fox", token), S::OK).await?;
    }
    expect(
        api.call(M::GET, "/pets/recovery-fixture", &carol),
        S::OK,
    )
    .await?;
    expect(api.call(M::GET, "/pets/dog", &bob), S::NOT_FOUND).await?;
    stack.assert_no_tuples(&client, "dog").await?;
    expect(
        api.call(M::PATCH, "/pets/fox", &carol).json(&input),
        S::NO_CONTENT,
    )
    .await?;
    restored.stop().await?;
    Ok(())
}
