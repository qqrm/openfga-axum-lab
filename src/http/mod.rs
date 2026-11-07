use crate::application::{App, Identity, PetInput};
use axum::{
    Json, Router,
    extract::{MatchedPath, Path, Request, State},
    http::{HeaderMap, Method, StatusCode, header::AUTHORIZATION},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use serde_json::json;

mod auth;
pub(crate) use auth::guard;

pub(crate) fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .filter(|v| !v.is_empty() && v.len() <= 256)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Login {
    username: String,
    password: String,
}

async fn login(State(app): State<App>, Json(input): Json<Login>) -> Response {
    match app.login(&input.username, input.password).await {
        Ok(token) => (
            [(axum::http::header::CACHE_CONTROL, "no-store")],
            Json(json!({"token":token,"expires_in":43200})),
        )
            .into_response(),
        Err(error) => error_response(error),
    }
}
async fn logout(State(app): State<App>, headers: HeaderMap) -> Response {
    let Some(token) = bearer(&headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match app.logout(token).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => error_response(error),
    }
}

fn error_response(error: crate::use_cases::Error) -> Response {
    use crate::use_cases::Error;
    match error {
        Error::Invalid => StatusCode::BAD_REQUEST,
        Error::NotFound => StatusCode::NOT_FOUND,
        Error::Conflict => StatusCode::CONFLICT,
        Error::Gone => StatusCode::GONE,
        Error::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        Error::Forbidden => StatusCode::FORBIDDEN,
        Error::Unauthenticated => StatusCode::UNAUTHORIZED,
        Error::Throttled => StatusCode::TOO_MANY_REQUESTS,
    }
    .into_response()
}
async fn list(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
) -> Response {
    match app.visible_pets(&identity.0).await {
        Ok(pets) => Json(pets).into_response(),
        Err(error) => error_response(error),
    }
}
async fn pet(State(app): State<App>, Path(id): Path<String>) -> Response {
    match app.get_pet(&id).await {
        Ok(pet) => Json(pet).into_response(),
        Err(error) => error_response(error),
    }
}
async fn create(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    headers: HeaderMap,
    Json(input): Json<PetInput>,
) -> Response {
    let Some(key) = headers.get("idempotency-key").and_then(|v| v.to_str().ok()) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    match app.create_pet(&identity.0, key, &input).await {
        Ok(id) => (StatusCode::CREATED, Json(json!({"id":id}))).into_response(),
        Err(error) => error_response(error),
    }
}
async fn edit(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path(id): Path<String>,
    Json(input): Json<PetInput>,
) -> Response {
    match app.edit_pet(&identity.0, &id, &input).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => error_response(error),
    }
}
async fn delete_pet(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path(id): Path<String>,
) -> Response {
    match app.delete_pet(&id, &identity.0).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => error_response(error),
    }
}
async fn relation(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path((id, relation, subject)): Path<(String, String, String)>,
    method: Method,
) -> Response {
    match app
        .set_relation(&identity.0, &id, &relation, &subject, method == Method::PUT)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => error_response(error),
    }
}
pub(crate) fn router(app: App) -> Router {
    Router::new()
        .route("/pets", get(list).post(create))
        .route("/pets/{id}", get(pet).patch(edit).delete(delete_pet))
        .route(
            "/pets/{id}/relations/{relation}/{subject}",
            axum::routing::put(relation).delete(relation),
        )
        .route_layer(middleware::from_fn_with_state(app.clone(), guard))
        .route(
            "/",
            get(|| async { Html(include_str!("../../assets/index.html")) }),
        )
        .route(
            "/assets/app.js",
            get(|| async {
                (
                    [(
                        axum::http::header::CONTENT_TYPE,
                        "text/javascript; charset=utf-8",
                    )],
                    include_str!("../../assets/app.js"),
                )
            }),
        )
        .route(
            "/assets/app.css",
            get(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../../assets/app.css"),
                )
            }),
        )
        .route("/healthz", get(|| async { "ok" }))
        .route("/metrics", get(metrics))
        .route("/session", axum::routing::post(login).delete(logout))
        .layer(middleware::from_fn(security_headers))
        .layer(middleware::from_fn_with_state(app.clone(), observe_request))
        .with_state(app)
}
async fn metrics(State(app): State<App>) -> Response {
    match app.metrics.render() {
        Ok(body) => (
            [(
                axum::http::header::CONTENT_TYPE,
                "text/plain; version=0.0.4; charset=utf-8",
            )],
            body,
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
async fn observe_request(State(app): State<App>, request: Request, next: Next) -> Response {
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .unwrap_or("unmatched")
        .to_owned();
    let method = match *request.method() {
        Method::GET => "GET",
        Method::POST => "POST",
        Method::PATCH => "PATCH",
        Method::PUT => "PUT",
        Method::DELETE => "DELETE",
        _ => "OTHER",
    };
    let start = std::time::Instant::now();
    let response = next.run(request).await;
    app.metrics
        .requests
        .with_label_values(&[method, &route, response.status().as_str()])
        .inc();
    app.metrics
        .duration
        .with_label_values(&[method, &route])
        .observe(start.elapsed().as_secs_f64());
    response
}
async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(axum::http::header::CONTENT_SECURITY_POLICY,
        axum::http::HeaderValue::from_static("default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'"));
    headers.insert(
        axum::http::header::X_CONTENT_TYPE_OPTIONS,
        axum::http::HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        axum::http::header::REFERRER_POLICY,
        axum::http::HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}
