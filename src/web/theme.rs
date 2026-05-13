use axum::extract::FromRequestParts;
use axum::http::header::REFERER;
use axum::http::{HeaderMap, StatusCode, request::Parts};
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::{Cookie, SameSite};
use std::fmt;

use crate::AppState;

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

    fn from_cookie_value(value: &str) -> Self {
        match value {
            "light" => Theme::Light,
            "dark" => Theme::Dark,
            _ => Theme::Dark,
        }
    }

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

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let jar = CookieJar::from_headers(&parts.headers);
        let theme = jar
            .get(THEME_COOKIE)
            .map(|c| Theme::from_cookie_value(c.value()))
            .unwrap_or(Theme::Dark);
        Ok(theme)
    }
}

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
/// ### Description
/// HTMX callers receive `204 No Content` so the swap is a no-op (the Alpine
/// handler updated the DOM optimistically). Plain browsers without HTMX get
/// a redirect back to the page they came from.
pub async fn toggle(
    axum::extract::State(state): axum::extract::State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Response {
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
