//! «Найти качество лучше» of an album (docs/TODO.md 57): an album that
//! came as MP3, wanted in good quality. A better copy of the album is
//! looked for through the wishlist's own search (the sources in the user's
//! order, the ranking, the waits) as an album item that looks for a better
//! copy, asked for by hand ([`hand_album`]). The best sure offer downloads
//! at once, but nothing is imported yet: its files are matched to the
//! album's tracks ([`nikgob_import::album_files`], the importer's own test)
//! and wait as ONE card in «Подтверди», each track that gets a really
//! better file ([`nikgob_core::quality::really_better`]) with what replaces
//! what. Tracks with the best file there is are left alone. «Заменить»
//! imports those files as any download is (each upgrades its track in
//! place, the old file goes to the library's Trash); one «Отменить» takes
//! the whole album back ([`nikgob_import::undo_replacement`]). «Оставить
//! как есть»: the downloaded files go and nothing changed. Every step is
//! logged. Files that wait survive a restart (the library cache keeps
//! where they are).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use nikgob_core::model::{
    Master, ReleaseId, SourceId, SourceOrigin, Track, TrackId, WishlistId, WishlistItem, WishlistOrigin, WishlistScope,
    WishlistStatus, quality_score, score,
};
use nikgob_core::source::Format;
use nikgob_import::tags::{FileInfo, TagSet};
use nikgob_import::{AlbumTrack, ImportTargets, Replacement};
use nikgob_view::{self as v, Command};

use super::upgrade::{FileRank, file_label, format_words};
use super::{Next, WishMsg};
use crate::Worker;
use crate::streams::Msg;

/// An album item that looks for a better copy the user asked for by hand
/// («Найти качество лучше» of the album): its files wait for «Заменить».
pub(crate) fn hand_album(item: &WishlistItem) -> bool {
    item.origin == WishlistOrigin::Upgrade && item.scope == WishlistScope::Album && item.better_than.is_some()
}

/// A downloaded file as its tags say, read off the worker.
pub(crate) struct CameFile {
    pub(crate) path: PathBuf,
    pub(crate) tags: TagSet,
    pub(crate) info: FileInfo,
}

/// A track of the album with its file, as a copy is matched to it.
pub(crate) struct Slot {
    pub(crate) track: TrackId,
    pub(crate) title: String,
    pub(crate) disc_no: u32,
    pub(crate) track_no: u32,
    pub(crate) duration_ms: Option<u32>,
    /// Its file as the rule of a better copy reads it, and in words.
    pub(crate) rank: FileRank,
    pub(crate) have: String,
}

/// One track a copy replaces the file of, and with what.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PlanRow {
    pub(crate) track: TrackId,
    /// «3. Сахарный».
    pub(crate) title: String,
    pub(crate) path: PathBuf,
    /// What its file is («MP3 320») and what comes («FLAC»).
    pub(crate) have: String,
    pub(crate) to: String,
}

/// The files of a copy that wait for «Заменить».
struct Staged {
    release: ReleaseId,
    origin: SourceOrigin,
    plan: Vec<PlanRow>,
}

/// A track as it was before «Заменить».
struct Before {
    track: Track,
    old: SourceId,
    master: Option<Master>,
}

/// What «Заменить» changes, while its import runs.
struct Replacing {
    release: ReleaseId,
    before: Vec<Before>,
}

#[derive(Default)]
pub(crate) struct AlbumBetter {
    /// Copies that came and wait in «Подтверди», by item.
    staged: HashMap<WishlistId, Staged>,
    /// Copies being imported after «Заменить», by item.
    replacing: HashMap<WishlistId, Replacing>,
    /// What «Отменить» takes back, by album.
    undo: HashMap<ReleaseId, Vec<Replacement>>,
}

/// Where the files that wait are, kept across a restart.
#[derive(serde::Serialize, serde::Deserialize)]
struct Kept {
    origin: SourceOrigin,
    files: Vec<PathBuf>,
}

