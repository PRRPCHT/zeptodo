use axum::http::StatusCode;

/// Liveness probe used by orchestrators.
///
/// ### Returns
/// - `(StatusCode, &'static str)`: Always `200 OK`.
pub async fn healthz() -> (StatusCode, &'static str) {
    (StatusCode::OK, "ok")
}
