use super::{
    Credentials, Document, Health, OBJECT, Result, bootstrap, client, credentials_path, fixture,
    valid_token,
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    response::{IntoResponse, Response},
    routing::{get, put},
};
use openfga_axum_lab::{
    application::{Authorizer, Tuple},
    infrastructure::OpenFgaHttp,
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{fs::OpenOptions, io::Write, sync::Arc, time::Duration};

#[derive(Clone)]
struct App {
    db: PgPool,
    auth: Arc<OpenFgaHttp>,
    health: Health,
}

enum ApiError {
    Unauthenticated,
    Forbidden,
    Invalid,
    Unavailable,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error) = match self {
            Self::Unauthenticated => (StatusCode::UNAUTHORIZED, "unauthenticated"),
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            Self::Invalid => (StatusCode::BAD_REQUEST, "invalid_request"),
            Self::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
        };
        (status, Json(json!({"error":error}))).into_response()
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers.get_all(AUTHORIZATION).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    let (scheme, token) = value.to_str().ok()?.split_once(' ')?;
    (scheme.eq_ignore_ascii_case("Bearer") && valid_token(token)).then_some(token)
}

impl App {
    async fn actor(&self, headers: &HeaderMap) -> std::result::Result<String, ApiError> {
        let token = bearer(headers).ok_or(ApiError::Unauthenticated)?;
        let actor: Option<String> = sqlx::query_scalar(
            "SELECT actor FROM browser_concept.sessions WHERE token_hash=$1 AND expires_at>now()",
        )
        .bind(Sha256::digest(token.as_bytes()).to_vec())
        .fetch_optional(&self.db)
        .await
        .map_err(|_| ApiError::Unavailable)?;
        actor.ok_or(ApiError::Unauthenticated)
    }

    async fn check(&self, actor: &str, relation: &str) -> std::result::Result<bool, ApiError> {
        self.auth
            .check(actor, relation, OBJECT)
            .await
            .map_err(|_| ApiError::Unavailable)
    }
}

async fn health_endpoint(State(app): State<App>) -> std::result::Result<Json<Health>, ApiError> {
    sqlx::query("SELECT 1 FROM browser_concept.documents WHERE id='demo'")
        .fetch_one(&app.db)
        .await
        .map_err(|_| ApiError::Unavailable)?;
    app.auth
        .ensure_available()
        .await
        .map_err(|_| ApiError::Unavailable)?;
    Ok(Json(app.health))
}

async fn permissions(
    State(app): State<App>,
    headers: HeaderMap,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    let actor = app.actor(&headers).await?;
    let can_read = app.check(&actor, "can_read").await?;
    let can_share = app.check(&actor, "can_share").await?;
    Ok(Json(
        json!({"actor":actor,"document":"demo","can_read":can_read,"can_share":can_share}),
    ))
}