fn kept_key(id: WishlistId) -> String {
    format!("better_album.{id}")
}

/// What came, as a format.
fn came_format(info: &FileInfo) -> Format {
    Format {
        codec: info.codec.clone(),
        bitrate_kbps: info.bitrate_kbps,
        bit_depth: info.bit_depth,
        sample_rate: info.sample_rate,
    }
}

/// «3. Сахарный», or the title alone without a number.
fn row_title(s: &Slot) -> String {
    if s.track_no > 0 { format!("{}. {}", s.track_no, s.title) } else { s.title.clone() }
}

/// What each track of an album gets from a copy that came: the file
/// matched to it ([`nikgob_import::album_files`]), only when it is really
/// better than the track's own ([`FileRank::beaten_by`]; ranked by its
/// format, the import checks it again). Tracks that get nothing are left
/// out: they stay as they are.
pub(crate) fn plan_of(slots: &[Slot], files: &[CameFile]) -> Vec<PlanRow> {
    let tracks: Vec<AlbumTrack<'_>> = slots
        .iter()
        .map(|s| AlbumTrack { title: &s.title, disc_no: s.disc_no, track_no: s.track_no, duration_ms: s.duration_ms })
        .collect();
    let refs: Vec<(&TagSet, &Path, Option<u32>)> = files
        .iter()
        .map(|f| (&f.tags, f.path.as_path(), Some(f.info.duration_ms()).filter(|d| *d > 0)))
        .collect();
    nikgob_import::album_files(&tracks, &refs)
        .into_iter()
        .zip(slots)
        .filter_map(|(m, s)| {
            let f = &files[m?];
            let q = quality_score(&f.info.codec, f.info.bitrate_kbps, f.info.sample_rate, f.info.bit_depth, false);
            s.rank.beaten_by(q).then(|| PlanRow {
                track: s.track,
                title: row_title(s),
                path: f.path.clone(),
                have: s.have.clone(),
                to: format_words(&came_format(&f.info)),
            })
        })
        .collect()
}

/// The «Подтверди» card of a copy that waits: every track it replaces the
/// file of and with what; nothing to choose, «Заменить» or not.
pub(crate) fn album_card(id: u64, title: String, plan: &[PlanRow]) -> v::Confirm {
    v::Confirm {
        yes_no: true,
        id,
        title,
        origin: "Качество лучше · альбом".into(),
        question: "Заменить файлы этих треков?".into(),
        options: plan
            .iter()
            .map(|r| v::ConfirmOption { label: r.title.clone(), detail: format!("{} вместо {}", r.to, r.have), recommended: false })
            .collect(),
        skip_label: "Оставить как есть".into(),
        accept_label: "Заменить".into(),
        pick: None,
    }
}

impl Worker {
    /// A track's file a genuine lossless copy would really be better than.
    fn track_can_get_better(&self, track: TrackId) -> bool {
        self.upgrade_wants(track)
            && self
                .lib
                .active_source(track)
                .ok()
                .flatten()
                .filter(|s| s.kind.is_local() && s.available && s.trashed_at.is_none() && s.quality_score > score::SAVED_STREAM)
                .is_some_and(|s| self.file_rank(&s).beaten_by(score::CD_LOSSLESS))
    }

