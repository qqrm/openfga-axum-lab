use crate::{
    api::{Api, expect},
    stack::{App, Stack, TestResult, client},
};
use reqwest::{Client, Method as M, StatusCode as S};
use serde_json::{Value, json};
use sqlx::PgPool;

const TABLES: &str = "/projects/acme/warehouses/analytics/tables";
const TABLE: &str = "/projects/acme/warehouses/analytics/tables/requests";
const OWNER: &str = "/projects/acme/warehouses/analytics/tables/requests/owner";
const GRANT: &str = "/projects/acme/warehouses/analytics/tables/requests/grants/select";
// Installed only in this test's disposable database, never in migrations.
const REJECT_WRITE: &str = r"
    CREATE FUNCTION reject_test_write() RETURNS trigger LANGUAGE plpgsql AS $$
    BEGIN
        RAISE EXCEPTION 'injected database write failure' USING ERRCODE = '58000';
    END;
    $$;
";

async fn session_count(db: &PgPool) -> TestResult<i64> {
    let count = sqlx::query_scalar("SELECT count(*) FROM sessions")
        .fetch_one(db)
        .await?;
    Ok(count)
}

async fn login_unavailable(stack: &Stack, client: &Client, app: &App, fault: &str) -> TestResult {
    let error = match stack.login(client, app, "alice").await {
        Err(error) => error,
        Ok(_) => return Err(format!("{fault}: login unexpectedly issued a session").into()),
    };
    let status = error
        .downcast_ref::<reqwest::Error>()
        .and_then(reqwest::Error::status);
    assert_eq!(status, Some(S::SERVICE_UNAVAILABLE), "{fault}");
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn session_database_errors_fail_closed_and_recover() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    let db = stack.pool().await?;
    let alice = stack.login(&client, &app, "alice").await?;
    expect(api.call(M::GET, TABLE, &alice), S::OK).await?;
    sqlx::raw_sql(REJECT_WRITE).execute(&db).await?;

    // Each fault reaches a different database boundary with valid credentials.
    // Repair it before the next fault, and retain the original session throughout.
    for (name, fault, repair) in [
        (
            "login attempt accounting",
            "CREATE TRIGGER reject_attempt BEFORE INSERT ON login_attempts
             FOR EACH ROW EXECUTE FUNCTION reject_test_write()",
            "DROP TRIGGER reject_attempt ON login_attempts",
        ),
        (
            "password lookup",
            "ALTER TABLE users RENAME COLUMN password_hash TO unavailable_password_hash",
            "ALTER TABLE users RENAME COLUMN unavailable_password_hash TO password_hash",
        ),
        (
            "session creation",
            "CREATE TRIGGER reject_session BEFORE INSERT ON sessions
             FOR EACH ROW EXECUTE FUNCTION reject_test_write()",
            "DROP TRIGGER reject_session ON sessions",
        ),
    ] {
        sqlx::raw_sql(fault).execute(&db).await?;
        login_unavailable(&stack, &client, &app, name).await?;
        assert_eq!(session_count(&db).await?, 1, "{name} created a session");
        expect(api.call(M::GET, TABLE, &alice), S::OK).await?;
        sqlx::raw_sql(repair).execute(&db).await?;
        let recovered = stack.login(&client, &app, "alice").await?;
        expect(api.call(M::GET, TABLE, &recovered), S::OK).await?;
        expect(api.call(M::DELETE, "/session", &recovered), S::NO_CONTENT).await?;
        assert_eq!(session_count(&db).await?, 1);
    }

    // A live token must not become a 401 or an authorized request when its
    // database lookup fails. DDL also invalidates PostgreSQL's prepared plans.
    sqlx::raw_sql("ALTER TABLE sessions RENAME COLUMN expires_at TO unavailable_expires_at")
        .execute(&db)
        .await?;
    expect(api.call(M::GET, TABLE, &alice), S::SERVICE_UNAVAILABLE).await?;
    assert_eq!(session_count(&db).await?, 1);
    sqlx::raw_sql("ALTER TABLE sessions RENAME COLUMN unavailable_expires_at TO expires_at")
        .execute(&db)
        .await?;
    expect(api.call(M::GET, TABLE, &alice), S::OK).await?;

    sqlx::raw_sql(
        "CREATE TRIGGER reject_logout BEFORE DELETE ON sessions
         FOR EACH ROW EXECUTE FUNCTION reject_test_write()",
    )
    .execute(&db)
    .await?;
    expect(
        api.call(M::DELETE, "/session", &alice),
        S::SERVICE_UNAVAILABLE,
    )
    .await?;
    assert_eq!(
        session_count(&db).await?,
        1,
        "failed logout deleted a session"
    );
    expect(api.call(M::GET, TABLE, &alice), S::OK).await?;
    sqlx::raw_sql("DROP TRIGGER reject_logout ON sessions; DROP FUNCTION reject_test_write()")
        .execute(&db)
        .await?;
    expect(api.call(M::DELETE, "/session", &alice), S::NO_CONTENT).await?;
    assert_eq!(session_count(&db).await?, 0);
    expect(api.call(M::GET, TABLE, &alice), S::UNAUTHORIZED).await?;
    db.close().await;
    app.stop().await?;
    Ok(())
}

