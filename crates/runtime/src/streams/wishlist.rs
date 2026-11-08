//! The wishlist pipeline (docs/stage3.md §3.5, §3.7, docs/stage4.5.md): a
//! new like of a stream-only track becomes an item; items are searched in
//! every download source that is on (torrents through the indexer,
//! khinsider, the free FLAC services, Soulseek), the offers that are the
//! wished track or album are ranked by one quality policy (lossless first,
//! then the user's order of the sources; see [`nikgob_sources::select`]),
//! and the best sure one is downloaded: a torrent's file (or the whole
//! album) picked from its list by the torrent client, the files of any
//! other source written into `Downloads/.sources/<item>`. The importer
//! attaches them to the item's track (the job's [`ImportTargets`]). A
//! doubtful offer waits in "Подтверди" (a torrent of a track only once its
//! list shows the track's file: [`Probe`]); nothing anywhere → «не нашёл», a
//! toast, and a search again a week later until the user dismisses it.
//! A source that turns a download away (a 403, «wait», no answer) is not
//! a miss: the next offer, of another source, takes over (a lossy or
//! doubtful one through «Подтверди»); with none left, the item is searched
//! again once a source that said no can be asked, after a wait that grows
//! each time ([`plan_retry`]); a source that keeps refusing is set aside a
//! while ([`Refusals`]); an album that came in part keeps what came, and
//! its rest comes from the next offer or later. «Загрузки → История» has
//! one line per such item, its latest state in plain words (why, when,
//! which sources next), gone once the item's file came.
//! Items from an import wait for the user's "скачать". At most
//! [`nikgob_view::Prefs::auto_download_limit`] items search or download at
//! once.
//!
//! The way out without any source: the audio saved from YouTube Music as
//! it is (a low-quality local copy), or a .torrent, magnet, file or folder
//! of the user's own, linked to the item's track. A copy does not close the
//! item: it keeps looking once a week in every source, and what is better
//! than the copy downloads at once and replaces it. Files of the library
//! below the quality the user wants are looked for again the same way, at
//! a pace of their own ([`upgrade`]).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_core::indexer::{Candidate, TorrentPayload};
use nikgob_core::model::{
    ReleaseId, SourceKind, SourceOrigin, TrackId, WishlistId, WishlistItem, WishlistOrigin,
    WishlistScope, WishlistStatus, score,
};
use nikgob_core::source::{Fetched, Offer, Source, SourceError, SourceKey, Want, WantScope};
use nikgob_core::streaming::{EntityKind, Service, StreamError};
use nikgob_import::ImportTargets;
use nikgob_metadata::text::{artist_key, title_key};
use nikgob_sources::select::{pick, rank};
use nikgob_sources::torrent::{candidate_of, look_first, offer_of, pick_files};
use nikgob_view::{self as v, Command};

use super::Msg;
use crate::Worker;

mod upgrade;

pub(crate) use upgrade::upgrade_job_words;

/// Bit of the "Подтверди" ids of wishlist items (import ids use bit 48,
/// torrent jobs small numbers).
pub(crate) const WISH_ID: u64 = 1 << 47;
/// An item without a torrent is searched again after this.
const RETRY_AFTER: Duration = Duration::from_secs(7 * 24 * 3600);
/// Offers shown in "Подтверди".
const CONFIRM_OPTIONS: usize = 3;
/// The note of an item whose files wait in «Подтверди» (docs/stage4.md §12.4).
pub(crate) const FILES_WAIT: &str = "Файлы ждут подтверждения";
/// The note of an item nothing was found for.
const NOT_FOUND: &str = "Не нашёл, где скачать — поищу снова через неделю";
/// The note of an item that has a copy saved from YouTube.
const HAS_COPY: &str = "Есть копия с YouTube — раз в неделю ищу качество лучше";
/// A source's search at most (the indexer's variants take a minute each).
const SEARCH_TIMEOUT: Duration = Duration::from_secs(300);
/// A download of a source other than torrents at most.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Where the files of sources other than torrents land, under Downloads/.
const SOURCES_DIR: &str = ".sources";
/// An item every offer of which was turned away is searched again after
/// this, doubling each time in a row up to [`LATER_LONGEST`]…
const LATER_FIRST: Duration = Duration::from_secs(5 * 60);
const LATER_LONGEST: Duration = Duration::from_secs(6 * 3600);
/// …this many times; then it waits the week like an item not found.
const LATER_TRIES: u32 = 6;
/// A retry asks the sources that turned the item away, and the others
/// too once this passed since they last answered for it (Soulseek's peers
/// come and go, new torrents come out).
const ASK_AGAIN_AFTER: Duration = Duration::from_secs(6 * 3600);
/// A shorter wait than this a source asks for is not told with its time.
const WAIT_TOLD: Duration = Duration::from_secs(15 * 60);
/// Downloads of a source turned away in a row that set it aside…
const ASIDE_AFTER: u32 = 2;
/// …for this, doubling while it keeps refusing, up to a day.
const ASIDE_FIRST: Duration = Duration::from_secs(30 * 60);
const ASIDE_LONGEST: Duration = Duration::from_secs(24 * 3600);

/// Messages of the pipeline's background steps.
pub(crate) enum WishMsg {
    Searched {
        item: WishlistId,
        result: Result<Vec<Offer>, String>,
        /// The sources that answered, and the ones that could not (why).
        answered: Vec<SourceKey>,
        down: Vec<(SourceKey, SourceError)>,
    },
    Fetched {
        item: WishlistId,
        offer: Box<Offer>,
        result: Result<Fetched, SourceError>,
    },
    Inspected {
        item: WishlistId,
        offer: Box<Offer>,
        payload: TorrentPayload,
        result: Result<nikgob_torrent::TorrentMeta, String>,
    },
    Saved {
        item: WishlistId,
        track: TrackId,
        result: Result<PathBuf, StreamError>,
    },
}

#[derive(Default)]
pub(crate) struct WishState {
    /// Sure offers of items being handled, the one tried first.
    cands: HashMap<WishlistId, Vec<Offer>>,
    /// Doubtful offers of those items: «Подтверди» when the sure ones fail.
    doubtful: HashMap<WishlistId, Vec<Offer>>,
    /// Stable "Подтверди" ids.
    confirm_ids: HashMap<WishlistId, u64>,
    next_confirm: u64,
    /// Audio saved from YouTube that the importer is taking, by file.
    saving: HashMap<PathBuf, WishlistId>,
    /// How much of an item's download is here (0..=100), while it downloads.
    pub(crate) progress: HashMap<WishlistId, u8>,
    /// Folders of the downloads of sources other than torrents, by item;
    /// they go once the import took the files.
    direct: HashMap<WishlistId, PathBuf>,
    /// How far those downloads are, as their tasks write it.
    live: Arc<parking_lot::Mutex<HashMap<WishlistId, u8>>>,
    /// Sources that could not answer their last search, and why.
    pub(crate) down: HashMap<SourceKey, String>,
    /// Sources whose last download failed though they found it, and why.
    pub(crate) fetch_down: HashMap<SourceKey, String>,
    /// Who an item's file comes from, while it downloads.
    from: HashMap<WishlistId, String>,
    /// Sources the user just turned on or set up: once one can search,
    /// what was not found is searched for again.
    pub(crate) enabled: HashSet<SourceKey>,
    /// An uploader's "Performer - Song" fetched from the performer's
    /// release: the artist and title its file takes once it lands.
    relabel: HashMap<WishlistId, (String, String)>,
    /// Doubtful offers of a track being looked into before «Подтверди».
    probe: HashMap<WishlistId, Probe>,
    /// Sources whose downloads keep turning us away.
    pub(crate) refusals: Refusals,
    /// What turned an item away this time (a source down at its search, a
    /// download refused): its retry waits for them, its note names them.
    turned: HashMap<WishlistId, Vec<(SourceKey, SourceError)>>,
    /// Retries in a row of an item that was turned away.
    later: HashMap<WishlistId, u32>,
    /// Albums of which a source gave a part: the rest goes on once the
    /// import took what came.
    partial: HashSet<WishlistId>,
    /// The sources an item searched again after being turned away asks.
    retry_only: HashMap<WishlistId, Vec<SourceKey>>,
    /// When each source last answered a search for an item.
    asked: HashMap<WishlistId, HashMap<SourceKey, Instant>>,
    /// Better copies of the library's files ([`upgrade`]): when the last
    /// searches for them started,
    upgrade_runs: Vec<Instant>,
    /// when the worker first could start one,
    upgrade_since: Option<Instant>,
    /// what an item's file was before the copy on its way,
    upgrade_from: HashMap<WishlistId, String>,
    /// and what its files are now, in words (until a file lands),
    upgrade_have: HashMap<WishlistId, Option<String>>,
    /// what a copy that came and was no better really is («FLAC из MP3
    /// ~128»), until its import ended,
    upgrade_dups: HashMap<WishlistId, String>,
    /// and the format of the file that came for a track (its master's, when
    /// the library keeps an Opus of it), for the words once it is done.
    upgrade_came: HashMap<TrackId, String>,
}

/// Sources whose downloads turned us away: after [`ASIDE_AFTER`] in a row
/// a source is set aside (its offers are not picked) for a while; a
/// download that works ends it.
#[derive(Default)]
pub(crate) struct Refusals(HashMap<SourceKey, Refused>);

#[derive(Default)]
pub(crate) struct Refused {
    /// Downloads turned away in a row.
    times: u32,
    until: Option<Instant>,
    /// How the last one was.
    why: Option<SourceError>,
}

impl Refusals {
    /// `key` turned a download away with `e`: the time it is set aside
    /// until, when it is now.
    pub(crate) fn refused(&mut self, key: SourceKey, e: &SourceError, now: Instant) -> Option<Instant> {
        let r = self.0.entry(key).or_default();
        r.times += 1;
        r.why = Some(e.clone());
        if r.times < ASIDE_AFTER {
            return None;
        }
        let long = (ASIDE_FIRST * 2u32.pow((r.times - ASIDE_AFTER).min(6))).min(ASIDE_LONGEST);
        let at = now + long.max(e.wait().unwrap_or_default());
        r.until = Some(at);
        Some(at)
    }

    /// A download of `key` worked: true when it was set aside.
    pub(crate) fn worked(&mut self, key: SourceKey) -> bool {
        self.0.remove(&key).is_some_and(|r| r.until.is_some())
    }

    /// Until when `key` is set aside, if it is.
    pub(crate) fn aside(&self, key: SourceKey, now: Instant) -> Option<Instant> {
        self.0.get(&key).and_then(|r| r.until).filter(|u| *u > now)
    }

    /// How `key`'s last refused download was.
    pub(crate) fn why(&self, key: SourceKey) -> Option<&SourceError> {
        self.0.get(&key).and_then(|r| r.why.as_ref())
    }
}

/// What an offer that did not work out led to.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Next {
    /// The next offer, of this provider.
    Offer(String),
    /// The doubtful offers (to «Подтверди», a track's torrents looked into first).
    Doubtful,
    /// Searched again at this unix ms.
    Later(i64),
    /// Nothing: searched again in a week.
    NotFound,
}

/// The doubtful offers of a track item on their way to «Подтверди»: a
/// torrent is shown only when its list has the track's file (a torrent
/// that names only the artist, or a word of their name, is another of
/// their releases as often as not).
#[derive(Default)]
struct Probe {
    /// The offers, best first.
    all: Vec<Offer>,
    /// Those of them still to look into (torrents), in turn.
    todo: Vec<usize>,
    /// Those whose list has the file.
    passed: HashSet<usize>,
}

/// Torrents of a track looked into before «Подтверди», at most (each
/// takes up to a minute), the likeliest first ([`look_first`]); the first
/// that has the file ends the looking.
const PROBE_MAX: usize = 3;

/// `cache` key prefix of «the item's song is not in this torrent» (per
/// track, or album, and torrent): such a torrent is never looked into or
/// offered for it again.
const MISS_KEY: &str = "wish_torrent_miss.v1";

/// `cache` key: the track items waiting in «Подтверди» with torrents shown
/// unlooked (before [`Probe`]) were searched for again.
const PROBED_KEY: &str = "wish_confirm_probed.v1";

impl WishState {
    /// The direct downloads' progress, for the rows.
    pub(crate) fn direct_progress(&self) -> HashMap<WishlistId, u8> {
        self.live.lock().clone()
    }
}

/// What asking for a download did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Wished {
    /// A new item, or one that waited / was dismissed and starts now.
    Started,
    /// It is on its way already.
    Already,
    /// Nothing to download (the files are here).
    Nothing,
}

/// A source that could not answer, in words for «Источники».
fn down_text(e: &SourceError) -> String {
    match e {
        SourceError::Blocked(_) => "Не пускает — попробую позже".into(),
        SourceError::RateLimited { .. } => "Просит подождать — попробую позже".into(),
        SourceError::Http { status } => format!("Отвечает ошибкой {status}"),
        _ => "Не отвечает".into(),
    }
}

/// A source whose download failed, in words for «Источники»; `None` when
/// the failure says nothing of the source (the disk, an empty offer).
fn fetch_down_text(e: &SourceError) -> Option<String> {
    match e {
        SourceError::Io(_) | SourceError::NothingToFetch | SourceError::NotConfigured => None,
        SourceError::Blocked(_) | SourceError::RateLimited { .. } => Some(down_text(e)),
        _ => Some("Находит, но не скачивает".into()),
    }
}

/// A source's name in a sentence (the indexer behind «Торренты» is the
/// one that answers or not).
fn source_name(key: SourceKey) -> &'static str {
    match key {
        SourceKey::Torrents => "Prowlarr",
        k => k.display_name(),
    }
}

/// Why `key` turned an item away, in plain words that name it: «Tidal
/// сейчас не отдаёт этот трек», «сервер Qobuz недоступен», «Qobuz просит
/// подождать до 03:00». `what`: «этот трек», «этот альбом», «остальное».
fn why_words(key: SourceKey, e: &SourceError, what: &str) -> String {
    let name = source_name(key);
    match e {
        SourceError::RateLimited { retry_after: Some(d) } if *d >= WAIT_TOLD => match super::accounts::hm_after(*d) {
            Some(hm) => format!("{name} просит подождать до {hm}"),
            None => format!("{name} просит подождать"),
        },
        SourceError::RateLimited { .. } => format!("{name} просит подождать"),
        SourceError::Blocked(_) => format!("{name} не пускает"),
        // A ban of the instance's account, a file it cannot give (DRM).
        SourceError::Http { status: 401 | 403 } | SourceError::Parse(_) => format!("{name} сейчас не отдаёт {what}"),
        SourceError::Http { status } if *status >= 500 => format!("сервер {name} недоступен"),
        SourceError::Unreachable(_) => format!("сервер {name} недоступен"),
        _ => format!("{name} не дал скачать"),
    }
}

/// «в 21:40» (or «через N мин» when the clock is unknown).
fn at_words(wait: Duration) -> String {
    match super::accounts::hm_after(wait) {
        Some(hm) => format!("в {hm}"),
        None => format!("через {} мин", wait.as_secs().div_ceil(60)),
    }
}

