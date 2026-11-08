use crate::application::{App, CatalogTable, Identity, TableInput, TableMetadata};
use crate::domain::{
    NamespaceId, Privilege, ProjectId, RoleId, Scope, Subject, TableAction, TableId, UserId,
    WarehouseId,
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, MatchedPath, Path, Query, Request, State},
    http::{HeaderMap, Method, StatusCode, header::AUTHORIZATION},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use serde_json::json;

mod auth;
pub(crate) use auth::guard;

pub(crate) const TABLES: &str = "/projects/{project}/warehouses/{warehouse}/tables";
pub(crate) const TABLE: &str = "/projects/{project}/warehouses/{warehouse}/tables/{id}";
pub(crate) const GRANTS: &str =
    "/projects/{project}/warehouses/{warehouse}/tables/{id}/grants/{relation}";
pub(crate) const OWNER: &str = "/projects/{project}/warehouses/{warehouse}/tables/{id}/owner";
pub(crate) const NAMESPACE: &str =
    "/projects/{project}/warehouses/{warehouse}/tables/{id}/namespace";
pub(crate) const CHECK: &str = "/projects/{project}/warehouses/{warehouse}/tables/{id}/check";
pub(crate) const ROLE_ASSIGNEE: &str = "/projects/{project}/roles/{role}/assignees/{user}";

