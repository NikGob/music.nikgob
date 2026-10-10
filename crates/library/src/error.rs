use std::io;

use crate::fractional::FractionalError;

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("i/o error: {0}")]
    Io(#[from] io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("fractional index error: {0}")]
    Fractional(#[from] FractionalError),
    #[error("{what} not found: {id}")]
    NotFound { what: &'static str, id: String },
    #[error("invalid value: {0}")]
    Invalid(String),
    #[error("database schema version {found} is newer than this build supports ({supported})")]
    SchemaTooNew { found: u32, supported: u32 },
    #[error("unsupported userdata format version {0}")]
    UnsupportedFormat(u32),
}

pub type Result<T> = std::result::Result<T, LibraryError>;

impl LibraryError {
    pub(crate) fn not_found(what: &'static str, id: impl ToString) -> Self {
        Self::NotFound {
            what,
            id: id.to_string(),
        }
    }
}