/// The first letter capitalised (a line that stands alone).
fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Where an offer comes from, for the lines: the source (a Soulseek
/// offer's provider is a user's name), a torrent as such.
fn offer_from(offer: &Offer) -> String {
    match offer.source {
        SourceKey::Torrents => "торрент".into(),
        k => k.display_name().into(),
    }
}

/// When an item every offer of which was turned away is searched again,
/// and the sources worth naming for then.
#[derive(Debug, PartialEq, Eq)]
struct Retry {
    wait: Duration,
    ask: Vec<SourceKey>,
}

/// The `n`th retry in a row of an item: at least [`LATER_FIRST`] doubling
/// each time (up to [`LATER_LONGEST`]), and not before the first source
/// that said no can be asked again (`turned`: each one with how long until
/// then, when known; up to a day). Named for then: those of them back by
/// then, and the `others` whose last answer for the item (how long ago)
/// will be [`ASK_AGAIN_AFTER`] old.
fn plan_retry(n: u32, turned: &[(SourceKey, Option<Duration>)], others: &[(SourceKey, Duration)]) -> Retry {
    let backoff = (LATER_FIRST * 2u32.pow(n.saturating_sub(1).min(10))).min(LATER_LONGEST);
    let soonest = turned.iter().map(|(_, back)| back.unwrap_or_default()).min().unwrap_or_default();
    let wait = backoff.max(soonest.min(ASIDE_LONGEST));
    let mut ask: Vec<SourceKey> = Vec::new();
    for (k, back) in turned {
        if back.is_none_or(|b| b <= wait) && !ask.contains(k) {
            ask.push(*k);
        }
    }
    for (k, ago) in others {
        if *ago + wait >= ASK_AGAIN_AFTER && !ask.contains(k) {
            ask.push(*k);
        }
    }
    Retry { wait, ask }
}

/// The line of an item every offer of which was turned away: what is
/// here of an album, why each source said no, and when which sources are
/// asked again.
fn later_line(have: Option<(usize, usize)>, why: &[String], wait: Duration, next: &[SourceKey]) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some((have, all)) = have {
        parts.push(format!("Есть {have} из {all}"));
    }
    parts.push(if why.is_empty() { "Источники не дали скачать".to_owned() } else { why.join(", ") });
    let names: Vec<&str> = next.iter().map(|k| source_name(*k)).collect();
    parts.push(if names.is_empty() {
        format!("попробую снова {}", at_words(wait))
    } else {
        format!("{} спрошу {}", at_words(wait), names.join(", "))
    });
    capitalised(&parts.join(" · "))
}

/// Offers saved in an item before stage 4.5 were the indexer's candidates.
fn offers_of_json(json: &str) -> Vec<Offer> {
    serde_json::from_str::<Vec<Offer>>(json)
        .ok()
        .or_else(|| serde_json::from_str::<Vec<Candidate>>(json).ok().map(|c| c.into_iter().map(offer_of).collect()))
        .unwrap_or_default()
}

fn status_rank(s: WishlistStatus) -> u8 {
    match s {
        WishlistStatus::Confirm => 0,
        WishlistStatus::Downloading => 1,
        WishlistStatus::Searching => 2,
        WishlistStatus::New => 3,
        WishlistStatus::Waiting => 4,
        WishlistStatus::NoTorrent => 5,
        WishlistStatus::Failed => 6,
        WishlistStatus::Done => 7,
        WishlistStatus::Dismissed => 8,
    }
}

impl Worker {
    fn now() -> i64 {
        nikgob_library::now_ms()
    }

    fn put_item(&mut self, item: &WishlistItem) {
        let _ = self.log_err("wishlist", self.lib.put_wishlist_item(item));
    }

    fn set_item(&mut self, id: WishlistId, f: impl FnOnce(&mut WishlistItem)) -> Option<WishlistItem> {
        let mut item = self.lib.get_wishlist_item(id).ok().flatten()?;
        f(&mut item);
        item.updated_at = Self::now();
        self.put_item(&item);
        Some(item)
    }

    /// A track with no file that streams: what the wishlist is for.
    fn stream_only(&self, track: TrackId) -> bool {
        let sources = self.lib.sources_of_track(track).unwrap_or_default();
        let live = |s: &&nikgob_core::model::Source| s.trashed_at.is_none();
        !sources.iter().filter(live).any(|s| s.kind.is_local())
            && sources.iter().filter(live).any(|s| !s.kind.is_local())
    }

    /// An open item of the same song as `item` under another track (a
    /// twin not merged yet): the same ISRC, or the same title and artist.
    fn open_twin(&self, item: &WishlistItem) -> Option<WishlistItem> {
        let key = |t: &Option<String>, a: &Option<String>| {
            (title_key(t.as_deref().unwrap_or_default()), artist_key(a.as_deref().unwrap_or_default()))
        };
        let mine = key(&item.title, &item.artist);
        self.lib.list_wishlist().unwrap_or_default().into_iter().find(|i| {
            i.scope == WishlistScope::Track
                && i.id != item.id
                && !matches!(i.status, WishlistStatus::Done | WishlistStatus::Failed)
                && (item.isrc.is_some() && i.isrc == item.isrc || !mine.0.base.is_empty() && key(&i.title, &i.artist) == mine)
        })
    }

    /// The fields an item takes from its track.
    fn item_for(&self, track: Option<TrackId>, release: Option<ReleaseId>) -> Option<WishlistItem> {
        let mut item = WishlistItem::new(WishlistStatus::New, Self::now());
        if let Some(t) = track.and_then(|t| self.lib.get_track(t).ok().flatten()) {
            item.title = Some(t.title.clone());
            item.artist = Some(t.artist_credit.display());
            item.isrc = t.isrc.clone();
            item.recording_mbid = t.recording_mbid.clone();
            item.track_id = Some(t.id);
            item.release_id = release.or(t.release_id);
            // The YouTube id is the one that plays (a song, not its clip).
            let plays = self
                .lib
                .sources_of_track(t.id)
                .unwrap_or_default()
                .into_iter()
                .filter(|s| s.kind == SourceKind::Youtube && s.available && s.trashed_at.is_none())
                .max_by_key(|s| s.quality_score)
                .map(|s| s.locator);
            for r in self
                .lib
                .external_ids(EntityKind::Track, *t.id.as_uuid())
                .unwrap_or_default()
            {
                match r.service {
                    Service::Spotify => item.spotify_id = Some(r.id),
                    Service::Youtube if plays.is_none() => item.youtube_id = Some(r.id),
                    Service::Youtube => item.youtube_id.clone_from(&plays),
                    // The wishlist searches torrents by name and ISRC.
                    Service::Deezer | Service::Soundcloud => {}
                }
            }
        } else {
            let r = release.and_then(|r| self.lib.get_release(r).ok().flatten())?;
            item.title = Some(r.title.clone());
            item.artist = Some(r.artist_credit.display());
            item.release_id = Some(r.id);
        }
        Some(item)
    }

    /// The track's metadata changed (a liked clip became its song): its
    /// track items that did not start yet take the new title, artist,
    /// album and ids.
    pub(crate) fn wish_track_changed(&mut self, track: TrackId) {
        let Some(fresh) = self.item_for(Some(track), None) else { return };
        for item in self.lib.wishlist_for_track(track).unwrap_or_default() {
            let unstarted = matches!(item.status, WishlistStatus::New | WishlistStatus::Waiting | WishlistStatus::NoTorrent);
            if !unstarted || item.scope != WishlistScope::Track {
                continue;
            }
            self.set_item(item.id, |i| {
                i.title.clone_from(&fresh.title);
                i.artist.clone_from(&fresh.artist);
                i.isrc.clone_from(&fresh.isrc);
                i.release_id = fresh.release_id;
                i.youtube_id.clone_from(&fresh.youtube_id);
            });
        }
        self.publish_wishlist();
    }

    /// A new like of `track`: a wishlist item when it has no file (an album
    /// item when the prefs say so), unless one exists or was dismissed, or
    /// likes do not download («Ничего»). True when an item was made.
    pub(crate) fn wish_for_like(&mut self, track: TrackId, origin: WishlistOrigin) -> bool {
        if !self.state.read().prefs.like_downloads || !self.stream_only(track) {
            return false;
        }
        let existing = self.lib.wishlist_for_track(track).unwrap_or_default();
        let open = |i: &WishlistItem| !matches!(i.status, WishlistStatus::Done | WishlistStatus::Failed);
        if existing.iter().any(open) {
            return false; // waiting already, or dismissed for good
        }
        let scope = self.state.read().prefs.like_download_scope;
        let release = self.lib.get_track(track).ok().flatten().and_then(|t| t.release_id);
        if scope == WishlistScope::Album
            && let Some(r) = release
            && self.lib.wishlist_for_release(r).unwrap_or_default().iter().any(open)
        {
            return false;
        }
        let Some(mut item) = self.item_for(Some(track), release) else { return false };
        item.scope = if release.is_some() { scope } else { WishlistScope::Track };
        if item.scope == WishlistScope::Album
            && let Some(r) = release.and_then(|r| self.lib.get_release(r).ok().flatten())
        {
            item.title = Some(r.title);
            item.artist = Some(r.artist_credit.display());
        }
        item.origin = origin;
        item.status = if origin == WishlistOrigin::Import {
            WishlistStatus::Waiting
        } else {
            WishlistStatus::New
        };
        if item.scope == WishlistScope::Track && self.open_twin(&item).is_some() {
            return false;
        }
        log::info!("wishlist: {} ({:?}, {:?})", item.title.as_deref().unwrap_or("?"), item.scope, origin);
        self.put_item(&item);
        true
    }

    /// A new like of an album: one album item, when some of it has no file
    /// (and likes download). True when an item was made.
    pub(crate) fn wish_for_album(&mut self, release: ReleaseId, tracks: &[TrackId], origin: WishlistOrigin) -> bool {
        if !self.state.read().prefs.like_downloads || !tracks.iter().any(|&t| self.stream_only(t)) {
            return false;
        }
        let open = |i: &WishlistItem| !matches!(i.status, WishlistStatus::Done | WishlistStatus::Failed);
        if self.lib.wishlist_for_release(release).unwrap_or_default().iter().any(open) {
            return false;
        }
        let Some(mut item) = self.item_for(None, Some(release)) else { return false };
        item.scope = WishlistScope::Album;
        item.origin = origin;
        item.status = if origin == WishlistOrigin::Import {
            WishlistStatus::Waiting
        } else {
            WishlistStatus::New
        };
        self.put_item(&item);
        true
    }

    // ------------------------------------------------------------- download without a like

    /// An item that waits, found nothing or was dismissed starts again.
    fn start_again(&mut self, item: &WishlistItem) -> Wished {
        match item.status {
            WishlistStatus::Waiting
            | WishlistStatus::NoTorrent
            | WishlistStatus::Failed
            | WishlistStatus::Dismissed => {
                self.set_item(item.id, |i| {
                    i.status = WishlistStatus::New;
                    i.next_try_at = None;
                });
                Wished::Started
            }
            WishlistStatus::Done => Wished::Nothing,
            _ => Wished::Already,
        }
    }

    /// `track` to «Хочу скачать» now (no like): its item starts, or a new
    /// track item is made for it.
    fn wish_track_now(&mut self, track: TrackId) -> Wished {
        let has_file = self.lib.get_track_row(track).ok().flatten().is_some_and(|r| r.has_local);
        if has_file {
            return Wished::Nothing;
        }
        let existing = self.lib.wishlist_for_track(track).unwrap_or_default();
        if let Some(i) = existing.iter().find(|i| i.status != WishlistStatus::Done) {
            return self.start_again(i);
        }
        let release = self.lib.get_track(track).ok().flatten().and_then(|t| t.release_id);
        if let Some(r) = release
            && let Some(i) = self
                .lib
                .wishlist_for_release(r)
                .unwrap_or_default()
                .iter()
                .find(|i| i.scope == WishlistScope::Album && !matches!(i.status, WishlistStatus::Done))
        {
            return self.start_again(i);
        }
        let Some(mut item) = self.item_for(Some(track), release) else { return Wished::Nothing };
        item.scope = WishlistScope::Track;
        item.origin = WishlistOrigin::Manual;
        item.status = WishlistStatus::New;
        if let Some(twin) = self.open_twin(&item) {
            return self.start_again(&twin);
        }
        self.put_item(&item);
        Wished::Started
    }

    /// The missing files of `release` to «Хочу скачать» now: one album
    /// item (started at once).
    fn wish_release_now(&mut self, release: ReleaseId) -> Wished {
        let p = self.lib.release_progress(release).ok().flatten();
        let tracks = self.lib.tracks_of_release_all(release).unwrap_or_default();
        let missing = p.is_none_or(|p| p.local < p.total) || tracks.iter().any(|t| !t.has_local);
        if !missing {
            return Wished::Nothing;
        }
        let items = self.lib.wishlist_for_release(release).unwrap_or_default();
        if let Some(i) = items
            .iter()
            .find(|i| i.scope == WishlistScope::Album && i.status != WishlistStatus::Done)
        {
            return self.start_again(i);
        }
        let Some(mut item) = self.item_for(None, Some(release)) else { return Wished::Nothing };
        item.scope = WishlistScope::Album;
        item.origin = WishlistOrigin::Manual;
        item.status = WishlistStatus::New;
        self.put_item(&item);
        Wished::Started
    }

    /// The toast after a download was asked for: where it went, and where
    /// to look (or what is missing to download at all).
    fn download_toast(&mut self, started: usize, already: usize, what: Option<String>) {
        let wishlist = v::Link::Route(v::Route::Downloads(v::DownloadsTab::Wishlist));
        if started == 0 {
            if already > 0 {
                self.toast_open("Уже в «Хочу скачать»", "Открыть", wishlist);
            } else {
                self.toast("Всё уже скачано", None);
            }
            return;
        }
        let text = match (started, what) {
            (1, Some(w)) => format!("В «Хочу скачать»: {w}"),
            (n, _) => format!(
                "В «Хочу скачать»: {n} {}",
                if n % 10 == 1 && n % 100 != 11 {
                    "трек"
                } else if (2..=4).contains(&(n % 10)) && !(12..=14).contains(&(n % 100)) {
                    "трека"
                } else {
                    "треков"
                }
            ),
        };
        if self.can_download() {
            self.toast_open(text, "Открыть", wishlist);
        } else {
            self.toast_open(
                format!("{text} · скачается, когда включишь источник скачивания"),
                "Настроить",
                v::Link::Setting(v::SettingLink::Torrents),
            );
        }
    }

    /// `Command::DownloadStream`: stream tracks to «Хочу скачать» without a
    /// like. Each gets its track row (outside the library until its file is
    /// here, then a library track as any downloaded one).
    pub(crate) fn download_stream(&mut self, remotes: &[nikgob_core::streaming::RemoteId]) {
        let known: Vec<nikgob_core::streaming::StreamTrack> =
            remotes.iter().filter_map(|r| self.streaming.known.get(r).cloned()).collect();
        if known.is_empty() {
            self.toast("Трек устарел — найди его заново", None);
            return;
        }
        let ids = self.upsert_streams(&known);
        self.download_tracks(&ids);
    }

