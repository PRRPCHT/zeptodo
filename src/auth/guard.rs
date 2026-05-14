use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Redirect, Response};
use tower_sessions::Session;

/// Session key holding the authenticated username.
pub const SESSION_USERNAME_KEY: &str = "username";

/// Extractor for handlers that require an authenticated user.
pub struct AuthedUser(pub String);

impl<S> FromRequestParts<S> for AuthedUser
where
    S: Send + Sync,
{
    type Rejection = Response;

    /// Resolve the authenticated username from the active session.
    ///
    /// ### Arguments
    /// - `parts`: Request parts injected by Axum.
    /// - `state`: Application state, used to load the `Session` extractor.
    ///
    /// ### Returns
    /// - `Ok(AuthedUser)`: A username string was present in the session.
    /// - `Err(Response)`: No session, no username in the session, or the
    ///   session store failed. The response is a 302 to `/login`.
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let session = Session::from_request_parts(parts, state)
            .await
            .map_err(|_| Redirect::to("/login").into_response())?;
        match session.get::<String>(SESSION_USERNAME_KEY).await {
            Ok(Some(name)) => Ok(AuthedUser(name)),
            _ => Err(Redirect::to("/login").into_response()),
        }
    }
}
