//! Better quality later: the library's files below the quality the user
//! wants ([`v::Prefs::upgrade_below`]) are searched for again on a schedule
//! ([`v::Prefs::upgrade_every`]), a few an hour, as «Хочу скачать» items of
//! their own origin ([`WishlistOrigin::Upgrade`]): a track, or its album
//! when more of it is below the bar. They go through the wishlist's own
//! search, sources, refusals and waits; only offers at least as good as the
//! bar count, and they wait in «Подтверди» («FLAC вместо MP3 128») unless
//! the user lets them replace the file by themselves
//! ([`v::Prefs::upgrade_auto`]). What comes is imported for the item's
//! tracks as any wished download is: the importer upgrades each track in
//! place (its id, likes, plays, playlists and lyrics stay) and the old file
//! goes to the library's Trash. Nothing better this time: the next search
//! when the schedule says.

use std::path::Path;
use std::time::{Duration, Instant};

use nikgob_core::model::{
    ReleaseId, Source, TrackId, WishlistId, WishlistItem, WishlistOrigin, WishlistScope, WishlistStatus, score,
};
use nikgob_core::i18n::{Locale, status_in};
use nikgob_core::quality::CheckSummary;
use nikgob_core::source::{Format, Offer};
use nikgob_view::{self as v, UpgradeBar, UpgradeEvery};

use super::{Next, RETRY_AFTER, offers_of_json};
use crate::Worker;

/// What a better copy asked for by hand looks for, in its row's words.
fn asked_wanted_in(loc: Locale) -> &'static str {
    status_in(loc).better_quality
}

/// The bar's words: «lossless», «MP3 320 или лучше».
fn bar_words_in(bar: UpgradeBar, loc: Locale) -> &'static str {
    let strings = status_in(loc);
    match bar {
        UpgradeBar::Mp3_320 => strings.mp3_320_or_better,
        UpgradeBar::Lossless => strings.lossless,
    }
}
/// Searches for better copies that start in an hour at most…
const PER_HOUR: usize = 4;
/// …and how many of them search or download at once.
const AT_ONCE: usize = 1;
/// The first one starts this long after the app did (it has enough to do then).
const FIRST_AFTER: Duration = Duration::from_secs(10 * 60);
const HOUR: Duration = Duration::from_secs(3600);
/// An album with this many files below the bar is looked for as a whole.
const ALBUM_FROM: usize = 2;
/// Items one tick looks at, at most (one that needs nothing any more is
/// dropped and does not count as a search).
const ROUNDS: usize = 8;
/// Files below the bar a new item is picked from (the worst first).
const CANDIDATES: u32 = 50;

/// How many searches for better copies may start at `now`: `recent` are
/// the starts of the ones before, `busy` the ones searching or downloading.
pub(crate) fn upgrade_slots(recent: &[Instant], busy: usize, now: Instant) -> usize {
    let last_hour = recent.iter().filter(|t| now.saturating_duration_since(**t) < HOUR).count();
    PER_HOUR.saturating_sub(last_hour).min(AT_ONCE.saturating_sub(busy))
}

/// When an item searched at `now_ms` is searched again (unix ms); `None`:
/// never.
pub(crate) fn next_search(every: UpgradeEvery, now_ms: i64) -> Option<i64> {
    every.period().map(|p| now_ms + p.as_millis() as i64)
}

/// A file of quality `score` is looked for again under `bar`: it is below
/// it, and a real file (audio saved from a stream has an item of its own).
pub(crate) fn below_bar(score: i32, bar: UpgradeBar) -> bool {
    score > score::SAVED_STREAM && score < bar.score()
}

/// A format in words: «MP3 128», «Opus 160», «FLAC», «FLAC 24/96».
pub(crate) fn format_words(f: &Format) -> String {
    let label = f.label();
    let name = nikgob_core::quality::codec_name(&f.codec);
    match label.split_once(' ') {
        Some((_, rest)) => format!("{name} {rest}"),
        None => name,
    }
}

/// A file's format in words: «MP3 128», «FLAC», «FLAC 24/96»; a «FLAC»
/// the spectrum check found made from a lossy file (its `quality_score`
/// says so, not its extension) is «FLAC из lossy», and one cut where a real
/// master may be cut too (ranked as an MP3 320, above a plain fake) «FLAC,
/// возможно из lossy».
pub(crate) fn file_label(src: &Source) -> String {
    file_label_in(Locale::Ru, src)
}

/// [`file_label`] in `loc`.
pub(crate) fn file_label_in(loc: Locale, src: &Source) -> String {
    let strings = status_in(loc);
    let label = format_words(&Format {
        codec: src.codec.clone(),
        bitrate_kbps: src.bitrate,
        bit_depth: src.bit_depth,
        sample_rate: src.sample_rate,
    });
    if !src.codec.is_lossless() || src.quality_score >= score::CD_LOSSLESS {
        label
    } else if src.quality_score > score::FAKE_LOSSLESS {
        format!("{label}{}", strings.maybe_lossy)
    } else {
        format!("{label}{}", strings.from_lossy)
    }
}

/// What a file is for the words of a better copy: a lossy copy ranked as
/// lossless was made from a lossless master the archive keeps, and that is
/// what improved: «FLAC 24/96 (в библиотеке Opus 158)». `master` is the
/// master's format in words, when known.
pub(crate) fn kept_label(src: &Source, master: Option<&str>) -> String {
    kept_label_in(Locale::Ru, src, master)
}

/// [`kept_label`] in `loc`.
pub(crate) fn kept_label_in(loc: Locale, src: &Source, master: Option<&str>) -> String {
    let own = file_label_in(loc, src);
    match master {
        Some(m) if !src.codec.is_lossless() && src.quality_score >= score::CD_LOSSLESS => {
            format!("{m} {}", nikgob_core::i18n::status::fmt_in_library_in(loc, &own))
        }
        _ => own,
    }
}

/// The line of an import done for a better copy, in place of «добавлено 0
/// · дублей 1»: what came of it, plainly. `dup` is what a copy that was not
/// better really is («FLAC из MP3 ~128»). `loc` is the locale the line is
/// written in (kept as [`crate::streams::upgrade_job_words`]).
pub(crate) fn upgrade_job_words(
    loc: Locale,
    imported: usize,
    duplicates: usize,
    left_out: usize,
    failed: usize,
    dup: Option<&str>,
) -> String {
    nikgob_core::i18n::status::fmt_upgrade_job_words_in(loc, imported, duplicates, left_out, failed, dup)
}

/// An offer's line in «Подтверди», saying what it improves: «FLAC вместо
/// MP3 128 · 312 МБ · RuTracker».
pub(crate) fn offer_detail(offer: &Offer, have: Option<&str>) -> String {
    nikgob_core::i18n::status::fmt_offer_improving_in(Locale::Ru, offer, have)
}