    /// `Command::DownloadTracks`: library tracks without a file → «Хочу
    /// скачать», each its own track item.
    pub(crate) fn download_tracks(&mut self, tracks: &[TrackId]) {
        let (mut started, mut already) = (0, 0);
        let mut first = None;
        for &t in tracks {
            match self.wish_track_now(t) {
                Wished::Started => {
                    started += 1;
                    if first.is_none() {
                        first = self.lib.get_track(t).ok().flatten().map(|t| format!("«{}»", t.title));
                    }
                }
                Wished::Already => already += 1,
                Wished::Nothing => {}
            }
        }
        self.download_toast(started, already, first);
        self.publish_wishlist();
        self.wish_tick();
        self.refresh_stream_page();
    }

    /// `Command::DownloadRelease` and the album of a service once fetched
    /// (`Command::DownloadStreamAlbum`): one album item.
    pub(crate) fn download_release(&mut self, release: ReleaseId) {
        let title = self.lib.get_release(release).ok().flatten().map(|r| format!("альбом «{}»", r.title));
        match self.wish_release_now(release) {
            Wished::Started => self.download_toast(1, 0, title),
            Wished::Already => self.download_toast(0, 1, None),
            Wished::Nothing => self.download_toast(0, 0, None),
        }
        self.publish_wishlist();
        self.wish_tick();
        self.refresh_stream_page();
    }

    /// The like went away: items it made that did not start yet go too
    /// («Скачать» without a like stays: it was asked for on its own).
    pub(crate) fn wish_drop_unstarted(&mut self, track: TrackId) {
        for i in self.lib.wishlist_for_track(track).unwrap_or_default() {
            if !matches!(i.origin, WishlistOrigin::Manual | WishlistOrigin::Upgrade)
                && matches!(
                    i.status,
                    WishlistStatus::New | WishlistStatus::Waiting | WishlistStatus::NoTorrent | WishlistStatus::Confirm
                )
            {
                let _ = self.lib.delete_wishlist_item(i.id);
                self.streaming.wish.cands.remove(&i.id);
            }
        }
    }

    /// The album's like went away: its album item goes too, unless it was
    /// asked for with «Скачать».
    pub(crate) fn wish_drop_album(&mut self, release: ReleaseId) {
        for i in self.lib.wishlist_for_release(release).unwrap_or_default() {
            if i.scope == WishlistScope::Album
                && !matches!(i.origin, WishlistOrigin::Manual | WishlistOrigin::Upgrade)
                && matches!(i.status, WishlistStatus::New | WishlistStatus::Waiting | WishlistStatus::NoTorrent | WishlistStatus::Confirm)
            {
                let _ = self.lib.delete_wishlist_item(i.id);
            }
        }
    }

    /// The user deletes `tracks` (to the Trash, or for good): the items of
    /// each of them that are not done are dismissed (a deleted song is
    /// not downloaded again). Returns the albums of their album items, to
    /// [`Self::wish_settle_albums`] once the tracks are gone.
    pub(crate) fn wish_drop_deleted(&mut self, tracks: &[TrackId]) -> Vec<ReleaseId> {
        let mut albums: Vec<ReleaseId> = Vec::new();
        for &t in tracks {
            for i in self.lib.wishlist_for_track(t).unwrap_or_default() {
                if i.scope == WishlistScope::Track && !matches!(i.status, WishlistStatus::Done | WishlistStatus::Dismissed) {
                    self.streaming.wish.cands.remove(&i.id);
                    self.wish_line_drop(i.id);
                    self.set_item(i.id, |i| {
                        i.status = WishlistStatus::Dismissed;
                        i.next_try_at = None;
                    });
                }
            }
            let release = self.lib.get_track(t).ok().flatten().and_then(|t| t.release_id);
            if let Some(r) = release.filter(|r| !albums.contains(r)) {
                albums.push(r);
            }
        }
        albums
    }

    /// Tracks of `releases` were deleted: an album item that waits for
    /// more of its album (not one on its way) is «Скачано» once the rest is
    /// there (the deleted songs count as not needed), and goes when none
    /// of its album is wanted any more.
    pub(crate) fn wish_settle_albums(&mut self, releases: &[ReleaseId]) {
        let mut changed = false;
        for &r in releases {
            if self.lib.get_release(r).ok().flatten().is_none() {
                continue;
            }
            for i in self.lib.wishlist_for_release(r).unwrap_or_default() {
                if i.scope != WishlistScope::Album
                    || matches!(
                        i.status,
                        WishlistStatus::Dismissed | WishlistStatus::Searching | WishlistStatus::Downloading | WishlistStatus::Confirm
                    )
                {
                    continue;
                }
                let Ok((have, all)) = nikgob_import::release_files(&self.lib, r) else { continue };
                let status = match (have, all) {
                    (0, 0) => WishlistStatus::Dismissed,
                    (have, all) if have == all => WishlistStatus::Done,
                    _ => continue,
                };
                if status == i.status && (status != WishlistStatus::Done || i.note.as_deref() == Some("Скачано")) {
                    continue;
                }
                self.streaming.wish.cands.remove(&i.id);
                self.streaming.wish.later.remove(&i.id);
                self.wish_line_drop(i.id);
                self.set_item(i.id, |i| {
                    i.status = status;
                    i.next_try_at = None;
                    if status == WishlistStatus::Done {
                        i.note = Some("Скачано".into());
                    }
                });
                changed = true;
            }
        }
        if changed {
            self.publish_wishlist();
        }
    }

    /// The tracks an item's files should become (an album's tracks outside
    /// the library too: the ones «Хочу скачать» made for it).
    fn targets_of(&self, item: &WishlistItem) -> Vec<TrackId> {
        let mut out: Vec<TrackId> = item.track_id.into_iter().collect();
        if item.scope == WishlistScope::Album
            && let Some(r) = item.release_id
        {
            for row in self.lib.tracks_of_release_all(r).unwrap_or_default() {
                if !out.contains(&row.id) {
                    out.push(row.id);
                }
            }
        }
        out
    }

    /// The import targets of the item a torrent was downloaded for: its
    /// tracks, its album and the item itself.
    pub(crate) fn wish_targets(&self, item: WishlistId) -> ImportTargets {
        self.lib
            .get_wishlist_item(item)
            .ok()
            .flatten()
            .map(|i| self.item_targets(&i, false))
            .unwrap_or_default()
    }

    fn item_targets(&self, item: &WishlistItem, force: bool) -> ImportTargets {
        let tracks = self.targets_of(item);
        // A better copy of one track is that track whatever its file's tags
        // say: it keeps its title, number, disc and album (and so its file
        // name), and only the file changes.
        let force = force || (item.origin == WishlistOrigin::Upgrade && item.scope == WishlistScope::Track && tracks.len() == 1);
        ImportTargets {
            tracks,
            force,
            release: item.release_id.filter(|_| item.scope == WishlistScope::Album),
            wishlist: Some(item.id),
            // A better copy replaces the item's files or nothing: what else
            // came (another album under the same word) is never imported.
            only_targets: item.origin == WishlistOrigin::Upgrade,
        }
    }

