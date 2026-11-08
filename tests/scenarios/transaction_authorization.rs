//! Revoke task authority while an authenticated handler waits for its SQL lock.
//!
//! A real PostgreSQL row lock holds the handler at SELECT ... FOR UPDATE. We
//! observe its lock dependency, rather than guessing when middleware finished.
//! Assigner changes are administrative fixtures because the task API does not
//! replace its immutable assigner. Each completed real OpenFGA write explicitly
//! advances the shared cache generation; this does not claim that arbitrary
//! out-of-band OpenFGA writes automatically invalidate cached decisions.

use crate::{
    api::{Api, expect},
    stack::{Stack, TestResult, client},
};
use reqwest::{Client, Method as M, RequestBuilder, StatusCode as S};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::time::Duration;
use tokio::time::{interval, timeout};

#[derive(Clone, Copy, Debug)]
enum Mutation {
    Edit,
    Delete,
    GrantViewer,
    RevokeViewer,
}

#[derive(serde::Deserialize)]
struct TuplePage {
    #[serde(default)]
    tuples: Vec<Value>,
    #[serde(default)]
    continuation_token: String,
}

fn edit_body() -> Value {
    json!({"title": "Reauthorized task", "description": "Assigner access restored.", "status": "done"})
}

impl Mutation {
    fn request(self, api: &Api, token: &str) -> RequestBuilder {
        let request = match self {
            Self::Edit => api
                .call(M::PATCH, "/tasks/access-review", token)
                .json(&edit_body()),
            Self::Delete => api.call(M::DELETE, "/tasks/access-review", token),
            Self::GrantViewer => {
                api.call(M::PUT, "/tasks/access-review/relations/viewer/dave", token)
            }
            Self::RevokeViewer => api.call(
                M::DELETE,
                "/tasks/access-review/relations/viewer/dave",
                token,
            ),
        };
        request.timeout(Duration::from_secs(30))
    }