async fn document(
    State(app): State<App>,
    headers: HeaderMap,
) -> std::result::Result<Json<Document>, ApiError> {
    let actor = app.actor(&headers).await?;
    if !app.check(&actor, "can_read").await? {
        return Err(ApiError::Forbidden);
    }
    let (id, title, body): (String, String, String) =
        sqlx::query_as("SELECT id,title,body FROM browser_concept.documents WHERE id='demo'")
            .fetch_one(&app.db)
            .await
            .map_err(|_| ApiError::Unavailable)?;
    Ok(Json(Document { id, title, body }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Viewer {
    user: String,
}

async fn mutate_viewer(
    app: App,
    headers: HeaderMap,
    input: Viewer,
    add: bool,
) -> std::result::Result<StatusCode, ApiError> {
    let actor = app.actor(&headers).await?;
    if input.user != "bob" {
        return Err(ApiError::Invalid);
    }
    // Owner is immutable in this scenario. The API cannot promote a viewer,
    // write can_share directly, or take the acting identity from the request.
    if !app.check(&actor, "can_share").await? {
        return Err(ApiError::Forbidden);
    }
    app.auth
        .write(
            &[Tuple {
                user: "user:bob".into(),
                relation: "viewer".into(),
                object: OBJECT.into(),
            }],
            add,
        )
        .await
        .map_err(|_| ApiError::Unavailable)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn grant(
    State(app): State<App>,
    headers: HeaderMap,
    input: std::result::Result<Json<Viewer>, JsonRejection>,
) -> std::result::Result<StatusCode, ApiError> {
    let Json(input) = input.map_err(|_| ApiError::Invalid)?;
    mutate_viewer(app, headers, input, true).await
}

async fn revoke(
    State(app): State<App>,
    headers: HeaderMap,
    input: std::result::Result<Json<Viewer>, JsonRejection>,
) -> std::result::Result<StatusCode, ApiError> {
    let Json(input) = input.map_err(|_| ApiError::Invalid)?;
    mutate_viewer(app, headers, input, false).await
}

fn new_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

pub(super) async fn serve() -> Result {
    let fixture = fixture()?;
    let path = credentials_path()?;
    // create_new prevents silently replacing credentials from another run.
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut credentials_file = options.open(path)?;
    let database = std::env::var("DATABASE_URL")?;
    let url = std::env::var("FGA_URL")?.trim_end_matches('/').to_owned();
    let db = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(5))
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET statement_timeout = '5s'")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&database)
        .await?;
    let credentials = Credentials {
        alice: new_token(),
        bob: new_token(),
    };
    let mut transaction = db.begin().await?;
    // This dedicated ephemeral schema must be new. Existing catalog or concept
    // data is never reset or migrated by this binary.
    sqlx::query("CREATE SCHEMA browser_concept")
        .execute(&mut *transaction)
        .await?;
    sqlx::query("CREATE TABLE browser_concept.documents (id text PRIMARY KEY CHECK(id='demo'), title text NOT NULL, body text NOT NULL)")
        .execute(&mut *transaction).await?;
    sqlx::query("CREATE TABLE browser_concept.sessions (token_hash bytea PRIMARY KEY, actor text NOT NULL CHECK(actor IN ('alice','bob')), expires_at timestamptz NOT NULL)")
        .execute(&mut *transaction).await?;
    sqlx::query("INSERT INTO browser_concept.documents(id,title,body) VALUES($1,$2,$3)")
        .bind(&fixture.document.id)
        .bind(&fixture.document.title)
        .bind(&fixture.document.body)
        .execute(&mut *transaction)
        .await?;
    for (actor, token) in [("alice", &credentials.alice), ("bob", &credentials.bob)] {
        sqlx::query("INSERT INTO browser_concept.sessions(token_hash,actor,expires_at) VALUES($1,$2,now()+interval '12 hours')")
            .bind(Sha256::digest(token.as_bytes()).to_vec()).bind(actor)
            .execute(&mut *transaction).await?;
    }
    transaction.commit().await?;
    let (auth, health) = bootstrap::provision(client()?, url).await?;
    serde_json::to_writer(&mut credentials_file, &credentials)?;
    credentials_file.flush()?;
    credentials_file.sync_all()?;
    drop(credentials_file);
    let app = App {
        db,
        auth: Arc::new(auth),
        health,
    };
    let router = Router::new()
        .route("/concept/health", get(health_endpoint))
        .route("/concept/permissions", get(permissions))
        .route("/concept/document", get(document))
        .route("/concept/document/viewer", put(grant).delete(revoke))
        .layer(DefaultBodyLimit::max(1024))
        .with_state(app);
    let address = std::env::var("CONCEPT_LISTEN_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".into());
    let listener = tokio::net::TcpListener::bind(&address).await?;
    tracing::info!(%address, "concept HTTP server listening");
    axum::serve(listener, router).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_requires_one_unambiguous_opaque_bearer() {
        let token = "a".repeat(64);
        let mut headers = HeaderMap::new();
        assert!(bearer(&headers).is_none());
        headers.insert(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
        assert_eq!(bearer(&headers), Some(token.as_str()));
        headers.append(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
        assert!(bearer(&headers).is_none());
        for value in [
            format!("Bearer {token}, Bearer {token}"),
            "Bearer alice".into(),
            format!("Bearer  {token}"),
        ] {
            headers.clear();
            headers.insert(AUTHORIZATION, value.parse().unwrap());
            assert!(bearer(&headers).is_none());
        }
    }

    #[test]
    fn viewer_body_cannot_supply_authority_or_extra_writes() {
        for body in [
            r#"{"user":"bob","actor":"alice"}"#,
            r#"{"user":"bob","relation":"owner"}"#,
            r#"{"user":"bob","user":"alice"}"#,
        ] {
            assert!(serde_json::from_str::<Viewer>(body).is_err());
        }
    }
}
