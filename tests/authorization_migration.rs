//! Real PostgreSQL + OpenFGA + Axum migration/cutover tests. Faults live at the
//! HTTP boundary; persistent data and authorization decisions use real servers.
#[path = "support/api.rs"]
mod api;
#[path = "migration_support/proxy.rs"]
mod proxy;
#[allow(dead_code)]
#[path = "cache_support/proxy.rs"]
mod runtime_proxy;
#[allow(dead_code)]
#[path = "support/stack.rs"]
mod stack;

use api::{Api, expect};
use proxy::Proxy;
use reqwest::{Method as M, StatusCode as S};
use serde_json::{Value, json};
use sqlx::PgPool;
use stack::{Stack, TestResult, client};
use std::{collections::BTreeSet, process::Stdio, time::Duration};
use tokio::{process::Command, time::timeout};

const TABLES: &str = "/projects/acme/warehouses/analytics/tables";
const REQUESTS: &str = "/projects/acme/warehouses/analytics/tables/requests";
const OTHER_TABLES: &str = "/projects/other/warehouses/external/tables";
const OTHER_REQUESTS: &str = "/projects/other/warehouses/external/tables/requests";

async fn run(mut command: Command, success: bool) -> TestResult<String> {
    let output = timeout(
        Duration::from_secs(180),
        command.kill_on_drop(true).output(),
    )
    .await??;
    assert_eq!(
        output.status.success(),
        success,
        "migration output: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?)
}

async fn state(pool: &PgPool) -> TestResult<(String, i64, i64)> {
    Ok(sqlx::query_as(
        "SELECT status,next_tuple,tuple_count FROM authorization_migration WHERE singleton",
    )
    .fetch_one(pool)
    .await?)
}

async fn pending(pool: &PgPool) -> TestResult<i64> {
    Ok(
        sqlx::query_scalar("SELECT pending FROM authz_cache_state WHERE singleton")
            .fetch_one(pool)
            .await?,
    )
}

async fn attempted(pool: &PgPool) -> TestResult<bool> {
    Ok(
        sqlx::query_scalar("SELECT attempted FROM authorization_migration WHERE singleton")
            .fetch_one(pool)
            .await?,
    )
}

async fn tuple_keys(stack: &Stack) -> TestResult<BTreeSet<(String, String, String)>> {
    let client = client()?;
    let config = stack.config()?;
    let store = config["store_id"].as_str().ok_or("missing store")?;
    let mut cursor = String::new();
    let mut seen_cursors = BTreeSet::new();
    let mut tuples = BTreeSet::new();
    loop {
        let response: Value = client
            .post(format!("{}/stores/{store}/read", stack.fga_http_url))
            .json(&json!({"page_size":100,"continuation_token":cursor,"consistency":"HIGHER_CONSISTENCY"}))
            .send().await?.error_for_status()?.json().await?;
        if let Some(page) = response["tuples"].as_array() {
            for tuple in page {
                let key = &tuple["key"];
                assert!(
                    tuples.insert((
                        key["user"].as_str().ok_or("missing tuple user")?.to_owned(),
                        key["relation"]
                            .as_str()
                            .ok_or("missing tuple relation")?
                            .to_owned(),
                        key["object"]
                            .as_str()
                            .ok_or("missing tuple object")?
                            .to_owned(),
                    )),
                    "duplicate tuple in target snapshot"
                );
            }
        }
        cursor = response["continuation_token"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        if cursor.is_empty() {
            return Ok(tuples);
        }
        assert!(
            seen_cursors.insert(cursor.clone()),
            "repeating tuple cursor"
        );
    }
}

async fn tuple_count(stack: &Stack) -> TestResult<usize> {
    Ok(tuple_keys(stack).await?.len())
}

fn ids(page: &Value) -> BTreeSet<&str> {
    page["items"]
        .as_array()
        .expect("paginated table list")
        .iter()
        .map(|row| row["id"].as_str().expect("table id"))
        .collect()
}

async fn closed(api: &Api, token: &str) -> TestResult {
    for path in [TABLES, REQUESTS, "/readyz"] {
        expect(api.call(M::GET, path, token), S::SERVICE_UNAVAILABLE).await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn sql_permissions_cut_over_then_roles_owners_and_parent_links_survive_restart_and_revoke()
-> TestResult {
    let client = client()?;
    let mut stack = Stack::start(&client).await?;
    let app = stack.app_unmigrated(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    let alice = stack.login(&client, &app, "alice").await?;
    let bob = stack.login(&client, &app, "bob").await?;
    let carol = stack.login(&client, &app, "carol").await?;
    let dave = stack.login(&client, &app, "dave").await?;
    let eve = stack.login(&client, &app, "eve").await?;
    closed(&api, &alice).await?;
    assert_eq!(tuple_count(&stack).await?, 0);
    let report: Value = serde_json::from_str(&run(stack.migration_command(), true).await?)?;
    assert_eq!(report["already_active"], false);
    assert!(
        report["verified_checks"].as_u64().unwrap() > 1000,
        "activation must include every known subject/action/object and an outsider"
    );
    for (token, expected) in [
        (&alice, BTreeSet::from(["requests", "latency", "errors"])),
        (&bob, BTreeSet::from(["requests", "latency", "errors"])),
        (&carol, BTreeSet::from(["requests"])),
        (&dave, BTreeSet::from(["latency"])),
    ] {
        let page = expect(api.call(M::GET, TABLES, token), S::OK).await?;
        assert_eq!(ids(&page), expected);
    }
    expect(api.call(M::GET, REQUESTS, &dave), S::FORBIDDEN).await?;
    for (token, path) in [
        (&eve, REQUESTS),
        (&eve, TABLES),
        (&alice, OTHER_REQUESTS),
        (&alice, OTHER_TABLES),
    ] {
        expect(api.call(M::GET, path, token), S::NOT_FOUND).await?;
    }
    let foreign = expect(api.call(M::GET, OTHER_REQUESTS, &eve), S::OK).await?;
    assert_eq!(foreign["warehouse_id"], "external");
    // Bob's inherited select role must not imply write or ownership management.
    expect(
        api.call(M::PUT, &format!("{REQUESTS}/owner"), &bob)
            .json(&json!({"owner_id":"bob"})),
        S::FORBIDDEN,
    )
    .await?;
    expect(
        api.call(
            M::DELETE,
            "/projects/acme/roles/analysts/assignees/bob",
            &alice,
        ),
        S::NO_CONTENT,
    )
    .await?;
    expect(api.call(M::GET, REQUESTS, &bob), S::FORBIDDEN).await?;
    assert_eq!(
        ids(&expect(api.call(M::GET, TABLES, &bob), S::OK).await?),
        BTreeSet::from(["errors"])
    );
    // Both ownership and the forward/reverse namespace link are real tuples.
    expect(
        api.call(M::PUT, &format!("{REQUESTS}/owner"), &carol)
            .json(&json!({"owner_id":"dave"})),
        S::NO_CONTENT,
    )
    .await?;
    expect(api.call(M::GET, REQUESTS, &carol), S::FORBIDDEN).await?;
    expect(api.call(M::GET, REQUESTS, &dave), S::OK).await?;
    expect(
        api.call(M::PUT, &format!("{REQUESTS}/namespace"), &dave)
            .json(&json!({"namespace_id":"restricted"})),
        S::FORBIDDEN,
    )
    .await?;
    expect(
        api.call(M::PUT, &format!("{REQUESTS}/namespace"), &alice)
            .json(&json!({"namespace_id":"restricted"})),
        S::NO_CONTENT,
    )
    .await?;
    let moved = expect(api.call(M::GET, REQUESTS, &dave), S::OK).await?;
    assert_eq!(moved["namespace_id"], "restricted");
    let before = tuple_count(&stack).await?;
    let rerun: Value = serde_json::from_str(&run(stack.migration_command(), true).await?)?;
    assert_eq!(rerun["already_active"], true);
    assert_eq!(tuple_count(&stack).await?, before);
    app.stop().await?;
    stack.restart_dependencies(&client).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client, &app);
    expect(api.call(M::GET, REQUESTS, &bob), S::FORBIDDEN).await?;
    expect(api.call(M::GET, REQUESTS, &carol), S::FORBIDDEN).await?;
    let owner = expect(api.call(M::GET, REQUESTS, &dave), S::OK).await?;
    assert_eq!(owner["owner_id"], "dave");
    assert_eq!(owner["namespace_id"], "restricted");
    assert_eq!(
        ids(&expect(api.call(M::GET, TABLES, &carol), S::OK).await?),
        BTreeSet::new()
    );
    app.stop().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn acknowledged_batches_and_lost_write_response_resume_after_actual_database_and_openfga_restart()
-> TestResult {
    let client = client()?;
    let mut stack = Stack::start(&client).await?;
    let app = stack.app_unmigrated(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    let alice = stack.login(&client, &app, "alice").await?;
    let pool = stack.pool().await?;
    let proxy = Proxy::start(client.clone(), stack.fga_http_url.clone()).await?;
    let mut paused = proxy.pause_write(2).await;
    let mut command = stack.migration_command();
    command
        .env("FGA_URL", &proxy.url)
        .env("AUTHZ_MIGRATION_BATCH_SIZE", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn()?;
    paused.captured().await?;
    // The second actual OpenFGA transaction succeeded, but the process never
    // received its acknowledgement. Its durable first-batch checkpoint remains.
    child.kill().await?;
    child.wait().await?;
    drop(paused);
    proxy.wait_idle().await?;
    assert!(attempted(&pool).await?);
    assert_eq!(state(&pool).await?.0, "writing");
    assert_eq!(state(&pool).await?.1, 1);
    assert_eq!(tuple_count(&stack).await?, 2);
    closed(&api, &alice).await?;
    assert!(sqlx::query("UPDATE catalog_tables SET owner_id='alice' WHERE warehouse_id='analytics' AND id='requests'").execute(&pool).await.is_err(),"bound source may not change while partially migrated");
    // A valid but different store/model is not accepted as a resume target.
    let original = std::fs::read(stack.config_path())?;
    let directory = tempfile::tempdir()?;
    let alternate = directory.path().join("other-target.json");
    let mut provision = stack.bootstrap_command();
    provision.env("FGA_CONFIG", &alternate);
    run(provision, true).await?;
    let mut wrong_target = stack.migration_command();
    wrong_target.env("FGA_CONFIG", &alternate);
    run(wrong_target, false).await?;
    assert_eq!(std::fs::read(stack.config_path())?, original);
    assert_eq!(state(&pool).await?.1, 1);
    // Checkpoint corruption/source disagreement must fail instead of silently
    // regenerating a different plan over already written relationships.
    let source_hash: String =
        sqlx::query_scalar("SELECT source_sha256 FROM authorization_migration WHERE singleton")
            .fetch_one(&pool)
            .await?;
    sqlx::query(
        "UPDATE authorization_migration SET source_sha256='tampered-source' WHERE singleton",
    )
    .execute(&pool)
    .await?;
    run(stack.migration_command(), false).await?;
    assert_eq!(state(&pool).await?.0, "writing");
    assert_eq!(state(&pool).await?.1, 1);
    sqlx::query("UPDATE authorization_migration SET source_sha256=$1 WHERE singleton")
        .bind(source_hash)
        .execute(&pool)
        .await?;
    app.stop().await?;
    pool.close().await;
    stack.stop_fga().await?;
    run(stack.migration_command(), false).await?;
    stack.restart_dependencies(&client).await?;
    let app = stack.app_unmigrated(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    closed(&api, &alice).await?;
    let report: Value = serde_json::from_str(&run(stack.migration_command(), true).await?)?;
    assert_eq!(report["already_active"], false);
    assert_eq!(
        tuple_count(&stack).await?,
        report["tuple_count"].as_u64().unwrap() as usize
    );
    closed(&api, &alice).await?;
    let pool = stack.pool().await?;
    let (status, next, total) = state(&pool).await?;
    assert_eq!(status, "active");
    assert_eq!(next, total);
    assert!(!attempted(&pool).await?);
    assert!(
        pending(&pool).await? > 0,
        "replay cannot discharge a lost acknowledgement"
    );
    pool.close().await;
    app.stop().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn conflicting_legacy_subjects_roles_and_relations_are_rejected_before_any_tuple_write()
-> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let app = stack.app_unmigrated(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    let alice = stack.login(&client, &app, "alice").await?;
    let pool = stack.pool().await?;
    for (kind, id, relation) in [
        ("user", "eve", "select"),
        ("role", "other-analysts", "select"),
        ("user", "missing", "select"),
        ("user", "*", "select"),
        ("user", "bob", "ownership"),
        ("user", "bob", "can_grant_select"),
    ] {
        sqlx::query("INSERT INTO legacy_resource_grants(object_kind,object_id,subject_kind,subject_id,relation) VALUES ('lakekeeper_table','analytics/requests',$1,$2,$3)").bind(kind).bind(id).bind(relation).execute(&pool).await?;
        run(stack.migration_command(), false).await?;
        assert_eq!(tuple_count(&stack).await?, 0);
        assert_eq!(state(&pool).await?.0, "pending");
        closed(&api, &alice).await?;
        sqlx::query("DELETE FROM legacy_resource_grants WHERE object_kind='lakekeeper_table' AND object_id='analytics/requests'").execute(&pool).await?;
    }
    sqlx::query("INSERT INTO legacy_role_assignments(role_id,subject_kind,subject_id) VALUES ('analysts','role','analyst-members')").execute(&pool).await?;
    run(stack.migration_command(), false).await?;
    assert_eq!(tuple_count(&stack).await?, 0);
    assert_eq!(state(&pool).await?.0, "pending");
    pool.close().await;
    app.stop().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn false_write_success_and_permission_mismatch_never_activate_then_correct_resume_verifies_again()
-> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let app = stack.app_unmigrated(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    let alice = stack.login(&client, &app, "alice").await?;
    let pool = stack.pool().await?;
    let proxy = Proxy::start(client.clone(), stack.fga_http_url.clone()).await?;
    for redirect in [true, false] {
        if redirect {
            proxy.redirect_write().await;
        } else {
            proxy.error_object_write().await;
        }
        let mut command = stack.migration_command();
        command.env("FGA_URL", &proxy.url);
        run(command, false).await?;
        assert_eq!(tuple_count(&stack).await?, 0);
        assert_eq!(state(&pool).await?.0, "writing");
        assert_eq!(state(&pool).await?.1, 0);
        assert!(attempted(&pool).await?);
        closed(&api, &alice).await?;
    }
    proxy.wrong_check().await;
    let mut command = stack.migration_command();
    command.env("FGA_URL", &proxy.url);
    run(command, false).await?;
    let (status, next, total) = state(&pool).await?;
    assert_eq!(status, "verifying");
    assert_eq!(next, total);
    assert_eq!(tuple_count(&stack).await?, total as usize);
    closed(&api, &alice).await?;
    let checks: i64 =
        sqlx::query_scalar("SELECT verified_checks FROM authorization_migration WHERE singleton")
            .fetch_one(&pool)
            .await?;
    assert_eq!(checks, 0);
    run(stack.migration_command(), true).await?;
    assert_eq!(state(&pool).await?.0, "active");
    assert!(!attempted(&pool).await?);
    assert!(pending(&pool).await? > 0);
    // Invalid acknowledgements cannot prove whether an intermediary retained
    // the request. A later exact verification leaves access fenced.
    closed(&api, &alice).await?;
    pool.close().await;
    app.stop().await?;
    Ok(())
}

// A durable lifecycle replay can finish its own idempotent intent. It cannot
// clear the independent fence left by a killed process's ambiguous write.
async fn lost_runtime_relationship_acknowledgement(operation: &str) -> TestResult {
    let client = client()?;
    let mut stack = Stack::start(&client).await?;
    let proxy = runtime_proxy::Proxy::start(client.clone(), stack.fga_url.clone()).await?;
    let app = stack.app(&proxy.url).await?;
    let api = Api::new(client.clone(), &app);
    let alice = stack.login(&client, &app, "alice").await?;
    let carol = stack.login(&client, &app, "carol").await?;
    let before = tuple_count(&stack).await?;
    let (actor, payload, expected_owner, expected_namespace) = match operation {
        "owner" => (&carol, json!({"owner_id":"dave"}), "dave", "metrics"),
        "namespace" => (
            &alice,
            json!({"namespace_id":"restricted"}),
            "carol",
            "restricted",
        ),
        _ => return Err("unsupported recovery test operation".into()),
    };
    let mut pause = proxy.pause("write", false).await;
    let request = api
        .call(M::PUT, &format!("{REQUESTS}/{operation}"), actor)
        .json(&payload);
    let request = tokio::spawn(async move { request.send().await });
    pause.captured().await?;
    // The actual OpenFGA transaction committed; the Axum process has neither
    // received its response nor committed its final metadata transaction.
    app.crash().await?;
    let response = timeout(Duration::from_secs(10), request).await??;
    assert!(
        response.is_err(),
        "a killed mutation must not report successful completion"
    );
    drop(pause);
    drop(proxy);
    let pool = stack.pool().await?;
    let row: (String, String, String) = sqlx::query_as(
        "SELECT status,owner_id,namespace_id FROM catalog_tables WHERE warehouse_id='analytics' AND id='requests'",
    ).fetch_one(&pool).await?;
    assert_eq!(row, ("updating".into(), "carol".into(), "metrics".into()));
    let intent: (String, String, String) = sqlx::query_as(
        "SELECT operation,owner_id,namespace_id FROM catalog_pending_updates WHERE warehouse_id='analytics' AND table_id='requests'",
    ).fetch_one(&pool).await?;
    assert_eq!(
        intent,
        (
            operation.into(),
            expected_owner.into(),
            expected_namespace.into()
        )
    );
    let pending: i64 = sqlx::query_scalar("SELECT pending FROM authz_cache_state WHERE singleton")
        .fetch_one(&pool)
        .await?;
    assert_eq!(
        pending, 1,
        "killed remote write must retain its durable fence"
    );
    pool.close().await;
    stack.restart_dependencies(&client).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client, &app);
    let pool = stack.pool().await?;
    let row: (String, String, String) = sqlx::query_as(
        "SELECT status,owner_id,namespace_id FROM catalog_tables WHERE warehouse_id='analytics' AND id='requests'",
    ).fetch_one(&pool).await?;
    assert_eq!(
        row,
        (
            "active".into(),
            expected_owner.into(),
            expected_namespace.into()
        )
    );
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM catalog_pending_updates")
        .fetch_one(&pool)
        .await?;
    assert_eq!(
        remaining, 0,
        "durable lifecycle intent should be replayed once"
    );
    let pending: i64 = sqlx::query_scalar("SELECT pending FROM authz_cache_state WHERE singleton")
        .fetch_one(&pool)
        .await?;
    assert_eq!(
        pending, 1,
        "replay must never clear a previous process's ambiguous-write fence"
    );
    let tuples = tuple_keys(&stack).await?;
    assert_eq!(
        tuples.len(),
        before,
        "idempotent replacement must not add duplicate ownership or links"
    );
    let table = "lakekeeper_table:analytics/requests";
    assert!(tuples.contains(&(
        format!("user:{expected_owner}"),
        "ownership".into(),
        table.into()
    )));
    assert!(tuples.contains(&(
        format!("namespace:{expected_namespace}"),
        "parent".into(),
        table.into()
    )));
    assert!(tuples.contains(&(
        table.into(),
        "child".into(),
        format!("namespace:{expected_namespace}")
    )));
    if operation == "owner" {
        assert!(!tuples.contains(&("user:carol".into(), "ownership".into(), table.into())));
    } else {
        assert!(!tuples.contains(&("namespace:metrics".into(), "parent".into(), table.into())));
        assert!(!tuples.contains(&(table.into(), "child".into(), "namespace:metrics".into())));
    }
    // Metadata and tuples converge, but no live permission is returned until
    // an operator performs a separate quiesced reconciliation of the fence.
    closed(&api, &alice).await?;
    closed(&api, &carol).await?;
    let report: Value = serde_json::from_str(&run(stack.migration_command(), true).await?)?;
    assert_eq!(report["already_active"], true);
    closed(&api, &alice).await?;
    assert_eq!(tuple_count(&stack).await?, before);
    pool.close().await;
    app.stop().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn owner_transfer_replays_after_process_and_storage_restart_without_clearing_uncertain_write_fence()
-> TestResult {
    lost_runtime_relationship_acknowledgement("owner").await
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn namespace_move_replays_both_links_after_restart_without_clearing_uncertain_write_fence()
-> TestResult {
    lost_runtime_relationship_acknowledgement("namespace").await
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn namespace_create_permission_exposes_the_namespace_but_not_existing_table_metadata()
-> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let app = stack.app_unmigrated(&stack.fga_url).await?;
    let pool = stack.pool().await?;
    // A separate synthetic actor makes create-only behavior observable without
    // mixing in the fixture's direct selects, ownership, or inherited roles.
    sqlx::query("INSERT INTO users(id,display_name,password_hash) SELECT 'frank','Frank',password_hash FROM users WHERE id='dave'")
        .execute(&pool).await?;
    sqlx::query("INSERT INTO catalog_memberships(project_id,user_id) VALUES('acme','frank')")
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO legacy_resource_grants(object_kind,object_id,subject_kind,subject_id,relation) VALUES('namespace','metrics','user','frank','create')")
        .execute(&pool).await?;
    let frank = stack.login(&client, &app, "frank").await?;
    let api = Api::new(client.clone(), &app);
    closed(&api, &frank).await?;
    run(stack.migration_command(), true).await?;
    let catalog = expect(api.call(M::GET, "/catalog", &frank), S::OK).await?;
    let scopes = catalog["scopes"].as_array().ok_or("missing scopes")?;
    assert_eq!(scopes.len(), 1);
    assert_eq!(scopes[0]["warehouse_id"], "analytics");
    let namespaces: BTreeSet<_> = scopes[0]["namespaces"]
        .as_array()
        .ok_or("missing namespaces")?
        .iter()
        .map(|namespace| namespace["id"].as_str().unwrap())
        .collect();
    assert_eq!(namespaces, BTreeSet::from(["metrics"]));
    let config = stack.config()?;
    let store = config["store_id"].as_str().ok_or("missing store")?;
    let create: Value = client.post(format!("{}/stores/{store}/check",stack.fga_http_url))
        .json(&json!({"authorization_model_id":config["model_id"],"consistency":"HIGHER_CONSISTENCY",
            "tuple_key":{"user":"user:frank","relation":"can_create_table","object":"namespace:metrics"}}))
        .send().await?.error_for_status()?.json().await?;
    assert_eq!(create["allowed"], true);
    let page = expect(api.call(M::GET, TABLES, &frank), S::OK).await?;
    assert_eq!(ids(&page), BTreeSet::new());
    for id in ["requests", "latency", "errors"] {
        let path = format!("{TABLES}/{id}");
        expect(api.call(M::GET, &path, &frank), S::FORBIDDEN).await?;
        let permission = expect(
            api.call(M::POST, &format!("{path}/check"), &frank)
                .json(&json!({"action":"can_get_metadata"})),
            S::OK,
        )
        .await?;
        assert_eq!(permission["allowed"], false);
    }
    pool.close().await;
    app.stop().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn startup_validates_mode_and_pinned_model_before_sql_and_deny_skips_optional_migration()
-> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let pool = stack.pool().await?;
    let no_sql = || {
        sqlx::query_scalar::<_, bool>(
            "SELECT to_regclass('public.catalog_projects') IS NULL AND to_regclass('public._sqlx_migrations') IS NULL",
        )
    };
    assert!(no_sql().fetch_one(&pool).await?);
    let mut invalid_mode = stack.app_command(&stack.fga_url);
    invalid_mode
        .env("AUTHZ_MODE", "typo")
        .env("FGA_CONFIG", "/nonexistent-preflight-config.json");
    run(invalid_mode, false).await?;
    assert!(
        no_sql().fetch_one(&pool).await?,
        "unknown mode must fail before SQL DDL/seed"
    );
    let original = std::fs::read(stack.config_path())?;
    let mut config = stack.config()?;
    config["model_sha256"] = json!("0".repeat(64));
    std::fs::write(stack.config_path(), serde_json::to_vec(&config)?)?;
    run(stack.app_command(&stack.fga_url), false).await?;
    run(stack.migration_command(), false).await?;
    assert!(
        no_sql().fetch_one(&pool).await?,
        "model mismatch must fail before any SQL side effect"
    );
    std::fs::write(stack.config_path(), original)?;
    let mut deny = stack.app_command("http://127.0.0.1:1");
    deny.env("AUTHZ_MODE", "deny")
        .env("DEMO_MIGRATE", "true")
        .env("FGA_HTTP_URL", "http://127.0.0.1:1")
        .env("FGA_CONFIG", "/nonexistent-deny-config.json");
    let app = Stack::spawn_app(deny).await?;
    let alice = stack.login(&client, &app, "alice").await?;
    let api = Api::new(client, &app);
    expect(api.call(M::GET, REQUESTS, &alice), S::FORBIDDEN).await?;
    expect(api.call(M::GET, "/readyz", &alice), S::SERVICE_UNAVAILABLE).await?;
    assert_eq!(state(&pool).await?.0, "pending");
    assert_eq!(
        tuple_count(&stack).await?,
        0,
        "deny must not perform optional import or recovery"
    );
    pool.close().await;
    app.stop().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn delayed_before_forward_migration_retains_debt_after_activation_and_storage_restart()
-> TestResult {
    let client = client()?;
    let mut stack = Stack::start(&client).await?;
    let app = stack.app_unmigrated(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    let alice = stack.login(&client, &app, "alice").await?;
    let carol = stack.login(&client, &app, "carol").await?;
    let pool = stack.pool().await?;
    let proxy = Proxy::start(client.clone(), stack.fga_http_url.clone()).await?;
    let mut pause = proxy.pause_before_write(1).await;
    let mut command = stack.migration_command();
    command
        .env("FGA_URL", &proxy.url)
        .env("AUTHZ_MIGRATION_BATCH_SIZE", "100")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn()?;
    pause.captured().await?;
    let (_, next, total) = state(&pool).await?;
    assert_eq!(next, 0);
    assert!(
        total <= 100,
        "the held batch must include the complete seed graph"
    );
    assert_eq!(
        tuple_count(&stack).await?,
        0,
        "the old batch is retained before the actual OpenFGA transaction"
    );
    assert!(attempted(&pool).await?);
    child.kill().await?;
    child.wait().await?;

    // A clean-looking remote snapshot does not prove that the old request is
    // gone. The replay verifies its graph but retains an independent fence.
    run(stack.migration_command(), true).await?;
    assert_eq!(state(&pool).await?.0, "active");
    assert!(!attempted(&pool).await?);
    let debt = pending(&pool).await?;
    assert!(debt > 0);
    let graph = tuple_keys(&stack).await?;
    assert!(graph.contains(&(
        "user:carol".into(),
        "ownership".into(),
        "lakekeeper_table:analytics/requests".into(),
    )));
    closed(&api, &alice).await?;
    expect(
        api.call(M::PUT, &format!("{REQUESTS}/owner"), &carol)
            .json(&json!({"owner_id":"dave"})),
        S::SERVICE_UNAVAILABLE,
    )
    .await?;
    let owner: String = sqlx::query_scalar(
        "SELECT owner_id FROM catalog_tables WHERE warehouse_id='analytics' AND id='requests'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(owner, "carol");
    app.stop().await?;

    // Startup's active no-op must not discard the debt while the old request
    // can still arrive. Afterwards the deliberately delayed batch is delivered.
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    assert_eq!(pending(&pool).await?, debt);
    closed(&api, &carol).await?;
    // A missing or expired original request must not pass as a successful
    // late delivery just because the replay already produced this same graph.
    pause.release_and_wait_for_ack().await?;
    proxy.wait_idle().await?;
    assert_eq!(tuple_keys(&stack).await?, graph);
    closed(&api, &alice).await?;
    app.stop().await?;
    pool.close().await;
    drop(proxy);

    stack.restart_dependencies(&client).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client, &app);
    let pool = stack.pool().await?;
    assert_eq!(state(&pool).await?.0, "active");
    assert_eq!(pending(&pool).await?, debt);
    assert_eq!(tuple_keys(&stack).await?, graph);
    let report: Value = serde_json::from_str(&run(stack.migration_command(), true).await?)?;
    assert_eq!(report["already_active"], true);
    assert_eq!(pending(&pool).await?, debt);
    closed(&api, &alice).await?;
    closed(&api, &carol).await?;
    pool.close().await;
    app.stop().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn catalog_seed_cutover_and_active_gate_refuse_openlane_data_or_binding() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let app = stack.app_unmigrated(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    let alice = stack.login(&client, &app, "alice").await?;
    let pool = stack.pool().await?;

    // A nullable hash makes an accidental demo-seed UPSERT observable. Rejected
    // startup must preserve all existing identities before touching the source.
    sqlx::query("UPDATE users SET password_hash=NULL WHERE id='eve'")
        .execute(&pool)
        .await?;
    let identities: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT id,password_hash FROM users ORDER BY id")
            .fetch_all(&pool)
            .await?;

    sqlx::query(
        "INSERT INTO organizations(id,name,owner_id) VALUES ('foreign','Foreign scenario','alice')",
    )
    .execute(&pool)
    .await?;
    assert!(
        openfga_axum_lab::authorization_migration::seed(&pool)
            .await
            .is_err()
    );
    run(stack.app_command(&stack.fga_url), false).await?;
    let after_startup: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT id,password_hash FROM users ORDER BY id")
            .fetch_all(&pool)
            .await?;
    assert!(
        identities == after_startup,
        "foreign source changed identities"
    );
    run(stack.migration_command(), false).await?;
    assert_eq!(state(&pool).await?.0, "pending");
    assert_eq!(tuple_count(&stack).await?, 0);
    closed(&api, &alice).await?;
    sqlx::query("DELETE FROM organizations WHERE id='foreign'")
        .execute(&pool)
        .await?;

    // A foreign binding alone is enough to reject mixing, even when its
    // source is empty and its migration has not started sending tuples.
    let config = stack.config()?;
    sqlx::query("UPDATE openlane_authorization_migration SET store_id=$1 WHERE singleton")
        .bind(config["store_id"].as_str().ok_or("missing store")?)
        .execute(&pool)
        .await?;
    assert!(
        openfga_axum_lab::authorization_migration::seed(&pool)
            .await
            .is_err()
    );
    run(stack.app_command(&stack.fga_url), false).await?;
    let after_startup: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT id,password_hash FROM users ORDER BY id")
            .fetch_all(&pool)
            .await?;
    assert!(
        identities == after_startup,
        "foreign binding changed identities"
    );
    run(stack.migration_command(), false).await?;
    assert_eq!(state(&pool).await?.0, "pending");
    assert_eq!(tuple_count(&stack).await?, 0);
    sqlx::query("UPDATE openlane_authorization_migration SET store_id=NULL WHERE singleton")
        .execute(&pool)
        .await?;

    run(stack.migration_command(), true).await?;
    expect(api.call(M::GET, REQUESTS, &alice), S::OK).await?;
    let graph = tuple_keys(&stack).await?;
    sqlx::query(
        "INSERT INTO organizations(id,name,owner_id) VALUES ('foreign','Foreign scenario','alice')",
    )
    .execute(&pool)
    .await?;
    // The already-active shortcut and readiness must enforce the same domain
    // boundary as a fresh migration; none may replay or reinterpret the source.
    run(stack.migration_command(), false).await?;
    assert_eq!(state(&pool).await?.0, "active");
    assert_eq!(tuple_keys(&stack).await?, graph);
    closed(&api, &alice).await?;
    pool.close().await;
    app.stop().await?;
    Ok(())
}
