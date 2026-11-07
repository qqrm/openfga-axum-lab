use super::{
    api::{Api, expect},
    stack::{self, Stack, TestResult, client},
};
#[path = "../cache_support/proxy.rs"]
mod proxy;

use proxy::Proxy;
use reqwest::{Client, Method as M, StatusCode as S};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};

const PENDING: &str = include_str!("../../scripts/recovery-fixture.sql");
const DELETING: &str = include_str!("../../scripts/deletion-fixture.sql");
const DATABASE_ERROR: &str = "injected recovery database failure";

async fn assert_status(db: &PgPool, id: &str, expected: &str) -> TestResult {
    let status: String = sqlx::query_scalar("SELECT status FROM pets WHERE id=$1")
        .bind(id)
        .fetch_one(db)
        .await?;
    assert_eq!(status, expected, "unexpected durable state for {id}");
    Ok(())
}

async fn assert_hidden(api: &Api, id: &str, token: &str) -> TestResult {
    expect(
        api.call(M::GET, &format!("/pets/{id}"), token),
        S::NOT_FOUND,
    )
    .await?;
    let visible = expect(api.call(M::GET, "/pets", token), S::OK).await?;
    assert!(
        visible
            .as_array()
            .ok_or("expected pet list")?
            .iter()
            .all(|pet| pet["id"] != id),
        "unfinished recovery exposed {id} in the pet list"
    );
    Ok(())
}

async fn assert_creation_tuples(stack: &Stack, client: &Client) -> TestResult {
    let config = stack.config()?;
    let store = config["store_id"].as_str().ok_or("missing store id")?;
    let page: Value = client
        .post(format!("{}/stores/{store}/read", stack.fga_url))
        .json(&json!({
            "tuple_key": {"object": "pet:recovery-fixture"},
            "consistency": "HIGHER_CONSISTENCY"
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let tuples: Vec<_> = page["tuples"]
        .as_array()
        .ok_or("missing tuples")?
        .iter()
        .map(|tuple| {
            let mut key = tuple["key"].clone();
            assert!(key["condition"].is_null(), "fixture must be unconditional");
            key.as_object_mut().expect("tuple key").remove("condition");
            key
        })
        .collect();
    assert_eq!(tuples.len(), 2, "creation must have exactly its two tuples");
    for (user, relation) in [("user:carol", "owner"), ("shelter:central", "shelter")] {
        assert!(tuples.iter().any(|tuple| *tuple
            == json!({"user": user, "relation": relation, "object": "pet:recovery-fixture"})));
    }
    assert!(
        page["continuation_token"].as_str().unwrap_or("").is_empty(),
        "unexpected extra creation tuples"
    );
    Ok(())
}

async fn lose_recovery_response(stack: &Stack, proxy: &Proxy, error: &str) -> TestResult {
    let mut pause = proxy.pause("write", true).await;
    let release = async {
        // This gate fires only after the real OpenFGA write/delete succeeded.
        // Release it immediately with a 502: recovery cannot trust that result.
        pause.captured().await?;
        drop(pause);
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    };
    let (startup, captured) = tokio::join!(stack.assert_startup_fails(&proxy.url, error), release);
    captured?;
    startup
}

async fn assert_creation_replays_safely(
    stack: &Stack,
    client: &Client,
    db: &PgPool,
    carol: &str,
    dave: &str,
) -> TestResult {
    let recovered = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &recovered);
    assert_status(db, "recovery-fixture", "active").await?;
    let pet = expect(api.call(M::GET, "/pets/recovery-fixture", carol), S::OK).await?;
    assert_eq!(pet["id"], "recovery-fixture");
    assert_eq!(pet["name"], "Luna");
    expect(
        api.call(M::GET, "/pets/recovery-fixture", dave),
        S::FORBIDDEN,
    )
    .await?;
    // Recovery must tolerate tuples committed by an earlier failed startup. It
    // must not duplicate them, grant an unrelated user access, or lose the row.
    assert_creation_tuples(stack, client).await?;
    expect(
        api.call(M::DELETE, "/pets/recovery-fixture", carol),
        S::NO_CONTENT,
    )
    .await?;
    assert_status(db, "recovery-fixture", "deleted").await?;
    stack.assert_no_tuples(client, "recovery-fixture").await?;
    recovered.stop().await
}

async fn assert_deletion_replays_safely(
    stack: &Stack,
    client: &Client,
    db: &PgPool,
    bob: &str,
) -> TestResult {
    let recovered = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &recovered);
    assert_status(db, "dog", "deleted").await?;
    assert_hidden(&api, "dog", bob).await?;
    stack.assert_no_tuples(client, "dog").await?;
    // Repeated deletion by the original actor must remain safe after recovery.
    expect(api.call(M::DELETE, "/pets/dog", bob), S::NO_CONTENT).await?;
    stack.assert_no_tuples(client, "dog").await?;
    recovered.stop().await
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn lost_provisioning_response_keeps_pending_until_recovery_replays() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let serving = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &serving);
    let carol = stack.login(&client, &serving, "carol").await?;
    let dave = stack.login(&client, &serving, "dave").await?;
    let db = PgPoolOptions::new()
        .max_connections(1)
        .connect(stack.database_url())
        .await?;
    stack.sql(PENDING).await?;
    let proxy = Proxy::start(client.clone(), stack.fga_url.clone()).await?;
    lose_recovery_response(&stack, &proxy, "pending authorization recovery failed").await?;
    assert_status(&db, "recovery-fixture", "pending").await?;
    assert_creation_tuples(&stack, &client).await?;
    assert_hidden(&api, "recovery-fixture", &carol).await?;
    assert_hidden(&api, "recovery-fixture", &dave).await?;
    assert_creation_replays_safely(&stack, &client, &db, &carol, &dave).await?;
    serving.stop().await?;
    db.close().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn lost_deletion_response_keeps_deleting_until_recovery_replays() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let serving = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &serving);
    let bob = stack.login(&client, &serving, "bob").await?;
    let db = PgPoolOptions::new()
        .max_connections(1)
        .connect(stack.database_url())
        .await?;
    stack.sql(DELETING).await?;
    let proxy = Proxy::start(client.clone(), stack.fga_url.clone()).await?;
    lose_recovery_response(&stack, &proxy, "pending deletion recovery failed").await?;
    assert_status(&db, "dog", "deleting").await?;
    assert_hidden(&api, "dog", &bob).await?;
    stack.assert_no_tuples(&client, "dog").await?;
    assert_deletion_replays_safely(&stack, &client, &db, &bob).await?;
    serving.stop().await?;
    db.close().await;
    Ok(())
}