    /// «Найти качество лучше» of an album: its files that can get better
    /// are looked for in a better copy now, which waits in «Подтверди»
    /// before anything changes. None of them can: said at once.
    pub(crate) fn find_better_album(&mut self, release: ReleaseId) {
        let Some(r) = self.lib.get_release(release).ok().flatten() else { return };
        let name = format!("{} — {}", r.artist_credit.display(), r.title);
        let tracks = self.lib.tracks_of_release(release).unwrap_or_default();
        let can: Vec<TrackId> = tracks.iter().map(|t| t.id).filter(|t| self.track_can_get_better(*t)).collect();
        let items = self.lib.wishlist_for_release(release).unwrap_or_default();
        if can.is_empty() {
            log::info!("upgrade: the user asks for a better copy of {name}: none of its files can get one");
            for i in &items {
                let open = !matches!(
                    i.status,
                    WishlistStatus::Searching | WishlistStatus::Downloading | WishlistStatus::Confirm | WishlistStatus::Done
                );
                if i.origin == WishlistOrigin::Upgrade && i.scope == WishlistScope::Album && open {
                    self.upgrade_drop(i.id);
                }
            }
            self.toast(format!("Лучше не нашёл: {name}"), None);
            return;
        }
        // Something of the album on its way or waiting in «Подтверди».
        let busy = |i: &WishlistItem| matches!(i.status, WishlistStatus::Searching | WishlistStatus::Downloading | WishlistStatus::Confirm);
        let on_its_way = items.iter().any(|i| i.scope == WishlistScope::Album && busy(i))
            || tracks
                .iter()
                .any(|t| self.lib.wishlist_for_track(t.id).unwrap_or_default().iter().any(|i| i.scope == WishlistScope::Track && busy(i)));
        if on_its_way {
            self.download_toast(0, 1, None);
            return;
        }
        if self.import.is_none() {
            self.toast("Импорт недоступен", None);
            return;
        }
        if !self.can_download() {
            self.toast_open("Нужен источник скачивания", "Настроить", v::Link::Setting(v::SettingLink::Torrents));
            return;
        }
        // An offer must beat its worst file at least; each track then takes
        // only what is really better than its own.
        let floor = can
            .iter()
            .filter_map(|t| self.lib.active_source(*t).ok().flatten())
            .map(|s| s.quality_score)
            .min()
            .unwrap_or(score::SAVED_STREAM);
        let again = items
            .into_iter()
            .find(|i| i.scope == WishlistScope::Album && i.origin == WishlistOrigin::Upgrade && i.status != WishlistStatus::Done);
        let item = match again {
            Some(i) => {
                self.upgrade_forget(i.id);
                self.set_item(i.id, |i| {
                    i.status = WishlistStatus::New;
                    i.next_try_at = None;
                    i.note = None;
                    i.candidates_json = None;
                    i.better_than = Some(floor);
                })
            }
            None => self.item_for(None, Some(release)).map(|mut i| {
                i.scope = WishlistScope::Album;
                i.origin = WishlistOrigin::Upgrade;
                i.status = WishlistStatus::New;
                i.better_than = Some(floor);
                i
            }),
        };
        let Some(item) = item else { return };
        self.put_item(&item);
        log::info!("upgrade: the user asks for a better copy of {name}: {} of its files can get one (worst {floor})", can.len());
        let wishlist = v::Link::Route(v::Route::Downloads(v::DownloadsTab::Wishlist));
        self.toast_open(format!("Ищу качество лучше: «{}»", r.title), "Открыть", wishlist);
        self.streaming.wish.upgrade_runs.push(Instant::now());
        self.wish_search(item);
        self.publish_wishlist();
    }

    /// The torrent of `item` finished: a better copy of an album asked
    /// for by hand is matched first, not imported. True when it took it.
    pub(crate) fn album_better_torrent(&mut self, item: WishlistId, files: &[PathBuf], info_hash: &str) -> bool {
        if !self.lib.get_wishlist_item(item).ok().flatten().is_some_and(|i| hand_album(&i)) {
            return false;
        }
        let origin = SourceOrigin::Torrent { info_hash: info_hash.to_owned(), path_in_torrent: String::new() };
        self.album_came(item, files.to_vec(), origin);
        true
    }

    /// The files of a torrent wait for «Заменить» (or are being imported
    /// after it): its cleanup leaves them.
    pub(crate) fn album_better_holds(&self, info_hash: &str) -> bool {
        let a = &self.streaming.wish.album;
        if a.staged.is_empty() && a.replacing.is_empty() {
            return false;
        }
        self.lib
            .get_download(info_hash)
            .ok()
            .flatten()
            .and_then(|d| d.wishlist_id)
            .is_some_and(|w| a.staged.contains_key(&w) || a.replacing.contains_key(&w))
    }

