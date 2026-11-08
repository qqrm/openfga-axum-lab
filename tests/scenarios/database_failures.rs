use crate::{
    api::{Api, expect},
    stack::{App, Stack, TestResult, client},
};
use reqwest::{Client, Method as M, StatusCode as S};
use serde_json::{Value, json};
use sqlx::PgPool;

const TASK: &str = "/tasks/access-review";
const RELATION: &str = "/tasks/access-review/relations/viewer/dave";
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
    expect(api.call(M::GET, TASK, &alice), S::OK).await?;
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
        expect(api.call(M::GET, TASK, &alice), S::OK).await?;
        sqlx::raw_sql(repair).execute(&db).await?;
        let recovered = stack.login(&client, &app, "alice").await?;
        expect(api.call(M::GET, TASK, &recovered), S::OK).await?;
        expect(api.call(M::DELETE, "/session", &recovered), S::NO_CONTENT).await?;
        assert_eq!(session_count(&db).await?, 1);
    }

    // A live token must not become a 401 or an authorized request when its
    // database lookup fails. DDL also invalidates PostgreSQL's prepared plans.
    sqlx::raw_sql("ALTER TABLE sessions RENAME COLUMN expires_at TO unavailable_expires_at")
        .execute(&db)
        .await?;
    expect(api.call(M::GET, TASK, &alice), S::SERVICE_UNAVAILABLE).await?;
    assert_eq!(session_count(&db).await?, 1);
    sqlx::raw_sql("ALTER TABLE sessions RENAME COLUMN unavailable_expires_at TO expires_at")
        .execute(&db)
        .await?;
    expect(api.call(M::GET, TASK, &alice), S::OK).await?;

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
    expect(api.call(M::GET, TASK, &alice), S::OK).await?;
    sqlx::raw_sql("DROP TRIGGER reject_logout ON sessions; DROP FUNCTION reject_test_write()")
        .execute(&db)
        .await?;
    expect(api.call(M::DELETE, "/session", &alice), S::NO_CONTENT).await?;
    assert_eq!(session_count(&db).await?, 0);
    expect(api.call(M::GET, TASK, &alice), S::UNAUTHORIZED).await?;
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

