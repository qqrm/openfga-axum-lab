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

const TASKS: &str = "/tasks";
const ACCESS: &str = "/tasks/access-review";
const OWNER: &str = "/organizations/acme/owner";

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
        "SELECT status,next_tuple,tuple_count FROM openlane_authorization_migration WHERE singleton",
    )
    .fetch_one(pool)
    .await?)
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
    page.as_array()
        .expect("task array")
        .iter()
        .map(|row| row["id"].as_str().expect("task id"))
        .collect()
}
async fn closed(api: &Api, token: &str) -> TestResult {
    for path in [TASKS, ACCESS, "/organizations", "/readyz"] {
        expect(api.call(M::GET, path, token), S::SERVICE_UNAVAILABLE).await?;
    }
    Ok(())
}
async fn pending(pool: &PgPool) -> TestResult<i64> {
    Ok(
        sqlx::query_scalar("SELECT pending FROM authz_cache_state WHERE singleton")
            .fetch_one(pool)
            .await?,
    )
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
    let erin = stack.login(&client, &app, "erin").await?;
    closed(&api, &alice).await?;
    assert_eq!(tuple_count(&stack).await?, 0);
    let report: Value = serde_json::from_str(&run(stack.migration_command(), true).await?)?;
    assert_eq!(report["already_active"], false);
    assert_eq!(
        report["verified_checks"], 420,
        "all five users plus an outsider, every org/task relation"
    );
    for (token, expected) in [
        (
            &alice,
            BTreeSet::from(["access-review", "vendor-review", "incident-response"]),
        ),
        (
            &bob,
            BTreeSet::from(["access-review", "vendor-review", "incident-response"]),
        ),
        (
            &carol,
            BTreeSet::from(["access-review", "incident-response"]),
        ),
        (&dave, BTreeSet::from(["vendor-review"])),
        (&erin, BTreeSet::from(["other-review"])),
    ] {
        assert_eq!(
            ids(&expect(api.call(M::GET, TASKS, token), S::OK).await?),
            expected
        );
    }
    for (token, path) in [
        (&dave, ACCESS),
        (&erin, ACCESS),
        (&alice, "/tasks/other-review"),
    ] {
        expect(api.call(M::GET, path, token), S::FORBIDDEN).await?;
    }
    expect(
        api.call(M::PUT, OWNER, &bob)
            .json(&json!({"expected_owner":"alice","new_owner":"bob"})),
        S::FORBIDDEN,
    )
    .await?;
    expect(
        api.call(
            M::DELETE,
            "/organizations/acme/permissions/can_view_task/bob",
            &alice,
        ),
        S::NO_CONTENT,
    )
    .await?;
    expect(api.call(M::GET, ACCESS, &bob), S::FORBIDDEN).await?;
    assert_eq!(
        ids(&expect(api.call(M::GET, TASKS, &bob), S::OK).await?),
        BTreeSet::from(["incident-response"])
    );
    expect(
        api.call(M::PUT, "/organizations/acme/members/bob", &alice)
            .json(&json!({"role":"member"})),
        S::NO_CONTENT,
    )
    .await?;
    expect(api.call(M::POST,TASKS,&bob).header("Idempotency-Key","demoted-member").json(&json!({"organization_id":"acme","title":"Denied","description":"","status":"todo"})),S::FORBIDDEN).await?;
    expect(
        api.call(M::PUT, "/tasks/access-review/relations/editor/dave", &alice),
        S::NO_CONTENT,
    )
    .await?;
    let editable = expect(api.call(M::GET, ACCESS, &dave), S::OK).await?;
    assert_eq!(editable["can_edit"], true);
    assert_eq!(editable["can_delete"], false);
    expect(
        api.call(
            M::DELETE,
            "/tasks/vendor-review/relations/viewer/dave",
            &alice,
        ),
        S::NO_CONTENT,
    )
    .await?;
    expect(
        api.call(M::PUT, OWNER, &alice)
            .json(&json!({"expected_owner":"alice","new_owner":"carol"})),
        S::NO_CONTENT,
    )
    .await?;
    stack.assert_single_owner(&client, "acme", "carol").await?;
    expect(api.call(M::GET, ACCESS, &alice), S::FORBIDDEN).await?;
    expect(
        api.call(
            M::DELETE,
            "/tasks/access-review/relations/editor/dave",
            &carol,
        ),
        S::NO_CONTENT,
    )
    .await?;
    expect(api.call(M::GET, ACCESS, &dave), S::FORBIDDEN).await?;
    let before = tuple_keys(&stack).await?;
    assert!(before.contains(&(
        "organization:acme".into(),
        "parent_context".into(),
        "task:access-review".into()
    )));
    let rerun: Value = serde_json::from_str(&run(stack.migration_command(), true).await?)?;
    assert_eq!(rerun["already_active"], true);
    assert_eq!(
        tuple_keys(&stack).await?,
        before,
        "active rerun must never restore a revoked initial grant"
    );
    app.stop().await?;
    stack.restart_dependencies(&client).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    for token in [&alice, &bob, &dave] {
        expect(api.call(M::GET, ACCESS, token), S::FORBIDDEN).await?;
    }
    let owner = expect(api.call(M::GET, ACCESS, &carol), S::OK).await?;
    assert_eq!(owner["can_delete"], true);
    assert_eq!(owner["organization_id"], "acme");
    stack.assert_single_owner(&client, "acme", "carol").await?;
    assert_eq!(tuple_keys(&stack).await?, before);
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
    child.kill().await?;
    child.wait().await?;
    drop(paused);
    proxy.wait_idle().await?;
    assert_eq!(state(&pool).await?.0, "writing");
    assert_eq!(state(&pool).await?.1, 1);
    assert_eq!(tuple_count(&stack).await?, 2);
    closed(&api, &alice).await?;
    for statement in [
        "UPDATE organizations SET owner_id='carol' WHERE id='acme'",
        "DELETE FROM task_relations WHERE task_id='vendor-review'",
        "UPDATE users SET display_name='Changed' WHERE id='alice'",
    ] {
        assert!(
            sqlx::query(statement).execute(&pool).await.is_err(),
            "bound source must remain frozen after process death"
        );
    }
    let original = std::fs::read(stack.config_path())?;
    let directory = tempfile::tempdir()?;
    let alternate = directory.path().join("other-target.json");
    let mut provision = stack.bootstrap_command();
    provision.env("FGA_CONFIG", &alternate);
    run(provision, true).await?;
    let mut wrong = stack.migration_command();
    wrong.env("FGA_CONFIG", &alternate);
    run(wrong, false).await?;
    assert_eq!(std::fs::read(stack.config_path())?, original);
    assert_eq!(state(&pool).await?.1, 1);
    let hash: String = sqlx::query_scalar(
        "SELECT source_sha256 FROM openlane_authorization_migration WHERE singleton",
    )
    .fetch_one(&pool)
    .await?;
    sqlx::query("UPDATE openlane_authorization_migration SET source_sha256='tampered-source' WHERE singleton").execute(&pool).await?;
    run(stack.migration_command(), false).await?;
    assert_eq!(state(&pool).await?.1, 1);
    sqlx::query("UPDATE openlane_authorization_migration SET source_sha256=$1 WHERE singleton")
        .bind(hash)
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
    assert_eq!(report["verified_checks"], 420);
    assert_eq!(
        tuple_count(&stack).await?,
        report["tuple_count"].as_u64().unwrap() as usize
    );
    let pool = stack.pool().await?;
    let (status, next, total) = state(&pool).await?;
    assert_eq!(status, "active");
    assert_eq!(next, total);
    assert!(pending(&pool).await? > 0);
    // Verified binding does not clear an uncertain request from a prior process.
    closed(&api, &alice).await?;
    app.stop().await?;
    pool.close().await;
    proxy.wait_idle().await?;
    stack.recover_after_backend_drain(&stack.fga_url).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client, &app);
    expect(api.call(M::GET, ACCESS, &alice), S::OK).await?;
    expect(api.call(M::GET, "/readyz", &alice), S::OK).await?;
    app.stop().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn invalid_sql_subjects_roles_owners_and_foreign_domain_are_rejected_before_tuple_write()
-> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let app = stack.app_unmigrated(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    let alice = stack.login(&client, &app, "alice").await?;
    let pool = stack.pool().await?;
    // Structural invalidity is rejected by SQL first; independent import
    // validation also rejects legal SQL values that are unsafe FGA identifiers.
    for statement in [
        "INSERT INTO organization_members VALUES('acme','bob','owner')",
        "INSERT INTO task_relations VALUES('access-review','missing','viewer')",
        "INSERT INTO task_relations VALUES('access-review','bob','can_delete')",
    ] {
        assert!(sqlx::query(statement).execute(&pool).await.is_err());
    }
    for user in ["*", "role:team#member"] {
        sqlx::query("INSERT INTO users(id,display_name,password_hash) SELECT $1,'Invalid',password_hash FROM users WHERE id='bob'").bind(user).execute(&pool).await?;
        sqlx::query("INSERT INTO task_relations(task_id,user_id,relation) VALUES('access-review',$1,'viewer')").bind(user).execute(&pool).await?;
        run(stack.migration_command(), false).await?;
        assert_eq!(tuple_count(&stack).await?, 0);
        assert_eq!(state(&pool).await?.0, "pending");
        closed(&api, &alice).await?;
        sqlx::query("DELETE FROM task_relations WHERE user_id=$1")
            .bind(user)
            .execute(&pool)
            .await?;
        sqlx::query("DELETE FROM users WHERE id=$1")
            .bind(user)
            .execute(&pool)
            .await?;
    }
    sqlx::query("UPDATE organizations SET owner_id='erin' WHERE id='acme'")
        .execute(&pool)
        .await?;
    run(stack.migration_command(), false).await?;
    sqlx::query("UPDATE organizations SET owner_id='alice' WHERE id='acme'")
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE tasks SET status='pending' WHERE id='access-review'")
        .execute(&pool)
        .await?;
    run(stack.migration_command(), false).await?;
    sqlx::query("UPDATE tasks SET status='active' WHERE id='access-review'")
        .execute(&pool)
        .await?;
    // Historical resource grants deliberately lack an object FK. Their presence
    // is incompatible even when all catalog projects have already disappeared.
    sqlx::query("INSERT INTO legacy_resource_grants(object_kind,object_id,subject_kind,subject_id,relation) VALUES('lakekeeper_table','orphan/requests','user','bob','select')")
        .execute(&pool).await?;
    run(stack.migration_command(), false).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM legacy_resource_grants")
            .fetch_one(&pool)
            .await?,
        1,
        "an orphan historic grant must be preserved, never ignored or reset"
    );
    assert_eq!(tuple_count(&stack).await?, 0);
    assert_eq!(state(&pool).await?.0, "pending");
    closed(&api, &alice).await?;
    sqlx::query("DELETE FROM legacy_resource_grants WHERE object_id='orphan/requests'")
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO catalog_projects(id,name) VALUES('legacy','Preserved historic source')",
    )
    .execute(&pool)
    .await?;
    run(stack.migration_command(), false).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM catalog_projects")
            .fetch_one(&pool)
            .await?,
        1,
        "incompatible data must never be reset or reinterpreted"
    );
    assert_eq!(tuple_count(&stack).await?, 0);
    assert_eq!(state(&pool).await?.0, "pending");
    closed(&api, &alice).await?;
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
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT verified_checks FROM openlane_authorization_migration WHERE singleton"
        )
        .fetch_one(&pool)
        .await?,
        0
    );
    closed(&api, &alice).await?;
    let report: Value = serde_json::from_str(&run(stack.migration_command(), true).await?)?;
    assert_eq!(report["verified_checks"], 420);
    assert_eq!(state(&pool).await?.0, "active");
    assert!(pending(&pool).await? > 0);
    closed(&api, &alice).await?;
    app.stop().await?;
    proxy.wait_idle().await?;
    pool.close().await;
    stack.recover_after_backend_drain(&stack.fga_url).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client, &app);
    expect(api.call(M::GET, ACCESS, &alice), S::OK).await?;
    app.stop().await?;
    Ok(())
}

