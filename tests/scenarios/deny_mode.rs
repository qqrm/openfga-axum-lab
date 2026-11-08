//! Exercise the emergency switch through a real child, including startup while
//! OpenFGA is stopped. Sessions remain usable, but existing grants and pending
//! recovery work must not turn into access or authorization mutations.

use crate::{
    api::{Api, expect},
    stack::{Stack, TestResult, client},
};
use reqwest::{Client, Method as M, StatusCode as S};
use serde_json::{Value, json};
use sqlx::PgPool;

const TABLES: &str = "/projects/acme/warehouses/analytics/tables";
const TABLE: &str = "/projects/acme/warehouses/analytics/tables/requests";

async fn cache_revision(db: &PgPool) -> TestResult<(i64, i64)> {
    Ok(
        sqlx::query_as("SELECT generation,pending FROM authz_cache_state WHERE singleton")
            .fetch_one(db)
            .await?,
    )
}

async fn tuple_keys(client: &Client, stack: &Stack) -> TestResult<Vec<Value>> {
    let config = stack.config()?;
    let store = config["store_id"].as_str().ok_or("missing store id")?;
    let response: Value = client
        .post(format!("{}/stores/{store}/read", stack.fga_http_url))
        .json(&json!({"page_size": 100, "consistency": "HIGHER_CONSISTENCY"}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(
        response["continuation_token"]
            .as_str()
            .unwrap_or("")
            .is_empty(),
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
    let grant = format!("{TABLE}/grants/select");
    let subject = json!({"subject":{"type":"user","id":"dave"}});
    expect(
        api.call(M::PUT, &grant, &carol).json(&subject),
        S::NO_CONTENT,
    )
    .await?;
    for token in [&alice, &bob, &carol, &dave] {
        expect(api.call(M::GET, TABLE, token), S::OK).await?;
    }
    let input = json!({"name":"Updated observations","description":"Deny-mode control"});
    let creation = json!({"namespace_id":"metrics","name":"Deletion control","description":"Deny-mode fixture"});
    let created = expect(
        api.call(M::POST, TABLES, &alice)
            .header("Idempotency-Key", "deny-mode-deleted-fixture")
            .json(&creation),
        S::CREATED,
    )
    .await?;
    let id = created["id"].as_str().ok_or("missing created table id")?;
    let deleted = format!("{TABLES}/{id}");
    expect(api.call(M::DELETE, &deleted, &alice), S::NO_CONTENT).await?;
    stack.assert_no_tuples(&client, id).await?;
    normal.stop().await?;

    // These are known interrupted SQL lifecycle intents with no ambiguous
    // remote request. Deny mode must leave both untouched; a normal startup
    // can replay them after the backend returns without clearing any debt.
    stack
        .sql(include_str!("../../scripts/recovery-fixture.sql"))
        .await?;
    stack
        .sql(include_str!("../../scripts/deletion-fixture.sql"))
        .await?;
    let db = stack.pool().await?;
    let before_rows = stack.domain_snapshot().await?;
    let before_revision = cache_revision(&db).await?;
    assert_eq!(
        before_revision.1, 0,
        "fixture must start without pending fences"
    );
    let before_tuples = tuple_keys(&client, &stack).await?;
    assert!(!before_tuples.is_empty());
    stack.stop_fga().await?;
    assert!(
        client
            .get(format!("{}/healthz", stack.fga_http_url))
            .send()
            .await
            .is_err(),
        "OpenFGA must be unreachable for the deny-mode startup"
    );

    let mut command = stack.app_command(&stack.fga_url);
    command.env("AUTHZ_MODE", "deny");
    let denied = Stack::spawn_app(command).await?;
    let api = Api::new(client.clone(), &denied);
    let health = api
        .request(M::GET, "/healthz")
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    assert_eq!(health, "ok");
    expect(api.request(M::GET, "/readyz"), S::SERVICE_UNAVAILABLE).await?;
    // New and existing sessions work without OpenFGA. A denied authorization
    // decision must not masquerade as invalid credentials.
    let fresh = stack.login(&client, &denied, "carol").await?;
    expect(api.request(M::GET, TABLES), S::UNAUTHORIZED).await?;
    for token in [&alice, &bob, &carol, &dave, &fresh] {
        let visible = expect(api.call(M::GET, TABLES, token), S::OK).await?;
        assert!(
            visible["items"]
                .as_array()
                .ok_or("missing table page")?
                .is_empty()
        );
        assert!(visible["next_cursor"].is_null());
        let catalog = expect(api.call(M::GET, "/catalog", token), S::OK).await?;
        assert!(
            catalog["scopes"]
                .as_array()
                .ok_or("missing scopes")?
                .is_empty()
        );
        expect(api.call(M::GET, TABLE, token), S::FORBIDDEN).await?;
        let checked = expect(
            api.call(M::POST, &format!("{TABLE}/check"), token)
                .json(&json!({"action":"can_get_metadata"})),
            S::OK,
        )
        .await?;
        assert_eq!(checked["allowed"], false);
    }
    expect(
        api.call(M::POST, TABLES, &alice)
            .header("Idempotency-Key", "deny-mode-must-not-create")
            .json(&creation),
        S::FORBIDDEN,
    )
    .await?;
    expect(api.call(M::PATCH, TABLE, &carol).json(&input), S::FORBIDDEN).await?;
    expect(api.call(M::DELETE, TABLE, &carol), S::FORBIDDEN).await?;
    for method in [M::PUT, M::DELETE] {
        expect(
            api.call(method.clone(), &grant, &carol).json(&subject),
            S::FORBIDDEN,
        )
        .await?;
        expect(
            api.call(
                method,
                "/projects/acme/roles/analysts/assignees/bob",
                &alice,
            ),
            S::FORBIDDEN,
        )
        .await?;
    }
    expect(
        api.call(M::PUT, &format!("{TABLE}/owner"), &carol)
            .json(&json!({"owner_id":"dave"})),
        S::FORBIDDEN,
    )
    .await?;
    expect(
        api.call(M::PUT, &format!("{TABLE}/namespace"), &alice)
            .json(&json!({"namespace_id":"restricted"})),
        S::FORBIDDEN,
    )
    .await?;
    for id in ["recovery-fixture", "errors"] {
        expect(
            api.call(M::GET, &format!("{TABLES}/{id}"), &alice),
            S::NOT_FOUND,
        )
        .await?;
    }
    // Continuing an authorized deletion fails at the deny backend without
    // mutating SQL. Its already-completed counterpart also requires availability.
    expect(
        api.call(M::DELETE, &format!("{TABLES}/errors"), &bob),
        S::SERVICE_UNAVAILABLE,
    )
    .await?;
    expect(
        api.call(M::DELETE, &deleted, &alice),
        S::SERVICE_UNAVAILABLE,
    )
    .await?;
    expect(api.call(M::DELETE, &deleted, &carol), S::NOT_FOUND).await?;
    expect(api.call(M::DELETE, "/session", &fresh), S::NO_CONTENT).await?;
    expect(api.call(M::GET, TABLES, &fresh), S::UNAUTHORIZED).await?;
    assert_eq!(
        stack.domain_snapshot().await?,
        before_rows,
        "deny mode changed durable catalog data"
    );
    assert_eq!(
        cache_revision(&db).await?,
        before_revision,
        "deny mode changed the authorization fence"
    );
    denied.stop().await?;
    db.close().await;

    // Inspect stored tuples before startup can legitimately replay the intents.
    stack.restart_dependencies(&client).await?;
    assert_eq!(tuple_keys(&client, &stack).await?, before_tuples);
    let restored = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &restored);
    let db = stack.pool().await?;
    assert_eq!(cache_revision(&db).await?.1, 0);
    expect(api.request(M::GET, "/readyz"), S::OK).await?;
    for token in [&alice, &bob, &carol, &dave] {
        expect(api.call(M::GET, TABLE, token), S::OK).await?;
    }
    let recovered = expect(
        api.call(M::GET, &format!("{TABLES}/recovery-fixture"), &carol),
        S::OK,
    )
    .await?;
    assert_eq!(recovered["owner_id"], "carol");
    expect(
        api.call(M::GET, &format!("{TABLES}/recovery-fixture"), &dave),
        S::FORBIDDEN,
    )
    .await?;
    expect(
        api.call(M::GET, &format!("{TABLES}/errors"), &bob),
        S::NOT_FOUND,
    )
    .await?;
    stack.assert_no_tuples(&client, "errors").await?;
    expect(
        api.call(M::PATCH, TABLE, &carol).json(&input),
        S::NO_CONTENT,
    )
    .await?;
    let changed = expect(api.call(M::GET, TABLE, &carol), S::OK).await?;
    assert_eq!(changed["name"], input["name"]);
    db.close().await;
    restored.stop().await?;
    Ok(())
}
