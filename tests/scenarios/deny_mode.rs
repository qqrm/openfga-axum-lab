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
    let viewer = "/tasks/access-review/relations/viewer/dave";
    expect(api.call(M::PUT, viewer, &carol), S::NO_CONTENT).await?;
    for token in [&alice, &bob, &carol, &dave] {
        expect(api.call(M::GET, "/tasks/access-review", token), S::OK).await?;
    }
    let input = crate::task_input();
    let creation = crate::create_input("acme");
    let created = expect(
        api.call(M::POST, "/tasks", &alice)
            .header("Idempotency-Key", "deny-mode-deleted-fixture")
            .json(&creation),
        S::CREATED,
    )
    .await?;
    let id = created["id"].as_str().ok_or("missing created task id")?;
    let deleted = format!("/tasks/{id}");
    expect(api.call(M::DELETE, &deleted, &alice), S::NO_CONTENT).await?;
    stack.assert_no_tuples(&client, id).await?;
    normal.stop().await?;

    // Deny mode must leave interrupted lifecycle work untouched. Explicit drained recovery
    // will reconcile these fixtures after the real backend returns.
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
    // This new login, plus existing sessions from the first process, must work
    // without OpenFGA. Authorization failure must not masquerade as bad login.
    let fresh = stack.login(&client, &denied, "carol").await?;
    expect(api.request(M::GET, "/tasks"), S::UNAUTHORIZED).await?;
    for token in [&alice, &bob, &carol, &dave, &fresh] {
        expect(api.call(M::GET, "/tasks", token), S::SERVICE_UNAVAILABLE).await?;
        expect(
            api.call(M::GET, "/organizations", token),
            S::SERVICE_UNAVAILABLE,
        )
        .await?;
        expect(
            api.call(M::GET, "/tasks/access-review", token),
            S::FORBIDDEN,
        )
        .await?;
    }
    expect(
        api.call(M::POST, "/tasks", &alice)
            .header("Idempotency-Key", "deny-mode-must-not-create")
            .json(&creation),
        S::FORBIDDEN,
    )
    .await?;
    expect(
        api.call(M::PATCH, "/tasks/access-review", &carol)
            .json(&input),
        S::FORBIDDEN,
    )
    .await?;
    expect(
        api.call(M::DELETE, "/tasks/access-review", &carol),
        S::FORBIDDEN,
    )
    .await?;
    expect(
        api.call(M::PUT, "/tasks/access-review/relations/editor/dave", &carol),
        S::FORBIDDEN,
    )
    .await?;
    expect(api.call(M::DELETE, viewer, &carol), S::FORBIDDEN).await?;
    expect(
        api.call(M::PUT, "/organizations/acme/owner", &alice)
            .json(&json!({"expected_owner": "alice", "new_owner": "bob"})),
        S::FORBIDDEN,
    )
    .await?;
    expect(
        api.call(M::PUT, "/organizations/acme/members/dave", &alice)
            .json(&json!({"role": "admin"})),
        S::FORBIDDEN,
    )
    .await?;
    expect(
        api.call(
            M::PUT,
            "/organizations/acme/permissions/can_edit_task/dave",
            &alice,
        ),
        S::FORBIDDEN,
    )
    .await?;
    for path in ["/tasks/recovery-fixture", "/tasks/incident-response"] {
        expect(api.call(M::GET, path, &alice), S::NOT_FOUND).await?;
    }
    // The unjournaled deleting fixture is not an active task. Even an already
    // completed deletion cannot report success when backend readiness is denied.
    expect(
        api.call(M::DELETE, "/tasks/incident-response", &bob),
        S::NOT_FOUND,
    )
    .await?;
    expect(
        api.call(M::DELETE, &deleted, &alice),
        S::SERVICE_UNAVAILABLE,
    )
    .await?;
    expect(api.call(M::DELETE, &deleted, &carol), S::NOT_FOUND).await?;
    expect(api.call(M::DELETE, "/session", &fresh), S::NO_CONTENT).await?;
    expect(api.call(M::GET, "/tasks", &fresh), S::UNAUTHORIZED).await?;
    assert_eq!(
        stack.domain_snapshot().await?,
        before_rows,
        "deny mode changed durable domain data"
    );
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
    // All application writers are stopped; deny mode issued no remote requests.
    // Only an explicit drained recovery may clear conservative lifecycle debt.
    stack.recover_after_backend_drain(&stack.fga_url).await?;
    let restored = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &restored);
    for token in [&alice, &bob, &carol, &dave] {
        expect(api.call(M::GET, "/tasks/access-review", token), S::OK).await?;
    }
    expect(api.call(M::GET, "/tasks/recovery-fixture", &carol), S::OK).await?;
    expect(
        api.call(M::GET, "/tasks/incident-response", &bob),
        S::NOT_FOUND,
    )
    .await?;
    stack.assert_no_tuples(&client, "incident-response").await?;
    expect(
        api.call(M::PATCH, "/tasks/access-review", &carol)
            .json(&input),
        S::NO_CONTENT,
    )
    .await?;
    restored.stop().await?;
    Ok(())
}
