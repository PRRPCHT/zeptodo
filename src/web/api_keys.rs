use askama::Template;
use axum::Form;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use tower_sessions::Session;

use crate::AppState;
use crate::auth::csrf;
use crate::auth::guard::AuthedUser;
use crate::domain::api_key::{self, ApiKey, ExpiryChoice};
use crate::web::layout::{self, LayoutContext};
use crate::web::theme::Theme;

/// Static description of an expiry option, used by both creation and edit menus.
#[derive(Debug, Clone)]
pub struct ExpiryOption {
    pub value: &'static str,
    pub label: &'static str,
}

/// Return the ordered list of expiry choices shown in the dropdown.
///
/// ### Returns
/// - `Vec<ExpiryOption>`: Five options from 30 days through "never".
fn expiry_options() -> Vec<ExpiryOption> {
    vec![
        ExpiryOption {
            value: "30",
            label: "30 days",
        },
        ExpiryOption {
            value: "90",
            label: "90 days",
        },
        ExpiryOption {
            value: "180",
            label: "180 days",
        },
        ExpiryOption {
            value: "365",
            label: "1 year",
        },
        ExpiryOption {
            value: "never",
            label: "Never",
        },
    ]
}

/// Template-ready projection of an [`ApiKey`].
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ApiKeyView {
    pub id: i64,
    pub key_prefix: String,
    pub description_text: String,
    pub expires_at_display: String,
    pub last_used_at_display: String,
    pub is_expired: bool,
    pub current_expiry_value: &'static str,
}

/// Format an optional UTC datetime as `YYYY-MM-DD HH:MM UTC`.
///
/// ### Arguments
/// - `value`: The datetime to format, if any.
/// - `fallback`: The string to return when `value` is `None`.
///
/// ### Returns
/// - `String`: A formatted timestamp or the fallback.
fn format_datetime(value: Option<DateTime<Utc>>, fallback: &str) -> String {
    match value {
        Some(dt) => dt.format("%Y-%m-%d %H:%M UTC").to_string(),
        None => fallback.to_owned(),
    }
}

/// Build an [`ApiKeyView`] for rendering.
///
/// ### Arguments
/// - `record`: The persisted API key row.
/// - `now`: Reference instant used to determine whether the row has expired.
///
/// ### Returns
/// - `ApiKeyView`: A view object with pre-formatted display strings.
fn build_view(record: &ApiKey, now: DateTime<Utc>) -> ApiKeyView {
    let is_expired = record.expires_at.is_some_and(|when| when <= now);
    let current_expiry_value: &'static str = match record.expires_at {
        None => "never",
        Some(_) => "365",
    };
    ApiKeyView {
        id: record.id,
        key_prefix: record.key_prefix.clone(),
        description_text: record.description.clone().unwrap_or_default(),
        expires_at_display: format_datetime(record.expires_at, "Never"),
        last_used_at_display: format_datetime(record.last_used_at, "Never"),
        is_expired,
        current_expiry_value,
    }
}

#[derive(Template)]
#[template(path = "api_keys.html")]
struct ApiKeysPage {
    layout: LayoutContext,
    csrf_token: String,
    keys: Vec<ApiKeyView>,
    expiry_options: Vec<ExpiryOption>,
    created_plaintext: Option<String>,
    created_prefix: Option<String>,
    error: Option<&'static str>,
}

/// Render the API keys management page.
///
/// ### Arguments
/// - `state`: Shared application state.
/// - `theme`: Resolved theme for this request.
/// - `session`: tower-sessions session, used for CSRF and layout.
/// - `user`: Authenticated user supplied by the [`AuthedUser`] extractor.
///
/// ### Returns
/// - `Response`: A 200 HTML page on success, or 500 on backend errors.
pub async fn index(
    State(state): State<AppState>,
    theme: Theme,
    session: Session,
    _user: AuthedUser,
) -> Response {
    render_page(&state, theme, &session, None, None, None).await
}

/// Decoded body of the `POST /api-keys` create form.
#[derive(Debug, Deserialize)]
pub struct CreateForm {
    #[serde(rename = "_csrf")]
    pub csrf: String,
    pub description: Option<String>,
    pub expiry: String,
}

/// Create a new API key and re-render the page with the plaintext visible once.
///
/// ### Arguments
/// - `state`: Shared application state.
/// - `theme`: Resolved theme for this request.
/// - `session`: tower-sessions session, used to validate the CSRF token.
/// - `user`: Authenticated user supplied by the [`AuthedUser`] extractor.
/// - `form`: Decoded form body.
///
/// ### Returns
/// - `Response`: A 200 HTML page including the freshly minted key. 400 on
///   invalid expiry choice, 403 on CSRF mismatch, 500 on backend errors.
pub async fn create(
    State(state): State<AppState>,
    theme: Theme,
    session: Session,
    _user: AuthedUser,
    Form(form): Form<CreateForm>,
) -> Response {
    if !csrf_ok(&session, &form.csrf).await {
        return forbidden_csrf();
    }
    let expiry = match ExpiryChoice::parse(&form.expiry) {
        Ok(c) => c,
        Err(_) => return bad_request("invalid expiry choice"),
    };
    let description = sanitize_optional(form.description.as_deref());

    let created = match api_key::create(&state.pool, description, expiry).await {
        Ok(c) => c,
        Err(err) => return internal_error("creating api key failed", err),
    };
    tracing::info!(
        target: "audit",
        key_id = created.record.id,
        prefix = %created.record.key_prefix,
        "api key created"
    );
    render_page(
        &state,
        theme,
        &session,
        Some(created.plaintext),
        Some(created.record.key_prefix.clone()),
        None,
    )
    .await
}

