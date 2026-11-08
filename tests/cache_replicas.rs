#[path = "cache_support/proxy.rs"]
mod proxy;
#[path = "support/stack.rs"]
pub mod stack;

use proxy::Proxy;
use reqwest::{Client, Method, StatusCode};
use serde_json::Value;
use stack::{App, Stack, TestResult};
use std::time::Duration;

const TASK: &str = "/tasks/access-review";
const RELATION: &str = "/tasks/access-review/relations/assignee/dave";

struct Scenario {
    client: Client,
    carol: String,
    dave: String,
}

impl Scenario {
    async fn check(&self, app: &App, expected: StatusCode) -> TestResult {
        self.expect(app, Method::GET, TASK, &self.dave, expected)
            .await
    }

    async fn grant(&self, app: &App) -> TestResult {
        self.expect(
            app,
            Method::PUT,
            RELATION,
            &self.carol,
            StatusCode::NO_CONTENT,
        )
        .await
    }

    async fn revoke(&self, app: &App, expected: StatusCode) -> TestResult {
        self.expect(app, Method::DELETE, RELATION, &self.carol, expected)
            .await
    }

    async fn expect(
        &self,
        app: &App,
        method: Method,
        path: &str,
        token: &str,
        expected: StatusCode,
    ) -> TestResult {
        let response = self
            .client
            .request(method.clone(), format!("{}{path}", app.url))
            .bearer_auth(token)
            .send()
            .await?;
        assert_eq!(
            response.status(),
            expected,
            "{method} {path} on {}",
            app.url
        );
        Ok(())
    }

    async fn metric(&self, app: &App, event: &str) -> TestResult<u64> {
        let body = self
            .client
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
}

fn client() -> TestResult<Client> {
    Ok(Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(8))
        .build()?)
}

async fn readiness(scenario: &Scenario, app: &App, expected: StatusCode) -> TestResult {
    let response = scenario
        .client
        .get(format!("{}/readyz", app.url))
        .send()
        .await?;
    assert_eq!(response.status(), expected);
    Ok(())
}

async fn reads_are_fenced(scenario: &Scenario, app: &App) -> TestResult {
    scenario.check(app, StatusCode::SERVICE_UNAVAILABLE).await?;
    scenario
        .expect(
            app,
            Method::GET,
            "/tasks",
            &scenario.dave,
            StatusCode::SERVICE_UNAVAILABLE,
        )
        .await?;
    readiness(scenario, app, StatusCode::SERVICE_UNAVAILABLE).await
}

