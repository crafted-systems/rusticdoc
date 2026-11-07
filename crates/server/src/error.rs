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
            Error::Status(code, _) => (
                StatusCode::from_u16(*code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                "reported",
            ),
        };
        Self::new(status, kind, e.to_string())
    }
}

impl From<rusticdoc_core::store::StoreError> for ApiError {
    /// Details of a store's failure name its bucket and endpoint, so they go to
    /// the log and the client gets the kind.
    fn from(e: rusticdoc_core::store::StoreError) -> Self {
        use rusticdoc_core::store::StoreError;
        match e {
            StoreError::NotFound => Self::new(StatusCode::NOT_FOUND, "not_found", "no such object"),
            StoreError::Backend(detail) => {
                tracing::error!(error = %detail, "store failed");
                Self::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "store_unavailable",
                    "the store cannot be reached; try again",
                )
            }
            StoreError::Integrity(detail) => {
                tracing::error!(error = %detail, "store returned something other than was written");
                Self::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "store_integrity",
                    "the stored object failed its integrity check",
                )
            }
        }
    }
}

impl From<axum::extract::multipart::MultipartError> for ApiError {
    fn from(e: axum::extract::multipart::MultipartError) -> Self {
        Self::bad_request(format!("multipart form: {}", e.body_text()))
    }
}

/// The text a client reads for an error. Engine failures stay out of it, with
/// their stderr and temp paths: they go to the log, and the client gets the
/// status's name.
pub fn public_text(status: StatusCode, kind: &str, message: &str) -> String {
    match kind {
        "timeout" => {
            "The request exceeded the time limit. Increase it with --api-timeout, or reduce the workload.".to_string()
        }
        "reported" => message.to_string(),
        _ if status.is_server_error() || status == StatusCode::UNAUTHORIZED => status
            .canonical_reason()
            .unwrap_or("Error")
            .to_string(),
        _ => message.to_string(),
    }
}

impl ApiError {
    /// Logs the whole error, inside the request's span.
    pub fn log(&self) {
        if self.status.is_server_error() {
            tracing::error!(error = self.body.error, message = %self.body.message, "request failed");
        } else {
            tracing::warn!(error = self.body.error, message = %self.body.message, "request rejected");
        }
    }

    /// The same error with only what a client may read.
    pub fn public(self) -> Self {
        let message = public_text(self.status, self.body.error, &self.body.message);
        Self::new(self.status, self.body.error, message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        self.log();
        let this = self.public();
        let text = ErrorText {
            kind: this.body.error,
            message: this.body.message.clone(),
        };
        let mut res = (this.status, Json(this.body)).into_response();
        res.extensions_mut().insert(text);
        res
    }
}

/// An error's kind and text, on its response, for the routes that do not answer
/// in JSON.
#[derive(Debug, Clone)]
pub struct ErrorText {
    pub kind: &'static str,
    pub message: String,
}

pub type ApiResult<T> = Result<T, ApiError>;

/// A file in a response, for the API description: generated clients return
/// it as bytes.
#[derive(Debug, Serialize, ToSchema)]
#[schema(value_type = String, format = Binary)]
pub struct BinaryFile(pub String);
