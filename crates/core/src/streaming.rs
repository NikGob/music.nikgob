//! Streaming services (Spotify, YouTube Music, the Deezer catalogue) behind
//! one trait.
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
use crate::model::{Codec, ReleaseType, SourceKind, UnixMs, score};

#[cfg(feature = "fake")]
mod fake;
#[cfg(feature = "fake")]
pub use fake::FakeService;

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

/// A streaming service. Stored as the same lowercase string as the matching
/// [`SourceKind`] (`"spotify"`, `"youtube"`, `"deezer"`). Deezer is a
/// catalogue only ([`StreamService::catalogue_only`]): no account, no sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Service {
    Spotify,
    Youtube,
    Deezer,
}

impl Service {
    pub const ALL: &'static [Self] = &[Self::Spotify, Self::Youtube, Self::Deezer];

    /// Stable lowercase name used in the database and in JSON.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Spotify => "spotify",
            Self::Youtube => "youtube",
            Self::Deezer => "deezer",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "spotify" => Some(Self::Spotify),
            "youtube" => Some(Self::Youtube),
            "deezer" => Some(Self::Deezer),
            _ => None,
        }
    }

    /// Name shown to the user.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Spotify => "Spotify",
            Self::Youtube => "YouTube Music",
            Self::Deezer => "Deezer",
        }
    }

    /// Codec of the audio the service streams (YouTube also has AAC
    /// formats; Opus is what is asked for). A Deezer track plays from
    /// YouTube Music.
    pub fn codec(self) -> Codec {
        match self {
            Self::Spotify => Codec::Vorbis,
            Self::Youtube | Self::Deezer => Codec::Opus,
        }
    }

    /// What the stream usually is, until the opened stream says
    /// ([`AudioStream::quality`]): "Vorbis 320 кбит/с", "Opus ~160 кбит/с".
    pub fn nominal_quality(self) -> String {
        match self {
            Self::Spotify => quality_label("Vorbis", 320, false),
            Self::Youtube | Self::Deezer => quality_label("Opus", 160, true),
        }
    }

    /// Nominal bitrate of the stream in kbps.
    pub const fn bitrate_kbps(self) -> u32 {
        match self {
            Self::Spotify => 320,
            Self::Youtube | Self::Deezer => 160,
        }
    }

    /// `quality_score` of the library source of a stream: below every
    /// local file ([`crate::model::score`]).
    pub const fn quality_score(self) -> i32 {
        match self {
            Self::Spotify => score::STREAM_SPOTIFY,
            Self::Youtube => score::STREAM_YOUTUBE,
            Self::Deezer => score::STREAM_DEEZER,
        }
    }

    /// The kind of the library [`crate::model::Source`] rows this service plays.
    pub const fn source_kind(self) -> SourceKind {
        match self {
            Self::Spotify => SourceKind::Spotify,
            Self::Youtube => SourceKind::Youtube,
            Self::Deezer => SourceKind::Deezer,
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
            Self::Deezer => Some(Service::Deezer),
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

/// How a library playlist follows its counterpart in a service.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaylistSyncMode {
    /// Changes go both ways (our edits are pushed to the service).
    TwoWay,
    /// The service is followed: its changes come to us, ours stay local.
    #[default]
    Follow,
    /// A one-time snapshot, never synced again.
    Once,
}

impl PlaylistSyncMode {
    pub const ALL: &'static [Self] = &[Self::TwoWay, Self::Follow, Self::Once];

    /// Stable name in the database (`playlist_remote.sync_mode`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TwoWay => "two_way",
            Self::Follow => "follow",
            Self::Once => "once",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "two_way" => Some(Self::TwoWay),
            "follow" => Some(Self::Follow),
            "once" => Some(Self::Once),
            _ => None,
        }
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
        /// The service can play sound right now. Spotify needs its separate
        /// playback authorization ([`LoginInput::Playback`]) and Premium;
        /// until then the runtime plays the same track from YouTube Music.
        audio: bool,
    },
    /// The last login failed or the session expired.
    Failed(String),
}

impl Account {
    pub const fn is_logged_in(&self) -> bool {
        matches!(self, Self::LoggedIn { .. })
    }
}

