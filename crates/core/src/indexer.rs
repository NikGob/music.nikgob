//! Torrent search abstraction.
//!
//! The rest of the app talks only to [`Indexer`]; concrete backends (Prowlarr,
//! Jackett, ...) live in their own crates and are held as `Arc<dyn Indexer>`.

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};

/// Boxed future returned by dyn-compatible async trait methods.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What the user is looking for. More fields (artist, album) can be added later.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchQuery {
    pub text: String,
    pub kind: SearchKind,
    /// Upper bound on returned candidates; `None` leaves it to the indexer.
    pub limit: Option<u32>,
}

impl SearchQuery {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    pub fn lossless(mut self) -> Self {
        self.kind = SearchKind::Lossless;
        self
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchKind {
    /// Any audio release.
    #[default]
    Any,
    /// Lossless releases only.
    Lossless,
}

/// Opaque data an [`Indexer`] implementation needs to fetch a candidate later.
/// Only the implementation that produced it interprets the contents.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateHandle(pub String);

/// Indexer-agnostic search result. Persisted in the Wishlist / confirm queue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    /// Opaque, unique per indexer.
    pub id: String,
    pub title: String,
    /// Tracker name, for display.
    pub indexer_name: String,
    pub size_bytes: Option<u64>,
    pub seeders: Option<u32>,
    pub leechers: Option<u32>,
    /// RFC 3339, as reported by the indexer.
    pub published: Option<String>,
    pub categories: Vec<u32>,
    pub info_url: Option<String>,
    pub info_hash: Option<String>,
    pub handle: CandidateHandle,
}

/// What [`Indexer::fetch_torrent`] hands back to the torrent client.
#[derive(Clone, PartialEq, Eq)]
pub enum TorrentPayload {
    /// Raw contents of a `.torrent` file.
    TorrentFile(Vec<u8>),
    Magnet(String),
}

impl std::fmt::Debug for TorrentPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TorrentFile(bytes) => write!(f, "TorrentFile({} bytes)", bytes.len()),
            Self::Magnet(uri) => f.debug_tuple("Magnet").field(uri).finish(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum IndexerError {
    #[error("indexer is not configured")]
    NotConfigured,
    #[error("indexer rejected the credentials")]
    Unauthorized,
    #[error("indexer is unreachable: {0}")]
    Unreachable(String),
    #[error("indexer answered with HTTP {status}")]
    Http { status: u16 },
    #[error("unexpected indexer response: {0}")]
    Parse(String),
    #[error("candidate has nothing to download")]
    NothingToFetch,
}

pub trait Indexer: Send + Sync {
    /// Short backend name for logs and UI.
    fn name(&self) -> &str;

    fn search(&self, query: &SearchQuery) -> BoxFuture<'_, Result<Vec<Candidate>, IndexerError>>;

    fn fetch_torrent(
        &self,
        candidate: &Candidate,
    ) -> BoxFuture<'_, Result<TorrentPayload, IndexerError>>;

    /// Whether the indexer answers with these settings, without a search:
    /// how many trackers it searches. Indexers that cannot tell say 0.
    fn check(&self) -> BoxFuture<'_, Result<usize, IndexerError>> {
        Box::pin(async { Ok(0) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct Null;

    impl Indexer for Null {
        fn name(&self) -> &str {
            "null"
        }
        fn search(&self, _: &SearchQuery) -> BoxFuture<'_, Result<Vec<Candidate>, IndexerError>> {
            Box::pin(async { Err(IndexerError::NotConfigured) })
        }
        fn fetch_torrent(
            &self,
            _: &Candidate,
        ) -> BoxFuture<'_, Result<TorrentPayload, IndexerError>> {
            Box::pin(async { Err(IndexerError::NothingToFetch) })
        }
    }

    #[test]
    fn trait_is_dyn_compatible() {
        let indexer: Arc<dyn Indexer> = Arc::new(Null);
        assert_eq!(indexer.name(), "null");
    }

    #[test]
    fn candidate_survives_json() {
        let c = Candidate {
            id: "1".into(),
            title: "Album [FLAC]".into(),
            indexer_name: "RuTracker".into(),
            size_bytes: Some(1 << 30),
            seeders: Some(5),
            leechers: None,
            published: Some("2024-01-02T03:04:05Z".into()),
            categories: vec![3000, 3040],
            info_url: None,
            info_hash: Some("abc".into()),
            handle: CandidateHandle("opaque".into()),
        };
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<Candidate>(&json).unwrap(), c);
    }
}
