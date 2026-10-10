//! Errors of the import pipeline.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("I/O error at {}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot read or write tags of {}: {message}", path.display())]
    Tags { path: PathBuf, message: String },
    #[error("cannot decode {}: {message}", path.display())]
    Decode { path: PathBuf, message: String },
    #[error("encoder: {0}")]
    Encode(String),
    #[error("image: {0}")]
    Image(String),
    #[error("unsupported input: {0}")]
    Unsupported(String),
    #[error("library: {0}")]
    Library(#[from] nikgob_library::LibraryError),
    #[error("{0}")]
    Other(String),
}

pub type Result<T, E = ImportError> = std::result::Result<T, E>;

impl ImportError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    pub fn tags(path: impl Into<PathBuf>, message: impl ToString) -> Self {
        Self::Tags {
            path: path.into(),
            message: message.to_string(),
        }
    }

    pub fn decode(path: impl Into<PathBuf>, message: impl ToString) -> Self {
        Self::Decode {
            path: path.into(),
            message: message.to_string(),
        }
    }
}
