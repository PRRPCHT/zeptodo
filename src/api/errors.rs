use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// JSON error envelope returned by every `/api/v1` handler.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

/// Body of the JSON error envelope.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorBody {
    pub code: &'static str,
    pub message: String,
}

/// Error type returned by REST handlers.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    /// Build a 400 `bad_request` error.
    ///
    /// ### Arguments
    /// - `message`: Human-readable explanation included in the JSON body.
    ///
    /// ### Returns
    /// - `ApiError`: Ready to be returned from a handler.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "bad_request",
            message: message.into(),
        }
    }

    /// Build a 401 `unauthorized` error.
    ///
    /// ### Arguments
    /// - `message`: Human-readable explanation included in the JSON body.
    ///
    /// ### Returns
    /// - `ApiError`: Ready to be returned from a handler or middleware.
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: message.into(),
        }
    }

    /// Build a 404 `not_found` error.
    ///
    /// ### Arguments
    /// - `message`: Human-readable explanation included in the JSON body.
    ///
    /// ### Returns
    /// - `ApiError`: Ready to be returned from a handler.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message: message.into(),
        }
    }

    /// Build a 429 `rate_limited` error.
    ///
    /// ### Arguments
    /// - `message`: Human-readable explanation included in the JSON body.
    ///
    /// ### Returns
    /// - `ApiError`: Ready to be returned from a layer or handler.
    pub fn too_many_requests(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: "rate_limited",
            message: message.into(),
        }
    }

    /// Build a 500 `internal_error` without an underlying cause.
    ///
    /// ### Arguments
    /// - `message`: Human-readable explanation included in the JSON body.
    ///
    /// ### Returns
    /// - `ApiError`: A generic 500 response.
    pub fn internal_message(message: &str) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
            message: message.to_owned(),
        }
    }

    /// Build a 500 `internal_error` and log the underlying cause.
    ///
    /// ### Arguments
    /// - `context`: Static description of the failed step, used in the log line.
    /// - `err`: The error that caused the failure. Not surfaced to clients.
    ///
    /// ### Returns
    /// - `ApiError`: A generic 500 response. The detail is logged, never sent.
    pub fn internal(context: &'static str, err: anyhow::Error) -> Self {
        tracing::error!(error = %err, "{context}");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
            message: "internal error".to_owned(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let envelope = ErrorEnvelope {
            error: ErrorBody {
                code: self.code,
                message: self.message,
            },
        };
        (self.status, Json(envelope)).into_response()
    }
}
