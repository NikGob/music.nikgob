//! A Soulseek client written for the player, with no external program.
//!
//! - [`Client`] is the embedded client: one server connection, a listener
//!   for peers, peer connections made directly or through the server's
//!   `ConnectToPeer` (whichever works first), searches, downloads with
//!   resume, and sharing of the library folders read-only (search answers,
//!   browse, folder contents, a queue of uploads with slots and a speed
//!   limit). It joins the distributed search network as a leaf only while
//!   sharing, so others' searches reach it; it never takes children.
//! - The `nikgob-slsk` binary runs the same client headless (a server, a
//!   Raspberry Pi) behind a small HTTP API ([`http`]), and
//!   [`RemoteClient`] talks to it; [`Soulseek`] is either of the two, with
//!   one set of calls.
//! - Protocol layouts follow the Nicotine+ protocol documentation
//!   (`doc/SLSKPROTOCOL.md`), using the modern connection order (direct and
//!   indirect attempts at once) and major version 177, which the
//!   documentation sets aside for clients outside the established ones.
//!
//! Light by design: everything runs as tasks on whatever Tokio runtime
//! starts the client (the player gives it one low-priority thread), idle
//! peer connections are closed, incoming searches are answered from a word
//! index, and only a few answers are sent at once (the rest are dropped).

mod client;
pub mod codec;
pub mod http;
mod limit;
pub mod msg;
mod remote;
pub mod share;
mod transfer;
mod types;

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

pub use client::{Client, SearchHandle};
pub use transfer::{Download, MAX_FILE_BYTES, partial_path};
pub use remote::RemoteClient;
pub use share::ScanOptions;
pub use types::{
    AlbumResult, Connection, DownloadInfo, DownloadState, Error, QueryKind, SearchQuery, SearchResult, ShareSettings,
    Status, UploadInfo, UploadState, group_albums,
};

/// The official server.
pub const DEFAULT_SERVER: &str = "server.slsknet.org:2242";
/// The port clients listen on by default.
pub const DEFAULT_PORT: u16 = 2234;
/// How long at least between two searches sent to the server, the default
/// of [`Config::search_pace`]: the server is said to ignore a client that
/// sends them closer than this.
pub const DEFAULT_SEARCH_PACE: Duration = Duration::from_secs(7);

/// How long the client waits for things on the network.
#[derive(Clone, Copy, Debug)]
pub struct Timeouts {
    /// A direct connection to a peer.
    pub connect: Duration,
    /// The peer's answer to an indirect connection request.
    pub indirect: Duration,
    /// The first wait before reconnecting to the server (doubles up to five
    /// minutes).
    pub reconnect: Duration,
    /// A peer connection with no traffic for this long is closed.
    pub peer_idle: Duration,
    /// The other side of a transfer must answer within this.
    pub transfer: Duration,
    /// A download waiting in a sharer's queue with no word for this long is
    /// asked for again (a sharer gone away fails it then).
    pub queue_check: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(8),
            indirect: Duration::from_secs(20),
            reconnect: Duration::from_secs(15),
            peer_idle: Duration::from_secs(120),
            transfer: Duration::from_secs(60),
            queue_check: Duration::from_secs(300),
        }
    }
}

/// How the client runs. Credentials come from the user's settings and are
/// never logged.
#[derive(Clone)]
pub struct Config {
    /// `host:port` of the server.
    pub server: String,
    pub username: String,
    pub password: String,
    /// The port peers connect to; 0 picks a free one. When the port is
    /// taken, the next few are tried.
    pub listen_port: u16,
    /// The folders shared (read-only).
    pub share_folders: Vec<PathBuf>,
    pub share: ShareSettings,
    /// Where the share index cache lives (never inside a shared folder).
    pub state_dir: Option<PathBuf>,
    pub scan: ScanOptions,
    /// How often the shared folders are listed again.
    pub rescan_every: Option<Duration>,
    /// How long at least between two searches sent to the server (0: no
    /// pace); [`Client::search`] holds every search but the first.
    pub search_pace: Duration,
    pub timeouts: Timeouts,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: DEFAULT_SERVER.into(),
            username: String::new(),
            password: String::new(),
            listen_port: DEFAULT_PORT,
            share_folders: Vec::new(),
            share: ShareSettings::default(),
            state_dir: None,
            scan: ScanOptions::default(),
            rescan_every: Some(Duration::from_secs(3 * 3600)),
            search_pace: DEFAULT_SEARCH_PACE,
            timeouts: Timeouts::default(),
        }
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("server", &self.server)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("listen_port", &self.listen_port)
            .field("share_folders", &self.share_folders)
            .field("share", &self.share)
            .field("state_dir", &self.state_dir)
            .finish_non_exhaustive()
    }
}

/// The embedded client or a remote `nikgob-slsk`, behind one set of calls.
#[derive(Clone)]
pub enum Soulseek {
    Embedded(Client),
    Remote(RemoteClient),
}

impl Soulseek {
    pub async fn status(&self) -> Result<Status, Error> {
        match self {
            Self::Embedded(c) => Ok(c.status()),
            Self::Remote(r) => r.status().await,
        }
    }

    /// Searches for `wait`, then returns what came in.
    pub async fn search(&self, query: &SearchQuery, wait: Duration) -> Result<Vec<SearchResult>, Error> {
        match self {
            Self::Embedded(c) => c.search(query, wait).await,
            Self::Remote(r) => r.search(query, wait).await,
        }
    }

    /// Every file of a sharer's folder (to take a whole album).
    pub async fn folder(&self, user: &str, folder: &str) -> Result<Vec<SearchResult>, Error> {
        match self {
            Self::Embedded(c) => c.folder(user, folder).await,
            Self::Remote(r) => r.folder(user, folder).await,
        }
    }

    /// Downloads `result` into `<dir>/<the sharer's folder name>/`. A
    /// remote service downloads on its side, then the file is fetched into
    /// `dir` here and removed there.
    pub async fn download(&self, result: &SearchResult, dir: &std::path::Path) -> Result<Download, Error> {
        match self {
            Self::Embedded(c) => Ok(c.download(result, dir)),
            Self::Remote(r) => r.download(result, dir).await,
        }
    }

    /// Downloads in a list (running, queued and the last finished).
    pub async fn downloads(&self) -> Result<Vec<DownloadInfo>, Error> {
        match self {
            Self::Embedded(c) => Ok(c.downloads()),
            Self::Remote(r) => r.downloads().await,
        }
    }

    /// Uploads in a list (running, queued and the last finished). A
    /// service from before the list existed answers with an error.
    pub async fn uploads(&self) -> Result<Vec<UploadInfo>, Error> {
        match self {
            Self::Embedded(c) => Ok(c.uploads()),
            Self::Remote(r) => r.uploads().await,
        }
    }

    pub async fn set_share(&self, share: ShareSettings) -> Result<(), Error> {
        match self {
            Self::Embedded(c) => {
                c.set_share(share);
                Ok(())
            }
            Self::Remote(r) => r.set_share(share).await,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn config_debug_hides_the_password() {
        let c = super::Config { password: "hunter2".into(), ..super::Config::default() };
        assert!(!format!("{c:?}").contains("hunter2"));
    }
}
