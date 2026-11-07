//! Shared plain data types (PLAN §4).
//!
//! Everything here is `serde` + `Clone` + `Debug` and does no I/O. The library
//! database, the sync layer and the UI all speak these types.
//!
//! Conventions:
//! * ids are UUID v7 newtypes (time-ordered, so they sort roughly by creation);
//! * timestamps are unix milliseconds ([`UnixMs`]);
//! * user data (playlists, entries, likes) carries `updated_at` / `updated_by` /
//!   `deleted` so it can be merged last-writer-wins and is never hard-deleted.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
pub use uuid::Uuid;

/// Unix time in milliseconds.
pub type UnixMs = i64;

// ---------------------------------------------------------------------------
// Ids
// ---------------------------------------------------------------------------

macro_rules! uuid_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            /// A fresh time-ordered (v7) id.
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            pub const fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            pub const fn as_uuid(&self) -> &Uuid {
                &self.0
            }
        }

        impl Default for $name {
            /// Same as [`Self::new`]: a default id is a fresh one.
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0.hyphenated(), f)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self)
            }
        }

        impl From<Uuid> for $name {
            fn from(uuid: Uuid) -> Self {
                Self(uuid)
            }
        }
    };
}

uuid_id!(
    /// Permanent track id. Stored in file tags so the DB can be rebuilt from files.
    TrackId
);
uuid_id!(ReleaseId);
uuid_id!(ArtistId);
uuid_id!(PlaylistId);
uuid_id!(
    /// Id of one playlist row (so concurrent inserts of the same track merge).
    EntryId
);
uuid_id!(SourceId);
uuid_id!(EventId);
uuid_id!(WishlistId);

/// Device / node identity. Opaque string: a random uuid for now, a public key
/// once the sync layer lands (PLAN §11).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(pub String);

impl DeviceId {
    pub fn random() -> Self {
        Self(Uuid::now_v7().hyphenated().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for DeviceId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl From<String> for DeviceId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

// ---------------------------------------------------------------------------
// String-backed enums
// ---------------------------------------------------------------------------

/// Error returned when parsing one of the string-backed enums fails.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseEnumError {
    pub ty: &'static str,
    pub value: String,
}

impl fmt::Display for ParseEnumError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown {} value {:?}", self.ty, self.value)
    }
}

impl std::error::Error for ParseEnumError {}