/// What Spotify's account says when it has no Premium: Spotify is not
/// connected at all then (docs/stage3.md §0, 2026-10-17). The service's
/// [`Account::Failed`] and the runtime's login error both read this.
pub const SPOTIFY_PREMIUM_REQUIRED: &str =
    "Нужен Spotify Premium — без него Spotify не подключить; YouTube Music работает и так";

/// What the user must do to finish logging in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoginPrompt {
    /// Open this url in the browser (the service redirects back to us).
    OpenUrl(String),
    /// Open `url` and enter `code` (OAuth device flow).
    DeviceCode { url: String, code: String },
    /// The app opened its own sign-in window (YouTube Music's Google
    /// sign-in); the text says what to do there.
    Window(String),
}

/// How to log in.
#[derive(Clone, PartialEq, Eq)]
pub enum LoginInput {
    /// Browser based OAuth (redirect or device code, whatever the service uses).
    /// For Spotify this signs in again with the Client ID it remembers (the
    /// last [`Self::PersonalApp`]); without one it answers
    /// [`StreamError::NotConfigured`]. For YouTube Music it is the Google
    /// sign-in window the app runs itself (the runtime's `WebLogin`), which
    /// ends in a [`Self::Cookies`] login; the service alone answers
    /// [`StreamError::Unsupported`].
    Browser,
    /// Spotify only: the separate local playback authorization (librespot,
    /// `streaming` scope). Other services answer [`StreamError::Unsupported`].
    Playback,
    /// Spotify only: the Web API sign-in (PKCE in the browser) with the
    /// user's own Development Mode app, the only Web API grant Spotify is
    /// used with. Needs Premium: without it the login ends in
    /// [`StreamError::PremiumRequired`] and nothing is kept.
    PersonalApp { client_id: String },
    /// Pasted cookies (`cookies.txt` content or a `Cookie:` header).
    Cookies(String),
}

impl fmt::Debug for LoginInput {
    /// Cookies are secrets: never printed.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Browser => f.write_str("Browser"),
            Self::Playback => f.write_str("Playback"),
            Self::PersonalApp { .. } => f.write_str("PersonalApp"),
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
    /// The account cannot stream audio from this service (Spotify Free).
    #[error("premium account required")]
    PremiumRequired,
    #[error("rate limited")]
    RateLimited { retry_after: Option<Duration> },
    /// Not available (region, withdrawn, private, no such id).
    #[error("not available")]
    Unavailable,
    /// Only for signed-in adults (YouTube's age gate): needs a login.
    #[error("age-restricted")]
    AgeRestricted,
    /// Blocked where the user is (a label or region block).
    #[error("blocked in this region")]
    RegionBlocked,
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
    /// What actually plays, for the player ("Opus ~160 кбит/с", "AAC ~128
    /// кбит/с", "Vorbis 320 кбит/с"), if known. Called after the first read.
    fn quality(&self) -> Option<String> {
        None
    }
}

/// "Opus ~160 кбит/с": a codec and its bitrate, `approx` for a variable
/// one (the UI's units are Russian, as in the settings' Opus bitrate).
pub fn quality_label(codec: &str, kbps: u32, approx: bool) -> String {
    format!("{codec} {}{kbps} кбит/с", if approx { "~" } else { "" })
}

/// Where [`StreamService::search_in_parts`] hands the results it already
/// has (each call carries everything found so far).
pub type SearchParts = std::sync::Arc<dyn Fn(StreamSearch) + Send + Sync>;

/// What a kind-limited search ([`StreamService::search_kind`]) asks for:
/// one category, one request where the service can do that.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SearchKind {
    /// Official songs ([`TrackKind::Song`]; every Spotify track).
    Songs,
    /// Music videos and uploads (YouTube only; Spotify answers nothing).
    Videos,
    Albums,
    Artists,
}

/// The newest end of the liked list ([`StreamService::liked_head`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LikedHead {
    /// The most recently liked tracks (the first page), newest first.
    pub tracks: Vec<StreamTrack>,
    /// How many tracks the whole list has, if the service says.
    pub total: Option<u32>,
    /// `tracks` is the whole list.
    pub complete: bool,
}

