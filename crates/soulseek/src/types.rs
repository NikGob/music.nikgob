//! What the rest of the app sees: queries, results, download states and
//! share settings. All of it is plain data with serde, so the embedded
//! client and the headless service's HTTP API hand out the same values.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::msg::FileEntry;
use crate::share::LOSSLESS_EXTENSIONS;

/// What a query looks for. Soulseek itself has one kind of search; an album
/// query is answered with the same file results, which [`group_albums`]
/// gathers per sharer and folder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QueryKind {
    #[default]
    Track,
    Album,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchQuery {
    pub text: String,
    #[serde(default)]
    pub kind: QueryKind,
}

impl SearchQuery {
    pub fn track(text: impl Into<String>) -> Self {
        Self { text: text.into(), kind: QueryKind::Track }
    }

    pub fn album(text: impl Into<String>) -> Self {
        Self { text: text.into(), kind: QueryKind::Album }
    }
}

/// One file someone shares.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResult {
    pub user: String,
    /// The sharer's full path, with backslashes (what a download asks for).
    pub path: String,
    pub size: u64,
    /// Upper-case extension: "FLAC", "MP3"…
    pub format: String,
    pub lossless: bool,
    /// kbps (lossy files).
    pub bitrate: Option<u32>,
    /// Seconds.
    pub duration: Option<u32>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
    /// The sharer has an upload slot free right now.
    pub free_slot: bool,
    /// The sharer's average upload speed, bytes per second.
    pub avg_speed: u32,
    /// How many uploads wait in the sharer's queue.
    pub queue_length: u32,
}

impl SearchResult {
    pub(crate) fn from_entry(user: &str, path: String, f: &FileEntry, free_slot: bool, avg_speed: u32, queue_length: u32) -> Self {
        let ext = if f.ext.is_empty() {
            path.rsplit('\\').next().and_then(|n| n.rsplit_once('.')).map_or("", |(_, e)| e).to_owned()
        } else {
            f.ext.clone()
        };
        let lower = ext.to_ascii_lowercase();
        let nz = |v: Option<u32>| v.filter(|v| *v > 0);
        Self {
            user: user.to_owned(),
            path,
            size: f.size,
            format: ext.to_ascii_uppercase(),
            lossless: LOSSLESS_EXTENSIONS.contains(&lower.as_str()),
            bitrate: nz(f.attr(0)),
            duration: nz(f.attr(1)),
            sample_rate: nz(f.attr(4)),
            bit_depth: nz(f.attr(5)),
            free_slot,
            avg_speed,
            queue_length,
        }
    }

    /// The file's name, without the folders.
    pub fn file_name(&self) -> &str {
        self.path.rsplit('\\').next().unwrap_or(&self.path)
    }

    /// The sharer's folder holding the file.
    pub fn folder(&self) -> &str {
        self.path.rsplit_once('\\').map_or("", |(d, _)| d)
    }
}

/// The files of one folder of one sharer, as an album candidate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlbumResult {
    pub user: String,
    pub folder: String,
    pub files: Vec<SearchResult>,
    pub size: u64,
    /// The format of most audio files in it.
    pub format: String,
    pub lossless: bool,
    pub free_slot: bool,
    pub queue_length: u32,
}

/// Gathers results by sharer and folder, larger folders first.
pub fn group_albums(results: &[SearchResult]) -> Vec<AlbumResult> {
    let mut out: Vec<AlbumResult> = Vec::new();
    for r in results {
        let folder = r.folder();
        match out.iter_mut().find(|a| a.user == r.user && a.folder == folder) {
            Some(a) => {
                a.size += r.size;
                a.files.push(r.clone());
            }
            None => out.push(AlbumResult {
                user: r.user.clone(),
                folder: folder.to_owned(),
                files: vec![r.clone()],
                size: r.size,
                format: String::new(),
                lossless: false,
                free_slot: r.free_slot,
                queue_length: r.queue_length,
            }),
        }
    }
    for a in &mut out {
        let mut counts: Vec<(&str, usize)> = Vec::new();
        for f in &a.files {
            if matches!(f.format.as_str(), "JPG" | "JPEG" | "PNG" | "CUE" | "LOG" | "") {
                continue;
            }
            match counts.iter_mut().find(|(k, _)| *k == f.format) {
                Some((_, n)) => *n += 1,
                None => counts.push((&f.format, 1)),
            }
        }
        counts.sort_by_key(|c| std::cmp::Reverse(c.1));
        a.format = counts.first().map(|(k, _)| (*k).to_owned()).unwrap_or_default();
        a.lossless = a.files.iter().any(|f| f.lossless && f.format == a.format);
        a.files.sort_by(|x, y| x.path.cmp(&y.path));
    }
    out.sort_by(|a, b| b.files.len().cmp(&a.files.len()).then(b.size.cmp(&a.size)));
    out
}

