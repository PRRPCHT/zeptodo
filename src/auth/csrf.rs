use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use subtle::ConstantTimeEq;
use tower_sessions::Session;

const SESSION_KEY: &str = "csrf_token";
const TOKEN_BYTES: usize = 32;

/// Read the per-session CSRF token, generating one if absent.
///
/// ### Arguments
/// - `session`: The user's session.
///
/// ### Returns
/// - `Ok(String)`: The CSRF token to embed in form responses.
/// - `Err`: The session store failed to read or write.
pub async fn token(session: &Session) -> Result<String> {
    if let Some(existing) = session
        .get::<String>(SESSION_KEY)
        .await
        .context("session read")?
    {
        return Ok(existing);
    }
    let mut bytes = [0u8; TOKEN_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    let token = URL_SAFE_NO_PAD.encode(bytes);
    session
        .insert(SESSION_KEY, &token)
        .await
        .context("session write")?;
    Ok(token)
}

/// Verify that a submitted token matches the one stored in the session.
///
/// ### Arguments
/// - `session`: The user's session.
/// - `submitted`: The value of the `_csrf` form field.
///
/// ### Returns
/// - `Ok(true)`: The token is present and matches.
/// - `Ok(false)`: No token in session, or values differ.
/// - `Err`: The session store failed to read.
pub async fn verify(session: &Session, submitted: &str) -> Result<bool> {
    let Some(stored) = session
        .get::<String>(SESSION_KEY)
        .await
        .context("session read")?
    else {
        return Ok(false);
    };
    Ok(stored.as_bytes().ct_eq(submitted.as_bytes()).into())
}