macro_rules! str_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $($(#[$vmeta:meta])* $variant:ident = $text:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        $vis enum $name {
            $($(#[$vmeta])* #[serde(rename = $text)] $variant),+
        }

        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// Stable lowercase name used in the database and in JSON.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }

            pub fn parse(s: &str) -> Option<Self> {
                match s {
                    $($text => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = ParseEnumError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::parse(s).ok_or_else(|| ParseEnumError {
                    ty: stringify!($name),
                    value: s.to_owned(),
                })
            }
        }
    };
}

str_enum!(
    pub enum ReleaseType {
        Album = "album",
        Single = "single",
        Ep = "ep",
        Compilation = "compilation",
        Other = "other",
    }
);

#[allow(clippy::derivable_impls)]
impl Default for ReleaseType {
    fn default() -> Self {
        Self::Album
    }
}

str_enum!(
    pub enum SourceKind {
        LocalFile = "local_file",
        Spotify = "spotify",
        Youtube = "youtube",
        /// A Deezer catalogue entry: it has no sound of its own, its track
        /// plays from YouTube Music (docs/stage3.md §0 «Каталог Deezer»).
        Deezer = "deezer",
        /// A SoundCloud track (AAC in HLS; DRM-only tracks play elsewhere).
        Soundcloud = "soundcloud",
    }
);

impl SourceKind {
    /// Lower is better: local file > Spotify > YouTube > SoundCloud >
    /// Deezer (PLAN §4; a Deezer entry never plays itself).
    pub const fn priority(self) -> u8 {
        match self {
            Self::LocalFile => 0,
            Self::Spotify => 1,
            Self::Youtube => 2,
            Self::Soundcloud => 3,
            Self::Deezer => 4,
        }
    }

    pub const fn is_local(self) -> bool {
        matches!(self, Self::LocalFile)
    }
}

str_enum!(
    pub enum EventType {
        Start = "start",
        Skip = "skip",
        Complete = "complete",
        Seek = "seek",
        Like = "like",
        Unlike = "unlike",
        AddToPlaylist = "add_to_playlist",
    }
);

str_enum!(
    /// Where playback was started from.
    pub enum EventContext {
        HomeSection = "home_section",
        Playlist = "playlist",
        Album = "album",
        Artist = "artist",
        Radio = "radio",
        Search = "search",
        Library = "library",
        Queue = "queue",
        Other = "other",
    }
);

str_enum!(
    /// Who chose what a start plays (recs-v2 §4.1): the user, or the app
    /// going on by itself.
    pub enum EventOrigin {
        /// A click on a track, an album, an artist, a search result.
        Manual = "manual",
        /// The next one of the queue (after the one before ended, or «Дальше»).
        Queue = "queue",
        /// «Автовоспроизведение» going on after the queue.
        Autoplay = "autoplay",
        /// A radio's pick.
        Radio = "radio",
        /// A mix's song.
        Mix = "mix",
        /// A play of another service's history brought in (Takeout,
        /// Spotify): long-term taste, never a session of this app.
        Import = "import",
    }
);

impl EventOrigin {
    /// Not the user's own choice: a recommendation, or the queue going on.
    pub const fn is_auto(self) -> bool {
        !matches!(self, Self::Manual)
    }
}

str_enum!(
    /// Why a play ended (recs-v2 §15, phase 2).
    pub enum EndReason {
        /// «Дальше».
        UserNext = "user_next",
        /// Something else was picked to play.
        UserPick = "user_pick",
        /// The app was closed while it played.
        AppClosed = "app_closed",
        /// It played to its end.
        Ended = "ended",
    }
);

#[allow(clippy::derivable_impls)]
impl Default for EventContext {
    fn default() -> Self {
        Self::Other
    }
}

str_enum!(
    /// Where a wishlist item stands (docs/stage3.md §3.7).
    pub enum WishlistStatus {
        /// Imported, waits for the user's "download" (bulk buttons).
        Waiting = "waiting",
        /// To be searched for automatically.
        New = "new",
        Searching = "searching",
        /// A doubtful match waits for the user.
        Confirm = "confirm",
        Downloading = "downloading",
        Done = "done",
        /// Nothing found; searched again later (`next_try_at`).
        NoTorrent = "no_torrent",
        Failed = "failed",
        /// The user said no: never offered again.
        Dismissed = "dismissed",
    }
);

str_enum!(
    /// What a wishlist item wants downloaded.
    pub enum WishlistScope {
        Track = "track",
        Album = "album",
    }
);

#[allow(clippy::derivable_impls)]
impl Default for WishlistScope {
    fn default() -> Self {
        Self::Track
    }
}

str_enum!(
    /// Why a wishlist item exists.
    pub enum WishlistOrigin {
        /// The user liked a stream track or album.
        Like = "like",
        /// Brought in by an import of likes / playlists.
        Import = "import",
        /// A like or playlist that arrived from a service sync.
        Sync = "sync",
        Manual = "manual",
        /// A library file below the quality the user wants: searched for a
        /// better copy on a schedule, which replaces it.
        Upgrade = "upgrade",
    }
);

#[allow(clippy::derivable_impls)]
impl Default for WishlistOrigin {
    fn default() -> Self {
        Self::Manual
    }
}

str_enum!(
    pub enum DownloadStatus {
        Queued = "queued",
        Downloading = "downloading",
        Seeding = "seeding",
        Done = "done",
        Paused = "paused",
        Failed = "failed",
    }
);

str_enum!(
    pub enum SeedPolicy {
        /// Use the global limits from settings.
        Standard = "standard",
        /// Do not upload after completion.
        NoSeed = "no_seed",
        /// Keep seeding until removed by hand.
        Keep = "keep",
    }
);

str_enum!(
    pub enum NodeKind {
        Pc = "pc",
        Pi = "pi",
        Server = "server",
        Phone = "phone",
    }
);

// ---------------------------------------------------------------------------
// Codec and quality
// ---------------------------------------------------------------------------

/// Audio codec of a source. Stored and serialised as a lowercase string.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum Codec {
    Flac,
    Opus,
    Mp3,
    Aac,
    Vorbis,
    Alac,
    Wav,
    Other(String),
}

impl Codec {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Flac => "flac",
            Self::Opus => "opus",
            Self::Mp3 => "mp3",
            Self::Aac => "aac",
            Self::Vorbis => "vorbis",
            Self::Alac => "alac",
            Self::Wav => "wav",
            Self::Other(s) => s,
        }
    }

    pub fn parse(s: &str) -> Self {
        let lower = s.trim().to_lowercase();
        match lower.as_str() {
            "flac" => Self::Flac,
            "opus" => Self::Opus,
            "mp3" | "mpeg" => Self::Mp3,
            "aac" => Self::Aac,
            "vorbis" => Self::Vorbis,
            "alac" => Self::Alac,
            "wav" | "wave" | "pcm" => Self::Wav,
            _ => Self::Other(lower),
        }
    }

    pub fn is_lossless(&self) -> bool {
        matches!(self, Self::Flac | Self::Alac | Self::Wav)
    }
}

