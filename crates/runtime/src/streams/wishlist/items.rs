use super::*;

/// A track is downloaded when its row has a local file that is not a
/// temporary copy saved from a stream: such a copy does not count as
/// downloaded, and «Хочу скачать» asks for the track again (a real file
/// replaces the copy by itself).
pub(super) fn has_real_file(row: &nikgob_library::TrackRow) -> bool {
    row.has_local && !row.is_temporary()
}

impl Worker {
    pub(super) fn now() -> i64 {
        nikgob_library::now_ms()
    }

    pub(super) fn put_item(&mut self, item: &WishlistItem) {
        let _ = self.log_err(nikgob_core::i18n::LibOp::Wishlist.label_in(self.locale()), self.lib.put_wishlist_item(item));
    }

    pub(super) fn set_item(&mut self, id: WishlistId, f: impl FnOnce(&mut WishlistItem)) -> Option<WishlistItem> {
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

    /// The library has `track`'s song on another album with a file (one
    /// recording: the same ISRC, recording MBID or AcoustID, linked now if
    /// it was not yet): a track item of it needs no download. An album
    /// item still completes its album.
    fn have_recording(&self, track: TrackId) -> bool {
        let linked = crate::same_audio::link_by_ids(&self.lib, Some(track));
        if linked > 0 {
            log::info!("wishlist: {track} is the same recording as a song on another album");
        }
        self.lib.recording_has_file(track).unwrap_or(false)
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
    pub(super) fn item_for(&self, track: Option<TrackId>, release: Option<ReleaseId>) -> Option<WishlistItem> {
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
        if self.have_recording(track) {
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
        // An import brings hundreds at once: counted, not listed.
        if origin != WishlistOrigin::Import {
            log::info!("wishlist: {} ({:?}, {:?})", item.title.as_deref().unwrap_or("?"), item.scope, origin);
        }
        self.put_item(&item);
        true
    }

    /// [`Self::wish_for_like`] for every track of `tracks`: at once for a
    /// few, a slice at a time between the loop's other turns for an import
    /// of hundreds (each item is a write of its own, and all of them in
    /// one turn held the loop for seconds).
    pub(crate) fn wish_for_likes(&mut self, tracks: &[TrackId], origin: WishlistOrigin) {
        if tracks.len() <= WISHES_AT_ONCE && self.wishes_later.is_empty() {
            for &t in tracks {
                self.wish_for_like(t, origin);
            }
            return;
        }
        let idle = self.wishes_later.is_empty();
        self.wishes_later.extend(tracks.iter().map(|&t| (t, origin)));
        if idle {
            let _ = self.internal_tx.send(crate::import::Internal::WishesLater);
        }
    }

    /// One slice of the likes that wait ([`Self::wish_for_likes`]); the
    /// wishlist is shown and searched once they are all in.
    pub(crate) fn wishes_later_step(&mut self) {
        let started = std::time::Instant::now();
        while started.elapsed() < WISHES_SLICE {
            let Some((track, origin)) = self.wishes_later.pop_front() else { break };
            if self.wish_for_like(track, origin) {
                self.wishes_made += 1;
            }
        }
        if self.wishes_later.is_empty() {
            log::info!("wishlist: {} items from the imported likes", std::mem::take(&mut self.wishes_made));
            self.publish_wishlist();
            self.wish_tick();
        } else {
            let _ = self.internal_tx.send(crate::import::Internal::WishesLater);
        }
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
    /// track item is made for it. An item of its whole album on its way
    /// brings it already; one that waits is not woken for it (it would
    /// fetch the whole album, more than the one song asked for).
    pub(super) fn wish_track_now(&mut self, track: TrackId) -> Wished {
        let has_file = self.lib.get_track_row(track).ok().flatten().is_some_and(|r| has_real_file(&r));
        if has_file || self.have_recording(track) {
            return Wished::Nothing;
        }
        let existing = self.lib.wishlist_for_track(track).unwrap_or_default();
        if let Some(i) = existing.iter().find(|i| i.status != WishlistStatus::Done) {
            return self.start_again(i);
        }
        let release = self.lib.get_track(track).ok().flatten().and_then(|t| t.release_id);
        if let Some(r) = release
            && self.lib.wishlist_for_release(r).unwrap_or_default().iter().any(|i| {
                i.scope == WishlistScope::Album
                    && matches!(
                        i.status,
                        WishlistStatus::New | WishlistStatus::Searching | WishlistStatus::Confirm | WishlistStatus::Downloading
                    )
            })
        {
            return Wished::Already;
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
    pub(super) fn wish_release_now(&mut self, release: ReleaseId) -> Wished {
        let p = self.lib.release_progress(release).ok().flatten();
        let tracks = self.lib.tracks_of_release_all(release).unwrap_or_default();
        let missing = p.is_none_or(|p| p.local < p.total) || tracks.iter().any(|t| !has_real_file(t));
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
    pub(super) fn download_toast(&mut self, started: usize, already: usize, what: Option<String>) {
        let wishlist = v::Link::Route(v::Route::Downloads(v::DownloadsTab::Wishlist));
        let loc = self.locale();
        let strings = nikgob_core::i18n::errors_in(loc);
        let open = nikgob_core::i18n::common_in(loc).open;
        if started == 0 {
            if already > 0 {
                self.toast_open(v::ToastKind::Info, strings.already_in_wishlist, open, wishlist);
            } else {
                self.toast_info(strings.everything_downloaded, None);
            }
            return;
        }
        let text = match (started, what) {
            (1, Some(w)) => nikgob_core::i18n::ErrorStrings::fmt_wishlist_added_one_in(loc, &w),
            (n, _) => {
                let tracks = nikgob_core::i18n::plural_forms(
                    loc,
                    n as u64,
                    &nikgob_core::i18n::common_in(loc).tracks,
                );
                nikgob_core::i18n::ErrorStrings::fmt_wishlist_added_in(loc, n as u64, tracks)
            }
        };
        if self.can_download() {
            self.toast_open(v::ToastKind::Success, text, open, wishlist);
        } else {
            self.toast_open(
                v::ToastKind::Success,
                nikgob_core::i18n::ErrorStrings::fmt_downloads_with_source_in(loc, &text),
                nikgob_core::i18n::downloads_in(loc).configure,
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
        for r in remotes.iter().filter(|r| !self.streaming.known.contains_key(r)).cloned().collect::<Vec<_>>() {
            self.api_skip(format!("{r} is not known: find it again"));
        }
        if known.is_empty() {
            self.toast_err(nikgob_core::i18n::errors_in(self.locale()).stream_track_stale, None);
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

    /// `Command::DownloadRelease`, «Скачать недостающие»: of an album only
    /// some songs of which are the user's, their songs without a file, a
    /// track item each (never the album: the songs the user did not choose
    /// stay where they are); of any other album its missing files
    /// ([`Self::download_whole_release`]).
    pub(crate) fn download_release(&mut self, release: ReleaseId) {
        let listing = self.lib.release_listing(release).ok().flatten();
        let rules = self.album_rules();
        if let Some((l, plan)) = listing.as_ref().map(|l| (l, l.plan(rules))).filter(|(_, plan)| plan.split) {
            let missing: Vec<TrackId> = l.tracks().filter(|t| !t.has_local && plan.mine.contains(&t.id)).map(|t| t.id).collect();
            self.download_tracks(&missing);
            return;
        }
        self.download_whole_release(release);
    }

    /// `Command::DownloadWholeRelease` («Скачать весь альбом») and the album
    /// of a service once fetched (`Command::DownloadStreamAlbum`): one album
    /// item. An album only some songs of which were the user's is added
    /// whole to the library first: its files will be.
    pub(crate) fn download_whole_release(&mut self, release: ReleaseId) {
        if self.lib.release_progress(release).ok().flatten().is_some_and(|p| p.partial())
            && self.lib.add_release_whole(release).is_ok()
        {
            self.load_library();
            self.album_changed(release);
        }
        let title = self.lib.get_release(release).ok().flatten().map(|r| {
            let strings = nikgob_core::i18n::downloads_in(self.locale());
            match self.locale() {
                Locale::Ru => format!("{} «{}»", strings.album, r.title),
                Locale::En => format!("{} “{}”", strings.album, r.title),
            }
        });
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
                if status == i.status && (status != WishlistStatus::Done || i.note.as_deref() == Some(nikgob_core::i18n::status::NOTE_DONE)) {
                    continue;
                }
                self.streaming.wish.cands.remove(&i.id);
                self.streaming.wish.later.remove(&i.id);
                self.wish_line_drop(i.id);
                self.set_item(i.id, |i| {
                    i.status = status;
                    i.next_try_at = None;
                    if status == WishlistStatus::Done {
                        i.note = Some(note_code(WishNote::Done));
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
    pub(super) fn targets_of(&self, item: &WishlistItem) -> Vec<TrackId> {
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

    pub(super) fn item_targets(&self, item: &WishlistItem, force: bool) -> ImportTargets {
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
            from: self.streaming.wish.offer_keys.get(&item.id).cloned(),
            pinned: Vec::new(),
        }
    }
}