    /// A copy of the album came for `id`: kept where it is, its files read.
    pub(super) fn album_came(&mut self, id: WishlistId, files: Vec<PathBuf>, origin: SourceOrigin) {
        self.streaming.wish.partial.remove(&id);
        let kept = Kept { origin: origin.clone(), files: files.clone() };
        if let Ok(json) = serde_json::to_string(&kept)
            && let Err(e) = self.lib.cache_put(&kept_key(id), Self::now(), &json)
        {
            log::warn!("upgrade: where the copy of {id} waits is not kept: {e}");
        }
        log::info!("upgrade: {} files of a better copy came for {}, matched before anything changes", files.len(), self.item_title(id));
        let tx = self.streaming.internal.clone();
        let Some(svc) = &self.import else { return };
        svc.spawn_blocking(move || {
            let came: Vec<CameFile> = files
                .into_iter()
                .filter_map(|path| match nikgob_import::tags::read_tags(&path) {
                    Ok((tags, info)) => Some(CameFile { path, tags, info }),
                    Err(e) => {
                        log::warn!("upgrade: {} is not read: {e}", path.display());
                        None
                    }
                })
                .collect();
            let _ = tx.send(crate::import::Internal::Stream(Msg::Wish(WishMsg::AlbumCame { item: id, origin, files: came })));
        });
    }

    /// The files of a copy were read: the tracks they replace wait in
    /// «Подтверди» as one card. None is better: they go, and the next
    /// offer is tried (or the click is answered «Лучше не нашёл»).
    pub(super) fn album_staged(&mut self, id: WishlistId, origin: SourceOrigin, files: Vec<CameFile>) {
        let item = self.lib.get_wishlist_item(id).ok().flatten();
        let Some(item) = item.filter(|i| !matches!(i.status, WishlistStatus::Dismissed | WishlistStatus::Done)) else {
            self.album_files_go(id, &origin);
            return;
        };
        let release = item
            .release_id
            .or_else(|| nikgob_import::wished_release(&self.lib, &item).ok().flatten().map(|r| r.id));
        let Some(release) = release else {
            self.album_files_go(id, &origin);
            return;
        };
        let slots: Vec<Slot> = self
            .lib
            .tracks_of_release(release)
            .unwrap_or_default()
            .into_iter()
            .filter(|t| self.upgrade_wants(t.id))
            .filter_map(|t| {
                let s = self
                    .lib
                    .active_source(t.id)
                    .ok()
                    .flatten()
                    .filter(|s| s.kind.is_local() && s.available && s.trashed_at.is_none() && s.quality_score > score::SAVED_STREAM)?;
                Some(Slot {
                    track: t.id,
                    title: t.title,
                    disc_no: t.disc_no,
                    track_no: t.track_no,
                    duration_ms: t.duration_ms,
                    rank: self.file_rank(&s),
                    have: file_label(&s),
                })
            })
            .collect();
        let plan = plan_of(&slots, &files);
        let title = self.item_title(id);
        if plan.is_empty() {
            log::info!("upgrade: nothing of the {} files that came for {title} is better than the album's", files.len());
            self.album_files_go(id, &origin);
            self.wish_line(id, title, "Скачанное не лучше прежних файлов — оставил их".into(), None);
            if matches!(self.wish_next_candidate(id), Next::NotFound) {
                self.upgrade_none(id, true);
            }
            return;
        }
        for r in &plan {
            log::info!("upgrade: {title}: «{}» {} → {} ({}), waits for «Заменить»", r.title, r.have, r.to, r.path.display());
        }
        let n = plan.len();
        self.streaming.wish.album.staged.insert(id, Staged { release, origin, plan });
        self.set_item(id, |i| {
            i.status = WishlistStatus::Confirm;
            i.next_try_at = None;
        });
        let what = crate::streams::plural(n, "трека", "треков", "треков");
        self.wish_line(id, title, format!("Нашёл качество лучше для {n} {what} · подтверди замену в «Подтверди»"), None);
        self.publish_wishlist();
    }

