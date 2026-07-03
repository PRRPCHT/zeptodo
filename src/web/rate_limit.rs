use std::hash::Hash;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::HeaderValue;
use axum::http::Method;
use axum::http::Request;
use axum::http::header::AUTHORIZATION;
use axum::response::IntoResponse;
use governor::clock::QuantaInstant;
use governor::middleware::{NoOpMiddleware, RateLimitingMiddleware};
use sha2::{Digest, Sha256};
use tower_governor::GovernorError;
use tower_governor::GovernorLayer;
use tower_governor::governor::{GovernorConfig, GovernorConfigBuilder};
use tower_governor::key_extractor::KeyExtractor;

use crate::api::errors::ApiError;

/// Login burst budget. 5 attempts within `LOGIN_PERIOD_SECS` per client IP.
const LOGIN_BURST: u32 = 5;
/// Replenish rate for the login limiter, in seconds per token.
const LOGIN_PERIOD_SECS: u64 = 12;

/// API burst budget. 60 requests within `API_PERIOD_MILLIS * 60` per key.
const API_BURST: u32 = 60;
/// Replenish rate for the API limiter, in milliseconds per token.
const API_PERIOD_MILLIS: u64 = 1_000;

/// Global per-IP burst budget. Tolerates a normal page load (HTML + a handful
/// of vendored JS files + the compiled CSS) several times in quick succession.
const GLOBAL_BURST: u32 = 120;
/// Replenish rate for the global limiter, in milliseconds per token (10 req/s sustained).
const GLOBAL_PERIOD_MILLIS: u64 = 100;

/// Interval between passes that prune stale buckets from the limiter key maps.
const RETAIN_INTERVAL: Duration = Duration::from_secs(60);

/// Type-erased callback that prunes one rate limiter's key map of stale buckets.
pub type RetainHandle = Box<dyn Fn() + Send + Sync + 'static>;

/// Build the rate-limit configuration applied to `POST /login`, keyed by the
/// caller's IP as resolved by `extractor`.
///
/// ### Arguments
/// - `extractor`: How the client IP is derived. Use `PeerIpKeyExtractor` to key
///   on the spoof-proof socket address, or `SmartIpKeyExtractor` to trust
///   forwarding headers when a reverse proxy is in front.
///
/// ### Returns
/// - The shareable configuration, wrapped in `Arc` for use with `GovernorLayer`.
pub fn login_config<K>(extractor: K) -> Arc<GovernorConfig<K, NoOpMiddleware>>
where
    K: KeyExtractor,
{
    let cfg = GovernorConfigBuilder::default()
        .key_extractor(extractor)
        .per_second(LOGIN_PERIOD_SECS)
        .burst_size(LOGIN_BURST)
        .methods(vec![Method::POST])
        .finish()
        .expect("login governor config: non-zero burst and period");
    Arc::new(cfg)
}

/// Build the global rate-limit configuration applied to every request, keyed by
/// the caller's IP as resolved by `extractor`.
///
/// ### Arguments
/// - `extractor`: How the client IP is derived. See `login_config`.
///
/// ### Returns
/// - The shareable configuration, wrapped in `Arc` for use with `GovernorLayer`.
pub fn global_config<K>(extractor: K) -> Arc<GovernorConfig<K, NoOpMiddleware>>
where
    K: KeyExtractor,
{
    let cfg = GovernorConfigBuilder::default()
        .key_extractor(extractor)
        .period(Duration::from_millis(GLOBAL_PERIOD_MILLIS))
        .burst_size(GLOBAL_BURST)
        .finish()
        .expect("global governor config: non-zero burst and period");
    Arc::new(cfg)
}