/// [`offer_detail`] in `loc`.
pub(crate) fn offer_detail_in(loc: Locale, offer: &Offer, have: Option<&str>) -> String {
    nikgob_core::i18n::status::fmt_offer_improving_in(loc, offer, have)
}

/// The note (and «История» line) of an item whose file was replaced.
pub(crate) fn done_words(from: Option<&str>, to: Option<&str>) -> String {
    done_words_in(Locale::Ru, from, to)
}

/// [`done_words`] in `loc`.
pub(crate) fn done_words_in(loc: Locale, from: Option<&str>, to: Option<&str>) -> String {
    nikgob_core::i18n::status::fmt_replaced_in(loc, from, to)
}

/// Something for a track is on its way or waits in «Подтверди»: an item of
/// the track (`items`, of any kind) or an album item of its album
/// (`album_items`) that searches, downloads or asks.
pub(crate) fn on_its_way(items: &[WishlistItem], album_items: &[WishlistItem]) -> bool {
    let busy = |i: &WishlistItem| matches!(i.status, WishlistStatus::Searching | WishlistStatus::Downloading | WishlistStatus::Confirm);
    items.iter().any(|i| i.scope == WishlistScope::Track && busy(i))
        || album_items.iter().any(|i| i.scope == WishlistScope::Album && busy(i))
}

/// The state of an item that looks for a better copy, for its row in
/// «Хочу скачать»: `have` what its files are now («MP3 128», «MP3 128 у 6
/// из 10»), `wanted` what is looked for, `next` when it looks again.
/// `None` where the usual words say it (downloading, files in «Подтверди»,
/// turned down).
pub(crate) fn upgrade_words(
    status: WishlistStatus,
    files_wait: bool,
    have: Option<&str>,
    wanted: &str,
    next: Option<&str>,
    note: Option<&str>,
) -> Option<String> {
    upgrade_words_in(Locale::Ru, status, files_wait, have, wanted, next, note)
}

/// [`upgrade_words`] in `loc`.
pub(crate) fn upgrade_words_in(
    loc: Locale,
    status: WishlistStatus,
    files_wait: bool,
    have: Option<&str>,
    wanted: &str,
    next: Option<&str>,
    note: Option<&str>,
) -> Option<String> {
    use nikgob_core::i18n::status::{UpgradeState, fmt_upgrade_state_in};
    use WishlistStatus as St;
    match status {
        St::New | St::Searching | St::Waiting => {
            Some(fmt_upgrade_state_in(loc, have, UpgradeState::Searching, wanted, None))
        }
        St::Confirm if !files_wait => Some(fmt_upgrade_state_in(loc, have, UpgradeState::FoundBetter, wanted, None)),
        St::NoTorrent | St::Failed => Some(fmt_upgrade_state_in(loc, have, UpgradeState::NothingBetter, wanted, next)),
        St::Done => {
            let note = note.unwrap_or(status_in(loc).replaced_better);
            let mut c = note.chars();
            Some(c.next().map(|f| f.to_lowercase().chain(c).collect()).unwrap_or_default())
        }
        St::Confirm | St::Downloading | St::Dismissed => None,
    }
}

/// A library file as the rule of a better copy reads it
/// ([`nikgob_core::quality::really_better`]).
#[derive(Clone, Debug)]
pub(crate) struct FileRank {
    pub(crate) codec: nikgob_core::model::Codec,
    pub(crate) rank: i32,
    pub(crate) check: Option<CheckSummary>,
}

impl FileRank {
    /// A copy of quality `offer` would really be better than this file.
    pub(crate) fn beaten_by(&self, offer: i32) -> bool {
        nikgob_core::quality::really_better(&self.codec, self.rank, self.check.as_ref(), offer)
    }
}

/// An offer of quality `offer` counts for an item whose files are `ranks`
/// (`None`: an item of any other kind, which takes it): it is really
/// better than one of them at least.
pub(crate) fn offer_counts(ranks: Option<&[FileRank]>, offer: i32) -> bool {
    ranks.is_none_or(|files| files.iter().any(|f| f.beaten_by(offer)))
}

impl Worker {
    fn upgrade_prefs(&self) -> (UpgradeEvery, UpgradeBar, bool) {
        let p = &self.state.read().prefs;
        (p.upgrade_every, p.upgrade_below, p.upgrade_auto)
    }

    /// The item looks for a better copy of a file the library has.
    pub(crate) fn is_upgrade(&self, id: WishlistId) -> bool {
        self.lib.get_wishlist_item(id).ok().flatten().is_some_and(|i| i.origin == WishlistOrigin::Upgrade)
    }

    /// The quality a file of `item` needs nothing better from: the bar,
    /// or anything above the file a better copy was asked for by hand.
    fn upgrade_line(&self, item: &WishlistItem) -> i32 {
        item.better_than.map_or_else(|| self.upgrade_prefs().1.score(), |q| q.saturating_add(1))
    }

    /// An offer for `item` must be better than this to count: the bar less
    /// one, for an item that looks for a better copy (the file's own
    /// quality, for one asked for by hand).
    pub(super) fn upgrade_floor(&self, item: &WishlistItem) -> Option<i32> {
        (item.origin == WishlistOrigin::Upgrade).then(|| self.upgrade_line(item) - 1)
    }

    /// A found copy replaces the item's file only once the user says so
    /// (always, for one asked for by hand). An album asked for by hand
    /// asks once its copy came, with each file it replaces
    /// ([`super::album_better`]): its best offer downloads without a word.
    pub(super) fn upgrade_asks(&self, item: &WishlistItem) -> bool {
        item.origin == WishlistOrigin::Upgrade
            && !super::album_better::hand_album(item)
            && (item.better_than.is_some() || !self.upgrade_prefs().2)
    }

    /// The one place that says whether a track's file is looked for in
    /// better quality at all. A track the user deleted is not (the library
    /// query leaves out `deleted_at` already); a song the user left out of
    /// a wished album is not either: that check belongs here, beside this
    /// one, so every path of the search (new items, an album's files, what
    /// is good enough) goes by it. A file there is nothing better than
    /// (our Opus of a genuine FLAC, a genuine FLAC) is never looked for:
    /// [`Self::nothing_better`], beside the offers' own rule
    /// ([`FileRank::beaten_by`]).
    pub(super) fn upgrade_wants(&self, track: TrackId) -> bool {
        // A song the user deleted from its album stays gone.
        self.lib.get_track(track).ok().flatten().is_some() && !self.lib.is_track_unwanted(track).unwrap_or(false)
    }

