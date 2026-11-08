use crate::application::{App, Authorizer};
use crate::infrastructure::DenyAll;
#[cfg(feature = "grpc")]
use crate::infrastructure::OpenFgaGrpc as OpenFga;
#[cfg(all(feature = "http", not(feature = "grpc")))]
use crate::infrastructure::OpenFgaHttp as OpenFga;
use crate::{cached_authorizer, configuration, http, infrastructure, metrics, operations};
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use std::{sync::Arc, time::Duration};

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    let database = configuration::secret("DATABASE_URL")?;
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
        let password = configuration::secret("DEMO_PASSWORD")?;
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
        Ok("openfga") | Err(std::env::VarError::NotPresent) => {
            #[cfg(feature = "grpc")]
            {
                let url = std::env::var("FGA_URL").unwrap_or_else(|_| "http://openfga:8081".into());
                let store = val("store_id")?;
                let model = val("model_id")?;
                Arc::new(OpenFga::connect(url, &store, &model).await?)
            }
            #[cfg(all(feature = "http", not(feature = "grpc")))]
            {
                let client = reqwest::Client::builder()
                    .no_proxy()
                    .redirect(reqwest::redirect::Policy::none())
                    .timeout(Duration::from_secs(2))
                    .build()?;
                let url = std::env::var("FGA_URL").unwrap_or_else(|_| "http://openfga:8080".into());
                let store = val("store_id")?;
                let model = val("model_id")?;
                Arc::new(OpenFga {
                    client,
                    url,
                    store,
                    model,
                })
            }
            #[cfg(not(any(feature = "grpc", feature = "http")))]
            {
                return Err("enable grpc or http feature".into());
            }
        }
        _ => return Err("AUTHZ_MODE must be openfga or deny".into()),
    };
    let readiness = operations::Readiness::new(db.clone(), auth.clone());
    let metrics = metrics::Metrics::new()?;
    let mut auth = auth;
    if std::env::var("AUTHZ_MODE").as_deref() != Ok("deny") {
        // Pet mutations can hold a connection from the main pool while
        // checking authorization. Keep revision queries on a separate pool.
        let coordinator = PgPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(2))
            .connect(&database)
            .await?;
        auth = Arc::new(cached_authorizer::CachedAuthorizer::new(
            auth,
            coordinator,
            metrics.authz_cache.clone(),
        ));
        infrastructure::recover_pending(&db, auth.as_ref()).await?;
    }
    let address = std::env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".into());
    let listener = tokio::net::TcpListener::bind(address).await?;
    // Unknown usernames still execute password verification, using a value
    // generated for this process and unrelated to configured accounts.
    let dummy_password_hash: Arc<str> =
        infrastructure::hash_password(&uuid::Uuid::new_v4().to_string())?.into();
    println!("Listening on {}", listener.local_addr()?);
    axum::serve(
        listener,
        http::router(App {
            db,
            auth,
            login_slots: Arc::new(tokio::sync::Semaphore::new(4)),
            dummy_password_hash,
            metrics,
        })
        .merge(operations::router(readiness)),
    )
    .await?;
    Ok(())
}
