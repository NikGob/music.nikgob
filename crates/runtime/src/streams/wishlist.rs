//! The wishlist pipeline: a
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
use nikgob_core::i18n::status::{AtWords, MasterWords, OfferWords, QualityWords, Refusal, StoredLine, What, WishNote};
use nikgob_core::i18n::{Locale, status_in};
use nikgob_core::model::{
    ReleaseId, SourceKind, SourceOrigin, TrackId, WishlistId, WishlistItem, WishlistOrigin,
    WishlistScope, WishlistStatus, score,
};
use nikgob_core::source::{Fetched, Offer, Source, SourceError, SourceKey, Want, WantScope};
use nikgob_core::streaming::{EntityKind, Service, StreamError};
use nikgob_import::ImportTargets;
use nikgob_metadata::text::{artist_key, title_key};
use nikgob_sources::select::{pick, rank};
use nikgob_sources::torrent::{candidate_of, image_rip, look_first, offer_of, pick_files};
use nikgob_view::{self as v, Command};

use super::Msg;
use crate::Worker;
use retry::{offers_of_json, picked_fits};

pub(super) mod album_better;
mod backoff;
mod upgrade;

mod retry;
mod items;
mod sources;
mod pipeline;
mod actions;
mod settle;

#[cfg(test)]
mod tests;


/// Bit of the "Подтверди" ids of wishlist items (import ids use bit 48,
/// torrent jobs small numbers).
pub(crate) const WISH_ID: u64 = 1 << 47;
/// An item without a torrent is searched again after this.
const RETRY_AFTER: Duration = Duration::from_secs(7 * 24 * 3600);
/// Offers shown in "Подтверди".
const CONFIRM_OPTIONS: usize = 3;
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
        /// Those of them sitting out, not asked (told as what set them aside).
        paused: Vec<SourceKey>,
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
        /// The run it was ([`crate::running::Runs::reserve`]): a save
        /// stopped or replaced meanwhile is dropped.
        run: u64,
        result: Result<PathBuf, StreamError>,
    },
    /// A stopped download's task has ended ([`Worker::stop_run`]).
    RunStopped {
        item: WishlistId,
        token: u64,
    },
    /// The files of a better copy of an album asked for by hand, read
    /// ([`album_better`]).
    AlbumCame {
        item: WishlistId,
        origin: SourceOrigin,
        files: Vec<album_better::CameFile>,
        /// Disc images among them, as their cue sheets say.
        images: bool,
    },
    /// «Отменить» of such a copy ran: `left` could not come back.
    AlbumRestored {
        release: ReleaseId,
        done: usize,
        failed: usize,
        left: Vec<nikgob_import::Replacement>,
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
    /// Where each item's running download is (its torrent's, its other
    /// source's, its import's), as the last poll found it: «Хочу скачать»
    /// and «Активные» both show these ([`crate::running`]).
    pub(crate) transfers: HashMap<WishlistId, v::Transfer>,
    /// The torrents' part of them, from the torrent client's last poll.
    pub(crate) torrent_transfers: HashMap<WishlistId, v::Transfer>,
    /// The downloads of the other sources (and saves from YouTube) running.
    pub(crate) runs: crate::running::Runs,
    /// Folders of the downloads of sources other than torrents, by item;
    /// they go once the import took the files.
    direct: HashMap<WishlistId, PathBuf>,
    /// How far those downloads are, as their tasks write it: the bytes
    /// here and of the whole.
    live: Arc<parking_lot::Mutex<HashMap<WishlistId, (u64, Option<u64>)>>>,
    /// Sources that could not answer their last search, and why.
    pub(crate) down: HashMap<SourceKey, SourceError>,
    /// Sources whose last download failed though they found it, and why.
    pub(crate) fetch_down: HashMap<SourceKey, SourceError>,
    /// Web sources sitting out after their searches turned us away (kept
    /// across restarts, [`backoff`]).
    pauses: backoff::Pauses,
    /// Each item's searches of its last day (kept across restarts).
    tries: backoff::DayTries,
    /// When a time told in «Источники» passes: they are told again.
    status_until: Option<Instant>,
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
    /// Items that waited in «Подтверди» with nothing to choose and search
    /// again now ([`Worker::wish_unstick`]).
    unstuck: Unstuck,
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
    upgrade_from: HashMap<WishlistId, QualityWords>,
    /// and what its files are now (until a file lands),
    upgrade_have: HashMap<WishlistId, Option<upgrade::HaveWords>>,
    /// what a copy that came and was no better really is («FLAC из MP3
    /// ~128»), until its import ended,
    upgrade_dups: HashMap<WishlistId, QualityWords>,
    /// and the format of the file that came for a track (its master's, when
    /// the library keeps an Opus of it), for the words once it is done.
    upgrade_came: HashMap<TrackId, MasterWords>,
    /// The offer an item's file comes from, as the importer notes a place
    /// that gave another song ([`offer_key`]), while it downloads.
    offer_keys: HashMap<WishlistId, String>,
    /// Items a file of which held another song's audio: once its import
    /// ends, the item looks again without that place.
    wrong: HashSet<WishlistId>,
    /// Better copies of albums asked for by hand ([`album_better`]).
    album: album_better::AlbumBetter,
}

