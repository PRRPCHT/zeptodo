use anyhow::{Context, Result};
use sqlx::SqlitePool;
use time::Duration;
use tower_sessions::cookie::{Key, SameSite};
use tower_sessions::service::SignedCookie;
use tower_sessions::{Expiry, SessionManagerLayer};
use tower_sessions_sqlx_store::SqliteStore;

use crate::config::Config;

const COOKIE_NAME: &str = "zeptodo_sid";
const IDLE_DAYS: i64 = 7;

/// Build the `tower-sessions` layer backed by SQLite, with cookie signing.
///
/// ### Arguments
/// - `pool`: Connection pool shared with the application data.
/// - `cfg`: Runtime configuration providing the signing secret and gating
///   the `Secure` cookie attribute. `Config::from_env` guarantees the
///   secret is at least 32 bytes, which `Key::derive_from` requires.
///
/// ### Returns
/// - `Ok(SessionManagerLayer<SqliteStore, SignedCookie>)`: Layer ready to attach to the router.
/// - `Err`: Store migration failed.
pub async fn build_layer(
    pool: SqlitePool,
    cfg: &Config,
) -> Result<SessionManagerLayer<SqliteStore, SignedCookie>> {
    let store = SqliteStore::new(pool);
    store
        .migrate()
        .await
        .context("session store migration failed")?;
    let signing_key = Key::derive_from(cfg.session_secret.as_bytes());
    Ok(SessionManagerLayer::new(store)
        .with_signed(signing_key)
        .with_name(COOKIE_NAME)
        .with_secure(cfg.cookies_secure())
        .with_http_only(true)
        .with_same_site(SameSite::Strict)
        .with_expiry(Expiry::OnInactivity(Duration::days(IDLE_DAYS))))
}
