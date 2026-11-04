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

use std::time::{Duration, Instant};

use nikgob_core::model::{
    ReleaseId, Source, TrackId, WishlistId, WishlistItem, WishlistOrigin, WishlistScope, WishlistStatus, score,
};
use nikgob_core::source::{Format, Offer};
use nikgob_view::{self as v, UpgradeBar, UpgradeEvery};

use super::{RETRY_AFTER, offers_of_json};
use crate::Worker;

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

/// A file's format in words: «MP3 128», «FLAC», «FLAC 24/96»; a «FLAC»
/// the spectrum check found made from a lossy file (its `quality_score`
/// says so, not its extension) is «FLAC из lossy».
pub(crate) fn file_label(src: &Source) -> String {
    let label = Format {
        codec: src.codec.clone(),
        bitrate_kbps: src.bitrate,
        bit_depth: src.bit_depth,
        sample_rate: src.sample_rate,
    }
    .label();
    if src.codec.is_lossless() && src.quality_score < score::CD_LOSSLESS {
        format!("{label} из lossy")
    } else {
        label
    }
}

/// An offer's line in «Подтверди», saying what it improves: «FLAC вместо
/// MP3 128 · 312 МБ · RuTracker».
pub(crate) fn offer_detail(offer: &Offer, have: Option<&str>) -> String {
    let detail = offer.detail();
    match (&offer.format, have) {
        (Some(f), Some(have)) => {
            let label = f.label();
            match detail.strip_prefix(&label) {
                Some(rest) => format!("{label} вместо {have}{rest}"),
                None => detail,
            }
        }
        _ => detail,
    }
}

/// The note (and «История» line) of an item whose file was replaced.
pub(crate) fn done_words(from: Option<&str>, to: Option<&str>) -> String {
    match (from, to) {
        (Some(f), Some(t)) if f != t => format!("Было {f} — теперь {t}"),
        (_, Some(t)) => format!("Теперь {t}"),
        _ => "Заменил на качество лучше".to_owned(),
    }
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
    use WishlistStatus as St;
    let have = have.map(|h| format!("{h} · ")).unwrap_or_default();
    match status {
        St::New | St::Searching | St::Waiting => Some(format!("{have}ищу {wanted}")),
        St::Confirm if !files_wait => Some(format!("{have}нашёл лучше — подтверди замену")),
        St::NoTorrent | St::Failed => Some(match next {
            Some(next) => format!("{have}лучше пока нет, {next}"),
            None => format!("{have}лучше пока нет"),
        }),
        St::Done => {
            let note = note.unwrap_or("Заменил на качество лучше");
            let mut c = note.chars();
            Some(c.next().map(|f| f.to_lowercase().chain(c).collect()).unwrap_or_default())
        }
        St::Confirm | St::Downloading | St::Dismissed => None,
    }
}

impl Worker {
    fn upgrade_prefs(&self) -> (UpgradeEvery, UpgradeBar, bool) {
        let p = &self.state.read().prefs;
        (p.upgrade_every, p.upgrade_below, p.upgrade_auto)
    }

    /// The item looks for a better copy of a file the library has.
    pub(super) fn is_upgrade(&self, id: WishlistId) -> bool {
        self.lib.get_wishlist_item(id).ok().flatten().is_some_and(|i| i.origin == WishlistOrigin::Upgrade)
    }

    /// An offer for `item` must be better than this to count: the bar less
    /// one, for an item that looks for a better copy.
    pub(super) fn upgrade_floor(&self, item: &WishlistItem) -> Option<i32> {
        (item.origin == WishlistOrigin::Upgrade).then(|| self.upgrade_prefs().1.score() - 1)
    }

    /// A found copy replaces the item's file only once the user says so.
    pub(super) fn upgrade_asks(&self, item: &WishlistItem) -> bool {
        item.origin == WishlistOrigin::Upgrade && !self.upgrade_prefs().2
    }

    /// The one place that says whether a track's file is looked for in
    /// better quality at all. A track the user deleted is not (the library
    /// query leaves out `deleted_at` already); a song the user left out of
    /// a wished album is not either: that check belongs here, beside this
    /// one, so every path of the search (new items, an album's files, what
    /// is good enough) goes by it.
    fn upgrade_wants(&self, track: TrackId) -> bool {
        self.lib.get_track(track).ok().flatten().is_some()
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
        let bar = self.upgrade_prefs().1.score();
        let files = self.upgrade_files(item);
        !files.is_empty() && files.iter().all(|s| s.quality_score >= bar)
    }

    /// The worst of the item's files below the bar, in words: what a copy
    /// that comes replaces.
    fn upgrade_worst(&self, item: &WishlistItem) -> Option<String> {
        let bar = self.upgrade_prefs().1.score();
        self.upgrade_files(item)
            .iter()
            .filter(|s| s.quality_score < bar)
            .min_by_key(|s| s.quality_score)
            .map(file_label)
    }

