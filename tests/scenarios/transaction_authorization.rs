//! Revoke catalog authority while an authenticated handler waits for its SQL lock.
//!
//! The handler first takes the project lock, then waits for the table row lock.
//! Observe that PostgreSQL dependency instead of guessing from request timing.
//! An administrative fixture removes Carol's real ownership tuple while SQL
//! still names her as owner: only a fresh OpenFGA decision after acquiring the
//! locks may authorize the mutation. A normal API transfer takes the same
//! project lock, so the fixture deliberately writes to OpenFGA directly. Each
//! acknowledged fixture write advances the shared cache generation explicitly;
//! arbitrary out-of-band writes do not automatically invalidate the cache.

use crate::{
    api::{Api, expect},
    stack::{Stack, TestResult, client},
};
use reqwest::{Client, Method as M, RequestBuilder, StatusCode as S};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{collections::BTreeSet, time::Duration};
use tokio::time::{interval, timeout};

const TABLE_PATH: &str = "/projects/acme/warehouses/analytics/tables/requests";
const GRANT_PATH: &str = "/projects/acme/warehouses/analytics/tables/requests/grants/select";
const TABLE_OBJECT: &str = "lakekeeper_table:analytics/requests";

#[derive(Clone, Copy, Debug)]
enum Mutation {
    Edit,
    Delete,
    GrantSelect,
    RevokeSelect,
}

#[derive(serde::Deserialize)]
struct TuplePage {
    #[serde(default)]
    tuples: Vec<Value>,
    #[serde(default)]
    continuation_token: String,
}

fn edit_body() -> Value {
    json!({"name": "Reauthorized table", "description": "Ownership access restored."})
}

impl Mutation {
    fn request(self, api: &Api, token: &str) -> RequestBuilder {
        let subject = json!({"subject": {"type": "user", "id": "dave"}});
        let request = match self {
            Self::Edit => api.call(M::PATCH, TABLE_PATH, token).json(&edit_body()),
            Self::Delete => api.call(M::DELETE, TABLE_PATH, token),
            Self::GrantSelect => api.call(M::PUT, GRANT_PATH, token).json(&subject),
            Self::RevokeSelect => api.call(M::DELETE, GRANT_PATH, token).json(&subject),
        };
        request.timeout(Duration::from_secs(30))
    }

    fn permission(self) -> &'static str {
        match self {
            Self::Edit => "can_commit",
            Self::Delete => "can_drop",
            Self::GrantSelect => "can_grant_select",
            Self::RevokeSelect => "can_revoke_select",
        }
    }

    fn handler_query(self) -> &'static str {
        match self {
            Self::Delete => {
                "SELECT status,delete_actor,namespace_id FROM catalog_tables WHERE project_id=$1 AND warehouse_id=$2 AND id=$3 FOR UPDATE"
            }
            _ => {
                "SELECT id,project_id,warehouse_id,namespace_id,owner_id,name,description,status FROM catalog_tables WHERE project_id=$1 AND warehouse_id=$2 AND id=$3 FOR UPDATE"
            }
        }
    }
}

async fn wait_for_handler(pool: &PgPool, blocker: i32, query: &str) -> TestResult {
    timeout(Duration::from_secs(10), async {
        let mut poll = interval(Duration::from_millis(20));
        loop {
            poll.tick().await;
            let blocked: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity \
                 WHERE datname = current_database() AND state = 'active' \
                 AND wait_event_type = 'Lock' AND query = $1 \
                 AND $2 = ANY(pg_blocking_pids(pid)))",
            )
            .bind(query)
            .bind(blocker)
            .fetch_one(pool)
            .await?;
            if blocked {
                return Ok::<(), sqlx::Error>(());
            }
        }
    })
    .await
    .map_err(|error| format!("handler did not reach its row lock: {query}: {error}"))??;
    Ok(())
}

async fn table_row(pool: &PgPool) -> TestResult<Value> {
    let row: String =
        sqlx::query_scalar("SELECT row_to_json(catalog_tables)::text FROM catalog_tables WHERE project_id='acme' AND warehouse_id='analytics' AND id='requests'")
            .fetch_one(pool)
            .await?;
    Ok(serde_json::from_str(&row)?)
}

