//! Recovery crosses two durable systems. A known SQL failure after a complete
//! OpenFGA acknowledgement can replay its hidden lifecycle row; an ambiguous
//! remote result retains independent global debt even after that replay.
use crate::{
    api::{Api, expect},
    proxy::Proxy,
    stack::{Stack, TestResult, client},
};
use reqwest::{Client, Method as M, StatusCode as S};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::BTreeSet;

const TABLES: &str = "/projects/acme/warehouses/analytics/tables";
const CONTROL: &str = "/projects/acme/warehouses/analytics/tables/requests";
const PENDING: &str = include_str!("../../scripts/recovery-fixture.sql");
const DELETING: &str = include_str!("../../scripts/deletion-fixture.sql");
// Startup stderr identifies the injected SQL failure; public API errors remain opaque.
const DATABASE_ERROR: &str = "injected recovery database failure";

async fn assert_status(db: &PgPool, id: &str, expected: &str) -> TestResult {
    let status: String = sqlx::query_scalar(
        "SELECT status FROM catalog_tables WHERE project_id='acme' AND warehouse_id='analytics' AND id=$1",
    )
    .bind(id)
    .fetch_one(db)
    .await?;
    assert_eq!(status, expected, "unexpected durable state for {id}");
    Ok(())
}

async fn pending(db: &PgPool) -> TestResult<i64> {
    Ok(
        sqlx::query_scalar("SELECT pending FROM authz_cache_state WHERE singleton")
            .fetch_one(db)
            .await?,
    )
}

async fn assert_not_found(api: &Api, id: &str, token: &str) -> TestResult {
    expect(
        api.call(M::GET, &format!("{TABLES}/{id}"), token),
        S::NOT_FOUND,
    )
    .await?;
    Ok(())
}

async fn assert_hidden(api: &Api, id: &str, token: &str) -> TestResult {
    assert_not_found(api, id, token).await?;
    let visible = expect(api.call(M::GET, TABLES, token), S::OK).await?;
    assert!(
        visible["items"]
            .as_array()
            .ok_or("expected catalog table page")?
            .iter()
            .all(|table| table["id"] != id),
        "unfinished recovery exposed {id} in the table list"
    );
    assert!(
        visible["next_cursor"].is_null(),
        "fixture list must be complete"
    );
    Ok(())
}

async fn assert_fenced(api: &Api, token: &str) -> TestResult {
    // Hidden lifecycle rows can return 404 before an authorization Check. An
    // unrelated active table and enumeration demonstrate the global fence.
    expect(api.call(M::GET, CONTROL, token), S::SERVICE_UNAVAILABLE).await?;
    expect(api.call(M::GET, TABLES, token), S::SERVICE_UNAVAILABLE).await?;
    expect(api.request(M::GET, "/readyz"), S::SERVICE_UNAVAILABLE).await?;
    Ok(())
}

fn tuple(user: &str, relation: &str, object: &str) -> (String, String, String) {
    (user.to_owned(), relation.to_owned(), object.to_owned())
}

async fn table_tuples(
    stack: &Stack,
    client: &Client,
    id: &str,
) -> TestResult<BTreeSet<(String, String, String)>> {
    let config = stack.config()?;
    let store = config["store_id"].as_str().ok_or("missing store id")?;
    let object = format!("lakekeeper_table:analytics/{id}");
    let mut cursor = String::new();
    let mut seen = BTreeSet::new();
    let mut tuples = BTreeSet::new();
    loop {
        let page: Value = client
            .post(format!("{}/stores/{store}/read", stack.fga_http_url))
            .json(&json!({
                "page_size": 100,
                "continuation_token": cursor,
                "consistency": "HIGHER_CONSISTENCY"
            }))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        for entry in page["tuples"].as_array().ok_or("missing tuples")? {
            let key = &entry["key"];
            let user = key["user"].as_str().ok_or("missing tuple user")?;
            let relation = key["relation"].as_str().ok_or("missing tuple relation")?;
            let target = key["object"].as_str().ok_or("missing tuple object")?;
            if target == object || user == object {
                assert!(key["condition"].is_null(), "fixture must be unconditional");
                assert!(
                    tuples.insert(tuple(user, relation, target)),
                    "duplicate tuple"
                );
            }
        }
        cursor = page["continuation_token"].as_str().unwrap_or("").to_owned();
        if cursor.is_empty() {
            return Ok(tuples);
        }
        assert!(seen.insert(cursor.clone()), "repeating tuple cursor");
    }
}