/// Build the `GovernorLayer` for `POST /login`, keyed by `extractor`.
///
/// ### Arguments
/// - `extractor`: How the client IP is derived (see `login_config`).
///
/// ### Returns
/// - `(layer, handle)`: The layer with the HTML error renderer wired in, and a
///   pruning handle for the layer's limiter (feed it to `spawn_retain_task`).
pub fn login_layer<K>(extractor: K) -> (GovernorLayer<K, NoOpMiddleware, Body>, RetainHandle)
where
    K: KeyExtractor,
    K::Key: Hash + Eq + Clone + Send + Sync + 'static,
{
    let config = login_config(extractor);
    let handle = retain_handle(&config);
    (GovernorLayer::new(config).error_handler(html_error), handle)
}

/// Build the global `GovernorLayer` applied to every request, keyed by `extractor`.
///
/// ### Arguments
/// - `extractor`: How the client IP is derived (see `login_config`).
///
/// ### Returns
/// - `(layer, handle)`: The layer with the HTML error renderer wired in, and a
///   pruning handle for the layer's limiter (feed it to `spawn_retain_task`).
pub fn global_layer<K>(extractor: K) -> (GovernorLayer<K, NoOpMiddleware, Body>, RetainHandle)
where
    K: KeyExtractor,
    K::Key: Hash + Eq + Clone + Send + Sync + 'static,
{
    let config = global_config(extractor);
    let handle = retain_handle(&config);
    (GovernorLayer::new(config).error_handler(html_error), handle)
}

/// Build a pruning handle for the limiter backing `config`.
///
/// ### Arguments
/// - `config`: The limiter configuration whose key map should be prunable.
///
/// ### Returns
/// - `RetainHandle`: A callback that prunes the limiter's stale buckets when invoked.
pub fn retain_handle<K, M>(config: &Arc<GovernorConfig<K, M>>) -> RetainHandle
where
    K: KeyExtractor,
    K::Key: Hash + Eq + Clone + Send + Sync + 'static,
    M: RateLimitingMiddleware<QuantaInstant> + Send + Sync + 'static,
{
    let limiter = config.limiter().clone();
    Box::new(move || limiter.retain_recent())
}

/// Spawn a background task that prunes every limiter's key map on a fixed interval.
///
/// ### Arguments
/// - `handles`: One pruning handle per limiter, produced by the layer builders
///   or `retain_handle`.
pub fn spawn_retain_task(handles: Vec<RetainHandle>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(RETAIN_INTERVAL);
        loop {
            interval.tick().await;
            for handle in &handles {
                handle();
            }
        }
    });
}

/// Build the rate-limit configuration applied to `/api/v1/*`, keyed by the
/// SHA-256 hash of the bearer token.
///
/// ### Returns
/// - The shareable configuration, wrapped in `Arc` for use with `GovernorLayer`.
pub fn api_config()
-> Arc<GovernorConfig<BearerHashKeyExtractor, governor::middleware::NoOpMiddleware>> {
    let cfg = GovernorConfigBuilder::default()
        .key_extractor(BearerHashKeyExtractor)
        .period(Duration::from_millis(API_PERIOD_MILLIS))
        .burst_size(API_BURST)
        .finish()
        .expect("api governor config: non-zero burst and period");
    Arc::new(cfg)
}

/// Render a `GovernorError` as an HTML response for the login form path.
///
/// ### Arguments
/// - `error`: The governor error to render.
///
/// ### Returns
/// - The HTTP response. `429 Too Many Requests` with a `Retry-After` header
///   on rate-limit hits; `500` for extractor failures (e.g. missing client IP).
pub fn html_error(error: GovernorError) -> axum::response::Response {
    match error {
        GovernorError::TooManyRequests { wait_time, headers } => {
            let mut response = (
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                "Too many requests. Please try again later.",
            )
                .into_response();
            if let Some(extra) = headers {
                response.headers_mut().extend(extra);
            }
            if let Ok(value) = HeaderValue::from_str(&wait_time.to_string()) {
                response.headers_mut().insert("retry-after", value);
            }
            response
        }
        GovernorError::UnableToExtractKey => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "rate-limit key extraction failed",
        )
            .into_response(),
        GovernorError::Other { code, msg, headers } => {
            let body = msg.unwrap_or_else(|| "rate-limit error".to_owned());
            let mut response = (code, body).into_response();
            if let Some(extra) = headers {
                response.headers_mut().extend(extra);
            }
            response
        }
    }
}

