use crate::application::{App, AuthorizationTarget, Authorizer};
use crate::infrastructure::DenyAll;
#[cfg(feature = "grpc")]
use crate::infrastructure::OpenFgaGrpc as OpenFga;
#[cfg(all(feature = "http", not(feature = "grpc")))]
use crate::infrastructure::OpenFgaHttp as OpenFga;
use crate::{cached_authorizer, configuration, http, infrastructure, metrics, operations};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::postgres::PgPoolOptions;
use std::path::Path;
use std::{sync::Arc, time::Duration};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    store_id: String,
    model_id: String,
    model_sha256: String,
}

fn target(config: Config) -> Result<AuthorizationTarget, Box<dyn std::error::Error + Send + Sync>> {
    let valid_id = |id: &str| id.len() == 26 && id.bytes().all(|byte| byte.is_ascii_alphanumeric());
    if !valid_id(&config.store_id) || !valid_id(&config.model_id) {
        return Err("invalid OpenFGA store/model identifiers".into());
    }
    let expected = Sha256::digest(include_bytes!("../fga/model.json"))
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if config.model_sha256 != expected {
        return Err(
            "configured model differs from the pinned catalog model; explicit migration required"
                .into(),
        );
    }
    Ok(AuthorizationTarget {
        store: config.store_id,
        model: config.model_id,
        model_sha256: config.model_sha256,
    })
}

pub async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();
    // Validate the kill switch before any initialization side effects. The
    // explicit deny provider must also skip optional demo migration/recovery,
    // so it remains usable while OpenFGA is unavailable.
    let mode = match std::env::var("AUTHZ_MODE") {
        Ok(mode) => mode,
        Err(std::env::VarError::NotPresent) => "openfga".into(),
        Err(error) => return Err(error.into()),
    };
    if !matches!(mode.as_str(), "openfga" | "deny") {
        return Err("AUTHZ_MODE must be openfga or deny".into());
    }
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
    let authorization_target = target(serde_json::from_slice(&std::fs::read(&path)?)?)?;
    if mode == "openfga" && std::env::var("DEMO_MIGRATE").as_deref() == Ok("true") {
        let http_url =
            std::env::var("FGA_HTTP_URL").unwrap_or_else(|_| "http://openfga:8080".into());
        let model_path =
            std::env::var("FGA_MODEL").unwrap_or_else(|_| "/app/fga/model.json".into());
        crate::authorization_migration::migrate(
            &db,
            &http_url,
            Path::new(&path),
            Path::new(&model_path),
        )
        .await?;
    }
    let auth: Arc<dyn Authorizer> = match mode.as_str() {
        "deny" => Arc::new(DenyAll),
        "openfga" => {
            #[cfg(feature = "grpc")]
            {
                let url = std::env::var("FGA_URL").unwrap_or_else(|_| "http://openfga:8081".into());
                let store = authorization_target.store.clone();
                let model = authorization_target.model.clone();
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
                let store = authorization_target.store.clone();
                let model = authorization_target.model.clone();
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
    let metrics = metrics::Metrics::new()?;
    let mut auth = auth;
    if mode == "openfga" {
        // Catalog mutations can hold a connection from the main pool while
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
        if crate::authorization_migration::ensure_active(
            &db,
            &authorization_target.store,
            &authorization_target.model,
            &authorization_target.model_sha256,
        )
        .await
        .is_ok()
        {
            infrastructure::recover_pending(&db, auth.as_ref()).await?;
        } else {
            tracing::warn!(
                "authorization migration is inactive; catalog and readiness remain closed"
            );
        }
    }
    let readiness =
        operations::Readiness::new(db.clone(), auth.clone(), authorization_target.clone());
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
            authorization_target,
        })
        .merge(operations::router(readiness))
        .layer(axum::middleware::from_fn(http::security_headers)),
    )
    .await?;
    Ok(())
}