    /// The check of a library file as the rule of a better copy reads it:
    /// only while it is of the file there now.
    pub(super) fn file_check(&self, src: &Source) -> Option<CheckSummary> {
        let check = self.lib.quality_check(src.id).ok().flatten()?;
        let p = Path::new(&src.locator);
        let path = if p.is_absolute() { p.to_owned() } else { self.res.root().join(p) };
        crate::quality::check_fresh(&path, &check).then(|| check.summary())
    }

    /// There is nothing better than the file
    /// ([`nikgob_core::quality::nothing_better`]).
    pub(crate) fn nothing_better(&self, src: &Source) -> bool {
        src.quality_score >= score::CD_LOSSLESS
            && nikgob_core::quality::nothing_better(&src.codec, src.quality_score, self.file_check(src).as_ref())
    }

    /// What the item's files are as the rule of a better copy reads them
    /// ([`FileRank`]); `None` for an item of any other kind.
    pub(super) fn upgrade_ranks(&self, item: &WishlistItem) -> Option<Vec<FileRank>> {
        (item.origin == WishlistOrigin::Upgrade).then(|| self.upgrade_files(item).iter().map(|s| self.file_rank(s)).collect())
    }

    /// A file as the rule of a better copy reads it.
    pub(super) fn file_rank(&self, src: &Source) -> FileRank {
        FileRank { codec: src.codec.clone(), rank: src.quality_score, check: self.file_check(src) }
    }

    /// Every file of the item is one there is nothing better than (none
    /// is left to look for).
    fn upgrade_settled(&self, item: &WishlistItem) -> bool {
        let tracks: Vec<TrackId> = match item.scope {
            WishlistScope::Track => item.track_id.into_iter().collect(),
            WishlistScope::Album => self.album_tracks(item),
        };
        let files: Vec<Source> = tracks
            .into_iter()
            .filter_map(|t| self.lib.active_source(t).ok().flatten())
            .filter(|s| s.kind.is_local() && s.available && s.trashed_at.is_none())
            .collect();
        !files.is_empty() && files.iter().all(|s| self.nothing_better(s))
    }

    /// The open «Качество лучше» items of `track` (not one that searches,
    /// downloads or asks) go: its file needs nothing better.
    fn upgrade_drop_track(&mut self, track: TrackId) {
        for i in self.lib.wishlist_for_track(track).unwrap_or_default() {
            let open = !matches!(
                i.status,
                WishlistStatus::Searching | WishlistStatus::Downloading | WishlistStatus::Confirm | WishlistStatus::Done
            );
            if i.origin == WishlistOrigin::Upgrade && i.scope == WishlistScope::Track && open {
                log::info!("upgrade: {} needs nothing better, its item goes", i.title.as_deref().unwrap_or("?"));
                self.upgrade_drop(i.id);
            }
        }
    }

