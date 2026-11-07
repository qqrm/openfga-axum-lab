use crate::{
    api::{Api, expect},
    stack::{App, Stack, TestResult, client},
};
use reqwest::{Client, Method as M, StatusCode as S};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};

const PET: &str = "/pets/fox";
const RELATION: &str = "/pets/fox/relations/viewer/dave";
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
    let db = PgPoolOptions::new()
        .max_connections(1)
        .connect(stack.database_url())
        .await?;
    let alice = stack.login(&client, &app, "alice").await?;
    expect(api.call(M::GET, PET, &alice), S::OK).await?;
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
        expect(api.call(M::GET, PET, &alice), S::OK).await?;
        sqlx::raw_sql(repair).execute(&db).await?;
        let recovered = stack.login(&client, &app, "alice").await?;
        expect(api.call(M::GET, PET, &recovered), S::OK).await?;
        expect(api.call(M::DELETE, "/session", &recovered), S::NO_CONTENT).await?;
        assert_eq!(session_count(&db).await?, 1);
    }

    // A live token must not become a 401 or an authorized request when its
    // database lookup fails. DDL also invalidates PostgreSQL's prepared plans.
    sqlx::raw_sql("ALTER TABLE sessions RENAME COLUMN expires_at TO unavailable_expires_at")
        .execute(&db)
        .await?;
    expect(api.call(M::GET, PET, &alice), S::SERVICE_UNAVAILABLE).await?;
    assert_eq!(session_count(&db).await?, 1);
    sqlx::raw_sql("ALTER TABLE sessions RENAME COLUMN unavailable_expires_at TO expires_at")
        .execute(&db)
        .await?;
    expect(api.call(M::GET, PET, &alice), S::OK).await?;

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
    assert_eq!(session_count(&db).await?, 1, "failed logout deleted a session");
    expect(api.call(M::GET, PET, &alice), S::OK).await?;
    sqlx::raw_sql("DROP TRIGGER reject_logout ON sessions; DROP FUNCTION reject_test_write()")
        .execute(&db)
        .await?;
    expect(api.call(M::DELETE, "/session", &alice), S::NO_CONTENT).await?;
    assert_eq!(session_count(&db).await?, 0);
    expect(api.call(M::GET, PET, &alice), S::UNAUTHORIZED).await?;
    db.close().await;
    app.stop().await?;
    Ok(())
}

async fn revision(db: &PgPool) -> TestResult<(i64, i64)> {
    let revision =
        sqlx::query_as("SELECT generation, pending FROM authz_cache_state WHERE singleton")
            .fetch_one(db)
            .await?;
    Ok(revision)
}

async fn cache_event(client: &Client, app: &App, event: &str) -> TestResult<u64> {
    let body = client
        .get(format!("{}/metrics", app.url))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let prefix = format!("authz_cache_events_total{{event=\"{event}\"}} ");
    match body.lines().find_map(|line| line.strip_prefix(&prefix)) {
        Some(value) => Ok(value.parse()?),
        None => Ok(0),
    }
}

