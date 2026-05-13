use askama::Template;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};

use crate::web::theme::Theme;

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const REPO_URL: &str = "https://github.com/your-org/zeptodo";

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate {
    theme: &'static str,
    version: &'static str,
    repo_url: &'static str,
}

/// Render the placeholder landing page.
///
/// ### Returns
/// - `Response`: Full HTML page with theme applied.
pub async fn index(theme: Theme) -> Response {
    render(IndexTemplate {
        theme: theme.as_str(),
        version: APP_VERSION,
        repo_url: REPO_URL,
    })
}

/// Liveness probe used by orchestrators.
///
/// ### Returns
/// - `(StatusCode, &'static str)`: Always `200 OK`.
pub async fn healthz() -> (StatusCode, &'static str) {
    (StatusCode::OK, "ok")
}

fn render<T: Template>(template: T) -> Response {
    match template.render() {
        Ok(body) => Html(body).into_response(),
        Err(err) => {
            tracing::error!(error = %err, "template render failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "template error").into_response()
        }
    }
}
