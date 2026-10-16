//! Streaming services (Spotify, YouTube Music) behind one trait.
//!
//! The rest of the app talks only to [`StreamService`]; the concrete backends
//! live in their own crates (`nikgob-spotify`, `nikgob-youtube`) and are held as
//! `Arc<dyn StreamService>`, like [`crate::indexer::Indexer`]. Everything here
//! is plain data plus the two traits; see docs/stage3.md §2 for the contract.
//!
//! # Threads
//!
//! * The async methods are polled on the runtime's tokio runtime (multi
//!   threaded), never on the UI thread and never on the runtime worker thread:
//!   the worker spawns a task and gets the result back as a message.
//!   Implementations must not block inside a future (use async I/O or
//!   `spawn_blocking`).
//! * The returned futures borrow only `&self`: copy any `&str` / slice
//!   argument into the future before returning it.
//! * [`StreamService::account`], [`StreamService::logout`] and
//!   [`StreamService::web_url`] are cheap and synchronous: no network, no
//!   waiting on a lock held across I/O. They may be called from any thread,
//!   the UI thread included.
//! * [`StreamService::open_audio`] is called by the audio engine off the UI
//!   and worker threads (on its own opener thread) and must not touch the
//!   network itself: the network work happens on the first read of the
//!   returned [`AudioStream`]. Reads then run on the engine's decoder thread,
//!   so a stream should download ahead on a background task and hand out
//!   buffered bytes, blocking only while the wanted range is not there yet.

use std::fmt;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub use crate::indexer::BoxFuture;
use crate::model::{ReleaseType, SourceKind, UnixMs};

#[cfg(feature = "fake")]
mod fake;
#[cfg(feature = "fake")]
pub use fake::FakeService;

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

/// A streaming service. Stored as the same lowercase string as the matching
/// [`SourceKind`] (`"spotify"`, `"youtube"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Service {
    Spotify,
    Youtube,
}

impl Service {
    pub const ALL: &'static [Self] = &[Self::Spotify, Self::Youtube];

    /// Stable lowercase name used in the database and in JSON.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Spotify => "spotify",
            Self::Youtube => "youtube",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "spotify" => Some(Self::Spotify),
            "youtube" => Some(Self::Youtube),
            _ => None,
        }
    }

    /// Name shown to the user.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Spotify => "Spotify",
            Self::Youtube => "YouTube Music",
        }
    }

    /// The kind of the library [`crate::model::Source`] rows this service plays.
    pub const fn source_kind(self) -> SourceKind {
        match self {
            Self::Spotify => SourceKind::Spotify,
            Self::Youtube => SourceKind::Youtube,
        }
    }
}

impl fmt::Display for Service {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<Service> for SourceKind {
    fn from(s: Service) -> Self {
        s.source_kind()
    }
}

impl TryFrom<SourceKind> for Service {
    /// A local file has no service; the kind comes back as the error.
    type Error = SourceKind;

    fn try_from(kind: SourceKind) -> Result<Self, SourceKind> {
        kind.service().ok_or(kind)
    }
}

impl SourceKind {
    /// The streaming service of a stream source; `None` for a local file.
    pub const fn service(self) -> Option<Service> {
        match self {
            Self::LocalFile => None,
            Self::Spotify => Some(Service::Spotify),
            Self::Youtube => Some(Service::Youtube),
        }
    }
}

/// What a remote id points at (for [`StreamService::web_url`] and the
/// library's `external_id` table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Track,
    /// An album (a release, in library terms).
    Album,
    Artist,
    Playlist,
}

impl EntityKind {
    pub const ALL: &'static [Self] = &[Self::Track, Self::Album, Self::Artist, Self::Playlist];

    /// Stable name in the library's `external_id.kind` column. An album is
    /// stored as `"release"`, the library's name for it.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Track => "track",
            Self::Album => "release",
            Self::Artist => "artist",
            Self::Playlist => "playlist",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "track" => Some(Self::Track),
            "release" | "album" => Some(Self::Album),
            "artist" => Some(Self::Artist),
            "playlist" => Some(Self::Playlist),
            _ => None,
        }
    }
}

/// An id inside one service: a Spotify base-62 id (not the `spotify:track:`
/// uri), a YouTube video / browse / playlist id.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RemoteId {
    pub service: Service,
    pub id: String,
}

