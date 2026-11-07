//! Revoke access after the HTTP guard succeeds but before the handler checks it.
//!
//! A real PostgreSQL row lock holds the handler at SELECT ... FOR UPDATE. We
//! observe its lock dependency, rather than guessing when middleware finished.
//! Owner changes are administrative fixtures because the API only changes
//! viewer/caregiver relations. Each completed real OpenFGA write explicitly
//! advances the shared cache generation; this does not claim that arbitrary
//! out-of-band OpenFGA writes automatically invalidate cached decisions.

use crate::{
    api::{Api, expect},
    stack::{Stack, TestResult, client},
};
use reqwest::{Client, Method as M, RequestBuilder, StatusCode as S};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
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
    json!({"name": "Reauthorized fox", "species": "fox", "biography": "Owner access restored."})
}

impl Mutation {
    fn request(self, api: &Api, token: &str) -> RequestBuilder {
        let request = match self {
            Self::Edit => api.call(M::PATCH, "/pets/fox", token).json(&edit_body()),
            Self::Delete => api.call(M::DELETE, "/pets/fox", token),
            Self::GrantViewer => api.call(M::PUT, "/pets/fox/relations/viewer/dave", token),
            Self::RevokeViewer => api.call(M::DELETE, "/pets/fox/relations/viewer/dave", token),
        };
        request.timeout(Duration::from_secs(30))
    }

