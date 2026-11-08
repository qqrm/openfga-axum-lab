use crate::application::{App, CreateTask, Identity, TaskInput};
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
    match app.visible_tasks(&identity.0).await {
        Ok(tasks) => Json(tasks).into_response(),
        Err(error) => error_response(error),
    }
}
async fn task(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path(id): Path<String>,
) -> Response {
    match app.get_task(&identity.0, &id).await {
        Ok(task) => Json(task).into_response(),
        Err(error) => error_response(error),
    }
}
async fn create(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    headers: HeaderMap,
    Json(input): Json<CreateTask>,
) -> Response {
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    match app.create_task(&identity.0, key, &input).await {
        Ok(id) => (StatusCode::CREATED, Json(json!({"id":id}))).into_response(),
        Err(error) => error_response(error),
    }
}
async fn edit(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path(id): Path<String>,
    Json(input): Json<TaskInput>,
) -> Response {
    mutation_response(app.edit_task(&identity.0, &id, &input).await)
}
async fn delete_task(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path(id): Path<String>,
) -> Response {
    mutation_response(app.delete_task(&identity.0, &id).await)
}
async fn relation(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path((id, relation, subject)): Path<(String, String, String)>,
    method: Method,
) -> Response {
    mutation_response(
        app.set_task_relation(&identity.0, &id, &relation, &subject, method == Method::PUT)
            .await,
    )
}
async fn organizations(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
) -> Response {
    match app.visible_organizations(&identity.0).await {
        Ok(organizations) => Json(organizations).into_response(),
        Err(error) => error_response(error),
    }
}
async fn members(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path(id): Path<String>,
) -> Response {
    match app.organization_members(&identity.0, &id).await {
        Ok(members) => Json(members).into_response(),
        Err(error) => error_response(error),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MemberRole {
    role: String,
}
async fn put_member(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path((id, subject)): Path<(String, String)>,
    Json(input): Json<MemberRole>,
) -> Response {
    mutation_response(
        app.set_member(&identity.0, &id, &subject, Some(&input.role))
            .await,
    )
}
async fn delete_member(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path((id, subject)): Path<(String, String)>,
) -> Response {
    mutation_response(app.set_member(&identity.0, &id, &subject, None).await)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ownership {
    expected_owner: String,
    new_owner: String,
}
async fn transfer_owner(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path(id): Path<String>,
    Json(input): Json<Ownership>,
) -> Response {
    mutation_response(
        app.transfer_ownership(&identity.0, &id, &input.expected_owner, &input.new_owner)
            .await,
    )
}
async fn organization_permission(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path((id, relation, subject)): Path<(String, String, String)>,
    method: Method,
) -> Response {
    mutation_response(
        app.set_organization_permission(
            &identity.0,
            &id,
            &relation,
            &subject,
            method == Method::PUT,
        )
        .await,
    )
}
fn mutation_response(result: crate::use_cases::Result<()>) -> Response {
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => error_response(error),
    }
}
pub(crate) fn router(app: App) -> Router {
    Router::new()
        .route("/tasks", get(list).post(create))
        .route("/tasks/{id}", get(task).patch(edit).delete(delete_task))
        .route(
            "/tasks/{id}/relations/{relation}/{subject}",
            axum::routing::put(relation).delete(relation),
        )
        .route("/organizations", get(organizations))
        .route("/organizations/{id}/members", get(members))
        .route(
            "/organizations/{id}/members/{subject}",
            axum::routing::put(put_member).delete(delete_member),
        )
        .route(
            "/organizations/{id}/owner",
            axum::routing::put(transfer_owner),
        )
        .route(
            "/organizations/{id}/permissions/{relation}/{subject}",
            axum::routing::put(organization_permission).delete(organization_permission),
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
