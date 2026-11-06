use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::{MatchedPath, Path, Request, State},
    http::{HeaderMap, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug)]
struct AuthError;

#[async_trait]
trait Authorizer: Send + Sync {
    async fn check(&self, user: &str, relation: &str, object: &str) -> Result<bool, AuthError>;
    async fn write(
        &self,
        subject: &str,
        relation: &str,
        object: &str,
        add: bool,
    ) -> Result<(), AuthError>;
}

struct Fga {
    client: reqwest::Client,
    url: String,
    store: String,
    model: String,
}
// An explicit demo-only legacy policy; unlike a bypass, disabling FGA still
// enforces access decisions. Mutations are disabled in legacy mode.
struct Legacy;
#[async_trait]
impl Authorizer for Legacy {
    async fn check(&self, user: &str, relation: &str, object: &str) -> Result<bool, AuthError> {
        let member_object = object == "dashboard:one" || object == "dashboard:two";
        Ok(match relation {
            "admin" => object == "organization:acme" && user == "alice",
            "can_view" => {
                (member_object && (user == "alice" || user == "bob"))
                    || (object == "dashboard:private" && user == "carol")
                    || (object == "dashboard:two" && user == "dave")
            }
            "can_manage" => {
                (member_object && user == "alice")
                    || (object == "dashboard:private" && user == "carol")
            }
            _ => false,
        })
    }
    async fn write(&self, _: &str, _: &str, _: &str, _: bool) -> Result<(), AuthError> {
        Err(AuthError)
    }
}
#[derive(Deserialize)]
struct Check {
    allowed: bool,
}
#[async_trait]
impl Authorizer for Fga {
    async fn check(&self, user: &str, relation: &str, object: &str) -> Result<bool, AuthError> {
        let response = self.client.post(format!("{}/stores/{}/check", self.url, self.store))
            .json(&json!({"authorization_model_id":self.model,"tuple_key":{"user":format!("user:{user}"),"relation":relation,"object":object}}))
            .send().await.map_err(|_|AuthError)?.error_for_status().map_err(|_|AuthError)?
            .json::<Check>().await.map_err(|_|AuthError)?;
        Ok(response.allowed)
    }
    async fn write(
        &self,
        subject: &str,
        relation: &str,
        object: &str,
        add: bool,
    ) -> Result<(), AuthError> {
        let mut data = json!({"authorization_model_id":self.model});
        data[if add { "writes" } else { "deletes" }] =
            json!({"tuple_keys":[{"user":subject,"relation":relation,"object":object}]});
        self.client
            .post(format!("{}/stores/{}/write", self.url, self.store))
            .json(&data)
            .send()
            .await
            .map_err(|_| AuthError)?
            .error_for_status()
            .map_err(|_| AuthError)?;
        Ok(())
    }
}
#[derive(Clone)]
struct App {
    auth: Arc<dyn Authorizer>,
}
const ITEMS: [&str; 3] = ["one", "two", "private"];
fn valid(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
fn user(h: &HeaderMap) -> Option<&str> {
    h.get("x-demo-user")?.to_str().ok().filter(|s| valid(s))
}
async fn guard(State(app): State<App>, req: Request, next: Next) -> Response {
    let Some(who) = user(req.headers()) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let path = req
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .unwrap_or("");
    let (relation, kind) = match (req.method(), path) {
        (&Method::GET, "/dashboards") => return next.run(req).await,
        (&Method::GET, "/dashboards/{id}") => ("can_view", "dashboard"),
        (&Method::PUT | &Method::DELETE, "/dashboards/{id}/relations/{relation}/{subject}") => {
            ("can_manage", "dashboard")
        }
        (&Method::PUT | &Method::DELETE, "/organizations/{id}/members/{subject}") => {
            ("admin", "organization")
        }
        _ => return StatusCode::FORBIDDEN.into_response(),
    };
    let Some(id) = req.uri().path().split('/').nth(2) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if (kind == "dashboard" && !ITEMS.contains(&id)) || (kind == "organization" && id != "acme") {
        return StatusCode::NOT_FOUND.into_response();
    }
    match app.auth.check(who, relation, &format!("{kind}:{id}")).await {
        Ok(true) => next.run(req).await,
        Ok(false) => StatusCode::FORBIDDEN.into_response(),
        Err(_) => {
            tracing::warn!("authorization backend error");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}
async fn list(State(app): State<App>, h: HeaderMap) -> Response {
    let Some(who) = user(&h) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let mut items = Vec::new();
    for id in ITEMS {
        match app
            .auth
            .check(who, "can_view", &format!("dashboard:{id}"))
            .await
        {
            Ok(true) => items.push(id),
            Ok(false) => {}
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        }
    }
    Json(items).into_response()
}
async fn item(Path(id): Path<String>) -> Json<Value> {
    Json(json!({"id": id}))
}
async fn change(
    State(app): State<App>,
    Path((id, relation, who)): Path<(String, String, String)>,
    method: Method,
) -> Response {
    if !valid(&who) || !matches!(relation.as_str(), "viewer" | "editor" | "owner" | "parent") {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if relation == "parent" && who != "acme" {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let subject = format!(
        "{}:{who}",
        if relation == "parent" {
            "organization"
        } else {
            "user"
        }
    );
    match app
        .auth
        .write(
            &subject,
            &relation,
            &format!("dashboard:{id}"),
            method == Method::PUT,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}
async fn change_member(
    State(app): State<App>,
    Path((id, who)): Path<(String, String)>,
    method: Method,
) -> Response {
    if !valid(&who) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match app
        .auth
        .write(
            &format!("user:{who}"),
            "member",
            &format!("organization:{id}"),
            method == Method::PUT,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}
fn router(app: App) -> Router {
    Router::new()
        .route("/dashboards", get(list))
        .route("/dashboards/{id}", get(item))
        .route(
            "/dashboards/{id}/relations/{relation}/{subject}",
            axum::routing::put(change).delete(change),
        )
        .route(
            "/organizations/{id}/members/{subject}",
            axum::routing::put(change_member).delete(change_member),
        )
        .route_layer(middleware::from_fn_with_state(app.clone(), guard))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(app)
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    let auth: Arc<dyn Authorizer> = match std::env::var("AUTH_BACKEND").as_deref() {
        Ok("legacy") => Arc::new(Legacy),
        Ok("fga") | Err(std::env::VarError::NotPresent) => {
            let path = std::env::var("FGA_CONFIG").unwrap_or_else(|_| "/data/config.json".into());
            let config: Value = serde_json::from_slice(&std::fs::read(path)?)?;
            let val = |k: &str| -> Result<String, Box<dyn std::error::Error>> {
                config[k]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .ok_or_else(|| format!("missing {k}").into())
            };
            Arc::new(Fga {
                client: reqwest::Client::builder()
                    .timeout(Duration::from_secs(2))
                    .build()?,
                url: std::env::var("FGA_URL").unwrap_or_else(|_| "http://openfga:8080".into()),
                store: val("store_id")?,
                model: val("model_id")?,
            })
        }
        _ => return Err("AUTH_BACKEND must be fga or legacy".into()),
    };
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(listener, router(App { auth })).await?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn id_validation() {
        assert!(valid("alice-1"));
        assert!(!valid("user:alice"));
        assert!(!valid(""));
        assert!(!valid(&"a".repeat(65)));
    }
}