async fn install_database_fault(stack: &Stack, trigger: &str) -> TestResult {
    stack
        .sql(
            "CREATE FUNCTION fail_recovery() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN RAISE EXCEPTION 'injected recovery database failure'; END; $$",
        )
        .await?;
    stack.sql(trigger).await
}

async fn remove_database_fault(stack: &Stack) -> TestResult {
    stack.sql("DROP TRIGGER recovery_fault ON pets").await?;
    stack.sql("DROP FUNCTION fail_recovery()").await
}

async fn failed_activation_keeps_pending(trigger: &str) -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let serving = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &serving);
    let carol = stack.login(&client, &serving, "carol").await?;
    let dave = stack.login(&client, &serving, "dave").await?;
    let db = PgPoolOptions::new()
        .max_connections(1)
        .connect(stack.database_url())
        .await?;
    stack.sql(PENDING).await?;
    install_database_fault(&stack, trigger).await?;
    stack
        .assert_startup_fails(&stack.fga_url, DATABASE_ERROR)
        .await?;
    assert_status(&db, "recovery-fixture", "pending").await?;
    assert_creation_tuples(&stack, &client).await?;
    assert_hidden(&api, "recovery-fixture", &carol).await?;
    assert_hidden(&api, "recovery-fixture", &dave).await?;
    remove_database_fault(&stack).await?;
    assert_creation_replays_safely(&stack, &client, &db, &carol, &dave).await?;
    serving.stop().await?;
    db.close().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn activation_update_error_keeps_pending_until_database_is_repaired() -> TestResult {
    failed_activation_keeps_pending(
        "CREATE TRIGGER recovery_fault BEFORE UPDATE ON pets FOR EACH ROW
         WHEN (OLD.id='recovery-fixture' AND OLD.status='pending' AND NEW.status='active')
         EXECUTE FUNCTION fail_recovery()",
    )
    .await
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn activation_commit_error_keeps_pending_until_database_is_repaired() -> TestResult {
    failed_activation_keeps_pending(
        "CREATE CONSTRAINT TRIGGER recovery_fault AFTER UPDATE ON pets
         DEFERRABLE INITIALLY DEFERRED FOR EACH ROW
         WHEN (OLD.id='recovery-fixture' AND OLD.status='pending' AND NEW.status='active')
         EXECUTE FUNCTION fail_recovery()",
    )
    .await
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn deletion_update_error_keeps_deleting_until_database_is_repaired() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let serving = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &serving);
    let bob = stack.login(&client, &serving, "bob").await?;
    let db = PgPoolOptions::new()
        .max_connections(1)
        .connect(stack.database_url())
        .await?;
    stack.sql(DELETING).await?;
    install_database_fault(
        &stack,
        "CREATE TRIGGER recovery_fault BEFORE UPDATE ON pets FOR EACH ROW
         WHEN (OLD.id='dog' AND OLD.status='deleting' AND NEW.status='deleted')
         EXECUTE FUNCTION fail_recovery()",
    )
    .await?;
    stack
        .assert_startup_fails(&stack.fga_url, DATABASE_ERROR)
        .await?;
    assert_status(&db, "dog", "deleting").await?;
    assert_hidden(&api, "dog", &bob).await?;
    stack.assert_no_tuples(&client, "dog").await?;
    remove_database_fault(&stack).await?;
    assert_deletion_replays_safely(&stack, &client, &db, &bob).await?;
    serving.stop().await?;
    db.close().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn missing_recovery_table_prevents_readiness_without_losing_pending_rows() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let serving = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &serving);
    let carol = stack.login(&client, &serving, "carol").await?;
    let dave = stack.login(&client, &serving, "dave").await?;
    let db = PgPoolOptions::new()
        .max_connections(1)
        .connect(stack.database_url())
        .await?;
    stack.sql(PENDING).await?;
    // Migrations have already run, and the failed child skips demo seeding.
    // Renaming only this run-owned table makes recovery's first SELECT fail.
    stack
        .sql("ALTER TABLE pets RENAME TO unavailable_pets")
        .await?;
    stack.assert_startup_fails(&stack.fga_url, "42P01").await?;
    stack
        .sql("ALTER TABLE unavailable_pets RENAME TO pets")
        .await?;
    assert_status(&db, "recovery-fixture", "pending").await?;
    stack.assert_no_tuples(&client, "recovery-fixture").await?;
    assert_hidden(&api, "recovery-fixture", &carol).await?;
    assert_creation_replays_safely(&stack, &client, &db, &carol, &dave).await?;
    serving.stop().await?;
    db.close().await;
    Ok(())
}
