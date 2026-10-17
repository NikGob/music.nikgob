//! The wishlist pipeline (docs/stage3.md §3.5, §3.7): a new like of a
//! stream-only track becomes an item; items are searched on the indexer
//! (artist + album, then artist + title; ranked like the torrent search:
//! FLAC first, release match, seeders, sane size), the track's file (or the
//! whole album) is picked from the best release and downloaded by the
//! torrent client, and the importer attaches the file to the item's track
//! (the job's [`ImportTargets`]). A doubtful release waits in "Подтверди";
//! nothing found → "нет торрента", a toast, and a search again a week later
//! until the user dismisses it. Items from an import wait for the user's
//! "скачать". At most [`nikgob_view::Prefs::auto_download_limit`] items search
//! or download at once.
//!
//! The way out without a torrent: the audio saved from YouTube Music as it
//! is (a low-quality local source a later torrent upgrades), or a .torrent,
//! magnet, file or folder of the user's own, linked to the item's track.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use nikgob_core::indexer::{Candidate, SearchQuery, TorrentPayload};
use nikgob_core::model::{
    ReleaseId, SourceKind, SourceOrigin, TrackId, WishlistId, WishlistItem, WishlistOrigin,
    WishlistScope, WishlistStatus, score,
};
use nikgob_core::streaming::{EntityKind, Service, StreamError};
use nikgob_import::ImportTargets;
use nikgob_metadata::text::{album_key, artist_key, parse_filename, search_artist, search_title, similarity, title_key};
use nikgob_view::{self as v, Command};

use super::Msg;
use crate::Worker;

/// Bit of the "Подтверди" ids of wishlist items (import ids use bit 48,
/// torrent jobs small numbers).
pub(crate) const WISH_ID: u64 = 1 << 47;
/// An item without a torrent is searched again after this.
const RETRY_AFTER: Duration = Duration::from_secs(7 * 24 * 3600);
/// Releases offered in "Подтверди".
const CONFIRM_OPTIONS: usize = 3;
/// A release title must name the artist this well (0..=1).
const ARTIST_MATCH: f64 = 0.85;
/// A file name must be this close to the track title (0..=1)…
const FILE_TITLE: f64 = 0.85;
/// …or this close when its number is the track's.
const FILE_NUMBERED: f64 = 0.4;

const AUDIO_EXT: &[&str] = &[
    "flac", "mp3", "m4a", "ogg", "opus", "wav", "ape", "wv", "aac", "alac", "aiff", "aif",
];

/// Messages of the pipeline's background steps.
pub(crate) enum WishMsg {
    Searched {
        item: WishlistId,
        result: Result<Vec<Candidate>, String>,
    },
    Fetched {
        item: WishlistId,
        cand: Box<Candidate>,
        result: Result<TorrentPayload, String>,
    },
    Inspected {
        item: WishlistId,
        cand: Box<Candidate>,
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
    /// Ranked releases of items being handled, best first.
    cands: HashMap<WishlistId, Vec<Candidate>>,
    /// Stable "Подтверди" ids.
    confirm_ids: HashMap<WishlistId, u64>,
    next_confirm: u64,
    /// Audio saved from YouTube that the importer is taking, by file.
    saving: HashMap<PathBuf, WishlistId>,
}

/// What an item wants, for building queries and picking files.
#[derive(Clone, Debug)]
pub(crate) struct Want {
    pub(crate) artist: String,
    pub(crate) album: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) track_no: u32,
    pub(crate) disc_no: u32,
    pub(crate) scope: WishlistScope,
}

/// Indexer queries, most specific first.
pub(crate) fn queries(w: &Want) -> Vec<String> {
    let artist = search_artist(&w.artist);
    let mut out = Vec::new();
    if let Some(a) = &w.album {
        out.push(format!("{artist} {}", search_title(a)));
    }
    if w.scope == WishlistScope::Track
        && let Some(t) = &w.title
    {
        out.push(format!("{artist} {}", search_title(t)));
    }
    out.dedup();
    out
}

