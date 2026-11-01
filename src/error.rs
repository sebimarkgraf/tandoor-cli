use serde_json::Value;

pub type Result<T> = std::result::Result<T, Error>;

/// API errors retain their HTTP status and structured validation details.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Config(String),
    #[error("HTTP request failed: {0}")]
    Transport(#[source] reqwest::Error),
    #[error("Tandoor returned HTTP {status}: {body}")]
    Api { status: u16, body: Value },
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid pagination: {0}")]
    Pagination(String),
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        // URLs may contain sensitive query parameters.
        Self::Transport(error.without_url())
    }
}
