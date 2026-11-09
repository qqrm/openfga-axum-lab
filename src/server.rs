use crate::application::{App, AuthorizationTarget, Authorizer};
use crate::infrastructure::DenyAll;
#[cfg(feature = "grpc")]
use crate::infrastructure::OpenFgaGrpc as OpenFga;
#[cfg(all(feature = "http", not(feature = "grpc")))]
use crate::infrastructure::OpenFgaHttp as OpenFga;
use crate::{
    authorization_migration, cached_authorizer, configuration, http, infrastructure, metrics,
    operations,
};
use sqlx::postgres::PgPoolOptions;
use std::{path::Path, sync::Arc, time::Duration};

type RunResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub async fn run() -> RunResult {
    tracing_subscriber::fmt::init();
    if std::env::args().len() != 1 {
        return Err("usage: app; catalog recovery requires explicit graph reconciliation".into());
    }
    // Configuration errors must not seed or modify SQL as a side effect.
    let openfga = match std::env::var("AUTHZ_MODE").as_deref() {
        Ok("openfga") | Err(std::env::VarError::NotPresent) => true,
        Ok("deny") => false,
        _ => return Err("AUTHZ_MODE must be openfga or deny".into()),
    };
    // This never disables the durable SQL fence or fresh Check for an allow.
    let denial_cache_enabled = match std::env::var("AUTHZ_DENIAL_CACHE_ENABLED").as_deref() {
        Ok("true") | Err(std::env::VarError::NotPresent) => true,
        Ok("false") => false,
        _ => return Err("AUTHZ_DENIAL_CACHE_ENABLED must be true or false".into()),
    };
    let config_path = std::env::var("FGA_CONFIG").unwrap_or_else(|_| "/data/config.json".into());
    let model_path = std::env::var("FGA_MODEL").unwrap_or_else(|_| "fga/model.json".into());
    let migration_url = std::env::var("FGA_HTTP_URL").unwrap_or_else(|_| {
        #[cfg(feature = "grpc")]
        {
            "http://openfga:8080".into()
        }
        #[cfg(not(feature = "grpc"))]
        {
            std::env::var("FGA_URL").unwrap_or_else(|_| "http://openfga:8080".into())
        }
    });
    let validated = if openfga {
        let (target, model) = authorization_migration::validate_configuration(
            Path::new(&config_path),
            Path::new(&model_path),
        )?;
        authorization_migration::validate_backend(&migration_url, &target, &model).await?;
        Some(target)
    } else {
        None
    };
    let authorization_target = validated.as_ref().map(|target| AuthorizationTarget {
        store: target.store_id.clone(),
        model: target.model_id.clone(),
        model_sha256: target.model_sha256.clone(),
    });
    let auth: Arc<dyn Authorizer> = if let Some(target) = &validated {
        #[cfg(feature = "grpc")]
        {
            let url = std::env::var("FGA_URL").unwrap_or_else(|_| "http://openfga:8081".into());
            Arc::new(OpenFga::connect(url, &target.store_id, &target.model_id).await?)
        }
        #[cfg(all(feature = "http", not(feature = "grpc")))]
        {
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(2))
                .build()?;
            let url = std::env::var("FGA_URL").unwrap_or_else(|_| "http://openfga:8080".into());
            Arc::new(OpenFga {
                client,
                url,
                store: target.store_id.clone(),
                model: target.model_id.clone(),
            })
        }
        #[cfg(not(any(feature = "grpc", feature = "http")))]
        {
            let _ = target;
            return Err("enable grpc or http feature".into());
        }
    } else {
        Arc::new(DenyAll)
    };
    let demo_password = if std::env::var("DEMO_SEED").as_deref() == Ok("true") {
        let password = configuration::secret("DEMO_PASSWORD")?;
        if password.len() < 12 {
            return Err("DEMO_PASSWORD must have at least 12 bytes".into());
        }
        Some(password)
    } else {
        None
    };
    let database = configuration::secret("DATABASE_URL")?;
    let db = PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&database)
        .await?;
    let migrations = std::env::var("MIGRATIONS_DIR").unwrap_or_else(|_| "migrations".into());
    sqlx::migrate::Migrator::new(Path::new(&migrations))
        .await?
        .run(&db)
        .await?;
    if let Some(password) = demo_password {
        infrastructure::seed(&db, &password).await?;
    }
    if openfga && std::env::var("DEMO_MIGRATE").as_deref() == Ok("true") {
        authorization_migration::migrate(
            &db,
            &migration_url,
            Path::new(&config_path),
            Path::new(&model_path),
        )
        .await?;
    }
    let metrics = metrics::Metrics::new()?;
    let mut auth = auth;
    if let Some(target) = &authorization_target {
        // Domain mutations hold a main-pool transaction while independently
        // committing the durable attempt marker and global cache fence.
        let authz_coordinator = PgPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(2))
            .connect(&database)
            .await?;
        auth = Arc::new(cached_authorizer::CachedAuthorizer::new(
            auth,
            authz_coordinator.clone(),
            metrics.authz_cache.clone(),
            denial_cache_enabled,
        ));
        let active = authorization_migration::ensure_active(
            &db,
            &target.store,
            &target.model,
            &target.model_sha256,
        )
        .await
        .is_ok();
        if active {
            // Replaying acknowledged intent does not clear earlier uncertainty.
            // Wrapped readiness may remain false; the server still listens so
            // health/operations can report the durable fence to an operator.
            infrastructure::recover_pending(&db, auth.as_ref()).await?;
        }
    }
    let readiness =
        operations::Readiness::new(db.clone(), auth.clone(), authorization_target.clone());
    let address = std::env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".into());
    let listener = tokio::net::TcpListener::bind(address).await?;
    let dummy_password_hash: Arc<str> =
        infrastructure::hash_password(&uuid::Uuid::new_v4().to_string())?.into();
    // Register before announcing readiness: an immediate SIGTERM must still
    // drain the server and return from main, including LLVM's profile flush.
    #[cfg(unix)]
    let shutdown = {
        use tokio::signal::unix::{SignalKind, signal};
        let mut interrupt = signal(SignalKind::interrupt())?;
        let mut terminate = signal(SignalKind::terminate())?;
        async move {
            tokio::select! {
                _ = interrupt.recv() => {},
                _ = terminate.recv() => {},
            }
        }
    };
    #[cfg(not(unix))]
    let shutdown = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
    };
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
    .with_graceful_shutdown(shutdown)
    .await?;
    Ok(())
}