/// «Не качать» (or a cancelled download) on an item the user's own
/// «Скачать» made takes it out of «Хочу скачать» altogether: it was there
/// only for that click. Any other item (a like's, an import's, a better
/// copy's) is turned down and can come back; one done stays done.
pub(crate) fn cancel_drops(item: &WishlistItem) -> bool {
    item.origin == WishlistOrigin::Manual && item.status != WishlistStatus::Done
}

/// An offer as a place a file comes from (`soulseek:<user\path>`,
/// `torrents:<id>`): what is not taken again for a track once a file from
/// it held another song.
pub(crate) fn offer_key(offer: &Offer) -> String {
    format!("{}:{}", offer.source.as_str(), offer.id)
}

/// The code of an item's note, as `wishlist.note` keeps it.
pub(crate) fn note_code(note: nikgob_core::i18n::status::WishNote) -> String {
    note.code().to_owned()
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

/// Items that waited in «Подтверди» with nothing to choose, searching
/// again once: one recovery each, which ends once the item is answered
/// (a card with choices), on its way, found or not, or gone.
#[derive(Default)]
pub(crate) struct Unstuck(HashSet<WishlistId>);

impl Unstuck {
    /// An empty card of `id`: true — search again (a recovery starts);
    /// false — it was searching again already and came back empty, so it is
    /// not found (and its recovery ends).
    fn empty(&mut self, id: WishlistId) -> bool {
        if self.0.insert(id) {
            true
        } else {
            self.0.remove(&id);
            false
        }
    }

    /// The items as they are now (status, and a card with choices): a
    /// recovery goes on only while its item searches or asks again.
    fn seen(&mut self, items: &HashMap<WishlistId, (WishlistStatus, bool)>) {
        self.0.retain(|id| {
            items.get(id).is_some_and(|(s, answered)| {
                !answered && matches!(s, WishlistStatus::New | WishlistStatus::Searching | WishlistStatus::Confirm)
            })
        });
    }
}

/// An item left on its way with nothing bringing it a file (no torrent,
/// no download, no import, no files waiting in «Подтверди») — after a
/// restart, say: it is searched for again rather than «качается» for ever.
fn stalled(status: WishlistStatus, note: Option<&str>, moving: bool) -> bool {
    let parked = note.is_some_and(|n| files_wait_note(Some(n)));
    !moving && (status == WishlistStatus::Downloading || (status == WishlistStatus::Confirm && parked))
}

/// Whether an item's stored note says its files wait in «Подтверди» (the
/// words of older versions still read as that).
fn files_wait_note(note: Option<&str>) -> bool {
    use nikgob_core::i18n::status::{WishNote, wish_note_of};
    note.and_then(wish_note_of) == Some(WishNote::FilesWait)
}

/// What an offer that did not work out led to.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Next {
    /// The next offer, of this source.
    Offer(SourceKey),
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
    /// The direct downloads' bytes, for the rows.
    pub(crate) fn direct_bytes(&self) -> HashMap<WishlistId, (u64, Option<u64>)> {
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

/// New likes up to this many take their wishlist items at once.
const WISHES_AT_ONCE: usize = 48;
/// How long the loop gives the waiting likes in one turn.
const WISHES_SLICE: std::time::Duration = std::time::Duration::from_millis(15);
