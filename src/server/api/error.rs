//! API error type and wire shape.
//!
//! Every `/api/v1/*` handler returns `Result<Json<T>, ApiError>`. The
//! ApiError → `IntoResponse` impl is the one place that maps provider
//! failures onto HTTP status codes + the structured JSON body the plan
//! prescribes:
//!
//! ```json
//! { "error": { "code": "not_found", "message": "block 12345 not found" } }
//! ```
//!
//! Classification:
//!
//! * `NotFound` (404) — the route resolved but the resource doesn't
//!   exist (unknown block number, unknown address, unknown ATS id).
//! * `BadRequest` (400) — caller-supplied input is malformed or
//!   out-of-range (e.g. `count=0` on a listing endpoint).
//! * `Internal` (500) — provider-side failure (RPC transport, decode,
//!   unexpected payload shape, sqlx). The client only needs "try
//!   again later"; richer classification stays server-side via
//!   tracing.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

use crate::data::DataError;

#[derive(Debug, Clone)]
pub enum ApiError {
    NotFound(String),
    BadRequest(String),
    Internal(String),
}

impl ApiError {
    fn status(&self) -> StatusCode {
        match self {
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn code(&self) -> &'static str {
        match self {
            ApiError::NotFound(_) => "not_found",
            ApiError::BadRequest(_) => "bad_request",
            ApiError::Internal(_) => "internal",
        }
    }

    fn message(&self) -> &str {
        match self {
            ApiError::NotFound(m) | ApiError::BadRequest(m) | ApiError::Internal(m) => m,
        }
    }
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: ErrorDetail<'a>,
}

#[derive(Serialize)]
struct ErrorDetail<'a> {
    code: &'a str,
    message: &'a str,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // Log internals at warn — 4xx stays at debug so well-behaved
        // clients hitting a `not_found` don't flood production logs.
        match &self {
            ApiError::Internal(msg) => {
                tracing::warn!(kind = "internal", message = %msg, "api error")
            }
            other => tracing::debug!(kind = other.code(), message = %other.message(), "api error"),
        }

        let body = ErrorBody {
            error: ErrorDetail {
                code: self.code(),
                message: self.message(),
            },
        };
        (self.status(), Json(body)).into_response()
    }
}

impl From<DataError> for ApiError {
    fn from(err: DataError) -> Self {
        match err {
            // An unknown network id surfaces as "network unconfigured"
            // from the provider — it's user-facing, not a server fault.
            DataError::NetworkUnconfigured(name) => {
                ApiError::NotFound(format!("network '{name}' is not configured"))
            }
            DataError::Unimplemented(what) => ApiError::Internal(format!("unimplemented: {what}")),
            DataError::NotSupported { network, what } => {
                ApiError::NotFound(format!("{what} is not available on network '{network}'"))
            }
            // Caller input was rejected before we touched the backend: 400.
            DataError::BadRequest(msg) => ApiError::BadRequest(msg),
            // Everything else is a provider / transport failure — surface
            // as 500 so the client treats it as retryable.
            DataError::Transport(msg)
            | DataError::Rpc(msg)
            | DataError::Decode(msg)
            | DataError::InvalidPayload(msg) => ApiError::Internal(msg),
        }
    }
}
