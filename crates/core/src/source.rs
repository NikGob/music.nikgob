//! Download sources: torrents through an indexer,
//! khinsider, free FLAC services, Soulseek later. Each is a [`Source`]:
//! `search` for a track or an album gives [`Offer`]s (format, quality,
//! size, source, availability), `fetch` brings one into the downloads
//! folder. The wishlist asks every enabled source and takes the best offer
//! by one quality policy ([`Offer::quality`]); the rest of the app does not
//! know the concrete sites.
//!
//! A torrent is fetched as its payload (the torrent client downloads the
//! files it picks); every other source writes the files itself.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::indexer::{BoxFuture, TorrentPayload};
use crate::model::{Codec, quality_score, score};

/// Which kind of source an offer comes from: the key of the source's line
/// on «Источники» (its order and switch) and of its settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKey {
    /// Torrents found through the indexer (Prowlarr).
    Torrents,
    /// downloads.khinsider.com: game soundtracks.
    Khinsider,
    /// The Soulseek network (crates/soulseek).
    Soulseek,
    /// Qobuz through a public Qobuz-DL instance.
    Qobuz,
    /// Tidal through a public hifi-api instance.
    Tidal,
    /// squidify.org (or any Subsonic server): anime, game and film
    /// soundtracks in FLAC.
    Squidify,
    /// The Internet Archive (archive.org): uploads of CD rips, rare and
    /// old albums, often in FLAC.
    Archive,
    /// Bandcamp's free and name-your-price releases.
    Bandcamp,
    /// The original files SoundCloud uploaders let download (needs the
    /// user's SoundCloud sign-in).
    Soundcloud,
    /// play.dogmazic.net: an archive of free (Creative Commons) music.
    Dogmazic,
}

impl SourceKey {
    /// Every key, in the default order of «Источники»: the cheap and
    /// reliable first, the free services last.
    pub const ALL: [Self; 10] = [
        Self::Torrents,
        Self::Khinsider,
        Self::Bandcamp,
        Self::Soulseek,
        Self::Qobuz,
        Self::Tidal,
        Self::Squidify,
        Self::Archive,
        Self::Soundcloud,
        Self::Dogmazic,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Torrents => "torrents",
            Self::Khinsider => "khinsider",
            Self::Soulseek => "soulseek",
            Self::Qobuz => "qobuz",
            Self::Tidal => "tidal",
            Self::Squidify => "squidify",
            Self::Archive => "archive",
            Self::Bandcamp => "bandcamp",
            Self::Soundcloud => "soundcloud",
            Self::Dogmazic => "dogmazic",
        }
    }

    /// The name the UI shows.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Torrents => "Торренты",
            Self::Khinsider => "khinsider",
            Self::Soulseek => "Soulseek",
            Self::Qobuz => "Qobuz",
            Self::Tidal => "Tidal",
            Self::Squidify => "Squidify",
            Self::Archive => "Internet Archive",
            Self::Bandcamp => "Bandcamp",
            Self::Soundcloud => "SoundCloud",
            Self::Dogmazic => "Dogmazic",
        }
    }

    /// A free service of someone else's account (a public instance): the
    /// group that needs no account of the user's own.
    pub fn is_service(self) -> bool {
        matches!(self, Self::Qobuz | Self::Tidal)
    }
}

/// Track or album.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WantScope {
    #[default]
    Track,
    Album,
}

/// What a wishlist item asks a source for. The item is the reference: an
/// offer of another artist is never taken.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Want {
    /// The credited artists, the main one first (no "feat." tails, no
    /// "and 1 more").
    pub artists: Vec<String>,
    /// Other spellings of the main artist (aliases: ZUTOMAYO and
    /// ずっと真夜中でいいのに。).
    #[serde(default)]
    pub aliases: Vec<String>,
    /// The track's title (for an album item, `None`).
    pub title: Option<String>,
    /// The album the track is on, or the album asked for.
    pub album: Option<String>,
    pub scope: WantScope,
    /// 0 = unknown.
    pub track_no: u32,
    pub disc_no: u32,
    pub duration_ms: Option<u32>,
    pub isrc: Option<String>,
    /// Tracks the album has (0 = unknown).
    pub album_tracks: u32,
    /// Titles of the album's tracks that have a file already: a fetch of
    /// the album leaves them out (the rest of a partly downloaded album).
    #[serde(default)]
    pub got: Vec<String>,
}

