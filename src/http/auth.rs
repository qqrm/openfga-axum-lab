use crate::application::{App, Identity};
use crate::domain;
use crate::use_cases::Error as UseCaseError;
use axum::{
    extract::{MatchedPath, Request, State},
    http::{HeaderMap, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

pub(crate) fn bearer(headers: &HeaderMap) -> Option<&str> {
    super::bearer(headers)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Requirement {
    AllowAuthenticated,
    ShelterCreation,
    PetMutation,
    Pet { relation: &'static str },
    Deny,
}

pub(crate) fn requirement(method: &Method, path: &str) -> Requirement {
    match (method, path) {
        (&Method::GET, "/pets") => Requirement::AllowAuthenticated,
        (&Method::POST, "/pets") => Requirement::ShelterCreation,
        (&Method::GET, "/pets/{id}") => Requirement::Pet {
            relation: "can_view",
        },
        (&Method::PATCH | &Method::DELETE, "/pets/{id}") => Requirement::PetMutation,
        (&Method::PUT | &Method::DELETE, "/pets/{id}/relations/{relation}/{subject}") => {
            Requirement::PetMutation
        }
        _ => Requirement::Deny,
    }
}

pub(crate) fn pet_id_from_path(path: &str) -> Option<&str> {
    let id = path.split('/').nth(2)?;
    if id.is_empty() { None } else { Some(id) }
}

pub(crate) async fn guard(State(app): State<App>, mut request: Request, next: Next) -> Response {
    let Some(token) = bearer(request.headers()) else {
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
    match requirement(request.method(), path) {
        Requirement::AllowAuthenticated => {
            request.extensions_mut().insert(Identity(who));
            next.run(request).await
        }
        Requirement::Deny => StatusCode::FORBIDDEN.into_response(),
        Requirement::ShelterCreation => {
            finish(
                app.authorize_shelter_creation(&who).await,
                who,
                request,
                next,
            )
            .await
        }
        Requirement::PetMutation => {
            // Mutation handlers check lifecycle and permissions while holding the
            // pet row lock. A separate check here can race with tuple deletion.
            let id = pet_id_from_path(request.uri().path()).unwrap_or("");
            let decision = if domain::valid_id(id) {
                Ok(())
            } else {
                Err(UseCaseError::Invalid)
            };
            finish(decision, who, request, next).await
        }
        Requirement::Pet { relation } => {
            let id = pet_id_from_path(request.uri().path()).unwrap_or("");
            finish(
                app.authorize_pet(&who, id, relation).await,
                who,
                request,
                next,
            )
            .await
        }
    }
}

async fn finish(
    decision: Result<(), UseCaseError>,
    who: String,
    mut request: Request,
    next: Next,
) -> Response {
    match decision {
        Ok(()) => {
            request.extensions_mut().insert(Identity(who));
            next.run(request).await
        }
        Err(error) => {
            match error {
                UseCaseError::Unavailable => tracing::warn!("authorization backend unavailable"),
                UseCaseError::Forbidden => tracing::debug!("authorization denied"),
                _ => {}
            }
            super::error_response(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Method;

    #[test]
    fn maps_routes_to_requirements() {
        assert_eq!(
            requirement(&Method::GET, "/pets"),
            Requirement::AllowAuthenticated
        );
        assert_eq!(
            requirement(&Method::POST, "/pets"),
            Requirement::ShelterCreation
        );
        assert_eq!(
            requirement(&Method::GET, "/pets/{id}"),
            Requirement::Pet {
                relation: "can_view"
            }
        );
        for (method, path) in [
            (Method::PATCH, "/pets/{id}"),
            (Method::DELETE, "/pets/{id}"),
            (Method::PUT, "/pets/{id}/relations/{relation}/{subject}"),
            (Method::DELETE, "/pets/{id}/relations/{relation}/{subject}"),
        ] {
            assert_eq!(requirement(&method, path), Requirement::PetMutation);
        }
        assert_eq!(requirement(&Method::POST, "/pets/{id}"), Requirement::Deny);
    }

    #[test]
    fn extracts_pet_id_from_path() {
        assert_eq!(pet_id_from_path("/pets/fox"), Some("fox"));
        assert_eq!(
            pet_id_from_path("/pets/fox/relations/caregiver/dave"),
            Some("fox")
        );
        assert_eq!(pet_id_from_path("/pets"), None);
        assert_eq!(pet_id_from_path("/pets/"), None);
    }
}