    fn permission(self) -> &'static str {
        match self {
            Self::Edit => "can_edit",
            _ => "can_delete",
        }
    }

    fn handler_query(self) -> &'static str {
        match self {
            Self::Delete => "SELECT status,delete_actor FROM tasks WHERE id=$1 FOR UPDATE",
            _ => "SELECT status FROM tasks WHERE id=$1 FOR UPDATE",
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

async fn task_row(pool: &PgPool) -> TestResult<Value> {
    let row: String =
        sqlx::query_scalar("SELECT row_to_json(tasks)::text FROM tasks WHERE id='access-review'")
            .fetch_one(pool)
            .await?;
    Ok(serde_json::from_str(&row)?)
}

async fn task_tuples(client: &Client, stack: &Stack) -> TestResult<Vec<Value>> {
    let config = stack.config()?;
    let store = config["store_id"].as_str().ok_or("missing store id")?;
    let response: TuplePage = client
        .post(format!("{}/stores/{store}/read", stack.fga_http_url))
        .json(
            &json!({"tuple_key": {"object": "task:access-review"}, "page_size": 100,
            "consistency": "HIGHER_CONSISTENCY"}),
        )
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(
        response.continuation_token.is_empty(),
        "fixture tuple snapshot must contain every page"
    );
    let mut tuples = Vec::with_capacity(response.tuples.len());
    for tuple in response.tuples {
        let mut key = tuple["key"].clone();
        let fields = key.as_object_mut().ok_or("missing tuple key object")?;
        // The protobuf JSON response may include an absent message as null.
        // Preserve every other key field and reject conditional fixture tuples.
        let condition = fields.remove("condition");
        assert!(
            condition.as_ref().is_none_or(Value::is_null),
            "fixture requires an unconditional tuple: condition={condition:?}, key={key}"
        );
        tuples.push(key);
    }
    tuples.sort_unstable_by_key(Value::to_string);
    Ok(tuples)
}

async fn assigner_allowed(client: &Client, stack: &Stack, permission: &str) -> TestResult<bool> {
    let config = stack.config()?;
    let store = config["store_id"].as_str().ok_or("missing store id")?;
    let response: Value = client
        .post(format!("{}/stores/{store}/check", stack.fga_http_url))
        .json(&json!({"authorization_model_id": config["model_id"],
        "consistency": "HIGHER_CONSISTENCY", "tuple_key": {
            "user": "user:carol", "relation": permission, "object": "task:access-review"
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

fn assigner_tuple() -> Value {
    json!({"user": "user:carol", "relation": "assigner", "object": "task:access-review"})
}

fn viewer_tuple() -> Value {
    json!({"user": "user:dave", "relation": "viewer", "object": "task:access-review"})
}

async fn set_assigner(client: &Client, stack: &Stack, pool: &PgPool, add: bool) -> TestResult {
    let config = stack.config()?;
    let store = config["store_id"].as_str().ok_or("missing store id")?;
    let mut body = json!({"authorization_model_id": config["model_id"]});
    body[if add { "writes" } else { "deletes" }] = json!({"tuple_keys": [assigner_tuple()]});
    client
        .post(format!("{}/stores/{store}/write", stack.fga_http_url))
        .json(&body)
        .send()
        .await?
        .error_for_status()?;
    // Only the blocked request is in flight. Establish a completed, externally
    // coordinated assigner change; do not wait for the decision cache's TTL.
    let changed = sqlx::query(
        "UPDATE authz_cache_state SET generation = generation + 1 WHERE singleton AND pending = 0",
    )
    .execute(pool)
    .await?
    .rows_affected();
    assert_eq!(
        changed, 1,
        "assigner fixture requires a quiescent cache fence"
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
    if matches!(mutation, Mutation::RevokeViewer) {
        expect(
            api.call(M::PUT, "/tasks/access-review/relations/viewer/dave", &carol),
            S::NO_CONTENT,
        )
        .await?;
    }
    assert!(assigner_allowed(&client, &stack, mutation.permission()).await?);
    let original_row = task_row(&pool).await?;
    let original_projection = stack.domain_snapshot().await?;
    let original_tuples = task_tuples(&client, &stack).await?;
    assert!(
        original_tuples.contains(&assigner_tuple()),
        "missing Carol assigner in real OpenFGA tuples: {original_tuples:#?}"
    );

    let mut lock = pool.begin().await?;
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *lock)
        .await?;
    sqlx::query("SELECT id FROM tasks WHERE id='access-review' FOR UPDATE")
        .execute(&mut *lock)
        .await?;
    let request = mutation.request(&api, &carol);
    let pending = tokio::spawn(async move { expect(request, S::FORBIDDEN).await });
    wait_for_handler(&pool, blocker, mutation.handler_query()).await?;
    assert!(
        !pending.is_finished(),
        "request must still wait for the lock"
    );
    println!("{mutation:?}: authentication passed; handler waits for PostgreSQL backend {blocker}");

    set_assigner(&client, &stack, &pool, false).await?;
    assert!(!assigner_allowed(&client, &stack, mutation.permission()).await?);
    let denied_tuples: Vec<_> = original_tuples
        .iter()
        .filter(|tuple| **tuple != assigner_tuple())
        .cloned()
        .collect();
    assert_eq!(task_tuples(&client, &stack).await?, denied_tuples);
    lock.commit().await?;
    pending.await??;
    assert_eq!(task_row(&pool).await?, original_row, "denial mutated SQL");
    assert_eq!(
        stack.domain_snapshot().await?,
        original_projection,
        "denial mutated desired grants, membership or durable intent"
    );
    assert_eq!(
        task_tuples(&client, &stack).await?,
        denied_tuples,
        "denial mutated authorization tuples"
    );

    // The identical request succeeds once assigner authority is restored, so a broken
    // dependency, invalid payload or permanently denied fixture cannot pass.
    set_assigner(&client, &stack, &pool, true).await?;
    assert!(assigner_allowed(&client, &stack, mutation.permission()).await?);
    expect(mutation.request(&api, &carol), S::NO_CONTENT).await?;
    let mut expected_row = original_row;
    let mut expected_tuples = original_tuples;
    match mutation {
        Mutation::Edit => {
            let edited = edit_body();
            for field in ["title", "description"] {
                expected_row[field] = edited[field].clone();
            }
            expected_row["task_status"] = edited["status"].clone();
            let task = expect(api.call(M::GET, "/tasks/access-review", &carol), S::OK).await?;
            assert_eq!(task["description"], edited["description"]);
        }
        Mutation::Delete => {
            expected_row["status"] = json!("deleted");
            expected_row["delete_actor"] = json!("carol");
            expected_tuples.clear();
            expect(
                api.call(M::GET, "/tasks/access-review", &carol),
                S::NOT_FOUND,
            )
            .await?;
        }
        Mutation::GrantViewer => {
            expected_tuples.push(viewer_tuple());
            expected_tuples.sort_unstable_by_key(Value::to_string);
            expect(api.call(M::GET, "/tasks/access-review", &dave), S::OK).await?;
        }
        Mutation::RevokeViewer => {
            expected_tuples.retain(|tuple| *tuple != viewer_tuple());
            expect(
                api.call(M::GET, "/tasks/access-review", &dave),
                S::FORBIDDEN,
            )
            .await?;
        }
    }
    assert_eq!(task_row(&pool).await?, expected_row);
    assert_eq!(task_tuples(&client, &stack).await?, expected_tuples);
    pool.close().await;
    app.stop().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn edit_rechecks_authorization_after_waiting_for_task_lock() -> TestResult {
    exercise_revocation(Mutation::Edit).await
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn delete_rechecks_authorization_after_waiting_for_task_lock() -> TestResult {
    exercise_revocation(Mutation::Delete).await
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn relation_grant_rechecks_authorization_after_waiting_for_task_lock() -> TestResult {
    exercise_revocation(Mutation::GrantViewer).await
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn relation_revoke_rechecks_authorization_after_waiting_for_task_lock() -> TestResult {
    exercise_revocation(Mutation::RevokeViewer).await
}