impl Want {
    /// The main artist ("" when unknown).
    pub fn artist(&self) -> &str {
        self.artists.first().map_or("", String::as_str)
    }

    /// Every name the main artist goes by.
    pub fn artist_names(&self) -> impl Iterator<Item = &str> {
        self.artists.iter().take(1).chain(self.aliases.iter()).map(String::as_str)
    }

    /// An album item, or a track that is its album's only track (a single).
    pub fn is_single(&self) -> bool {
        self.scope == WantScope::Track
            && match (&self.title, &self.album) {
                (Some(t), Some(a)) => t.trim().eq_ignore_ascii_case(a.trim()),
                _ => false,
            }
    }
}

/// What an offer holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OfferScope {
    /// One track.
    #[default]
    Track,
    /// One album (a torrent of it, an album page).
    Album,
    /// Several albums (a discography, a collection): the track or album is
    /// picked out of it.
    Collection,
}

/// The audio an offer promises (from the source's own words; the import
/// checks the file).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Format {
    pub codec: Codec,
    pub bitrate_kbps: Option<u32>,
    pub bit_depth: Option<u8>,
    pub sample_rate: Option<u32>,
}

impl Format {
    pub fn new(codec: Codec) -> Self {
        Self {
            codec,
            bitrate_kbps: None,
            bit_depth: None,
            sample_rate: None,
        }
    }

    pub fn lossless(&self) -> bool {
        self.codec.is_lossless()
    }

    /// "FLAC 24/96", "MP3 320", "FLAC".
    pub fn label(&self) -> String {
        let mut out = match &self.codec {
            Codec::Other(s) if s.is_empty() => "?".to_owned(),
            c => c.as_str().to_uppercase(),
        };
        if self.lossless() {
            if let (Some(b), Some(r)) = (self.bit_depth, self.sample_rate)
                && (b > 16 || r > 48_000)
            {
                let khz = r as f64 / 1000.0;
                let khz = if khz.fract() == 0.0 { format!("{khz:.0}") } else { format!("{khz:.1}") };
                out.push_str(&format!(" {b}/{khz}"));
            }
        } else if let Some(k) = self.bitrate_kbps {
            out.push_str(&format!(" {k}"));
        }
        out
    }
}

/// One way to get the track or album from a source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    pub source: SourceKey,
    /// Who has it, for display: the tracker of a torrent, the service.
    pub provider: String,
    /// Unique within the source.
    pub id: String,
    /// How the source titles it (a torrent's name, "Artist — Title").
    pub title: String,
    /// What the source says it is, when it says it apart from the title.
    #[serde(default)]
    pub artist: Option<String>,
    #[serde(default)]
    pub track_title: Option<String>,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<u32>,
    #[serde(default)]
    pub isrc: Option<String>,
    pub scope: OfferScope,
    /// `None`: the source does not say (a torrent without a format in its name).
    pub format: Option<Format>,
    pub size_bytes: Option<u64>,
    /// Seeders of a torrent, free slots of a peer; `None` for a site that
    /// is just there.
    pub seeders: Option<u32>,
    pub info_url: Option<String>,
    /// What the source needs to fetch it later; only it reads this.
    pub handle: String,
}

impl Offer {
    /// The quality policy: Hi-Res FLAC > CD FLAC > Opus ≥ AAC 256
    /// ≈ MP3 320 > MP3 V0 > other; a format nobody named counts as a
    /// mediocre lossy file. A YouTube copy is below all of them
    /// ([`score::SAVED_STREAM`]).
    pub fn quality(&self) -> i32 {
        match &self.format {
            Some(f) => quality_score(&f.codec, f.bitrate_kbps, f.sample_rate, f.bit_depth, false),
            None => score::MP3_V0 - 100,
        }
    }

