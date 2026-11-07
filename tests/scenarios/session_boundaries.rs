use crate::{
    api::{Api, expect},
    stack::{Stack, TestResult, client},
};
use argon2::{
    Argon2, PasswordHasher,
    password_hash::{SaltString, rand_core::OsRng},
};
use reqwest::{
    Method as M, StatusCode as S,
    header::{AUTHORIZATION, CACHE_CONTROL, HeaderValue},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, postgres::PgPoolOptions};

async fn session_count(db: &PgPool) -> TestResult<i64> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM sessions")
        .fetch_one(db)
        .await?)
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn login_input_limits_and_unusable_credentials_fail_closed() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    let db = PgPoolOptions::new()
        .max_connections(1)
        .connect(stack.database_url())
        .await?;

    for input in [
        json!({"username": "", "password": "synthetic-password"}),
        json!({"username": "user:carol", "password": "synthetic-password"}),
        json!({"username": "x".repeat(65), "password": "synthetic-password"}),
        json!({"username": "carol", "password": ""}),
        json!({"username": "carol", "password": "p".repeat(1025)}),
    ] {
        expect(api.request(M::POST, "/session").json(&input), S::UNAUTHORIZED).await?;
    }
    for input in [
        json!({"username": "carol", "password": "synthetic-password", "admin": true}),
        json!({"username": "carol"}),
    ] {
        let response = api.request(M::POST, "/session").json(&input).send().await?;
        assert_eq!(response.status(), S::UNPROCESSABLE_ENTITY);
    }
    assert_eq!(session_count(&db).await?, 0);
    let attempts: i64 = sqlx::query_scalar("SELECT count(*) FROM login_attempts")
        .fetch_one(&db)
        .await?;
    assert_eq!(attempts, 0, "invalid input consumed a login-attempt record");

    // Exercise the inclusive limits with real Argon2 verification and SQL.
    let username = "x".repeat(64);
    let password = "p".repeat(1024);
    let encoded = Argon2::default()
        .hash_password(password.as_bytes(), &SaltString::generate(&mut OsRng))?
        .to_string();
    sqlx::query("INSERT INTO users(id,display_name,password_hash) VALUES ($1,$1,$2)")
        .bind(&username)
        .bind(encoded)
        .execute(&db)
        .await?;
    let response = api
        .request(M::POST, "/session")
        .json(&json!({"username": &username, "password": &password}))
        .send()
        .await?;
    assert_eq!(response.status(), S::OK);
    assert_eq!(
        response
            .headers()
            .get(CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    let body: Value = response.json().await?;
    assert_eq!(body["expires_in"], json!(43200));
    let token = body["token"].as_str().ok_or("missing session token")?;
    assert!(token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let stored: Vec<u8> = sqlx::query_scalar("SELECT token_hash FROM sessions WHERE user_id=$1")
        .bind(&username)
        .fetch_one(&db)
        .await?;
    assert!(stored == Sha256::digest(token.as_bytes()).to_vec());
    assert!(
        stored.as_slice() != token.as_bytes(),
        "stored a raw session token"
    );
    expect(api.call(M::GET, "/pets", token), S::OK).await?;
    expect(api.call(M::DELETE, "/session", token), S::NO_CONTENT).await?;
    assert_eq!(session_count(&db).await?, 0);

    let original: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id='carol'")
        .fetch_one(&db)
        .await?;
    for unusable in [Some("not-an-argon2-hash"), None] {
        sqlx::query("UPDATE users SET password_hash=$1 WHERE id='carol'")
            .bind(unusable)
            .execute(&db)
            .await?;
        // Stack.login supplies the real, otherwise correct fixture password.
        let error = match stack.login(&client, &app, "carol").await {
            Err(error) => error,
            Ok(_) => return Err("unusable credentials issued a session".into()),
        };
        let status = error
            .downcast_ref::<reqwest::Error>()
            .and_then(reqwest::Error::status);
        assert_eq!(status, Some(S::UNAUTHORIZED));
        assert_eq!(session_count(&db).await?, 0);

        sqlx::query("UPDATE users SET password_hash=$1 WHERE id='carol'")
            .bind(&original)
            .execute(&db)
            .await?;
        let restored = stack.login(&client, &app, "carol").await?;
        expect(api.call(M::GET, "/pets/fox", &restored), S::OK).await?;
        expect(api.call(M::DELETE, "/session", &restored), S::NO_CONTENT).await?;
        assert_eq!(session_count(&db).await?, 0);
    }
    db.close().await;
    app.stop().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn invalid_expired_and_revoked_sessions_cannot_be_reused() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    let db = PgPoolOptions::new()
        .max_connections(1)
        .connect(stack.database_url())
        .await?;
    let live = stack.login(&client, &app, "carol").await?;
    let expired = stack.login(&client, &app, "carol").await?;
    assert!(live != expired, "separate logins reused a session token");
    for token in [&live, &expired] {
        expect(api.call(M::GET, "/pets/fox", token), S::OK).await?;
    }

    // Preserve the current literal Bearer prefix and bounded opaque-token
    // contract. No malformed request may revoke an independent live session.
    for (case, header) in [
        ("missing header", None),
        ("empty header", Some(HeaderValue::from_static(""))),
        (
            "wrong scheme",
            Some(HeaderValue::from_static("Basic synthetic")),
        ),
        (
            "missing separator",
            Some(HeaderValue::from_static("Bearer")),
        ),
        ("empty token", Some(HeaderValue::from_static("Bearer "))),
        (
            "lowercase scheme",
            Some(HeaderValue::from_str(&format!("bearer {live}"))?),
        ),
        (
            "non-ASCII header",
            Some(HeaderValue::from_bytes(b"Bearer \xff")?),
        ),
        (
            "nonexistent token",
            Some(HeaderValue::from_str(&format!("Bearer {}", "0".repeat(64)))?),
        ),
        (
            "maximum-length nonexistent token",
            Some(HeaderValue::from_str(&format!("Bearer {}", "0".repeat(256)))?),
        ),
        (
            "oversized token",
            Some(HeaderValue::from_str(&format!("Bearer {}", "0".repeat(257)))?),
        ),
    ] {
        for (method, path) in [(M::GET, "/pets/fox"), (M::DELETE, "/session")] {
            let mut request = api.request(method, path);
            if let Some(value) = &header {
                request = request.header(AUTHORIZATION, value.clone());
            }
            let response = request.send().await?;
            assert_eq!(
                response.status(),
                S::UNAUTHORIZED,
                "{case} accepted at {path}"
            );
        }
    }
    assert_eq!(session_count(&db).await?, 2);
    expect(api.call(M::GET, "/pets/fox", &live), S::OK).await?;

    // Use PostgreSQL's own clock, not sleeps or a wall-clock race with expiry.
    let changed = sqlx::query(
        "UPDATE sessions SET expires_at=now()-interval '1 second' WHERE token_hash=$1",
    )
    .bind(Sha256::digest(expired.as_bytes()).to_vec())
    .execute(&db)
    .await?
    .rows_affected();
    assert_eq!(changed, 1);
    expect(api.call(M::GET, "/pets/fox", &expired), S::UNAUTHORIZED).await?;
    expect(api.call(M::DELETE, "/session", &expired), S::UNAUTHORIZED).await?;
    expect(api.call(M::GET, "/pets/fox", &live), S::OK).await?;

    expect(api.call(M::DELETE, "/session", &live), S::NO_CONTENT).await?;
    expect(api.call(M::DELETE, "/session", &live), S::UNAUTHORIZED).await?;
    expect(api.call(M::GET, "/pets/fox", &live), S::UNAUTHORIZED).await?;
    assert_eq!(session_count(&db).await?, 1, "only the expired row remains");
    let fresh = stack.login(&client, &app, "carol").await?;
    assert!(
        fresh != live && fresh != expired,
        "new login reused an old token"
    );
    expect(api.call(M::GET, "/pets/fox", &fresh), S::OK).await?;
    for old in [&live, &expired] {
        expect(api.call(M::GET, "/pets/fox", old), S::UNAUTHORIZED).await?;
    }
    expect(api.call(M::DELETE, "/session", &fresh), S::NO_CONTENT).await?;
    assert_eq!(session_count(&db).await?, 1);
    db.close().await;
    app.stop().await?;
    Ok(())
}