async fn assert_creation_tuples(stack: &Stack, client: &Client) -> TestResult {
    let object = "lakekeeper_table:analytics/recovery-fixture";
    assert_eq!(
        table_tuples(stack, client, "recovery-fixture").await?,
        BTreeSet::from([
            tuple("user:carol", "ownership", object),
            tuple("namespace:metrics", "parent", object),
            tuple(object, "child", "namespace:metrics"),
        ]),
        "creation must have exactly its owner, parent and inverse child tuples"
    );
    Ok(())
}

async fn lose_recovery_response(stack: &Stack, proxy: &Proxy, error: &str) -> TestResult {
    let acknowledged = proxy.acknowledged_writes();
    let mut pause = proxy.pause("write", true).await;
    let release = async {
        // The actual upstream transaction succeeded. Return 502 to the child
        // instead of its acknowledgement, without killing its coverage profile.
        pause.captured().await?;
        drop(pause);
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    };
    let (startup, captured) = tokio::join!(stack.assert_startup_fails(&proxy.url, error), release);
    captured?;
    startup?;
    proxy.wait_idle().await?;
    assert_eq!(
        proxy.acknowledged_writes(),
        acknowledged + 1,
        "fault must hide exactly one completed upstream Write"
    );
    Ok(())
}

async fn assert_creation_replays_safely(
    stack: &Stack,
    client: &Client,
    db: &PgPool,
    carol: &str,
    dave: &str,
    debt: i64,
) -> TestResult {
    let recovered = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &recovered);
    assert_status(db, "recovery-fixture", "active").await?;
    assert_creation_tuples(stack, client).await?;
    assert_eq!(
        pending(db).await?,
        debt,
        "replay must preserve previous debt"
    );
    if debt > 0 {
        assert_fenced(&api, carol).await?;
        assert_fenced(&api, dave).await?;
        expect(
            api.call(M::GET, &format!("{TABLES}/recovery-fixture"), carol),
            S::SERVICE_UNAVAILABLE,
        )
        .await?;
        expect(
            api.call(M::DELETE, &format!("{TABLES}/recovery-fixture"), carol),
            S::SERVICE_UNAVAILABLE,
        )
        .await?;
        recovered.stop().await?;
        // A second ordinary startup has no remaining intent. That fact still
        // cannot prove that an older remote request is gone or clear its fence.
        let restarted = stack.app(&stack.fga_url).await?;
        assert_eq!(pending(db).await?, debt);
        assert_status(db, "recovery-fixture", "active").await?;
        assert_creation_tuples(stack, client).await?;
        assert_fenced(&Api::new(client.clone(), &restarted), carol).await?;
        return restarted.stop().await;
    }

    expect(api.request(M::GET, "/readyz"), S::OK).await?;
    let table = expect(
        api.call(M::GET, &format!("{TABLES}/recovery-fixture"), carol),
        S::OK,
    )
    .await?;
    assert_eq!(table["id"], "recovery-fixture");
    assert_eq!(table["name"], "Recovered observations");
    assert_eq!(table["owner_id"], "carol");
    expect(
        api.call(M::GET, &format!("{TABLES}/recovery-fixture"), dave),
        S::FORBIDDEN,
    )
    .await?;
    expect(
        api.call(M::DELETE, &format!("{TABLES}/recovery-fixture"), carol),
        S::NO_CONTENT,
    )
    .await?;
    assert_status(db, "recovery-fixture", "deleted").await?;
    stack.assert_no_tuples(client, "recovery-fixture").await?;
    assert_eq!(pending(db).await?, 0);
    expect(api.request(M::GET, "/readyz"), S::OK).await?;
    recovered.stop().await
}