/// Render a `GovernorError` as a JSON envelope for the REST API.
///
/// ### Arguments
/// - `error`: The governor error to render.
///
/// ### Returns
/// - The JSON `{error: {code, message}}` envelope. `429` for quota
///   exhaustion, `500` otherwise.
pub fn api_error(error: GovernorError) -> axum::response::Response {
    match error {
        GovernorError::TooManyRequests { wait_time, headers } => {
            let mut response = ApiError::too_many_requests("rate limit exceeded").into_response();
            if let Some(extra) = headers {
                response.headers_mut().extend(extra);
            }
            if let Ok(value) = HeaderValue::from_str(&wait_time.to_string()) {
                response.headers_mut().insert("retry-after", value);
            }
            response
        }
        GovernorError::UnableToExtractKey => {
            ApiError::internal_message("rate-limit key extraction failed").into_response()
        }
        GovernorError::Other { msg, .. } => {
            ApiError::internal_message(&msg.unwrap_or_else(|| "rate-limit error".to_owned()))
                .into_response()
        }
    }
}

/// `KeyExtractor` that buckets requests by the SHA-256 hash of the bearer
/// token they present, or a shared `"none"` bucket when absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BearerHashKeyExtractor;

impl KeyExtractor for BearerHashKeyExtractor {
    type Key = String;

    fn extract<T>(&self, req: &Request<T>) -> Result<Self::Key, GovernorError> {
        let token = req
            .headers()
            .get(AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());
        match token {
            Some(t) => {
                let mut hasher = Sha256::new();
                hasher.update(t.as_bytes());
                let digest = hasher.finalize();
                Ok(hex_encode(&digest))
            }
            None => Ok("none".to_owned()),
        }
    }
}

/// Encode a byte slice as a lowercase hex string.
///
/// ### Arguments
/// - `bytes`: Input bytes (typically the 32-byte SHA-256 digest).
///
/// ### Returns
/// - A 2-bytes-per-byte lowercase hex string.
fn hex_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(TABLE[(b >> 4) as usize] as char);
        out.push(TABLE[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;

    fn req_with_auth(value: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().uri("/api/v1/tasks");
        if let Some(v) = value {
            builder = builder.header(AUTHORIZATION, v);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[test]
    fn same_token_hashes_to_same_key() {
        let extractor = BearerHashKeyExtractor;
        let a = extractor
            .extract(&req_with_auth(Some("Bearer abc123")))
            .unwrap();
        let b = extractor
            .extract(&req_with_auth(Some("Bearer abc123")))
            .unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn different_tokens_hash_differently() {
        let extractor = BearerHashKeyExtractor;
        let a = extractor
            .extract(&req_with_auth(Some("Bearer abc")))
            .unwrap();
        let b = extractor
            .extract(&req_with_auth(Some("Bearer xyz")))
            .unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn retain_handle_prunes_without_disrupting_the_limiter() {
        let config = api_config();
        let limiter = config.limiter().clone();
        let handle = retain_handle(&config);

        assert!(limiter.check_key(&"token-a".to_owned()).is_ok());
        handle();
        handle();
        assert!(limiter.check_key(&"token-b".to_owned()).is_ok());
    }

    #[test]
    fn missing_or_malformed_authorization_falls_back_to_shared_bucket() {
        let extractor = BearerHashKeyExtractor;
        assert_eq!(extractor.extract(&req_with_auth(None)).unwrap(), "none");
        assert_eq!(
            extractor
                .extract(&req_with_auth(Some("Basic abcdef")))
                .unwrap(),
            "none"
        );
        assert_eq!(
            extractor.extract(&req_with_auth(Some("Bearer "))).unwrap(),
            "none"
        );
    }
}
