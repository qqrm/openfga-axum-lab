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
    let response = request.send().await?;
    assert_eq!(response.status(), expected, "unexpected HTTP status");
    let bytes = response.bytes().await?;
    if bytes.is_empty() {
        Ok(Value::Null)
    } else {
        Ok(serde_json::from_slice(&bytes)?)
    }
}
