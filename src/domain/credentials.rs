use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use std::str::FromStr;

use crate::auth::password;
use crate::config::Config;

/// Single-row credentials record, gated by `CHECK (id = 1)` in the schema.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Credentials {
    pub username: String,
    pub password_hash: String,
    pub timezone: String,
    #[allow(dead_code)]
    pub last_login_at: Option<DateTime<Utc>>,
}

/// Load the stored credentials row, if any.
///
/// ### Arguments
/// - `pool`: SQLite pool with migrations already applied.
///
/// ### Returns
/// - `Ok(Some(Credentials))`: First-boot reconciliation has already happened.
/// - `Ok(None)`: No row exists yet (first boot).
/// - `Err`: SQLite query failed.
pub async fn get(pool: &SqlitePool) -> Result<Option<Credentials>> {
    let row = sqlx::query_as::<_, Credentials>(
        "SELECT username, password_hash, timezone, last_login_at FROM credentials WHERE id = 1",
    )
    .fetch_optional(pool)
    .await
    .context("loading credentials")?;
    Ok(row)
}

/// Update `last_login_at` to the current UTC instant.
///
/// ### Arguments
/// - `pool`: SQLite pool with migrations already applied.
///
/// ### Returns
/// - `Ok(())`: Update was applied.
/// - `Err`: SQLite query failed.
pub async fn mark_login(pool: &SqlitePool) -> Result<()> {
    let now = Utc::now();
    sqlx::query("UPDATE credentials SET last_login_at = ? WHERE id = 1")
        .bind(now)
        .execute(pool)
        .await
        .context("updating last_login_at")?;
    Ok(())
}

/// Reconcile the stored credentials row against `USERNAME` / `PASSWORD` /
/// `TIMEZONE` environment variables.
///
/// ### Description
/// Behavior matrix:
///
/// 1. No row, env vars set: create the row with an Argon2id hash and the
///    requested timezone (defaulting to UTC if unset).
/// 2. No row, env vars missing: fail loudly so the operator cannot boot
///    into an unauthenticated app.
/// 3. Row exists: each env var is compared independently against the stored
///    value. Username and timezone compare by string equality; password
///    compares with `argon2::verify` because Argon2id hashes are
///    non-deterministic. Mismatches trigger an in-place UPDATE and an audit
///    log line (never including the plaintext password).
/// 4. Empty or unset env vars mean "no change".
///
/// ### Arguments
/// - `pool`: SQLite pool with migrations already applied.
/// - `cfg`: Loaded environment configuration.
///
/// ### Returns
/// - `Ok(())`: The row exists and matches the requested state.
/// - `Err`: Required env vars were missing on first boot, the timezone was
///   invalid, or a SQL query failed.
pub async fn reconcile(pool: &SqlitePool, cfg: &Config) -> Result<()> {
    let existing = get(pool).await?;
    match existing {
        None => bootstrap(pool, cfg).await,
        Some(current) => update_in_place(pool, cfg, &current).await,
    }
}

/// Create the single credentials row on first boot.
///
/// ### Arguments
/// - `pool`: SQLite pool with migrations already applied.
/// - `cfg`: Loaded environment configuration. Both `USERNAME` and `PASSWORD`
///   must be set; `TIMEZONE` defaults to `UTC` when absent.
///
/// ### Returns
/// - `Ok(())`: The row was inserted.
/// - `Err`: A required env var was missing, the timezone was invalid, or the
///   INSERT failed.
async fn bootstrap(pool: &SqlitePool, cfg: &Config) -> Result<()> {
    let username = cfg
        .username
        .as_deref()
        .ok_or_else(|| anyhow!("USERNAME env var is required for first boot"))?;
    let password = cfg
        .password
        .as_deref()
        .ok_or_else(|| anyhow!("PASSWORD env var is required for first boot"))?;
    let timezone = cfg.timezone.as_deref().unwrap_or("UTC");
    chrono_tz::Tz::from_str(timezone)
        .map_err(|e| anyhow!("invalid TIMEZONE value '{timezone}': {e}"))?;

    let hash = password::hash(password)?;
    sqlx::query(
        "INSERT INTO credentials (id, username, password_hash, timezone) VALUES (1, ?, ?, ?)",
    )
    .bind(username)
    .bind(&hash)
    .bind(timezone)
    .execute(pool)
    .await
    .context("inserting bootstrap credentials")?;

    tracing::info!(
        target: "audit",
        username = %username,
        timezone = %timezone,
        "credentials bootstrapped"
    );
    Ok(())
}