/// Decoded body of the `POST /api-keys/{id}/edit_expiry` form.
#[derive(Debug, Deserialize)]
pub struct EditExpiryForm {
    #[serde(rename = "_csrf")]
    pub csrf: String,
    pub expiry: String,
}

/// Update the expiration window of an existing key.
///
/// ### Arguments
/// - `state`: Shared application state.
/// - `session`: tower-sessions session, used to validate the CSRF token.
/// - `headers`: Request headers, used to detect HTMX callers.
/// - `user`: Authenticated user supplied by the [`AuthedUser`] extractor.
/// - `id`: Primary key of the row to update.
/// - `form`: Decoded form body.
///
/// ### Returns
/// - `Response`: A 302 to `/api-keys`. 400 on invalid expiry, 403 on CSRF
///   mismatch, 404 when no key matches `id`, 500 on backend errors.
pub async fn edit_expiry(
    State(state): State<AppState>,
    session: Session,
    headers: HeaderMap,
    _user: AuthedUser,
    Path(id): Path<i64>,
    Form(form): Form<EditExpiryForm>,
) -> Response {
    if !csrf_ok(&session, &form.csrf).await {
        return forbidden_csrf();
    }
    let expiry = match ExpiryChoice::parse(&form.expiry) {
        Ok(c) => c,
        Err(_) => return bad_request("invalid expiry choice"),
    };
    match api_key::update_expiry(&state.pool, id, expiry).await {
        Ok(Some(_)) => {
            tracing::info!(target: "audit", key_id = id, "api key expiry updated");
            after_mutation(&headers)
        }
        Ok(None) => not_found(),
        Err(err) => internal_error("updating api key expiry failed", err),
    }
}

/// Decoded body of the `POST /api-keys/{id}/edit_description` form.
#[derive(Debug, Deserialize)]
pub struct EditDescriptionForm {
    #[serde(rename = "_csrf")]
    pub csrf: String,
    pub description: Option<String>,
}

/// Update the human-readable description of an existing key.
///
/// ### Arguments
/// - `state`: Shared application state.
/// - `session`: tower-sessions session, used to validate the CSRF token.
/// - `headers`: Request headers, used to detect HTMX callers.
/// - `user`: Authenticated user supplied by the [`AuthedUser`] extractor.
/// - `id`: Primary key of the row to update.
/// - `form`: Decoded form body.
///
/// ### Returns
/// - `Response`: A 302 to `/api-keys`. 403 on CSRF mismatch, 404 when no key
///   matches `id`, 500 on backend errors.
pub async fn edit_description(
    State(state): State<AppState>,
    session: Session,
    headers: HeaderMap,
    _user: AuthedUser,
    Path(id): Path<i64>,
    Form(form): Form<EditDescriptionForm>,
) -> Response {
    if !csrf_ok(&session, &form.csrf).await {
        return forbidden_csrf();
    }
    let description = sanitize_optional(form.description.as_deref());
    match api_key::update_description(&state.pool, id, description).await {
        Ok(Some(_)) => {
            tracing::info!(target: "audit", key_id = id, "api key description updated");
            after_mutation(&headers)
        }
        Ok(None) => not_found(),
        Err(err) => internal_error("updating api key description failed", err),
    }
}

/// Decoded body of the `POST /api-keys/{id}/delete` form (CSRF token only).
#[derive(Debug, Deserialize)]
pub struct DeleteForm {
    #[serde(rename = "_csrf")]
    pub csrf: String,
}

/// Permanently delete an API key.
///
/// ### Arguments
/// - `state`: Shared application state.
/// - `session`: tower-sessions session, used to validate the CSRF token.
/// - `headers`: Request headers, used to detect HTMX callers.
/// - `user`: Authenticated user supplied by the [`AuthedUser`] extractor.
/// - `id`: Primary key of the row to delete.
/// - `form`: Decoded form body (CSRF token only).
///
/// ### Returns
/// - `Response`: A 302 to `/api-keys`. 403 on CSRF mismatch, 500 on backend errors.
pub async fn delete(
    State(state): State<AppState>,
    session: Session,
    headers: HeaderMap,
    _user: AuthedUser,
    Path(id): Path<i64>,
    Form(form): Form<DeleteForm>,
) -> Response {
    if !csrf_ok(&session, &form.csrf).await {
        return forbidden_csrf();
    }
    match api_key::delete(&state.pool, id).await {
        Ok(true) => {
            tracing::info!(target: "audit", key_id = id, "api key deleted");
            after_mutation(&headers)
        }
        Ok(false) => after_mutation(&headers),
        Err(err) => internal_error("deleting api key failed", err),
    }
}