    /// The card of a copy of the album that waits, when one does.
    pub(super) fn album_better_card(&self, item: &WishlistItem, id: u64) -> Option<v::Confirm> {
        let st = self.streaming.wish.album.staged.get(&item.id)?;
        let title = match self.lib.get_release(st.release).ok().flatten() {
            Some(r) => format!("{} — {}", r.title, r.artist_credit.display()),
            None => item.title.clone().unwrap_or_default(),
        };
        Some(album_card(id, title, &st.plan))
    }

    /// The answer of the card of `id`, when it is a copy of an album that
    /// waits: `Some(_)` «Заменить», `None` «Оставить как есть». False for
    /// any other card.
    pub(super) fn album_better_answer(&mut self, id: WishlistId, option: Option<usize>) -> bool {
        if !self.streaming.wish.album.staged.contains_key(&id) {
            return false;
        }
        match option {
            Some(_) => self.album_replace(id),
            None => self.album_keep(id),
        }
        self.publish_wishlist();
        true
    }

    /// «Оставить как есть»: the copy goes, nothing changed, and the item is
    /// turned down (a new click asks again).
    fn album_keep(&mut self, id: WishlistId) {
        let Some(st) = self.streaming.wish.album.staged.remove(&id) else { return };
        log::info!("upgrade: the user keeps the files of {}, the copy goes", self.item_title(id));
        self.album_files_go(id, &st.origin);
        self.upgrade_forget(id);
        self.set_item(id, |i| {
            i.status = WishlistStatus::Dismissed;
            i.next_try_at = None;
            i.note = Some("Оставил прежние файлы".into());
        });
        self.wish_line_drop(id);
    }

    /// «Заменить»: the files of the plan are imported for their tracks as
    /// any download is; what each track was is kept for «Отменить».
    fn album_replace(&mut self, id: WishlistId) {
        let Some(st) = self.streaming.wish.album.staged.remove(&id) else { return };
        self.album_forget_kept(id);
        // A torrent's files may have gone meanwhile.
        let plan: Vec<PlanRow> = st.plan.into_iter().filter(|r| r.path.exists()).collect();
        if plan.is_empty() {
            log::warn!("upgrade: the files of the copy for {} are gone, it looks again", self.item_title(id));
            self.toast("Скачанные файлы пропали — поищу снова", None);
            self.wish_retry(id);
            return;
        }
        let mut before = Vec::new();
        for r in &plan {
            let Some(track) = self.lib.get_track(r.track).ok().flatten() else { continue };
            let Some(src) = self.lib.active_source(r.track).ok().flatten().filter(|s| s.kind.is_local()) else { continue };
            log::info!("upgrade: replacing «{}» {} ({}) with {} ({})", r.title, r.have, src.locator, r.to, r.path.display());
            before.push(Before { track, old: src.id, master: self.lib.get_master(r.track).ok().flatten() });
        }
        let targets = ImportTargets {
            tracks: plan.iter().map(|r| r.track).collect(),
            release: Some(st.release),
            wishlist: Some(id),
            only_targets: true,
            from: self.streaming.wish.offer_keys.get(&id).cloned(),
            ..ImportTargets::default()
        };
        let title = format!("Качество лучше: {}", self.item_title(id));
        let files: Vec<PathBuf> = plan.iter().map(|r| r.path.clone()).collect();
        if self.import_for(files, title, st.origin, targets) {
            self.streaming.wish.album.replacing.insert(id, Replacing { release: st.release, before });
            self.set_item(id, |i| {
                i.status = WishlistStatus::Downloading;
                i.note = None;
            });
        }
    }