    /// What the item's files are, in words: «MP3 128», or «MP3 128 у 6 из
    /// 10» for an album some of whose files are good; `None` once none is
    /// below the bar. Kept until a file lands.
    pub(super) fn upgrade_have(&mut self, item: &WishlistItem) -> Option<String> {
        if let Some(have) = self.streaming.wish.upgrade_have.get(&item.id) {
            return have.clone();
        }
        let bar = self.upgrade_prefs().1.score();
        let files = self.upgrade_files(item);
        let below: Vec<&Source> = files.iter().filter(|s| s.quality_score < bar).collect();
        let have = below.iter().min_by_key(|s| s.quality_score).map(|worst| {
            let label = file_label(worst);
            if item.scope == WishlistScope::Album && below.len() < files.len() {
                format!("{label} у {} из {}", below.len(), files.len())
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
        let wanted = self.upgrade_prefs().1.wanted();
        upgrade_words(item.status, files_wait, have.as_deref(), wanted, next, item.note.as_deref())
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
            if !self.upgrade_files(&item).iter().any(|s| s.quality_score < bar.score()) {
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
            .find(|(t, ..)| self.upgrade_wants(*t))?;
        let album: Option<ReleaseId> = release.filter(|r| {
            let below = self
                .lib
                .tracks_of_release(*r)
                .unwrap_or_default()
                .iter()
                .filter(|t| self.upgrade_wants(t.id))
                .filter_map(|t| self.lib.active_source(t.id).ok().flatten())
                .filter(|s| s.kind.is_local() && below_bar(s.quality_score, bar))
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
    fn upgrade_drop(&mut self, id: WishlistId) {
        let _ = self.lib.delete_wishlist_item(id);
        self.upgrade_forget(id);
        self.wish_line_drop(id);
        self.publish_wishlist();
    }

    /// What the pipeline kept of the item.
    fn upgrade_forget(&mut self, id: WishlistId) {
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

    /// Nothing better this time (or the user kept the file): searched again
    /// when the schedule says.
    pub(super) fn upgrade_none(&mut self, id: WishlistId) {
        let now = Self::now();
        let next = next_search(self.upgrade_prefs().0, now).unwrap_or(now + RETRY_AFTER.as_millis() as i64);
        self.set_item(id, |i| {
            i.status = WishlistStatus::NoTorrent;
            i.next_try_at = Some(next);
            i.note = Some("Лучше пока нет".into());
        });
        self.streaming.wish.upgrade_from.remove(&id);
        self.wish_line_drop(id);
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
        let have = self.upgrade_worst(&item);
        let found = shown.first().and_then(|o| o.format.as_ref()).map(Format::label);
        let what = match (found, have) {
            (Some(f), Some(h)) => format!("Нашёл {f} вместо {h}"),
            (Some(f), None) => format!("Нашёл {f}"),
            _ => "Нашёл качество лучше".to_owned(),
        };
        let title = self.item_title(id);
        self.wish_line(id, title, format!("{what} · подтверди замену в «Подтверди»"), None);
    }

    /// The item's files are good enough now: «Скачано», in words of what
    /// changed, in its row and in «История».
    pub(super) fn upgrade_done(&mut self, item: &WishlistItem) {
        let id = item.id;
        let from = self.streaming.wish.upgrade_from.remove(&id);
        let to = self.upgrade_files(item).iter().min_by_key(|s| s.quality_score).map(file_label);
        let words = done_words(from.as_deref(), to.as_deref());
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

    /// The item's import ended (no file of it waits in «Подтверди»): done
    /// when its files are good enough; else the rest of an album from the
    /// next offer, or the next offer, or the next search.
    pub(super) fn upgrade_settle(&mut self, item: &WishlistItem) {
        let id = item.id;
        let partial = self.streaming.wish.partial.remove(&id);
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
        // quality, not the song): the next offer, else the next search.
        if matches!(item.status, WishlistStatus::Downloading | WishlistStatus::Confirm) {
            self.streaming.wish.from.remove(&id);
            self.wish_next_candidate(id);
        }
    }

    /// «Подтверди» of a better copy: what each offer improves.
    pub(super) fn upgrade_confirm_view(&mut self, item: &WishlistItem, id: u64) -> v::Confirm {
        let offers = item.candidates_json.as_deref().map(offers_of_json).unwrap_or_default();
        let have = self.upgrade_worst(item);
        let what = match (&item.artist, &item.title) {
            (Some(a), Some(t)) => format!("{t} — {a}"),
            (_, Some(t)) => t.clone(),
            _ => "Качество лучше".into(),
        };
        let album = item.scope == WishlistScope::Album;
        v::Confirm {
            id,
            title: what,
            origin: if album { "Качество лучше · альбом" } else { "Качество лучше · трек" }.into(),
            options: offers
                .iter()
                .enumerate()
                .map(|(i, o)| v::ConfirmOption {
                    label: o.title.clone(),
                    detail: offer_detail(o, have.as_deref()),
                    recommended: Some(i) == offers.iter().position(|o| o.seeders != Some(0)),
                })
                .collect(),
            skip_label: "Оставить как есть".into(),
            question: if album { "Заменить файлы альбома на это?" } else { "Заменить файл трека на это?" }.into(),
            accept_label: "Заменить".into(),
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
        let mut src = Source::new(TrackId::new(), SourceKind::LocalFile, "a.mp3", Codec::Mp3);
        src.bitrate = Some(128);
        src.quality_score = 500;
        assert_eq!(file_label(&src), "MP3 128");
        let mut fake = Source::new(TrackId::new(), SourceKind::LocalFile, "a.flac", Codec::Flac);
        fake.quality_score = score::FAKE_LOSSLESS;
        assert_eq!(file_label(&fake), "FLAC из lossy", "the score says, not the extension");
        fake.quality_score = score::CD_LOSSLESS;
        assert_eq!(file_label(&fake), "FLAC");
        assert_eq!(done_words(Some("MP3 128"), Some("FLAC")), "Было MP3 128 — теперь FLAC");
        assert_eq!(done_words(None, Some("FLAC")), "Теперь FLAC");
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
    }
}