async fn authoritative_check(stack: &Stack, scenario: &Scenario) -> TestResult<bool> {
    let config = stack.config()?;
    let response: Value = scenario
        .client
        .post(format!(
            "{}/stores/{}/check",
            stack.fga_http_url,
            config["store_id"].as_str().ok_or("store_id")?
        ))
        .json(&serde_json::json!({
            "authorization_model_id": config["model_id"],
            "consistency": "HIGHER_CONSISTENCY",
            "tuple_key": {"user":"user:dave", "relation":"can_view", "object":"task:access-review"}
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    response["allowed"]
        .as_bool()
        .ok_or_else(|| "missing allowed".into())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn fresh_allows_and_denial_invalidation_work_across_replicas() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let proxy = Proxy::start(client.clone(), stack.fga_url.clone()).await?;
    let first = stack.app(&stack.fga_url).await?;
    let second = stack.app(&proxy.url).await?;
    let scenario = Scenario {
        carol: stack.login(&client, &first, "carol").await?,
        dave: stack.login(&client, &second, "dave").await?,
        client,
    };
    scenario.grant(&first).await?;
    for replica in [&first, &second] {
        let before = scenario.metric(replica, "miss").await?;
        scenario.check(replica, StatusCode::OK).await?;
        scenario.check(replica, StatusCode::OK).await?;
        assert!(scenario.metric(replica, "miss").await? > before);
        assert_eq!(scenario.metric(replica, "hit_allow").await?, 0);
    }
    scenario.revoke(&first, StatusCode::NO_CONTENT).await?;
    for replica in [&first, &second] {
        scenario.check(replica, StatusCode::FORBIDDEN).await?;
        let before = scenario.metric(replica, "hit_deny").await?;
        scenario.check(replica, StatusCode::FORBIDDEN).await?;
        assert_eq!(scenario.metric(replica, "hit_deny").await?, before + 1);
    }
    // A grant advances the shared revision and invalidates local denials.
    scenario.grant(&first).await?;
    scenario.check(&first, StatusCode::OK).await?;
    scenario.check(&second, StatusCode::OK).await?;

    // Openlane reads use revision revalidation instead of holding the SQL lock
    // across Check. A late allow must be discarded after an acknowledged revoke.
    let before = scenario.metric(&second, "retry").await?;
    let mut pause = proxy.pause("check", false).await;
    let (check, revoke) = tokio::join!(scenario.check(&second, StatusCode::FORBIDDEN), async {
        let response: Value = serde_json::from_slice(&pause.captured().await?)?;
        assert_eq!(response["allowed"], true);
        scenario.revoke(&first, StatusCode::NO_CONTENT).await?;
        drop(pause);
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    });
    revoke?;
    check?;
    assert!(scenario.metric(&second, "retry").await? > before);
    scenario.check(&first, StatusCode::FORBIDDEN).await?;
    scenario.check(&second, StatusCode::FORBIDDEN).await?;

    // An isolated Check failure must close access even while service, store
    // and model readiness remain healthy after repeated successful reads.
    scenario.grant(&first).await?;
    scenario.check(&second, StatusCode::OK).await?;
    scenario.check(&second, StatusCode::OK).await?;
    let mut pause = proxy.pause("check", true).await;
    let (check, fault) = tokio::join!(
        scenario.check(&second, StatusCode::SERVICE_UNAVAILABLE),
        async {
            let response: Value = serde_json::from_slice(&pause.captured().await?)?;
            assert_eq!(response["allowed"], true);
            readiness(&scenario, &second, StatusCode::OK).await?;
            drop(pause);
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        }
    );
    fault?;
    check?;
    scenario.check(&second, StatusCode::OK).await?;
    Ok(())
}

async fn uncertain_change(add: bool) -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let proxy = Proxy::start(client.clone(), stack.fga_url.clone()).await?;
    let first = stack.app(&stack.fga_url).await?;
    let second = stack.app(&proxy.url).await?;
    let scenario = Scenario {
        carol: stack.login(&client, &first, "carol").await?,
        dave: stack.login(&client, &second, "dave").await?,
        client,
    };
    if !add {
        scenario.grant(&first).await?;
    }
    for _ in 0..2 {
        scenario
            .check(
                &first,
                if add {
                    StatusCode::FORBIDDEN
                } else {
                    StatusCode::OK
                },
            )
            .await?;
    }
    let before = scenario.metric(&first, "blocked_pending").await?;
    let mut pause = proxy.pause("write", false).await;
    let (write, blocked) = tokio::join!(
        scenario.expect(
            &second,
            if add { Method::PUT } else { Method::DELETE },
            RELATION,
            &scenario.carol,
            StatusCode::SERVICE_UNAVAILABLE
        ),
        async {
            // The real write committed; only its response is withheld.
            pause.captured().await?;
            assert_eq!(authoritative_check(&stack, &scenario).await?, add);
            assert!(stack.authorization_pending("acme").await?);
            reads_are_fenced(&scenario, &first).await?;
            reads_are_fenced(&scenario, &second).await
        }
    );
    // Wait for the API deadline before releasing the captured response.
    drop(pause);
    blocked?;
    write?;
    assert!(scenario.metric(&first, "blocked_pending").await? > before);
    for replica in [&first, &second] {
        reads_are_fenced(&scenario, replica).await?;
    }
    scenario
        .expect(
            &first,
            Method::PUT,
            RELATION,
            &scenario.carol,
            StatusCode::SERVICE_UNAVAILABLE,
        )
        .await?;
    second.stop().await?;
    let restarted = stack.app(&proxy.url).await?;
    assert!(!stack.authorization_pending("acme").await?);
    assert_eq!(authoritative_check(&stack, &scenario).await?, add);
    // Replay acknowledges the desired operation but cannot prove that an older
    // buffered request is drained. The durable global debt survives replay.
    for replica in [&first, &restarted] {
        reads_are_fenced(&scenario, replica).await?;
    }
    assert!(scenario.metric(&restarted, "blocked_pending").await? > 0);
    scenario
        .expect(
            &restarted,
            Method::PUT,
            RELATION,
            &scenario.carol,
            StatusCode::SERVICE_UNAVAILABLE,
        )
        .await?;

    first.stop().await?;
    restarted.stop().await?;
    proxy.wait_idle().await?;
    stack.recover_after_backend_drain(&stack.fga_url).await?;
    let first = stack.app(&stack.fga_url).await?;
    let second = stack.app(&proxy.url).await?;
    for replica in [&first, &second] {
        readiness(&scenario, replica, StatusCode::OK).await?;
        scenario
            .check(
                replica,
                if add {
                    StatusCode::OK
                } else {
                    StatusCode::FORBIDDEN
                },
            )
            .await?;
        assert_eq!(scenario.metric(replica, "hit_allow").await?, 0);
    }
    scenario.revoke(&first, StatusCode::NO_CONTENT).await?;
    scenario.grant(&first).await?;
    scenario.check(&second, StatusCode::OK).await?;
    scenario.revoke(&first, StatusCode::NO_CONTENT).await?;
    scenario.check(&second, StatusCode::FORBIDDEN).await?;
    println!(
        "Ambiguous grant/revoke stayed read-fenced across replay; explicit drained recovery restored live Checks and mutations"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn ambiguous_grant_and_revoke_remain_fenced_until_drained_recovery() -> TestResult {
    uncertain_change(true).await?;
    uncertain_change(false).await?;
    Ok(())
}

#[cfg(not(feature = "grpc"))]
#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn redirected_revoke_is_never_reported_as_committed() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let proxy = Proxy::start(client.clone(), stack.fga_url.clone()).await?;
    let first = stack.app(&stack.fga_url).await?;
    let second = stack.app(&proxy.url).await?;
    let scenario = Scenario {
        carol: stack.login(&client, &first, "carol").await?,
        dave: stack.login(&client, &second, "dave").await?,
        client,
    };
    scenario.grant(&first).await?;
    scenario.check(&first, StatusCode::OK).await?;
    scenario.check(&second, StatusCode::OK).await?;
    proxy
        .reject_once("write", StatusCode::TEMPORARY_REDIRECT)
        .await;
    scenario
        .revoke(&second, StatusCode::SERVICE_UNAVAILABLE)
        .await?;
    assert!(
        authoritative_check(&stack, &scenario).await?,
        "proxy did not forward the revoke"
    );
    for replica in [&first, &second] {
        reads_are_fenced(&scenario, replica).await?;
    }
    Ok(())
}
