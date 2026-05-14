use anyhow::{Context, Result};
use sqlx::SqlitePool;
use time::Duration;
use tower_sessions::cookie::SameSite;
use tower_sessions::{Expiry, SessionManagerLayer};
use tower_sessions_sqlx_store::SqliteStore;

use crate::config::Config;

const COOKIE_NAME: &str = "zeptodo_sid";
const IDLE_DAYS: i64 = 7;

/// Build the `tower-sessions` layer backed by SQLite.
///
/// ### Arguments
/// - `pool`: Connection pool shared with the application data.
/// - `cfg`: Runtime configuration used to gate the `Secure` cookie attribute.
///
/// ### Returns
/// - `Ok(SessionManagerLayer<SqliteStore>)`: Layer ready to attach to the router.
/// - `Err`: Store migration failed.
pub async fn build_layer(
    pool: SqlitePool,
    cfg: &Config,
) -> Result<SessionManagerLayer<SqliteStore>> {
    let store = SqliteStore::new(pool);
    store
        .migrate()
        .await
        .context("session store migration failed")?;
    Ok(SessionManagerLayer::new(store)
        .with_name(COOKIE_NAME)
        .with_secure(cfg.cookies_secure())
        .with_http_only(true)
        .with_same_site(SameSite::Strict)
        .with_expiry(Expiry::OnInactivity(Duration::days(IDLE_DAYS))))
}
