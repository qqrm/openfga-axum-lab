//! Exercise both login limits through the real child process. PostgreSQL's
//! clock controls the minute-window fixture; table-lock dependencies show when
//! all four in-flight logins hold permits, without racing Argon2 or sleeping.

use crate::{
    api::{Api, expect},
    stack::{Stack, TestResult, client},
};
use reqwest::{Method as M, StatusCode as S};
use serde_json::json;
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::time::Duration;
use tokio::{
    task::JoinSet,
    time::{interval, timeout},
};

async fn attempts(pool: &PgPool, username: &str) -> TestResult<i32> {
    Ok(
        sqlx::query_scalar("SELECT attempts FROM login_attempts WHERE subject=$1")
            .bind(username)
            .fetch_one(pool)
            .await?,
    )
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn login_rate_limit_allows_twentieth_attempt_and_resets_expired_window() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .connect(stack.database_url())
        .await?;
    // Seed the quota near its boundary instead of burning 19 Argon2 checks.
    sqlx::query(
        "INSERT INTO login_attempts(subject,attempts,window_start) VALUES ('alice',19,now())",
    )
    .execute(&pool)
    .await?;
    let alice = stack.login(&client, &app, "alice").await?;
    assert_eq!(attempts(&pool, "alice").await?, 20);
    // More than four denials also catch permits leaking on this early return.
    for expected_attempts in 21..=25 {
        expect(
            api.request(M::POST, "/session")
                .json(&json!({"username": "alice", "password": "wrong-password"})),
            S::TOO_MANY_REQUESTS,
        )
        .await?;
        assert_eq!(attempts(&pool, "alice").await?, expected_attempts);
    }
    let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions WHERE user_id='alice'")
        .fetch_one(&pool)
        .await?;
    assert_eq!(sessions, 1, "throttled logins must not create sessions");
    expect(api.call(M::GET, "/pets/fox", &alice), S::OK).await?;
    // This quota is per username; it must not disable an unrelated account.
    let bob = stack.login(&client, &app, "bob").await?;
    expect(api.call(M::GET, "/pets/fox", &bob), S::OK).await?;
    assert_eq!(attempts(&pool, "bob").await?, 1);

    sqlx::query(
        "UPDATE login_attempts SET window_start=now()-interval '2 minutes' WHERE subject='alice'",
    )
    .execute(&pool)
    .await?;
    let renewed = stack.login(&client, &app, "alice").await?;
    assert!(renewed != alice, "new login reused an existing session token");
    expect(api.call(M::GET, "/pets/fox", &renewed), S::OK).await?;
    let (count, fresh): (i32, bool) = sqlx::query_as(
        "SELECT attempts, window_start BETWEEN now()-interval '1 minute' AND now() \
         FROM login_attempts WHERE subject='alice'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(count, 1);
    assert!(fresh, "reset must replace the expired window timestamp");
    pool.close().await;
    app.stop().await?;
    Ok(())
}

async fn wait_for_four_blocked_logins(pool: &PgPool, blocker: i32) -> TestResult {
    timeout(Duration::from_secs(10), async {
        let mut poll = interval(Duration::from_millis(20));
        loop {
            poll.tick().await;
            let waiting: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity \
                 WHERE datname=current_database() AND state='active' \
                 AND wait_event_type='Lock' AND query LIKE 'INSERT INTO login_attempts(%' \
                 AND $1 = ANY(pg_blocking_pids(pid))",
            )
            .bind(blocker)
            .fetch_one(pool)
            .await?;
            if waiting == 4 {
                return Ok::<(), sqlx::Error>(());
            }
        }
    })
    .await
    .map_err(|error| format!("four logins did not reach the held table lock: {error}"))??;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn login_concurrency_limit_rejects_fifth_request_and_releases_all_permits() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(5))
        .connect(stack.database_url())
        .await?;

    // Repeating saturation proves that all four permits return after failed
    // password verification; a single successful retry could miss a leak.
    for round in 1..=2 {
        let mut lock = pool.begin().await?;
        sqlx::query("SET LOCAL lock_timeout = '5s'")
            .execute(&mut *lock)
            .await?;
        sqlx::query("LOCK TABLE login_attempts IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *lock)
            .await?;
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *lock)
            .await?;
        let mut pending = JoinSet::new();
        for username in ["alice", "bob", "carol", "dave"] {
            let request = api
                .request(M::POST, "/session")
                .json(&json!({"username": username, "password": "wrong-password"}))
                .timeout(Duration::from_secs(30));
            pending.spawn(async move { expect(request, S::UNAUTHORIZED).await });
        }
        wait_for_four_blocked_logins(&pool, blocker).await?;
        println!("round {round}: all four login requests wait for PostgreSQL backend {blocker}");
        expect(
            api.request(M::POST, "/session")
                .json(&json!({"username": "throttle-probe", "password": "wrong-password"}))
                .timeout(Duration::from_secs(5)),
            S::TOO_MANY_REQUESTS,
        )
        .await?;
        lock.commit().await?;
        let mut completed = 0;
        while let Some(result) = pending.join_next().await {
            result??;
            completed += 1;
        }
        assert_eq!(completed, 4);
        for username in ["alice", "bob", "carol", "dave"] {
            assert_eq!(attempts(&pool, username).await?, round);
        }
        let probe_attempts: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM login_attempts WHERE subject='throttle-probe'",
        )
        .fetch_one(&pool)
        .await?;
        assert_eq!(
            probe_attempts, 0,
            "overload must not consume a subject's quota"
        );
        let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
            .fetch_one(&pool)
            .await?;
        assert_eq!(sessions, 0, "failed logins must not create sessions");
    }

    let alice = stack.login(&client, &app, "alice").await?;
    expect(api.call(M::GET, "/pets/fox", &alice), S::OK).await?;
    assert_eq!(attempts(&pool, "alice").await?, 3);
    pool.close().await;
    app.stop().await?;
    Ok(())
}