/// Where a download is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DownloadState {
    /// Asked for, or waiting in the sharer's queue (with the place when the
    /// sharer told it).
    Queued { place: Option<u32> },
    /// The sharer is ready; the file connection is being set up.
    Connecting,
    Transferring { received: u64, size: u64, bytes_per_sec: u64 },
    Done { path: PathBuf },
    Failed { reason: String },
}

impl DownloadState {
    pub fn is_finished(&self) -> bool {
        matches!(self, Self::Done { .. } | Self::Failed { .. })
    }
}

/// A download in a list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadInfo {
    pub id: u64,
    pub user: String,
    pub path: String,
    pub size: u64,
    pub state: DownloadState,
}

/// What and how we share.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShareSettings {
    /// Share the library and answer searches and browse requests.
    pub enabled: bool,
    /// KiB per second for all uploads together; 0 = no limit.
    pub upload_limit_kbps: u32,
    /// Uploads at once.
    pub upload_slots: u32,
}

impl Default for ShareSettings {
    fn default() -> Self {
        Self { enabled: true, upload_limit_kbps: 0, upload_slots: 2 }
    }
}

/// The connection to the network.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Connection {
    #[default]
    Disconnected,
    Connecting,
    LoggedIn,
    /// Will not reconnect by itself (wrong password, logged in elsewhere).
    Failed { reason: String },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub connection: Connection,
    pub username: String,
    /// The port peers connect to.
    pub listen_port: u16,
    pub shared_files: u32,
    pub shared_dirs: u32,
    pub uploads_active: u32,
    pub uploads_queued: u32,
    pub uploaded_bytes: u64,
    pub downloads_active: u32,
    /// A parent in the distributed network sends us others' searches (or
    /// the server does, when we are a branch root).
    pub distributed: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not logged in to Soulseek")]
    NotLoggedIn,
    #[error("login refused: {0}")]
    Login(String),
    #[error("user {0} is offline")]
    Offline(String),
    #[error("cannot connect to {0}")]
    Unreachable(String),
    #[error("timed out")]
    Timeout,
    #[error("refused: {0}")]
    Denied(String),
    #[error("cancelled")]
    Cancelled,
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("remote service: {0}")]
    Remote(String),
    #[error("settings: {0}")]
    Config(String),
}

impl From<crate::codec::Malformed> for Error {
    fn from(e: crate::codec::Malformed) -> Self {
        Self::Protocol(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(user: &str, path: &str, size: u64) -> SearchResult {
        let f = FileEntry { name: path.into(), size, ext: String::new(), attrs: vec![(1, 200), (4, 44100), (5, 24)] };
        SearchResult::from_entry(user, path.into(), &f, true, 0, 0)
    }

    #[test]
    fn result_fields() {
        let x = r("u", "@@a\\Artist\\Album\\01 Song.flac", 5);
        assert_eq!(x.format, "FLAC");
        assert!(x.lossless);
        assert_eq!(x.bit_depth, Some(24));
        assert_eq!(x.bitrate, None);
        assert_eq!(x.file_name(), "01 Song.flac");
        assert_eq!(x.folder(), "@@a\\Artist\\Album");
    }

    #[test]
    fn albums_group_by_user_and_folder() {
        let res = vec![
            r("a", "x\\Album\\1.flac", 10),
            r("a", "x\\Album\\2.flac", 10),
            r("a", "x\\Album\\cover.jpg", 1),
            r("b", "y\\Album\\1.mp3", 4),
        ];
        let g = group_albums(&res);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].user, "a");
        assert_eq!(g[0].files.len(), 3);
        assert_eq!(g[0].format, "FLAC");
        assert!(g[0].lossless);
        assert_eq!(g[1].format, "MP3");
        assert!(!g[1].lossless);
    }

    #[test]
    fn states_serialize_tagged() {
        let s = serde_json::to_string(&DownloadState::Queued { place: Some(3) }).unwrap();
        assert_eq!(s, r#"{"state":"queued","place":3}"#);
        let info = DownloadInfo {
            id: 1,
            user: "u".into(),
            path: "a\\b".into(),
            size: 5,
            state: DownloadState::Transferring { received: 1, size: 5, bytes_per_sec: 2 },
        };
        let back: DownloadInfo = serde_json::from_str(&serde_json::to_string(&info).unwrap()).unwrap();
        assert_eq!(back, info);
    }
}