impl RemoteId {
    pub fn new(service: Service, id: impl Into<String>) -> Self {
        Self {
            service,
            id: id.into(),
        }
    }
}

impl fmt::Display for RemoteId {
    /// `spotify:4uLU6hMCjMI75M1A2tKUQC`
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.service, self.id)
    }
}

// ---------------------------------------------------------------------------
// Catalog data
// ---------------------------------------------------------------------------

/// A credited artist. `id` is `None` when the service gives a bare name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistRef {
    pub id: Option<RemoteId>,
    pub name: String,
}

impl ArtistRef {
    pub fn new(id: Option<RemoteId>, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
        }
    }
}

/// The album a track belongs to, as far as the track listing tells.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlbumRef {
    pub id: Option<RemoteId>,
    pub title: String,
    /// Album artists; empty = the track's artists.
    #[serde(default)]
    pub artists: Vec<ArtistRef>,
    #[serde(default)]
    pub year: Option<i32>,
}

impl AlbumRef {
    pub fn new(id: Option<RemoteId>, title: impl Into<String>) -> Self {
        Self {
            id,
            title: title.into(),
            artists: Vec::new(),
            year: None,
        }
    }
}

/// What kind of item a track is. YouTube Music mixes official songs ("ATV"),
/// music videos and user uploads; Spotify only has songs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    #[default]
    Song,
    Video,
    Upload,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamTrack {
    pub id: RemoteId,
    pub title: String,
    pub artists: Vec<ArtistRef>,
    pub album: Option<AlbumRef>,
    pub duration_ms: Option<u64>,
    /// Upper case, no dashes.
    pub isrc: Option<String>,
    pub disc_no: Option<u32>,
    pub track_no: Option<u32>,
    /// Largest cover image.
    pub cover_url: Option<String>,
    #[serde(default)]
    pub kind: TrackKind,
    /// `false` when the service lists the track but will not play it (region,
    /// withdrawn).
    pub playable: bool,
}

impl StreamTrack {
    /// A playable song with just an id and a title.
    pub fn new(id: RemoteId, title: impl Into<String>) -> Self {
        Self {
            id,
            title: title.into(),
            artists: Vec::new(),
            album: None,
            duration_ms: None,
            isrc: None,
            disc_no: None,
            track_no: None,
            cover_url: None,
            kind: TrackKind::Song,
            playable: true,
        }
    }

    /// `"A, B"`: the artist names joined for display.
    pub fn artist_line(&self) -> String {
        join_names(&self.artists)
    }
}