impl From<String> for Codec {
    fn from(s: String) -> Self {
        Self::parse(&s)
    }
}

impl From<Codec> for String {
    fn from(c: Codec) -> Self {
        match c {
            Codec::Other(s) => s,
            known => known.as_str().to_owned(),
        }
    }
}

impl fmt::Display for Codec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Scores are on a 0..=1000 scale; only the ordering matters.
pub mod score {
    pub const HI_RES_LOSSLESS: i32 = 1000;
    pub const CD_LOSSLESS: i32 = 900;
    /// Opus at our working bitrate (~160 kbps VBR) and above.
    pub const OPUS_160: i32 = 800;
    pub const AAC_256: i32 = 780;
    pub const MP3_320: i32 = 775;
    pub const MP3_V0: i32 = 700;
    /// A "lossless" file whose spectrum is cut like a lossy one. Below MP3 V0.
    pub const FAKE_LOSSLESS: i32 = 690;
    pub const OTHER: i32 = 300;
    /// Audio saved from a stream as it is ("Сохранить с YouTube"): a local
    /// file, but marked low quality, so any real download upgrades it.
    pub const SAVED_STREAM: i32 = 295;
    /// A Spotify stream (Vorbis 320). Below every local file: a downloaded
    /// copy always wins, whatever its codec.
    pub const STREAM_SPOTIFY: i32 = 290;
    /// A YouTube stream (Opus ~160 or AAC), below Spotify.
    pub const STREAM_YOUTUBE: i32 = 280;
    /// A YouTube music video (a clip: its own intro and outro, not the
    /// record), one below [`STREAM_YOUTUBE`] so the song of the same track
    /// always plays first. A library track whose active source has this
    /// score is shown as a clip.
    pub const STREAM_YOUTUBE_CLIP: i32 = 279;
    /// A Deezer catalogue entry: no sound of its own (it plays from YouTube
    /// Music), so below every stream that plays.
    pub const STREAM_DEEZER: i32 = 270;
    /// A SoundCloud stream (AAC 160 in HLS): below YouTube's song and clip,
    /// above the Deezer catalogue.
    pub const STREAM_SOUNDCLOUD: i32 = 275;
}

