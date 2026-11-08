//! Authentication use cases. Tokens are opaque and stored only as hashes.
use crate::domain;
use crate::{application::App, infrastructure::hash_token, use_cases::Error};
use argon2::{Argon2, PasswordVerifier, password_hash::PasswordHash};

impl App {
    #[tracing::instrument(skip(self, token), name = "auth.authenticate")]
    pub(crate) async fn authenticate(&self, token: &str) -> Result<Option<String>, Error> {
        if token.is_empty() || token.len() > 256 {
            return Ok(None);
        }
        sqlx::query_scalar(
            "SELECT user_id FROM sessions WHERE token_hash = $1 AND expires_at > now()",
        )
        .bind(hash_token(token))
        .fetch_optional(&self.db)
        .await
        .map_err(|_| Error::Unavailable)
    }
    #[tracing::instrument(skip(self, username, password), name = "auth.login")]
    pub(crate) async fn login(&self, username: &str, password: String) -> Result<String, Error> {
        if !domain::valid_id(username) || password.is_empty() || password.len() > 1024 {
            return Err(Error::Unauthenticated);
        }
        let permit = self
            .login_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Throttled)?;
        let attempts: i32 = sqlx::query_scalar(
            "INSERT INTO login_attempts(subject,attempts,window_start) VALUES ($1,1,now()) \
             ON CONFLICT(subject) DO UPDATE SET attempts=CASE WHEN login_attempts.window_start < now()-interval '1 minute' THEN 1 ELSE login_attempts.attempts+1 END, \
             window_start=CASE WHEN login_attempts.window_start < now()-interval '1 minute' THEN now() ELSE login_attempts.window_start END RETURNING attempts")
            .bind(username).fetch_one(&self.db).await.map_err(|_| Error::Unavailable)?;
        if attempts > 20 {
            return Err(Error::Throttled);
        }
        let stored: Option<String> = sqlx::query_scalar(
            "SELECT password_hash FROM users WHERE id=$1 AND password_hash IS NOT NULL",
        )
        .bind(username)
        .fetch_optional(&self.db)
        .await
        .map_err(|_| Error::Unavailable)?;
        let exists = stored.is_some();
        let encoded = stored.unwrap_or_else(|| self.dummy_password_hash.to_string());
        let verified = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            PasswordHash::new(&encoded).ok().is_some_and(|hash| {
                Argon2::default()
                    .verify_password(password.as_bytes(), &hash)
                    .is_ok()
            })
        })
        .await
        .map_err(|_| Error::Unavailable)?;
        if !verified || !exists {
            tracing::info!("login failed");
            return Err(Error::Unauthenticated);
        }
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        sqlx::query("INSERT INTO sessions(token_hash,user_id,expires_at) VALUES ($1,$2,now()+interval '12 hours')")
            .bind(hash_token(&token)).bind(username).execute(&self.db).await.map_err(|_| Error::Unavailable)?;
        tracing::info!("login succeeded");
        Ok(token)
    }
    #[tracing::instrument(skip(self, token), name = "auth.logout")]
    pub(crate) async fn logout(&self, token: &str) -> Result<(), Error> {
        if token.is_empty() || token.len() > 256 {
            return Err(Error::Unauthenticated);
        }
        let deleted = sqlx::query("DELETE FROM sessions WHERE token_hash=$1 AND expires_at>now()")
            .bind(hash_token(token))
            .execute(&self.db)
            .await
            .map_err(|_| Error::Unavailable)?
            .rows_affected();
        if deleted == 1 {
            Ok(())
        } else {
            Err(Error::Unauthenticated)
        }
    }
}
