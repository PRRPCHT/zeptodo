use axum::Form;
use axum::extract::FromRequestParts;
use axum::http::header::REFERER;
use axum::http::{HeaderMap, StatusCode, request::Parts};
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::{Cookie, SameSite};
use serde::Deserialize;
use std::fmt;
use tower_sessions::Session;

use crate::AppState;
use crate::auth::csrf;

const THEME_COOKIE: &str = "zeptodo_theme";
const ONE_YEAR_SECONDS: i64 = 60 * 60 * 24 * 365;

/// User-selected colour theme.
///
/// ### Description
/// Resolved from the `zeptodo_theme` cookie. Any unrecognised value, as
/// well as a missing cookie, resolves to `Dark`, matching the product
/// decision that dark mode is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
}

impl Theme {
    /// Return the canonical lowercase identifier emitted into `data-theme`.
    ///
    /// ### Returns
    /// - `&'static str`: `"light"` or `"dark"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }

    /// Parse the raw cookie value into a [`Theme`], defaulting to dark on any unknown value.
    ///
    /// ### Arguments
    /// - `value`: The raw cookie string.
    ///
    /// ### Returns
    /// - `Theme`: `Light` for `"light"`, `Dark` for `"dark"` or anything else.
    fn from_cookie_value(value: &str) -> Self {
        match value {
            "light" => Theme::Light,
            "dark" => Theme::Dark,
            _ => Theme::Dark,
        }
    }

    /// Return the opposite theme.
    ///
    /// ### Returns
    /// - `Theme`: `Dark` when called on `Light`, and vice versa.
    fn toggled(self) -> Self {
        match self {
            Theme::Light => Theme::Dark,
            Theme::Dark => Theme::Light,
        }
    }
}

impl fmt::Display for Theme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl<S> FromRequestParts<S> for Theme
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    /// Resolve the theme for this request from the `zeptodo_theme` cookie.
    ///
    /// ### Arguments
    /// - `parts`: Request parts injected by Axum.
    /// - `_state`: Unused. The cookie is the only source.
    ///
    /// ### Returns
    /// - `Ok(Theme)`: The parsed theme, defaulting to `Dark` if the cookie is
    ///   missing or unrecognized. This extractor is infallible.
    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let jar = CookieJar::from_headers(&parts.headers);
        let theme = jar
            .get(THEME_COOKIE)
            .map(|c| Theme::from_cookie_value(c.value()))
            .unwrap_or(Theme::Dark);
        Ok(theme)
    }
}

/// Decoded body of the `POST /theme/toggle` form (CSRF token only).
#[derive(Debug, Deserialize)]
pub struct ToggleForm {
    #[serde(rename = "_csrf")]
    pub csrf: String,
}

/// Build a theme cookie ready to attach to a response.
///
/// ### Arguments
/// - `value`: The theme value to encode in the cookie.
/// - `secure`: Whether to set the `Secure` attribute.
///
/// ### Returns
/// - `Cookie<'static>`: A cookie with `Path=/`, `SameSite=Strict`, a one-year
///   max-age, and `HttpOnly` deliberately disabled so the switcher can read it
///   for instant feedback.
fn build_cookie(value: Theme, secure: bool) -> Cookie<'static> {
    let mut cookie = Cookie::new(THEME_COOKIE, value.as_str().to_owned());
    cookie.set_path("/");
    cookie.set_http_only(false);
    cookie.set_secure(secure);
    cookie.set_same_site(SameSite::Strict);
    cookie.set_max_age(time::Duration::seconds(ONE_YEAR_SECONDS));
    cookie
}

/// Flip the theme cookie and respond appropriately for HTMX or plain clients.
///
/// ### Arguments
/// - `state`: Shared application state, used to decide whether the response
///   cookie should be `Secure`.
/// - `headers`: Request headers, used to detect HTMX and to read `Referer`.
/// - `session`: The tower-sessions session, used to validate the CSRF token.
/// - `jar`: Incoming cookies, used to read the current theme and to attach the
///   updated cookie to the response.
/// - `form`: The decoded form body (CSRF token only).
///
/// ### Returns
/// - `Response`: `204 No Content` with the updated cookie for HTMX clients, a
///   302 redirect to `Referer` (or `/`) for plain clients, or 403 on CSRF
///   mismatch.
pub async fn toggle(
    axum::extract::State(state): axum::extract::State<AppState>,
    headers: HeaderMap,
    session: Session,
    jar: CookieJar,
    Form(form): Form<ToggleForm>,
) -> Response {
    if !csrf::verify(&session, &form.csrf).await.unwrap_or(false) {
        return (StatusCode::FORBIDDEN, "invalid CSRF token").into_response();
    }

    let current = jar
        .get(THEME_COOKIE)
        .map(|c| Theme::from_cookie_value(c.value()))
        .unwrap_or(Theme::Dark);
    let next = current.toggled();
    let jar = jar.add(build_cookie(next, state.config.cookies_secure()));

    if headers.get("HX-Request").is_some() {
        return (jar, StatusCode::NO_CONTENT).into_response();
    }

    let target = headers
        .get(REFERER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("/")
        .to_owned();
    (jar, Redirect::to(&target)).into_response()
}