/// Ranks a source by quality, PLAN §6:
/// Hi-Res FLAC > CD FLAC > Opus >= AAC 256 ~ MP3 320 > MP3 V0 > other.
///
/// * lossless is Hi-Res when `bit_depth > 16` or `sample_rate > 48000`;
/// * `fake_lossless` (spectrum cut-off detected) scores a lossless file as a
///   mediocre lossy one, below MP3 V0;
/// * lossy codecs are scored by bitrate (kbps); unknown bitrate gets a
///   conservative value.
pub fn quality_score(
    codec: &Codec,
    bitrate_kbps: Option<u32>,
    sample_rate: Option<u32>,
    bit_depth: Option<u8>,
    fake_lossless: bool,
) -> i32 {
    if codec.is_lossless() {
        if fake_lossless {
            return score::FAKE_LOSSLESS;
        }
        let hi_res = bit_depth.is_some_and(|b| b > 16) || sample_rate.is_some_and(|s| s > 48_000);
        return if hi_res {
            score::HI_RES_LOSSLESS
        } else {
            score::CD_LOSSLESS
        };
    }

    let kbps = bitrate_kbps;
    let at_least = |n: u32| kbps.is_some_and(|b| b >= n);
    match codec {
        Codec::Opus => match kbps {
            None => 780,
            Some(_) if at_least(192) => 820,
            Some(_) if at_least(160) => score::OPUS_160,
            Some(_) if at_least(128) => 760,
            Some(_) if at_least(96) => 700,
            Some(_) => 600,
        },
        Codec::Aac => match kbps {
            None => 600,
            Some(_) if at_least(256) => score::AAC_256,
            Some(_) if at_least(192) => 700,
            Some(_) if at_least(128) => 600,
            Some(_) => 450,
        },
        Codec::Mp3 => match kbps {
            None => 600,
            Some(_) if at_least(320) => score::MP3_320,
            // V0 averages 220-260 kbps.
            Some(_) if at_least(225) => score::MP3_V0,
            Some(_) if at_least(190) => 620,
            Some(_) if at_least(128) => 500,
            Some(_) => 350,
        },
        Codec::Vorbis => match kbps {
            None => 560,
            Some(_) if at_least(256) => 760,
            Some(_) if at_least(192) => 690,
            Some(_) if at_least(128) => 580,
            Some(_) => 430,
        },
        _ => score::OTHER,
    }
}

// ---------------------------------------------------------------------------
// Catalog
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artist {
    pub id: ArtistId,
    pub mbid: Option<String>,
    pub name: String,
    /// Sort form ("Beatles, The"); defaults to `name`.
    pub sort_name: String,
    /// Image references (paths or urls), best first.
    #[serde(default)]
    pub images: Vec<String>,
    /// Unix ms; 0 = "set on insert".
    #[serde(default)]
    pub added_at: UnixMs,
}

impl Artist {
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            id: ArtistId::new(),
            mbid: None,
            sort_name: name.clone(),
            name,
            images: Vec::new(),
            added_at: 0,
        }
    }
}

/// One credited artist plus the text that follows it (MusicBrainz style).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistCreditPart {
    pub artist_id: Option<ArtistId>,
    /// The name as credited (may differ from the artist's canonical name).
    pub name: String,
    /// Text placed between this part and the next one: `" feat. "`, `" & "`.
    #[serde(default)]
    pub join_phrase: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ArtistCredit(pub Vec<ArtistCreditPart>);

impl ArtistCredit {
    pub fn single(name: impl Into<String>, artist_id: Option<ArtistId>) -> Self {
        Self(vec![ArtistCreditPart {
            artist_id,
            name: name.into(),
            join_phrase: String::new(),
        }])
    }

    /// Appends a part; `join_before` is the text between the previous part and
    /// this one (`" feat. "`), stored as the previous part's join phrase.
    pub fn push(
        &mut self,
        join_before: &str,
        name: impl Into<String>,
        artist_id: Option<ArtistId>,
    ) {
        if let Some(last) = self.0.last_mut() {
            last.join_phrase = join_before.to_owned();
        }
        self.0.push(ArtistCreditPart {
            artist_id,
            name: name.into(),
            join_phrase: String::new(),
        });
    }

    /// `"A feat. B"`: names glued with their join phrases. A missing phrase
    /// between two parts becomes `", "`; the last part's phrase is the
    /// line's tail (the bracket closing `リコ(CV:富田美憂)`).
    pub fn display(&self) -> String {
        let mut out = String::new();
        let last = self.0.len().saturating_sub(1);
        for (i, part) in self.0.iter().enumerate() {
            out.push_str(&part.name);
            if i < last {
                if part.join_phrase.is_empty() {
                    out.push_str(", ");
                } else {
                    out.push_str(&part.join_phrase);
                }
            } else if part.join_phrase.chars().all(|c| matches!(c, ')' | '）' | ' ')) {
                out.push_str(part.join_phrase.trim_end());
            }
        }
        out
    }

    /// First (main) credited artist.
    pub fn primary(&self) -> Option<&ArtistCreditPart> {
        self.0.first()
    }