#[derive(Deserialize)]
pub(crate) struct ScopePath {
    project: ProjectId,
    warehouse: WarehouseId,
}
impl ScopePath {
    fn scope(&self) -> Scope {
        Scope {
            project: self.project.clone(),
            warehouse: self.warehouse.clone(),
        }
    }
}
#[derive(Deserialize)]
pub(crate) struct TablePath {
    project: ProjectId,
    warehouse: WarehouseId,
    id: TableId,
}
impl TablePath {
    pub(crate) fn scope(&self) -> Scope {
        Scope {
            project: self.project.clone(),
            warehouse: self.warehouse.clone(),
        }
    }
}
#[derive(Deserialize)]
struct GrantPath {
    project: ProjectId,
    warehouse: WarehouseId,
    id: TableId,
    relation: String,
}
#[derive(Deserialize)]
struct RolePath {
    project: ProjectId,
    role: RoleId,
    user: UserId,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerInput {
    owner_id: UserId,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NamespaceInput {
    namespace_id: NamespaceId,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckInput {
    action: String,
}
#[derive(Deserialize)]
#[serde(
    tag = "type",
    content = "id",
    rename_all = "lowercase",
    deny_unknown_fields
)]
enum SubjectInput {
    User(UserId),
    Role(RoleId),
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantInput {
    subject: SubjectInput,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListQuery {
    cursor: Option<String>,
    limit: Option<u32>,
}

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
    complete(app.logout(token).await)
}
fn error_response(error: crate::use_cases::Error) -> Response {
    use crate::use_cases::Error;
    let status = match error {
        Error::Invalid => StatusCode::BAD_REQUEST,
        Error::NotFound => StatusCode::NOT_FOUND,
        Error::Conflict => StatusCode::CONFLICT,
        Error::Gone => StatusCode::GONE,
        Error::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        Error::Forbidden => StatusCode::FORBIDDEN,
        Error::Unauthenticated => StatusCode::UNAUTHORIZED,
        Error::Throttled => StatusCode::TOO_MANY_REQUESTS,
    };
    (status, Json(json!({"error":error.to_string()}))).into_response()
}
fn complete(result: Result<(), crate::use_cases::Error>) -> Response {
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => error_response(error),
    }
}
async fn catalog(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
) -> Response {
    match app.catalog(&identity.0).await {
        Ok(value) => Json(value).into_response(),
        Err(error) => error_response(error),
    }
}
async fn list(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path(path): Path<ScopePath>,
    Query(query): Query<ListQuery>,
) -> Response {
    match app
        .visible_tables(
            &identity.0,
            &path.scope(),
            query.cursor.as_deref(),
            query.limit.unwrap_or(50),
        )
        .await
    {
        Ok(page) => Json(page).into_response(),
        Err(error) => error_response(error),
    }
}
async fn table(axum::Extension(table): axum::Extension<CatalogTable>) -> Response {
    Json(table).into_response()
}
async fn create(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path(path): Path<ScopePath>,
    headers: HeaderMap,
    Json(input): Json<TableInput>,
) -> Response {
    let Some(key) = headers.get("idempotency-key").and_then(|v| v.to_str().ok()) else {
        return error_response(crate::use_cases::Error::Invalid);
    };
    match app
        .create_table(&identity.0, &path.scope(), key, &input)
        .await
    {
        Ok(id) => (StatusCode::CREATED, Json(json!({"id":id}))).into_response(),
        Err(error) => error_response(error),
    }
}
async fn edit(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path(path): Path<TablePath>,
    Json(input): Json<TableMetadata>,
) -> Response {
    complete(
        app.edit_table(&identity.0, &path.scope(), &path.id, &input)
            .await,
    )
}
async fn delete_table(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path(path): Path<TablePath>,
) -> Response {
    complete(app.delete_table(&identity.0, &path.scope(), &path.id).await)
}
async fn grant(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path(path): Path<GrantPath>,
    method: Method,
    Json(input): Json<GrantInput>,
) -> Response {
    let Some(privilege) = Privilege::parse(&path.relation) else {
        return error_response(crate::use_cases::Error::Invalid);
    };
    let subject = match input.subject {
        SubjectInput::User(id) => Subject::User(id),
        SubjectInput::Role(id) => Subject::Role(id),
    };
    complete(
        app.set_grant(
            &identity.0,
            &Scope {
                project: path.project,
                warehouse: path.warehouse,
            },
            &path.id,
            privilege,
            &subject,
            method == Method::PUT,
        )
        .await,
    )
}
async fn owner(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path(path): Path<TablePath>,
    Json(input): Json<OwnerInput>,
) -> Response {
    complete(
        app.transfer_owner(&identity.0, &path.scope(), &path.id, &input.owner_id)
            .await,
    )
}
async fn namespace(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path(path): Path<TablePath>,
    Json(input): Json<NamespaceInput>,
) -> Response {
    complete(
        app.move_table(&identity.0, &path.scope(), &path.id, &input.namespace_id)
            .await,
    )
}
async fn role_assignee(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path(path): Path<RolePath>,
    method: Method,
) -> Response {
    complete(
        app.set_role_assignee(
            &identity.0,
            &path.project,
            &path.role,
            &path.user,
            method == Method::PUT,
        )
        .await,
    )
}
async fn check(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path(path): Path<TablePath>,
    Json(input): Json<CheckInput>,
) -> Response {
    let Some(action) = TableAction::parse(&input.action) else {
        return error_response(crate::use_cases::Error::Invalid);
    };
    match app
        .check_table(&identity.0, &path.scope(), &path.id, action)
        .await
    {
        Ok(value) => Json(value).into_response(),
        Err(error) => error_response(error),
    }
}
pub(crate) fn router(app: App) -> Router {
    Router::new()
        .route("/catalog", get(catalog))
        .route(TABLES, get(list).post(create))
        .route(TABLE, get(table).patch(edit).delete(delete_table))
        .route(GRANTS, axum::routing::put(grant).delete(grant))
        .route(OWNER, axum::routing::put(owner))
        .route(NAMESPACE, axum::routing::put(namespace))
        .route(CHECK, axum::routing::post(check))
        .route(
            ROLE_ASSIGNEE,
            axum::routing::put(role_assignee).delete(role_assignee),
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
        .route("/healthz", get(|| async { "ok" }))
        .route("/metrics", get(metrics))
        .route("/session", axum::routing::post(login).delete(logout))
        .layer(DefaultBodyLimit::max(16 * 1024))
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
    headers.insert(axum::http::header::CONTENT_SECURITY_POLICY,axum::http::HeaderValue::from_static("default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'"));
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
    fn request_dtos_reject_identity_and_privilege_smuggling() {
        assert!(
            serde_json::from_value::<TableInput>(json!({
                "namespace_id":"metrics","name":"Requests","description":"",
                "owner_id":"alice"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<CheckInput>(json!({
                "action":"can_get_metadata","subject":"user:alice"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<GrantInput>(json!({
                "subject":{"type":"user","id":"eve","project_id":"other"}
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<GrantInput>(json!({
                "subject":{"type":"role","id":"analysts#assignee"}
            }))
            .is_err()
        );
    }

    #[test]
    fn typed_paths_and_subjects_refuse_fga_syntax() {
        assert!(serde_json::from_value::<OwnerInput>(json!({"owner_id":"user:alice"})).is_err());
        assert!(
            serde_json::from_value::<NamespaceInput>(json!({"namespace_id":"../other"})).is_err()
        );
        assert!(
            serde_json::from_value::<GrantInput>(json!({"subject":{"type":"service","id":"bot"}}))
                .is_err()
        );
        assert!(
            serde_json::from_value::<GrantInput>(
                json!({"subject":{"type":"role","id":"analysts"}})
            )
            .is_ok()
        );
    }
}