    fn permission(self) -> &'static str {
        match self {
            Self::Edit => "can_edit",
            _ => "can_manage",
        }
    }

    fn handler_query(self) -> &'static str {
        match self {
            Self::Delete => "SELECT status,delete_actor FROM pets WHERE id=$1 FOR UPDATE",
            _ => "SELECT status FROM pets WHERE id=$1 FOR UPDATE",
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

async fn fox_row(pool: &PgPool) -> TestResult<Value> {
    let row: String = sqlx::query_scalar("SELECT row_to_json(pets)::text FROM pets WHERE id='fox'")
        .fetch_one(pool)
        .await?;
    Ok(serde_json::from_str(&row)?)
}

async fn fox_tuples(client: &Client, stack: &Stack) -> TestResult<Vec<Value>> {
    let config = stack.config()?;
    let store = config["store_id"].as_str().ok_or("missing store id")?;
    let response: TuplePage = client
        .post(format!("{}/stores/{store}/read", stack.fga_url))
        .json(
            &json!({"tuple_key": {"object": "pet:fox"}, "page_size": 100,
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

async fn owner_allowed(client: &Client, stack: &Stack, permission: &str) -> TestResult<bool> {
    let config = stack.config()?;
    let store = config["store_id"].as_str().ok_or("missing store id")?;
    let response: Value = client
        .post(format!("{}/stores/{store}/check", stack.fga_url))
        .json(&json!({"authorization_model_id": config["model_id"],
        "consistency": "HIGHER_CONSISTENCY", "tuple_key": {
            "user": "user:carol", "relation": permission, "object": "pet:fox"
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
    json!({"user": "user:carol", "relation": "owner", "object": "pet:fox"})
}

fn viewer_tuple() -> Value {
    json!({"user": "user:dave", "relation": "viewer", "object": "pet:fox"})
}

async fn set_owner(client: &Client, stack: &Stack, pool: &PgPool, add: bool) -> TestResult {
    let config = stack.config()?;
    let store = config["store_id"].as_str().ok_or("missing store id")?;
    let mut body = json!({"authorization_model_id": config["model_id"]});
    body[if add { "writes" } else { "deletes" }] = json!({"tuple_keys": [owner_tuple()]});
    client
        .post(format!("{}/stores/{store}/write", stack.fga_url))
        .json(&body)
        .send()
        .await?
        .error_for_status()?;
    // Only the blocked request is in flight. Establish a completed, externally
    // coordinated owner change; do not wait for the decision cache's TTL.
    let changed = sqlx::query(
        "UPDATE authz_cache_state SET generation = generation + 1 WHERE singleton AND pending = 0",
    )
    .execute(pool)
    .await?
    .rows_affected();
    assert_eq!(changed, 1, "owner fixture requires a quiescent cache fence");
    Ok(())
}

async fn exercise_revocation(mutation: Mutation) -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let app = stack.app(&stack.fga_url).await?;
    let api = Api::new(client.clone(), &app);
    let carol = stack.login(&client, &app, "carol").await?;
    let dave = stack.login(&client, &app, "dave").await?;
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .acquire_timeout(Duration::from_secs(5))
        .connect(stack.database_url())
        .await?;
    if matches!(mutation, Mutation::RevokeViewer) {
        expect(
            api.call(M::PUT, "/pets/fox/relations/viewer/dave", &carol),
            S::NO_CONTENT,
        )
        .await?;
    }
    assert!(owner_allowed(&client, &stack, mutation.permission()).await?);
    let original_row = fox_row(&pool).await?;
    let original_tuples = fox_tuples(&client, &stack).await?;
    assert!(
        original_tuples.contains(&owner_tuple()),
        "missing Carol owner in real OpenFGA tuples: {original_tuples:#?}"
    );

    let mut lock = pool.begin().await?;
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *lock)
        .await?;
    sqlx::query("SELECT id FROM pets WHERE id='fox' FOR UPDATE")
        .execute(&mut *lock)
        .await?;
    let request = mutation.request(&api, &carol);
    let pending = tokio::spawn(async move { expect(request, S::FORBIDDEN).await });
    wait_for_handler(&pool, blocker, mutation.handler_query()).await?;
    assert!(
        !pending.is_finished(),
        "request must still wait for the lock"
    );
    println!("{mutation:?}: HTTP guard passed; handler waits for PostgreSQL backend {blocker}");

    set_owner(&client, &stack, &pool, false).await?;
    assert!(!owner_allowed(&client, &stack, mutation.permission()).await?);
    let denied_tuples: Vec<_> = original_tuples
        .iter()
        .filter(|tuple| **tuple != owner_tuple())
        .cloned()
        .collect();
    assert_eq!(fox_tuples(&client, &stack).await?, denied_tuples);
    lock.commit().await?;
    pending.await??;
    assert_eq!(fox_row(&pool).await?, original_row, "denial mutated SQL");
    assert_eq!(
        fox_tuples(&client, &stack).await?,
        denied_tuples,
        "denial mutated authorization tuples"
    );

    // The identical request succeeds once ownership is restored, so a broken
    // dependency, invalid payload or permanently denied fixture cannot pass.
    set_owner(&client, &stack, &pool, true).await?;
    assert!(owner_allowed(&client, &stack, mutation.permission()).await?);
    expect(mutation.request(&api, &carol), S::NO_CONTENT).await?;
    let mut expected_row = original_row;
    let mut expected_tuples = original_tuples;
    match mutation {
        Mutation::Edit => {
            let edited = edit_body();
            for field in ["name", "species", "biography"] {
                expected_row[field] = edited[field].clone();
            }
            let pet = expect(api.call(M::GET, "/pets/fox", &carol), S::OK).await?;
            assert_eq!(pet["biography"], edited["biography"]);
        }
        Mutation::Delete => {
            expected_row["status"] = json!("deleted");
            expected_row["delete_actor"] = json!("carol");
            expected_tuples.clear();
            expect(api.call(M::GET, "/pets/fox", &carol), S::NOT_FOUND).await?;
        }
        Mutation::GrantViewer => {
            expected_tuples.push(viewer_tuple());
            expected_tuples.sort_unstable_by_key(Value::to_string);
            expect(api.call(M::GET, "/pets/fox", &dave), S::OK).await?;
        }
        Mutation::RevokeViewer => {
            expected_tuples.retain(|tuple| *tuple != viewer_tuple());
            expect(api.call(M::GET, "/pets/fox", &dave), S::FORBIDDEN).await?;
        }
    }
    assert_eq!(fox_row(&pool).await?, expected_row);
    assert_eq!(fox_tuples(&client, &stack).await?, expected_tuples);
    pool.close().await;
    app.stop().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn edit_rechecks_authorization_after_waiting_for_pet_lock() -> TestResult {
    exercise_revocation(Mutation::Edit).await
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn delete_rechecks_authorization_after_waiting_for_pet_lock() -> TestResult {
    exercise_revocation(Mutation::Delete).await
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn relation_grant_rechecks_authorization_after_waiting_for_pet_lock() -> TestResult {
    exercise_revocation(Mutation::GrantViewer).await
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn relation_revoke_rechecks_authorization_after_waiting_for_pet_lock() -> TestResult {
    exercise_revocation(Mutation::RevokeViewer).await
}
