//! A bounded authorization check may fail closed while unrelated writes keep
//! changing the global generation. Exercise that admission failure separately
//! from the grant/delete lifecycle race, whose handlers must all be admitted.

use crate::{
    api::{Api, expect},
    stack::{self, App, Stack, TestResult, client},
};
#[path = "../cache_support/proxy.rs"]
mod proxy;

use proxy::Proxy;
use reqwest::{Client, Method as M, StatusCode as S};
use serde_json::{Value, json};
use std::time::Duration;

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
    let alice = stack.login(&client, &app, "alice").await?;
    let before = retry_count(&client, &app).await?;
    let mut pause = proxy.pause("check", false).await;
    let request = api
        .call(M::GET, "/pets/fox", &carol)
        .timeout(Duration::from_secs(30));
    let churn = async {
        // Each actual mutation commits a new shared generation while a real,
        // successful OpenFGA response for Carol's unchanged ownership is held.
        for (attempt, method) in [M::PUT, M::DELETE, M::PUT].into_iter().enumerate() {
            let response: Value = serde_json::from_slice(&pause.captured().await?)?;
            assert_eq!(response["allowed"], json!(true));
            expect(
                api.call(method, "/pets/cat/relations/caregiver/bob", &alice),
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
    expect(api.call(M::GET, "/pets/fox", &carol), S::OK).await?;
    expect(
        api.call(M::DELETE, "/pets/cat/relations/caregiver/bob", &alice),
        S::NO_CONTENT,
    )
    .await?;
    assert_eq!(retry_count(&client, &app).await?, before + 3);
    app.stop().await?;
    Ok(())
}