    pub fn artist_ids(&self) -> impl Iterator<Item = ArtistId> + '_ {
        self.0.iter().filter_map(|p| p.artist_id)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub id: ReleaseId,
    pub mbid: Option<String>,
    pub release_group_mbid: Option<String>,
    pub title: String,
    pub artist_credit: ArtistCredit,
    pub year: Option<i32>,
    #[serde(default)]
    pub release_type: ReleaseType,
    /// Cover reference (path relative to the library or url). The palette is
    /// stored separately, keyed by release id.
    pub cover_ref: Option<String>,
    pub total_tracks: Option<u32>,
    #[serde(default)]
    pub added_at: UnixMs,
}

impl Release {
    pub fn new(title: impl Into<String>, artist_credit: ArtistCredit) -> Self {
        Self {
            id: ReleaseId::new(),
            mbid: None,
            release_group_mbid: None,
            title: title.into(),
            artist_credit,
            year: None,
            release_type: ReleaseType::Album,
            cover_ref: None,
            total_tracks: None,
            added_at: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub recording_mbid: Option<String>,
    pub isrc: Option<String>,
    pub acoustid: Option<String>,
    pub title: String,
    pub artist_credit: ArtistCredit,
    pub release_id: Option<ReleaseId>,
    pub disc_no: u32,
    pub track_no: u32,
    pub duration_ms: Option<u32>,
    #[serde(default)]
    pub added_at: UnixMs,
}

impl Track {
    pub fn new(title: impl Into<String>, artist_credit: ArtistCredit) -> Self {
        Self {
            id: TrackId::new(),
            recording_mbid: None,
            isrc: None,
            acoustid: None,
            title: title.into(),
            artist_credit,
            release_id: None,
            disc_no: 1,
            track_no: 0,
            duration_ms: None,
            added_at: 0,
        }
    }
}

/// Where a source came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SourceOrigin {
    Torrent {
        info_hash: String,
        path_in_torrent: String,
    },
    Inbox,
    Stream,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub id: SourceId,
    pub track_id: TrackId,
    pub kind: SourceKind,
    /// Path (local file), uri (Spotify) or video id (YouTube).
    pub locator: String,
    pub codec: Codec,
    /// kbps
    pub bitrate: Option<u32>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub quality_score: i32,
    /// Exactly one non-trashed source per track is active; maintained by the
    /// library, callers just read it.
    pub is_active: bool,
    pub file_hash: Option<String>,
    pub size_bytes: Option<u64>,
    pub origin: SourceOrigin,
    /// `false` = known to be unplayable right now (stream removed, file missing).
    pub available: bool,
    #[serde(default)]
    pub added_at: UnixMs,
    /// Set while the source sits in Trash.
    pub trashed_at: Option<UnixMs>,
}

impl Source {
    pub fn new(
        track_id: TrackId,
        kind: SourceKind,
        locator: impl Into<String>,
        codec: Codec,
    ) -> Self {
        let origin = match kind {
            SourceKind::LocalFile => SourceOrigin::Inbox,
            _ => SourceOrigin::Stream,
        };
        Self {
            id: SourceId::new(),
            track_id,
            kind,
            locator: locator.into(),
            quality_score: quality_score(&codec, None, None, None, false),
            codec,
            bitrate: None,
            sample_rate: None,
            bit_depth: None,
            is_active: false,
            file_hash: None,
            size_bytes: None,
            origin,
            available: true,
            added_at: 0,
            trashed_at: None,
        }
    }

