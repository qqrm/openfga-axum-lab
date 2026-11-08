use crate::application::{App, Identity};
use crate::domain::UserId;
use axum::{
    extract::{FromRequestParts, MatchedPath, Path, Request, State},
    http::{HeaderMap, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

pub(crate) fn bearer(headers: &HeaderMap) -> Option<&str> {
    super::bearer(headers)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Requirement {
    Catalog,
    List,
    Create,
    TableRead,
    TableMutation,
    TableCheck,
    RoleMutation,
    Deny,
}

pub(crate) fn requirement(method: &Method, path: &str) -> Requirement {
    match (method, path) {
        (&Method::GET, "/catalog") => Requirement::Catalog,
        (&Method::GET, super::TABLES) => Requirement::List,
        (&Method::POST, super::TABLES) => Requirement::Create,
        (&Method::GET, super::TABLE) => Requirement::TableRead,
        (&Method::PATCH | &Method::DELETE, super::TABLE)
        | (&Method::PUT | &Method::DELETE, super::GRANTS)
        | (&Method::PUT, super::OWNER | super::NAMESPACE) => Requirement::TableMutation,
        (&Method::POST, super::CHECK) => Requirement::TableCheck,
        (&Method::PUT | &Method::DELETE, super::ROLE_ASSIGNEE) => Requirement::RoleMutation,
        _ => Requirement::Deny,
    }
}

pub(crate) async fn guard(State(app): State<App>, request: Request, next: Next) -> Response {
    let Some(token) = bearer(request.headers()) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let actor = match app.authenticate(token).await {
        Ok(Some(user)) => match UserId::try_from(user) {
            Ok(user) => user,
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        },
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
    let requirement = requirement(request.method(), path);
    if requirement == Requirement::Deny {
        return StatusCode::FORBIDDEN.into_response();
    }
    let (mut parts, body) = request.into_parts();
    if requirement == Requirement::TableRead {
        let Path(path) = match Path::<super::TablePath>::from_request_parts(&mut parts, &app).await
        {
            Ok(path) => path,
            Err(error) => return error.into_response(),
        };
        // The middleware performs the real OpenFGA check and fetches metadata
        // under the same project read lock. Mutations authorize inside their
        // write transaction instead of trusting a stale middleware decision.
        match app.get_table(&actor, &path.scope(), &path.id).await {
            Ok(table) => {
                parts.extensions.insert(table);
            }
            Err(error) => return super::error_response(error),
        }
    }
    parts.extensions.insert(Identity(actor));
    next.run(Request::from_parts(parts, body)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_methods_and_routes_are_denied() {
        assert_eq!(
            requirement(&Method::GET, super::super::TABLE),
            Requirement::TableRead
        );
        assert_eq!(
            requirement(&Method::POST, super::super::TABLES),
            Requirement::Create
        );
        assert_eq!(
            requirement(&Method::DELETE, super::super::GRANTS),
            Requirement::TableMutation
        );
        assert_eq!(
            requirement(&Method::PUT, super::super::ROLE_ASSIGNEE),
            Requirement::RoleMutation
        );
        assert_eq!(
            requirement(&Method::POST, super::super::TABLE),
            Requirement::Deny
        );
        assert_eq!(requirement(&Method::GET, "/unmapped"), Requirement::Deny);
        assert_eq!(
            requirement(&Method::HEAD, super::super::TABLE),
            Requirement::Deny
        );
    }
}
