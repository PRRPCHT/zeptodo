use anyhow::{Context, Result, bail};
use std::env;

const SESSION_SECRET_MIN_BYTES: usize = 32;

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
    pub behind_proxy: bool,
}

impl Config {
    /// Load configuration from the process environment.
    ///
    /// ### Returns
    /// - `Ok(Config)`: All required variables are present, non-empty, and valid.
    /// - `Err`: A required variable is missing or empty, or `SESSION_SECRET`
    ///   is shorter than 32 bytes.
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            bind_addr: required("BIND_ADDR")?,
            database_url: required("DATABASE_URL")?,
            base_url: required("BASE_URL")?,
            session_secret: validated_session_secret(required("SESSION_SECRET")?)?,
            username: optional("USERNAME"),
            password: optional("PASSWORD"),
            timezone: optional("TIMEZONE"),
            log_dir: optional("LOG_DIR"),
            behind_proxy: env_flag("BEHIND_PROXY"),
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

/// Read a required environment variable, treating empty strings as missing.
///
/// ### Arguments
/// - `key`: Name of the environment variable to read.
///
/// ### Returns
/// - `Ok(String)`: The non-empty value of the variable.
/// - `Err`: The variable is unset or set to an empty string.
fn required(key: &str) -> Result<String> {
    let value = env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .with_context(|| format!("environment variable {key} is required"))?;
    Ok(value)
}

/// Read an optional environment variable, treating empty strings as unset.
///
/// ### Arguments
/// - `key`: Name of the environment variable to read.
///
/// ### Returns
/// - `Some(String)`: The variable is set to a non-empty value.
/// - `None`: The variable is unset or set to the empty string.
fn optional(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.is_empty())
}

/// Read an environment variable as a boolean flag, defaulting to `false`.
///
/// ### Arguments
/// - `key`: Name of the environment variable to read.
///
/// ### Returns
/// - `bool`: `true` only when the value is one of the accepted truthy tokens.
fn env_flag(key: &str) -> bool {
    match optional(key) {
        Some(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        None => false,
    }
}

/// Validate that the session secret is long enough for cookie signing.
///
/// ### Arguments
/// - `secret`: The raw `SESSION_SECRET` value.
///
/// ### Returns
/// - `Ok(String)`: The secret, at least 32 bytes long.
/// - `Err`: The secret is shorter than 32 bytes.
fn validated_session_secret(secret: String) -> Result<String> {
    if secret.len() < SESSION_SECRET_MIN_BYTES {
        bail!(
            "SESSION_SECRET must be at least {SESSION_SECRET_MIN_BYTES} bytes, got {}",
            secret.len()
        );
    }
    Ok(secret)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_secret_shorter_than_32_bytes_is_rejected() {
        let result = validated_session_secret("x".repeat(31));
        assert!(result.is_err());
        let message = result.unwrap_err().to_string();
        assert!(message.contains("at least 32 bytes"));
    }

    #[test]
    fn session_secret_of_32_bytes_is_accepted() {
        let secret = "x".repeat(32);
        assert_eq!(validated_session_secret(secret.clone()).unwrap(), secret);
    }

    #[test]
    fn env_flag_reads_truthy_and_falsy_values() {
        let key = "ZEPTODO_TEST_BEHIND_PROXY_FLAG";
        for truthy in ["1", "true", "TRUE", " yes ", "On"] {
            unsafe { env::set_var(key, truthy) };
            assert!(env_flag(key), "expected {truthy:?} to be truthy");
        }
        for falsy in ["0", "false", "no", "off", "", "banana"] {
            unsafe { env::set_var(key, falsy) };
            assert!(!env_flag(key), "expected {falsy:?} to be falsy");
        }
        unsafe { env::remove_var(key) };
        assert!(!env_flag(key));
    }
}