async fn assert_deletion_replays_safely(
    stack: &Stack,
    client: &Client,
    db: &PgPool,
    bob: &str,
    debt: i64,
) -> TestResult {
    let recovered = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &recovered);
    assert_status(db, "errors", "deleted").await?;
    stack.assert_no_tuples(client, "errors").await?;
    assert_eq!(
        pending(db).await?,
        debt,
        "replay must preserve previous debt"
    );
    assert_not_found(&api, "errors", bob).await?;
    if debt > 0 {
        assert_fenced(&api, bob).await?;
        expect(
            api.call(M::DELETE, &format!("{TABLES}/errors"), bob),
            S::SERVICE_UNAVAILABLE,
        )
        .await?;
        recovered.stop().await?;
        let restarted = stack.app(&stack.fga_url).await?;
        assert_eq!(pending(db).await?, debt);
        assert_status(db, "errors", "deleted").await?;
        stack.assert_no_tuples(client, "errors").await?;
        assert_fenced(&Api::new(client.clone(), &restarted), bob).await?;
        return restarted.stop().await;
    }

    assert_hidden(&api, "errors", bob).await?;
    expect(api.request(M::GET, "/readyz"), S::OK).await?;
    expect(
        api.call(M::DELETE, &format!("{TABLES}/errors"), bob),
        S::NO_CONTENT,
    )
    .await?;
    stack.assert_no_tuples(client, "errors").await?;
    assert_eq!(pending(db).await?, 0);
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
    let db = stack.pool().await?;
    assert_eq!(pending(&db).await?, 0);
    stack.sql(PENDING).await?;
    let proxy = Proxy::start(client.clone(), stack.fga_url.clone()).await?;
    lose_recovery_response(&stack, &proxy, "pending catalog provisioning failed").await?;
    assert_status(&db, "recovery-fixture", "pending").await?;
    assert_creation_tuples(&stack, &client).await?;
    assert_eq!(pending(&db).await?, 1);
    for token in [&carol, &dave] {
        assert_not_found(&api, "recovery-fixture", token).await?;
        assert_fenced(&api, token).await?;
    }
    serving.stop().await?;
    assert_creation_replays_safely(&stack, &client, &db, &carol, &dave, 1).await?;
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
    let db = stack.pool().await?;
    assert_eq!(pending(&db).await?, 0);
    stack.sql(DELETING).await?;
    let proxy = Proxy::start(client.clone(), stack.fga_url.clone()).await?;
    lose_recovery_response(&stack, &proxy, "pending catalog deletion failed").await?;
    assert_status(&db, "errors", "deleting").await?;
    assert_eq!(pending(&db).await?, 1);
    assert_not_found(&api, "errors", &bob).await?;
    assert_fenced(&api, &bob).await?;
    // Direct tuples were removed, but the separate inverse-link cleanup has
    // not run because the first Write's response was lost.
    let remaining = table_tuples(&stack, &client, "errors").await?;
    assert_eq!(
        remaining,
        BTreeSet::from([tuple(
            "lakekeeper_table:analytics/errors",
            "child",
            "namespace:metrics",
        )])
    );
    let before_retry: (i64, i64) =
        sqlx::query_as("SELECT generation,pending FROM authz_cache_state WHERE singleton")
            .fetch_one(&db)
            .await?;
    // Public retries are not the startup recovery boundary. Even the recorded
    // delete actor cannot continue remote cleanup while uncertainty is fenced.
    expect(
        api.call(M::DELETE, &format!("{TABLES}/errors"), &bob),
        S::SERVICE_UNAVAILABLE,
    )
    .await?;
    assert_status(&db, "errors", "deleting").await?;
    assert_eq!(table_tuples(&stack, &client, "errors").await?, remaining);
    let after_retry: (i64, i64) =
        sqlx::query_as("SELECT generation,pending FROM authz_cache_state WHERE singleton")
            .fetch_one(&db)
            .await?;
    assert_eq!(
        after_retry, before_retry,
        "a fenced retry must not begin a write"
    );
    serving.stop().await?;
    assert_deletion_replays_safely(&stack, &client, &db, &bob, 1).await?;
    db.close().await;
    Ok(())
}

async fn install_database_fault(stack: &Stack, trigger: &'static str) -> TestResult {
    stack
        .sql(
            "CREATE FUNCTION fail_recovery() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN RAISE EXCEPTION 'injected recovery database failure'; END; $$",
        )
        .await?;
    stack.sql(trigger).await
}

async fn remove_database_fault(stack: &Stack) -> TestResult {
    stack
        .sql("DROP TRIGGER recovery_fault ON catalog_tables")
        .await?;
    stack.sql("DROP FUNCTION fail_recovery()").await
}