/// Render the API keys page, optionally with a freshly minted plaintext to show once.
///
/// ### Arguments
/// - `state`: Shared application state.
/// - `theme`: Resolved theme.
/// - `session`: tower-sessions session.
/// - `created_plaintext`: Plaintext of a just-created key, shown exactly once.
/// - `created_prefix`: Prefix of the just-created key, paired with the plaintext.
/// - `error`: Optional error banner.
///
/// ### Returns
/// - `Response`: A 200 HTML page, or 500 on backend errors.
async fn render_page(
    state: &AppState,
    theme: Theme,
    session: &Session,
    created_plaintext: Option<String>,
    created_prefix: Option<String>,
    error: Option<&'static str>,
) -> Response {
    let layout = match layout::build(theme, session).await {
        Ok(l) => l,
        Err(err) => return internal_error("layout build failed", err),
    };
    let csrf_token = layout.csrf_token.clone();
    let now = Utc::now();
    let keys = match api_key::list(&state.pool).await {
        Ok(rows) => rows.iter().map(|r| build_view(r, now)).collect(),
        Err(err) => return internal_error("listing api keys failed", err),
    };
    render(ApiKeysPage {
        layout,
        csrf_token,
        keys,
        expiry_options: expiry_options(),
        created_plaintext,
        created_prefix,
        error,
    })
}

/// Build the post-mutation response: redirect for full-page callers, no-op
/// 204 for HTMX callers (the page reloads itself via `hx-on::after-request`).
///
/// ### Arguments
/// - `headers`: Request headers, used to detect HTMX callers.
///
/// ### Returns
/// - `Response`: A 302 to `/api-keys` for normal browser submissions, or 204
///   when the caller is HTMX.
fn after_mutation(headers: &HeaderMap) -> Response {
    if is_htmx(headers) {
        (StatusCode::NO_CONTENT, [("HX-Redirect", "/api-keys")]).into_response()
    } else {
        Redirect::to("/api-keys").into_response()
    }
}

/// Detect whether the current request was issued by HTMX.
///
/// ### Arguments
/// - `headers`: Request headers.
///
/// ### Returns
/// - `bool`: `true` when the `HX-Request` header is present.
fn is_htmx(headers: &HeaderMap) -> bool {
    headers.get("HX-Request").is_some()
}

/// Verify a submitted CSRF token, returning `false` on any error path.
///
/// ### Arguments
/// - `session`: tower-sessions session holding the canonical token.
/// - `submitted`: The value submitted in the form's `_csrf` field.
///
/// ### Returns
/// - `bool`: `true` when the submitted token matches.
async fn csrf_ok(session: &Session, submitted: &str) -> bool {
    csrf::verify(session, submitted).await.unwrap_or(false)
}

/// Normalise an optional form field, returning `None` for whitespace-only input.
///
/// ### Arguments
/// - `input`: The raw value from the form, if any.
///
/// ### Returns
/// - `Some(String)`: The input is non-empty after trimming.
/// - `None`: The input was missing, empty, or whitespace-only.
fn sanitize_optional(input: Option<&str>) -> Option<String> {
    input.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

/// Render an Askama template into an HTML response.
///
/// ### Arguments
/// - `template`: Any value implementing [`Template`].
///
/// ### Returns
/// - `Response`: A 200 HTML response on success, or 500 with a generic message
///   when the template renderer returned an error.
fn render<T: Template>(template: T) -> Response {
    match template.render() {
        Ok(body) => Html(body).into_response(),
        Err(err) => {
            tracing::error!(error = %err, "template render failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "template error").into_response()
        }
    }
}

/// Build the 403 response used when the CSRF token fails to verify.
///
/// ### Returns
/// - `Response`: A 403 with a generic plain-text body.
fn forbidden_csrf() -> Response {
    (StatusCode::FORBIDDEN, "invalid CSRF token").into_response()
}

/// Build a 400 response with a static message body.
///
/// ### Arguments
/// - `message`: The body to send back to the client.
///
/// ### Returns
/// - `Response`: A 400 with the supplied plain-text body.
fn bad_request(message: &'static str) -> Response {
    (StatusCode::BAD_REQUEST, message).into_response()
}

/// Build the 404 response used when an id does not exist.
///
/// ### Returns
/// - `Response`: A 404 with a generic plain-text body.
fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "api key not found").into_response()
}

/// Log a backend error and build the 500 response.
///
/// ### Arguments
/// - `context`: Static description of the failed step, used in the log line.
/// - `err`: The error that caused the failure.
///
/// ### Returns
/// - `Response`: A 500 with a generic plain-text body. The detailed error is
///   logged, never sent to the client.
fn internal_error(context: &'static str, err: anyhow::Error) -> Response {
    tracing::error!(error = %err, "{context}");
    (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
}
