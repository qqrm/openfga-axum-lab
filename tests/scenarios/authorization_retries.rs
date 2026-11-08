//! A bounded authorization check may fail closed while unrelated writes keep
//! changing the global generation. Exercise that admission failure separately
//! from lifecycle races, where a durable pending-intent fence may reject admission.

use crate::{
    api::{Api, expect},
    proxy::Proxy,
    stack::{App, Stack, TestResult, client},
};
use reqwest::{Client, Method as M, StatusCode as S};
use serde_json::{Value, json};
use std::time::Duration;

const TABLE_PATH: &str = "/projects/acme/warehouses/analytics/tables/requests";
const OTHER_GRANT_PATH: &str = "/projects/other/warehouses/external/tables/requests/grants/select";

async fn retry_count(client: &Client, app: &App) -> TestResult<u64> {
    let metrics = client
        .get(format!("{}/metrics", app.url))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    Ok(metrics
        .lines()
        .find_map(|line| line.strip_prefix("authz_cache_events_total{event=\"retry\"} "))
        .unwrap_or("0")
        .parse()?)
}

#[tokio::test]
#[ignore = "requires a local Docker engine"]
async fn generation_churn_exhausts_retry_budget_then_recovers() -> TestResult {
    let client = client()?;
    let stack = Stack::start(&client).await?;
    let proxy = Proxy::start(client.clone(), stack.fga_url.clone()).await?;
    let app = stack.app(&proxy.url).await?;
    let api = Api::new(client.clone(), &app);
    let carol = stack.login(&client, &app, "carol").await?;
    let eve = stack.login(&client, &app, "eve").await?;
    let subject = json!({"subject": {"type": "user", "id": "eve"}});
    let before = retry_count(&client, &app).await?;
    let mut pause = proxy.pause("check", false).await;
    let request = api
        .call(M::GET, TABLE_PATH, &carol)
        .timeout(Duration::from_secs(30));
    let churn = async {
        // Carol's GET holds the acme project's shared SQL lock while its real
        // successful Check response is paused. Eve writes in another project;
        // a same-project mutation would deadlock against that read lock.
        // Grant/revoke of Eve's direct select tuple leaves both owners intact,
        // but each completed write advances the global cache generation.
        for (attempt, method) in [M::PUT, M::DELETE, M::PUT].into_iter().enumerate() {
            let response: Value = serde_json::from_slice(&pause.captured().await?)?;
            assert_eq!(response["allowed"], json!(true));
            expect(
                api.call(method, OTHER_GRANT_PATH, &eve).json(&subject),
                S::NO_CONTENT,
            )
            .await?;
            if attempt < 2 {
                // Arm the next check before releasing this one: the retry
                // cannot race ahead of the test and obtain a stable decision.
                let next = proxy.pause("check", false).await;
                drop(pause);
                pause = next;
            }
        }
        drop(pause);
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    };
    let (response, churn) = tokio::join!(expect(request, S::SERVICE_UNAVAILABLE), churn);
    churn?;
    response?;
    assert_eq!(retry_count(&client, &app).await?, before + 3);

    // Once writes stop, the same request must recover without a new process,
    // session or permission grant. A broken backend cannot satisfy this check.
    expect(api.call(M::GET, TABLE_PATH, &carol), S::OK).await?;
    expect(
        api.call(M::DELETE, OTHER_GRANT_PATH, &eve).json(&subject),
        S::NO_CONTENT,
    )
    .await?;
    assert_eq!(retry_count(&client, &app).await?, before + 3);
    app.stop().await?;
    Ok(())
}