async fn table_tuples(client: &Client, stack: &Stack) -> TestResult<Vec<Value>> {
    let config = stack.config()?;
    let store = config["store_id"].as_str().ok_or("missing store id")?;
    let mut cursor = String::new();
    let mut seen = BTreeSet::new();
    let mut tuples = Vec::new();
    loop {
        let response: TuplePage = client
            .post(format!("{}/stores/{store}/read", stack.fga_http_url))
            .json(&json!({"page_size": 100, "continuation_token": cursor,
                "consistency": "HIGHER_CONSISTENCY"}))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        for tuple in response.tuples {
            let mut key = tuple["key"].clone();
            let fields = key.as_object_mut().ok_or("missing tuple key object")?;
            // Include the reverse namespace child tuple as well as the table's
            // own relationships, so successful deletion must remove both sides.
            if fields.get("object").and_then(Value::as_str) != Some(TABLE_OBJECT)
                && fields.get("user").and_then(Value::as_str) != Some(TABLE_OBJECT)
            {
                continue;
            }
            // Protobuf JSON may encode an absent message as null. Preserve all
            // other fields and reject conditional fixtures instead of ignoring them.
            let condition = fields.remove("condition");
            assert!(
                condition.as_ref().is_none_or(Value::is_null),
                "fixture requires an unconditional tuple: condition={condition:?}, key={key}"
            );
            tuples.push(key);
        }
        if response.continuation_token.is_empty() {
            break;
        }
        assert!(
            seen.insert(response.continuation_token.clone()),
            "repeating tuple snapshot cursor"
        );
        cursor = response.continuation_token;
    }
    tuples.sort_unstable_by_key(Value::to_string);
    Ok(tuples)
}

