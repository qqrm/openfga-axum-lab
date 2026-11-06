use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::{MatchedPath, Path, Request, State},
    http::{HeaderMap, Method, StatusCode, header::AUTHORIZATION},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};

#[derive(Debug)]
struct AuthError;
#[derive(Serialize)]
struct Tuple {
    user: String,
    relation: String,
    object: String,
}
#[async_trait]
trait Authorizer: Send + Sync {
    async fn check(&self, user: &str, relation: &str, object: &str) -> Result<bool, AuthError>;
    async fn write(&self, tuples: &[Tuple], add: bool) -> Result<(), AuthError>;
}
// Emergency kill switch. It never grants access or silently falls back.
struct DenyAll;
#[async_trait]
impl Authorizer for DenyAll {
    async fn check(&self, _: &str, _: &str, _: &str) -> Result<bool, AuthError> {
        Ok(false)
    }
    async fn write(&self, _: &[Tuple], _: bool) -> Result<(), AuthError> {
        Err(AuthError)
    }
}
struct OpenFga {
    client: reqwest::Client,
    url: String,
    store: String,
    model: String,
}
#[derive(Deserialize)]
struct Check {
    allowed: bool,
}
#[async_trait]
impl Authorizer for OpenFga {
    async fn check(&self, user: &str, relation: &str, object: &str) -> Result<bool, AuthError> {
        let response = self
            .client
            .post(format!("{}/stores/{}/check", self.url, self.store))
            .json(
                &json!({"authorization_model_id":self.model,"consistency":"HIGHER_CONSISTENCY",
                "tuple_key":{"user":format!("user:{user}"),"relation":relation,"object":object}}),
            )
            .send()
            .await
            .map_err(|_| AuthError)?
            .error_for_status()
            .map_err(|_| AuthError)?
            .json::<Check>()
            .await
            .map_err(|_| AuthError)?;
        Ok(response.allowed)
    }
    async fn write(&self, tuples: &[Tuple], add: bool) -> Result<(), AuthError> {
        let mut data = json!({"authorization_model_id":self.model});
        data[if add { "writes" } else { "deletes" }] = if add {
            json!({"tuple_keys":tuples, "on_duplicate":"ignore"})
        } else {
            json!({"tuple_keys":tuples, "on_missing":"ignore"})
        };
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
    db: PgPool,
    auth: Arc<dyn Authorizer>,
}
#[derive(Clone)]
struct Identity(String);
#[derive(Serialize)]
struct Pet {
    id: String,
    name: String,
    species: String,
    biography: String,
}
#[derive(Deserialize)]
struct PetInput {
    name: String,
    species: String,
    biography: String,
}

fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
fn valid_input(pet: &PetInput) -> bool {
    !pet.name.trim().is_empty()
        && pet.name.len() <= 100
        && !pet.species.trim().is_empty()
        && pet.species.len() <= 50
        && pet.biography.len() <= 2000
}
fn hash_token(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}
async fn authenticate(db: &PgPool, headers: &HeaderMap) -> Result<Option<String>, sqlx::Error> {
    let token = headers
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .filter(|v| !v.is_empty() && v.len() <= 256);
    let Some(token) = token else { return Ok(None) };
    sqlx::query_scalar("SELECT id FROM users WHERE token_hash = $1")
        .bind(hash_token(token))
        .fetch_optional(db)
        .await
}
async fn guard(State(app): State<App>, mut request: Request, next: Next) -> Response {
    let who = match authenticate(&app.db, request.headers()).await {
        Ok(Some(user)) => user,
        Ok(None) => return StatusCode::UNAUTHORIZED.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .unwrap_or("");
    let (relation, object) = match (request.method(), path) {
        (&Method::GET, "/pets") => {
            request.extensions_mut().insert(Identity(who));
            return next.run(request).await;
        }
        (&Method::POST, "/pets") => ("can_create", "shelter:central".to_owned()),
        (&Method::GET, "/pets/{id}") => ("can_view", String::new()),
        (&Method::PATCH, "/pets/{id}") => ("can_edit", String::new()),
        (&Method::DELETE, "/pets/{id}") => ("can_manage", String::new()),
        (&Method::PUT | &Method::DELETE, "/pets/{id}/relations/{relation}/{subject}") => {
            ("can_manage", String::new())
        }
        _ => return StatusCode::FORBIDDEN.into_response(),
    };
    let object = if object.is_empty() {
        let id = request.uri().path().split('/').nth(2).unwrap_or("");
        if !valid_id(id) {
            return StatusCode::BAD_REQUEST.into_response();
        }
        let exists: Result<bool, _> = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pets WHERE id = $1 AND status = 'active')",
        )
        .bind(id)
        .fetch_one(&app.db)
        .await;
        match exists {
            Ok(true) => format!("pet:{id}"),
            Ok(false) => return StatusCode::NOT_FOUND.into_response(),
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        }
    } else {
        object
    };
    match app.auth.check(&who, relation, &object).await {
        Ok(true) => {
            request.extensions_mut().insert(Identity(who));
            next.run(request).await
        }
        Ok(false) => StatusCode::FORBIDDEN.into_response(),
        Err(_) => {
            tracing::warn!("authorization backend unavailable");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}
fn row_pet(row: sqlx::postgres::PgRow) -> Pet {
    Pet {
        id: row.get("id"),
        name: row.get("name"),
        species: row.get("species"),
        biography: row.get("biography"),
    }
}
async fn list(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
) -> Response {
    let rows = match sqlx::query(
        "SELECT id, name, species, biography FROM pets WHERE status = 'active' ORDER BY id",
    )
    .fetch_all(&app.db)
    .await
    {
        Ok(rows) => rows,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let mut pets = Vec::new();
    for row in rows {
        let pet = row_pet(row);
        match app
            .auth
            .check(&identity.0, "can_view", &format!("pet:{}", pet.id))
            .await
        {
            Ok(true) => pets.push(pet),
            Ok(false) => {}
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        }
    }
    Json(pets).into_response()
}
async fn pet(State(app): State<App>, Path(id): Path<String>) -> Response {
    match sqlx::query(
        "SELECT id, name, species, biography FROM pets WHERE id = $1 AND status = 'active'",
    )
    .bind(id)
    .fetch_optional(&app.db)
    .await
    {
        Ok(Some(row)) => Json(row_pet(row)).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}
async fn create(
    State(app): State<App>,
    axum::Extension(identity): axum::Extension<Identity>,
    Json(input): Json<PetInput>,
) -> Response {
    if !valid_input(&input) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let id = uuid::Uuid::new_v4().to_string();
    let inserted = sqlx::query("INSERT INTO pets (id,name,species,biography,owner_id,status) VALUES ($1,$2,$3,$4,$5,'pending')")
        .bind(&id).bind(&input.name).bind(&input.species).bind(&input.biography).bind(&identity.0)
        .execute(&app.db).await;
    if inserted.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let object = format!("pet:{id}");
    let tuples = [
        Tuple {
            user: format!("user:{}", identity.0),
            relation: "owner".into(),
            object: object.clone(),
        },
        Tuple {
            user: "shelter:central".into(),
            relation: "shelter".into(),
            object,
        },
    ];
    if app.auth.write(&tuples, true).await.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if sqlx::query("UPDATE pets SET status='active' WHERE id=$1 AND status='pending'")
        .bind(&id)
        .execute(&app.db)
        .await
        .is_err()
    {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    (StatusCode::CREATED, Json(json!({"id":id}))).into_response()
}
async fn edit(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(input): Json<PetInput>,
) -> Response {
    if !valid_input(&input) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match sqlx::query(
        "UPDATE pets SET name=$2,species=$3,biography=$4 WHERE id=$1 AND status='active'",
    )
    .bind(id)
    .bind(input.name)
    .bind(input.species)
    .bind(input.biography)
    .execute(&app.db)
    .await
    {
        Ok(result) if result.rows_affected() == 1 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}
async fn delete_pet(State(app): State<App>, Path(id): Path<String>) -> Response {
    match sqlx::query("UPDATE pets SET status='deleted' WHERE id=$1 AND status='active'")
        .bind(id)
        .execute(&app.db)
        .await
    {
        Ok(result) if result.rows_affected() == 1 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}
async fn relation(
    State(app): State<App>,
    Path((id, relation, subject)): Path<(String, String, String)>,
    method: Method,
) -> Response {
    if !valid_id(&subject) || !matches!(relation.as_str(), "viewer" | "caregiver") {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let exists: Result<bool, _> =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id=$1)")
            .bind(&subject)
            .fetch_one(&app.db)
            .await;
    match exists {
        Ok(true) => {}
        Ok(false) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    let tuple = Tuple {
        user: format!("user:{subject}"),
        relation,
        object: format!("pet:{id}"),
    };
    match app.auth.write(&[tuple], method == Method::PUT).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}
fn router(app: App) -> Router {
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
            get(|| async { Html(include_str!("../assets/index.html")) }),
        )
        .route("/healthz", get(|| async { "ok" }))
        .with_state(app)
}
async fn seed(db: &PgPool) -> Result<(), sqlx::Error> {
    for (id, name, token) in [
        ("alice", "Alice", "demo-alice"),
        ("bob", "Bob", "demo-bob"),
        ("carol", "Carol", "demo-carol"),
        ("dave", "Dave", "demo-dave"),
    ] {
        sqlx::query("INSERT INTO users (id,display_name,token_hash) VALUES ($1,$2,$3) ON CONFLICT (id) DO NOTHING")
            .bind(id).bind(name).bind(hash_token(token)).execute(db).await?;
    }
    for (id, name, species, biography, owner) in [
        (
            "fox",
            "Mika",
            "fox",
            "A curious fox who likes quiet afternoons.",
            "carol",
        ),
        (
            "cat",
            "Pixel",
            "cat",
            "A sleepy cat with a loud purr.",
            "alice",
        ),
        (
            "dog",
            "Rex",
            "dog",
            "A friendly dog who loves walks.",
            "bob",
        ),
    ] {
        sqlx::query("INSERT INTO pets (id,name,species,biography,owner_id,status) VALUES ($1,$2,$3,$4,$5,'active') ON CONFLICT (id) DO NOTHING")
            .bind(id).bind(name).bind(species).bind(biography).bind(owner).execute(db).await?;
    }
    Ok(())
}
async fn recover_pending(
    db: &PgPool,
    auth: &dyn Authorizer,
) -> Result<(), Box<dyn std::error::Error>> {
    let rows = sqlx::query("SELECT id, owner_id FROM pets WHERE status='pending' ORDER BY id")
        .fetch_all(db)
        .await?;
    for row in rows {
        let id: String = row.get("id");
        let owner: String = row.get("owner_id");
        let object = format!("pet:{id}");
        let tuples = [
            Tuple {
                user: format!("user:{owner}"),
                relation: "owner".into(),
                object: object.clone(),
            },
            Tuple {
                user: "shelter:central".into(),
                relation: "shelter".into(),
                object,
            },
        ];
        auth.write(&tuples, true)
            .await
            .map_err(|_| "pending authorization recovery failed")?;
        sqlx::query("UPDATE pets SET status='active' WHERE id=$1 AND status='pending'")
            .bind(id)
            .execute(db)
            .await?;
    }
    Ok(())
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    let database = std::env::var("DATABASE_URL")?;
    let db = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database)
        .await?;
    let migrations = std::env::var("MIGRATIONS_DIR").unwrap_or_else(|_| "migrations".into());
    sqlx::migrate::Migrator::new(std::path::Path::new(&migrations))
        .await?
        .run(&db)
        .await?;
    seed(&db).await?;
    let path = std::env::var("FGA_CONFIG").unwrap_or_else(|_| "/data/config.json".into());
    let config: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let val = |k: &str| -> Result<String, Box<dyn std::error::Error>> {
        config[k]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| format!("missing {k}").into())
    };
    let auth: Arc<dyn Authorizer> = match std::env::var("AUTHZ_MODE").as_deref() {
        Ok("deny") => Arc::new(DenyAll),
        Ok("openfga") | Err(std::env::VarError::NotPresent) => Arc::new(OpenFga {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(2))
                .build()?,
            url: std::env::var("FGA_URL").unwrap_or_else(|_| "http://openfga:8080".into()),
            store: val("store_id")?,
            model: val("model_id")?,
        }),
        _ => return Err("AUTHZ_MODE must be openfga or deny".into()),
    };
    if std::env::var("AUTHZ_MODE").as_deref() != Ok("deny") {
        recover_pending(&db, auth.as_ref()).await?;
    }
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(listener, router(App { db, auth })).await?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ids_and_input_are_bounded() {
        assert!(valid_id("fox-123"));
        assert!(!valid_id("pet:fox"));
        assert!(!valid_id(""));
        assert!(!valid_id(&"x".repeat(65)));
        assert!(!valid_input(&PetInput {
            name: "".into(),
            species: "fox".into(),
            biography: "".into()
        }));
    }
}
