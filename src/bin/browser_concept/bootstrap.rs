//! Provision one independent store; never reuse the catalog model or journal.
use super::{BODY_LIMIT, HTTP_TIMEOUT, Health, MODEL, OBJECT, Result, digest, fixture, valid_ulid};
use openfga_axum_lab::{
    application::{Authorizer, Tuple},
    infrastructure::OpenFgaHttp,
    model_binding::policy_model,
};
use reqwest::{Client, RequestBuilder, StatusCode, header::CONTENT_TYPE};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::time::Duration;

const MODEL_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
struct Ready {
    status: String,
}

#[derive(Deserialize)]
struct Store {
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreatedModel {
    authorization_model_id: String,
}

#[derive(Deserialize)]
struct ModelResponse {
    authorization_model: Value,
}

#[derive(Deserialize)]
struct ReadTuple {
    key: Tuple,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TuplePage {
    #[serde(default)]
    tuples: Vec<ReadTuple>,
    #[serde(default)]
    continuation_token: String,
}

async fn send<T: DeserializeOwned>(
    builder: RequestBuilder,
    expected: StatusCode,
    budget: Duration,
) -> Result<T> {
    tokio::time::timeout(budget, async {
        let (client, request) = builder.timeout(budget).build_split();
        let request = request?;
        let expected_url = request.url().clone();
        let mut response = client.execute(request).await?;
        if response.status() != expected || response.url() != &expected_url {
            return Err("OpenFGA bootstrap did not confirm the operation".into());
        }
        if !response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
        {
            return Err("OpenFGA bootstrap response is not JSON".into());
        }
        if response
            .content_length()
            .is_some_and(|length| length > BODY_LIMIT as u64)
        {
            return Err("OpenFGA bootstrap response is too large".into());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if chunk.len() > BODY_LIMIT - bytes.len() {
                return Err("OpenFGA bootstrap response is too large".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.iter().find(|byte| !byte.is_ascii_whitespace()) != Some(&b'{') {
            return Err("OpenFGA bootstrap response must be a JSON object".into());
        }
        Ok(serde_json::from_slice(&bytes)?)
    })
    .await?
}

async fn read_tuples(client: &Client, url: &str, store: &str) -> Result<Vec<Tuple>> {
    let page: TuplePage = send(
        client
            .post(format!("{url}/stores/{store}/read"))
            .json(&json!({"page_size":2,"consistency":"HIGHER_CONSISTENCY"})),
        StatusCode::OK,
        HTTP_TIMEOUT,
    )
    .await?;
    if !page.continuation_token.is_empty() {
        return Err("concept store contains unexpected extra tuples".into());
    }
    Ok(page.tuples.into_iter().map(|tuple| tuple.key).collect())
}

pub(super) async fn provision(client: Client, url: String) -> Result<(OpenFgaHttp, Health)> {
    let fixture = fixture()?;
    let model: Value = serde_json::from_slice(MODEL)?;
    // Readiness probes may retry. Model publication and tuple mutations never do.
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if let Ok(ready) = send::<Ready>(
                client.get(format!("{url}/healthz")),
                StatusCode::OK,
                HTTP_TIMEOUT,
            )
            .await
                && ready.status == "SERVING"
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    })
    .await?;
    let store: Store = send(
        client
            .post(format!("{url}/stores"))
            .json(&json!({"name":"browser-document-concept"})),
        StatusCode::CREATED,
        HTTP_TIMEOUT,
    )
    .await?;
    if !valid_ulid(&store.id) {
        return Err("OpenFGA returned an invalid concept store identifier".into());
    }
    let created: CreatedModel = send(
        client
            .post(format!("{url}/stores/{}/authorization-models", store.id))
            .json(&model),
        StatusCode::CREATED,
        MODEL_TIMEOUT,
    )
    .await?;
    if !valid_ulid(&created.authorization_model_id) {
        return Err("OpenFGA returned an invalid concept model identifier".into());
    }
    let readback: ModelResponse = send(
        client.get(format!(
            "{url}/stores/{}/authorization-models/{}",
            store.id, created.authorization_model_id
        )),
        StatusCode::OK,
        MODEL_TIMEOUT,
    )
    .await?;
    if readback.authorization_model["id"].as_str() != Some(created.authorization_model_id.as_str())
        || policy_model(&readback.authorization_model)? != policy_model(&model)?
    {
        return Err("OpenFGA readback differs from the embedded concept model".into());
    }
    if !read_tuples(&client, &url, &store.id).await?.is_empty() {
        return Err("new concept store was not empty".into());
    }
    let health = Health {
        protocol: 1,
        document: fixture.document.id,
        owner: fixture.owner,
        store_id: store.id.clone(),
        model_id: created.authorization_model_id.clone(),
        model_sha256: digest(MODEL),
        title: fixture.document.title,
    };
    let auth = OpenFgaHttp {
        client: client.clone(),
        url: url.clone(),
        store: store.id.clone(),
        model: created.authorization_model_id,
    };
    let owner = Tuple {
        user: "user:alice".into(),
        relation: "owner".into(),
        object: OBJECT.into(),
    };
    auth.write(std::slice::from_ref(&owner), true).await?;
    if read_tuples(&client, &url, &store.id).await? != [owner] {
        return Err("concept owner tuple was not confirmed by readback".into());
    }
    for (actor, expected) in [("alice", true), ("bob", false)] {
        for relation in ["can_read", "can_share"] {
            if auth.check(actor, relation, OBJECT).await? != expected {
                return Err("initial concept permissions differ from the fixture".into());
            }
        }
    }
    Ok((auth, health))
}