    pub fn lossless(&self) -> bool {
        self.format.as_ref().is_some_and(Format::lossless)
    }

    /// "FLAC 24/96 · 312 МБ · 14 сид. · RuTracker".
    pub fn detail(&self) -> String {
        let mut parts = Vec::new();
        if let Some(f) = &self.format {
            parts.push(f.label());
        }
        if let Some(b) = self.size_bytes {
            parts.push(human_bytes(b));
        }
        if let Some(s) = self.seeders {
            parts.push(format!("{s} сид."));
        }
        let provider = if self.provider.is_empty() {
            self.source.display_name().to_owned()
        } else {
            self.provider.clone()
        };
        parts.push(provider);
        parts.join(" · ")
    }
}

/// "312 МБ", "1.4 ГБ".
pub fn human_bytes(b: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = b as f64 / MB;
    if mb >= 1024.0 {
        format!("{:.1} ГБ", mb / 1024.0)
    } else if mb >= 10.0 {
        format!("{mb:.0} МБ")
    } else {
        format!("{mb:.1} МБ")
    }
}

/// What a fetch brought.
#[derive(Debug)]
pub enum Fetched {
    /// A torrent for the torrent client (it picks the files).
    Torrent(TorrentPayload),
    /// Files written into the folder given to `fetch`.
    Files(Vec<PathBuf>),
    /// Some of an album's files: the rest could not be had (`missing`,
    /// their names) for `error`, the reason of the last one.
    Partial {
        files: Vec<PathBuf>,
        missing: Vec<String>,
        error: SourceError,
    },
}

/// Bytes done and the total when known, as a download goes.
pub type Progress = Arc<dyn Fn(u64, Option<u64>) + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SourceError {
    #[error("source is not configured")]
    NotConfigured,
    #[error("source is unreachable: {0}")]
    Unreachable(String),
    /// A bot check, a ban, every instance refusing: not a miss, later.
    #[error("source turned us away: {0}")]
    Blocked(String),
    #[error("source asked to wait")]
    RateLimited { retry_after: Option<Duration> },
    #[error("source answered with HTTP {status}")]
    Http { status: u16 },
    #[error("unexpected source response: {0}")]
    Parse(String),
    #[error("the offer has nothing to download")]
    NothingToFetch,
    #[error("cannot write the download: {0}")]
    Io(String),
}

impl SourceError {
    /// The source cannot answer now (as opposed to having nothing).
    pub fn is_outage(&self) -> bool {
        matches!(
            self,
            Self::Unreachable(_) | Self::Blocked(_) | Self::RateLimited { .. } | Self::Http { .. }
        )
    }

    /// The source turned us away (a ban, a 401/403, a rate limit, a server
    /// error, no answer), as opposed to not having the thing (a 404, a
    /// file it cannot give): another source may have it, or this one later.
    pub fn is_refusal(&self) -> bool {
        match self {
            Self::Unreachable(_) | Self::Blocked(_) | Self::RateLimited { .. } => true,
            Self::Http { status } => matches!(status, 401 | 403 | 429) || *status >= 500,
            _ => false,
        }
    }

    /// How long the source asked to wait, when it said.
    pub fn wait(&self) -> Option<Duration> {
        match self {
            Self::RateLimited { retry_after } => *retry_after,
            _ => None,
        }
    }
}

impl From<crate::indexer::IndexerError> for SourceError {
    fn from(e: crate::indexer::IndexerError) -> Self {
        use crate::indexer::IndexerError as E;
        match e {
            E::NotConfigured => Self::NotConfigured,
            E::Unauthorized => Self::Blocked("the indexer rejected the API key".into()),
            E::Unreachable(s) => Self::Unreachable(s),
            E::Http { status } => Self::Http { status },
            E::Parse(s) => Self::Parse(s),
            E::NothingToFetch => Self::NothingToFetch,
        }
    }
}

