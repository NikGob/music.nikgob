//! Errors of the import pipeline.

use std::path::PathBuf;

use nikgob_core::i18n::{ErrorStrings, Locale};
use nikgob_core::model::TrackId;

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
    /// Named masters sit inside album folders under `dir`: relocation refuses to split them.
    #[error("{}", ErrorStrings::masters_named_in_albums_in(Locale::Ru, &dir.display().to_string(), *count))]
    NamedMastersInAlbumFolders { dir: PathBuf, count: usize },
    /// Size or BLAKE3 digest did not match the expected value.
    #[error("{}", ErrorStrings::hash_verification_failed_in(Locale::Ru, &path.display().to_string(), expected, *expected_size, actual, *actual_size))]
    HashVerificationFailed {
        path: PathBuf,
        expected: String,
        expected_size: Option<u64>,
        actual: String,
        actual_size: Option<u64>,
    },
    /// Sidecar file size did not match written bytes after relocation.
    #[error("{}", ErrorStrings::sidecar_verification_failed_in(Locale::Ru, &path.display().to_string()))]
    SidecarVerificationFailed { path: PathBuf },
    /// The file holds another song's audio: it is not
    /// imported as `title`; `sounds_like` is the song it holds, when known.
    #[error("в файле «{}» другая песня{}", .title, wrong_tail(.sounds_like))]
    WrongAudio {
        track: Option<TrackId>,
        title: String,
        sounds_like: Option<String>,
    },
    #[error("{0}")]
    Other(String),
}

fn wrong_tail(sounds_like: &Option<String>) -> String {
    sounds_like.as_ref().map(|s| format!(" — «{s}»")).unwrap_or_default()
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

    /// Formats the error message in `loc`, preserving paths, hashes, and counts.
    pub fn message_in(&self, loc: Locale) -> String {
        match self {
            Self::NamedMastersInAlbumFolders { dir, count } => {
                ErrorStrings::masters_named_in_albums_in(loc, &dir.display().to_string(), *count)
            }
            Self::HashVerificationFailed {
                path,
                expected,
                expected_size,
                actual,
                actual_size,
            } => ErrorStrings::hash_verification_failed_in(
                loc,
                &path.display().to_string(),
                expected,
                *expected_size,
                actual,
                *actual_size,
            ),
            Self::SidecarVerificationFailed { path } => {
                ErrorStrings::sidecar_verification_failed_in(loc, &path.display().to_string())
            }
            _ => self.to_string(),
        }
    }
}
