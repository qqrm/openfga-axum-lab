#[path = "cache_support/proxy.rs"]
mod proxy;
#[path = "support/stack.rs"]
pub mod stack;

use proxy::Proxy;
use reqwest::{Client, Method, StatusCode};
use serde_json::Value;
use stack::{App, Stack, TestResult};
use std::time::Duration;

const TABLE: &str = "/projects/acme/warehouses/analytics/tables/requests";
const GRANT: &str = "/projects/acme/warehouses/analytics/tables/requests/grants/modify";

struct Scenario {
    client: Client,
    carol: String,
    dave: String,
}

impl Scenario {
    async fn check(&self, app: &App, expected: StatusCode) -> TestResult {
        self.expect(app, Method::GET, TABLE, &self.dave, expected)
            .await
    }

    async fn grant(&self, app: &App) -> TestResult {
        self.expect(app, Method::PUT, GRANT, &self.carol, StatusCode::NO_CONTENT)
            .await
    }

    async fn revoke(&self, app: &App, expected: StatusCode) -> TestResult {
        self.expect(app, Method::DELETE, GRANT, &self.carol, expected)
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
        let mut request = self
            .client
            .request(method.clone(), format!("{}{path}", app.url))
            .bearer_auth(token);
        if path == GRANT {
            request = request.json(&serde_json::json!({
                "subject": {"type": "user", "id": "dave"}
            }));
        }
        let response = request.send().await?;
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
            "tuple_key": {"user": "user:dave", "relation": "can_get_metadata", "object": "lakekeeper_table:analytics/requests"}
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
async fn fresh_checks_and_denial_invalidation_work_across_replicas() -> TestResult {
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
        assert_eq!(scenario.metric(replica, "miss").await?, before + 2);
        assert_eq!(scenario.metric(replica, "hit_allow").await?, 0);
    }
    scenario.revoke(&first, StatusCode::NO_CONTENT).await?;
    for replica in [&first, &second] {
        scenario.check(replica, StatusCode::FORBIDDEN).await?;
        let before = scenario.metric(replica, "hit_deny").await?;
        scenario.check(replica, StatusCode::FORBIDDEN).await?;
        assert_eq!(scenario.metric(replica, "hit_deny").await?, before + 1);
    }
    // A grant advances the shared revision, invalidating local denials.
    scenario.grant(&first).await?;
    scenario.check(&first, StatusCode::OK).await?;
    scenario.check(&second, StatusCode::OK).await?;

    // Metadata reads retain the project read lock through the live Check.
    // A revoke must wait for this authorized read before it can linearize.
    let mut pause = proxy.pause("check", false).await;
    let (check, revoke) = tokio::join!(scenario.check(&second, StatusCode::OK), async {
        let response: Value = serde_json::from_slice(&pause.captured().await?)?;
        assert_eq!(response["allowed"], true);
        let revoke = scenario.revoke(&first, StatusCode::NO_CONTENT);
        tokio::pin!(revoke);
        assert!(
            tokio::time::timeout(Duration::from_millis(150), &mut revoke)
                .await
                .is_err(),
            "revoke completed while the authorized metadata read held its project lock"
        );
        drop(pause);
        revoke.await
    });
    revoke?;
    check?;
    // Once the revoke is acknowledged, no prior allow may survive on either replica.
    scenario.check(&first, StatusCode::FORBIDDEN).await?;
    scenario.check(&second, StatusCode::FORBIDDEN).await?;

    // Warm successful requests cannot mask an isolated Check failure. Health,
    // GetStore and GetModel remain successful throughout this fault.
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
    let before = scenario.metric(&first, "blocked_pending").await?;
    let mut pause = proxy.pause("write", false).await;
    let (write, blocked) = tokio::join!(
        scenario.expect(
            &second,
            if add { Method::PUT } else { Method::DELETE },
            GRANT,
            &scenario.carol,
            StatusCode::SERVICE_UNAVAILABLE
        ),
        async {
            // The real mutation committed; only its response is withheld.
            pause.captured().await?;
            assert_eq!(authoritative_check(&stack, &scenario).await?, add);
            scenario
                .check(&first, StatusCode::SERVICE_UNAVAILABLE)
                .await?;
            scenario
                .check(&second, StatusCode::SERVICE_UNAVAILABLE)
                .await?;
            readiness(&scenario, &first, StatusCode::SERVICE_UNAVAILABLE).await
        }
    );
    // Keep the response held until the request timed out, so the caller cannot
    // know whether the grant/revoke committed and must retain the durable fence.
    drop(pause);
    blocked?;
    write?;
    assert!(scenario.metric(&first, "blocked_pending").await? > before);
    for replica in [&first, &second] {
        scenario
            .check(replica, StatusCode::SERVICE_UNAVAILABLE)
            .await?;
        readiness(&scenario, replica, StatusCode::SERVICE_UNAVAILABLE).await?;
    }
    // A new API mutation cannot silently heal a pre-existing unknown outcome.
    scenario
        .expect(
            &first,
            Method::PUT,
            GRANT,
            &scenario.carol,
            StatusCode::SERVICE_UNAVAILABLE,
        )
        .await?;
    second.stop().await?;
    let restarted = stack.app(&proxy.url).await?;
    scenario
        .check(&restarted, StatusCode::SERVICE_UNAVAILABLE)
        .await?;
    readiness(&scenario, &restarted, StatusCode::SERVICE_UNAVAILABLE).await?;
    assert!(scenario.metric(&restarted, "blocked_pending").await? > 0);
    Ok(())
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn ambiguous_grant_and_revoke_remain_fenced_after_restart() -> TestResult {
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
        scenario
            .check(replica, StatusCode::SERVICE_UNAVAILABLE)
            .await?;
        readiness(&scenario, replica, StatusCode::SERVICE_UNAVAILABLE).await?;
    }
    Ok(())
}
