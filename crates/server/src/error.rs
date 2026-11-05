use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use rusticdoc_core::Error;
use serde::Serialize;
use utoipa::ToSchema;

/// The JSON body of every error response.
#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorBody {
    /// A stable, machine-readable kind.
    #[schema(example = "invalid_request")]
    pub error: &'static str,
    /// What went wrong, for a person.
    #[schema(example = "unknown step 'teleport'")]
    pub message: String,
}

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub body: ErrorBody,
}

impl ApiError {
    pub fn new(status: StatusCode, error: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            body: ErrorBody {
                error,
                message: message.into(),
            },
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }
}

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        let (status, kind) = match &e {
            Error::InvalidRequest(_) => (StatusCode::BAD_REQUEST, "invalid_request"),
            Error::Template(_) => (StatusCode::UNPROCESSABLE_ENTITY, "template_error"),
            Error::Unsupported(_) => (StatusCode::NOT_IMPLEMENTED, "not_supported"),
            Error::EngineUnavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, "engine_unavailable"),
            Error::Engine { .. } => (StatusCode::INTERNAL_SERVER_ERROR, "engine_failed"),
            Error::Timeout(_) => (StatusCode::SERVICE_UNAVAILABLE, "timeout"),
            Error::Io(_) => (StatusCode::INTERNAL_SERVER_ERROR, "io_error"),
        };
        Self::new(status, kind, e.to_string())
    }
}

impl From<axum::extract::multipart::MultipartError> for ApiError {
    fn from(e: axum::extract::multipart::MultipartError) -> Self {
        Self::bad_request(format!("multipart form: {}", e.body_text()))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if self.status.is_server_error() {
            tracing::error!(error = self.body.error, message = %self.body.message, "request failed");
        } else {
            tracing::warn!(error = self.body.error, message = %self.body.message, "request rejected");
        }
        (self.status, Json(self.body)).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

/// A file in a response, for the API description: generated clients return
/// it as bytes.
#[derive(Debug, Serialize, ToSchema)]
#[schema(value_type = String, format = Binary)]
pub struct BinaryFile(pub String);