    /// The import after «Заменить» ended: what it replaced is kept for
    /// «Отменить» (the toast's), and the item is done. Nothing replaced
    /// (what came was a fake after all, or not the songs): «Лучше не
    /// нашёл». False when the item was no such import.
    pub(super) fn album_replaced(&mut self, item: &WishlistItem) -> bool {
        let Some(rep) = self.streaming.wish.album.replacing.remove(&item.id) else { return false };
        let mut done: Vec<Replacement> = Vec::new();
        for b in rep.before {
            match self.lib.active_source(b.track.id).ok().flatten() {
                Some(s) if s.id != b.old && s.kind.is_local() => {
                    log::info!("upgrade: «{}» now plays {}, the old file is in the Trash", b.track.title, s.locator);
                    done.push(Replacement { track: b.track.id, old: b.old, new: s.id, master_before: b.master, before: b.track });
                }
                _ => log::info!("upgrade: «{}» kept its file", b.track.title),
            }
        }
        if done.is_empty() {
            self.upgrade_none(item.id, true);
            return true;
        }
        let album = self.lib.get_release(rep.release).ok().flatten().map(|r| r.title).unwrap_or_default();
        let n = done.len();
        self.streaming.wish.album.undo.insert(rep.release, done);
        if item.status != WishlistStatus::Done {
            self.upgrade_done(item);
        }
        let what = crate::streams::plural(n, "файл", "файла", "файлов");
        self.toast(
            format!("«{album}»: заменил {n} {what} · прежние в Корзине"),
            Some(("Отменить".into(), Command::UndoBetterAlbum(rep.release))),
        );
        self.publish(|s| {
            if let Some(t) = s.toasts.last_mut() {
                t.long = true;
            }
        });
        true
    }

    /// «Отменить» of a replaced album: every file comes back from the
    /// Trash, the new ones go there.
    pub(crate) fn undo_better_album(&mut self, release: ReleaseId) {
        let Some(list) = self.streaming.wish.album.undo.remove(&release) else {
            log::info!("upgrade: nothing of {release} to take back");
            return;
        };
        let Some(svc) = &self.import else {
            self.streaming.wish.album.undo.insert(release, list);
            return;
        };
        let (lib, trash, tx) = (self.lib.clone(), svc.config().trash_dir.clone(), self.streaming.internal.clone());
        log::info!("upgrade: the user takes back {} replaced files of {release}", list.len());
        svc.spawn_blocking(move || {
            let (mut done, mut failed, mut left) = (0, 0, Vec::new());
            for r in list {
                match nikgob_import::undo_replacement(&lib, &trash, &r) {
                    Ok(()) => done += 1,
                    Err(e) => {
                        log::warn!("upgrade: «{}» stays replaced: {e}", r.before.title);
                        failed += 1;
                        left.push(r);
                    }
                }
            }
            let _ = tx.send(crate::import::Internal::Stream(Msg::Wish(WishMsg::AlbumRestored { release, done, failed, left })));
        });
    }

    /// The album's files came back (`failed` could not, kept to try again):
    /// said, and its item is turned down, not looked for again by itself.
    pub(super) fn album_restored(&mut self, release: ReleaseId, done: usize, failed: usize, left: Vec<Replacement>) {
        log::info!("upgrade: {done} files of {release} are back, {failed} not");
        let retry = (!left.is_empty()).then(|| ("Повторить".to_owned(), Command::UndoBetterAlbum(release)));
        if !left.is_empty() {
            self.streaming.wish.album.undo.insert(release, left);
        }
        let text = if failed == 0 {
            format!("Вернул прежние файлы: {done}")
        } else {
            format!("Вернул прежние файлы: {done}, не получилось: {failed}")
        };
        self.toast(text, retry);
        for i in self.lib.wishlist_for_release(release).unwrap_or_default() {
            if i.origin == WishlistOrigin::Upgrade && i.scope == WishlistScope::Album && done > 0 {
                self.set_item(i.id, |i| {
                    i.status = WishlistStatus::Dismissed;
                    i.next_try_at = None;
                    i.note = Some("Вернул прежние файлы".into());
                });
            }
        }
        self.reload_after_import();
        self.publish_wishlist();
    }

