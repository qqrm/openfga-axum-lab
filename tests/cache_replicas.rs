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

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn revocation_invalidates_warm_allows_across_replicas() -> TestResult {
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(8))
        .build()?;
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
        let before = scenario.metric(replica, "hit_allow").await?;
        scenario.check(replica, StatusCode::OK).await?;
        scenario.check(replica, StatusCode::OK).await?;
        assert!(scenario.metric(replica, "hit_allow").await? > before);
    }
    scenario.revoke(&first, StatusCode::NO_CONTENT).await?;
    for replica in [&first, &second] {
        scenario.check(replica, StatusCode::FORBIDDEN).await?;
    }
    println!("Both warm caches denied access after the revoke");

    // Capture the actual allow from OpenFGA before revoking on the other replica.
    scenario.grant(&first).await?;
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
    scenario.check(&second, StatusCode::FORBIDDEN).await?;
    println!("The late OpenFGA allow was discarded");

    // A hit waits for readiness. Complete a revoke while that response is held.
    scenario.grant(&first).await?;
    scenario.check(&second, StatusCode::OK).await?;
    let before = scenario.metric(&second, "retry").await?;
    let mut pause = proxy.pause("healthz", false).await;
    let (check, revoke) = tokio::join!(scenario.check(&second, StatusCode::FORBIDDEN), async {
        pause.captured().await?;
        let result = scenario.revoke(&first, StatusCode::NO_CONTENT).await;
        drop(pause);
        result
    });
    revoke?;
    check?;
    assert!(scenario.metric(&second, "retry").await? > before);
    println!("The overlapping cache hit was discarded");

    scenario.grant(&first).await?;
    scenario.check(&second, StatusCode::OK).await?;
    let mut pause = proxy.pause("healthz", true).await;
    let (check, fault) = tokio::join!(
        scenario.check(&second, StatusCode::SERVICE_UNAVAILABLE),
        async {
            let captured = pause.captured().await;
            drop(pause);
            captured
        }
    );
    fault?;
    check?;

    // Let the API time out after the proxy has received the successful write.
    scenario.check(&first, StatusCode::OK).await?;
    scenario.check(&first, StatusCode::OK).await?;
    let mut pause = proxy.pause("write", false).await;
    let (write, check) = tokio::join!(
        scenario.revoke(&second, StatusCode::SERVICE_UNAVAILABLE),
        async {
            pause.captured().await?;
            assert!(stack.authorization_pending("acme").await?);
            // Until the durable operation is acknowledged or replayed, the
            // complete organization is fenced, including its warm allows.
            scenario
                .check(&first, StatusCode::SERVICE_UNAVAILABLE)
                .await
        }
    );
    drop(pause);
    check?;
    write?;
    scenario
        .check(&second, StatusCode::SERVICE_UNAVAILABLE)
        .await?;
    scenario
        .revoke(&first, StatusCode::SERVICE_UNAVAILABLE)
        .await?;

    second.stop().await?;
    let restarted = stack.app(&proxy.url).await?;
    assert!(!stack.authorization_pending("acme").await?);
    for replica in [&first, &restarted] {
        scenario.check(replica, StatusCode::FORBIDDEN).await?;
    }

    assert!(scenario.metric(&restarted, "bypass_pending").await? > 0);
    scenario
        .expect(
            &first,
            Method::PUT,
            RELATION,
            &scenario.carol,
            StatusCode::SERVICE_UNAVAILABLE,
        )
        .await?;
    // Replay acknowledges desired state, not the absence of an older buffered
    // request. Writes remain quarantined until the explicit operator drain.
    first.stop().await?;
    restarted.stop().await?;
    proxy.wait_idle().await?;
    stack.recover_after_backend_drain(&stack.fga_url).await?;
    let first = stack.app(&stack.fga_url).await?;
    let second = stack.app(&proxy.url).await?;
    scenario.grant(&first).await?;
    scenario.check(&second, StatusCode::OK).await?;
    scenario.check(&second, StatusCode::OK).await?;
    scenario.revoke(&first, StatusCode::NO_CONTENT).await?;
    scenario.check(&second, StatusCode::FORBIDDEN).await?;
    assert_eq!(scenario.metric(&second, "bypass_pending").await?, 0);
    assert!(scenario.metric(&second, "hit_allow").await? > 0);
    println!(
        "Replay restored safe reads; only verified drain and explicit recovery restored writes and normal caching"
    );
    Ok(())
}