async fn fga_allows_dave(stack: &Stack, client: &Client) -> TestResult<bool> {
    let config = stack.config()?;
    let store = config["store_id"].as_str().ok_or("missing store id")?;
    let result: Value = client
        .post(format!("{}/stores/{store}/check", stack.fga_http_url))
        .json(&json!({"authorization_model_id": config["model_id"],
        "consistency": "HIGHER_CONSISTENCY", "tuple_key": {
            "user": "user:dave", "relation": "can_view", "object": "task:access-review"
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
        for path in [TASK, "/tasks", "/organizations"] {
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
            expect(api.call(M::GET, TASK, &carol), S::OK).await?;
        }
    }
    // Remove only the cache coordinator: credentials and task SQL remain live.
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
    sqlx::query("INSERT INTO authz_cache_state(generation,pending) VALUES ($1,$2)")
        .bind(initial.0)
        .bind(initial.1)
        .execute(&db)
        .await?;
    for app in [&first, &second] {
        expect(
            Api::new(client.clone(), app).call(M::GET, TASK, &carol),
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
    expect(api.request(M::GET, "/readyz"), S::OK).await?;
    assert!(!fga_allows_dave(&stack, &client).await?);
    sqlx::raw_sql(REJECT_WRITE).execute(&db).await?;
    sqlx::raw_sql(
        "CREATE TRIGGER reject_begin BEFORE UPDATE ON authz_cache_state
        FOR EACH ROW WHEN (NEW.pending > OLD.pending) EXECUTE FUNCTION reject_test_write()",
    )
    .execute(&db)
    .await?;
    expect(api.call(M::PUT, RELATION, &carol), S::SERVICE_UNAVAILABLE).await?;
    assert_eq!(
        revision(&db).await?,
        initial,
        "failed marker must roll back"
    );
    assert!(
        !fga_allows_dave(&stack, &client).await?,
        "no remote grant may precede its marker"
    );
    assert!(stack.authorization_pending("acme").await?);
    let attempted: bool =
        sqlx::query_scalar("SELECT attempted FROM authorization_jobs WHERE organization_id='acme'")
            .fetch_one(&db)
            .await?;
    assert!(
        attempted,
        "durable attempt records ambiguity before the remote boundary"
    );
    let desired: i64 = sqlx::query_scalar("SELECT count(*) FROM task_relations WHERE task_id='access-review' AND user_id='dave' AND relation='viewer'")
        .fetch_one(&db).await?;
    assert_eq!(desired, 1, "the committed SQL intent must not disappear");
    println!(
        "fault reproduced: global marker unchanged at {initial:?}; organization pending; attempted journal; desired grant present; OpenFGA grant absent"
    );
    fenced(&client, &[&first, &second], &dave).await?;
    sqlx::raw_sql(
        "DROP TRIGGER reject_begin ON authz_cache_state; DROP FUNCTION reject_test_write()",
    )
    .execute(&db)
    .await?;
    // A fresh API request cannot bypass the durable organization fence.
    expect(api.call(M::PUT, RELATION, &carol), S::SERVICE_UNAVAILABLE).await?;
    first.stop().await?;
    second.stop().await?;
    // All writers are stopped and the failed marker proved no remote request ran.
    stack.recover_after_backend_drain(&stack.fga_url).await?;
    let restored = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &restored);
    expect(api.request(M::GET, "/readyz"), S::OK).await?;
    assert!(fga_allows_dave(&stack, &client).await?);
    expect(api.call(M::GET, TASK, &dave), S::OK).await?;
    assert!(!stack.authorization_pending("acme").await?);
    let recovered = revision(&db).await?;
    assert!(recovered.0 > initial.0);
    assert_eq!(recovered.1, 0);
    expect(api.call(M::DELETE, RELATION, &carol), S::NO_CONTENT).await?;
    expect(api.call(M::GET, TASK, &dave), S::FORBIDDEN).await?;
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
    expect(api.call(M::PUT, RELATION, &carol), S::NO_CONTENT).await?;
    let granted = revision(&db).await?;
    assert_eq!(granted.1, 0);
    for app in [&first, &second] {
        let api = Api::new(client.clone(), app);
        for _ in 0..2 {
            expect(api.call(M::GET, TASK, &dave), S::OK).await?;
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
        api.call(M::DELETE, RELATION, &carol),
        S::SERVICE_UNAVAILABLE,
    )
    .await?;
    assert!(
        !fga_allows_dave(&stack, &client).await?,
        "the real revoke must have committed"
    );
    assert_eq!(revision(&db).await?, (granted.0 + 1, 1));
    assert!(stack.authorization_pending("acme").await?);
    fenced(&client, &[&first, &second], &dave).await?;
    sqlx::raw_sql(
        "DROP TRIGGER reject_finalize ON authz_cache_state; DROP FUNCTION reject_test_write()",
    )
    .execute(&db)
    .await?;
    second.stop().await?;
    let restarted = stack.app(&stack.fga_url).await?;
    let replayed = revision(&db).await?;
    assert!(
        replayed.0 > granted.0 && replayed.1 > 0,
        "repair/replay must retain uncertainty"
    );
    assert!(!stack.authorization_pending("acme").await?);
    fenced(&client, &[&first, &restarted], &dave).await?;
    expect(api.call(M::PUT, RELATION, &carol), S::SERVICE_UNAVAILABLE).await?;
    first.stop().await?;
    restarted.stop().await?;
    // Every real mutation response has completed and all application writers stopped.
    stack.recover_after_backend_drain(&stack.fga_url).await?;
    let restored = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &restored);
    expect(api.request(M::GET, "/readyz"), S::OK).await?;
    assert_eq!(revision(&db).await?.1, 0);
    expect(api.call(M::GET, TASK, &dave), S::FORBIDDEN).await?;
    expect(api.call(M::GET, TASK, &carol), S::OK).await?;
    expect(api.call(M::PUT, RELATION, &carol), S::NO_CONTENT).await?;
    expect(api.call(M::GET, TASK, &dave), S::OK).await?;
    expect(api.call(M::DELETE, RELATION, &carol), S::NO_CONTENT).await?;
    expect(api.call(M::GET, TASK, &dave), S::FORBIDDEN).await?;
    db.close().await;
    restored.stop().await
}
