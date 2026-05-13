use anyhow::{Context, Result};
use std::env;

/// Runtime configuration loaded from environment variables.
///
/// ### Description
/// Empty strings are treated identically to unset variables. This mirrors
/// the credentials rotation pattern where operators temporarily set a value,
/// restart, then blank it out before restarting again.
#[derive(Debug, Clone)]
pub struct Config {
    pub bind_addr: String,
    pub database_url: String,
    pub base_url: String,
    pub session_secret: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub timezone: Option<String>,
    pub log_dir: Option<String>,
}

impl Config {
    /// Load configuration from the process environment.
    ///
    /// ### Returns
    /// - `Ok(Config)`: All required variables are present and non-empty.
    /// - `Err`: A required variable is missing or empty.
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            bind_addr: required("BIND_ADDR")?,
            database_url: required("DATABASE_URL")?,
            base_url: required("BASE_URL")?,
            session_secret: required("SESSION_SECRET")?,
            username: optional("USERNAME"),
            password: optional("PASSWORD"),
            timezone: optional("TIMEZONE"),
            log_dir: optional("LOG_DIR"),
        })
    }

    /// Whether response cookies should set the `Secure` attribute.
    ///
    /// ### Returns
    /// - `bool`: `true` when `BASE_URL` has an `https://` scheme.
    pub fn cookies_secure(&self) -> bool {
        self.base_url.starts_with("https://")
    }
}

fn required(key: &str) -> Result<String> {
    let value = env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .with_context(|| format!("environment variable {key} is required"))?;
    Ok(value)
}

fn optional(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.is_empty())
}