    pub(crate) fn want_of(&self, item: &WishlistItem) -> Want {
        let track = item.track_id.and_then(|t| self.lib.get_track(t).ok().flatten());
        let release = item
            .release_id
            .or(track.as_ref().and_then(|t| t.release_id))
            .and_then(|r| self.lib.get_release(r).ok().flatten());
        // The credited names, the album's artist first (a soundtrack's
        // songs are credited to their singers).
        let mut artists: Vec<String> = Vec::new();
        let credits = release
            .iter()
            .flat_map(|r| r.artist_credit.0.iter())
            .chain(track.iter().flat_map(|t| t.artist_credit.0.iter()));
        let mut main = None;
        for part in credits {
            let name = part.name.trim();
            if name.is_empty() || artists.iter().any(|a| a == name) {
                continue;
            }
            main = main.or(part.artist_id);
            artists.push(name.to_owned());
        }
        if artists.is_empty()
            && let Some(a) = item.artist.as_deref().filter(|a| !a.trim().is_empty())
        {
            artists.push(a.trim().to_owned());
        }
        // Other spellings of the main artist: its own name and its aliases.
        let mut aliases: Vec<String> = Vec::new();
        if let Some(id) = main {
            if let Some(a) = self.lib.get_artist(id).ok().flatten()
                && Some(&a.name) != artists.first()
            {
                aliases.push(a.name);
            }
            for (who, name) in self.lib.artist_aliases().unwrap_or_default() {
                if who == id && !aliases.contains(&name) && Some(&name) != artists.first() {
                    aliases.push(name);
                }
            }
        }
        Want {
            artists,
            aliases,
            album: release.as_ref().map(|r| r.title.clone()),
            title: match item.scope {
                WishlistScope::Track => track.as_ref().map(|t| t.title.clone()).or_else(|| item.title.clone()),
                WishlistScope::Album => None,
            },
            scope: match item.scope {
                WishlistScope::Track => WantScope::Track,
                WishlistScope::Album => WantScope::Album,
            },
            track_no: track.as_ref().map_or(0, |t| t.track_no),
            disc_no: track.as_ref().map_or(1, |t| t.disc_no),
            duration_ms: track.as_ref().and_then(|t| t.duration_ms),
            isrc: track.as_ref().and_then(|t| t.isrc.clone()).or_else(|| item.isrc.clone()),
            album_tracks: release.as_ref().and_then(|r| r.total_tracks).unwrap_or(0),
            // What of the album came already (a part of it from a source
            // that then turned us away), and the songs the user deleted
            // from it: a fetch leaves them out.
            got: match (item.scope, &release) {
                // A better copy of an album: what is good enough already.
                (WishlistScope::Album, Some(r)) if item.origin == WishlistOrigin::Upgrade => {
                    let bar = self.state.read().prefs.upgrade_below.score();
                    self.lib
                        .tracks_of_release(r.id)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|t| {
                            self.lib
                                .active_source(t.id)
                                .ok()
                                .flatten()
                                .is_some_and(|s| s.kind.is_local() && s.quality_score >= bar)
                        })
                        .map(|t| t.title)
                        .collect()
                }
                (WishlistScope::Album, Some(r)) => self
                    .lib
                    .tracks_of_release(r.id)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|t| t.has_local)
                    .map(|t| t.title)
                    .chain(self.lib.unwanted_of_release(r.id).unwrap_or_default().into_iter().map(|u| u.title))
                    .collect(),
                _ => Vec::new(),
            },
        }
    }

    /// The download sources that are on, in the user's order: the indexer's
    /// torrents (once Prowlarr is set up) and the sources the app has.
    pub(crate) fn download_sources(&self) -> Vec<Arc<dyn Source>> {
        let order = self.state.read().prefs.source_order();
        let mut all: Vec<Arc<dyn Source>> = self.sources.clone();
        if let Some(i) = &self.indexer {
            all.push(Arc::new(nikgob_sources::torrent::IndexerSource::new(i.clone())));
        }
        order
            .iter()
            .filter(|s| s.on)
            .filter_map(|s| all.iter().find(|x| x.key() == s.key).cloned())
            .collect()
    }

    /// A source the user turned on or set up can search now: everything not
    /// found is searched for again, there too.
    pub(crate) fn wish_sources_ready(&mut self) {
        if self.streaming.wish.enabled.is_empty() {
            return;
        }
        let keys: Vec<SourceKey> = self.download_sources().iter().map(|s| s.key()).collect();
        let before = self.streaming.wish.enabled.len();
        self.streaming.wish.enabled.retain(|k| !keys.contains(k));
        if self.streaming.wish.enabled.len() < before {
            self.wish_retry_all();
        }
    }

    /// Something can search for the items by itself.
    pub(crate) fn can_download(&self) -> bool {
        !self.download_sources().is_empty()
    }

    /// The quality of the copy `item`'s track has (saved from YouTube),
    /// if that is all it has.
    fn copy_quality(&self, item: &WishlistItem) -> Option<i32> {
        let src = self.lib.active_source(item.track_id?).ok().flatten()?;
        (src.kind.is_local() && src.quality_score <= score::SAVED_STREAM).then_some(src.quality_score)
    }

    /// «Источники»: every source in the user's order, what the app has,
    /// and how the last search of each went.
    pub(crate) fn publish_sources(&mut self) {
        let order = self.state.read().prefs.source_order();
        let mut down = self.streaming.wish.fetch_down.clone();
        // A search that failed says more: the source is not there at all.
        down.extend(self.streaming.wish.down.clone());
        let have: Vec<SourceKey> = self.sources.iter().map(|s| s.key()).collect();
        let indexer = self.indexer.is_some();
        let rows: Vec<v::SourceView> = order
            .iter()
            .map(|s| v::SourceView {
                key: s.key,
                name: s.key.display_name().to_owned(),
                available: if s.key == SourceKey::Torrents { indexer } else { have.contains(&s.key) },
                status: down.get(&s.key).cloned(),
            })
            .collect();
        let can = self.can_download();
        self.publish(|st| {
            st.downloads.sources = rows;
            st.downloads.wishlist.can_search = can;
        });
    }

    // ------------------------------------------------------------- scheduler

    /// Starts searches while there are free slots.
    pub(crate) fn wish_tick(&mut self) {
        if self.import.is_none() || !self.can_download() {
            return;
        }
        let limit = self.state.read().prefs.auto_download_limit.max(1) as usize;
        let busy = [WishlistStatus::Searching, WishlistStatus::Downloading]
            .iter()
            .map(|s| self.lib.list_wishlist_by_status(*s).map_or(0, |l| l.len()))
            .sum::<usize>();
        let mut free = limit.saturating_sub(busy);
        if free == 0 {
            return;
        }
        let now = Self::now();
        // Items whose file came another way are done; track items an album
        // item covers are searched with it, never apart.
        match self.lib.settle_downloaded_wishlist() {
            Ok(0) => {}
            Ok(n) => {
                log::info!("wishlist: {n} items have their file already");
                self.publish_wishlist();
            }
            Err(e) => log::warn!("wishlist: settling the downloaded items: {e}"),
        }
        let covered = self.lib.wishlist_covered().unwrap_or_default();
        let ask = (free + covered.len()) as u32;
        let mut due: Vec<WishlistItem> = self
            .lib
            .wishlist_due(WishlistStatus::New, now, ask)
            .unwrap_or_default()
            .into_iter()
            .filter(|i| !covered.contains(&i.id))
            .collect();
        if due.len() < free {
            // The weekly search of what was not found, or has only the
            // YouTube copy.
            due.extend(
                self.lib
                    .wishlist_due(WishlistStatus::NoTorrent, now, ask)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|i| i.next_try_at.is_some() && !covered.contains(&i.id)),
            );
        }
        for item in due {
            if free == 0 {
                break;
            }
            free -= 1;
            self.wish_search(item);
        }
    }

    fn wish_search(&mut self, item: WishlistItem) {
        let mut sources = self.download_sources();
        // Searched again because sources turned it away: those are asked,
        // and the others once they had their [`ASK_AGAIN_AFTER`] (what had
        // nothing then may have it now).
        if let Some(only) = self.streaming.wish.retry_only.remove(&item.id) {
            let asked = self.streaming.wish.asked.get(&item.id);
            let stale = |k: SourceKey| asked.and_then(|a| a.get(&k)).is_some_and(|t| t.elapsed() >= ASK_AGAIN_AFTER);
            let some: Vec<Arc<dyn Source>> =
                sources.iter().filter(|s| only.contains(&s.key()) || stale(s.key())).cloned().collect();
            if !some.is_empty() {
                sources = some;
            }
        }
        if sources.is_empty() {
            return;
        }
        let want = self.want_of(&item);
        if want.artists.is_empty() || (want.title.is_none() && want.album.is_none()) {
            self.no_torrent(item.id, false);
            return;
        }
        let id = item.id;
        self.set_item(id, |i| {
            i.status = WishlistStatus::Searching;
            i.attempts += 1;
            i.note = Some("Ищу…".into());
        });
        let names: Vec<&str> = sources.iter().map(|s| s.name()).collect();
        log::info!(
            "wishlist: searching {} — {} in {names:?}",
            want.artist(),
            want.title.as_deref().or(want.album.as_deref()).unwrap_or("?")
        );
        self.streaming.spawn(async move {
            let asks = sources.iter().map(|s| {
                let s = s.clone();
                let want = &want;
                async move {
                    let r = match tokio::time::timeout(SEARCH_TIMEOUT, s.search(want)).await {
                        Ok(r) => r,
                        Err(_) => Err(SourceError::Unreachable("no answer in time".into())),
                    };
                    (s.key(), r)
                }
            });
            let mut offers = Vec::new();
            let (mut answered, mut down) = (Vec::new(), Vec::new());
            for (key, r) in futures_util::future::join_all(asks).await {
                match r {
                    Ok(o) => {
                        log::info!("wishlist: {} offers from {key:?}", o.len());
                        offers.extend(o);
                        answered.push(key);
                    }
                    Err(SourceError::NotConfigured) => {}
                    Err(e) => {
                        log::warn!("wishlist: {key:?}: {e}");
                        down.push((key, e));
                    }
                }
            }
            // Nobody could answer: not "not found", just later.
            let result = if answered.is_empty() && !down.is_empty() {
                Err(down.iter().map(|(k, e)| format!("{}: {e}", k.display_name())).collect::<Vec<_>>().join("; "))
            } else {
                Ok(offers)
            };
            Msg::Wish(WishMsg::Searched {
                item: id,
                result,
                answered,
                down,
            })
        });
        self.publish_wishlist();
    }

    pub(crate) fn wish_msg(&mut self, m: WishMsg) {
        match m {
            WishMsg::Searched {
                item,
                result,
                answered,
                down,
            } => {
                let now = Instant::now();
                for k in answered {
                    self.streaming.wish.down.remove(&k);
                    self.streaming.wish.asked.entry(item).or_default().insert(k, now);
                }
                for (k, e) in &down {
                    self.streaming.wish.down.insert(*k, down_text(e));
                }
                let turned: Vec<(SourceKey, SourceError)> = down.into_iter().filter(|(_, e)| e.is_refusal()).collect();
                self.streaming.wish.turned.insert(item, turned);
                self.publish_sources();
                self.wish_searched(item, result);
            }
            WishMsg::Fetched { item, offer, result } if self.streaming.wish.probe.contains_key(&item) => match result {
                Ok(Fetched::Torrent(p)) => self.wish_inspect(item, *offer, p),
                Ok(Fetched::Files(_) | Fetched::Partial { .. }) | Err(_) => self.probe_done(item, false),
            },
            WishMsg::Fetched { item, offer, result } => match result {
                Ok(Fetched::Torrent(p)) => self.wish_inspect(item, *offer, p),
                Ok(Fetched::Files(files)) => {
                    let was_aside = self.streaming.wish.refusals.worked(offer.source);
                    if self.streaming.wish.fetch_down.remove(&offer.source).is_some() {
                        self.publish_sources();
                    }
                    let key = offer.source;
                    self.wish_files(item, *offer, files);
                    if was_aside {
                        self.wish_wake(key);
                    }
                }
                Ok(Fetched::Partial { files, missing, error }) => self.wish_partial(item, *offer, files, missing.len(), error),
                Err(e) => {
                    log::warn!("wishlist: fetching {} from {:?}: {e}", offer.title, offer.source);
                    if offer.source != SourceKey::Torrents
                        && let Some(text) = fetch_down_text(&e)
                    {
                        self.streaming.wish.fetch_down.insert(offer.source, text);
                        self.publish_sources();
                    }
                    self.streaming.wish.live.lock().remove(&item);
                    self.drop_direct(item);
                    let refused = e.is_refusal() && offer.source != SourceKey::Torrents;
                    if refused {
                        self.wish_turned_away(item, &offer, &e);
                    }
                    let next = self.wish_next_candidate(item);
                    if refused {
                        self.refusal_line(item, &offer, &e, &next);
                    }
                }
            },
            WishMsg::Inspected {
                item,
                offer,
                payload,
                result,
            } => self.wish_inspected(item, *offer, payload, result),
            WishMsg::Saved { item, track, result } => self.wish_saved(item, track, result),
        }
    }

    fn wish_searched(&mut self, id: WishlistId, result: Result<Vec<Offer>, String>) {
        let Some(item) = self.lib.get_wishlist_item(id).ok().flatten() else { return };
        if item.status != WishlistStatus::Searching {
            return; // dismissed meanwhile
        }
        let offers = match result {
            Ok(c) => c,
            Err(e) => {
                // The sources are down: not "not found", just later.
                log::warn!("wishlist search: {e}");
                if self.streaming.wish.turned.get(&id).is_some_and(|t| !t.is_empty()) {
                    self.wish_later(id);
                    return;
                }
                self.set_item(id, |i| {
                    i.status = WishlistStatus::New;
                    i.next_try_at = Some(Self::now() + 3_600_000);
                    i.note = Some("Источники не ответили — попробую через час".into());
                });
                self.publish_wishlist();
                return;
            }
        };
        let want = self.want_of(&item);
        let order: Vec<SourceKey> = self.state.read().prefs.source_order().iter().map(|s| s.key).collect();
        // A better copy of a file the library has: only what is at least as
        // good as the user wants.
        let copy = self.copy_quality(&item).or_else(|| self.upgrade_floor(&item));
        // A source set aside (its downloads kept failing) is not picked;
        // the item may wait for it.
        let now = Instant::now();
        let mut offers = offers;
        let mut aside: Vec<SourceKey> = Vec::new();
        offers.retain(|o| {
            let out = self.streaming.wish.refusals.aside(o.source, now).is_some();
            if out && !aside.contains(&o.source) {
                aside.push(o.source);
            }
            !out
        });
        for key in aside {
            let left = self.streaming.wish.refusals.aside(key, now).map(|u| u - now);
            log::info!("wishlist: {key:?} is set aside, its offers wait");
            // Told as what it kept doing (a 403 is not «просит подождать»).
            let why = self.streaming.wish.refusals.why(key).cloned().unwrap_or(SourceError::RateLimited { retry_after: left });
            self.streaming.wish.turned.entry(id).or_default().push((key, why));
        }
        let ranked: Vec<_> = rank(offers, &want, &order)
            .into_iter()
            // With a copy, only what is better than it.
            .filter(|r| copy.is_none_or(|q| r.offer.quality() > q))
            // A torrent whose list had no file of it before: never again.
            .filter(|r| {
                let missed = self.missed_before(&item, &r.offer);
                if missed {
                    log::info!("wishlist: {} had no file of it before", r.offer.title);
                }
                !missed
            })
            // A better copy of an album is a folder with the album's songs
            // in it, not one that only shares a word with it.
            .filter(|r| match crate::soulseek::folder_listing(&r.offer) {
                Some(files) => {
                    let fits = self.upgrade_folder_fits(&item, &files);
                    if !fits {
                        log::info!("wishlist: {} has none of the songs of {:?}", r.offer.title, want.album);
                    }
                    fits
                }
                None => true,
            })
            .collect();
        if ranked.is_empty() {
            // Nothing found, but a source could not be asked: later, not
            // in a week.
            if self.streaming.wish.turned.get(&id).is_some_and(|t| !t.is_empty()) {
                self.wish_later(id);
            } else {
                self.no_torrent(id, true);
            }
            return;
        }
        // It replaces a file the user has: asked first, unless they let it.
        if self.upgrade_asks(&item) {
            let offers: Vec<Offer> = ranked.into_iter().map(|r| r.offer).collect();
            self.confirm_after_look(id, offers);
            return;
        }
        match pick(&ranked, copy) {
            Some(first) => {
                // Every sure one the policy takes without asking (lossless,
                // or better than the copy), best first, the picked first:
                // when one source turns the download away, the next takes over.
                let takes = |r: &nikgob_sources::select::Ranked| {
                    r.verdict.sure && (r.offer.lossless() || copy.is_some_and(|q| r.offer.quality() > q))
                };
                let mut sure: Vec<Offer> = vec![ranked[first].offer.clone()];
                sure.extend(
                    ranked
                        .iter()
                        .enumerate()
                        .filter(|(i, r)| *i != first && takes(r))
                        .map(|(_, r)| r.offer.clone()),
                );
                // The rest (doubtful, or lossy and sure) go to «Подтверди»
                // when every one taken is turned away: another source's
                // file to choose, not a wait for the one that refuses.
                let doubtful: Vec<Offer> = ranked
                    .iter()
                    .enumerate()
                    .filter(|(i, r)| *i != first && !takes(r))
                    .map(|(_, r)| r.offer.clone())
                    .collect();
                let best = sure[0].clone();
                self.streaming.wish.cands.insert(id, sure);
                self.streaming.wish.doubtful.insert(id, doubtful);
                self.wish_fetch(id, best);
            }
            None => {
                let doubtful: Vec<Offer> = ranked.into_iter().map(|r| r.offer).collect();
                self.confirm_after_look(id, doubtful);
            }
        }
    }

    /// The doubtful `offers` (best first) into «Подтверди»; for a track,
    /// the torrents among them only once their list shows its file
    /// ([`Probe`]).
    fn confirm_after_look(&mut self, id: WishlistId, offers: Vec<Offer>) {
        // A track's torrents, and those of an album a better copy is looked
        // for (its list must have the album's songs), are looked into first,
        // the likeliest by their titles and sizes first.
        let item = self.lib.get_wishlist_item(id).ok().flatten();
        let track = item
            .as_ref()
            .is_some_and(|i| i.scope == WishlistScope::Track || i.origin == WishlistOrigin::Upgrade);
        let mut looked: Vec<usize> = offers
            .iter()
            .enumerate()
            .filter(|(_, o)| track && o.source == SourceKey::Torrents)
            .map(|(i, _)| i)
            .collect();
        if let Some(item) = &item {
            let want = self.want_of(item);
            // Stable: alike ones stay in the policy's order.
            looked.sort_by(|a, b| look_first(&offers[*b], &want).total_cmp(&look_first(&offers[*a], &want)));
        }
        looked.truncate(PROBE_MAX);
        // The torrents not looked into stay out; the rest keep their order.
        let mut all: Vec<Offer> = Vec::new();
        let mut at: HashMap<usize, usize> = HashMap::new();
        for (i, o) in offers.into_iter().enumerate() {
            if track && o.source == SourceKey::Torrents && !looked.contains(&i) {
                continue;
            }
            at.insert(i, all.len());
            all.push(o);
        }
        let todo: Vec<usize> = looked.iter().filter_map(|i| at.get(i).copied()).collect();
        self.streaming.wish.probe.insert(id, Probe { all, todo, passed: HashSet::new() });
        self.probe_next(id);
    }

    /// The next torrent of the item's [`Probe`], or «Подтверди» with what
    /// passed once none is left.
    fn probe_next(&mut self, id: WishlistId) {
        let Some(p) = self.streaming.wish.probe.get_mut(&id) else { return };
        if p.todo.is_empty() {
            let Some(p) = self.streaming.wish.probe.remove(&id) else { return };
            let shown: Vec<Offer> = p
                .all
                .into_iter()
                .enumerate()
                .filter(|(i, o)| o.source != SourceKey::Torrents || p.passed.contains(i))
                .map(|(_, o)| o)
                .take(CONFIRM_OPTIONS)
                .collect();
            if shown.is_empty() {
                // None of them had the file; a source that said no may give it later.
                if self.streaming.wish.turned.get(&id).is_some_and(|t| !t.is_empty()) {
                    self.wish_later(id);
                } else {
                    self.no_torrent(id, true);
                }
            } else {
                self.ask_confirm(id, &shown);
            }
            return;
        }
        let offer = p.all[p.todo[0]].clone();
        let Some(source) = self.source_of(offer.source) else {
            if let Some(p) = self.streaming.wish.probe.get_mut(&id) {
                p.todo.remove(0);
            }
            return self.probe_next(id);
        };
        let Some(item) = self.lib.get_wishlist_item(id).ok().flatten() else { return };
        let want = self.want_of(&item);
        let dest = self.direct_dir(id);
        self.set_item(id, |i| i.note = Some(format!("Смотрю, есть ли трек в раздаче: {}", offer.title)));
        self.streaming.spawn(async move {
            let quiet: nikgob_core::source::Progress = Arc::new(|_, _| {});
            let result = match tokio::time::timeout(Duration::from_secs(60), source.fetch(&offer, &want, &dest, quiet)).await {
                Ok(r) => r,
                Err(_) => Err(SourceError::Unreachable("the torrent took too long".into())),
            };
            Msg::Wish(WishMsg::Fetched { item: id, offer: Box::new(offer), result })
        });
        self.publish_wishlist();
    }

    /// The torrent the item's [`Probe`] looks into now has the track's
    /// file (`found`) or not: on to the next, or to «Подтверди» with it
    /// once one has it.
    fn probe_done(&mut self, id: WishlistId, found: bool) {
        if let Some(p) = self.streaming.wish.probe.get_mut(&id)
            && !p.todo.is_empty()
        {
            let i = p.todo.remove(0);
            if found {
                p.passed.insert(i);
                p.todo.clear();
            }
        }
        self.probe_next(id);
    }

    /// The user picks among `shown` in «Подтверди».
    fn ask_confirm(&mut self, id: WishlistId, shown: &[Offer]) {
        let json = serde_json::to_string(shown).ok();
        let upgrade = self.is_upgrade(id);
        self.set_item(id, |i| {
            i.status = WishlistStatus::Confirm;
            i.candidates_json = json;
            i.note = Some(if upgrade { "Нашёл качество лучше — подтверди замену" } else { "Не уверен, что это оно — выбери" }.into());
        });
        if upgrade {
            self.upgrade_offered(id, shown);
        }
        self.publish_wishlist();
    }

    /// The source of an offer, on or not (a pick in «Подтверди» from a
    /// source switched off since still downloads).
    fn source_of(&self, key: SourceKey) -> Option<Arc<dyn Source>> {
        if key == SourceKey::Torrents {
            return self
                .indexer
                .as_ref()
                .map(|i| Arc::new(nikgob_sources::torrent::IndexerSource::new(i.clone())) as Arc<dyn Source>);
        }
        self.sources.iter().find(|s| s.key() == key).cloned()
    }

    /// The folder a direct download of `item` writes into.
    fn direct_dir(&self, item: WishlistId) -> PathBuf {
        let root = match &self.import {
            Some(svc) => svc.config().downloads_dir.join(SOURCES_DIR),
            None => std::env::temp_dir().join("nikgob-sources"),
        };
        root.join(item.to_string())
    }

    /// The folder of `item`'s direct download goes (the import took it, or
    /// it failed).
    fn drop_direct(&mut self, item: WishlistId) {
        if let Some(dir) = self.streaming.wish.direct.remove(&item)
            && dir.exists()
            && let Err(e) = std::fs::remove_dir_all(&dir)
        {
            log::warn!("removing {}: {e}", dir.display());
        }
    }

    fn wish_fetch(&mut self, id: WishlistId, offer: Offer) {
        let Some(source) = self.source_of(offer.source) else {
            self.wish_next_candidate(id);
            return;
        };
        let Some(item) = self.lib.get_wishlist_item(id).ok().flatten() else { return };
        self.upgrade_fetching(&item);
        let want = self.want_of(&item);
        match nikgob_sources::matching::performer_of(&offer, &want).filter(|_| item.scope == WishlistScope::Track) {
            Some(names) => self.streaming.wish.relabel.insert(id, names),
            None => self.streaming.wish.relabel.remove(&id),
        };
        let torrent = offer.source == SourceKey::Torrents;
        let dest = self.direct_dir(id);
        let from = if offer.provider.is_empty() { offer.source.display_name().to_owned() } else { offer.provider.clone() };
        self.streaming.wish.from.insert(id, from);
        if torrent {
            self.set_item(id, |i| i.note = Some(format!("Раздача: {}", offer.title)));
        } else {
            let _ = std::fs::remove_dir_all(&dest);
            if let Err(e) = std::fs::create_dir_all(&dest) {
                log::warn!("{}: {e}", dest.display());
                self.toast("Не получилось создать папку для загрузки", None);
                return;
            }
            self.streaming.wish.direct.insert(id, dest.clone());
            self.streaming.wish.live.lock().insert(id, 0);
            let note = format!("{} · {}", offer.detail(), offer.title);
            self.set_item(id, |i| {
                i.status = WishlistStatus::Downloading;
                i.note = Some(note);
            });
        }
        let live = self.streaming.wish.live.clone();
        let size = offer.size_bytes;
        let progress: nikgob_core::source::Progress = Arc::new(move |done, total| {
            if let Some(t) = total.or(size).filter(|t| *t > 0) {
                let pct = (done.saturating_mul(100) / t).min(99) as u8;
                live.lock().insert(id, pct);
            }
        });
        self.streaming.spawn(async move {
            let limit = if torrent { Duration::from_secs(120) } else { FETCH_TIMEOUT };
            let result = match tokio::time::timeout(limit, source.fetch(&offer, &want, &dest, progress)).await {
                Ok(r) => r,
                Err(_) => Err(SourceError::Unreachable("the download took too long".into())),
            };
            Msg::Wish(WishMsg::Fetched {
                item: id,
                offer: Box::new(offer),
                result,
            })
        });
        self.publish_wishlist();
    }

    /// A source other than torrents brought its files: the import takes
    /// them for the item, as a file the user handed in would be.
    fn wish_files(&mut self, id: WishlistId, offer: Offer, files: Vec<PathBuf>) {
        self.streaming.wish.live.lock().remove(&id);
        let Some(item) = self.lib.get_wishlist_item(id).ok().flatten() else {
            self.drop_direct(id);
            return;
        };
        if matches!(item.status, WishlistStatus::Dismissed | WishlistStatus::Done) || files.is_empty() {
            self.drop_direct(id);
            if files.is_empty() {
                self.wish_next_candidate(id);
            }
            return;
        }
        let one = files.len() == 1 && item.scope == WishlistScope::Track && self.targets_of(&item).len() == 1;
        let targets = self.item_targets(&item, one);
        let title = format!("{}: {}", offer.provider, item.title.clone().unwrap_or_default());
        let note = format!("{} · {}", offer.detail(), offer.title);
        if self.import_for(files, title, SourceOrigin::Inbox, targets) {
            self.set_item(id, |i| {
                i.status = WishlistStatus::Downloading;
                i.note = Some(note);
            });
        } else {
            self.drop_direct(id);
            self.wish_next_candidate(id);
        }
        self.publish_wishlist();
    }

    fn wish_inspect(&mut self, id: WishlistId, offer: Offer, payload: TorrentPayload) {
        let tx = self.streaming.internal.clone();
        self.with_client(Box::new(move |w, c| {
            let Some(svc) = &w.import else { return };
            svc.spawn(async move {
                let result = c.inspect(payload.clone()).await.map_err(|e| e.to_string());
                let _ = tx.send(crate::import::Internal::Stream(Msg::Wish(WishMsg::Inspected {
                    item: id,
                    offer: Box::new(offer),
                    payload,
                    result,
                })));
            });
        }));
    }

    fn wish_inspected(
        &mut self,
        id: WishlistId,
        offer: Offer,
        payload: TorrentPayload,
        result: Result<nikgob_torrent::TorrentMeta, String>,
    ) {
        let Some(item) = self.lib.get_wishlist_item(id).ok().flatten() else { return };
        if matches!(item.status, WishlistStatus::Dismissed | WishlistStatus::Done) {
            self.streaming.wish.probe.remove(&id);
            return;
        }
        if self.streaming.wish.probe.contains_key(&id) {
            let want = self.want_of(&item);
            let found = match &result {
                Ok(meta) => {
                    let files: Vec<(usize, String, u64)> = meta.files.iter().map(|f| (f.index, f.path.clone(), f.size)).collect();
                    let found = pick_files(&files, &want).is_some() && self.torrent_fits(&item, &files);
                    if !found {
                        self.note_miss(&item, &offer, Some(meta.info_hash.as_str()));
                    }
                    found
                }
                // Not listed (no peers in time): not a miss, asked again later.
                Err(_) => false,
            };
            log::info!("wishlist: {} {} of {:?}", offer.title, if found { "has a file" } else { "has no file" }, want.title);
            self.probe_done(id, found);
            return;
        }
        let meta = match result {
            Ok(m) => m,
            Err(e) => {
                log::warn!("wishlist: listing {}: {e}", offer.title);
                self.wish_next_candidate(id);
                return;
            }
        };
        let files: Vec<(usize, String, u64)> = meta.files.iter().map(|f| (f.index, f.path.clone(), f.size)).collect();
        let want = self.want_of(&item);
        let Some(picked) = pick_files(&files, &want).filter(|_| self.torrent_fits(&item, &files)) else {
            log::info!("wishlist: {} has no file of {:?}", offer.title, want.title.as_ref().or(want.album.as_ref()));
            self.note_miss(&item, &offer, Some(meta.info_hash.as_str()));
            self.wish_next_candidate(id);
            return;
        };
        let size: u64 = files.iter().filter(|(i, ..)| picked.contains(i)).map(|(.., s)| s).sum();
        let format = offer.format.as_ref().map_or_else(|| "?".to_owned(), |f| f.label());
        let seeders = candidate_of(&offer).and_then(|c| c.seeders).unwrap_or(0);
        let note = format!(
            "{format} · {} · {seeders} сид. · {}",
            crate::torrents::human_bytes(size),
            offer.title
        );
        self.set_item(id, |i| {
            i.status = WishlistStatus::Downloading;
            i.note = Some(note);
        });
        self.streaming.wish.cands.remove(&id);
        self.streaming.wish.doubtful.remove(&id);
        let magnet = match &payload {
            TorrentPayload::Magnet(m) => Some(m.clone()),
            TorrentPayload::TorrentFile(_) => None,
        };
        self.add_payload_for(payload, Some(picked), magnet, Some(id));
        self.publish_wishlist();
    }

    /// What an item's torrent misses are kept under: its track, or its
    /// album.
    fn miss_target(item: &WishlistItem) -> Option<String> {
        match item.scope {
            WishlistScope::Track => item.track_id.map(|t| format!("t{t}")),
            _ => item.release_id.map(|r| format!("r{r}")),
        }
    }

    /// The torrent's `cache` keys of a miss for the item: its info hash
    /// (the indexer's, the list's) and the indexer's id of it.
    fn miss_keys(item: &WishlistItem, offer: &Offer, listed: Option<&str>) -> Vec<String> {
        let Some(target) = Self::miss_target(item) else { return Vec::new() };
        let cand = candidate_of(offer);
        let mut keys: Vec<String> = cand
            .as_ref()
            .and_then(|c| c.info_hash.as_deref())
            .into_iter()
            .chain(listed)
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .map(|h| format!("{MISS_KEY}:{target}:{}", h.to_lowercase()))
            .collect();
        if !offer.id.is_empty() {
            keys.push(format!("{MISS_KEY}:{target}:id:{}:{}", offer.provider, offer.id));
        }
        keys.dedup();
        keys
    }

    /// The torrent's list came without the item's file: kept, so it is
    /// never looked into or offered for the item again.
    fn note_miss(&self, item: &WishlistItem, offer: &Offer, listed: Option<&str>) {
        for key in Self::miss_keys(item, offer, listed) {
            if let Err(e) = self.lib.cache_put(&key, Self::now(), "1") {
                log::warn!("wishlist: a torrent's miss is not kept: {e}");
            }
        }
    }

    /// The torrent's list was seen before without the item's file.
    fn missed_before(&self, item: &WishlistItem, offer: &Offer) -> bool {
        offer.source == SourceKey::Torrents
            && Self::miss_keys(item, offer, None).iter().any(|k| self.lib.cache_get(k).ok().flatten().is_some())
    }

    /// A torrent's list (path, size) for an item that looks for a better
    /// copy of an album: it has the album's songs. Anything else passes.
    fn torrent_fits(&self, item: &WishlistItem, files: &[(usize, String, u64)]) -> bool {
        let names: Vec<(String, Option<u32>)> = files.iter().map(|(_, p, _)| (p.clone(), None)).collect();
        self.upgrade_folder_fits(item, &names)
    }

    /// The offer did not work out: the next sure one, else the doubtful
    /// ones in «Подтверди», else later when a source turned the item away,
    /// else not found.
    fn wish_next_candidate(&mut self, id: WishlistId) -> Next {
        // Turned down (or done) meanwhile: nothing more for it.
        if self
            .lib
            .get_wishlist_item(id)
            .ok()
            .flatten()
            .is_none_or(|i| matches!(i.status, WishlistStatus::Dismissed | WishlistStatus::Done))
        {
            let w = &mut self.streaming.wish;
            w.cands.remove(&id);
            w.doubtful.remove(&id);
            w.probe.remove(&id);
            return Next::NotFound;
        }
        let next = match self.streaming.wish.cands.get_mut(&id) {
            Some(list) if !list.is_empty() => {
                list.remove(0);
                list.first().cloned()
            }
            _ => None,
        };
        match next {
            Some(c) => {
                let from = offer_from(&c);
                self.wish_fetch(id, c);
                Next::Offer(from)
            }
            None => {
                self.streaming.wish.cands.remove(&id);
                let doubtful = self.streaming.wish.doubtful.remove(&id).unwrap_or_default();
                if !doubtful.is_empty() {
                    self.confirm_after_look(id, doubtful);
                    Next::Doubtful
                } else if self.streaming.wish.turned.get(&id).is_some_and(|t| !t.is_empty()) {
                    self.wish_later(id)
                } else {
                    self.no_torrent(id, true);
                    Next::NotFound
                }
            }
        }
    }

    /// A source gave part of an album: what came is imported, the rest
    /// goes on once the import took it ([`Self::album_goes_on`]).
    fn wish_partial(&mut self, id: WishlistId, offer: Offer, files: Vec<PathBuf>, missing: usize, error: SourceError) {
        let all = files.len() + missing;
        log::warn!("wishlist: {} from {:?}: {} of {all} tracks, the rest: {error}", offer.title, offer.source, files.len());
        let why = if error.is_refusal() {
            self.wish_turned_away(id, &offer, &error);
            why_words(offer.source, &error, "остальное")
        } else {
            self.streaming.wish.refusals.worked(offer.source);
            format!("остальных {missing} у {} нет", source_name(offer.source))
        };
        // A better copy says only what it found and what came of it.
        if !self.is_upgrade(id) {
            let what = self.item_title(id);
            self.wish_line(id, what, format!("Скачано {} из {all} · {why}", files.len()), None);
        }
        self.streaming.wish.partial.insert(id);
        self.wish_files(id, offer, files);
    }

    /// `offer`'s source turned the item's download away with `e`: noted
    /// for the item and the source (set aside when it keeps doing so), and
    /// the item's other offers of that source go (they would be refused too).
    fn wish_turned_away(&mut self, id: WishlistId, offer: &Offer, e: &SourceError) {
        let key = offer.source;
        self.streaming.wish.turned.entry(id).or_default().push((key, e.clone()));
        if let Some(list) = self.streaming.wish.cands.get_mut(&id) {
            let mut first = true;
            list.retain(|o| std::mem::take(&mut first) || o.source != key);
        }
        if let Some(until) = self.streaming.wish.refusals.refused(key, e, Instant::now()) {
            let left = until.saturating_duration_since(Instant::now());
            log::info!("wishlist: {key:?} keeps turning downloads away, set aside for {} min", left.as_secs() / 60);
            let when = super::accounts::hm_after(left).map_or_else(String::new, |hm| format!(" до {hm}"));
            self.streaming.wish.fetch_down.insert(key, format!("Находит, но не скачивает — пропускаю{when}"));
            self.publish_sources();
        }
    }

    /// A download of `key` worked after it had been set aside: the items
    /// that wait for it are searched again now.
    fn wish_wake(&mut self, key: SourceKey) {
        let waiting: Vec<WishlistId> =
            self.streaming.wish.retry_only.iter().filter(|(_, keys)| keys.contains(&key)).map(|(id, _)| *id).collect();
        let now = Self::now();
        for id in waiting {
            self.set_item(id, |i| {
                if i.status == WishlistStatus::New && i.next_try_at.is_some_and(|t| t > now) {
                    i.next_try_at = Some(now);
                }
            });
        }
        self.wish_tick();
    }

    /// «Загрузки → История»: `offer`'s source turned the item away, and
    /// what came of it (`next`): the next source, or the choice in
    /// «Подтверди». A wait has its line from [`Self::wish_later`].
    fn refusal_line(&mut self, id: WishlistId, offer: &Offer, e: &SourceError, next: &Next) {
        let then = match next {
            Next::Offer(from) => format!("дальше — {from}"),
            Next::Doubtful => "есть варианты под вопросом — выбери в «Хочу скачать»".to_owned(),
            Next::Later(_) | Next::NotFound => return,
        };
        if self.is_upgrade(id) {
            return;
        }
        let why = why_words(offer.source, e, self.item_what(id));
        let title = self.item_title(id);
        self.wish_line(id, title, capitalised(&format!("{why} · {then}")), None);
    }

    /// Every offer of the item was turned away (or a source could not be
    /// asked): it is searched again ([`plan_retry`]) [`LATER_TRIES`] times,
    /// then in a week. The item's note and its line in «История» say why
    /// and when which sources are asked.
    fn wish_later(&mut self, id: WishlistId) -> Next {
        let turned = self.streaming.wish.turned.remove(&id).unwrap_or_default();
        let Some(item) = self.lib.get_wishlist_item(id).ok().flatten() else { return Next::NotFound };
        if matches!(item.status, WishlistStatus::Dismissed | WishlistStatus::Done) {
            return Next::NotFound;
        }
        // One reason per source: what it kept doing when it is set aside.
        let now = Instant::now();
        let mut seen: Vec<(SourceKey, SourceError, Option<Duration>)> = Vec::new();
        for (k, e) in turned {
            if seen.iter().any(|(s, ..)| *s == k) {
                continue;
            }
            let aside = self.streaming.wish.refusals.aside(k, now).map(|u| u - now);
            let back = match (e.wait(), aside) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            };
            let why = match aside {
                Some(_) => self.streaming.wish.refusals.why(k).cloned().unwrap_or(e),
                None => e,
            };
            seen.push((k, why, back));
        }
        let partly = match item.scope {
            WishlistScope::Album => match self.album_files(&item) {
                (have, all) if have > 0 && have < all => Some((have, all)),
                _ => None,
            },
            WishlistScope::Track => None,
        };
        let what = match (item.scope, partly) {
            (_, Some(_)) => "остальное",
            (WishlistScope::Album, None) => "этот альбом",
            (WishlistScope::Track, None) => "этот трек",
        };
        let why: Vec<String> = seen.iter().map(|(k, e, _)| why_words(*k, e, what)).collect();
        let n = {
            let n = self.streaming.wish.later.entry(id).or_default();
            *n += 1;
            *n
        };
        if n > LATER_TRIES {
            // Tried enough for now: the weekly search, in every source.
            self.streaming.wish.later.remove(&id);
            self.streaming.wish.retry_only.remove(&id);
            let copy = self.copy_quality(&item).is_some();
            self.no_torrent(id, false);
            // A better copy is looked for when its schedule says, quietly.
            if item.origin == WishlistOrigin::Upgrade {
                return Next::NotFound;
            }
            let reasons = if why.is_empty() { "Источники не дали скачать".to_owned() } else { why.join(", ") };
            let line = capitalised(&format!("{reasons} · поищу снова через неделю"));
            if !copy {
                self.set_item(id, |i| i.note = Some(line.clone()));
                self.publish_wishlist();
            }
            let title = self.item_title(id);
            self.wish_line(id, title, String::new(), Some(line));
            return Next::NotFound;
        }
        let keys: Vec<SourceKey> = seen.iter().map(|(k, ..)| *k).collect();
        let asked = self.streaming.wish.asked.get(&id);
        let others: Vec<(SourceKey, Duration)> = self
            .download_sources()
            .iter()
            .map(|s| s.key())
            .filter(|k| !keys.contains(k))
            .filter_map(|k| asked.and_then(|a| a.get(&k)).map(|t| (k, t.elapsed())))
            .collect();
        let backs: Vec<(SourceKey, Option<Duration>)> = seen.iter().map(|(k, _, b)| (*k, *b)).collect();
        let retry = plan_retry(n, &backs, &others);
        let at = Self::now() + retry.wait.as_millis() as i64;
        self.streaming.wish.retry_only.insert(id, keys);
        let note = later_line(partly, &why, retry.wait, &retry.ask);
        log::info!("wishlist: {id} was turned away ({}), again in {} min", why.join(", "), retry.wait.as_secs() / 60);
        self.set_item(id, |i| {
            i.status = WishlistStatus::New;
            i.next_try_at = Some(at);
            i.note = Some(note.clone());
        });
        if item.origin != WishlistOrigin::Upgrade {
            let title = self.item_title(id);
            self.wish_line(id, title, note, None);
        }
        self.publish_wishlist();
        Next::Later(at)
    }

    /// «этот трек» or «этот альбом», as the item is.
    fn item_what(&self, id: WishlistId) -> &'static str {
        match self.lib.get_wishlist_item(id).ok().flatten().map(|i| i.scope) {
            Some(WishlistScope::Album) => "этот альбом",
            _ => "этот трек",
        }
    }

    /// The item's name for the lines: «Artist — Title».
    fn item_title(&self, id: WishlistId) -> String {
        match self.lib.get_wishlist_item(id).ok().flatten() {
            Some(i) => match (&i.artist, &i.title) {
                (Some(a), Some(t)) => format!("{a} — {t}"),
                (_, Some(t)) => t.clone(),
                _ => "трек".into(),
            },
            None => "трек".into(),
        }
    }

    fn no_torrent(&mut self, id: WishlistId, tell: bool) {
        if self.is_upgrade(id) {
            self.upgrade_none(id);
            return;
        }
        let copy = self.lib.get_wishlist_item(id).ok().flatten().is_some_and(|i| self.copy_quality(&i).is_some());
        let Some(item) = self.set_item(id, |i| {
            i.status = WishlistStatus::NoTorrent;
            i.next_try_at = Some(Self::now() + RETRY_AFTER.as_millis() as i64);
            i.note = Some(if copy { HAS_COPY } else { NOT_FOUND }.into());
        }) else {
            return;
        };
        if tell && !copy && self.state.read().prefs.download_toasts {
            let what = match (&item.artist, &item.title) {
                (Some(a), Some(t)) => format!("{a} — {t}"),
                (_, Some(t)) => t.clone(),
                _ => "трек".into(),
            };
            let action = (item.track_id.is_some() && self.service(Service::Youtube).is_some())
                .then(|| ("Сохранить с YouTube".to_owned(), Command::WishlistSaveFromYoutube(id)));
            self.toast(format!("Не нашёл, где скачать: {what}"), action);
        }
        self.publish_wishlist();
    }

    // ------------------------------------------------------------- user actions

    fn select(&self, select: &v::WishlistSelect) -> Vec<WishlistItem> {
        let all = self.lib.list_wishlist().unwrap_or_default();
        let artist_of = |i: &WishlistItem| -> Option<nikgob_core::model::ArtistId> {
            let rel = i
                .release_id
                .or(i.track_id.and_then(|t| self.lib.get_track(t).ok().flatten()).and_then(|t| t.release_id));
            let from_release = rel
                .and_then(|r| self.lib.get_release(r).ok().flatten())
                .and_then(|r| r.artist_credit.0.first().and_then(|p| p.artist_id));
            from_release.or_else(|| {
                i.track_id
                    .and_then(|t| self.lib.get_track(t).ok().flatten())
                    .and_then(|t| t.artist_credit.0.first().and_then(|p| p.artist_id))
            })
        };
        all.into_iter()
            .filter(|i| match select {
                v::WishlistSelect::Items(ids) => ids.contains(&i.id),
                v::WishlistSelect::Album(r) => {
                    i.release_id == Some(*r)
                        || i.track_id
                            .and_then(|t| self.lib.get_track(t).ok().flatten())
                            .is_some_and(|t| t.release_id == Some(*r))
                }
                v::WishlistSelect::Artist(a) => artist_of(i) == Some(*a),
                v::WishlistSelect::AllWaiting => i.status == WishlistStatus::Waiting,
            })
            .collect()
    }

    pub(crate) fn wish_download(&mut self, select: &v::WishlistSelect) {
        let mut n = 0;
        for item in self.select(select) {
            if matches!(item.status, WishlistStatus::Waiting | WishlistStatus::NoTorrent | WishlistStatus::Failed) {
                self.set_item(item.id, |i| {
                    i.status = WishlistStatus::New;
                    i.next_try_at = None;
                });
                n += 1;
            }
        }
        if n > 0 {
            self.toast(format!("В очереди на скачивание: {n}"), None);
        }
        self.wish_tick();
        self.publish_wishlist();
    }

    pub(crate) fn wish_dismiss(&mut self, ids: &[WishlistId]) {
        let mut title = None;
        let mut n = 0;
        for &id in ids {
            let w = &mut self.streaming.wish;
            w.cands.remove(&id);
            w.doubtful.remove(&id);
            w.probe.remove(&id);
            w.partial.remove(&id);
            w.retry_only.remove(&id);
            w.later.remove(&id);
            w.upgrade_dups.remove(&id);
            // A better copy turned down is never fetched on: its torrent
            // goes with what it downloaded (a direct download's folder goes
            // once it ends, unimported).
            if self.is_upgrade(id) {
                self.cancel_wish_torrent(id);
            }
            self.wish_line_drop(id);
            if let Some(i) = self.set_item(id, |i| {
                i.status = WishlistStatus::Dismissed;
                i.next_try_at = None;
            }) {
                n += 1;
                title = i.title;
            }
        }
        if n > 0 {
            let text = match (n, title) {
                (1, Some(t)) => format!("Не буду качать «{t}»"),
                (n, _) => format!("Не буду качать: {n}"),
            };
            self.toast(text, Some(("Отменить".into(), Command::WishlistRestore(ids.to_vec()))));
        }
        self.publish_wishlist();
    }

    /// `Command::WishlistRestore`: dismissed items come back; an imported
    /// one waits for «скачать» again, the others start.
    pub(crate) fn wish_restore(&mut self, ids: &[WishlistId]) {
        for &id in ids {
            self.set_item(id, |i| {
                if i.status == WishlistStatus::Dismissed {
                    i.status = if i.origin == WishlistOrigin::Import {
                        WishlistStatus::Waiting
                    } else {
                        WishlistStatus::New
                    };
                    i.next_try_at = None;
                }
            });
        }
        self.publish_wishlist();
        self.wish_tick();
    }

    pub(crate) fn wish_retry(&mut self, id: WishlistId) {
        self.streaming.wish.later.remove(&id);
        self.streaming.wish.retry_only.remove(&id);
        let item = self.set_item(id, |i| {
            if i.status != WishlistStatus::Dismissed {
                i.status = WishlistStatus::New;
                i.next_try_at = None;
            }
        });
        // A better copy the user asks for now is searched now, not at the
        // pace of the ones that look by themselves.
        if let Some(item) = item
            && item.origin == WishlistOrigin::Upgrade
            && item.status == WishlistStatus::New
            && self.import.is_some()
            && self.can_download()
        {
            self.streaming.wish.upgrade_runs.push(Instant::now());
            self.wish_search(item);
        }
        self.wish_tick();
        self.publish_wishlist();
    }

    /// Everything not found (or only saved from YouTube) is searched for
    /// again, in every source that is on now.
    pub(crate) fn wish_retry_all(&mut self) {
        let now = Self::now();
        let mut n = 0;
        for status in [WishlistStatus::NoTorrent, WishlistStatus::Failed] {
            for mut item in self.lib.list_wishlist_by_status(status).unwrap_or_default() {
                // Better copies keep their own schedule.
                if item.origin == WishlistOrigin::Upgrade {
                    continue;
                }
                item.status = WishlistStatus::New;
                item.next_try_at = None;
                item.updated_at = now;
                self.put_item(&item);
                n += 1;
            }
        }
        // And what waits to be searched again after a source turned it away.
        for mut item in self.lib.list_wishlist_by_status(WishlistStatus::New).unwrap_or_default() {
            if item.origin != WishlistOrigin::Upgrade && item.next_try_at.is_some_and(|t| t > now) {
                self.streaming.wish.later.remove(&item.id);
                self.streaming.wish.retry_only.remove(&item.id);
                item.next_try_at = None;
                item.updated_at = now;
                self.put_item(&item);
                n += 1;
            }
        }
        if n > 0 {
            self.toast(format!("Ищу заново: {n}"), None);
        }
        self.wish_tick();
        self.publish_wishlist();
    }

    /// "Подтверди" for a wishlist item: `Some(i)` downloads release `i`,
    /// `None` = none of them (searched again in a week).
    pub(crate) fn wish_confirm(&mut self, confirm: u64, option: Option<usize>) {
        let Some(id) = self
            .streaming
            .wish
            .confirm_ids
            .iter()
            .find(|(_, n)| **n == confirm)
            .map(|(id, _)| *id)
        else {
            return;
        };
        let Some(item) = self.lib.get_wishlist_item(id).ok().flatten() else { return };
        let offers = item.candidates_json.as_deref().map(offers_of_json).unwrap_or_default();
        match option.and_then(|i| offers.get(i).cloned()) {
            Some(c) => {
                self.set_item(id, |i| i.status = WishlistStatus::Searching);
                self.streaming.wish.cands.insert(id, vec![c.clone()]);
                self.streaming.wish.doubtful.remove(&id);
                self.wish_fetch(id, c);
            }
            // A source turned the item away: it may give it later.
            None if self.streaming.wish.turned.get(&id).is_some_and(|t| !t.is_empty()) => {
                self.wish_later(id);
            }
            None => self.no_torrent(id, false),
        }
        self.publish_wishlist();
    }

    pub(crate) fn wish_save_from_youtube(&mut self, id: WishlistId) {
        let Some(item) = self.lib.get_wishlist_item(id).ok().flatten() else { return };
        let Some(track) = item.track_id else {
            self.toast("С YouTube сохраняется только трек, не альбом", None);
            return;
        };
        let Some(yt) = self.service(Service::Youtube) else {
            self.toast("YouTube Music выключен", None);
            return;
        };
        let source = self
            .lib
            .sources_of_track(track)
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.kind == SourceKind::Youtube && s.available && s.trashed_at.is_none())
            .max_by_key(|s| s.quality_score);
        let Some(source) = source else {
            if self.can_resolve(track) {
                self.streaming.save_after_resolve.insert(track, id);
                self.begin_resolve(track);
                self.toast("Ищу трек на YouTube Music…", None);
            } else {
                self.toast("Не нашёл этот трек на YouTube Music", None);
            }
            return;
        };
        self.set_item(id, |i| {
            i.status = WishlistStatus::Downloading;
            i.note = Some("Сохраняю с YouTube…".into());
        });
        let dir = self.save_dir();
        self.streaming.spawn(async move {
            let result = yt.save_audio(&source.locator, &dir).await;
            Msg::Wish(WishMsg::Saved { item: id, track, result })
        });
        self.publish_wishlist();
    }

    fn wish_saved(&mut self, id: WishlistId, track: TrackId, result: Result<PathBuf, StreamError>) {
        match result {
            Ok(path) => {
                self.streaming.wish.saving.insert(path.clone(), id);
                let targets = ImportTargets {
                    tracks: vec![track],
                    force: true,
                    ..ImportTargets::default()
                };
                if !self.import_for(vec![path], "Сохранение с YouTube".into(), SourceOrigin::Stream, targets) {
                    self.set_item(id, |i| i.status = WishlistStatus::NoTorrent);
                }
            }
            Err(e) => {
                log::warn!("saving from YouTube: {e}");
                self.toast(format!("Не получилось сохранить с YouTube: {}", super::error_text(&e)), None);
                self.set_item(id, |i| {
                    i.status = WishlistStatus::NoTorrent;
                    i.note = Some("Не получилось сохранить с YouTube".into());
                });
                self.publish_wishlist();
            }
        }
    }

    pub(crate) fn wish_attach(&mut self, id: WishlistId, what: v::WishlistAttach) {
        let Some(item) = self.lib.get_wishlist_item(id).ok().flatten() else { return };
        let tracks = self.targets_of(&item).len();
        let note = match &what {
            v::WishlistAttach::TorrentFile(_) | v::WishlistAttach::Magnet(_) => "Своя раздача",
            v::WishlistAttach::Files(_) | v::WishlistAttach::Folder(_) => "Свой файл",
        };
        let started = match what {
            v::WishlistAttach::Files(paths) => {
                let force = paths.len() == 1 && item.scope == WishlistScope::Track && tracks == 1;
                let targets = self.item_targets(&item, force);
                self.import_for(paths, "Импорт для «Хочу скачать»".into(), SourceOrigin::Inbox, targets)
            }
            v::WishlistAttach::Folder(dir) => {
                let targets = self.item_targets(&item, false);
                self.import_for(vec![dir], "Импорт для «Хочу скачать»".into(), SourceOrigin::Inbox, targets)
            }
            v::WishlistAttach::TorrentFile(path) => match std::fs::read(&path) {
                Ok(bytes) => {
                    self.add_payload_for(TorrentPayload::TorrentFile(bytes), None, None, Some(id));
                    true
                }
                Err(e) => {
                    log::warn!("{}: {e}", path.display());
                    self.toast("Не получилось прочитать .torrent", None);
                    false
                }
            },
            v::WishlistAttach::Magnet(m) => {
                let m = m.trim().to_owned();
                if m.starts_with("magnet:") {
                    self.add_payload_for(TorrentPayload::Magnet(m.clone()), None, Some(m), Some(id));
                    true
                } else {
                    self.toast("Это не magnet-ссылка", None);
                    false
                }
            }
        };
        if started {
            self.set_item(id, |i| {
                i.status = WishlistStatus::Downloading;
                i.note = Some(note.into());
            });
            self.publish_wishlist();
        }
    }

    // ------------------------------------------------------------- results

    /// A file landed (or was a duplicate of a file there): items it
    /// satisfies are done; a copy saved from YouTube keeps its item looking
    /// for a real file.
    /// A file became `track`'s (from `path`): for a better copy, what it
    /// is, read while the file is still there.
    pub(crate) fn wish_file_came(&mut self, track: TrackId, path: &Path) {
        if self.lib.wishlist_for_track(track).unwrap_or_default().iter().any(|i| i.origin == WishlistOrigin::Upgrade)
            || self
                .lib
                .get_track(track)
                .ok()
                .flatten()
                .and_then(|t| t.release_id)
                .is_some_and(|r| self.lib.wishlist_for_release(r).unwrap_or_default().iter().any(|i| i.origin == WishlistOrigin::Upgrade))
        {
            self.upgrade_came(track, path);
        }
    }

    pub(crate) fn wish_imported(&mut self, track: TrackId, path: &Path) {
        // What the files of the better-copy items are has changed.
        self.streaming.wish.upgrade_have.clear();
        if let Some(item) = self.streaming.wish.saving.remove(path) {
            let _ = std::fs::remove_file(path);
            self.set_item(item, |i| {
                i.status = WishlistStatus::NoTorrent;
                i.next_try_at = Some(Self::now() + RETRY_AFTER.as_millis() as i64);
                i.note = Some(HAS_COPY.into());
            });
            self.publish_wishlist();
            return;
        }
        let Some(src) = self.lib.active_source(track).ok().flatten() else { return };
        if !src.kind.is_local() || src.quality_score <= score::SAVED_STREAM {
            return;
        }
        let mut changed = false;
        let mut items = self.lib.wishlist_for_track(track).unwrap_or_default();
        let release = self.lib.get_track(track).ok().flatten().and_then(|t| t.release_id);
        if let Some(r) = release {
            items.extend(
                self.lib
                    .wishlist_for_release(r)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|i| {
                        i.scope == WishlistScope::Album
                            && matches!(i.status, WishlistStatus::Downloading | WishlistStatus::Confirm)
                    }),
            );
        }
        for i in items {
            if matches!(i.status, WishlistStatus::Done | WishlistStatus::Dismissed) {
                continue;
            }
            // A better copy is done once every file of it is good enough.
            if i.origin == WishlistOrigin::Upgrade {
                if self.upgrade_reached(&i) {
                    self.upgrade_done(&i);
                    changed = true;
                }
                continue;
            }
            // An album is «Скачано» once every one of its tracks has a file
            // and nothing waits in «Подтверди».
            if i.scope == WishlistScope::Album && !self.album_complete(&i) {
                continue;
            }
            self.set_item(i.id, |i| {
                i.status = WishlistStatus::Done;
                i.next_try_at = None;
                i.note = Some("Скачано".into());
            });
            self.streaming.wish.asked.remove(&i.id);
            self.wish_line_drop(i.id);
            changed = true;
        }
        if changed {
            self.publish_wishlist();
            self.wish_tick();
        }
    }

    /// Lines of `item`'s album that have a file, and all its lines (the
    /// count its page says). An album that went (folded into its fuller
    /// edition) is found again by name, and the item follows it.
    fn album_files(&self, item: &WishlistItem) -> (usize, usize) {
        nikgob_import::wished_release(&self.lib, item)
            .ok()
            .flatten()
            .and_then(|r| nikgob_import::release_files(&self.lib, r.id).ok())
            .unwrap_or_default()
    }

    fn album_complete(&self, item: &WishlistItem) -> bool {
        let (have, all) = self.album_files(item);
        all > 0 && have == all && !self.files_wait(item.id)
    }

    /// Files downloaded for the item wait in «Подтверди».
    fn files_wait(&self, item: WishlistId) -> bool {
        self.import
            .as_ref()
            .is_some_and(|svc| svc.pending().iter().any(|p| p.wishlist == Some(item)))
    }

    /// A file downloaded for the item waits in «Подтверди»: the item says
    /// so and leads there, it is not «Скачано».
    pub(crate) fn wish_parked(&mut self, item: WishlistId) {
        let changed = self.set_item(item, |i| {
            if !matches!(i.status, WishlistStatus::Dismissed) {
                i.status = WishlistStatus::Confirm;
                i.note = Some(FILES_WAIT.into());
            }
        });
        if changed.is_some() {
            self.publish_wishlist();
        }
    }

    /// The item's import ended (or a file of it was answered in
    /// «Подтверди»): «Подтверди» while files wait there, «Скачано» when its
    /// track (every track of its album) has a file, else what is missing.
    pub(crate) fn wish_settle(&mut self, id: WishlistId) {
        let Some(item) = self.lib.get_wishlist_item(id).ok().flatten() else { return };
        if self.files_wait(id) {
            if !matches!(item.status, WishlistStatus::Dismissed) {
                self.wish_parked(id);
            }
            return;
        }
        // A direct download was imported (or turned down): its copy goes.
        self.drop_direct(id);
        if matches!(item.status, WishlistStatus::Dismissed) {
            return;
        }
        if item.origin == WishlistOrigin::Upgrade {
            self.upgrade_settle(&item);
            return;
        }
        let partial = self.streaming.wish.partial.remove(&id);
        let (status, note) = match item.scope {
            WishlistScope::Album => match self.album_files(&item) {
                (have, all) if all > 0 && have == all => (WishlistStatus::Done, "Скачано".to_owned()),
                // A source gave part of the album: the rest from the next
                // offer (it leaves out what came), or later when a source
                // turned it away.
                _ if partial && self.album_goes_on(id) => return,
                (have, all) if have > 0 => (WishlistStatus::Done, format!("Скачано {have} из {all}")),
                // Nothing of the album landed: still on its way, or failed.
                _ if item.status == WishlistStatus::Confirm => (WishlistStatus::Downloading, String::new()),
                _ => return,
            },
            WishlistScope::Track => {
                let local = item
                    .track_id
                    .and_then(|t| self.lib.active_source(t).ok().flatten())
                    .is_some_and(|s| s.kind.is_local() && s.quality_score > score::SAVED_STREAM);
                if !local {
                    if item.status == WishlistStatus::Confirm && item.note.as_deref() == Some(FILES_WAIT) {
                        self.set_item(id, |i| i.status = WishlistStatus::Downloading);
                        self.publish_wishlist();
                    }
                    // A direct download that did not become the track's
                    // file (another song, a fake): the next offer.
                    if item.status == WishlistStatus::Downloading
                        && !self.torrent_of(id)
                        && self.streaming.wish.from.remove(&id).is_some()
                    {
                        self.wish_next_candidate(id);
                    }
                    return;
                }
                if let (Some(track), Some(names)) = (item.track_id, self.streaming.wish.relabel.remove(&id)) {
                    self.relabel(track, names);
                }
                // What came, honestly: «Скачано · FLAC из MP3 ~320».
                let got = item
                    .track_id
                    .and_then(|t| self.lib.get_track_row(t).ok().flatten())
                    .and_then(|r| r.codec.as_ref().map(|c| nikgob_core::quality::row_label(c, r.check.as_ref())));
                (WishlistStatus::Done, got.map_or_else(|| "Скачано".to_owned(), |q| format!("Скачано · {q}")))
            }
        };
        if status == WishlistStatus::Done {
            let w = &mut self.streaming.wish;
            w.cands.remove(&id);
            w.doubtful.remove(&id);
            w.from.remove(&id);
            w.relabel.remove(&id);
            w.turned.remove(&id);
            w.later.remove(&id);
            w.retry_only.remove(&id);
            w.asked.remove(&id);
            self.wish_line_drop(id);
        }
        self.set_item(id, |i| {
            i.status = status;
            i.next_try_at = None;
            if !note.is_empty() {
                i.note = Some(note);
            }
        });
        self.publish_wishlist();
        self.wish_tick();
    }

    /// The rest of an album a source gave part of goes on: the next offer
    /// of another source, or later when the source turned it away. False:
    /// nothing more to try (it settles as it is).
    fn album_goes_on(&mut self, id: WishlistId) -> bool {
        let more = self.streaming.wish.cands.get(&id).is_some_and(|l| l.len() > 1);
        let turned = self.streaming.wish.turned.get(&id).is_some_and(|t| !t.is_empty());
        if !more && !turned {
            return false;
        }
        self.streaming.wish.from.remove(&id);
        // The doubtful ones are not asked about for the rest.
        self.streaming.wish.doubtful.remove(&id);
        if more {
            self.wish_next_candidate(id);
        } else {
            self.streaming.wish.cands.remove(&id);
            self.wish_later(id);
        }
        true
    }

    /// The file of an uploader's song takes its performer's names: the
    /// artist, the title, a single of its own (the cover is found for it).
    fn relabel(&mut self, track: TrackId, (artist, title): (String, String)) {
        let Some(svc) = &self.import else { return };
        log::info!("wishlist: {track} is {artist} — {title}, not the uploader's");
        let importer = svc.importer().clone();
        let tx = svc.internal();
        svc.spawn_blocking(move || {
            let changes = nikgob_import::TagChanges {
                album: Some(title.clone()),
                album_artist: Some(artist.clone()),
                title: Some(title),
                artist: Some(artist),
                ..Default::default()
            };
            let r = importer.retag(track, changes).map(|_| ()).map_err(|e| e.to_string());
            let _ = tx.send(crate::import::Internal::Relabeled(track, r));
        });
    }

    /// An import of saved stream audio failed: the item goes back.
    pub(crate) fn wish_import_failed(&mut self, path: &Path) {
        if let Some(item) = self.streaming.wish.saving.remove(path) {
            let _ = std::fs::remove_file(path);
            self.set_item(item, |i| {
                i.status = WishlistStatus::NoTorrent;
                i.note = Some("Не получилось сохранить с YouTube".into());
            });
            self.publish_wishlist();
        }
    }

    /// How far the downloads of sources other than torrents are, on their
    /// rows (each second; the torrents' come with theirs).
    pub(crate) fn wish_poll_direct(&mut self) {
        let live = self.streaming.wish.direct_progress();
        let mut next = self.streaming.wish.progress.clone();
        next.retain(|k, _| !self.streaming.wish.direct.contains_key(k) || live.contains_key(k));
        next.extend(live);
        if next == self.streaming.wish.progress {
            return;
        }
        self.streaming.wish.progress = next.clone();
        self.publish(|s| {
            for r in &mut s.downloads.wishlist.items {
                r.progress = next.get(&r.id).copied().filter(|_| r.status == WishlistStatus::Downloading);
            }
        });
    }

    /// The item downloads through a torrent of the torrent client.
    fn torrent_of(&self, id: WishlistId) -> bool {
        self.lib.list_downloads().unwrap_or_default().iter().any(|d| d.wishlist_id == Some(id))
    }

    /// The torrent of a turned-down item stops, with what it downloaded.
    fn cancel_wish_torrent(&mut self, id: WishlistId) {
        let hashes: Vec<String> = self
            .lib
            .list_downloads()
            .unwrap_or_default()
            .into_iter()
            .filter(|d| d.wishlist_id == Some(id))
            .map(|d| d.info_hash)
            .collect();
        if hashes.is_empty() {
            return;
        }
        log::info!("wishlist: {id} turned down, its torrent goes");
        self.with_client(Box::new(move |w, c| {
            for h in hashes {
                let _ = w.lib.delete_download(&h);
                if let Some(t) = c.list().into_iter().find(|t| t.info_hash == h) {
                    let tid = t.id;
                    let c = c.clone();
                    w.spawn_torrent_op(async move { c.remove(tid, true).await.map_err(|e| e.to_string()) });
                }
            }
        }));
    }

    /// After a restart: searches that were running start over, and so do
    /// downloads of sources other than torrents (they do not go on), whose
    /// folders go unless files of them wait in «Подтверди».
    pub(crate) fn wish_restart(&mut self) {
        for i in self.lib.list_wishlist_by_status(WishlistStatus::Searching).unwrap_or_default() {
            self.set_item(i.id, |i| i.status = WishlistStatus::New);
        }
        for i in self.lib.list_wishlist_by_status(WishlistStatus::Downloading).unwrap_or_default() {
            if !self.torrent_of(i.id) && !self.files_wait(i.id) {
                self.set_item(i.id, |i| {
                    i.status = WishlistStatus::New;
                    i.next_try_at = None;
                });
            }
        }
        if let Some(svc) = &self.import {
            let root = svc.config().downloads_dir.join(SOURCES_DIR);
            let waiting: Vec<String> = svc.pending().iter().filter_map(|p| p.wishlist.map(|w| w.to_string())).collect();
            for dir in std::fs::read_dir(&root).into_iter().flatten().flatten() {
                let name = dir.file_name().to_string_lossy().into_owned();
                if !waiting.contains(&name) {
                    let _ = std::fs::remove_dir_all(dir.path());
                }
            }
        }
        // Files that waited in «Подтверди» may be gone since.
        let probed = self.lib.cache_get(PROBED_KEY).ok().flatten().is_some();
        for i in self.lib.list_wishlist_by_status(WishlistStatus::Confirm).unwrap_or_default() {
            if i.note.as_deref() == Some(FILES_WAIT) {
                if !self.files_wait(i.id) {
                    self.wish_settle(i.id);
                }
                continue;
            }
            // Offers the matching takes no more (it got stricter since),
            // or torrents of a track shown before their lists were looked
            // into (once): searched for again rather than asked about.
            let offers = i.candidates_json.as_deref().map(offers_of_json).unwrap_or_default();
            let unlooked = !probed && i.scope == WishlistScope::Track && offers.iter().any(|o| o.source == SourceKey::Torrents);
            // A better copy of an album shown as a folder without its songs.
            let strangers = offers
                .iter()
                .filter_map(crate::soulseek::folder_listing)
                .any(|files| !self.upgrade_folder_fits(&i, &files));
            if unlooked || strangers || rank(offers.clone(), &self.want_of(&i), &[]).len() < offers.len() {
                self.set_item(i.id, |i| {
                    i.status = WishlistStatus::New;
                    i.next_try_at = None;
                    i.candidates_json = None;
                });
            }
        }
        if !probed {
            let _ = self.lib.cache_put(PROBED_KEY, Self::now(), "1");
        }
    }

    // ------------------------------------------------------------- view

    pub(crate) fn publish_wishlist(&mut self) {
        // Downloaded already, or a track of an album item: not a row of
        // its own.
        let covered = self.lib.wishlist_covered().unwrap_or_default();
        let mut items = self.lib.list_wishlist().unwrap_or_default();
        items.retain(|i| !covered.contains(&i.id));
        items.sort_by(|a, b| {
            status_rank(a.status)
                .cmp(&status_rank(b.status))
                .then(b.created_at.cmp(&a.created_at))
        });
        let now = Self::now();
        let youtube = self.service(Service::Youtube).is_some();
        let can_search = self.can_download();
        // Albums and artists repeat over the items (an album of 12 = 12
        // items): each is read once per publish.
        let mut releases: std::collections::HashMap<nikgob_core::model::ReleaseId, Option<nikgob_core::model::Release>> =
            std::collections::HashMap::new();
        let mut artist_names: std::collections::HashMap<nikgob_core::model::ArtistId, Option<String>> =
            std::collections::HashMap::new();
        let mut rows = Vec::new();
        let mut by_album: Vec<v::WishlistGroup> = Vec::new();
        let mut by_artist: Vec<v::WishlistGroup> = Vec::new();
        let mut counts: Vec<(WishlistStatus, u32)> = Vec::new();
        let mut confirms = Vec::new();
        let mut covers = Vec::new();
        for item in &items {
            let track = item.track_id.and_then(|t| self.lib.get_track(t).ok().flatten());
            let release_id = item.release_id.or(track.as_ref().and_then(|t| t.release_id));
            let release = release_id.and_then(|r| {
                releases
                    .entry(r)
                    .or_insert_with(|| self.lib.get_release(r).ok().flatten())
                    .clone()
            });
            let artist_id = release
                .as_ref()
                .and_then(|r| r.artist_credit.0.first().and_then(|p| p.artist_id))
                .or_else(|| track.as_ref().and_then(|t| t.artist_credit.0.first().and_then(|p| p.artist_id)));
            let artist = item.artist.clone().unwrap_or_default();
            match counts.iter_mut().find(|(s, _)| *s == item.status) {
                Some((_, n)) => *n += 1,
                None => counts.push((item.status, 1)),
            }
            let waiting = u32::from(item.status == WishlistStatus::Waiting);
            if item.status != WishlistStatus::Dismissed {
                if let Some(r) = &release {
                    match by_album.iter_mut().find(|g| g.release == Some(r.id)) {
                        Some(g) => {
                            g.items.push(item.id);
                            g.waiting += waiting;
                        }
                        None => by_album.push(v::WishlistGroup {
                            title: r.title.clone(),
                            subtitle: r.artist_credit.display(),
                            release: Some(r.id),
                            artist: artist_id,
                            items: vec![item.id],
                            waiting,
                        }),
                    }
                }
                if let Some(a) = artist_id {
                    match by_artist.iter_mut().find(|g| g.artist == Some(a)) {
                        Some(g) => {
                            g.items.push(item.id);
                            g.waiting += waiting;
                        }
                        None => by_artist.push(v::WishlistGroup {
                            title: artist_names
                                .entry(a)
                                .or_insert_with(|| self.lib.get_artist(a).ok().flatten().map(|a| a.name))
                                .clone()
                                .unwrap_or_else(|| artist.clone()),
                            subtitle: String::new(),
                            release: None,
                            artist: Some(a),
                            items: vec![item.id],
                            waiting,
                        }),
                    }
                }
            }
            let files_wait = item.status == WishlistStatus::Confirm && self.files_wait(item.id);
            let copy = item.scope == WishlistScope::Track
                && !matches!(item.status, WishlistStatus::Done | WishlistStatus::Dismissed)
                && self.copy_quality(item).is_some();
            let source = (item.status == WishlistStatus::Downloading)
                .then(|| self.streaming.wish.from.get(&item.id).cloned())
                .flatten();
            if item.status == WishlistStatus::Confirm && !files_wait {
                confirms.push(self.wish_confirm_view(item));
            }
            let missing = match (item.scope, item.status) {
                (WishlistScope::Album, WishlistStatus::Done) => {
                    let (have, all) = self.album_files(item);
                    (all - have.min(all)) as u32
                }
                _ => 0,
            };
            let next_try = item
                .next_try_at
                .filter(|_| item.status == WishlistStatus::NoTorrent)
                .map(|at| {
                    let days = ((at - now).max(0) as u64).div_ceil(86_400_000);
                    format!("поищу снова через {days} дн.")
                });
            let upgrade = self.upgrade_row_words(item, files_wait, next_try.as_deref(), can_search);
            rows.push(v::WishlistRow {
                id: item.id,
                title: item.title.clone().unwrap_or_default(),
                artist,
                album: (item.scope == WishlistScope::Track)
                    .then(|| release.as_ref().map(|r| r.title.clone()))
                    .flatten(),
                track: item.track_id,
                release: release_id,
                artist_id,
                status: item.status,
                scope: item.scope,
                origin: item.origin,
                note: item.note.clone(),
                next_try,
                attempts: item.attempts,
                can_save_from_youtube: youtube
                    && item.track_id.is_some()
                    && item.origin != WishlistOrigin::Upgrade
                    && !matches!(item.status, WishlistStatus::Done | WishlistStatus::Dismissed),
                cover: None,
                progress: (item.status == WishlistStatus::Downloading)
                    .then(|| {
                        self.streaming
                            .wish
                            .progress
                            .get(&item.id)
                            .copied()
                            .or_else(|| self.streaming.wish.live.lock().get(&item.id).copied())
                    })
                    .flatten(),
                confirm_files: files_wait,
                missing,
                has_copy: copy,
                source,
                upgrade,
            });
            covers.push(release_id);
        }
        counts.sort_by_key(|(s, _)| status_rank(*s));
        let view = v::WishlistView {
            loaded: true,
            items: rows,
            by_album,
            by_artist,
            counts,
            can_search,
        };
        let mut view = view;
        for (row, release) in view.items.iter_mut().zip(covers) {
            row.cover = release.and_then(|r| self.res.release_cover(r));
        }
        self.publish(|s| {
            s.downloads.wishlist = view;
            s.downloads.confirm.retain(|c| c.id & WISH_ID == 0);
            s.downloads.confirm.extend(confirms);
        });
    }

    fn wish_confirm_view(&mut self, item: &WishlistItem) -> v::Confirm {
        let id = match self.streaming.wish.confirm_ids.get(&item.id) {
            Some(n) => *n,
            None => {
                self.streaming.wish.next_confirm += 1;
                let n = WISH_ID | self.streaming.wish.next_confirm;
                self.streaming.wish.confirm_ids.insert(item.id, n);
                n
            }
        };
        if item.origin == WishlistOrigin::Upgrade {
            return self.upgrade_confirm_view(item, id);
        }
        let offers = item.candidates_json.as_deref().map(offers_of_json).unwrap_or_default();
        let what = match (&item.artist, &item.title) {
            (Some(a), Some(t)) => format!("{t} — {a}"),
            (_, Some(t)) => t.clone(),
            _ => "Хочу скачать".into(),
        };
        v::Confirm {
            id,
            title: what,
            origin: match item.scope {
                WishlistScope::Track => "Хочу скачать · трек".into(),
                WishlistScope::Album => "Хочу скачать · альбом".into(),
            },
            options: offers
                .iter()
                .enumerate()
                .map(|(i, o)| v::ConfirmOption {
                    label: o.title.clone(),
                    detail: o.detail(),
                    // Without seeders it may never come.
                    recommended: Some(i) == offers.iter().position(|o| o.seeders != Some(0)),
                })
                .collect(),
            skip_label: "Не то — поищу позже".into(),
            question: match item.scope {
                WishlistScope::Track => "Что из этого — этот трек?".into(),
                WishlistScope::Album => "Что из этого — этот альбом?".into(),
            },
            accept_label: "Скачать".into(),
            pick: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_core::indexer::CandidateHandle;

    /// One refusal is not yet a pattern; the second sets the source aside,
    /// for at least as long as it asked to wait; a download that works ends it.
    #[test]
    fn a_source_is_set_aside_after_refusing_again() {
        let mut r = Refusals::default();
        let now = Instant::now();
        let e = SourceError::Http { status: 403 };
        assert_eq!(r.refused(SourceKey::Tidal, &e, now), None);
        assert_eq!(r.aside(SourceKey::Tidal, now), None);
        let until = r.refused(SourceKey::Tidal, &e, now).unwrap();
        assert_eq!(until - now, ASIDE_FIRST);
        assert_eq!(r.aside(SourceKey::Tidal, now), Some(until));
        assert_eq!(r.aside(SourceKey::Qobuz, now), None, "one source only");
        let long = SourceError::RateLimited { retry_after: Some(Duration::from_secs(3 * 3600)) };
        assert_eq!(r.refused(SourceKey::Tidal, &long, now).unwrap() - now, Duration::from_secs(3 * 3600));
        assert_eq!(r.why(SourceKey::Tidal), Some(&long));
        assert!(r.worked(SourceKey::Tidal), "it was set aside");
        assert_eq!(r.aside(SourceKey::Tidal, now), None);
        assert!(!r.worked(SourceKey::Tidal));
    }

    const MIN: Duration = Duration::from_secs(60);

    /// The first retry: five minutes, unless every source that said no
    /// asked for longer; then not before the first of them is back.
    #[test]
    fn a_retry_waits_for_the_first_source_back() {
        let r = plan_retry(1, &[(SourceKey::Tidal, None)], &[]);
        assert_eq!(r, Retry { wait: LATER_FIRST, ask: vec![SourceKey::Tidal] });
        // Qobuz is out of its day's requests for 6 h, Tidal set aside 30 min:
        // Tidal's half hour, and only Tidal is named.
        let r = plan_retry(1, &[(SourceKey::Qobuz, Some(360 * MIN)), (SourceKey::Tidal, Some(30 * MIN))], &[]);
        assert_eq!(r, Retry { wait: 30 * MIN, ask: vec![SourceKey::Tidal] });
        // One of them can be asked any time: the backoff, doubling.
        let r = plan_retry(3, &[(SourceKey::Qobuz, Some(360 * MIN)), (SourceKey::Tidal, None)], &[]);
        assert_eq!(r, Retry { wait: 20 * MIN, ask: vec![SourceKey::Tidal] });
        // Never past a day, never more than the longest backoff otherwise.
        assert_eq!(plan_retry(1, &[(SourceKey::Qobuz, Some(72 * 60 * MIN))], &[]).wait, ASIDE_LONGEST);
        assert_eq!(plan_retry(30, &[(SourceKey::Tidal, None)], &[]).wait, LATER_LONGEST);
    }

    /// The sources that had nothing for the item are asked again with the
    /// retry once their answer is six hours old: Soulseek answered 5 h 50
    /// ago and the retry is in 20 min, torrents a minute ago.
    #[test]
    fn a_retry_asks_again_what_had_nothing_long_ago() {
        let others = [(SourceKey::Soulseek, 350 * MIN), (SourceKey::Torrents, MIN)];
        let r = plan_retry(3, &[(SourceKey::Tidal, None)], &others);
        assert_eq!(r.ask, [SourceKey::Tidal, SourceKey::Soulseek]);
    }

    /// The reasons in words a listener understands, the source named.
    #[test]
    fn why_a_source_said_no_in_plain_words() {
        let t = "этот трек";
        assert_eq!(why_words(SourceKey::Tidal, &SourceError::Http { status: 403 }, t), "Tidal сейчас не отдаёт этот трек");
        assert_eq!(why_words(SourceKey::Tidal, &SourceError::Parse("a DASH manifest".into()), t), "Tidal сейчас не отдаёт этот трек");
        assert_eq!(why_words(SourceKey::Qobuz, &SourceError::Unreachable("dns error".into()), t), "сервер Qobuz недоступен");
        assert_eq!(why_words(SourceKey::Qobuz, &SourceError::Http { status: 522 }, t), "сервер Qobuz недоступен");
        assert_eq!(why_words(SourceKey::Khinsider, &SourceError::Blocked("a bot check".into()), t), "khinsider не пускает");
        assert_eq!(why_words(SourceKey::Torrents, &SourceError::Unreachable("x".into()), t), "сервер Prowlarr недоступен");
        let short = SourceError::RateLimited { retry_after: Some(MIN) };
        assert_eq!(why_words(SourceKey::Qobuz, &short, t), "Qobuz просит подождать");
        let long = SourceError::RateLimited { retry_after: Some(300 * MIN) };
        assert!(why_words(SourceKey::Qobuz, &long, t).starts_with("Qobuz просит подождать до "));
    }

    /// One line says it all: what is here, why, when and whom it asks.
    #[test]
    fn the_line_of_a_wait() {
        let why = ["Tidal сейчас не отдаёт остальное".to_owned(), "сервер Qobuz недоступен".to_owned()];
        let line = later_line(Some((1, 10)), &why, 30 * MIN, &[SourceKey::Tidal, SourceKey::Soulseek]);
        assert!(line.starts_with("Есть 1 из 10 · Tidal сейчас не отдаёт остальное, сервер Qobuz недоступен · "), "{line}");
        assert!(line.ends_with(" спрошу Tidal, Soulseek"), "{line}");
        let line = later_line(None, &["сервер Qobuz недоступен".to_owned()], 5 * MIN, &[]);
        assert!(line.starts_with("Сервер Qobuz недоступен · попробую снова "), "{line}");
    }

    #[test]
    fn offers_saved_before_the_sources_still_read() {
        let old = vec![Candidate {
            id: "1".into(),
            title: "Кино - Группа крови (1988) [FLAC]".into(),
            indexer_name: "RuTracker".into(),
            size_bytes: Some(400 << 20),
            seeders: Some(9),
            leechers: None,
            published: None,
            categories: vec![3040],
            info_url: None,
            info_hash: None,
            handle: CandidateHandle("{}".into()),
        }];
        let offers = offers_of_json(&serde_json::to_string(&old).unwrap());
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].source, SourceKey::Torrents);
        assert_eq!(offers[0].detail(), "FLAC · 400 МБ · 9 сид. · RuTracker");
        assert_eq!(candidate_of(&offers[0]).unwrap(), old[0]);
        let new = offers_of_json(&serde_json::to_string(&offers).unwrap());
        assert_eq!(new, offers);
    }
}
