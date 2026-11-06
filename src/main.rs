mod application;
mod authentication;
mod cached_authorizer;
mod configuration;
mod http;
mod infrastructure;
mod metrics;
mod operations;
mod use_cases;

use application::{App, Authorizer};
use infrastructure::{DenyAll, OpenFga};
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use std::{sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    let database = configuration::secret("DATABASE_URL")?;
    let db = PgPoolOptions::new().max_connections(10).connect(&database).await?;
    let migrations = std::env::var("MIGRATIONS_DIR").unwrap_or_else(|_| "migrations".into());
    sqlx::migrate::Migrator::new(std::path::Path::new(&migrations)).await?.run(&db).await?;
    if std::env::var("DEMO_SEED").as_deref() == Ok("true") {
        let password = configuration::secret("DEMO_PASSWORD")?;
        if password.len() < 12 { return Err("DEMO_PASSWORD must have at least 12 bytes".into()); }
        infrastructure::seed(&db, &password).await?;
    }
    let path = std::env::var("FGA_CONFIG").unwrap_or_else(|_| "/data/config.json".into());
    let config: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let readiness = operations::Readiness::new(db.clone(), &config)?;
    let val = |k: &str| -> Result<String, Box<dyn std::error::Error>> {
        config[k].as_str().filter(|s| !s.is_empty()).map(str::to_owned)
            .ok_or_else(|| format!("missing {k}").into())
    };
    let auth: Arc<dyn Authorizer> = match std::env::var("AUTHZ_MODE").as_deref() {
        Ok("deny") => Arc::new(DenyAll),
        Ok("openfga") | Err(std::env::VarError::NotPresent) => Arc::new(OpenFga {
            client: reqwest::Client::builder().timeout(Duration::from_secs(2)).build()?,
            url: std::env::var("FGA_URL").unwrap_or_else(|_| "http://openfga:8080".into()),
            store: val("store_id")?, model: val("model_id")?,
        }),
        _ => return Err("AUTHZ_MODE must be openfga or deny".into()),
    };
    let metrics = metrics::Metrics::new()?;
    let mut auth = auth;
    if std::env::var("AUTHZ_MODE").as_deref() != Ok("deny") {
        // Keep revision queries separate from transactions holding pet row locks.
        let coordinator = PgPoolOptions::new().max_connections(4)
            .acquire_timeout(Duration::from_secs(2)).connect(&database).await?;
        auth = Arc::new(cached_authorizer::CachedAuthorizer::new(auth, coordinator, metrics.authz_cache.clone()));
        infrastructure::recover_pending(&db, auth.as_ref()).await?;
    }
    let address = std::env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".into());
    let listener = tokio::net::TcpListener::bind(address).await?;
    let dummy_password_hash: Arc<str> = infrastructure::hash_password(&uuid::Uuid::new_v4().to_string())?.into();
    println!("Listening on {}", listener.local_addr()?);
    axum::serve(listener, http::router(App {
        db, auth, login_slots: Arc::new(tokio::sync::Semaphore::new(4)), dummy_password_hash, metrics,
    }).merge(operations::router(readiness))).await?;
    Ok(())
}
