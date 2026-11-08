use super::stack::{App, TestResult};
use reqwest::{Client, Method, RequestBuilder, StatusCode};
use serde_json::Value;

pub struct Api {
    client: Client,
    url: String,
}

impl Api {
    pub fn new(client: Client, app: &App) -> Self {
        Self {
            client,
            url: app.url.clone(),
        }
    }

    pub fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.client.request(method, format!("{}{path}", self.url))
    }

    pub fn call(&self, method: Method, path: &str, token: &str) -> RequestBuilder {
        self.request(method, path).bearer_auth(token)
    }
}

pub async fn expect(request: RequestBuilder, expected: StatusCode) -> TestResult<Value> {
    let (client, request) = request.build_split();
    let request = request.map_err(reqwest::Error::without_url)?;
    let method = request.method().clone();
    // Report the route without query parameters, URL credentials, headers, or
    // request/response bodies: login responses may contain a session token.
    let path = request.url().path().to_owned();
    let response = client
        .execute(request)
        .await
        .map_err(|error| format!("{method} {path}: request failed: {}", error.without_url()))?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .map_or("<missing>", |value| value.to_str().unwrap_or("<invalid>"))
        .to_owned();
    let context = format!("{method} {path}: status={status}, content-type={content_type}");
    assert_eq!(status, expected, "unexpected HTTP status; {context}");
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("{context}: reading body failed: {}", error.without_url()))?;
    if bytes.is_empty() {
        if status.is_success() && status != StatusCode::NO_CONTENT {
            return Err(format!("{context}: expected a JSON response, got an empty body").into());
        }
        Ok(Value::Null)
    } else if !status.is_success()
        && content_type.split(';').next().map(str::trim) == Some("text/plain")
    {
        // Axum's typed-path and body-limit extractors return plain-text
        // rejections. The exact expected status was asserted above; success
        // bodies and JSON error responses must still decode as valid JSON.
        Ok(Value::Null)
    } else {
        serde_json::from_slice(&bytes).map_err(|error| {
            format!(
                "{context}: invalid JSON ({:?} at line {}, column {}); body omitted",
                error.classify(),
                error.line(),
                error.column()
            )
            .into()
        })
    }
}