/// [`StreamTrack::kind`] fits `kind` (songs: everything but videos and
/// uploads).
fn fits(t: &StreamTrack, kind: SearchKind) -> bool {
    match kind {
        SearchKind::Songs => t.kind == TrackKind::Song,
        SearchKind::Videos => t.kind != TrackKind::Song,
        SearchKind::Albums | SearchKind::Artists => false,
    }
}

/// Keeps only the `kind` part of a full search answer.
pub fn only_kind(mut s: StreamSearch, kind: SearchKind) -> StreamSearch {
    s.tracks.retain(|t| fits(t, kind));
    if kind != SearchKind::Albums {
        s.albums.clear();
    }
    if kind != SearchKind::Artists {
        s.artists.clear();
    }
    s.playlists.clear();
    s
}

// ---------------------------------------------------------------------------
// The service
// ---------------------------------------------------------------------------

pub trait StreamService: Send + Sync {
    fn service(&self) -> Service;

    /// A catalogue without an account or sound of its own (Deezer): usable
    /// whenever it is switched on, never logged in ([`Account::LoggedOut`] is
    /// not a missing login here). Its tracks play from YouTube Music the way
    /// a Spotify track without Premium does, and likes stay local: the
    /// runtime sends it no likes and syncs no playlists with it.
    fn catalogue_only(&self) -> bool {
        false
    }

    /// Current account state. Cheap, no network.
    fn account(&self) -> Account;

    /// Logs in; resolves when done (or failed). Meanwhile [`Self::account`]
    /// reads [`Account::LoggingIn`] with the prompt to show.
    fn login(&self, input: LoginInput) -> BoxFuture<'_, Result<(), StreamError>>;

    /// Forgets the session and any stored tokens. No network.
    fn logout(&self);

    /// Catalog search: tracks, albums, artists, playlists, best first.
    fn search(&self, query: &str) -> BoxFuture<'_, Result<StreamSearch, StreamError>>;