/// The folded words of `needle` all occur in `hay` (both folded keys).
fn contains_words(hay: &str, needle: &str) -> bool {
    !needle.is_empty() && needle.split(' ').all(|w| hay.split(' ').any(|h| h == w))
}

/// Ranks releases for `w`: those naming the artist, by the torrent search's
/// score; `true` = sure enough to download without asking (lossless, names
/// the album or the title, has seeders).
pub(crate) fn rank(cands: Vec<Candidate>, w: &Want) -> Vec<(Candidate, bool)> {
    let artist = artist_key(&w.artist);
    let album = w.album.as_deref().map(album_key);
    let title = w.title.as_deref().map(|t| title_key(t).base);
    let query = queries(w).into_iter().next().unwrap_or_default();
    let mut scored: Vec<(f64, Candidate, bool)> = cands
        .into_iter()
        .filter_map(|c| {
            let hay = title_key(&c.title).base;
            let names_artist = contains_words(&hay, &artist)
                || hay
                    .split(" - ")
                    .next()
                    .is_some_and(|head| similarity(head.trim(), &artist) >= ARTIST_MATCH);
            if !names_artist {
                return None;
            }
            let names_it = album.as_deref().is_some_and(|a| contains_words(&hay, a))
                || (w.scope == WishlistScope::Track
                    && title.as_deref().is_some_and(|t| contains_words(&hay, t)));
            let lossless = crate::search::guess_format(&c.title)
                .is_some_and(|f| ["FLAC", "ALAC", "WAV", "APE"].iter().any(|l| f.starts_with(l)));
            let seeded = c.seeders.is_some_and(|s| s > 0);
            let s = crate::search::score(&c, &query);
            Some((s, c, names_it && lossless && seeded))
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.into_iter().map(|(_, c, sure)| (c, sure)).collect()
}

fn ext_of(path: &str) -> String {
    Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// The disc a path is on, from a "CD2" / "Disc 2" / "Disk 2" folder.
fn disc_of(path: &str) -> Option<u32> {
    let lower = path.to_lowercase();
    for part in lower.split('/').rev().skip(1) {
        let p = part.trim();
        for prefix in ["cd", "disc", "disk", "диск"] {
            if let Some(rest) = p.strip_prefix(prefix) {
                let digits: String = rest
                    .trim_start_matches([' ', '_', '-', '.'])
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect();
                if let Ok(n) = digits.parse() {
                    return Some(n);
                }
            }
        }
    }
    None
}

/// The files to download for `w`: the track's file (fuzzy title, or its
/// number with a similar title, on its disc), or every audio file of the
/// album. `None` when the torrent does not have it.
pub(crate) fn pick_files(files: &[(usize, String, u64)], w: &Want) -> Option<Vec<usize>> {
    let audio: Vec<&(usize, String, u64)> = files
        .iter()
        .filter(|(_, p, _)| AUDIO_EXT.contains(&ext_of(p).as_str()))
        .collect();
    if audio.is_empty() {
        return None;
    }
    match (w.scope, &w.title) {
        (WishlistScope::Track, Some(title)) => {
            let want = title_key(title).base;
            let mut best: Option<(f64, usize)> = None;
            for (index, path, _) in &audio {
                if let Some(d) = disc_of(path)
                    && d != w.disc_no.max(1)
                {
                    continue;
                }
                let name = path.rsplit('/').next().unwrap_or(path);
                let parsed = parse_filename(name);
                let stem = Path::new(name)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let got = title_key(parsed.title.as_deref().unwrap_or(&stem)).base;
                let sim = similarity(&got, &want).max(similarity(&title_key(&stem).base, &want));
                let numbered = w.track_no > 0 && parsed.track_no == Some(w.track_no);
                if sim >= FILE_TITLE || (numbered && sim >= FILE_NUMBERED) {
                    let rank = sim + if numbered { 0.5 } else { 0.0 };
                    if best.is_none_or(|(r, _)| rank > r) {
                        best = Some((rank, *index));
                    }
                }
            }
            best.map(|(_, i)| vec![i])
        }
        _ => {
            // A torrent of several albums: the folder named like the album.
            let album = w.album.as_deref().map(album_key);
            let in_album: Vec<usize> = audio
                .iter()
                .filter(|(_, p, _)| {
                    album.as_deref().is_some_and(|a| {
                        let dirs = p.rsplit_once('/').map_or("", |(d, _)| d);
                        contains_words(&title_key(dirs).base, a)
                    })
                })
                .map(|(i, ..)| *i)
                .collect();
            Some(if in_album.is_empty() {
                audio.iter().map(|(i, ..)| *i).collect()
            } else {
                in_album
            })
        }
    }
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

fn human_size(b: Option<u64>) -> Option<String> {
    b.map(crate::torrents::human_bytes)
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
            for r in self
                .lib
                .external_ids(EntityKind::Track, *t.id.as_uuid())
                .unwrap_or_default()
            {
                match r.service {
                    Service::Spotify => item.spotify_id = Some(r.id),
                    Service::Youtube => item.youtube_id = Some(r.id),
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

    /// A new like of `track`: a wishlist item when it has no file (an album
    /// item when the prefs say so), unless one exists or was dismissed.
    pub(crate) fn wish_for_like(&mut self, track: TrackId, origin: WishlistOrigin) {
        if !self.stream_only(track) {
            return;
        }
        let existing = self.lib.wishlist_for_track(track).unwrap_or_default();
        let open = |i: &WishlistItem| !matches!(i.status, WishlistStatus::Done | WishlistStatus::Failed);
        if existing.iter().any(open) {
            return; // waiting already, or dismissed for good
        }
        let scope = self.state.read().prefs.like_download_scope;
        let release = self.lib.get_track(track).ok().flatten().and_then(|t| t.release_id);
        if scope == WishlistScope::Album
            && let Some(r) = release
            && self.lib.wishlist_for_release(r).unwrap_or_default().iter().any(open)
        {
            return;
        }
        let Some(mut item) = self.item_for(Some(track), release) else { return };
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
        log::info!("wishlist: {} ({:?}, {:?})", item.title.as_deref().unwrap_or("?"), item.scope, origin);
        self.put_item(&item);
    }

    /// A new like of an album: one album item, when some of it has no file.
    pub(crate) fn wish_for_album(&mut self, release: ReleaseId, tracks: &[TrackId], origin: WishlistOrigin) {
        if !tracks.iter().any(|&t| self.stream_only(t)) {
            return;
        }
        let open = |i: &WishlistItem| !matches!(i.status, WishlistStatus::Done | WishlistStatus::Failed);
        if self.lib.wishlist_for_release(release).unwrap_or_default().iter().any(open) {
            return;
        }
        let Some(mut item) = self.item_for(None, Some(release)) else { return };
        item.scope = WishlistScope::Album;
        item.origin = origin;
        item.status = if origin == WishlistOrigin::Import {
            WishlistStatus::Waiting
        } else {
            WishlistStatus::New
        };
        self.put_item(&item);
    }

    /// The like went away: items that did not start yet go too.
    pub(crate) fn wish_drop_unstarted(&mut self, track: TrackId) {
        for i in self.lib.wishlist_for_track(track).unwrap_or_default() {
            if matches!(
                i.status,
                WishlistStatus::New | WishlistStatus::Waiting | WishlistStatus::NoTorrent | WishlistStatus::Confirm
            ) {
                let _ = self.lib.delete_wishlist_item(i.id);
                self.streaming.wish.cands.remove(&i.id);
            }
        }
    }

    pub(crate) fn wish_drop_album(&mut self, release: ReleaseId) {
        for i in self.lib.wishlist_for_release(release).unwrap_or_default() {
            if i.scope == WishlistScope::Album
                && matches!(i.status, WishlistStatus::New | WishlistStatus::Waiting | WishlistStatus::NoTorrent | WishlistStatus::Confirm)
            {
                let _ = self.lib.delete_wishlist_item(i.id);
            }
        }
    }

    /// The tracks an item's files should become.
    fn targets_of(&self, item: &WishlistItem) -> Vec<TrackId> {
        let mut out: Vec<TrackId> = item.track_id.into_iter().collect();
        if item.scope == WishlistScope::Album
            && let Some(r) = item.release_id
        {
            for row in self.lib.tracks_of_release(r).unwrap_or_default() {
                if !out.contains(&row.id) {
                    out.push(row.id);
                }
            }
        }
        out
    }

    /// The import targets of the item a torrent was downloaded for.
    pub(crate) fn wish_targets(&self, item: WishlistId) -> ImportTargets {
        self.lib
            .get_wishlist_item(item)
            .ok()
            .flatten()
            .map(|i| ImportTargets::new(self.targets_of(&i)))
            .unwrap_or_default()
    }

    fn want_of(&self, item: &WishlistItem) -> Want {
        let track = item.track_id.and_then(|t| self.lib.get_track(t).ok().flatten());
        let release = item
            .release_id
            .or(track.as_ref().and_then(|t| t.release_id))
            .and_then(|r| self.lib.get_release(r).ok().flatten());
        Want {
            artist: release
                .as_ref()
                .map(|r| r.artist_credit.display())
                .filter(|a| !a.is_empty())
                .or_else(|| item.artist.clone())
                .unwrap_or_default(),
            album: release.as_ref().map(|r| r.title.clone()),
            title: track.as_ref().map(|t| t.title.clone()).or_else(|| item.title.clone()),
            track_no: track.as_ref().map_or(0, |t| t.track_no),
            disc_no: track.as_ref().map_or(1, |t| t.disc_no),
            scope: item.scope,
        }
    }

    // ------------------------------------------------------------- scheduler

    /// Starts searches while there are free slots.
    pub(crate) fn wish_tick(&mut self) {
        let (Some(_), Some(_)) = (self.indexer.as_ref(), self.import.as_ref()) else {
            return;
        };
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
        let mut due = self
            .lib
            .wishlist_due(WishlistStatus::New, now, free as u32)
            .unwrap_or_default();
        if due.len() < free {
            due.extend(
                self.lib
                    .wishlist_due(WishlistStatus::NoTorrent, now, (free - due.len()) as u32)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|i| i.next_try_at.is_some()),
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
        let Some(indexer) = self.indexer.clone() else { return };
        let want = self.want_of(&item);
        let qs = queries(&want);
        if qs.is_empty() {
            self.no_torrent(item.id, false);
            return;
        }
        let id = item.id;
        self.set_item(id, |i| {
            i.status = WishlistStatus::Searching;
            i.attempts += 1;
            i.note = Some("Ищу раздачу…".into());
        });
        log::info!("wishlist: searching {qs:?}");
        self.streaming.spawn(async move {
            let mut last_err = None;
            for q in qs {
                let mut query = SearchQuery::new(q);
                query.limit = Some(100);
                match tokio::time::timeout(Duration::from_secs(90), indexer.search(&query)).await {
                    Ok(Ok(c)) if !c.is_empty() => {
                        return Msg::Wish(WishMsg::Searched { item: id, result: Ok(c) });
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => last_err = Some(e.to_string()),
                    Err(_) => last_err = Some("индексатор не ответил".into()),
                }
            }
            Msg::Wish(WishMsg::Searched {
                item: id,
                result: last_err.map_or(Ok(Vec::new()), Err),
            })
        });
        self.publish_wishlist();
    }

    pub(crate) fn wish_msg(&mut self, m: WishMsg) {
        match m {
            WishMsg::Searched { item, result } => self.wish_searched(item, result),
            WishMsg::Fetched { item, cand, result } => match result {
                Ok(p) => self.wish_inspect(item, *cand, p),
                Err(e) => {
                    log::warn!("wishlist: fetching {}: {e}", cand.title);
                    self.wish_next_candidate(item);
                }
            },
            WishMsg::Inspected {
                item,
                cand,
                payload,
                result,
            } => self.wish_inspected(item, *cand, payload, result),
            WishMsg::Saved { item, track, result } => self.wish_saved(item, track, result),
        }
    }

    fn wish_searched(&mut self, id: WishlistId, result: Result<Vec<Candidate>, String>) {
        let Some(item) = self.lib.get_wishlist_item(id).ok().flatten() else { return };
        if item.status != WishlistStatus::Searching {
            return; // dismissed meanwhile
        }
        let cands = match result {
            Ok(c) => c,
            Err(e) => {
                // The indexer is down: not "no torrent", just later.
                log::warn!("wishlist search: {e}");
                self.set_item(id, |i| {
                    i.status = WishlistStatus::New;
                    i.next_try_at = Some(Self::now() + 3_600_000);
                    i.note = Some(format!("Поиск не удался: {e}"));
                });
                self.publish_wishlist();
                return;
            }
        };
        let want = self.want_of(&item);
        let ranked = rank(cands, &want);
        let Some((best, sure)) = ranked.first().cloned() else {
            self.no_torrent(id, true);
            return;
        };
        let list: Vec<Candidate> = ranked.into_iter().map(|(c, _)| c).collect();
        if sure {
            self.streaming.wish.cands.insert(id, list);
            self.wish_fetch(id, best);
        } else {
            // Doubtful: the user picks among the best few.
            let shown: Vec<Candidate> = list.into_iter().take(CONFIRM_OPTIONS).collect();
            let json = serde_json::to_string(&shown).ok();
            self.set_item(id, |i| {
                i.status = WishlistStatus::Confirm;
                i.candidates_json = json;
                i.note = Some("Не уверен в раздаче — выбери".into());
            });
            self.publish_wishlist();
        }
    }

    fn wish_fetch(&mut self, id: WishlistId, cand: Candidate) {
        let Some(indexer) = self.indexer.clone() else { return };
        self.set_item(id, |i| i.note = Some(format!("Раздача: {}", cand.title)));
        self.streaming.spawn(async move {
            let result = tokio::time::timeout(Duration::from_secs(60), indexer.fetch_torrent(&cand))
                .await
                .map_err(|_| "индексатор не отдал торрент".to_owned())
                .and_then(|r| r.map_err(|e| e.to_string()));
            Msg::Wish(WishMsg::Fetched {
                item: id,
                cand: Box::new(cand),
                result,
            })
        });
    }

    fn wish_inspect(&mut self, id: WishlistId, cand: Candidate, payload: TorrentPayload) {
        let tx = self.streaming.internal.clone();
        self.with_client(Box::new(move |w, c| {
            let Some(svc) = &w.import else { return };
            svc.spawn(async move {
                let result = c.inspect(payload.clone()).await.map_err(|e| e.to_string());
                let _ = tx.send(crate::import::Internal::Stream(Msg::Wish(WishMsg::Inspected {
                    item: id,
                    cand: Box::new(cand),
                    payload,
                    result,
                })));
            });
        }));
    }

    fn wish_inspected(
        &mut self,
        id: WishlistId,
        cand: Candidate,
        payload: TorrentPayload,
        result: Result<nikgob_torrent::TorrentMeta, String>,
    ) {
        let Some(item) = self.lib.get_wishlist_item(id).ok().flatten() else { return };
        if matches!(item.status, WishlistStatus::Dismissed | WishlistStatus::Done) {
            return;
        }
        let meta = match result {
            Ok(m) => m,
            Err(e) => {
                log::warn!("wishlist: listing {}: {e}", cand.title);
                self.wish_next_candidate(id);
                return;
            }
        };
        let files: Vec<(usize, String, u64)> = meta.files.iter().map(|f| (f.index, f.path.clone(), f.size)).collect();
        let want = self.want_of(&item);
        let Some(picked) = pick_files(&files, &want) else {
            log::info!("wishlist: {} has no file of {:?}", cand.title, want.title);
            self.wish_next_candidate(id);
            return;
        };
        let size: u64 = files.iter().filter(|(i, ..)| picked.contains(i)).map(|(.., s)| s).sum();
        let format = crate::search::guess_format(&cand.title).unwrap_or_else(|| "?".into());
        let note = format!(
            "{format} · {} · {} сид. · {}",
            crate::torrents::human_bytes(size),
            cand.seeders.unwrap_or(0),
            cand.title
        );
        self.set_item(id, |i| {
            i.status = WishlistStatus::Downloading;
            i.note = Some(note);
        });
        self.streaming.wish.cands.remove(&id);
        let magnet = match &payload {
            TorrentPayload::Magnet(m) => Some(m.clone()),
            TorrentPayload::TorrentFile(_) => None,
        };
        self.add_payload_for(payload, Some(picked), magnet, Some(id));
        self.publish_wishlist();
    }

    /// The release did not work out: the next ranked one, else no torrent.
    fn wish_next_candidate(&mut self, id: WishlistId) {
        let next = match self.streaming.wish.cands.get_mut(&id) {
            Some(list) if !list.is_empty() => {
                list.remove(0);
                list.first().cloned()
            }
            _ => None,
        };
        match next {
            Some(c) => self.wish_fetch(id, c),
            None => {
                self.streaming.wish.cands.remove(&id);
                self.no_torrent(id, true);
            }
        }
    }

    fn no_torrent(&mut self, id: WishlistId, tell: bool) {
        let Some(item) = self.set_item(id, |i| {
            i.status = WishlistStatus::NoTorrent;
            i.next_try_at = Some(Self::now() + RETRY_AFTER.as_millis() as i64);
            i.note = Some("Не нашёл раздачу — поищу снова через неделю".into());
        }) else {
            return;
        };
        if tell {
            let what = match (&item.artist, &item.title) {
                (Some(a), Some(t)) => format!("{a} — {t}"),
                (_, Some(t)) => t.clone(),
                _ => "трек".into(),
            };
            let action = (item.track_id.is_some() && self.service(Service::Youtube).is_some())
                .then(|| ("Сохранить с YouTube".to_owned(), Command::WishlistSaveFromYoutube(id)));
            self.toast(format!("Не нашёл раздачу: {what} · Wishlist в «Загрузках»"), action);
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
        for &id in ids {
            self.streaming.wish.cands.remove(&id);
            self.set_item(id, |i| {
                i.status = WishlistStatus::Dismissed;
                i.next_try_at = None;
            });
        }
        self.publish_wishlist();
    }

    pub(crate) fn wish_retry(&mut self, id: WishlistId) {
        self.set_item(id, |i| {
            if i.status != WishlistStatus::Dismissed {
                i.status = WishlistStatus::New;
                i.next_try_at = None;
            }
        });
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
        let cands: Vec<Candidate> = item
            .candidates_json
            .as_deref()
            .and_then(|j| serde_json::from_str(j).ok())
            .unwrap_or_default();
        match option.and_then(|i| cands.get(i).cloned()) {
            Some(c) => {
                self.set_item(id, |i| i.status = WishlistStatus::Searching);
                self.streaming.wish.cands.insert(id, vec![c.clone()]);
                self.wish_fetch(id, c);
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
            .find(|s| s.kind == SourceKind::Youtube && s.available && s.trashed_at.is_none());
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
        let tracks = self.targets_of(&item);
        let note = match &what {
            v::WishlistAttach::TorrentFile(_) | v::WishlistAttach::Magnet(_) => "Своя раздача",
            v::WishlistAttach::Files(_) | v::WishlistAttach::Folder(_) => "Свой файл",
        };
        let started = match what {
            v::WishlistAttach::Files(paths) => {
                let force = paths.len() == 1 && item.scope == WishlistScope::Track && tracks.len() == 1;
                let targets = ImportTargets { tracks, force };
                self.import_for(paths, "Импорт для Wishlist".into(), SourceOrigin::Inbox, targets)
            }
            v::WishlistAttach::Folder(dir) => {
                self.import_for(vec![dir], "Импорт для Wishlist".into(), SourceOrigin::Inbox, ImportTargets::new(tracks))
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
    pub(crate) fn wish_imported(&mut self, track: TrackId, path: &Path) {
        if let Some(item) = self.streaming.wish.saving.remove(path) {
            let _ = std::fs::remove_file(path);
            self.set_item(item, |i| {
                i.status = WishlistStatus::NoTorrent;
                i.next_try_at = Some(Self::now() + RETRY_AFTER.as_millis() as i64);
                i.note = Some("Есть копия с YouTube (низкое качество) — ищу раздачу".into());
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
                    .filter(|i| i.scope == WishlistScope::Album && i.status == WishlistStatus::Downloading),
            );
        }
        for i in items {
            if matches!(i.status, WishlistStatus::Done | WishlistStatus::Dismissed) {
                continue;
            }
            self.set_item(i.id, |i| {
                i.status = WishlistStatus::Done;
                i.next_try_at = None;
                i.note = Some("Скачано".into());
            });
            changed = true;
        }
        if changed {
            self.publish_wishlist();
            self.wish_tick();
        }
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

    /// After a restart: searches that were running start over.
    pub(crate) fn wish_restart(&mut self) {
        for i in self.lib.list_wishlist_by_status(WishlistStatus::Searching).unwrap_or_default() {
            self.set_item(i.id, |i| i.status = WishlistStatus::New);
        }
    }

    // ------------------------------------------------------------- view

    pub(crate) fn publish_wishlist(&mut self) {
        let mut items = self.lib.list_wishlist().unwrap_or_default();
        items.sort_by(|a, b| {
            status_rank(a.status)
                .cmp(&status_rank(b.status))
                .then(b.created_at.cmp(&a.created_at))
        });
        let now = Self::now();
        let youtube = self.service(Service::Youtube).is_some();
        let mut rows = Vec::new();
        let mut by_album: Vec<v::WishlistGroup> = Vec::new();
        let mut by_artist: Vec<v::WishlistGroup> = Vec::new();
        let mut counts: Vec<(WishlistStatus, u32)> = Vec::new();
        let mut confirms = Vec::new();
        for item in &items {
            let track = item.track_id.and_then(|t| self.lib.get_track(t).ok().flatten());
            let release_id = item.release_id.or(track.as_ref().and_then(|t| t.release_id));
            let release = release_id.and_then(|r| self.lib.get_release(r).ok().flatten());
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
                            title: self.lib.get_artist(a).ok().flatten().map_or_else(|| artist.clone(), |a| a.name),
                            subtitle: String::new(),
                            release: None,
                            artist: Some(a),
                            items: vec![item.id],
                            waiting,
                        }),
                    }
                }
            }
            if item.status == WishlistStatus::Confirm {
                confirms.push(self.wish_confirm_view(item));
            }
            let next_try = item
                .next_try_at
                .filter(|_| item.status == WishlistStatus::NoTorrent)
                .map(|at| {
                    let days = ((at - now).max(0) as u64).div_ceil(86_400_000);
                    format!("следующий поиск через {days} дн.")
                });
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
                    && !matches!(item.status, WishlistStatus::Done | WishlistStatus::Dismissed),
            });
        }
        counts.sort_by_key(|(s, _)| status_rank(*s));
        let view = v::WishlistView {
            loaded: true,
            items: rows,
            by_album,
            by_artist,
            counts,
        };
        self.publish(|s, _| {
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
        let cands: Vec<Candidate> = item
            .candidates_json
            .as_deref()
            .and_then(|j| serde_json::from_str(j).ok())
            .unwrap_or_default();
        let what = match (&item.artist, &item.title) {
            (Some(a), Some(t)) => format!("{t} — {a}"),
            (_, Some(t)) => t.clone(),
            _ => "Wishlist".into(),
        };
        v::Confirm {
            id,
            title: what,
            origin: match item.scope {
                WishlistScope::Track => "Wishlist · трек".into(),
                WishlistScope::Album => "Wishlist · альбом".into(),
            },
            options: cands
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let mut detail = Vec::new();
                    detail.extend(crate::search::guess_format(&c.title));
                    detail.extend(human_size(c.size_bytes));
                    if let Some(s) = c.seeders {
                        detail.push(format!("{s} сид."));
                    }
                    detail.push(c.indexer_name.clone());
                    v::ConfirmOption {
                        label: c.title.clone(),
                        detail: detail.join(" · "),
                        recommended: i == 0,
                    }
                })
                .collect(),
            skip_label: "Не то — поищу позже".into(),
            accept_label: "Скачать".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_core::indexer::CandidateHandle;

    fn cand(title: &str, seeders: u32) -> Candidate {
        Candidate {
            id: title.into(),
            title: title.into(),
            indexer_name: "t".into(),
            size_bytes: Some(400_000_000),
            seeders: Some(seeders),
            leechers: None,
            published: None,
            categories: vec![],
            info_url: None,
            info_hash: None,
            handle: CandidateHandle::default(),
        }
    }

    fn want(scope: WishlistScope) -> Want {
        Want {
            artist: "Кино".into(),
            album: Some("Группа крови".into()),
            title: Some("Война".into()),
            track_no: 3,
            disc_no: 1,
            scope,
        }
    }

    #[test]
    fn queries_name_the_album_then_the_track() {
        assert_eq!(queries(&want(WishlistScope::Track)), ["Кино Группа крови", "Кино Война"]);
        assert_eq!(queries(&want(WishlistScope::Album)), ["Кино Группа крови"]);
    }

    #[test]
    fn releases_of_the_artist_ranked_flac_first_and_sure_only_when_lossless() {
        let ranked = rank(
            vec![
                cand("Ария - Группа крови (cover) FLAC", 50),
                cand("Кино - Группа крови (1988) MP3 320", 90),
                cand("Кино - Группа крови (1988) [FLAC]", 20),
                cand("Кино - Дискография FLAC", 300),
            ],
            &want(WishlistScope::Track),
        );
        let titles: Vec<&str> = ranked.iter().map(|(c, _)| c.title.as_str()).collect();
        assert_eq!(titles[0], "Кино - Группа крови (1988) [FLAC]");
        assert!(!titles.iter().any(|t| t.starts_with("Ария")), "another artist");
        assert!(ranked[0].1, "lossless, names the album, seeded");
        let mp3 = ranked.iter().find(|(c, _)| c.title.contains("MP3")).unwrap();
        assert!(!mp3.1, "lossy: asked about");
    }

    #[test]
    fn picks_the_track_file_on_its_disc_or_the_whole_album() {
        let files = vec![
            (0, "Кино - Группа крови/01 - Группа крови.flac".into(), 30),
            (1, "Кино - Группа крови/02 - Закрой за мной дверь.flac".into(), 30),
            (2, "Кино - Группа крови/03 - Война.flac".into(), 30),
            (3, "Кино - Группа крови/cover.jpg".into(), 1),
        ];
        assert_eq!(pick_files(&files, &want(WishlistScope::Track)), Some(vec![2]));
        assert_eq!(pick_files(&files, &want(WishlistScope::Album)), Some(vec![0, 1, 2]));
        // By number when the name is spelled differently.
        let mut w = want(WishlistScope::Track);
        w.title = Some("Voyna (War)".into());
        assert_eq!(pick_files(&files, &w), Some(vec![2]));
        let mut w = want(WishlistScope::Track);
        w.title = Some("Звезда".into());
        w.track_no = 9;
        assert_eq!(pick_files(&files, &w), None);
        // Disc folders.
        let discs = vec![
            (0, "Album/CD1/03 - Война.flac".into(), 30),
            (1, "Album/CD2/03 - Война.flac".into(), 30),
        ];
        let mut w = want(WishlistScope::Track);
        w.disc_no = 2;
        assert_eq!(pick_files(&discs, &w), Some(vec![1]));
        assert_eq!(disc_of("A/Disc 2/01.flac"), Some(2));
        assert_eq!(disc_of("A/01.flac"), None);
    }
}
