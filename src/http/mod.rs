use crate::application::{App, CreateTask, Identity, TaskInput};
use crate::domain::{OrganizationId, TaskId, UserId};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, MatchedPath, Path, Request, State},
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
    let mut values = headers.get_all(AUTHORIZATION).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    let (scheme, token) = value.to_str().ok()?.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer")
        || token.len() != 64
        || !token.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    Some(token)
}

fn idempotency_key(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers.get_all("idempotency-key").iter();
    let value = values.next()?.to_str().ok()?;
    (values.next().is_none() && crate::domain::valid_idempotency_key(value)).then_some(value)
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
    Path(id): Path<TaskId>,
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
    let Some(key) = idempotency_key(&headers) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    match app.create_task(&identity.0, key, &input).await {
        Ok(id) => (StatusCode::CREATED, Json(json!({"id":id}))).into_response(),
        Err(error) => error_response(error),
    }
}
async fn edit(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path(id): Path<TaskId>,
    Json(input): Json<TaskInput>,
) -> Response {
    mutation_response(app.edit_task(&identity.0, &id, &input).await)
}
async fn delete_task(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path(id): Path<TaskId>,
) -> Response {
    mutation_response(app.delete_task(&identity.0, &id).await)
}
async fn relation(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path((id, relation, subject)): Path<(TaskId, String, UserId)>,
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
    Path(id): Path<OrganizationId>,
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
    Path((id, subject)): Path<(OrganizationId, UserId)>,
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
    Path((id, subject)): Path<(OrganizationId, UserId)>,
) -> Response {
    mutation_response(app.set_member(&identity.0, &id, &subject, None).await)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ownership {
    expected_owner: UserId,
    new_owner: UserId,
}
async fn transfer_owner(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path(id): Path<OrganizationId>,
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
    Path((id, relation, subject)): Path<(OrganizationId, String, UserId)>,
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
        .route(
            "/model",
            get(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    include_str!("../../fga/provenance.json"),
                )
            }),
        )
        .route(
            "/model/schema",
            get(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    include_str!("../../fga/model.json"),
                )
            }),
        )
        .route("/metrics", get(metrics))
        .route("/session", axum::routing::post(login).delete(logout))
        .layer(DefaultBodyLimit::max(16 * 1024))
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
pub(crate) async fn security_headers(request: Request, next: Next) -> Response {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_rejects_duplicate_headers_and_combined_or_ambiguous_tokens() {
        let token = "a".repeat(64);
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
        assert_eq!(bearer(&headers), Some(token.as_str()));
        headers.append(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
        assert_eq!(bearer(&headers), None);
        for value in [
            format!("Bearer {token}, Bearer {token}"),
            format!("Bearer  {token}"),
            format!("Bearer {token} "),
            format!("Basic {token}"),
            format!("Bearer {}", "x".repeat(64)),
        ] {
            headers.clear();
            headers.insert(AUTHORIZATION, value.parse().unwrap());
            assert_eq!(bearer(&headers), None);
        }
        headers.clear();
        headers.insert(AUTHORIZATION, format!("bearer {token}").parse().unwrap());
        assert_eq!(bearer(&headers), Some(token.as_str()));
    }

    #[test]
    fn idempotency_rejects_duplicate_and_combined_keys() {
        let mut headers = HeaderMap::new();
        assert_eq!(idempotency_key(&headers), None);
        headers.insert("idempotency-key", "request-123".parse().unwrap());
        assert_eq!(idempotency_key(&headers), Some("request-123"));
        headers.append("idempotency-key", "request-456".parse().unwrap());
        assert_eq!(idempotency_key(&headers), None);
        for value in [
            "request-123, request-456",
            "request-123 ",
            "short",
            "user:alice",
        ] {
            headers.clear();
            headers.insert("idempotency-key", value.parse().unwrap());
            assert_eq!(idempotency_key(&headers), None);
        }
    }

    #[test]
    fn request_dtos_reject_identity_and_privilege_smuggling() {
        for field in ["assigner_id", "owner_id", "can_edit", "relations"] {
            let mut value = json!({"organization_id":"acme","title":"Review access","description":"","status":"todo"});
            value[field] = json!("alice");
            assert!(serde_json::from_value::<CreateTask>(value).is_err());
        }
        assert!(
            serde_json::from_value::<TaskInput>(json!({
                "title":"Review access","description":"","status":"todo","organization_id":"other"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<Ownership>(json!({
                "expected_owner":"alice","new_owner":"bob","actor":"alice"
            }))
            .is_err()
        );
    }

    #[test]
    fn typed_identifiers_refuse_fga_syntax() {
        for value in [
            "user:alice",
            "bob#owner",
            "*",
            "../other",
            "",
            "task/review",
        ] {
            assert!(serde_json::from_value::<UserId>(json!(value)).is_err());
            assert!(serde_json::from_value::<TaskId>(json!(value)).is_err());
            assert!(serde_json::from_value::<OrganizationId>(json!(value)).is_err());
            assert!(
                serde_json::from_value::<Ownership>(json!({
                    "expected_owner":"alice","new_owner":value
                }))
                .is_err()
            );
        }
        assert!(serde_json::from_value::<OrganizationId>(json!("acme-123")).is_ok());
    }
}