    /// Recomputes `quality_score` from the technical fields.
    pub fn with_quality(mut self, fake_lossless: bool) -> Self {
        self.quality_score = quality_score(
            &self.codec,
            self.bitrate,
            self.sample_rate,
            self.bit_depth,
            fake_lossless,
        );
        self
    }
}

/// Lossless master kept in backup (PLAN §4, §11).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Master {
    pub track_id: TrackId,
    pub codec: Codec,
    pub file_hash: String,
    pub size_bytes: u64,
    pub origin: Option<SourceOrigin>,
    #[serde(default)]
    pub replicas: Vec<Replica>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Replica {
    pub file_hash: String,
    pub node_id: DeviceId,
    pub verified_at: Option<UnixMs>,
}

// ---------------------------------------------------------------------------
// User data (CRDT-friendly)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Playlist {
    pub id: PlaylistId,
    pub name: String,
    pub cover: Option<String>,
    pub created_at: UnixMs,
    pub updated_at: UnixMs,
    pub updated_by: DeviceId,
    /// Tombstone.
    pub deleted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaylistEntry {
    pub id: EntryId,
    pub playlist_id: PlaylistId,
    pub track_id: TrackId,
    /// Fractional-index key (base-62); entries are ordered by `(position, id)`.
    pub position: String,
    pub added_at: UnixMs,
    pub updated_at: UnixMs,
    pub updated_by: DeviceId,
    /// Tombstone.
    pub deleted: bool,
}

/// What a like points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum LikeTarget {
    Track(TrackId),
    Release(ReleaseId),
    Artist(ArtistId),
}

impl LikeTarget {
    pub const fn kind_str(&self) -> &'static str {
        match self {
            Self::Track(_) => "track",
            Self::Release(_) => "release",
            Self::Artist(_) => "artist",
        }
    }

    pub fn id_string(&self) -> String {
        match self {
            Self::Track(id) => id.to_string(),
            Self::Release(id) => id.to_string(),
            Self::Artist(id) => id.to_string(),
        }
    }

    pub fn parse(kind: &str, id: &str) -> Option<Self> {
        match kind {
            "track" => id.parse().ok().map(Self::Track),
            "release" => id.parse().ok().map(Self::Release),
            "artist" => id.parse().ok().map(Self::Artist),
            _ => None,
        }
    }
}

impl From<TrackId> for LikeTarget {
    fn from(id: TrackId) -> Self {
        Self::Track(id)
    }
}

impl From<ReleaseId> for LikeTarget {
    fn from(id: ReleaseId) -> Self {
        Self::Release(id)
    }
}

impl From<ArtistId> for LikeTarget {
    fn from(id: ArtistId) -> Self {
        Self::Artist(id)
    }
}

/// LWW-element-set member: the like is live while `deleted == false`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Like {
    #[serde(flatten)]
    pub target: LikeTarget,
    pub updated_at: UnixMs,
    pub updated_by: DeviceId,
    pub deleted: bool,
}

/// Append-only listening history record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub id: EventId,
    pub ts: UnixMs,
    pub track_id: TrackId,
    pub source_kind: Option<SourceKind>,
    #[serde(rename = "type")]
    pub event_type: EventType,
    pub position_ms: Option<u32>,
    #[serde(default)]
    pub context: EventContext,
    pub device: DeviceId,
    /// Who chose it (a start); `None` in a history older than the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<EventOrigin>,
    /// What played it: a release, a playlist, a mix, a radio's seed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    /// Why it ended (a skip or a complete).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_reason: Option<EndReason>,
    /// How far the app was from the user while it played (an end): 0 the
    /// window focused, 0.5 in view behind another, 1 hidden or minimised,
    /// averaged over the time it played.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention: Option<f32>,
    /// The player's mean volume while it played (an end), 0…1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<f32>,
}

/// What a recorded play says beyond its kind and position.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EventMeta {
    pub origin: Option<EventOrigin>,
    pub context_id: Option<String>,
    pub end_reason: Option<EndReason>,
    pub attention: Option<f32>,
    pub volume: Option<f32>,
}

