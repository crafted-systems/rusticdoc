use std::time::Duration;

/// Every failure the pipeline can report. The server maps each variant to a
/// status code; the CLI prints it.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The request itself is wrong: a bad step name, missing data, an
    /// unparseable option.
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    /// The template could not be parsed or rendered. The message names the
    /// tag and, where known, where it sits.
    #[error("template error: {0}")]
    Template(String),

    /// The format or operation exists in the design but is not implemented
    /// in this version.
    #[error("not supported yet: {0}")]
    Unsupported(String),

    /// An engine is not configured, for example no LibreOffice binary.
    #[error("engine unavailable: {0}")]
    EngineUnavailable(String),

    /// An engine ran and failed.
    #[error("{engine} failed: {message}")]
    Engine {
        engine: &'static str,
        message: String,
    },

    /// The job ran past its time limit and was killed.
    #[error("timed out after {0:?}")]
    Timeout(Duration),

    /// A failure with the exact status and text the client should see.
    #[error("{1}")]
    Status(u16, String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// True when the client, not the service, has to change something.
    pub fn is_client_error(&self) -> bool {
        matches!(
            self,
            Error::InvalidRequest(_) | Error::Template(_) | Error::Unsupported(_)
        )
    }
}