async fn failed_activation_keeps_pending(trigger: &'static str) -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let serving = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &serving);
    let carol = stack.login(&client, &serving, "carol").await?;
    let dave = stack.login(&client, &serving, "dave").await?;
    let db = stack.pool().await?;
    stack.sql(PENDING).await?;
    install_database_fault(&stack, trigger).await?;
    stack
        .assert_startup_fails(&stack.fga_url, DATABASE_ERROR)
        .await?;
    assert_status(&db, "recovery-fixture", "pending").await?;
    assert_creation_tuples(&stack, &client).await?;
    assert_eq!(pending(&db).await?, 0, "the remote Write was acknowledged");
    assert_hidden(&api, "recovery-fixture", &carol).await?;
    assert_hidden(&api, "recovery-fixture", &dave).await?;
    expect(api.call(M::GET, CONTROL, &carol), S::OK).await?;
    expect(api.request(M::GET, "/readyz"), S::SERVICE_UNAVAILABLE).await?;
    remove_database_fault(&stack).await?;
    serving.stop().await?;
    assert_creation_replays_safely(&stack, &client, &db, &carol, &dave, 0).await?;
    db.close().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn activation_update_error_keeps_pending_until_database_is_repaired() -> TestResult {
    failed_activation_keeps_pending(
        "CREATE TRIGGER recovery_fault BEFORE UPDATE ON catalog_tables FOR EACH ROW
         WHEN (OLD.project_id='acme' AND OLD.warehouse_id='analytics' AND OLD.id='recovery-fixture'
               AND OLD.status='pending' AND NEW.status='active')
         EXECUTE FUNCTION fail_recovery()",
    )
    .await
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn activation_commit_error_keeps_pending_until_database_is_repaired() -> TestResult {
    failed_activation_keeps_pending(
        "CREATE CONSTRAINT TRIGGER recovery_fault AFTER UPDATE ON catalog_tables
         DEFERRABLE INITIALLY DEFERRED FOR EACH ROW
         WHEN (OLD.project_id='acme' AND OLD.warehouse_id='analytics' AND OLD.id='recovery-fixture'
               AND OLD.status='pending' AND NEW.status='active')
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
    let db = stack.pool().await?;
    stack.sql(DELETING).await?;
    install_database_fault(
        &stack,
        "CREATE TRIGGER recovery_fault BEFORE UPDATE ON catalog_tables FOR EACH ROW
         WHEN (OLD.project_id='acme' AND OLD.warehouse_id='analytics' AND OLD.id='errors'
               AND OLD.status='deleting' AND NEW.status='deleted')
         EXECUTE FUNCTION fail_recovery()",
    )
    .await?;
    stack
        .assert_startup_fails(&stack.fga_url, DATABASE_ERROR)
        .await?;
    assert_status(&db, "errors", "deleting").await?;
    assert_eq!(
        pending(&db).await?,
        0,
        "both deletion writes were acknowledged"
    );
    assert_hidden(&api, "errors", &bob).await?;
    expect(api.call(M::GET, CONTROL, &bob), S::OK).await?;
    expect(api.request(M::GET, "/readyz"), S::SERVICE_UNAVAILABLE).await?;
    stack.assert_no_tuples(&client, "errors").await?;
    remove_database_fault(&stack).await?;
    serving.stop().await?;
    assert_deletion_replays_safely(&stack, &client, &db, &bob, 0).await?;
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
    let db = stack.pool().await?;
    stack.sql(PENDING).await?;
    // Migrations already ran; the failed child skips seeding and migration.
    // Renaming this disposable table makes lifecycle discovery itself fail.
    stack
        .sql("ALTER TABLE catalog_tables RENAME TO unavailable_catalog_tables")
        .await?;
    expect(api.request(M::GET, "/readyz"), S::SERVICE_UNAVAILABLE).await?;
    stack
        .assert_startup_fails(&stack.fga_url, "catalog_tables")
        .await?;
    stack
        .sql("ALTER TABLE unavailable_catalog_tables RENAME TO catalog_tables")
        .await?;
    assert_status(&db, "recovery-fixture", "pending").await?;
    assert_eq!(pending(&db).await?, 0);
    stack.assert_no_tuples(&client, "recovery-fixture").await?;
    assert_hidden(&api, "recovery-fixture", &carol).await?;
    assert_hidden(&api, "recovery-fixture", &dave).await?;
    expect(api.request(M::GET, "/readyz"), S::SERVICE_UNAVAILABLE).await?;
    serving.stop().await?;
    assert_creation_replays_safely(&stack, &client, &db, &carol, &dave, 0).await?;
    db.close().await;
    Ok(())
}