    /// [`Self::search`] that shows its first part early: `part` gets what is
    /// already found while slower parts (playlists, more categories) are
    /// still on their way; the future resolves with the whole answer.
    fn search_in_parts(
        &self,
        query: &str,
        part: SearchParts,
    ) -> BoxFuture<'_, Result<StreamSearch, StreamError>> {
        let _ = part;
        self.search(query)
    }

    /// The quick mixed answer for the «Всё» page of the search: what the
    /// service can say in its fewest requests (YouTube Music: one mixed
    /// request; Deezer: tracks, albums and artists, no playlists). The
    /// category requests stay for the service's own chip
    /// ([`Self::search_in_parts`]). Parts go to `part` as they come.
    fn search_top(
        &self,
        query: &str,
        part: SearchParts,
    ) -> BoxFuture<'_, Result<StreamSearch, StreamError>> {
        self.search_in_parts(query, part)
    }

    /// One category of a search ([`SearchKind`]), for background work
    /// (resolution, discography, share): one request where the service can
    /// do that, sent in its background lane. The answer has only that part.
    fn search_kind(
        &self,
        query: &str,
        kind: SearchKind,
    ) -> BoxFuture<'_, Result<StreamSearch, StreamError>> {
        let query = query.to_owned();
        Box::pin(async move { self.search(&query).await.map(|s| only_kind(s, kind)) })
    }

    fn track(&self, id: &str) -> BoxFuture<'_, Result<StreamTrack, StreamError>>;

    /// Many tracks at once, in the order of `ids` (`None`: no such track),
    /// in as few requests as the service allows. An error is the whole
    /// call's (rate limited, not logged in).
    fn tracks(&self, ids: &[String]) -> BoxFuture<'_, Result<Vec<Option<StreamTrack>>, StreamError>> {
        let ids = ids.to_vec();
        Box::pin(async move {
            let mut out = Vec::with_capacity(ids.len());
            for id in &ids {
                match self.track(id).await {
                    Ok(t) => out.push(Some(t)),
                    Err(StreamError::Unavailable) => out.push(None),
                    Err(e) => return Err(e),
                }
            }
            Ok(out)
        })
    }

    /// An album with its tracks.
    fn album(&self, id: &str) -> BoxFuture<'_, Result<StreamAlbum, StreamError>>;

    /// An artist with the discography (albums without tracks).
    fn artist(&self, id: &str) -> BoxFuture<'_, Result<StreamArtist, StreamError>>;

    /// A playlist with its tracks.
    fn playlist(&self, id: &str) -> BoxFuture<'_, Result<StreamPlaylist, StreamError>>;

    /// Only the [`StreamPlaylist::version`] of a playlist, as cheaply as the
    /// service can tell it (Spotify: `fields=snapshot_id`), never from a
    /// cache. `None`: the service has no versions.
    fn playlist_version(&self, id: &str) -> BoxFuture<'_, Result<Option<String>, StreamError>> {
        let id = id.to_owned();
        Box::pin(async move {
            self.forget(EntityKind::Playlist, &id);
            Ok(self.playlist(&id).await?.version)
        })
    }

    /// Drops whatever the service keeps in memory about this item, so the
    /// next read asks the service (a sync after the version changed). No
    /// network.
    fn forget(&self, kind: EntityKind, id: &str) {
        let _ = (kind, id);
    }

    /// Until when the service is paused after a rate limit or a bot check
    /// (calls fail at once with [`StreamError::RateLimited`] meanwhile).
    /// Cheap, no network.
    fn paused_until(&self) -> Option<std::time::SystemTime> {
        None
    }

    /// The track with this ISRC, if the service can look ISRCs up and has it.
    fn find_isrc(&self, isrc: &str) -> BoxFuture<'_, Result<Option<StreamTrack>, StreamError>>;

    /// The song (ATV: the record, with its album and cover) of a music
    /// video the service lists, if it has one; `video` is the video as the
    /// library knows it (title, artists, length). A `video` that turns out
    /// to be a song already answers itself. Background work: as few
    /// requests as the service can do it in, in its background lane.
    /// Services without music videos have none.
    fn song_for_video(&self, video: &StreamTrack) -> BoxFuture<'_, Result<Option<StreamTrack>, StreamError>> {
        let _ = video;
        Box::pin(std::future::ready(Ok(None)))
    }

    /// The user's liked tracks, most recently liked first. Needs a login.
    fn liked(&self) -> BoxFuture<'_, Result<Vec<StreamTrack>, StreamError>>;

    /// The newest page of the liked list and the list's length: one
    /// request, enough to tell "nothing new" (see [`LikedHead`]). Needs a
    /// login.
    fn liked_head(&self) -> BoxFuture<'_, Result<LikedHead, StreamError>> {
        Box::pin(async move {
            let tracks = self.liked().await?;
            Ok(LikedHead {
                total: u32::try_from(tracks.len()).ok(),
                tracks,
                complete: true,
            })
        })
    }

    /// Likes (or unlikes) a track. Needs a login.
    fn set_liked(&self, id: &str, liked: bool) -> BoxFuture<'_, Result<(), StreamError>>;

    /// [`Self::set_liked`] for many tracks, in as few requests as the
    /// service allows (Spotify: 40 per request; YouTube: one at a time).
    fn set_liked_many(&self, ids: &[String], liked: bool) -> BoxFuture<'_, Result<(), StreamError>> {
        let ids = ids.to_vec();
        Box::pin(async move {
            for id in &ids {
                self.set_liked(id, liked).await?;
            }
            Ok(())
        })
    }

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

    /// Bytes of this service's recently played audio kept on disk.
    fn cache_size(&self) -> u64 {
        0
    }

    /// Bounds that cache (bytes); 0 turns it off.
    fn set_cache_limit(&self, bytes: u64) {
        let _ = bytes;
    }

    /// Empties that cache.
    fn clear_cache(&self) {}

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
    fn sync_mode_strings() {
        for &m in PlaylistSyncMode::ALL {
            assert_eq!(PlaylistSyncMode::parse(m.as_str()), Some(m));
            assert_eq!(serde_json::to_string(&m).unwrap(), format!("\"{}\"", m.as_str()));
        }
        assert_eq!(PlaylistSyncMode::default(), PlaylistSyncMode::Follow);
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
