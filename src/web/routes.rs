use askama::Template;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use tower_sessions::Session;

use crate::auth::guard::AuthedUser;
use crate::web::layout::{self, LayoutContext};
use crate::web::theme::Theme;

#[derive(Template)]
#[template(path = "todos.html")]
struct TodosPage {
    layout: LayoutContext,
}

/// Liveness probe used by orchestrators.
///
/// ### Returns
/// - `(StatusCode, &'static str)`: Always `200 OK`.
pub async fn healthz() -> (StatusCode, &'static str) {
    (StatusCode::OK, "ok")
}

/// Render the master to-do list placeholder at the application root.
///
/// ### Arguments
/// - `theme`: The resolved theme for this request.
/// - `session`: The tower-sessions session.
/// - `user`: The authenticated user, supplied by the extractor.
///
/// ### Returns
/// - `Response`: A 200 page with the placeholder list, or 500 if layout or
///   template rendering fails.
pub async fn todos(theme: Theme, session: Session, user: AuthedUser) -> Response {
    tracing::debug!(username = %user.0, "rendering todos");
    let layout = match layout::build(theme, &session).await {
        Ok(l) => l,
        Err(err) => {
            tracing::error!(error = %err, "layout build failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
    };
    let page = TodosPage { layout };
    match page.render() {
        Ok(html) => Html(html).into_response(),
        Err(err) => {
            tracing::error!(error = %err, "template render failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "template error").into_response()
        }
    }
}
