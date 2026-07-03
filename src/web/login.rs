use askama::Template;
use axum::Form;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;
use tower_sessions::Session;

use crate::AppState;
use crate::auth::csrf;
use crate::auth::guard::SESSION_USERNAME_KEY;
use crate::auth::password;
use crate::domain::credentials;
use crate::web::layout::{self, LayoutContext};
use crate::web::theme::Theme;

#[derive(Template)]
#[template(path = "login.html")]
struct LoginPage {
    layout: LayoutContext,
    error: Option<&'static str>,
    username: String,
}

/// Decoded body of the `POST /login` form.
#[derive(Debug, Deserialize)]
pub struct LoginForm {
    #[serde(rename = "_csrf")]
    pub csrf: String,
    pub username: String,
    pub password: String,
}

/// Render the login form, or redirect to `/` if the user is already authenticated.
///
/// ### Arguments
/// - `theme`: The resolved theme for this request.
/// - `session`: The tower-sessions session.
///
/// ### Returns
/// - `Response`: A 302 to `/` when authenticated, or a 200 page with the
///   login form when anonymous.
pub async fn get_login(theme: Theme, session: Session) -> Response {
    if session
        .get::<String>(SESSION_USERNAME_KEY)
        .await
        .ok()
        .flatten()
        .is_some()
    {
        return Redirect::to("/").into_response();
    }
    render_login(theme, &session, None, String::new()).await
}

/// Handle a login submission.
///
/// ### Arguments
/// - `state`: Shared application state (pool + config).
/// - `session`: The tower-sessions session.
/// - `theme`: The resolved theme for re-renders on failure.
/// - `form`: The decoded form body (CSRF token + credentials).
///
/// ### Returns
/// - `Response`: A 302 to `/` on success, a re-rendered login page with
///   status 401 on bad credentials, 403 on CSRF mismatch, or 500 on backend
///   errors.
pub async fn post_login(
    State(state): State<AppState>,
    session: Session,
    theme: Theme,
    Form(form): Form<LoginForm>,
) -> Response {
    if !csrf::verify(&session, &form.csrf).await.unwrap_or(false) {
        tracing::warn!(target: "audit", "login rejected: invalid CSRF token");
        return (StatusCode::FORBIDDEN, "invalid CSRF token").into_response();
    }

    let attempted_username = form.username.trim().to_owned();
    let creds = match credentials::get(&state.pool).await {
        Ok(Some(c)) => c,
        Ok(None) => {
            tracing::error!("credentials row missing at login time");
            return render_login(
                theme,
                &session,
                Some("Server is not configured."),
                attempted_username,
            )
            .await;
        }
        Err(err) => {
            tracing::error!(error = %err, "credentials lookup failed");
            return render_login(theme, &session, Some("Internal error."), attempted_username)
                .await;
        }
    };

    // Verify the password unconditionally so that a wrong username and a wrong password take the same time.
    let username_ok = creds.username == attempted_username;
    let password_ok = password::verify(&form.password, &creds.password_hash).unwrap_or(false);

    if !(username_ok && password_ok) {
        tracing::warn!(
            target: "audit",
            username = %attempted_username,
            "login failure"
        );
        return render_login(
            theme,
            &session,
            Some("Invalid username or password."),
            attempted_username,
        )
        .await;
    }

    if let Err(err) = credentials::mark_login(&state.pool).await {
        tracing::error!(error = %err, "failed to update last_login_at");
    }

    if let Err(err) = session.cycle_id().await {
        tracing::error!(error = %err, "failed to cycle session id");
    }
    if let Err(err) = session.insert(SESSION_USERNAME_KEY, &creds.username).await {
        tracing::error!(error = %err, "failed to write session");
        return (StatusCode::INTERNAL_SERVER_ERROR, "session write failed").into_response();
    }

    tracing::info!(target: "audit", username = %creds.username, "login success");
    Redirect::to("/").into_response()
}

/// Clear the session and redirect to the login page.
///
/// ### Arguments
/// - `session`: The tower-sessions session, which will be flushed on success.
/// - `form`: The decoded form body (CSRF token only).
///
/// ### Returns
/// - `Response`: A 302 to `/login` on success, or 403 when the CSRF token does
///   not match the value stored in the session.
pub async fn post_logout(session: Session, Form(form): Form<LogoutForm>) -> Response {
    if !csrf::verify(&session, &form.csrf).await.unwrap_or(false) {
        return (StatusCode::FORBIDDEN, "invalid CSRF token").into_response();
    }
    let username = session
        .get::<String>(SESSION_USERNAME_KEY)
        .await
        .ok()
        .flatten();
    if let Err(err) = session.flush().await {
        tracing::error!(error = %err, "failed to flush session on logout");
    }
    if let Some(name) = username {
        tracing::info!(target: "audit", username = %name, "logout");
    }
    Redirect::to("/login").into_response()
}

/// Decoded body of the `POST /logout` form.
#[derive(Debug, Deserialize)]
pub struct LogoutForm {
    #[serde(rename = "_csrf")]
    pub csrf: String,
}

/// Render the login page with the given context, used both for the initial GET
/// and for re-renders on failure.
///
/// ### Arguments
/// - `theme`: The resolved theme for this request.
/// - `session`: The tower-sessions session, used to mint or read the CSRF token.
/// - `error`: An optional error message to display in an alert at the top of the form.
/// - `username`: Pre-fill value for the username input (useful on failed login).
///
/// ### Returns
/// - `Response`: A 200 page when `error` is `None`, otherwise a 401 page with
///   the alert rendered. 500 if the layout or template rendering fails.
async fn render_login(
    theme: Theme,
    session: &Session,
    error: Option<&'static str>,
    username: String,
) -> Response {
    let layout = match layout::build(theme, session).await {
        Ok(l) => l,
        Err(err) => {
            tracing::error!(error = %err, "layout build failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
    };
    let status = if error.is_some() {
        StatusCode::UNAUTHORIZED
    } else {
        StatusCode::OK
    };
    let body = LoginPage {
        layout,
        error,
        username,
    };
    match body.render() {
        Ok(html) => (status, Html(html)).into_response(),
        Err(err) => {
            tracing::error!(error = %err, "template render failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "template error").into_response()
        }
    }
}
