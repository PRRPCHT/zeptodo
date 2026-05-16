use std::time::Instant;

use axum::extract::{Request, State};
use axum::http::header::AUTHORIZATION;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::AppState;
use crate::api::errors::ApiError;
use crate::domain::api_key;

/// Identity stamped onto authenticated requests by [`require_api_key`].
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct ApiCaller {
    pub key_id: i64,
}

/// Reject requests missing a valid `Authorization: Bearer <token>` header.
///
/// ### Arguments
/// - `state`: Shared application state used to access the SQLite pool.
/// - `req`: Incoming request. Mutated to attach the [`ApiCaller`] extension.
/// - `next`: Next layer in the stack.
///
/// ### Returns
/// - `Response`: The downstream response on success, or a JSON 401 envelope
///   when the token is missing, malformed, unknown, or expired.
pub async fn require_api_key(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    let start = Instant::now();
    let route = req.uri().path().to_owned();

    let token = match extract_bearer(&req) {
        Ok(t) => t,
        Err(err) => return finish_unauth(err, &route, start),
    };

    let verified = match api_key::verify(&state.pool, &token).await {
        Ok(Some(v)) => v,
        Ok(None) => {
            return finish_unauth(
                ApiError::unauthorized("invalid or expired api key"),
                &route,
                start,
            );
        }
        Err(err) => {
            let api_err = ApiError::internal("verifying api key failed", err);
            let response = api_err.into_response();
            log_request(None, &route, response.status().as_u16(), start);
            return response;
        }
    };

    req.extensions_mut().insert(ApiCaller {
        key_id: verified.id,
    });

    let pool = state.pool.clone();
    let key_id = verified.id;
    tokio::spawn(async move {
        match api_key::mark_used(&pool, key_id).await {
            Ok(true) => {
                tracing::info!(target: "audit", key_id, "api key first use");
            }
            Ok(false) => {}
            Err(err) => {
                tracing::warn!(error = %err, key_id, "marking api key as used failed");
            }
        }
    });

    let response = next.run(req).await;
    log_request(Some(verified.id), &route, response.status().as_u16(), start);
    response
}

/// Parse the `Authorization` header and extract the bearer token.
///
/// ### Arguments
/// - `req`: Incoming request.
///
/// ### Returns
/// - `Ok(String)`: Trimmed bearer token.
/// - `Err(ApiError)`: A 401 envelope when the header is missing or not Bearer.
fn extract_bearer(req: &Request) -> Result<String, ApiError> {
    let header = req
        .headers()
        .get(AUTHORIZATION)
        .ok_or_else(|| ApiError::unauthorized("missing Authorization header"))?;
    let value = header
        .to_str()
        .map_err(|_| ApiError::unauthorized("malformed Authorization header"))?;
    let token = value
        .strip_prefix("Bearer ")
        .ok_or_else(|| ApiError::unauthorized("expected Bearer scheme"))?
        .trim();
    if token.is_empty() {
        return Err(ApiError::unauthorized("empty bearer token"));
    }
    Ok(token.to_owned())
}

/// Render an [`ApiError`] response and emit the request log line.
///
/// ### Arguments
/// - `err`: The error to render.
/// - `route`: Request path captured before consuming the request.
/// - `start`: Instant captured at the start of the request.
///
/// ### Returns
/// - `Response`: The JSON error envelope built from `err`.
fn finish_unauth(err: ApiError, route: &str, start: Instant) -> Response {
    let response = err.into_response();
    log_request(None, route, response.status().as_u16(), start);
    response
}

/// Emit the structured request log line.
///
/// ### Arguments
/// - `key_id`: Matched key id, when authentication succeeded.
/// - `route`: Request path.
/// - `status`: HTTP status code of the response.
/// - `start`: Instant captured at the start of the request.
fn log_request(key_id: Option<i64>, route: &str, status: u16, start: Instant) {
    let duration_ms = start.elapsed().as_millis() as u64;
    match key_id {
        Some(id) => tracing::info!(
            target: "api",
            key_id = id,
            route = %route,
            status = status,
            duration_ms = duration_ms,
            "api request"
        ),
        None => tracing::info!(
            target: "api",
            route = %route,
            status = status,
            duration_ms = duration_ms,
            "api request"
        ),
    }
}