/// Apply per-field updates to the existing credentials row when env vars differ.
///
/// ### Arguments
/// - `pool`: SQLite pool with migrations already applied.
/// - `cfg`: Loaded environment configuration. Unset or empty fields are
///   treated as "no change".
/// - `current`: The currently stored credentials row.
///
/// ### Returns
/// - `Ok(())`: All applicable updates were applied (possibly none).
/// - `Err`: The timezone was invalid or an UPDATE failed.
async fn update_in_place(pool: &SqlitePool, cfg: &Config, current: &Credentials) -> Result<()> {
    if let Some(username) = cfg.username.as_deref()
        && username != current.username
    {
        sqlx::query("UPDATE credentials SET username = ? WHERE id = 1")
            .bind(username)
            .execute(pool)
            .await
            .context("updating username")?;
        tracing::info!(
            target: "audit",
            old = %current.username,
            new = %username,
            "username changed at startup"
        );
    }

    if let Some(password_plain) = cfg.password.as_deref() {
        let matches = password::verify(password_plain, &current.password_hash).unwrap_or(false);
        if !matches {
            let hash = password::hash(password_plain)?;
            sqlx::query("UPDATE credentials SET password_hash = ? WHERE id = 1")
                .bind(&hash)
                .execute(pool)
                .await
                .context("updating password_hash")?;
            tracing::info!(target: "audit", "password changed at startup");
        }
    }

    if let Some(timezone) = cfg.timezone.as_deref()
        && timezone != current.timezone
    {
        chrono_tz::Tz::from_str(timezone)
            .map_err(|e| anyhow!("invalid TIMEZONE value '{timezone}': {e}"))?;
        sqlx::query("UPDATE credentials SET timezone = ? WHERE id = 1")
            .bind(timezone)
            .execute(pool)
            .await
            .context("updating timezone")?;
        tracing::info!(
            target: "audit",
            old = %current.timezone,
            new = %timezone,
            "timezone changed at startup"
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn in_memory_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }

    fn cfg(username: Option<&str>, password: Option<&str>, timezone: Option<&str>) -> Config {
        Config {
            bind_addr: "127.0.0.1:0".into(),
            database_url: "sqlite::memory:".into(),
            base_url: "http://localhost".into(),
            session_secret: "x".repeat(64),
            username: username.map(str::to_string),
            password: password.map(str::to_string),
            timezone: timezone.map(str::to_string),
            log_dir: None,
        }
    }

    #[tokio::test]
    async fn bootstrap_creates_row_when_env_present() {
        let pool = in_memory_pool().await;
        let c = cfg(Some("admin"), Some("secret"), Some("Europe/Paris"));
        reconcile(&pool, &c).await.unwrap();
        let row = get(&pool).await.unwrap().unwrap();
        assert_eq!(row.username, "admin");
        assert_eq!(row.timezone, "Europe/Paris");
        assert!(password::verify("secret", &row.password_hash).unwrap());
    }

    #[tokio::test]
    async fn bootstrap_fails_when_credentials_missing() {
        let pool = in_memory_pool().await;
        let c = cfg(None, None, None);
        assert!(reconcile(&pool, &c).await.is_err());
    }

    #[tokio::test]
    async fn update_changes_username_and_keeps_password() {
        let pool = in_memory_pool().await;
        reconcile(&pool, &cfg(Some("a"), Some("p"), None))
            .await
            .unwrap();
        reconcile(&pool, &cfg(Some("b"), None, None)).await.unwrap();
        let row = get(&pool).await.unwrap().unwrap();
        assert_eq!(row.username, "b");
        assert!(password::verify("p", &row.password_hash).unwrap());
    }

    #[tokio::test]
    async fn update_rehashes_password_when_plaintext_differs() {
        let pool = in_memory_pool().await;
        reconcile(&pool, &cfg(Some("a"), Some("old"), None))
            .await
            .unwrap();
        let before = get(&pool).await.unwrap().unwrap().password_hash;
        reconcile(&pool, &cfg(None, Some("new"), None))
            .await
            .unwrap();
        let after = get(&pool).await.unwrap().unwrap();
        assert_ne!(before, after.password_hash);
        assert!(password::verify("new", &after.password_hash).unwrap());
    }

    #[tokio::test]
    async fn invalid_timezone_rejected() {
        let pool = in_memory_pool().await;
        let res = reconcile(&pool, &cfg(Some("a"), Some("p"), Some("Not/A_Zone"))).await;
        assert!(res.is_err());
    }
}