    /// The files of a copy that will not be imported go: a torrent with
    /// what it downloaded (its files stay when they are masters of the
    /// library), a direct download's folder.
    fn album_files_go(&mut self, id: WishlistId, origin: &SourceOrigin) {
        self.album_forget_kept(id);
        match origin {
            SourceOrigin::Torrent { info_hash, .. } => {
                let keep = !self.lib.masters_of_torrent(info_hash).unwrap_or_default().is_empty();
                if keep {
                    log::info!("upgrade: the torrent of {id} holds masters of the library, its files stay");
                    let _ = self.lib.delete_download(info_hash);
                } else {
                    self.cancel_wish_torrent(id);
                }
            }
            _ => self.drop_direct(id),
        }
    }

    /// Where the files of a copy wait, as the cache keeps it.
    fn album_kept(&self, id: WishlistId) -> Option<Kept> {
        let (_, json) = self.lib.cache_get(&kept_key(id)).ok().flatten()?;
        serde_json::from_str(&json).ok()
    }

    fn album_forget_kept(&mut self, id: WishlistId) {
        if let Some((_, json)) = self.lib.cache_get(&kept_key(id)).ok().flatten() {
            let _ = self.lib.cache_forget(&kept_key(id), &json);
        }
    }

    /// The item was turned down («Не качать»): a copy of it that waited
    /// goes.
    pub(super) fn album_dismissed(&mut self, id: WishlistId) {
        if let Some(st) = self.streaming.wish.album.staged.remove(&id) {
            log::info!("upgrade: {id} turned down, the copy that waited goes");
            self.album_files_go(id, &st.origin);
        }
    }

    /// After a restart, the items whose copies waited in «Подтверди»
    /// (their folders must stay).
    pub(super) fn album_waiting(&self) -> Vec<WishlistId> {
        self.lib
            .list_wishlist_by_status(WishlistStatus::Confirm)
            .unwrap_or_default()
            .into_iter()
            .filter(|i| hand_album(i) && self.album_kept(i.id).is_some())
            .map(|i| i.id)
            .collect()
    }