async fn lost_runtime_relationship_acknowledgement(operation: &str) -> TestResult {
    let client = client()?;
    let mut stack = Stack::start(&client).await?;
    let proxy = runtime_proxy::Proxy::start(client.clone(), stack.fga_url.clone()).await?;
    let app = stack.app(&proxy.url).await?;
    let api = Api::new(client.clone(), &app);
    let alice = stack.login(&client, &app, "alice").await?;
    let bob = stack.login(&client, &app, "bob").await?;
    let before = tuple_count(&stack).await?;
    let (path, payload) = match operation {
        "owner" => (OWNER, json!({"expected_owner":"alice","new_owner":"bob"})),
        "role" => ("/organizations/acme/members/bob", json!({"role":"member"})),
        _ => return Err("unsupported recovery test operation".into()),
    };
    let mut pause = proxy.pause("write", false).await;
    let request = api.call(M::PUT, path, &alice).json(&payload);
    let request = tokio::spawn(async move { request.send().await });
    pause.captured().await?;
    app.crash().await?;
    assert!(
        timeout(Duration::from_secs(10), request).await??.is_err(),
        "killed process cannot acknowledge a mutation"
    );
    drop(pause);
    proxy.wait_idle().await?;
    let pool = stack.pool().await?;
    let (is_pending, owner): (bool, String) =
        sqlx::query_as("SELECT authz_pending,owner_id FROM organizations WHERE id='acme'")
            .fetch_one(&pool)
            .await?;
    assert!(is_pending);
    assert_eq!(owner, if operation == "owner" { "bob" } else { "alice" });
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT attempted FROM authorization_jobs WHERE organization_id='acme'"
        )
        .fetch_one(&pool)
        .await?
    );
    assert_eq!(pending(&pool).await?, 1);
    pool.close().await;
    stack.restart_dependencies(&client).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    let pool = stack.pool().await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM authorization_jobs")
            .fetch_one(&pool)
            .await?,
        0
    );
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT authz_uncertain AND NOT authz_pending FROM organizations WHERE id='acme'"
        )
        .fetch_one(&pool)
        .await?
    );
    assert_eq!(
        pending(&pool).await?,
        2,
        "replay retains original cache debt and adds its durable intent marker"
    );
    let tuples = tuple_keys(&stack).await?;
    assert_eq!(tuples.len(), before);
    if operation == "owner" {
        assert!(tuples.contains(&(
            "user:bob".into(),
            "owner".into(),
            "organization:acme".into()
        )));
        assert!(!tuples.contains(&(
            "user:alice".into(),
            "owner".into(),
            "organization:acme".into()
        )));
    } else {
        assert!(tuples.contains(&(
            "user:bob".into(),
            "member".into(),
            "organization:acme".into()
        )));
        assert!(!tuples.contains(&(
            "user:bob".into(),
            "admin".into(),
            "organization:acme".into()
        )));
    }
    closed(&api, &alice).await?;
    closed(&api, &bob).await?;
    let report: Value = serde_json::from_str(&run(stack.migration_command(), true).await?)?;
    assert_eq!(report["already_active"], true);
    assert_eq!(tuple_keys(&stack).await?, tuples);
    closed(&api, &alice).await?;
    app.stop().await?;
    pool.close().await;
    proxy.wait_idle().await?;
    stack.recover_after_backend_drain(&stack.fga_url).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    expect(api.call(M::GET, ACCESS, &bob), S::OK).await?;
    let actor = if operation == "owner" { &bob } else { &alice };
    expect(
        api.call(M::GET, "/organizations/acme/members", actor),
        S::OK,
    )
    .await?;
    assert_eq!(tuple_keys(&stack).await?, tuples);
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
async fn role_replacement_replays_both_sides_after_restart_without_clearing_uncertain_write_fence()
-> TestResult {
    lost_runtime_relationship_acknowledgement("role").await
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn admin_create_permission_does_not_grant_existing_task_access_and_blocked_precedence_migrates_exactly()
-> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let app = stack.app_unmigrated(&stack.fga_url).await?;
    let pool = stack.pool().await?;
    sqlx::query("INSERT INTO users(id,display_name,password_hash) SELECT 'frank','Frank',password_hash FROM users WHERE id='dave'").execute(&pool).await?;
    sqlx::query("INSERT INTO organization_members(organization_id,user_id,role) VALUES('acme','frank','admin')").execute(&pool).await?;
    sqlx::query("INSERT INTO task_relations(task_id,user_id,relation) VALUES('access-review','carol','blocked'),('access-review','bob','editor'),('access-review','bob','blocked')").execute(&pool).await?;
    let frank = stack.login(&client, &app, "frank").await?;
    let api = Api::new(client.clone(), &app);
    closed(&api, &frank).await?;
    let report: Value = serde_json::from_str(&run(stack.migration_command(), true).await?)?;
    assert_eq!(report["verified_checks"], 490);
    let orgs = expect(api.call(M::GET, "/organizations", &frank), S::OK).await?;
    assert_eq!(orgs[0]["can_create_task"], true);
    assert_eq!(orgs[0]["can_manage"], false);
    assert_eq!(
        ids(&expect(api.call(M::GET, TASKS, &frank), S::OK).await?),
        BTreeSet::new()
    );
    for id in ["access-review", "vendor-review", "incident-response"] {
        expect(
            api.call(M::GET, &format!("/tasks/{id}"), &frank),
            S::FORBIDDEN,
        )
        .await?;
    }
    let config = stack.config()?;
    let store = config["store_id"].as_str().ok_or("missing store")?;
    for (user, relation, allowed) in [
        ("carol", "can_view", false),
        ("carol", "can_edit", true),
        ("carol", "can_delete", true),
        ("bob", "can_view", false),
        ("bob", "can_edit", false),
        ("bob", "can_delete", false),
    ] {
        let result:Value=client.post(format!("{}/stores/{store}/check",stack.fga_http_url)).json(&json!({"authorization_model_id":config["model_id"],"consistency":"HIGHER_CONSISTENCY","tuple_key":{"user":format!("user:{user}"),"relation":relation,"object":"task:access-review"}})).send().await?.error_for_status()?.json().await?;
        assert_eq!(
            result["allowed"], allowed,
            "exact upstream blocked/deleter precedence"
        );
    }
    let created=expect(api.call(M::POST,TASKS,&frank).header("Idempotency-Key","create-only-admin").json(&json!({"organization_id":"acme","title":"New task","description":"Admin may create and becomes assigner","status":"todo"})),S::CREATED).await?;
    let id = created["id"].as_str().ok_or("missing created task id")?;
    let created = expect(api.call(M::GET, &format!("/tasks/{id}"), &frank), S::OK).await?;
    assert_eq!(created["can_delete"], true);
    assert_eq!(created["assigner_id"], "frank");
    pool.close().await;
    app.stop().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn delayed_old_migration_batch_cannot_resurrect_owner_after_verified_resume() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let app = stack.app_unmigrated(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    let alice = stack.login(&client, &app, "alice").await?;
    let pool = stack.pool().await?;
    let proxy = Proxy::start(client.clone(), stack.fga_http_url.clone()).await?;
    let mut pause = proxy.pause_before_write(1).await;
    let mut command = stack.migration_command();
    command
        .env("FGA_URL", &proxy.url)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn()?;
    pause.captured().await?;
    assert_eq!(
        tuple_count(&stack).await?,
        0,
        "captured batch has not reached OpenFGA"
    );
    child.kill().await?;
    child.wait().await?;
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT attempted FROM openlane_authorization_migration WHERE singleton"
        )
        .fetch_one(&pool)
        .await?
    );
    run(stack.migration_command(), true).await?;
    assert_eq!(state(&pool).await?.0, "active");
    assert!(pending(&pool).await? > 0);
    stack.assert_single_owner(&client, "acme", "alice").await?;
    closed(&api, &alice).await?;
    expect(
        api.call(M::PUT, OWNER, &alice)
            .json(&json!({"expected_owner":"alice","new_owner":"bob"})),
        S::SERVICE_UNAVAILABLE,
    )
    .await?;
    // A delayed original request is deliberately delivered after replay. The
    // durable global debt prevents any conflicting owner/role write until the
    // queue is observably empty and all application writers have stopped.
    drop(pause);
    proxy.wait_idle().await?;
    app.stop().await?;
    stack.assert_single_owner(&client, "acme", "alice").await?;
    stack.recover_after_backend_drain(&stack.fga_url).await?;
    assert_eq!(pending(&pool).await?, 0);
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    expect(
        api.call(M::PUT, OWNER, &alice)
            .json(&json!({"expected_owner":"alice","new_owner":"bob"})),
        S::NO_CONTENT,
    )
    .await?;
    let before = tuple_keys(&stack).await?;
    run(stack.migration_command(), true).await?;
    assert_eq!(tuple_keys(&stack).await?, before);
    stack.assert_single_owner(&client, "acme", "bob").await?;
    expect(api.call(M::GET, ACCESS, &alice), S::FORBIDDEN).await?;
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
            "SELECT to_regclass('public.organizations') IS NULL AND to_regclass('public._sqlx_migrations') IS NULL",
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
    expect(api.call(M::GET, ACCESS, &alice), S::FORBIDDEN).await?;
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
