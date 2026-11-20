use super::*;
use super::retry::{offers_of_json, status_rank};

/// How long a stopped download's folder waits after its task ended.
const STOP_GRACE: Duration = Duration::from_secs(2);

impl Worker {

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
                i.note = Some(note_code(WishNote::HasCopy));
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
                i.note = Some(nikgob_core::i18n::status::NOTE_DONE.to_owned());
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
    pub(super) fn album_files(&self, item: &WishlistItem) -> (usize, usize) {
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
    pub(super) fn files_wait(&self, item: WishlistId) -> bool {
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
                i.note = Some(note_code(WishNote::FilesWait));
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
        // A file of this import held another song: taken here once, so a
        // later settle of the item does not look again for it.
        let wrong = self.streaming.wish.wrong.remove(&id);
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
                // Every track of the album has a file: the plain «Скачано»
                // its next settle and [`Self::wish_settle_albums`] expect.
                (have, all) if all > 0 && have == all => (WishlistStatus::Done, note_code(WishNote::Done)),
                // A source gave part of the album: the rest from the next
                // offer (it leaves out what came), or later when a source
                // turned it away.
                _ if partial && self.album_goes_on(id) => return,
                // A part of it is here: how much, and the rest searched for.
                (have, all) if have > 0 => (
                    WishlistStatus::Done,
                    StoredLine::DoneCount { have, all }.encode(),
                ),
                // Nothing of the album landed: still on its way, or left
                // with nothing bringing it (searched again).
                _ if stalled(item.status, item.note.as_deref(), self.moving(id)) => {
                    self.wish_searchable(id);
                    return;
                }
                _ if item.status == WishlistStatus::Confirm => (WishlistStatus::Downloading, String::new()),
                _ => return,
            },
            WishlistScope::Track => {
                let local = item
                    .track_id
                    .and_then(|t| self.lib.active_source(t).ok().flatten())
                    .is_some_and(|s| s.kind.is_local() && s.quality_score > score::SAVED_STREAM);
                if !local {
                    let mut status = item.status;
                    if status == WishlistStatus::Confirm && files_wait_note(item.note.as_deref()) {
                        self.set_item(id, |i| i.status = WishlistStatus::Downloading);
                        self.publish_wishlist();
                        status = WishlistStatus::Downloading;
                    }
                    // A file that held another song's audio (a torrent's
                    // too): the next offer, or a search without that place.
                    if status == WishlistStatus::Downloading && wrong {
                        self.wish_look_again(id);
                        return;
                    }
                    // A direct download that did not become the track's
                    // file (another song, a fake): the next offer.
                    if status == WishlistStatus::Downloading
                        && !self.torrent_of(id)
                        && self.streaming.wish.from.remove(&id).is_some()
                    {
                        self.wish_next_candidate(id);
                    } else if stalled(status, None, self.moving(id)) {
                        // Nothing brings it a file (its files waited before
                        // a restart, say): searched for again.
                        self.wish_searchable(id);
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
                    .and_then(|r| r.codec.map(|c| (c, r.check)))
                    .map(|(codec, check)| QualityWords::Label {
                        codec,
                        bitrate_kbps: None,
                        // A row's short label says the doubt only in the
                        // quality panel.
                        check: check.filter(|c| c.sure()),
                    });
                let note = got.map_or_else(|| note_code(WishNote::Done), |q| StoredLine::DoneQuality { quality: q }.encode());
                (WishlistStatus::Done, note)
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
            w.offer_keys.remove(&id);
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
    pub(super) fn album_goes_on(&mut self, id: WishlistId) -> bool {
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
    pub(super) fn relabel(&mut self, track: TrackId, (artist, title): (String, String)) {
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

    /// The places (offer keys) a file of which held another song for the
    /// item's track: not taken for it again.
    pub(super) fn wrong_places(&self, item: &WishlistItem) -> HashSet<String> {
        let Some(track) = item.track_id.filter(|_| item.scope == WishlistScope::Track) else {
            return HashSet::new();
        };
        self.lib
            .wrong_sources(&[track])
            .unwrap_or_default()
            .into_values()
            .flatten()
            .collect()
    }

    /// A file downloaded for the item held another song's audio (the importer noted the file and its place for the track): the
    /// item says so, and looks again once its import ends.
    pub(crate) fn wish_wrong_audio(&mut self, id: WishlistId, sounds_like: Option<String>) {
        // Only a track looks again at once (a better copy goes to its next
        // offer anyway); an album settles with what came, so its line does
        // not promise a search.
        let looks = self
            .lib
            .get_wishlist_item(id)
            .ok()
            .flatten()
            .is_some_and(|i| i.scope == WishlistScope::Track || i.origin == WishlistOrigin::Upgrade);
        if looks {
            self.streaming.wish.wrong.insert(id);
        }
        let what = self.item_title(id);
        let from = self.streaming.wish.from.get(&id).cloned();
        let text = StoredLine::WrongAudio { from, sounds_like, looks }.encode();
        self.wish_line(id, what, text, None);
    }

    /// The item's file held another song: the next sure offer, else a
    /// search again at once (the place that gave it is left out).
    fn wish_look_again(&mut self, id: WishlistId) {
        self.streaming.wish.from.remove(&id);
        self.streaming.wish.offer_keys.remove(&id);
        if self.streaming.wish.cands.get(&id).is_some_and(|l| l.len() > 1) {
            self.wish_next_candidate(id);
            return;
        }
        self.streaming.wish.cands.remove(&id);
        self.streaming.wish.doubtful.remove(&id);
        self.set_item(id, |i| {
            i.status = WishlistStatus::New;
            i.next_try_at = None;
        });
        self.publish_wishlist();
        self.wish_tick();
    }

    /// The track's file was found holding another song and went to the
    /// Trash («В файле другая песня»): it is looked for again.
    pub(crate) fn wish_again(&mut self, track: TrackId) {
        self.wish_track_now(track);
        self.publish_wishlist();
        self.wish_tick();
    }

    /// The track's file came back (the undo of «В файле другая песня»):
    /// what its own item started for it stops (its torrent goes; a direct
    /// download that ends later is dropped, the item being done), and the
    /// item is done. An album's item goes on for its other tracks.
    pub(crate) fn wish_file_back(&mut self, track: TrackId) {
        for i in self.lib.wishlist_for_track(track).unwrap_or_default() {
            if i.scope != WishlistScope::Track
                || i.origin == WishlistOrigin::Upgrade
                || matches!(i.status, WishlistStatus::Done | WishlistStatus::Dismissed)
            {
                continue;
            }
            let w = &mut self.streaming.wish;
            w.cands.remove(&i.id);
            w.doubtful.remove(&i.id);
            w.probe.remove(&i.id);
            w.from.remove(&i.id);
            w.offer_keys.remove(&i.id);
            w.wrong.remove(&i.id);
            self.cancel_wish_torrent(i.id);
        }
        self.wish_imported(track, Path::new(""));
    }

    /// An import of saved stream audio failed: the item goes back.
    pub(crate) fn wish_import_failed(&mut self, path: &Path) {
        if let Some(item) = self.streaming.wish.saving.remove(path) {
            let _ = std::fs::remove_file(path);
            self.set_item(item, |i| {
                i.status = WishlistStatus::NoTorrent;
                i.note = Some(note_code(WishNote::SaveFailed));
            });
            self.publish_wishlist();
        }
    }

    /// Each second: the sources' told times, and where every running
    /// download is ([`crate::running`]).
    pub(crate) fn wish_poll_direct(&mut self) {
        self.sources_expire();
        self.publish_running();
    }

    /// The item downloads through a torrent of the torrent client.
    fn torrent_of(&self, id: WishlistId) -> bool {
        self.lib.list_downloads().unwrap_or_default().iter().any(|d| d.wishlist_id == Some(id))
    }

    /// The torrent of a turned-down item stops; what it downloaded goes
    /// through the stopped payloads' cleanup ([`Worker::drop_torrent`]),
    /// which keeps the library's masters and waits for the importer.
    pub(super) fn cancel_wish_torrent(&mut self, id: WishlistId) {
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
                w.drop_torrent(&c, &h);
            }
        }));
    }

    /// After a restart: searches that were running start over, and so do
    /// downloads of sources other than torrents (they do not go on), whose
    /// folders go unless files of them wait in «Подтверди».
    ///
    /// What turned the downloads away before goes on ([`backoff`]): the
    /// pauses of the sources, the retries in a row, the day's searches.
    pub(crate) fn wish_restart(&mut self) {
        self.load_backoff();
        // Copies of albums that waited for «Заменить» (their folders stay).
        let albums = self.album_waiting();
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
            let mut waiting: Vec<String> = svc.pending().iter().filter_map(|p| p.wishlist.map(|w| w.to_string())).collect();
            waiting.extend(albums.iter().map(ToString::to_string));
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
            if albums.contains(&i.id) {
                continue;
            }
            if files_wait_note(i.note.as_deref()) {
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
        for id in albums {
            self.album_restage(id);
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
        let loc = self.locale();
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
        let mut empty = Vec::new();
        // Where the items searching again after an empty card are now.
        let mut seen: HashMap<WishlistId, (WishlistStatus, bool)> = HashMap::new();
        let recovering = !self.streaming.wish.unstuck.0.is_empty();
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
            // An item whose files waited there and do no more asks nothing
            // of its own (its import settles it).
            if item.status == WishlistStatus::Confirm && !files_wait && !files_wait_note(item.note.as_deref()) {
                let card = self.wish_confirm_view(item);
                if card.answerable() {
                    if recovering {
                        seen.insert(item.id, (item.status, true));
                    }
                    confirms.push(card);
                } else if !self.album_copy_waits(item.id) {
                    empty.push(item.id);
                }
            }
            if recovering {
                seen.entry(item.id).or_insert((item.status, false));
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
                    nikgob_core::i18n::status::fmt_search_again_days_in(self.locale(), days)
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
                note: item.note.as_deref().map(|n| nikgob_core::i18n::status::line_words_in(loc, n)),
                next_try,
                attempts: item.attempts,
                can_save_from_youtube: youtube
                    && item.track_id.is_some()
                    && item.origin != WishlistOrigin::Upgrade
                    && !matches!(item.status, WishlistStatus::Done | WishlistStatus::Dismissed),
                cover: None,
                transfer: (item.status == WishlistStatus::Downloading)
                    .then(|| self.streaming.wish.transfers.get(&item.id).copied())
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
        if recovering {
            self.streaming.wish.unstuck.seen(&seen);
        }
        for id in empty {
            self.wish_unstick(id);
        }
    }

    /// An item in «Подтверди» with nothing to choose (its offers could not
    /// be read, or none was kept): its card is not shown, and it searches
    /// again; should that very search end the same way, it is not found.
    fn wish_unstick(&mut self, id: WishlistId) {
        let first = self.streaming.wish.unstuck.empty(id);
        log::warn!("wishlist: {id} waited in «Подтверди» with nothing to choose, {}", if first { "searched again" } else { "not found" });
        if first {
            self.streaming.wish.confirm_ids.remove(&id);
            self.wish_searchable(id);
        } else {
            self.no_torrent(id, false);
        }
    }

    /// The item searches again from the start (nothing of its last offers
    /// kept).
    fn wish_searchable(&mut self, id: WishlistId) {
        self.set_item(id, |i| {
            i.status = WishlistStatus::New;
            i.next_try_at = None;
            i.candidates_json = None;
            i.note = None;
        });
        self.publish_wishlist();
        self.wish_tick();
    }

    /// Something still brings `id` a file: its torrent, a download of
    /// another source (or its .torrent being fetched), an import, files
    /// waiting in «Подтверди».
    fn moving(&self, id: WishlistId) -> bool {
        self.torrent_of(id)
            || self.files_wait(id)
            || self.streaming.wish.runs.runs(id)
            || self.streaming.wish.from.contains_key(&id)
            || self.importing_wishes().contains(&id)
    }

    /// `item`'s download stops: its task is aborted, and once it has
    /// really stopped (and a Soulseek client let go of the file) its folder
    /// goes ([`Self::run_stopped`]). True when one ran.
    pub(crate) fn stop_run(&mut self, item: WishlistId) -> bool {
        let Some((token, task)) = self.streaming.wish.runs.stop(item) else { return false };
        self.streaming.wish.live.lock().remove(&item);
        self.streaming.spawn(async move {
            if let Some(t) = task {
                let _ = t.await;
            }
            // The embedded client stops writing a cancelled transfer on its
            // own runtime, a moment later.
            tokio::time::sleep(STOP_GRACE).await;
            Msg::Wish(WishMsg::RunStopped { item, token })
        });
        true
    }

    /// A stopped download's task ended: its folder goes (and its place in
    /// the downloads' folders), unless `item` runs again — a new run took
    /// the folder over.
    pub(super) fn run_stopped(&mut self, item: WishlistId, token: u64) {
        if self.streaming.wish.runs.runs(item) {
            log::debug!("wishlist: {item} runs again, the folder of run {token} stays");
            return;
        }
        self.drop_direct(item);
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
        let loc = self.locale();
        let strings = status_in(loc);
        let what = match (&item.artist, &item.title) {
            (Some(a), Some(t)) => format!("{t} — {a}"),
            (_, Some(t)) => t.clone(),
            _ => nikgob_core::i18n::downloads_in(loc).tab_wishlist.to_owned(),
        };
        v::Confirm {
            yes_no: false,
            id,
            title: what,
            // The question says a track or an album; the tab, what for.
            origin: String::new(),
            options: offers
                .iter()
                .enumerate()
                .map(|(i, o)| v::ConfirmOption {
                    label: o.title.clone(),
                    detail: nikgob_core::i18n::status::fmt_offer_in(loc, o),
                    // Without seeders it may never come.
                    recommended: Some(i) == offers.iter().position(|o| o.seeders != Some(0)),
                })
                .collect(),
            skip_label: nikgob_core::i18n::errors_in(loc).search_later.into(),
            question: match item.scope {
                WishlistScope::Track => strings.card_question_track,
                WishlistScope::Album => strings.card_question_album,
            }
            .into(),
            accept_label: strings.card_accept.into(),
            pick: None,
        }
    }
}
