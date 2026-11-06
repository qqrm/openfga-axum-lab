mod application;
mod authentication;
mod http;
mod infrastructure;
mod metrics;
mod use_cases;

use application::{App, Authorizer};
use infrastructure::{DenyAll, OpenFga};
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use std::{sync::Arc, time::Duration};

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
    if std::env::var("DEMO_SEED").as_deref() == Ok("true") {
        let password = std::env::var("DEMO_PASSWORD")?;
        if password.len() < 12 {
            return Err("DEMO_PASSWORD must have at least 12 bytes".into());
        }
        infrastructure::seed(&db, &password).await?;
    }
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
        infrastructure::recover_pending(&db, auth.as_ref()).await?;
    }
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    let dummy_password_hash: Arc<str> =
        infrastructure::hash_password("invalid-account-dummy-password")?.into();
    axum::serve(
        listener,
        http::router(App {
            db,
            auth,
            login_slots: Arc::new(tokio::sync::Semaphore::new(4)),
            dummy_password_hash,
            metrics: metrics::Metrics::new()?,
        }),
    )
    .await?;
    Ok(())
}