pub trait Source: Send + Sync {
    fn key(&self) -> SourceKey;

    /// Short name for logs and the UI.
    fn name(&self) -> &str;

    /// Offers for `want`, in no particular order; the caller matches and
    /// ranks them. Empty = the source does not have it.
    fn search<'a>(&'a self, want: &'a Want) -> BoxFuture<'a, Result<Vec<Offer>, SourceError>>;

    /// Brings `offer` (one of this source's) for `want` into `dest` (an
    /// empty folder of its own), or hands back the torrent.
    fn fetch<'a>(
        &'a self,
        offer: &'a Offer,
        want: &'a Want,
        dest: &'a Path,
        progress: Progress,
    ) -> BoxFuture<'a, Result<Fetched, SourceError>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(format: Option<Format>) -> Offer {
        Offer {
            source: SourceKey::Qobuz,
            provider: String::new(),
            id: "1".into(),
            title: "t".into(),
            artist: None,
            track_title: None,
            album: None,
            duration_ms: None,
            isrc: None,
            scope: OfferScope::Track,
            format,
            size_bytes: Some(30 << 20),
            seeders: None,
            info_url: None,
            handle: String::new(),
        }
    }

    #[test]
    fn the_policy_puts_lossless_first_and_unknown_below_lossy() {
        let hires = Format { bit_depth: Some(24), sample_rate: Some(96_000), ..Format::new(Codec::Flac) };
        let cd = Format::new(Codec::Flac);
        let mp3 = Format { bitrate_kbps: Some(320), ..Format::new(Codec::Mp3) };
        let q = |f: Option<Format>| offer(f).quality();
        assert!(q(Some(hires.clone())) > q(Some(cd.clone())));
        assert!(q(Some(cd)) > q(Some(mp3.clone())));
        assert!(q(Some(mp3)) > q(None));
        assert!(q(None) > score::SAVED_STREAM, "anything real beats the YouTube copy");
        assert_eq!(hires.label(), "FLAC 24/96");
        assert_eq!(Format { sample_rate: Some(44_100), bit_depth: Some(24), ..Format::new(Codec::Flac) }.label(), "FLAC 24/44.1");
    }

    #[test]
    fn offers_survive_json_and_say_what_they_are() {
        let o = offer(Some(Format::new(Codec::Flac)));
        let json = serde_json::to_string(&o).unwrap();
        assert_eq!(serde_json::from_str::<Offer>(&json).unwrap(), o);
        assert_eq!(o.detail(), "FLAC · 30 МБ · Qobuz");
    }

    #[test]
    fn a_single_is_a_track_named_like_its_album() {
        let w = Want {
            title: Some("Underdog".into()),
            album: Some("underdog".into()),
            ..Want::default()
        };
        assert!(w.is_single());
        assert!(!Want { album: Some("Kaizin".into()), ..w }.is_single());
    }

    #[test]
    fn a_refusal_is_told_from_a_miss() {
        for e in [
            SourceError::Http { status: 403 },
            SourceError::Http { status: 401 },
            SourceError::Http { status: 503 },
            SourceError::RateLimited { retry_after: None },
            SourceError::Blocked("cf".into()),
            SourceError::Unreachable("dns".into()),
        ] {
            assert!(e.is_refusal(), "{e:?}");
        }
        for e in [
            SourceError::Http { status: 404 },
            SourceError::NothingToFetch,
            SourceError::Parse("a DASH manifest".into()),
            SourceError::Io("disk".into()),
        ] {
            assert!(!e.is_refusal(), "{e:?}");
        }
        assert_eq!(SourceError::RateLimited { retry_after: Some(Duration::from_secs(9)) }.wait(), Some(Duration::from_secs(9)));
    }

    #[test]
    fn source_trait_is_dyn_compatible() {
        fn takes(_: Option<Arc<dyn Source>>) {}
        takes(None);
    }
}
