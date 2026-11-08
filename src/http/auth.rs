use crate::application::{App, Identity};
use axum::{
    extract::{MatchedPath, Request, State},
    http::{Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

/// Authentication is shared; each domain use case authorizes its own object.
/// Mutators do so under organization/task locks. Readers validate revisions.
fn registered_domain_route(method: &Method, path: &str) -> bool {
    matches!(
        (method, path),
        (&Method::GET | &Method::POST, "/tasks")
            | (
                &Method::GET | &Method::PATCH | &Method::DELETE,
                "/tasks/{id}"
            )
            | (
                &Method::PUT | &Method::DELETE,
                "/tasks/{id}/relations/{relation}/{subject}"
            )
            | (
                &Method::GET,
                "/organizations" | "/organizations/{id}/members"
            )
            | (
                &Method::PUT | &Method::DELETE,
                "/organizations/{id}/members/{subject}"
            )
            | (&Method::PUT, "/organizations/{id}/owner")
            | (
                &Method::PUT | &Method::DELETE,
                "/organizations/{id}/permissions/{relation}/{subject}"
            )
    )
}

pub(crate) async fn guard(State(app): State<App>, mut request: Request, next: Next) -> Response {
    let Some(token) = super::bearer(request.headers()) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let who = match app.authenticate(token).await {
        Ok(Some(user)) => user,
        Ok(None) => return StatusCode::UNAUTHORIZED.into_response(),
        Err(_) => {
            tracing::warn!("authentication backend unavailable");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    };
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .unwrap_or("");
    if !registered_domain_route(request.method(), path) {
        return StatusCode::FORBIDDEN.into_response();
    }
    request.extensions_mut().insert(Identity(who));
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_explicit_domain_routes_are_authenticated_for_dispatch() {
        for (method, path) in [
            (Method::GET, "/tasks"),
            (Method::POST, "/tasks"),
            (Method::GET, "/tasks/{id}"),
            (Method::PATCH, "/tasks/{id}"),
            (Method::DELETE, "/tasks/{id}"),
            (Method::PUT, "/tasks/{id}/relations/{relation}/{subject}"),
            (Method::DELETE, "/tasks/{id}/relations/{relation}/{subject}"),
            (Method::GET, "/organizations"),
            (Method::GET, "/organizations/{id}/members"),
            (Method::PUT, "/organizations/{id}/owner"),
            (Method::PUT, "/organizations/{id}/members/{subject}"),
            (Method::DELETE, "/organizations/{id}/members/{subject}"),
            (
                Method::PUT,
                "/organizations/{id}/permissions/{relation}/{subject}",
            ),
            (
                Method::DELETE,
                "/organizations/{id}/permissions/{relation}/{subject}",
            ),
        ] {
            assert!(registered_domain_route(&method, path), "{method} {path}");
        }
        for (method, path) in [
            (Method::PUT, "/tasks/{id}"),
            (Method::POST, "/organizations"),
            (Method::DELETE, "/organizations/{id}/owner"),
            (Method::GET, "/unknown"),
        ] {
            assert!(!registered_domain_route(&method, path));
        }
    }
}