    /// The tracks of an album item as a folder must have them to be its
    /// album: their titles, places and lengths.
    fn album_tracklist(&self, item: &WishlistItem) -> Vec<(String, u32, u32, Option<u32>)> {
        nikgob_import::wished_release(&self.lib, item)
            .ok()
            .flatten()
            .map(|r| {
                self.lib
                    .tracks_of_release(r.id)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|t| self.upgrade_wants(t.id))
                    .map(|t| (t.title, t.disc_no, t.track_no, t.duration_ms))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether a folder (its files' names and lengths) is the album an item
    /// that looks for a better copy of an album is for: enough of the
    /// library's tracks are in it (another artist's album under the same
    /// word has none). Anything else passes.
    pub(super) fn upgrade_folder_fits(&self, item: &WishlistItem, files: &[(String, Option<u32>)]) -> bool {
        if item.origin != WishlistOrigin::Upgrade || item.scope != WishlistScope::Album {
            return true;
        }
        let list = self.album_tracklist(item);
        let tracks: Vec<nikgob_import::AlbumTrack<'_>> = list
            .iter()
            .map(|(title, disc_no, track_no, duration_ms)| nikgob_import::AlbumTrack {
                title,
                disc_no: *disc_no,
                track_no: *track_no,
                duration_ms: *duration_ms,
            })
            .collect();
        let files: Vec<(&str, Option<u32>)> = files.iter().map(|(n, d)| (n.as_str(), *d)).collect();
        nikgob_import::folder_has_album(&tracks, &files)
    }

    /// The tracks of an album item that are in the library.
    fn album_tracks(&self, item: &WishlistItem) -> Vec<TrackId> {
        nikgob_import::wished_release(&self.lib, item)
            .ok()
            .flatten()
            .map(|r| self.lib.tracks_of_release(r.id).unwrap_or_default().into_iter().map(|t| t.id).collect())
            .unwrap_or_default()
    }

    /// The files the item's track (every track of its album) plays.
    fn upgrade_files(&self, item: &WishlistItem) -> Vec<Source> {
        let tracks = match item.scope {
            WishlistScope::Track => item.track_id.into_iter().collect(),
            WishlistScope::Album => self.album_tracks(item),
        };
        tracks
            .into_iter()
            .filter(|t| self.upgrade_wants(*t))
            .filter_map(|t| self.lib.active_source(t).ok().flatten())
            .filter(|s| s.kind.is_local() && s.available && s.trashed_at.is_none() && s.quality_score > score::SAVED_STREAM)
            .collect()
    }

    /// Every file of the item is good enough now.
    pub(super) fn upgrade_reached(&self, item: &WishlistItem) -> bool {
        let bar = self.upgrade_line(item);
        let files = self.upgrade_files(item);
        !files.is_empty() && files.iter().all(|s| s.quality_score >= bar)
    }

    /// The worst of the item's files below the bar, in words: what a copy
    /// that comes replaces.
    fn upgrade_worst(&self, item: &WishlistItem) -> Option<String> {
        let bar = self.upgrade_line(item);
        let loc = self.locale();
        self.upgrade_files(item)
            .iter()
            .filter(|s| s.quality_score < bar)
            .min_by_key(|s| s.quality_score)
            .map(|s| file_label_in(loc, s))
    }

    /// What the item's files are, in words: «MP3 128», or «MP3 128 у 6 из
    /// 10» for an album some of whose files are good; `None` once none is
    /// below the bar. Kept until a file lands.
    pub(super) fn upgrade_have(&mut self, item: &WishlistItem) -> Option<String> {
        if let Some(have) = self.streaming.wish.upgrade_have.get(&item.id) {
            return have.clone();
        }
        let bar = self.upgrade_line(item);
        let files = self.upgrade_files(item);
        let below: Vec<&Source> = files.iter().filter(|s| s.quality_score < bar).collect();
        let loc = self.locale();
        let have = below.iter().min_by_key(|s| s.quality_score).map(|worst| {
            let label = file_label_in(loc, worst);
            if item.scope == WishlistScope::Album && below.len() < files.len() {
                nikgob_core::i18n::status::fmt_how_many_of_in(loc, &label, below.len(), files.len())
            } else {
                label
            }
        });
        self.streaming.wish.upgrade_have.insert(item.id, have.clone());
        have
    }

    /// The words of an upgrade item's row (see [`upgrade_words`]).
    pub(super) fn upgrade_row_words(&mut self, item: &WishlistItem, files_wait: bool, next: Option<&str>, can_search: bool) -> Option<String> {
        let waits = matches!(item.status, WishlistStatus::New | WishlistStatus::Searching | WishlistStatus::Waiting);
        if item.origin != WishlistOrigin::Upgrade || (waits && !can_search) {
            return None;
        }
        let have = match item.status {
            WishlistStatus::Done => None,
            _ => self.upgrade_have(item),
        };
        let loc = self.locale();
        let wanted = if item.better_than.is_some() { asked_wanted_in(loc) } else { bar_words_in(self.upgrade_prefs().1, loc) };
        upgrade_words_in(loc, item.status, files_wait, have.as_deref(), wanted, next, item.note.as_deref())
    }

    /// Searches for better copies, while the schedule and the pace allow:
    /// the items whose next search came, then new ones, the worst files
    /// first. With «Никогда», the items that wait go.
    pub(crate) fn upgrade_tick(&mut self) {
        let (every, bar, _) = self.upgrade_prefs();
        if every.period().is_none() {
            self.upgrades_off();
            return;
        }
        if self.import.is_none() || !self.can_download() {
            return;
        }
        let now = Instant::now();
        let since = *self.streaming.wish.upgrade_since.get_or_insert(now);
        if now.saturating_duration_since(since) < FIRST_AFTER {
            return;
        }
        // The wishlist's own items first: they take the slots they need.
        let limit = self.state.read().prefs.auto_download_limit.max(1) as usize;
        let (mut busy, mut busy_upgrades) = (0, 0);
        for status in [WishlistStatus::Searching, WishlistStatus::Downloading] {
            for i in self.lib.list_wishlist_by_status(status).unwrap_or_default() {
                busy += 1;
                busy_upgrades += usize::from(i.origin == WishlistOrigin::Upgrade);
            }
        }
        self.streaming.wish.upgrade_runs.retain(|t| now.saturating_duration_since(*t) < HOUR);
        let mut free = upgrade_slots(&self.streaming.wish.upgrade_runs, busy_upgrades, now).min(limit.saturating_sub(busy));
        let now_ms = Self::now();
        for _ in 0..ROUNDS {
            if free == 0 {
                break;
            }
            let due = self.lib.upgrades_due(now_ms, 1).unwrap_or_default().into_iter().next();
            let Some(item) = due.or_else(|| self.upgrade_new(bar)) else { break };
            // Good enough already (a download meanwhile, a lower bar): nothing
            // to look for.
            let line = self.upgrade_line(&item);
            let ranks = self.upgrade_ranks(&item).unwrap_or_default();
            if !ranks.iter().any(|f| f.rank < line && f.beaten_by(line)) {
                log::info!("upgrade: {} needs nothing better now", item.title.as_deref().unwrap_or("?"));
                self.upgrade_drop(item.id);
                continue;
            }
            self.streaming.wish.upgrade_runs.push(now);
            free -= 1;
            self.wish_search(item);
        }
    }

    /// A new item for the worst file below `bar` that has none: its album's
    /// when more of the album is below it, else its track's.
    fn upgrade_new(&mut self, bar: UpgradeBar) -> Option<WishlistItem> {
        let (track, release, quality) = self
            .lib
            .upgrade_candidates(bar.score(), CANDIDATES)
            .ok()?
            .into_iter()
            .find(|(t, ..)| {
                self.upgrade_wants(*t) && !self.lib.active_source(*t).ok().flatten().is_some_and(|s| self.nothing_better(&s))
            })?;
        let album: Option<ReleaseId> = release.filter(|r| {
            let below = self
                .lib
                .tracks_of_release(*r)
                .unwrap_or_default()
                .iter()
                .filter(|t| self.upgrade_wants(t.id))
                .filter_map(|t| self.lib.active_source(t.id).ok().flatten())
                .filter(|s| s.kind.is_local() && below_bar(s.quality_score, bar) && !self.nothing_better(s))
                .count();
            below >= ALBUM_FROM
        });
        let mut item = match album {
            Some(r) => {
                let mut i = self.item_for(None, Some(r))?;
                i.scope = WishlistScope::Album;
                i
            }
            None => {
                let mut i = self.item_for(Some(track), release)?;
                i.scope = WishlistScope::Track;
                i
            }
        };
        item.origin = WishlistOrigin::Upgrade;
        item.status = WishlistStatus::New;
        log::info!(
            "upgrade: looking for better than {quality} for {} ({:?})",
            item.title.as_deref().unwrap_or("?"),
            item.scope
        );
        self.put_item(&item);
        Some(item)
    }

    /// The item goes: it needs nothing any more, or the user turned the
    /// search off.
    pub(super) fn upgrade_drop(&mut self, id: WishlistId) {
        let _ = self.lib.delete_wishlist_item(id);
        self.upgrade_forget(id);
        self.wish_line_drop(id);
        self.publish_wishlist();
    }

    /// What the pipeline kept of the item.
    pub(super) fn upgrade_forget(&mut self, id: WishlistId) {
        let w = &mut self.streaming.wish;
        w.cands.remove(&id);
        w.doubtful.remove(&id);
        w.from.remove(&id);
        w.relabel.remove(&id);
        w.turned.remove(&id);
        w.later.remove(&id);
        w.retry_only.remove(&id);
        w.asked.remove(&id);
        w.upgrade_from.remove(&id);
        w.upgrade_have.remove(&id);
        w.upgrade_dups.remove(&id);
    }

    /// «Никогда»: the items that wait for their next search go (one the
    /// user is asked about in «Подтверди», or that downloads, stays).
    fn upgrades_off(&mut self) {
        let waiting = self.lib.upgrades_due(i64::MAX, 1000).unwrap_or_default();
        if waiting.is_empty() {
            return;
        }
        log::info!("upgrade: off, {} items go", waiting.len());
        for i in waiting {
            let _ = self.lib.delete_wishlist_item(i.id);
            self.upgrade_forget(i.id);
            self.wish_line_drop(i.id);
        }
        self.publish_wishlist();
    }

    /// Nothing better this time: searched again when the schedule says.
    /// `found_nothing`: the search itself found nothing better; one the
    /// user asked for by hand then answers the click (whatever the toasts
    /// of the downloads running by themselves are set to) and looks on as
    /// any other. Anything else (a download that failed, a source that
    /// kept saying no) keeps it asking before it replaces the file.
    pub(super) fn upgrade_none(&mut self, id: WishlistId, found_nothing: bool) {
        // Nothing can be better than its files (our Opus of a genuine FLAC,
        // a genuine FLAC): it leaves the search for better copies for good.
        if found_nothing
            && let Some(item) = self.lib.get_wishlist_item(id).ok().flatten()
            && self.upgrade_settled(&item)
        {
            let title = self.item_title(id);
            log::info!("upgrade: nothing can be better than the files of {title}, its item goes");
            self.upgrade_drop(id);
            if item.better_than.is_some() {
                self.toast_info(nikgob_core::i18n::ErrorStrings::fmt_better_not_found_in(self.locale(), &title), None);
            }
            return;
        }
        let now = Self::now();
        let next = next_search(self.upgrade_prefs().0, now).unwrap_or(now + RETRY_AFTER.as_millis() as i64);
        let mut asked = false;
        self.set_item(id, |i| {
            i.status = WishlistStatus::NoTorrent;
            i.next_try_at = Some(next);
            i.note = Some(super::note_code(nikgob_core::i18n::status::WishNote::NothingBetter));
            if found_nothing {
                asked = i.better_than.take().is_some();
            }
        });
        self.streaming.wish.upgrade_from.remove(&id);
        self.streaming.wish.upgrade_have.remove(&id);
        self.wish_line_drop(id);
        if asked {
            let title = self.item_title(id);
            log::info!("upgrade: nothing better than the file of {title}");
            self.toast_info(nikgob_core::i18n::ErrorStrings::fmt_better_not_found_in(self.locale(), &title), None);
        }
        self.publish_wishlist();
    }

    /// «Оставить как есть» on a copy the user asked for by hand: it is
    /// turned down (a click on «Найти качество лучше…» asks again); never
    /// handed to the schedule, which may replace files without asking.
    /// False for any other item.
    pub(super) fn upgrade_kept(&mut self, id: WishlistId) -> bool {
        let Some(item) = self.lib.get_wishlist_item(id).ok().flatten() else { return false };
        if item.origin != WishlistOrigin::Upgrade || item.better_than.is_none() {
            return false;
        }
        log::info!("upgrade: the user keeps the file of {}", item.title.as_deref().unwrap_or("?"));
        self.upgrade_forget(id);
        self.streaming.wish.turned.remove(&id);
        self.set_item(id, |i| {
            i.status = WishlistStatus::Dismissed;
            i.next_try_at = None;
            i.note = Some(super::note_code(nikgob_core::i18n::status::WishNote::KeptFile));
        });
        self.wish_line_drop(id);
        self.publish_wishlist();
        true
    }

    /// «Найти качество лучше…» of `track`: a copy better than its file is
    /// looked for now, by the sources the wishlist asks in the user's order,
    /// and waits in «Подтверди» for the user's word whatever the prefs say.
    /// A track with no file goes to «Хочу скачать» as «Скачать» sends it;
    /// one with only audio saved from a stream has its own item look again.
    pub(crate) fn find_better(&mut self, track: TrackId) {
        let file = self
            .lib
            .active_source(track)
            .ok()
            .flatten()
            .filter(|s| s.kind.is_local() && s.available && s.trashed_at.is_none());
        let Some(file) = file else {
            self.download_tracks(&[track]);
            return;
        };
        let wishlist = v::Link::Route(v::Route::Downloads(v::DownloadsTab::Wishlist));
        let items = self.lib.wishlist_for_track(track).unwrap_or_default();
        let release = self.lib.get_track(track).ok().flatten().and_then(|t| t.release_id);
        // Something for the track on its way or waiting in «Подтверди»
        // (its own item of any kind, or its album's): not a second one.
        let album_items = release.map(|r| self.lib.wishlist_for_release(r).unwrap_or_default()).unwrap_or_default();
        if on_its_way(&items, &album_items) {
            self.download_toast(0, 1, None);
            return;
        }
        if file.quality_score <= score::SAVED_STREAM {
            // Its item looks for a real file already: it looks again now.
            let open = |i: &&WishlistItem| i.scope == WishlistScope::Track && i.status != WishlistStatus::Done;
            match items.iter().find(open) {
                Some(i) => {
                    let (id, what) = (i.id, i.title.as_ref().map(|t| format!("«{t}»")));
                    // One the user turned down is asked for again.
                    self.set_item(id, |i| {
                        if i.status == WishlistStatus::Dismissed {
                            i.status = WishlistStatus::New;
                            i.next_try_at = None;
                        }
                    });
                    self.wish_retry(id);
                    self.download_toast(1, 0, what);
                }
                None => self.download_tracks(&[track]),
            }
            return;
        }
        // Nothing is better than it (our Opus of a genuine FLAC, a genuine
        // FLAC): said at once, and it leaves the search for better copies.
        if self.nothing_better(&file) {
            let title = self
                .lib
                .get_track(track)
                .ok()
                .flatten()
                .map(|t| format!("{} — {}", t.artist_credit.display(), t.title))
                .unwrap_or_default();
            log::info!("upgrade: the user asks for better than {} of {title}: there is none", file_label(&file));
            self.upgrade_drop_track(track);
            self.toast_info(nikgob_core::i18n::ErrorStrings::fmt_better_not_found_in(self.locale(), &title), None);
            return;
        }
        if self.import.is_none() {
            self.toast_err(nikgob_core::i18n::errors_in(self.locale()).import_unavailable, None);
            return;
        }
        if !self.can_download() {
            let strings = nikgob_core::i18n::errors_in(self.locale());
            self.toast_open(
                v::ToastKind::Error,
                strings.need_download_source,
                nikgob_core::i18n::downloads_in(self.locale()).set_up,
                v::Link::Setting(v::SettingLink::Torrents),
            );
            return;
        }
        let above = file.quality_score;
        // Its item that waits for the schedule, or that the user turned
        // down before: asked for again, it looks now.
        let again = items
            .into_iter()
            .find(|i| i.scope == WishlistScope::Track && i.origin == WishlistOrigin::Upgrade && i.status != WishlistStatus::Done);
        let item = match again {
            Some(i) => {
                self.upgrade_forget(i.id);
                self.set_item(i.id, |i| {
                    i.status = WishlistStatus::New;
                    i.next_try_at = None;
                    i.note = None;
                    i.better_than = Some(above);
                })
            }
            None => {
                let made = self.item_for(Some(track), release);
                made.map(|mut i| {
                    i.scope = WishlistScope::Track;
                    i.origin = WishlistOrigin::Upgrade;
                    i.status = WishlistStatus::New;
                    i.better_than = Some(above);
                    i
                })
            }
        };
        let Some(item) = item else { return };
        self.put_item(&item);
        log::info!(
            "upgrade: the user asks for better than {} ({above}) for {}",
            file_label(&file),
            item.title.as_deref().unwrap_or("?")
        );
        let what = item.title.clone().unwrap_or_default();
        let loc = self.locale();
        self.toast_open(
            v::ToastKind::Working,
            nikgob_core::i18n::ErrorStrings::fmt_searching_better_in(loc, &what),
            nikgob_core::i18n::common_in(loc).open,
            wishlist,
        );
        self.streaming.wish.upgrade_runs.push(Instant::now());
        self.wish_search(item);
        self.publish_wishlist();
    }

    /// A copy of the item is on its way: what it replaces, for the words
    /// once it came.
    pub(super) fn upgrade_fetching(&mut self, item: &WishlistItem) {
        if item.origin == WishlistOrigin::Upgrade
            && !self.streaming.wish.upgrade_from.contains_key(&item.id)
            && let Some(worst) = self.upgrade_worst(item)
        {
            self.streaming.wish.upgrade_from.insert(item.id, worst);
        }
    }

    /// Better copies wait in «Подтверди»: «История» says what was found.
    pub(super) fn upgrade_offered(&mut self, id: WishlistId, shown: &[Offer]) {
        let Some(item) = self.lib.get_wishlist_item(id).ok().flatten() else { return };
        let loc = self.locale();
        let have = self.upgrade_worst(&item);
        let found = shown.first().and_then(|o| o.format.as_ref()).map(Format::label);
        let tab = nikgob_core::i18n::downloads_in(loc).tab_confirm;
        let what = nikgob_core::i18n::status::fmt_found_instead_in(loc, found.as_deref(), have.as_deref(), tab);
        let title = self.item_title(id);
        self.wish_line(id, title, what, None);
    }

    /// The item's files are good enough now: «Скачано», in words of what
    /// changed, in its row and in «История». A lossy copy made from a
    /// lossless master says what the master is: «FLAC 24/96 (в библиотеке
    /// Opus 158)», not a downgrade.
    pub(super) fn upgrade_done(&mut self, item: &WishlistItem) {
        let id = item.id;
        let loc = self.locale();
        let from = self.streaming.wish.upgrade_from.remove(&id);
        let worst = self.upgrade_files(item).into_iter().min_by_key(|s| s.quality_score);
        let to = worst.map(|s| {
            let master = self.streaming.wish.upgrade_came.remove(&s.track_id).or_else(|| self.master_words(&s));
            kept_label_in(loc, &s, master.as_deref())
        });
        let words = done_words_in(loc, from.as_deref(), to.as_deref());
        log::info!("upgrade: {} — {words}", item.title.as_deref().unwrap_or("?"));
        self.upgrade_forget(id);
        let note = words.clone();
        self.set_item(id, |i| {
            i.status = WishlistStatus::Done;
            i.next_try_at = None;
            i.note = Some(note);
        });
        let title = self.item_title(id);
        self.wish_line(id, title, words, None);
        self.publish_wishlist();
    }

    /// The master of a track's lossy copy in words, from what the archive
    /// knows: its codec, and the rate the check of the copy saw.
    fn master_words(&self, src: &Source) -> Option<String> {
        let master = self.lib.get_master(src.track_id).ok().flatten()?;
        let rate = self.lib.quality_check(src.id).ok().flatten().map(|c| c.sample_rate).filter(|r| *r > 0);
        let hi_res = src.quality_score >= score::HI_RES_LOSSLESS;
        let name = nikgob_core::quality::codec_name(&master.codec);
        let khz_word = match self.locale() {
            Locale::Ru => "кГц",
            Locale::En => "kHz",
        };
        Some(match rate {
            Some(r) if hi_res && r > 48_000 => {
                let khz = f64::from(r) / 1000.0;
                if khz.fract() == 0.0 { format!("{name} {khz:.0} {khz_word}") } else { format!("{name} {khz:.1} {khz_word}") }
            }
            _ if hi_res => format!("{name} Hi-Res"),
            _ => name,
        })
    }

    /// What a copy that came for the item and was no better really is.
    pub(crate) fn upgrade_dup(&self, id: WishlistId) -> Option<String> {
        self.streaming.wish.upgrade_dups.get(&id).cloned()
    }

    /// A file that came for the item was not better than the one there:
    /// what it really is, for the words once its import ended.
    pub(crate) fn wish_duplicate(&mut self, id: WishlistId, quality: Option<String>) {
        if let Some(q) = quality
            && self.is_upgrade(id)
        {
            self.streaming.wish.upgrade_dups.insert(id, q);
        }
    }

    /// What a file that came for an upgrade item is, read from the file
    /// itself while it is still there (its master's format, for the words).
    pub(super) fn upgrade_came(&mut self, track: TrackId, path: &std::path::Path) {
        if let Ok(info) = nikgob_import::tags::probe_info(path) {
            let f = Format {
                codec: info.codec,
                bitrate_kbps: info.bitrate_kbps,
                bit_depth: info.bit_depth,
                sample_rate: info.sample_rate,
            };
            self.streaming.wish.upgrade_came.insert(track, format_words(&f));
        }
    }

    /// The item's import ended (no file of it waits in «Подтверди»): done
    /// when its files are good enough; else the rest of an album from the
    /// next offer, or the next offer, or the next search. A copy that came
    /// and was no better is said plainly («скачанный FLAC оказался из MP3
    /// ~128 — оставил прежний файл»), and the item waits for its schedule
    /// rather than asking again in minutes the sources that said «wait».
    pub(super) fn upgrade_settle(&mut self, item: &WishlistItem) {
        // The import after «Заменить» of an album asked for by hand.
        if self.album_replaced(item) {
            return;
        }
        let id = item.id;
        let partial = self.streaming.wish.partial.remove(&id);
        let dup = self.streaming.wish.upgrade_dups.remove(&id);
        if item.status == WishlistStatus::Done {
            return;
        }
        if self.upgrade_reached(item) {
            self.upgrade_done(item);
            self.wish_tick();
            return;
        }
        if partial && item.scope == WishlistScope::Album && self.album_goes_on(id) {
            return;
        }
        // What came did not make it better (another release, the same
        // quality, a fake, not the song): the next offer, else the next search.
        if matches!(item.status, WishlistStatus::Downloading | WishlistStatus::Confirm) {
            self.streaming.wish.from.remove(&id);
            self.streaming.wish.turned.remove(&id);
            let next = self.wish_next_candidate(id);
            // The import's own line says what else came of it (left out,
            // failed); a copy that was no better is said here too.
            let Some(dup) = dup else { return };
            let loc = self.locale();
            let strings = status_in(loc);
            let why = upgrade_job_words(loc, 0, 1, 0, 0, Some(&dup));
            let mut why = {
                let mut c = why.chars();
                c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
            };
            match next {
                Next::Offer(from) => why.push_str(&format!(" · {}", nikgob_core::i18n::status::fmt_next_offer_in(loc, &from))),
                Next::Doubtful => why.push_str(&format!(" · {}", strings.next_doubtful)),
                Next::Later(_) | Next::NotFound => {}
            }
            let title = self.item_title(id);
            self.wish_line(id, title, why, None);
        }
    }

    /// «Подтверди» of a better copy: what each offer improves.
    pub(super) fn upgrade_confirm_view(&mut self, item: &WishlistItem, id: u64) -> v::Confirm {
        // A copy of an album that came and waits: one card of its tracks.
        if let Some(card) = self.album_better_card(item, id) {
            return card;
        }
        let offers = item.candidates_json.as_deref().map(offers_of_json).unwrap_or_default();
        let loc = self.locale();
        let strings = status_in(loc);
        let have = self.upgrade_worst(item);
        let what = match (&item.artist, &item.title) {
            (Some(a), Some(t)) => format!("{t} — {a}"),
            (_, Some(t)) => t.clone(),
            _ => super::capitalised(strings.better_quality),
        };
        let album = item.scope == WishlistScope::Album;
        v::Confirm {
            yes_no: false,
            id,
            title: what,
            origin: if album { strings.better_album } else { strings.better_track }.into(),
            options: offers
                .iter()
                .map(|o| v::ConfirmOption {
                    label: o.title.clone(),
                    detail: offer_detail_in(loc, o, have.as_deref()),
                    // Never «РЕКОМЕНДУЕМ»: «Принять все рекомендованные»
                    // must not replace a file the user has; each replacement
                    // is its own «Заменить».
                    recommended: false,
                })
                .collect(),
            skip_label: strings.keep_as_is.into(),
            question: if album { strings.replace_album_question } else { strings.replace_track_question }.into(),
            accept_label: strings.replace.into(),
            pick: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_core::model::{Codec, SourceKind};
    use nikgob_core::source::{OfferScope, SourceKey};

    const MIN: Duration = Duration::from_secs(60);

    /// A few an hour, one at a time.
    #[test]
    fn searches_are_spread_over_the_hour() {
        let now = Instant::now() + 2 * HOUR;
        assert_eq!(upgrade_slots(&[], 0, now), AT_ONCE);
        assert_eq!(upgrade_slots(&[], 1, now), 0, "one searches already");
        let three = [now - 50 * MIN, now - 30 * MIN, now - MIN];
        assert_eq!(upgrade_slots(&three, 0, now), 1);
        let four = [now - 50 * MIN, now - 30 * MIN, now - 20 * MIN, now - MIN];
        assert_eq!(upgrade_slots(&four, 0, now), 0, "{PER_HOUR} an hour");
        let old = [now - 70 * MIN, now - 65 * MIN, now - 61 * MIN, now - 60 * MIN];
        assert_eq!(upgrade_slots(&old, 0, now), 1, "the hour went by");
    }

    /// The next search of a file: a day, a week or a month on; never.
    #[test]
    fn the_schedule() {
        const DAY: i64 = 24 * 3600 * 1000;
        assert_eq!(next_search(UpgradeEvery::Day, 1_000), Some(1_000 + DAY));
        assert_eq!(next_search(UpgradeEvery::Week, 1_000), Some(1_000 + 7 * DAY));
        assert_eq!(next_search(UpgradeEvery::Month, 1_000), Some(1_000 + 30 * DAY));
        assert_eq!(next_search(UpgradeEvery::Never, 1_000), None);
        assert_eq!(v::Prefs::default().upgrade_every, UpgradeEvery::Week, "weekly unless the user says");
        assert!(!v::Prefs::default().upgrade_auto, "a found copy is asked about unless the user says");
    }

    /// «Только lossless» looks again for every lossy file, «MP3 320 и выше»
    /// for what is worse than MP3 320; audio saved from a stream has an item
    /// of its own, lossless is never looked for again.
    #[test]
    fn the_bar() {
        let lossless = UpgradeBar::Lossless;
        assert_eq!(v::Prefs::default().upgrade_below, lossless);
        for s in [500, score::MP3_320, score::OPUS_160, score::AAC_256, score::FAKE_LOSSLESS] {
            assert!(below_bar(s, lossless), "{s}");
        }
        assert!(!below_bar(score::CD_LOSSLESS, lossless));
        assert!(!below_bar(score::HI_RES_LOSSLESS, lossless));
        assert!(!below_bar(score::SAVED_STREAM, lossless), "the YouTube copy's item looks already");
        let mp3 = UpgradeBar::Mp3_320;
        assert!(below_bar(500, mp3), "MP3 128");
        assert!(below_bar(score::MP3_V0, mp3));
        assert!(!below_bar(score::MP3_320, mp3));
        assert!(!below_bar(score::OPUS_160, mp3), "Opus 160 is as good");
        assert!(!below_bar(score::CD_LOSSLESS, mp3));
    }

    fn offer(format: Format) -> Offer {
        Offer {
            source: SourceKey::Torrents,
            provider: "RuTracker".into(),
            id: "1".into(),
            title: "MORGENSHTERN - ЛЕГЕНДАРНАЯ ПЫЛЬ (2020) FLAC".into(),
            artist: None,
            track_title: None,
            album: None,
            duration_ms: None,
            isrc: None,
            scope: OfferScope::Album,
            format: Some(format),
            size_bytes: Some(312 << 20),
            seeders: Some(14),
            info_url: None,
            handle: String::new(),
        }
    }

    /// An offer says what it improves; a file says what it is.
    #[test]
    fn what_a_better_copy_improves_in_words() {
        let flac = offer(Format::new(Codec::Flac));
        assert_eq!(offer_detail(&flac, Some("MP3 128")), "FLAC вместо MP3 128 · 312 МБ · 14 сид. · RuTracker");
        assert_eq!(offer_detail(&flac, None), "FLAC · 312 МБ · 14 сид. · RuTracker");
        assert_eq!(
            offer_detail_in(Locale::En, &flac, Some("MP3 128")),
            "FLAC instead of MP3 128 · 312 MB · 14 seeds · RuTracker"
        );
        let mut src = Source::new(TrackId::new(), SourceKind::LocalFile, "a.mp3", Codec::Mp3);
        src.bitrate = Some(128);
        src.quality_score = 500;
        assert_eq!(file_label(&src), "MP3 128");
        let mut opus = Source::new(TrackId::new(), SourceKind::LocalFile, "a.opus", Codec::Opus);
        opus.bitrate = Some(160);
        opus.quality_score = score::OPUS_160;
        assert_eq!(file_label(&opus), "Opus 160");
        let mut fake = Source::new(TrackId::new(), SourceKind::LocalFile, "a.flac", Codec::Flac);
        fake.quality_score = score::FAKE_LOSSLESS;
        assert_eq!(file_label(&fake), "FLAC из lossy", "the score says, not the extension");
        assert_eq!(file_label_in(Locale::En, &fake), "FLAC from a lossy source");
        // Cut where a real master may be cut too: ranked as an MP3 320.
        fake.quality_score = score::MP3_320;
        assert_eq!(file_label(&fake), "FLAC, возможно из lossy");
        fake.quality_score = score::CD_LOSSLESS;
        assert_eq!(file_label(&fake), "FLAC");
        assert_eq!(done_words(Some("MP3 128"), Some("FLAC")), "Было MP3 128 — теперь FLAC");
        assert_eq!(done_words(None, Some("FLAC")), "Теперь FLAC");
        assert_eq!(done_words_in(Locale::En, Some("MP3 128"), Some("FLAC")), "Was MP3 128 — now FLAC");
    }

    /// «Treehouse»: the hi-res FLAC went to the archive, the library plays
    /// its Opus; the words say what improved, not «теперь OPUS 158».
    #[test]
    fn a_copy_made_from_a_master_says_what_the_master_is() {
        let mut opus = Source::new(TrackId::new(), SourceKind::LocalFile, "06 - Treehouse.opus", Codec::Opus);
        opus.bitrate = Some(158);
        opus.quality_score = score::HI_RES_LOSSLESS;
        assert_eq!(file_label(&opus), "Opus 158");
        let to = kept_label(&opus, Some("FLAC 24/96"));
        assert_eq!(to, "FLAC 24/96 (в библиотеке Opus 158)");
        assert_eq!(done_words(Some("MP3 242"), Some(&to)), "Было MP3 242 — теперь FLAC 24/96 (в библиотеке Opus 158)");
        assert_eq!(kept_label_in(Locale::En, &opus, Some("FLAC 24/96")), "FLAC 24/96 (library has Opus 158)");
        // An Opus that is all there is says so.
        opus.quality_score = score::OPUS_160;
        assert_eq!(kept_label(&opus, Some("FLAC")), "Opus 158");
        assert_eq!(format_words(&Format { codec: Codec::Flac, bitrate_kbps: None, bit_depth: Some(24), sample_rate: Some(96_000) }), "FLAC 24/96");
        // Ranked as its master: not below either bar, so not looked for again.
        assert!(!below_bar(score::HI_RES_LOSSLESS, UpgradeBar::Lossless));
    }

    /// «добавлено 0 · дублей 1» said nothing; now the line says why.
    #[test]
    fn an_upgrade_import_says_plainly_what_came_of_it() {
        let w = |imported, duplicates, left_out, failed, dup| {
            upgrade_job_words(Locale::Ru, imported, duplicates, left_out, failed, dup)
        };
        assert_eq!(
            w(0, 1, 0, 0, Some("FLAC из MP3 ~128")),
            "скачанный FLAC оказался из MP3 ~128 — оставил прежний файл"
        );
        assert_eq!(w(0, 1, 0, 0, Some("MP3 320")), "скачанный MP3 320 не лучше прежнего — оставил прежний файл");
        assert_eq!(
            w(0, 1, 0, 0, Some("FLAC, возможно из MP3 320")),
            "скачанный FLAC, возможно, из MP3 320 — оставил прежний файл"
        );
        assert_eq!(w(1, 0, 0, 0, None), "заменено 1");
        assert_eq!(w(0, 0, 6, 0, None), "скачанное не из этого альбома — ничего не добавил");
        assert_eq!(w(3, 1, 2, 0, None), "заменено 3 · не лучше прежних 1 · не из этого альбома 2");
        assert!(!w(0, 1, 0, 0, None).contains("дублей"));
        assert_eq!(upgrade_job_words(Locale::En, 1, 0, 0, 0, None), "replaced 1");
        assert_eq!(
            upgrade_job_words(Locale::En, 0, 1, 0, 0, Some("FLAC from MP3 ~128")),
            "the downloaded FLAC turned out to be from MP3 ~128 — kept the previous file"
        );
    }

    /// Any item of the track that searches, downloads or asks (of any
    /// kind), or such an album item of its album, holds a second search.
    #[test]
    fn a_track_is_on_its_way_by_any_item_of_it() {
        use WishlistStatus as St;
        let item = |st, scope| WishlistItem { scope, ..WishlistItem::new(st, 1) };
        for st in [St::Searching, St::Downloading, St::Confirm] {
            assert!(on_its_way(&[item(st, WishlistScope::Track)], &[]), "{st:?}");
            assert!(on_its_way(&[], &[item(st, WishlistScope::Album)]), "{st:?}");
            assert!(!on_its_way(&[], &[item(st, WishlistScope::Track)]), "another track of the album");
        }
        for st in [St::New, St::Waiting, St::NoTorrent, St::Failed, St::Dismissed, St::Done] {
            assert!(!on_its_way(&[item(st, WishlistScope::Track)], &[item(st, WishlistScope::Album)]), "{st:?}");
        }
    }

    /// The row says where the item stands in plain words.
    #[test]
    fn an_upgrade_rows_words() {
        use WishlistStatus as St;
        let w = |st, files, have: Option<&str>, next: Option<&str>, note: Option<&str>| {
            upgrade_words(st, files, have, "lossless", next, note)
        };
        assert_eq!(w(St::New, false, Some("MP3 128"), None, None).as_deref(), Some("MP3 128 · ищу lossless"));
        assert_eq!(
            w(St::Confirm, false, Some("MP3 128 у 6 из 10"), None, None).as_deref(),
            Some("MP3 128 у 6 из 10 · нашёл лучше — подтверди замену")
        );
        assert_eq!(w(St::Confirm, true, Some("MP3 128"), None, None), None, "files wait: the usual words");
        assert_eq!(
            w(St::NoTorrent, false, Some("MP3 320"), Some("поищу снова через 7 дн."), None).as_deref(),
            Some("MP3 320 · лучше пока нет, поищу снова через 7 дн.")
        );
        assert_eq!(w(St::Done, false, None, None, Some("Было MP3 128 — теперь FLAC")).as_deref(), Some("было MP3 128 — теперь FLAC"));
        assert_eq!(w(St::Downloading, false, Some("MP3 128"), None, None), None);
        // The same state in English, plurals and all.
        let en = |st, have: Option<&str>, next: Option<&str>| upgrade_words_in(Locale::En, st, false, have, "lossless", next, None);
        assert_eq!(en(St::New, Some("MP3 128"), None).as_deref(), Some("MP3 128 · looking for lossless"));
        assert_eq!(
            en(St::NoTorrent, Some("MP3 320"), Some("trying again in 7 d.")).as_deref(),
            Some("MP3 320 · nothing better yet, trying again in 7 d.")
        );
        assert_eq!(
            en(St::Done, None, None).as_deref(),
            Some("replaced with better quality"),
            "the words the note is made of, lowercased"
        );
    }
}