    /// After a restart: a copy that waited is read again and asks again;
    /// one whose files are gone is looked for again.
    pub(super) fn album_restage(&mut self, id: WishlistId) {
        if self.import.is_none() {
            return;
        }
        let Some(k) = self.album_kept(id) else { return };
        if k.files.is_empty() || !k.files.iter().all(|f| f.exists()) {
            log::info!("upgrade: the copy that waited for {id} is gone, it looks again");
            self.album_forget_kept(id);
            self.set_item(id, |i| {
                i.status = WishlistStatus::New;
                i.next_try_at = None;
                i.candidates_json = None;
            });
            return;
        }
        if !matches!(k.origin, SourceOrigin::Torrent { .. }) {
            let dir = self.direct_dir(id);
            self.streaming.wish.direct.insert(id, dir);
        }
        // No card until its files are read again.
        self.set_item(id, |i| i.status = WishlistStatus::Downloading);
        self.album_came(id, k.files, k.origin);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use nikgob_core::model::Codec;
    use nikgob_core::quality::{QualityCheck, SoundOrigin, Verdict};

    use super::*;

    fn slot(title: &str, no: u32, secs: u32, codec: Codec, rank: i32) -> Slot {
        Slot {
            track: TrackId::new(),
            title: title.into(),
            disc_no: 1,
            track_no: no,
            duration_ms: Some(secs * 1000),
            rank: FileRank { codec, rank, check: None },
            have: "MP3 320".into(),
        }
    }

    fn came(title: &str, no: u32, secs: u32, codec: Codec, bitrate: Option<u32>) -> CameFile {
        CameFile {
            path: PathBuf::from(format!("/dl/Album/{no:02} - {title}.flac")),
            tags: TagSet { title: Some(title.into()), track_no: Some(no), ..TagSet::default() },
            info: FileInfo {
                codec,
                duration: Duration::from_secs(u64::from(secs)),
                bitrate_kbps: bitrate,
                sample_rate: Some(44_100),
                bit_depth: Some(16),
                channels: Some(2),
            },
        }
    }

    /// A FLAC album over an album of MP3s, one of which is already our
    /// Opus of a genuine FLAC and one an MP3 320 the copy has only as an
    /// MP3 320: each MP3 gets its FLAC, the Opus and the same-quality MP3
    /// are left alone, a bonus song of the copy replaces nothing.
    #[test]
    fn each_track_takes_only_a_really_better_file() {
        let mp3_320 = quality_score(&Codec::Mp3, Some(320), None, None, false);
        let slots = [
            slot("Интро", 1, 60, Codec::Mp3, 500),
            slot("Сахарный", 2, 200, Codec::Opus, score::CD_LOSSLESS),
            slot("Последний", 3, 240, Codec::Mp3, mp3_320),
            slot("Финал", 4, 300, Codec::Mp3, mp3_320),
        ];
        let files = [
            came("Финал", 4, 300, Codec::Mp3, Some(320)),
            came("Последний", 3, 240, Codec::Flac, None),
            came("Бонус", 5, 180, Codec::Flac, None),
            came("Сахарный", 2, 200, Codec::Flac, None),
            came("Интро", 1, 60, Codec::Flac, None),
        ];
        let plan = plan_of(&slots, &files);
        let got: Vec<(&str, &Path, &str)> = plan.iter().map(|r| (r.title.as_str(), r.path.as_path(), r.to.as_str())).collect();
        assert_eq!(
            got,
            [
                ("1. Интро", Path::new("/dl/Album/01 - Интро.flac"), "FLAC"),
                ("3. Последний", Path::new("/dl/Album/03 - Последний.flac"), "FLAC"),
            ]
        );
        assert_eq!(plan[0].track, slots[0].track);
        assert_eq!(plan[1].track, slots[2].track);
        // A fake FLAC of the album (checked) gets the genuine one too.
        let fake = QualityCheck {
            verdict: Verdict::Transcoded,
            cutoff_hz: 16_000.0,
            steepness_db: 60.0,
            origin: SoundOrigin::estimate(Verdict::Transcoded, 16_000.0),
            sample_rate: 44_100,
            spectrum_db: Vec::new(),
            made_from: None,
            file_size: 1,
            file_mtime: 1,
            checked_at: 1,
        };
        let mut fake_slot = slot("Сахарный", 2, 200, Codec::Flac, score::FAKE_LOSSLESS);
        fake_slot.rank.check = Some(fake.summary());
        assert_eq!(plan_of(&[fake_slot], &files).len(), 1);
        assert!(plan_of(&slots, &[]).is_empty());
    }

    /// One card for the whole album: each track and what replaces its
    /// file, nothing to pick, «Заменить» or «Оставить как есть».
    #[test]
    fn the_card_lists_what_replaces_what() {
        let plan = [PlanRow {
            track: TrackId::new(),
            title: "1. Интро".into(),
            path: PathBuf::from("/dl/01.flac"),
            have: "MP3 128".into(),
            to: "FLAC".into(),
        }];
        let card = album_card(7, "Сахарный — SugaRiddle".into(), &plan);
        assert!(card.yes_no && card.pick.is_none());
        assert_eq!(card.options[0].label, "1. Интро");
        assert_eq!(card.options[0].detail, "FLAC вместо MP3 128");
        assert!(!card.options[0].recommended, "never accepted with the recommended ones");
        assert_eq!((card.accept_label.as_str(), card.skip_label.as_str()), ("Заменить", "Оставить как есть"));
    }
}