async fn fga_allows_dave(stack: &Stack, client: &Client) -> TestResult<bool> {
    let config = stack.config()?;
    let store = config["store_id"].as_str().ok_or("missing store id")?;
    let model = config["model_id"].as_str().ok_or("missing model id")?;
    let result: Value = client
        .post(format!("{}/stores/{store}/check", stack.fga_url))
        .json(&json!({
            "authorization_model_id": model,
            "consistency": "HIGHER_CONSISTENCY",
            "tuple_key": {"user": "user:dave", "relation": "can_view", "object": "pet:fox"}
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    result["allowed"].as_bool().ok_or("missing FGA decision".into())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn coordinator_database_errors_preserve_fences_and_recover() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let first = stack.app(&stack.fga_url).await?;
    let second = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &first);
    let db = PgPoolOptions::new()
        .max_connections(1)
        .connect(stack.database_url())
        .await?;
    let carol = stack.login(&client, &first, "carol").await?;
    let dave = stack.login(&client, &second, "dave").await?;
    let initial = revision(&db).await?;
    assert_eq!(initial.1, 0);

    for app in [&first, &second] {
        let api = Api::new(client.clone(), app);
        let before = cache_event(&client, app, "hit_allow").await?;
        expect(api.call(M::GET, PET, &carol), S::OK).await?;
        expect(api.call(M::GET, PET, &carol), S::OK).await?;
        assert_eq!(cache_event(&client, app, "hit_allow").await?, before + 1);
    }
    // Only the coordinator row is unavailable: session authentication and the
    // pet tables remain live, so this cannot fail early in the session guard.
    sqlx::query("DELETE FROM authz_cache_state WHERE singleton")
        .execute(&db)
        .await?;
    for app in [&first, &second] {
        let api = Api::new(client.clone(), app);
        expect(api.call(M::GET, PET, &carol), S::SERVICE_UNAVAILABLE).await?;
        expect(api.call(M::GET, "/pets", &carol), S::SERVICE_UNAVAILABLE).await?;
    }
    let healthy_login = stack.login(&client, &first, "alice").await?;
    expect(api.call(M::DELETE, "/session", &healthy_login), S::NO_CONTENT).await?;
    sqlx::query("INSERT INTO authz_cache_state(generation, pending) VALUES ($1, $2)")
        .bind(initial.0)
        .bind(initial.1)
        .execute(&db)
        .await?;
    for app in [&first, &second] {
        let api = Api::new(client.clone(), app);
        expect(api.call(M::GET, PET, &carol), S::OK).await?;
    }

    sqlx::raw_sql(REJECT_WRITE).execute(&db).await?;
    sqlx::raw_sql(
        "CREATE TRIGGER reject_begin BEFORE UPDATE ON authz_cache_state
         FOR EACH ROW WHEN (NEW.pending > OLD.pending)
         EXECUTE FUNCTION reject_test_write()",
    )
    .execute(&db)
    .await?;
    assert!(!fga_allows_dave(&stack, &client).await?);
    expect(api.call(M::PUT, RELATION, &carol), S::SERVICE_UNAVAILABLE).await?;
    assert_eq!(revision(&db).await?, initial, "failed begin persisted a fence");
    // Ask real OpenFGA directly: a cached deny could hide an accidental grant.
    assert!(!fga_allows_dave(&stack, &client).await?);
    expect(api.call(M::GET, PET, &dave), S::FORBIDDEN).await?;
    sqlx::raw_sql("DROP TRIGGER reject_begin ON authz_cache_state")
        .execute(&db)
        .await?;
    expect(api.call(M::PUT, RELATION, &carol), S::NO_CONTENT).await?;
    assert!(fga_allows_dave(&stack, &client).await?);
    let granted = revision(&db).await?;
    assert_eq!(granted, (initial.0 + 2, 0));

    sqlx::raw_sql(
        "CREATE TRIGGER reject_finalize BEFORE UPDATE ON authz_cache_state
         FOR EACH ROW WHEN (NEW.pending < OLD.pending)
         EXECUTE FUNCTION reject_test_write()",
    )
    .execute(&db)
    .await?;
    for app in [&first, &second] {
        let api = Api::new(client.clone(), app);
        let before = cache_event(&client, app, "hit_allow").await?;
        expect(api.call(M::GET, PET, &dave), S::OK).await?;
        expect(api.call(M::GET, PET, &dave), S::OK).await?;
        assert_eq!(cache_event(&client, app, "hit_allow").await?, before + 1);
    }
    expect(api.call(M::DELETE, RELATION, &carol), S::SERVICE_UNAVAILABLE).await?;
    assert!(!fga_allows_dave(&stack, &client).await?);
    let fenced = (granted.0 + 1, 1);
    assert_eq!(revision(&db).await?, fenced);
    for app in [&first, &second] {
        let before = cache_event(&client, app, "bypass_pending").await?;
        let api = Api::new(client.clone(), app);
        expect(api.call(M::GET, PET, &dave), S::FORBIDDEN).await?;
        assert_eq!(cache_event(&client, app, "bypass_pending").await?, before + 1);
    }

    // Repair restores writes, but cannot erase the earlier unresolved fence.
    sqlx::raw_sql(
        "DROP TRIGGER reject_finalize ON authz_cache_state; DROP FUNCTION reject_test_write()",
    )
    .execute(&db)
    .await?;
    // Retrying the already applied revoke is safe and must not clear its fence.
    expect(api.call(M::DELETE, RELATION, &carol), S::NO_CONTENT).await?;
    assert_eq!(revision(&db).await?, (fenced.0 + 2, 1));
    let second_api = Api::new(client.clone(), &second);
    let before = cache_event(&client, &second, "bypass_pending").await?;
    expect(api.call(M::PUT, RELATION, &carol), S::NO_CONTENT).await?;
    expect(second_api.call(M::GET, PET, &dave), S::OK).await?;
    expect(api.call(M::DELETE, RELATION, &carol), S::NO_CONTENT).await?;
    expect(second_api.call(M::GET, PET, &dave), S::FORBIDDEN).await?;
    assert_eq!(revision(&db).await?, (fenced.0 + 6, 1));
    assert_eq!(cache_event(&client, &second, "bypass_pending").await?, before + 2);
    assert!(!fga_allows_dave(&stack, &client).await?);

    second.stop().await?;
    let restarted = stack.app(&stack.fga_url).await?;
    let restarted_api = Api::new(client.clone(), &restarted);
    expect(restarted_api.call(M::GET, PET, &dave), S::FORBIDDEN).await?;
    assert_eq!(revision(&db).await?, (fenced.0 + 6, 1));
    assert_eq!(cache_event(&client, &restarted, "bypass_pending").await?, 1);
    db.close().await;
    restarted.stop().await?;
    first.stop().await?;
    Ok(())
}
