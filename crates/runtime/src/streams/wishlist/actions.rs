use super::*;
use super::retry::offers_of_json;

impl Worker {
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
            self.toast_info(nikgob_core::i18n::ErrorStrings::fmt_queued_for_download_in(self.locale(), n), None);
        }
        self.wish_tick();
        self.publish_wishlist();
    }

    /// «Не качать» on items, and a download of theirs the user cancelled
    /// (its torrent's or its import's «Отменить»): nothing of them is
    /// fetched on. What is on its way stops (its torrent with what it
    /// downloaded, a direct download's folder once it ends, an import
    /// taking back what it added). An item the user's own «Скачать» made
    /// leaves «Хочу скачать» altogether ([`cancel_drops`]); any other is
    /// turned down and can come back.
    pub(crate) fn wish_dismiss(&mut self, ids: &[WishlistId]) {
        let mut title = None;
        let mut n = 0;
        let mut dismissed: Vec<WishlistId> = Vec::new();
        let (mut tracks, mut albums): (Vec<TrackId>, Vec<ReleaseId>) = (Vec::new(), Vec::new());
        for &id in ids {
            let Some(item) = self.lib.get_wishlist_item(id).ok().flatten() else { continue };
            let w = &mut self.streaming.wish;
            w.cands.remove(&id);
            w.doubtful.remove(&id);
            w.probe.remove(&id);
            w.partial.remove(&id);
            w.retry_only.remove(&id);
            w.later.remove(&id);
            w.upgrade_dups.remove(&id);
            w.from.remove(&id);
            w.live.lock().remove(&id);
            // A download of another source stops (a Soulseek peer is told);
            // its folder goes once it has.
            let stopped = self.stop_run(id);
            self.album_dismissed(id);
            // A torrent on its way goes with what it downloaded (a better
            // copy's always: it must never replace a file); a seeding one
            // of an item done stays.
            let on_its_way = matches!(item.status, WishlistStatus::Downloading | WishlistStatus::Searching | WishlistStatus::Confirm);
            // Its imports stop first; the torrent's files go only once they
            // took back what they added ([`Self::cancel_wish_torrent`]).
            if on_its_way {
                self.cancel_imports_of(id);
            }
            if item.origin == WishlistOrigin::Upgrade || on_its_way {
                self.cancel_wish_torrent(id);
            }
            self.wish_line_drop(id);
            n += 1;
            title.clone_from(&item.title);
            if cancel_drops(&item) {
                log::info!("wishlist: {} cancelled, it leaves «Хочу скачать»", item.title.as_deref().unwrap_or("?"));
                let _ = self.lib.delete_wishlist_item(id);
                self.streaming.wish.confirm_ids.remove(&id);
                if !stopped {
                    self.drop_direct(id);
                }
                match (item.scope, item.track_id, item.release_id) {
                    (WishlistScope::Track, Some(t), _) => tracks.push(t),
                    (WishlistScope::Album, _, Some(r)) => albums.push(r),
                    _ => {}
                }
                continue;
            }
            self.set_item(id, |i| {
                i.status = WishlistStatus::Dismissed;
                i.next_try_at = None;
            });
            dismissed.push(id);
        }
        if n > 0 {
            let loc = self.locale();
            let text = match (n, title) {
                (1, Some(t)) => nikgob_core::i18n::ErrorStrings::fmt_wont_download_one_in(loc, &t),
                (n, _) => nikgob_core::i18n::ErrorStrings::fmt_wont_download_in(loc, n),
            };
            // «Отменить» brings back what was turned down, or asks again
            // for what left.
            let undo = match (dismissed.is_empty(), tracks.is_empty(), albums.as_slice()) {
                (false, true, []) => Command::WishlistRestore(dismissed),
                (true, false, []) => Command::DownloadTracks(tracks),
                (true, true, [r]) => Command::DownloadRelease(*r),
                // Of several kinds: all of it at once.
                _ => Command::WishlistUndoDismiss { restore: dismissed, tracks, albums },
            };
            let act = self.undo_action(undo);
            self.toast_info(text, Some(act));
        }
        self.publish_wishlist();
    }

    /// `Command::WishlistUndoDismiss`: a «Не качать» of items of several
    /// kinds taken back at once.
    pub(crate) fn wish_undo_dismiss(&mut self, restore: &[WishlistId], tracks: &[TrackId], albums: &[ReleaseId]) {
        for &r in albums {
            let _ = self.wish_release_now(r);
        }
        for &t in tracks {
            let _ = self.wish_track_now(t);
        }
        self.wish_restore(restore);
        self.refresh_stream_page();
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
        // The user asks: the day's count starts over.
        self.streaming.wish.tries.forget(id);
        self.save_backoff();
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
                self.streaming.wish.tries.forget(item.id);
                self.put_item(&item);
                n += 1;
            }
        }
        // And what waits to be searched again after a source turned it away.
        for mut item in self.lib.list_wishlist_by_status(WishlistStatus::New).unwrap_or_default() {
            if item.origin != WishlistOrigin::Upgrade && item.next_try_at.is_some_and(|t| t > now) {
                self.streaming.wish.later.remove(&item.id);
                self.streaming.wish.retry_only.remove(&item.id);
                self.streaming.wish.tries.forget(item.id);
                item.next_try_at = None;
                item.updated_at = now;
                self.put_item(&item);
                n += 1;
            }
        }
        if n > 0 {
            self.save_backoff();
            self.toast_work(nikgob_core::i18n::ErrorStrings::fmt_searching_again_in(self.locale(), n), None);
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
        // A copy of an album that came: «Заменить» or «Оставить как есть».
        if self.album_better_answer(id, option) {
            return;
        }
        let Some(item) = self.lib.get_wishlist_item(id).ok().flatten() else { return };
        let offers = item.candidates_json.as_deref().map(offers_of_json).unwrap_or_default();
        match option.and_then(|i| offers.get(i).cloned()) {
            Some(c) => {
                self.set_item(id, |i| i.status = WishlistStatus::Searching);
                self.streaming.wish.cands.insert(id, vec![c.clone()]);
                self.streaming.wish.doubtful.remove(&id);
                self.wish_fetch(id, c);
            }
            // A copy asked for by hand that the user keeps: turned down.
            None if self.upgrade_kept(id) => {}
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
        let err = nikgob_core::i18n::errors_in(self.locale());
        let Some(track) = item.track_id else {
            self.toast_err(err.youtube_save_tracks_only, None);
            return;
        };
        let Some(yt) = self.service(Service::Youtube) else {
            self.toast_err(err.youtube_disabled, None);
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
                self.toast_work(err.finding_on_youtube, None);
            } else {
                self.toast_err(err.not_found_on_youtube, None);
            }
            return;
        };
        self.set_item(id, |i| {
            i.status = WishlistStatus::Downloading;
            i.note = Some(note_code(WishNote::SavingYoutube));
        });
        let dir = self.save_dir();
        let run = self.streaming.wish.runs.reserve();
        let task = self.streaming.spawn(async move {
            let result = yt.save_audio(&source.locator, &dir).await;
            Msg::Wish(WishMsg::Saved { item: id, track, run, result })
        });
        self.streaming.wish.from.insert(id, "YouTube".into());
        let title = self.item_title(id);
        self.streaming.wish.runs.start(id, run, title, "YouTube".into(), None, None, Some(task));
        self.publish_wishlist();
        self.publish_running();
    }

    pub(super) fn wish_saved(&mut self, id: WishlistId, track: TrackId, run: u64, result: Result<PathBuf, StreamError>) {
        // Stopped (✕, «Не качать») or started again since: what it saved
        // goes, the item stays as it is now.
        if !self.streaming.wish.runs.is_current(id, run) {
            log::info!("wishlist: a save from YouTube of {id} came after it was stopped, dropped");
            if let Ok(path) = result {
                let _ = std::fs::remove_file(path);
            }
            return;
        }
        self.streaming.wish.runs.end(id);
        match result {
            Ok(path) => {
                self.streaming.wish.saving.insert(path.clone(), id);
                let targets = ImportTargets {
                    tracks: vec![track],
                    force: true,
                    ..ImportTargets::default()
                };
                if !self.import_for(vec![path], StoredLine::SavingFromYoutube.encode(), SourceOrigin::Stream, targets) {
                    self.set_item(id, |i| i.status = WishlistStatus::NoTorrent);
                }
            }
            Err(e) => {
                log::warn!("saving from YouTube: {e}");
                let loc = self.locale();
                let msg = nikgob_core::i18n::ErrorStrings::youtube_save_failed_in(loc, &super::super::error_text_in(&e, loc));
                self.toast_err(msg, None);
                self.set_item(id, |i| {
                    i.status = WishlistStatus::NoTorrent;
                    i.note = Some(note_code(WishNote::SaveFailed));
                });
                self.publish_wishlist();
            }
        }
    }

    pub(crate) fn wish_attach(&mut self, id: WishlistId, what: v::WishlistAttach) {
        let Some(item) = self.lib.get_wishlist_item(id).ok().flatten() else { return };
        let tracks = self.targets_of(&item).len();
        let loc = self.locale();
        let note = match &what {
            v::WishlistAttach::TorrentFile(_) | v::WishlistAttach::Magnet(_) => note_code(WishNote::OwnTorrent),
            v::WishlistAttach::Files(_) | v::WishlistAttach::Folder(_) => note_code(WishNote::OwnFile),
        };
        let started = match what {
            v::WishlistAttach::Files(paths) => {
                let force = paths.len() == 1 && item.scope == WishlistScope::Track && tracks == 1;
                let targets = self.item_targets(&item, force);
                self.import_for(paths, StoredLine::ImportForWishlist.encode(), SourceOrigin::Inbox, targets)
            }
            v::WishlistAttach::Folder(dir) => {
                let targets = self.item_targets(&item, false);
                self.import_for(vec![dir], StoredLine::ImportForWishlist.encode(), SourceOrigin::Inbox, targets)
            }
            v::WishlistAttach::TorrentFile(path) => match std::fs::read(&path) {
                Ok(bytes) => {
                    self.add_payload_for(TorrentPayload::TorrentFile(bytes), None, None, Some(id));
                    true
                }
                Err(e) => {
                    log::warn!("{}: {e}", path.display());
                    self.toast_err(nikgob_core::i18n::errors_in(loc).could_not_read_torrent, None);
                    false
                }
            },
            v::WishlistAttach::Magnet(m) => {
                let m = m.trim().to_owned();
                if m.starts_with("magnet:") {
                    self.add_payload_for(TorrentPayload::Magnet(m.clone()), None, Some(m), Some(id));
                    true
                } else {
                    self.toast_err(nikgob_core::i18n::errors_in(loc).not_a_magnet, None);
                    false
                }
            }
        };
        if started {
            self.set_item(id, |i| {
                i.status = WishlistStatus::Downloading;
                i.note = Some(note);
            });
            self.publish_wishlist();
        }
    }
}
