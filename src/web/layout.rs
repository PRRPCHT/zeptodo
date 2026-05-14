use anyhow::Result;
use tower_sessions::Session;

use crate::auth::csrf;
use crate::auth::guard::SESSION_USERNAME_KEY;
use crate::web::theme::Theme;

/// Render-time context shared by every page through the base layout.
#[derive(Debug, Clone)]
pub struct LayoutContext {
    pub theme: &'static str,
    pub username: Option<String>,
    pub csrf_token: String,
    pub version: &'static str,
    pub repo_url: &'static str,
}

/// Source of truth for the placeholder GitHub repo URL displayed in the footer.
pub const REPO_URL: &str = "https://github.com/PRRPCHT/zeptodo";

/// Build a [`LayoutContext`] for the current request.
///
/// ### Arguments
/// - `theme`: The resolved theme for this request.
/// - `session`: The tower-sessions session.
///
/// ### Returns
/// - `Ok(LayoutContext)`: Context ready to inject into a template.
/// - `Err`: The session store failed to read or write.
pub async fn build(theme: Theme, session: &Session) -> Result<LayoutContext> {
    let username = session
        .get::<String>(SESSION_USERNAME_KEY)
        .await
        .ok()
        .flatten();
    let csrf_token = csrf::token(session).await?;
    Ok(LayoutContext {
        theme: theme.as_str(),
        username,
        csrf_token,
        version: env!("CARGO_PKG_VERSION"),
        repo_url: REPO_URL,
    })
}