async fn revision(db: &PgPool) -> TestResult<(i64, i64)> {
    Ok(
        sqlx::query_as("SELECT generation,pending FROM authz_cache_state WHERE singleton")
            .fetch_one(db)
            .await?,
    )
}

async fn fga_can_read(stack: &Stack, client: &Client, user: &str) -> TestResult<bool> {
    let config = stack.config()?;
    let store = config["store_id"].as_str().ok_or("missing store id")?;
    let result: Value = client
        .post(format!("{}/stores/{store}/check", stack.fga_http_url))
        .json(&json!({"authorization_model_id": config["model_id"],
        "consistency": "HIGHER_CONSISTENCY", "tuple_key": {
            "user": format!("user:{user}"), "relation": "can_get_metadata", "object": "lakekeeper_table:analytics/requests"
        }}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    result["allowed"]
        .as_bool()
        .ok_or("missing FGA decision".into())
}

async fn fenced(client: &Client, apps: &[&App], token: &str) -> TestResult {
    for app in apps {
        let api = Api::new(client.clone(), app);
        for path in [TABLE, TABLES, "/catalog"] {
            expect(api.call(M::GET, path, token), S::SERVICE_UNAVAILABLE).await?;
        }
        expect(api.request(M::GET, "/readyz"), S::SERVICE_UNAVAILABLE).await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn missing_coordinator_row_fails_closed_without_disabling_sessions() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let first = stack.app(&stack.fga_url).await?;
    let second = stack.app(&stack.fga_url).await?;
    let db = stack.pool().await?;
    let carol = stack.login(&client, &first, "carol").await?;
    let initial = revision(&db).await?;
    assert_eq!(initial.1, 0);
    for app in [&first, &second] {
        let api = Api::new(client.clone(), app);
        for _ in 0..2 {
            expect(api.call(M::GET, TABLE, &carol), S::OK).await?;
        }
    }
    // Remove only the cache coordinator: credentials and catalog SQL remain live.
    sqlx::query("DELETE FROM authz_cache_state WHERE singleton")
        .execute(&db)
        .await?;
    fenced(&client, &[&first, &second], &carol).await?;
    let healthy = stack.login(&client, &first, "alice").await?;
    expect(
        Api::new(client.clone(), &first).call(M::DELETE, "/session", &healthy),
        S::NO_CONTENT,
    )
    .await?;
    // Only reads and session operations occurred while this row was absent.
    // Restoring the exact fixture is not reconciliation of an uncertain write.
    sqlx::query("INSERT INTO authz_cache_state(generation,pending) VALUES ($1,$2)")
        .bind(initial.0)
        .bind(initial.1)
        .execute(&db)
        .await?;
    for app in [&first, &second] {
        expect(
            Api::new(client.clone(), app).call(M::GET, TABLE, &carol),
            S::OK,
        )
        .await?;
    }
    db.close().await;
    first.stop().await?;
    second.stop().await
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn failed_write_marker_preserves_intent_without_touching_openfga() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let first = stack.app(&stack.fga_url).await?;
    let second = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &first);
    let db = stack.pool().await?;
    let carol = stack.login(&client, &first, "carol").await?;
    let dave = stack.login(&client, &second, "dave").await?;
    let initial = revision(&db).await?;
    assert_eq!(initial.1, 0, "fixture must begin without global debt");
    for app in [&first, &second] {
        let api = Api::new(client.clone(), app);
        expect(api.request(M::GET, "/readyz"), S::OK).await?;
        expect(api.call(M::GET, TABLE, &carol), S::OK).await?;
    }
    assert!(!fga_can_read(&stack, &client, "dave").await?);
    sqlx::raw_sql(REJECT_WRITE).execute(&db).await?;
    sqlx::raw_sql(
        "CREATE TRIGGER reject_begin BEFORE UPDATE ON authz_cache_state
        FOR EACH ROW WHEN (NEW.pending > OLD.pending) EXECUTE FUNCTION reject_test_write()",
    )
    .execute(&db)
    .await?;
    // Catalog grants have no desired-state SQL journal. Ownership replacement
    // commits its intent and hidden lifecycle before entering CachedAuthorizer.
    expect(
        api.call(M::PUT, OWNER, &carol)
            .json(&json!({"owner_id":"dave"})),
        S::SERVICE_UNAVAILABLE,
    )
    .await?;
    assert_eq!(
        revision(&db).await?,
        initial,
        "failed marker must roll back"
    );
    assert!(
        !fga_can_read(&stack, &client, "dave").await?,
        "no remote ownership write may precede its marker"
    );
    assert!(fga_can_read(&stack, &client, "carol").await?);
    let staged: (String, String, String, String, String) = sqlx::query_as(
        "SELECT t.status,t.owner_id,u.owner_id,u.writes::text,u.deletes::text
        FROM catalog_tables t JOIN catalog_pending_updates u
        ON u.project_id=t.project_id AND u.warehouse_id=t.warehouse_id AND u.table_id=t.id
        WHERE t.project_id='acme' AND t.warehouse_id='analytics' AND t.id='requests'",
    )
    .fetch_one(&db)
    .await?;
    assert_eq!(staged.0, "updating");
    assert_eq!(
        staged.1, "carol",
        "SQL ownership changes only after acknowledgement"
    );
    assert_eq!(
        staged.2, "dave",
        "the durable replacement intent must survive"
    );
    assert_eq!(
        serde_json::from_str::<Value>(&staged.3)?,
        json!([{
            "user":"user:dave", "relation":"ownership", "object":"lakekeeper_table:analytics/requests"
        }])
    );
    assert_eq!(
        serde_json::from_str::<Value>(&staged.4)?,
        json!([{
            "user":"user:carol", "relation":"ownership", "object":"lakekeeper_table:analytics/requests"
        }])
    );
    for app in [&first, &second] {
        let api = Api::new(client.clone(), app);
        // The lifecycle hides this object before either old or new permissions
        // can disclose it; no global marker means unrelated reads still work.
        for token in [&carol, &dave] {
            expect(api.call(M::GET, TABLE, token), S::NOT_FOUND).await?;
        }
        let page = expect(api.call(M::GET, TABLES, &dave), S::OK).await?;
        let items = page["items"].as_array().ok_or("missing catalog page")?;
        assert!(items.iter().all(|table| table["id"] != "requests"));
        assert!(items.iter().any(|table| table["id"] == "latency"));
        expect(api.call(M::GET, &format!("{TABLES}/latency"), &dave), S::OK).await?;
        expect(api.request(M::GET, "/readyz"), S::SERVICE_UNAVAILABLE).await?;
    }
    stack
        .assert_startup_fails(
            &stack.fga_url,
            "pending catalog relationship replacement failed",
        )
        .await?;
    assert_eq!(revision(&db).await?, initial);
    assert!(!fga_can_read(&stack, &client, "dave").await?);
    sqlx::raw_sql(
        "DROP TRIGGER reject_begin ON authz_cache_state; DROP FUNCTION reject_test_write()",
    )
    .execute(&db)
    .await?;
    // A new public operation cannot bypass the staged table lifecycle.
    expect(
        api.call(M::PUT, OWNER, &carol)
            .json(&json!({"owner_id":"dave"})),
        S::NOT_FOUND,
    )
    .await?;
    first.stop().await?;
    second.stop().await?;
    // The failed SQL marker prevented polling the remote operation. With no
    // uncertainty debt, ordinary startup can replay the original durable intent.
    // No blanket reset or special recovery command participates in this test.
    let restored = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &restored);
    expect(api.request(M::GET, "/readyz"), S::OK).await?;
    assert!(fga_can_read(&stack, &client, "dave").await?);
    assert!(!fga_can_read(&stack, &client, "carol").await?);
    let table = expect(api.call(M::GET, TABLE, &dave), S::OK).await?;
    assert_eq!(table["owner_id"], "dave");
    expect(api.call(M::GET, TABLE, &carol), S::FORBIDDEN).await?;
    let intents: i64 = sqlx::query_scalar("SELECT count(*) FROM catalog_pending_updates")
        .fetch_one(&db)
        .await?;
    assert_eq!(intents, 0);
    let recovered = revision(&db).await?;
    assert!(recovered.0 > initial.0);
    assert_eq!(recovered.1, 0);
    expect(
        api.call(M::PUT, OWNER, &dave)
            .json(&json!({"owner_id":"carol"})),
        S::NO_CONTENT,
    )
    .await?;
    expect(api.call(M::GET, TABLE, &carol), S::OK).await?;
    expect(api.call(M::GET, TABLE, &dave), S::FORBIDDEN).await?;

    // A completed deletion leaves a tombstone, not unfinished work. Exercise
    // the real API so readiness cannot accidentally reject all non-active rows.
    let alice = stack.login(&client, &restored, "alice").await?;
    let created = expect(
        api.call(M::POST, TABLES, &alice)
            .header("Idempotency-Key", uuid::Uuid::new_v4().to_string())
            .json(&json!({"namespace_id":"metrics","name":"Readiness tombstone","description":""})),
        S::CREATED,
    )
    .await?;
    let id = created["id"].as_str().ok_or("missing created table")?;
    expect(
        api.call(M::DELETE, &format!("{TABLES}/{id}"), &alice),
        S::NO_CONTENT,
    )
    .await?;
    let status: String = sqlx::query_scalar(
        "SELECT status FROM catalog_tables WHERE project_id='acme' AND warehouse_id='analytics' AND id=$1",
    )
    .bind(id)
    .fetch_one(&db)
    .await?;
    assert_eq!(status, "deleted");
    expect(api.request(M::GET, "/readyz"), S::OK).await?;

    // An orphaned intent must independently block readiness, even with a
    // terminal row. This synthetic, never-dispatched fixture has empty writes;
    // removing it is fixture cleanup, not reconciliation of remote uncertainty.
    let before_fixture = revision(&db).await?;
    sqlx::query(
        "INSERT INTO catalog_pending_updates(project_id,warehouse_id,table_id,actor_id,operation,owner_id,namespace_id,writes,deletes)
        VALUES('acme','analytics',$1,'alice','owner','alice','metrics','[]'::jsonb,'[]'::jsonb)",
    )
    .bind(id)
    .execute(&db)
    .await?;
    expect(api.request(M::GET, "/readyz"), S::SERVICE_UNAVAILABLE).await?;
    let cleaned = sqlx::query(
        "DELETE FROM catalog_pending_updates WHERE project_id='acme' AND warehouse_id='analytics' AND table_id=$1",
    )
    .bind(id)
    .execute(&db)
    .await?;
    assert_eq!(cleaned.rows_affected(), 1);
    assert_eq!(revision(&db).await?, before_fixture);
    expect(api.request(M::GET, "/readyz"), S::OK).await?;
    db.close().await;
    restored.stop().await
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn failed_finalization_retains_a_read_fence_after_repair_and_restart() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let first = stack.app(&stack.fga_url).await?;
    let second = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &first);
    let db = stack.pool().await?;
    let carol = stack.login(&client, &first, "carol").await?;
    let dave = stack.login(&client, &second, "dave").await?;
    let subject = json!({"subject":{"type":"user","id":"dave"}});
    expect(
        api.call(M::PUT, GRANT, &carol).json(&subject),
        S::NO_CONTENT,
    )
    .await?;
    let granted = revision(&db).await?;
    assert_eq!(granted.1, 0);
    assert!(fga_can_read(&stack, &client, "dave").await?);
    for app in [&first, &second] {
        let api = Api::new(client.clone(), app);
        for _ in 0..2 {
            expect(api.call(M::GET, TABLE, &dave), S::OK).await?;
        }
    }
    sqlx::raw_sql(REJECT_WRITE).execute(&db).await?;
    sqlx::raw_sql(
        "CREATE TRIGGER reject_finalize BEFORE UPDATE ON authz_cache_state
        FOR EACH ROW WHEN (NEW.pending < OLD.pending) EXECUTE FUNCTION reject_test_write()",
    )
    .execute(&db)
    .await?;
    expect(
        api.call(M::DELETE, GRANT, &carol).json(&subject),
        S::SERVICE_UNAVAILABLE,
    )
    .await?;
    assert!(
        !fga_can_read(&stack, &client, "dave").await?,
        "the real revoke must have committed"
    );
    let uncertain = revision(&db).await?;
    assert_eq!(uncertain, (granted.0 + 1, 1));
    // Direct grants are authoritative in OpenFGA, not journaled as catalog
    // replacement intents. Absence of an intent cannot discharge global debt.
    let pending_intents: i64 = sqlx::query_scalar("SELECT count(*) FROM catalog_pending_updates")
        .fetch_one(&db)
        .await?;
    assert_eq!(pending_intents, 0);
    fenced(&client, &[&first, &second], &dave).await?;
    sqlx::raw_sql(
        "DROP TRIGGER reject_finalize ON authz_cache_state; DROP FUNCTION reject_test_write()",
    )
    .execute(&db)
    .await?;
    second.stop().await?;
    let restarted = stack.app(&stack.fga_url).await?;
    assert_eq!(
        revision(&db).await?,
        uncertain,
        "database repair and startup must preserve unresolved authorization debt"
    );
    fenced(&client, &[&first, &restarted], &dave).await?;
    for app in [&first, &restarted] {
        let api = Api::new(client.clone(), app);
        expect(api.call(M::GET, TABLE, &carol), S::SERVICE_UNAVAILABLE).await?;
        expect(
            api.call(M::PUT, GRANT, &carol).json(&subject),
            S::SERVICE_UNAVAILABLE,
        )
        .await?;
    }
    // Positive controls distinguish the durable fence from a backend outage:
    // OpenFGA and sessions are healthy, but neither can reopen protected reads.
    assert!(fga_can_read(&stack, &client, "carol").await?);
    assert!(!fga_can_read(&stack, &client, "dave").await?);
    let healthy = stack.login(&client, &restarted, "alice").await?;
    expect(
        Api::new(client.clone(), &restarted).call(M::DELETE, "/session", &healthy),
        S::NO_CONTENT,
    )
    .await?;
    assert_eq!(revision(&db).await?, uncertain);
    db.close().await;
    first.stop().await?;
    restarted.stop().await
}