async fn owner_allowed(client: &Client, stack: &Stack, permission: &str) -> TestResult<bool> {
    let config = stack.config()?;
    let store = config["store_id"].as_str().ok_or("missing store id")?;
    let response: Value = client
        .post(format!("{}/stores/{store}/check", stack.fga_http_url))
        .json(&json!({"authorization_model_id": config["model_id"],
        "consistency": "HIGHER_CONSISTENCY", "tuple_key": {
            "user": "user:carol", "relation": permission, "object": TABLE_OBJECT
        }}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    response["allowed"]
        .as_bool()
        .ok_or("missing decision".into())
}

fn owner_tuple() -> Value {
    json!({"user": "user:carol", "relation": "ownership", "object": TABLE_OBJECT})
}

fn select_tuple() -> Value {
    json!({"user": "user:dave", "relation": "select", "object": TABLE_OBJECT})
}

async fn set_ownership(client: &Client, stack: &Stack, pool: &PgPool, add: bool) -> TestResult {
    let config = stack.config()?;
    let store = config["store_id"].as_str().ok_or("missing store id")?;
    let mut body = json!({"authorization_model_id": config["model_id"]});
    body[if add { "writes" } else { "deletes" }] = json!({"tuple_keys": [owner_tuple()]});
    client
        .post(format!("{}/stores/{store}/write", stack.fga_http_url))
        .json(&body)
        .send()
        .await?
        .error_for_status()?;
    // Only the blocked request is in flight. Complete this externally
    // coordinated ownership change without waiting for the denial cache's TTL.
    let changed = sqlx::query(
        "UPDATE authz_cache_state SET generation = generation + 1 WHERE singleton AND pending = 0",
    )
    .execute(pool)
    .await?
    .rows_affected();
    assert_eq!(
        changed, 1,
        "ownership fixture requires a quiescent cache fence"
    );
    Ok(())
}

async fn exercise_revocation(mutation: Mutation) -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    let carol = stack.login(&client, &app, "carol").await?;
    let dave = stack.login(&client, &app, "dave").await?;
    let pool = stack.pool().await?;
    if matches!(mutation, Mutation::RevokeSelect) {
        expect(Mutation::GrantSelect.request(&api, &carol), S::NO_CONTENT).await?;
    }
    assert!(owner_allowed(&client, &stack, mutation.permission()).await?);
    let original_row = table_row(&pool).await?;
    assert_eq!(original_row["owner_id"], json!("carol"));
    let original_projection = stack.domain_snapshot().await?;
    let original_tuples = table_tuples(&client, &stack).await?;
    assert!(
        original_tuples.contains(&owner_tuple()),
        "missing Carol ownership in real OpenFGA tuples: {original_tuples:#?}"
    );
    assert!(
        original_tuples.contains(&json!({
            "user": TABLE_OBJECT, "relation": "child", "object": "namespace:metrics"
        })),
        "fixture must include the reverse namespace link"
    );

    let mut lock = pool.begin().await?;
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *lock)
        .await?;
    sqlx::query("SELECT id FROM catalog_tables WHERE project_id='acme' AND warehouse_id='analytics' AND id='requests' FOR UPDATE")
        .execute(&mut *lock)
        .await?;
    let request = mutation.request(&api, &carol);
    let pending = tokio::spawn(async move { expect(request, S::FORBIDDEN).await });
    wait_for_handler(&pool, blocker, mutation.handler_query()).await?;
    assert!(
        !pending.is_finished(),
        "request must still wait for the lock"
    );
    println!(
        "{mutation:?}: authenticated handler holds the project lock and waits for table-lock backend {blocker}"
    );

    set_ownership(&client, &stack, &pool, false).await?;
    assert!(!owner_allowed(&client, &stack, mutation.permission()).await?);
    let denied_tuples: Vec<_> = original_tuples
        .iter()
        .filter(|tuple| **tuple != owner_tuple())
        .cloned()
        .collect();
    assert_eq!(table_tuples(&client, &stack).await?, denied_tuples);
    lock.commit().await?;
    pending.await??;
    assert_eq!(table_row(&pool).await?, original_row, "denial mutated SQL");
    assert_eq!(
        stack.domain_snapshot().await?,
        original_projection,
        "denial mutated catalog state, membership or durable intent"
    );
    assert_eq!(
        table_tuples(&client, &stack).await?,
        denied_tuples,
        "denial mutated authorization tuples"
    );

    // The identical request succeeds once ownership authority is restored, so
    // a broken dependency, invalid payload or permanently denied fixture cannot pass.
    set_ownership(&client, &stack, &pool, true).await?;
    assert!(owner_allowed(&client, &stack, mutation.permission()).await?);
    expect(mutation.request(&api, &carol), S::NO_CONTENT).await?;
    let mut expected_row = original_row;
    let mut expected_tuples = original_tuples;
    match mutation {
        Mutation::Edit => {
            let edited = edit_body();
            for field in ["name", "description"] {
                expected_row[field] = edited[field].clone();
            }
            let table = expect(api.call(M::GET, TABLE_PATH, &carol), S::OK).await?;
            assert_eq!(table["name"], edited["name"]);
            assert_eq!(table["description"], edited["description"]);
        }
        Mutation::Delete => {
            expected_row["status"] = json!("deleted");
            expected_row["delete_actor"] = json!("carol");
            expected_tuples.clear();
            expect(api.call(M::GET, TABLE_PATH, &carol), S::NOT_FOUND).await?;
        }
        Mutation::GrantSelect => {
            expected_tuples.push(select_tuple());
            expected_tuples.sort_unstable_by_key(Value::to_string);
            expect(api.call(M::GET, TABLE_PATH, &dave), S::OK).await?;
        }
        Mutation::RevokeSelect => {
            expected_tuples.retain(|tuple| *tuple != select_tuple());
            expect(api.call(M::GET, TABLE_PATH, &dave), S::FORBIDDEN).await?;
        }
    }
    assert_eq!(table_row(&pool).await?, expected_row);
    assert_eq!(table_tuples(&client, &stack).await?, expected_tuples);
    pool.close().await;
    app.stop().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn edit_rechecks_authorization_after_waiting_for_table_lock() -> TestResult {
    exercise_revocation(Mutation::Edit).await
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn delete_rechecks_authorization_after_waiting_for_table_lock() -> TestResult {
    exercise_revocation(Mutation::Delete).await
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn select_grant_rechecks_authorization_after_waiting_for_table_lock() -> TestResult {
    exercise_revocation(Mutation::GrantSelect).await
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn select_revoke_rechecks_authorization_after_waiting_for_table_lock() -> TestResult {
    exercise_revocation(Mutation::RevokeSelect).await
}