/// Artist names joined with `", "`.
pub fn join_names(artists: &[ArtistRef]) -> String {
    artists
        .iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamAlbum {
    pub id: RemoteId,
    pub title: String,
    pub artists: Vec<ArtistRef>,
    pub year: Option<i32>,
    #[serde(default)]
    pub release_type: ReleaseType,
    pub cover_url: Option<String>,
    pub total_tracks: Option<u32>,
    /// Empty in listings ([`StreamArtist::albums`], search results); filled by
    /// [`StreamService::album`].
    #[serde(default)]
    pub tracks: Vec<StreamTrack>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamArtist {
    pub id: RemoteId,
    pub name: String,
    pub image_url: Option<String>,
    /// The discography, newest first, albums without their tracks. Empty in
    /// search results.
    #[serde(default)]
    pub albums: Vec<StreamAlbum>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamPlaylist {
    pub id: RemoteId,
    pub name: String,
    /// Display name of the owner.
    pub owner: Option<String>,
    pub cover_url: Option<String>,
    /// Spotify `snapshot_id` / YouTube etag: changes whenever the content
    /// changes. `None` when the service has no such thing.
    pub version: Option<String>,
    /// The user may change it (owns it or is a collaborator).
    pub editable: bool,
    pub total_tracks: Option<u32>,
    /// In playlist order. Empty from [`StreamService::my_playlists`] and in
    /// search results; filled by [`StreamService::playlist`].
    #[serde(default)]
    pub tracks: Vec<StreamTrack>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamSearch {
    pub tracks: Vec<StreamTrack>,
    pub albums: Vec<StreamAlbum>,
    pub artists: Vec<StreamArtist>,
    pub playlists: Vec<StreamPlaylist>,
}

/// One entry of the service's listening history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Played {
    pub track: StreamTrack,
    /// When it was played (unix ms).
    pub at_ms: UnixMs,
}

// ---------------------------------------------------------------------------
// Account
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Account {
    #[default]
    LoggedOut,
    /// A [`StreamService::login`] is in progress; the prompt (if any) is what
    /// the user has to act on.
    LoggingIn(Option<LoginPrompt>),
    LoggedIn {
        /// Display name.
        name: String,
        /// `None` when the service does not say.
        premium: Option<bool>,
    },
    /// The last login failed or the session expired.
    Failed(String),
}

impl Account {
    pub const fn is_logged_in(&self) -> bool {
        matches!(self, Self::LoggedIn { .. })
    }
}

/// What the user must do to finish logging in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoginPrompt {
    /// Open this url in the browser (the service redirects back to us).
    OpenUrl(String),
    /// Open `url` and enter `code` (OAuth device flow).
    DeviceCode { url: String, code: String },
}

/// How to log in.
#[derive(Clone, PartialEq, Eq)]
pub enum LoginInput {
    /// Browser based OAuth (redirect or device code, whatever the service uses).
    Browser,
    /// Pasted cookies (`cookies.txt` content or a `Cookie:` header).
    Cookies(String),
}

impl fmt::Debug for LoginInput {
    /// Cookies are secrets: never printed.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Browser => f.write_str("Browser"),
            Self::Cookies(c) => write!(f, "Cookies({} bytes)", c.len()),
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StreamError {
    /// Missing setup (no Client ID in settings.toml).
    #[error("service is not configured")]
    NotConfigured,
    #[error("not logged in")]
    NotLoggedIn,
    #[error("rate limited")]
    RateLimited { retry_after: Option<Duration> },
    /// Not available (region, withdrawn, private, no such id).
    #[error("not available")]
    Unavailable,
    /// The service cannot do this at all (saving audio from Spotify).
    #[error("not supported by this service")]
    Unsupported,
    #[error("service answered with HTTP {status}")]
    Http { status: u16 },
    #[error("network error: {0}")]
    Network(String),
    #[error("unexpected response: {0}")]
    Parse(String),
}

// ---------------------------------------------------------------------------
// Audio
// ---------------------------------------------------------------------------

/// Values of [`AudioStream::format_hint`] (the container, as a file extension).
pub mod format {
    /// Ogg (Vorbis from Spotify, after its own header is stripped).
    pub const OGG: &str = "ogg";
    /// WebM / Matroska (YouTube Opus, itag 251).
    pub const WEBM: &str = "webm";
    /// MP4 audio (YouTube AAC, itag 140/141).
    pub const M4A: &str = "m4a";
    /// RIFF WAVE (the fake service).
    pub const WAV: &str = "wav";
}

/// The encoded bytes of one stream, as the audio engine reads them.
///
/// Created by [`StreamService::open_audio`] without any network traffic; the
/// first `read`/`seek` does the network work. Read on the engine's decoder
/// thread (see the module docs).
pub trait AudioStream: Read + Seek + Send {
    /// Total size in bytes, if known. Called after the first read.
    fn byte_len(&self) -> Option<u64>;
    /// Container of the bytes, one of [`format`].
    fn format_hint(&self) -> &'static str;
    /// Loudness correction for normalization in dB (Spotify's normalisation
    /// data, YouTube's loudness), if the service gives one. Called after the
    /// first read.
    fn gain_db(&self) -> Option<f32> {
        None
    }
    /// Length of the audio, if the service tells (the container usually does).
    fn duration(&self) -> Option<Duration> {
        None
    }
}

// ---------------------------------------------------------------------------
// The service
// ---------------------------------------------------------------------------

pub trait StreamService: Send + Sync {
    fn service(&self) -> Service;

    /// Current account state. Cheap, no network.
    fn account(&self) -> Account;

    /// Logs in; resolves when done (or failed). Meanwhile [`Self::account`]
    /// reads [`Account::LoggingIn`] with the prompt to show.
    fn login(&self, input: LoginInput) -> BoxFuture<'_, Result<(), StreamError>>;

    /// Forgets the session and any stored tokens. No network.
    fn logout(&self);

    /// Catalog search: tracks, albums, artists, playlists, best first.
    fn search(&self, query: &str) -> BoxFuture<'_, Result<StreamSearch, StreamError>>;

    fn track(&self, id: &str) -> BoxFuture<'_, Result<StreamTrack, StreamError>>;

    /// An album with its tracks.
    fn album(&self, id: &str) -> BoxFuture<'_, Result<StreamAlbum, StreamError>>;

    /// An artist with the discography (albums without tracks).
    fn artist(&self, id: &str) -> BoxFuture<'_, Result<StreamArtist, StreamError>>;

    /// A playlist with its tracks.
    fn playlist(&self, id: &str) -> BoxFuture<'_, Result<StreamPlaylist, StreamError>>;

    /// The track with this ISRC, if the service can look ISRCs up and has it.
    fn find_isrc(&self, isrc: &str) -> BoxFuture<'_, Result<Option<StreamTrack>, StreamError>>;

    /// The user's liked tracks, most recently liked first. Needs a login.
    fn liked(&self) -> BoxFuture<'_, Result<Vec<StreamTrack>, StreamError>>;

    /// Likes (or unlikes) a track. Needs a login.
    fn set_liked(&self, id: &str, liked: bool) -> BoxFuture<'_, Result<(), StreamError>>;

    /// The user's own and followed playlists, without tracks. Needs a login.
    fn my_playlists(&self) -> BoxFuture<'_, Result<Vec<StreamPlaylist>, StreamError>>;

    /// Creates an empty playlist; returns its id. Needs a login.
    fn create_playlist(&self, name: &str) -> BoxFuture<'_, Result<String, StreamError>>;

    /// Appends `add` (track ids, in order) and removes every occurrence of
    /// `remove`. Returns the new [`StreamPlaylist::version`]. Needs a login.
    fn playlist_edit(
        &self,
        id: &str,
        add: &[String],
        remove: &[String],
    ) -> BoxFuture<'_, Result<Option<String>, StreamError>>;

    /// Recently played tracks, newest first. Needs a login.
    fn recent(&self) -> BoxFuture<'_, Result<Vec<Played>, StreamError>>;

    /// The audio of a track. No network here: everything happens on the
    /// first read of the stream (see the module docs).
    fn open_audio(&self, id: &str) -> Result<Box<dyn AudioStream>, StreamError>;

    /// Saves the audio as it is (YouTube: Opus remuxed to Ogg, no re-encode)
    /// into `dir`; returns the file. Spotify: [`StreamError::Unsupported`].
    fn save_audio(&self, id: &str, dir: &Path) -> BoxFuture<'_, Result<PathBuf, StreamError>>;

    /// Link to the item on the service's website (open.spotify.com/…,
    /// music.youtube.com/…). No network.
    fn web_url(&self, kind: EntityKind, id: &str) -> String;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_maps_to_source_kind_and_back() {
        for &s in Service::ALL {
            let kind = SourceKind::from(s);
            assert_eq!(kind.as_str(), s.as_str());
            assert_eq!(Service::try_from(kind), Ok(s));
            assert_eq!(kind.service(), Some(s));
            assert_eq!(Service::parse(s.as_str()), Some(s));
            assert_eq!(
                serde_json::to_string(&s).unwrap(),
                format!("\"{}\"", s.as_str())
            );
        }
        assert_eq!(
            Service::try_from(SourceKind::LocalFile),
            Err(SourceKind::LocalFile)
        );
    }

    #[test]
    fn entity_kind_strings() {
        for &k in EntityKind::ALL {
            assert_eq!(EntityKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(EntityKind::Album.as_str(), "release");
        assert_eq!(EntityKind::parse("album"), Some(EntityKind::Album));
    }

    #[test]
    fn track_survives_json_and_cookies_stay_secret() {
        let mut t = StreamTrack::new(RemoteId::new(Service::Spotify, "abc"), "Song");
        t.artists = vec![ArtistRef::new(None, "A"), ArtistRef::new(None, "B")];
        t.album = Some(AlbumRef::new(None, "Album"));
        t.isrc = Some("ZZFAK0000001".into());
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(serde_json::from_str::<StreamTrack>(&json).unwrap(), t);
        assert_eq!(t.artist_line(), "A, B");
        assert_eq!(
            RemoteId::new(Service::Youtube, "x").to_string(),
            "youtube:x"
        );
        let dbg = format!("{:?}", LoginInput::Cookies("SID=secret".into()));
        assert!(!dbg.contains("secret"), "{dbg}");
    }
}