impl Event {
    pub fn new(track_id: TrackId, event_type: EventType, ts: UnixMs, device: DeviceId) -> Self {
        Self {
            id: EventId::new(),
            ts,
            track_id,
            source_kind: None,
            event_type,
            position_ms: None,
            context: EventContext::Other,
            device,
            origin: None,
            context_id: None,
            end_reason: None,
            attention: None,
            volume: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Derived data and tails
// ---------------------------------------------------------------------------

str_enum!(
    /// What a track's sound analysis was computed from (docs/stage4.md §2).
    /// An analysis of the file always wins over one of a stream.
    pub enum AnalysisSource {
        /// The track's local file.
        File = "file",
        /// The track's song on YouTube (Music), for a track without a file.
        Youtube = "youtube",
    }
);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Analysis {
    pub track_id: TrackId,
    pub bliss_vector: Vec<f32>,
    pub analyzed_with: String,
    #[serde(default = "analysis_from_file")]
    pub source: AnalysisSource,
}

fn analysis_from_file() -> AnalysisSource {
    AnalysisSource::File
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Palette {
    pub release_id: ReleaseId,
    pub seed_argb: u32,
    /// Cached light/dark scheme (opaque to the library).
    pub scheme_blob: Option<Vec<u8>>,
    /// Tiny blurred cover texture for the ambient glow.
    pub ambient_blur: Option<Vec<u8>>,
}

impl Palette {
    pub fn new(release_id: ReleaseId, seed_argb: u32) -> Self {
        Self {
            release_id,
            seed_argb,
            scheme_blob: None,
            ambient_blur: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WishlistItem {
    pub id: WishlistId,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub recording_mbid: Option<String>,
    pub isrc: Option<String>,
    pub spotify_id: Option<String>,
    pub youtube_id: Option<String>,
    pub status: WishlistStatus,
    /// Candidate list as JSON text (shape owned by the torrent layer).
    pub candidates_json: Option<String>,
    pub created_at: UnixMs,
    pub updated_at: UnixMs,
    /// The library track it is for (a stream track until the file arrives).
    #[serde(default)]
    pub track_id: Option<TrackId>,
    /// The album, for [`WishlistScope::Album`] (or the track's album).
    #[serde(default)]
    pub release_id: Option<ReleaseId>,
    #[serde(default)]
    pub scope: WishlistScope,
    #[serde(default)]
    pub origin: WishlistOrigin,
    /// Searches done so far.
    #[serde(default)]
    pub attempts: u32,
    /// Not searched again before this time (unix ms).
    #[serde(default)]
    pub next_try_at: Option<UnixMs>,
    /// Free text for the user (why it failed, what was chosen).
    #[serde(default)]
    pub note: Option<String>,
}

impl WishlistItem {
    /// A fresh item in `status`, stamped `now`, with every optional field empty.
    pub fn new(status: WishlistStatus, now: UnixMs) -> Self {
        Self {
            id: WishlistId::new(),
            title: None,
            artist: None,
            recording_mbid: None,
            isrc: None,
            spotify_id: None,
            youtube_id: None,
            status,
            candidates_json: None,
            created_at: now,
            updated_at: now,
            track_id: None,
            release_id: None,
            scope: WishlistScope::Track,
            origin: WishlistOrigin::Manual,
            attempts: 0,
            next_try_at: None,
            note: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Download {
    pub info_hash: String,
    pub magnet: Option<String>,
    pub torrent_file: Option<String>,
    #[serde(default)]
    pub selected_files: Vec<String>,
    pub status: DownloadStatus,
    pub seed_policy: SeedPolicy,
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub wishlist_id: Option<WishlistId>,
    pub created_at: UnixMs,
    pub updated_at: UnixMs,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub id: DeviceId,
    pub name: String,
    pub kind: NodeKind,
    pub last_seen: Option<UnixMs>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_roundtrip_and_are_time_ordered() {
        let a = TrackId::new();
        let b = TrackId::new();
        assert_ne!(a, b);
        assert_eq!(a.to_string().parse::<TrackId>().unwrap(), a);
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(json, format!("\"{a}\""));
        assert_eq!(serde_json::from_str::<TrackId>(&json).unwrap(), a);
        assert!(a < b, "v7 ids created in sequence sort in order");
    }

    #[test]
    fn credit_display() {
        let mut c = ArtistCredit::single("A", None);
        assert_eq!(c.display(), "A");
        c.push(" feat. ", "B", None);
        assert_eq!(c.display(), "A feat. B");
        c.push(" & ", "C", None);
        assert_eq!(c.display(), "A feat. B & C");

        let mut c = ArtistCredit::single("X", None);
        c.push("", "Y", None);
        assert_eq!(c.display(), "X, Y");
        assert_eq!(ArtistCredit::default().display(), "");
    }

    #[test]
    fn enums_roundtrip() {
        for k in SourceKind::ALL {
            assert_eq!(SourceKind::parse(k.as_str()), Some(*k));
            let json = serde_json::to_string(k).unwrap();
            assert_eq!(json, format!("\"{}\"", k.as_str()));
        }
        assert_eq!("ep".parse::<ReleaseType>().unwrap(), ReleaseType::Ep);
        assert!("nope".parse::<ReleaseType>().is_err());
        assert!(SourceKind::LocalFile.priority() < SourceKind::Spotify.priority());
        assert!(SourceKind::Spotify.priority() < SourceKind::Youtube.priority());
        assert!(SourceKind::Youtube.priority() < SourceKind::Deezer.priority());
    }

    #[test]
    fn codec_string_form() {
        assert_eq!(Codec::parse("FLAC"), Codec::Flac);
        assert_eq!(Codec::parse("ape"), Codec::Other("ape".into()));
        let json = serde_json::to_string(&Codec::Other("ape".into())).unwrap();
        assert_eq!(json, "\"ape\"");
        assert_eq!(
            serde_json::from_str::<Codec>("\"opus\"").unwrap(),
            Codec::Opus
        );
    }

    #[test]
    fn like_target_json_and_parse() {
        let t = TrackId::new();
        let like = Like {
            target: LikeTarget::Track(t),
            updated_at: 5,
            updated_by: "dev".into(),
            deleted: false,
        };
        let json = serde_json::to_string(&like).unwrap();
        assert!(json.contains("\"kind\":\"track\""), "{json}");
        assert_eq!(serde_json::from_str::<Like>(&json).unwrap(), like);
        assert_eq!(
            LikeTarget::parse("track", &t.to_string()),
            Some(LikeTarget::Track(t))
        );
        assert_eq!(LikeTarget::parse("album", "x"), None);
    }

    #[test]
    fn quality_ordering_follows_plan() {
        let q = |c: Codec, br, sr, bd, fake| quality_score(&c, br, sr, bd, fake);
        let hires = q(Codec::Flac, None, Some(96_000), Some(24), false);
        let cd = q(Codec::Flac, None, Some(44_100), Some(16), false);
        let opus = q(Codec::Opus, Some(160), None, None, false);
        let aac256 = q(Codec::Aac, Some(256), None, None, false);
        let mp3_320 = q(Codec::Mp3, Some(320), None, None, false);
        let v0 = q(Codec::Mp3, Some(245), None, None, false);
        let mp3_128 = q(Codec::Mp3, Some(128), None, None, false);
        let fake = q(Codec::Flac, None, Some(44_100), Some(16), true);
        let other = q(Codec::Other("ape".into()), None, None, None, false);

        assert!(hires > cd);
        assert!(cd > opus);
        assert!(opus >= aac256);
        assert!(
            aac256 >= mp3_320 && aac256 - mp3_320 < 20,
            "approximately equal"
        );
        assert!(mp3_320 > v0);
        assert!(v0 > fake, "fake FLAC is below MP3 V0");
        assert!(fake > mp3_128);
        assert!(mp3_128 > other);
        // Streams rank below every local file, YouTube below Spotify.
        assert!(score::STREAM_SPOTIFY < other.min(q(Codec::Mp3, Some(64), None, None, false)));
        const { assert!(score::STREAM_YOUTUBE < score::STREAM_SPOTIFY) };
        // Opus / AAC / MP3 rank by bitrate too.
        assert!(q(Codec::Opus, Some(192), None, None, false) > opus);
        assert!(opus > q(Codec::Opus, Some(96), None, None, false));
        // 16-bit at 96 kHz still counts as Hi-Res.
        assert_eq!(q(Codec::Alac, None, Some(96_000), Some(16), false), hires);
    }

    #[test]
    fn source_new_scores_by_codec() {
        let s = Source::new(TrackId::new(), SourceKind::LocalFile, "a.flac", Codec::Flac);
        assert_eq!(s.quality_score, score::CD_LOSSLESS);
        let s = Source {
            sample_rate: Some(96_000),
            ..s
        }
        .with_quality(false);
        assert_eq!(s.quality_score, score::HI_RES_LOSSLESS);
        let s = s.with_quality(true);
        assert_eq!(s.quality_score, score::FAKE_LOSSLESS);
    }
}
