use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum MetadataError {
    /// A required setting is missing (e.g. AcoustID key, MusicBrainz contact).
    #[error("not configured: {0}")]
    NotConfigured(&'static str),
    /// Transport-level failure (DNS, connect, timeout, TLS). URL is stripped.
    #[error("network error: {0}")]
    Network(String),
    #[error("not found")]
    NotFound,
    /// 429/503 persisted after all retries.
    #[error("rate limited (retry after {0:?})")]
    RateLimited(Option<Duration>),
    #[error("unexpected HTTP status {status}: {body}")]
    Status { status: u16, body: String },
    #[error("invalid response: {0}")]
    Parse(String),
    /// The service answered 200 but reported an error in the body.
    #[error("service error: {0}")]
    Service(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

pub type Result<T> = std::result::Result<T, MetadataError>;
