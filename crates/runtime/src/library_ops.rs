use super::*;

use nikgob_core::model::{EventType, LikeTarget, PlaylistId};
use nikgob_library::{Sort, TrackSortKey};
use nikgob_view::LIKED_PLAYLIST;

impl Worker {
    // ---------------------------------------------------------------- loads

    /// The library's playlists and the albums liked as a whole among them
    /// ([`nikgob_view::LibraryView::liked_albums`]), both latest first,
    /// «Любимые» staying first; under «только скачанное» the albums with a
    /// file.
    fn shelf(&mut self) -> (Vec<nikgob_view::PlaylistCard>, Vec<(usize, nikgob_view::ReleaseCard)>) {
        let only_local = self.state.read().prefs.library_only_downloaded;
        let mut errors = Vec::new();
        let loc = self.locale();
        let shelf = library_load::shelf(&self.lib, &mut self.res, only_local, loc, &mut errors);
        for what in errors {
            self.toast_err(nikgob_core::i18n::ErrorStrings::error_with_in(loc, &what), None);
        }
        shelf
    }

    /// The library filter on «Треки»: what the library search finds,
    /// without the stream when «только скачанное» is on.
    pub(crate) fn filter_tracks(&mut self, q: &str) {
        let q = q.trim();
        if q.is_empty() {
            self.publish(|s| s.library.track_hits = None);
            return;
        }
        let only_local = self.state.read().prefs.library_only_downloaded;
        let rows: Vec<_> = self
            .log_err("фильтр треков", self.lib.search(q, 500))
            .map(|r| r.tracks)
            .unwrap_or_default()
            .into_iter()
            .filter(|t| t.has_local || !only_local)
            .collect();
        let q = q.to_owned();
        let loc = self.locale();
        let hits = rows.iter().map(|t| self.res.track_in(t, loc)).collect();
        self.publish(|s| {
            s.library.track_hits = Some((q, hits));
        });
    }

    pub(crate) fn load_tracks(&mut self, offset: usize, limit: usize, sort: nikgob_view::TrackSort) {
        let key = match sort {
            nikgob_view::TrackSort::Artist => Sort::asc(TrackSortKey::Artist),
            nikgob_view::TrackSort::Title => Sort::asc(TrackSortKey::Title),
            nikgob_view::TrackSort::Album => Sort::asc(TrackSortKey::Album),
            nikgob_view::TrackSort::Added => Sort::desc(TrackSortKey::Added),
            nikgob_view::TrackSort::Duration => Sort::asc(TrackSortKey::Duration),
        };
        // A library load on its way leaves these rows be.
        self.library_load.tracks_asked += 1;
        let filter = track_filter(self.state.read().prefs.library_only_downloaded);
        let rows = self
            .log_err("треки", self.lib.tracks_filtered(offset as u32, limit as u32, key, filter))
            .unwrap_or_default();
        let loc = self.locale();
        let rows = rows.iter().map(|r| self.res.track_in(r, loc)).collect();
        self.publish(|s| {
            s.library.tracks_offset = offset;
            s.library.tracks = rows;
            s.library.track_sort = sort;
        });
    }

    pub(crate) fn load_release(&mut self, id: nikgob_core::model::ReleaseId) {
        let Some(release) = self.lib.get_release(id).ok().flatten() else {
            return;
        };
        // Its songs outside the library too: the ones a service knows it
        // has and nothing here plays from a file (▶ stream, ↓ download).
        // A line per place the album counts: a hole where none has a row.
        let Some(listing) = self.lib.release_listing(id).ok().flatten() else {
            return;
        };
        let liked = self.lib.release_liked(id).unwrap_or(false);
        let rules = self.album_rules();
        let loc = self.locale();
        let mut detail = self.res.release_detail_in(&release, &listing, liked, rules, loc);
        detail.added_whole = self.lib.release_choice(id).is_ok_and(|(whole, _)| whole);
        (detail.released, detail.label) = self.release_facts(&release, &detail.tracks);
        self.publish(|s| {
            s.release = Some(detail);
        });
        self.complete_tracklist(id);
    }

    /// The day an album came out (beyond its year) and its label: from the
    /// tags of its first file that reads, the day else from a service.
    fn release_facts(
        &self,
        release: &nikgob_core::model::Release,
        tracks: &[nikgob_view::TrackRow],
    ) -> (Option<nikgob_view::ReleaseDay>, Option<String>) {
        let tags = tracks.iter().filter(|t| t.has_local).find_map(|t| {
            let src = self.lib.active_source(t.id).ok().flatten().filter(|s| s.kind.is_local())?;
            nikgob_import::tags::read_tags(&self.res.path(&src.locator)).ok().map(|(tags, _)| tags)
        });
        let label = tags
            .as_ref()
            .and_then(|t| t.label.as_deref())
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned);
        let day = |d: &str| nikgob_view::ReleaseDay::parse(d, release.year);
        let released = tags
            .as_ref()
            .and_then(|t| t.date.as_deref())
            .and_then(day)
            .or_else(|| self.kept_released(release.id).as_deref().and_then(day));
        (released, label)
    }

    pub(crate) fn load_artist(&mut self, id: nikgob_core::model::ArtistId) {
        let Some(artist) = self.lib.get_artist(id).ok().flatten() else {
            return;
        };
        let releases = self.lib.releases_of_artist(id).unwrap_or_default();
        let mut tracks = self.lib.tracks_of_artist(id, 0, 500).unwrap_or_default();
        // "Популярное у тебя": by plays, top 5.
        let mut plays: Vec<(u32, usize)> = tracks
            .iter()
            .enumerate()
            .map(|(i, t)| (self.lib.track_stats(t.id).map(|s| s.plays).unwrap_or(0), i))
            .collect();
        plays.sort_by_key(|p| std::cmp::Reverse(p.0));
        let top: Vec<_> = plays.iter().take(5).map(|&(_, i)| tracks[i].clone()).collect();
        tracks.clear();
        let liked = self.lib.is_liked(id).unwrap_or(false);
        let loc = self.locale();
        let detail = self.res.artist_detail_in(&artist, &top, &releases, liked, loc);
        self.publish(|s| {
            s.artist = Some(detail);
        });
        self.kick_photos(Some(id));
        // "Нет в библиотеке": from the cache, looked up again when it is an
        // hour old.
        self.load_discography(id, false);
    }

    /// What a playlist plays from `start` (see [`Self::playable`]).
    pub(crate) fn playlist_tracks(&mut self, id: PlaylistId, start: Option<TrackId>) -> (Vec<TrackId>, String) {
        if id == LIKED_PLAYLIST {
            if !self.only_downloaded() {
                let ids = self.lib.liked_track_ids().unwrap_or_default();
                return (ids, "Любимые".into());
            }
            let rows = self.lib.liked_tracks(0, u32::MAX).unwrap_or_default();
            return (self.playable(&rows, start), "Любимые".into());
        }
        let entries = self.lib.playlist_entries(id, 0, u32::MAX).unwrap_or_default();
        let name = self
            .lib
            .get_playlist(id)
            .ok()
            .flatten()
            .map_or_else(String::new, |p| p.name);
        let rows: Vec<nikgob_library::TrackRow> = entries.into_iter().map(|e| e.track).collect();
        (self.playable(&rows, start), name)
    }

    pub(crate) fn load_playlist(&mut self, id: PlaylistId) {
        let rows: Vec<nikgob_library::TrackRow> = if id == LIKED_PLAYLIST {
            self.lib.liked_tracks(0, u32::MAX).unwrap_or_default()
        } else {
            self.lib
                .playlist_entries(id, 0, u32::MAX)
                .unwrap_or_default()
                .into_iter()
                .map(|e| e.track)
                .collect()
        };
        let summary = self
            .lib
            .list_playlists()
            .unwrap_or_default()
            .into_iter()
            .find(|p| p.id.unwrap_or(LIKED_PLAYLIST) == id);
        let Some(summary) = summary else { return };
        let sync = self.playlist_sync_view(id);
        let loc = self.locale();
        let mut detail = self.res.playlist_detail_in(&summary, &rows, loc);
        detail.sync = sync;
        self.publish(|s| {
            s.playlist = Some(detail);
        });
    }

    /// What the home and a world's page are built for now.
    pub(crate) fn visit(&self) -> home_v2::Visit {
        let playing = self.queue.current().map(|e| e.track);
        let mut visit = home_v2::Visit::now(playing, &self.state.read().prefs);
        visit.sound = Some(self.analysis.index().clone());
        visit.profiles = self.snapshot().map(|s| s.profiles.clone());
        visit.epoch = self.home_epoch();
        visit
    }
}

impl Worker {
    /// A world's page ([`Command::LoadWorld`]).
    pub(crate) fn load_world(&mut self, key: &str) {
        let visit = self.visit();
        let Some(w) = home_v2::world(&self.lib, &visit, key) else {
            self.publish(|s| s.world = None);
            return;
        };
        let (_, releases) = if visit.local_only { (Vec::new(), Vec::new()) } else { self.discover_rows() };
        let new_album = home_v2::world_release(&w.world, &releases).map(|i| releases[i].clone());
        let loc = self.locale();
        let mut view = self.res.world_view_in(&w, new_album, loc);
        view.mixes = self.world_mix_cards(&view.world);
        let stage = view.world.release;
        self.publish(|s| s.world = Some(view));
        self.kick_hd_covers(stage);
    }

    /// Liking an album saves it; when album likes like the tracks too
    /// ([`nikgob_view::Prefs::album_like_tracks`]), it likes all its tracks,
    /// and unliking unlikes them all.
    pub(crate) fn like_release(&mut self, id: nikgob_core::model::ReleaseId, on: bool) {
        let before = self.library_tracks();
        if !self.state.read().prefs.album_like_tracks {
            self.save_release(id, on, before);
            return;
        }
        let Some(tracks) = self.log_err("лайк", self.lib.like_release(id, on)) else { return };
        for &t in &tracks {
            let _ = self.lib.record_event(
                t,
                if on { EventType::Like } else { EventType::Unlike },
                None,
                None,
                EventContext::Album,
            );
            self.push_like(t, on);
        }
        // An album like downloads the whole album (one wishlist item).
        let wished = on && self.wish_for_album(id, &tracks, nikgob_core::model::WishlistOrigin::Like);
        if !on {
            self.wish_drop_album(id);
            for &t in &tracks {
                self.wish_drop_unstarted(t);
            }
        }
        self.publish_wishlist();
        self.wish_tick();
        let n = tracks.len() as u64;
        let what = format!("{n} {}", if n % 10 == 1 && n % 100 != 11 {
            "трек"
        } else if (2..=4).contains(&(n % 10)) && !(12..=14).contains(&(n % 100)) {
            "трека"
        } else {
            "треков"
        });
        let text = match (on, wished) {
            (true, true) if self.can_download() => format!("Альбом в Любимых · {what} · скачаю в фоне"),
            (true, true) => format!("Альбом в Любимых · {what} · скачается, когда подключишь торренты"),
            (true, false) => format!("Альбом в Любимых · {what}"),
            (false, _) => format!("Альбом убран из Любимых · {what}"),
        };
        let undo = Some(self.undo_action(Command::Like(LikeTarget::Release(id), !on)));
        if on {
            self.toast_ok(text, undo);
        } else {
            self.toast_info(text, undo);
        }
        self.publish_liked_state(&tracks, Some(id), None, on);
        self.patch_home(move |home| release_liked_on_home(home, id, on));
        self.after_like_change(before, vec![id]);
    }

    /// An album saved (or unsaved) alone: its tracks keep their likes; a
    /// saved album still downloads whole, as a like of it always did.
    fn save_release(&mut self, id: nikgob_core::model::ReleaseId, on: bool, before: Held) {
        if self.log_err("лайк", self.lib.like_release(id, on)).is_none() {
            return;
        }
        let wished = if on {
            let rows = self.lib.tracks_of_release_all(id).unwrap_or_default();
            let local: Vec<TrackId> = rows.iter().filter(|r| r.has_local).map(|r| r.id).collect();
            let tracks = if local.is_empty() { rows.iter().map(|r| r.id).collect() } else { local };
            self.wish_for_album(id, &tracks, nikgob_core::model::WishlistOrigin::Like)
        } else {
            self.wish_drop_album(id);
            false
        };
        self.publish_wishlist();
        self.wish_tick();
        let text = match (on, wished) {
            (true, true) if self.can_download() => "В любимых альбомах · скачаю в фоне",
            (true, true) => "В любимых альбомах · скачается, когда подключишь торренты",
            (true, false) => "Добавлено в любимые альбомы",
            (false, _) => "Убрано из любимых альбомов",
        };
        let undo = Some(self.undo_action(Command::Like(LikeTarget::Release(id), !on)));
        if on {
            self.toast_ok(text, undo);
        } else {
            self.toast_info(text, undo);
        }
        self.publish_liked_state(&[], Some(id), None, on);
        self.patch_home(move |home| release_liked_on_home(home, id, on));
        self.after_like_change(before, vec![id]);
    }

    /// A like of a single's song saves (or unsaves) the single as well,
    /// when [`nikgob_view::Prefs::single_like_saves`] asks for it.
    pub(crate) fn single_follows_like(&mut self, track: TrackId, on: bool) -> Option<()> {
        if !self.state.read().prefs.single_like_saves {
            return None;
        }
        let release = self.lib.get_track(track).ok().flatten()?.release_id?;
        let r = self.lib.get_release(release).ok().flatten()?;
        let rows = self.lib.tracks_of_release_all(release).unwrap_or_default();
        let single = r.release_type == nikgob_core::model::ReleaseType::Single || (rows.len() == 1 && r.total_tracks.unwrap_or(1) <= 1);
        if !single || self.lib.is_liked(LikeTarget::Release(release)).unwrap_or(false) == on {
            return None;
        }
        // Saved with its song: no choice of the whole single (its other
        // songs, an instrumental, stay out of the user's songs).
        self.log_err("лайк", self.lib.save_release_with_song(release, on))
    }

    /// Track likes changed: album hearts and "Любимые" follow.
    pub(crate) fn after_like_change(&mut self, before: Held, releases: Vec<nikgob_core::model::ReleaseId>) {
        self.refresh_stream_page();
        self.library_edited(before, releases);
        let (release, playlist) = {
            let s = self.state.read();
            (s.release.as_ref().map(|r| r.card.id), s.playlist.as_ref().map(|p| p.card.id))
        };
        if let Some(r) = release {
            self.load_release(r);
        }
        if playlist == Some(LIKED_PLAYLIST) {
            self.load_playlist(LIKED_PLAYLIST);
        }
    }

    pub(crate) fn release_track_ids(&self, id: nikgob_core::model::ReleaseId) -> Vec<TrackId> {
        self.lib
            .tracks_of_release(id)
            .unwrap_or_default()
            .iter()
            .map(|r| r.id)
            .collect()
    }

    pub(crate) fn set_playlist_cover_mode(&mut self, id: PlaylistId, mode: nikgob_view::PlaylistCoverMode) {
        let stored = match mode {
            nikgob_view::PlaylistCoverMode::Smart => None,
            nikgob_view::PlaylistCoverMode::Collage => Some(playlist_cover::COLLAGE_COVER),
            // A picture comes through `SetPlaylistCover`.
            nikgob_view::PlaylistCoverMode::Custom => return,
        };
        let previous = self.lib.get_playlist(id).ok().flatten().and_then(|p| p.cover);
        if self.log_err("обложка", self.lib.set_playlist_cover(id, stored)).is_some() {
            // The custom picture it leaves: ours to delete only when it is a
            // copy in `.musiclib/covers` (never a file of the user's).
            if let Some(old) = previous.filter(|c| playlist_cover::mode_of(Some(c)) == nikgob_view::PlaylistCoverMode::Custom) {
                let old = self.res.path(&old);
                let covers = self.res.path(".musiclib/covers");
                if old.parent() == Some(covers.as_path()) && old.is_file() {
                    let _ = std::fs::remove_file(&old);
                }
            }
            self.library_edited(Held::Same, Vec::new());
            self.load_playlist(id);
        }
    }

    /// How many tracks the library holds now, to tell whether an edit of
    /// likes or playlists changed what is in it ([`Self::library_edited`]).
    pub(crate) fn library_tracks(&self) -> Held {
        Held::Tracks(self.lib.count_tracks(nikgob_library::TrackFilter::All).ok())
    }

    /// The tracks an album plays as its page is set to open
    /// ([`Self::release_tracks_for`]).
    pub(crate) fn release_tracks(&mut self, id: nikgob_core::model::ReleaseId) -> Vec<TrackId> {
        self.release_tracks_for(id, None, None).0
    }

    /// How albums show and play, as the settings say.
    pub(crate) fn album_rules(&self) -> nikgob_library::AlbumRules {
        let p = &self.state.read().prefs;
        nikgob_library::AlbumRules { mode: p.album_mode, other_versions: p.album_other_versions }
    }

    /// The tracks an album plays, in its order, and whether that is all of
    /// it ([`nikgob_library::ReleaseListing::plays`]): of an album only some
    /// songs of which are the user's, those songs («Мои треки») unless the
    /// whole album is asked for (`scope` `Some(true)`, «Слушать весь
    /// альбом»), the settings open it whole or `start` is a song outside
    /// them; of an album added to the library all of it but the other
    /// versions it does not play. The ones that only stream too, under
    /// «Только скачанное» as well (a song without a file is looked up when
    /// it comes up, and one not found is skipped with a notice).
    pub(crate) fn release_tracks_for(
        &mut self,
        id: nikgob_core::model::ReleaseId,
        scope: Option<bool>,
        start: Option<TrackId>,
    ) -> (Vec<TrackId>, bool) {
        let Some(listing) = self.log_err("альбом", self.lib.release_listing(id)).flatten() else {
            return (Vec::new(), true);
        };
        listing.plays(self.album_rules(), scope, start)
    }

    /// «В библиотеку» / «Убрать из библиотеки» of a song
    /// ([`Command::TrackInLibrary`]), with «Отменить». Nothing downloads:
    /// it is no like.
    /// `on`: «В библиотеку» / «Убрать из библиотеки», with «Отменить» that
    /// puts back exactly what was there (the mark and the song's choice on
    /// its album); `restore`: that «Отменить».
    pub(crate) fn track_in_library(&mut self, track: TrackId, on: Option<bool>, restore: Option<nikgob_library::TrackLibraryWas>) {
        let before = self.library_tracks();
        if let Some(on) = on {
            let Some((now, was)) = self.log_err("библиотека", self.lib.set_track_in_library(track, on)) else { return };
            let text = match (on, now) {
                (true, _) => "Трек в библиотеке",
                (false, false) => "Трек убран из библиотеки",
                // A like or a playlist keeps it.
                (false, true) => "Трек остаётся в библиотеке: он в «Любимых» или в плейлисте",
            };
            let undo = (on || !now)
                .then(|| self.undo_action(Command::TrackLibraryRestore { track, put: was.put, choice: was.choice }));
            if on {
                self.toast_ok(text, undo);
            } else {
                self.toast_info(text, undo);
            }
        } else if let Some(was) = restore
            && self.log_err("библиотека", self.lib.restore_track_in_library(track, was)).is_none()
        {
            return;
        }
        let in_lib = self.lib.track_in_library(track).ok().flatten().unwrap_or(false);
        self.publish_track_in_library(track, in_lib);
        let release = self.lib.get_track(track).ok().flatten().and_then(|t| t.release_id);
        self.library_edited(before, release.into_iter().collect());
        if let Some(r) = release {
            self.album_changed(r);
        }
        self.publish_player();
    }

    /// «Добавить в библиотеку» (the songs left out of it by
    /// hand too; «Отменить» puts back the choice as it was) or back to the
    /// songs chosen one by one; nothing downloads.
    pub(crate) fn add_whole_album(&mut self, id: nikgob_core::model::ReleaseId, on: bool) {
        let before = self.library_tracks();
        if on {
            let Some(undo) = self.log_err("альбом", self.lib.add_release_whole(id)) else { return };
            let back = undo.map(|u| self.undo_action(Command::UndoWholeAlbum { id, mine: u.mine, excluded: u.excluded }));
            self.toast_ok("Альбом в библиотеке", back);
        } else {
            if self.log_err("альбом", self.lib.set_release_whole(id, false)).is_none() {
                return;
            }
            let back = self.undo_action(Command::AddWholeAlbum { id, on: true });
            self.toast_info("В библиотеке только твои треки альбома", Some(back));
        }
        self.library_edited(before, vec![id]);
        self.album_changed(id);
    }

    /// A song of an album into the user's songs of it, or out of them, with
    /// «Отменить» (the choice stored before comes back). In: a choice that
    /// left it out goes, and while it still is not one of them it is added.
    /// Out: a choice that added it goes, and while it still is one of them
    /// it is left out (its file, like and playlists stay).
    pub(crate) fn album_track_mine(&mut self, release: nikgob_core::model::ReleaseId, track: TrackId, mine: bool) {
        let before = self.library_tracks();
        let Some(was) = self.log_err("альбом", self.lib.track_choice(release, track)) else { return };
        let is_mine = |lib: &nikgob_library::Library| {
            lib.release_listing(release).ok().flatten().is_some_and(|l| l.wanted.contains(&track))
        };
        let step = if was == Some(!mine) { None } else { Some(mine) };
        if self.log_err("альбом", self.lib.set_track_choice(release, track, step)).is_none() {
            return;
        }
        if step.is_none() && is_mine(&self.lib) != mine {
            let _ = self.log_err("альбом", self.lib.set_track_choice(release, track, Some(mine)));
        }
        let title = self.lib.get_track(track).ok().flatten().map(|t| t.title).unwrap_or_default();
        let text = if mine { format!("«{title}» в твоих треках альбома") } else { format!("«{title}» убран из твоих треков альбома") };
        let undo = Some(self.undo_action(Command::AlbumTrackChoice { release, track, choice: was }));
        if mine {
            self.toast_ok(text, undo);
        } else {
            self.toast_info(text, undo);
        }
        self.library_edited(before, vec![release]);
        self.album_changed(release);
    }

    /// What the user chose of an album changed: its open page, and Главная
    /// (its shelves count the user's songs).
    pub(crate) fn album_changed(&mut self, id: nikgob_core::model::ReleaseId) {
        self.home_input_changed();
        if self.state.read().release.as_ref().is_some_and(|r| r.card.id == id) {
            self.load_release(id);
        }
    }

    /// Plays an album ([`Command::PlayRelease`]; `scope` `Some(true)`:
    /// [`Command::PlayReleaseWhole`], `Some(false)`:
    /// [`Command::PlayReleaseMine`]).
    pub(crate) fn play_release(&mut self, id: nikgob_core::model::ReleaseId, start: Option<TrackId>, shuffle: bool, scope: Option<bool>) {
        let (ids, whole) = self.release_tracks_for(id, scope, start);
        if ids.is_empty() && !whole {
            // Nothing of it to play (none of its songs the user's): the
            // queue that plays stays, no tracklist joins it.
            self.toast_nothing_to_play();
            return;
        }
        let origin = self
            .lib
            .get_release(id)
            .ok()
            .flatten()
            .map_or(nikgob_view::QueueOrigin::None, |r| nikgob_view::QueueOrigin::Album(r.title));
        self.turn.lap("album read");
        self.play_list(ids, start, shuffle, nikgob_view::ListKind::Album, origin, EventContext::Album, Some(format!("release:{id}")));
        // An album known in part plays at once; the rest of its tracklist
        // joins the queue when the services tell it.
        self.album_queue = Some((id, self.queue.generation(), whole));
        self.complete_tracklist(id);
        self.turn.lap("tracklist");
    }

    /// «Только скачанное» is on, of the home or of the library.
    fn only_downloaded(&self) -> bool {
        let p = &self.state.read().prefs;
        p.mixes_downloaded_only || p.library_only_downloaded
    }

    /// What a list of tracks (a playlist, an artist's songs) plays: all of
    /// it; under «Только скачанное» only its songs with a file and `start`,
    /// the song picked to start from (played even when it only streams). A
    /// list without a single file plays whole. An album plays whole always
    /// ([`Self::release_tracks`]).
    pub(crate) fn playable(&self, rows: &[nikgob_library::TrackRow], start: Option<TrackId>) -> Vec<TrackId> {
        let only_local = self.only_downloaded() && rows.iter().any(|r| r.has_local);
        rows.iter().filter(|r| !only_local || r.has_local || Some(r.id) == start).map(|r| r.id).collect()
    }

    /// The releases of `tracks`, once each.

    pub(crate) fn releases_of(&self, tracks: &[TrackId]) -> Vec<nikgob_core::model::ReleaseId> {
        let mut ids: Vec<_> =
            tracks.iter().filter_map(|t| self.lib.get_track(*t).ok().flatten()?.release_id).collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Likes or playlists were edited. When the library holds the same
    /// tracks as `before` (a local song liked, a playlist renamed…), only
    /// what such an edit changes there is read again and published: the
    /// playlist cards, the cards of `releases` and the page of «Треки»
    /// (their hearts). When it
    /// does not (a stream song liked or added to a playlist joins the
    /// library, one unliked leaves it), or the library was never loaded,
    /// all of it is read again.
    pub(crate) fn library_edited(&mut self, before: Held, releases: Vec<nikgob_core::model::ReleaseId>) {
        self.home_input_changed();
        let same = match before {
            Held::Same => true,
            Held::Tracks(Some(n)) => self.library_tracks() == Held::Tracks(Some(n)),
            Held::Tracks(None) => false,
        };
        // A load on its way would put back what is patched here.
        if !same || !self.state.read().library.loaded || self.library_load_pending() {
            self.load_library();
            return;
        }
        self.stream_analysis_changed();
        self.recs_library_changed();
        let (playlists, liked_albums) = self.shelf();
        let rows: Vec<_> =
            releases.iter().filter_map(|id| self.lib.get_release_row(*id).ok().flatten()).collect();
        let res = &mut self.res;
        let cards: Vec<_> = rows.iter().map(|r| res.release(r)).collect();
        let liked = !cards.is_empty();
        self.publish(|s| {
            s.library.playlists = playlists;
            s.library.liked_albums = liked_albums;
            let l = &mut s.library;
            for list in [&mut l.releases, &mut l.recently_added, &mut l.often_played] {
                for c in list.iter_mut() {
                    if let Some(new) = cards.iter().find(|n| n.id == c.id) {
                        *c = new.clone();
                    }
                }
            }
        });
        // Likes: the hearts of «Треки» too.
        if liked {
            let (offset, sort) = {
                let s = self.state.read();
                (s.library.tracks_offset, s.library.track_sort)
            };
            self.load_tracks(offset, 200, sort);
        }
    }

    /// Updates `liked` flags already shown across player, queue, library, detail pages, and search without reloading everything.
    pub(crate) fn publish_liked_state(
        &mut self,
        tracks: &[TrackId],
        release: Option<nikgob_core::model::ReleaseId>,
        artist: Option<nikgob_core::model::ArtistId>,
        on: bool,
    ) {
        let mut track_ids: Vec<TrackId> = tracks.to_vec();
        for &t in tracks {
            if let Ok(twins) = self.lib.recording_twins(t) {
                track_ids.extend(twins);
            }
        }
        track_ids.sort_unstable();
        track_ids.dedup();
        let stream_in_lib: std::collections::HashMap<TrackId, bool> = if !on && !track_ids.is_empty() {
            track_ids
                .iter()
                .map(|&id| (id, self.lib.track_in_library(id).ok().flatten().unwrap_or(false)))
                .collect()
        } else {
            std::collections::HashMap::new()
        };
        self.publish(|s| {
            let match_track = |id: TrackId| track_ids.binary_search(&id).is_ok();
            let fix = |t: &mut nikgob_view::TrackRow| {
                if match_track(t.id) {
                    t.liked = on;
                }
            };
            let fix_stream = |t: &mut nikgob_view::StreamTrackRow| {
                if let Some(id) = t.track
                    && match_track(id)
                {
                    t.liked = on;
                    t.in_library = if on { true } else { *stream_in_lib.get(&id).unwrap_or(&false) };
                }
            };
            let fix_release = |r: &mut nikgob_view::ReleaseCard| {
                if release == Some(r.id) {
                    r.liked = on;
                }
            };
            let fix_artist = |a: &mut nikgob_view::ArtistCard| {
                if artist == Some(a.id) {
                    a.liked = on;
                }
            };
            if let Some(c) = s.player.current.as_mut() {
                fix(c);
            }
            if let Some(c) = s.queue.current.as_mut() {
                fix(c);
            }
            s.queue.next.iter_mut().for_each(|e| fix(&mut e.track));
            s.queue.prev.iter_mut().for_each(|e| fix(&mut e.track));
            s.library.tracks.iter_mut().for_each(fix);
            if let Some(r) = s.release.as_mut() {
                r.tracks.iter_mut().for_each(fix);
                if release == Some(r.card.id) {
                    r.liked = on;
                }
                fix_release(&mut r.card);
            }
            if let Some(p) = s.playlist.as_mut() {
                p.tracks.iter_mut().for_each(fix);
            }
            if let Some(a) = s.artist.as_mut() {
                a.top_tracks.iter_mut().for_each(fix);
                if artist == Some(a.card.id) {
                    a.liked = on;
                    a.card.liked = on;
                }
            }
            s.library.releases.iter_mut().for_each(fix_release);
            s.library.artists.iter_mut().for_each(fix_artist);
            patch_search_liked(&mut s.search, &fix, &fix_stream, &fix_release, &fix_artist);
        });
    }

    fn publish_track_in_library(&mut self, track: TrackId, in_library: bool) {
        self.publish(|s| {
            let fix = |t: &mut nikgob_view::StreamTrackRow| {
                if t.track == Some(track) {
                    t.in_library = in_library;
                }
            };
            for sec in &mut s.search.streams {
                sec.tracks.iter_mut().for_each(&fix);
            }
            for t in &mut s.search.answer.tracks {
                if let nikgob_view::SearchTrack::Stream(r) = t {
                    fix(r);
                }
            }
            if let Some(nikgob_view::TopResult::Track(nikgob_view::SearchTrack::Stream(r))) = s.search.answer.top.as_mut() {
                fix(r);
            }
        });
    }

    /// Updates `liked` flags already shown without reloading everything.
    pub(crate) fn refresh_liked(&mut self, target: LikeTarget, on: bool, before: Held) {
        let (tracks, release, artist) = match target {
            LikeTarget::Track(t) => (vec![t], None, None),
            LikeTarget::Release(id) => (Vec::new(), Some(id), None),
            LikeTarget::Artist(id) => (Vec::new(), None, Some(id)),
        };
        self.publish_liked_state(&tracks, release, artist, on);
        // The home's cards: kept for a page on its way too.
        match target {
            LikeTarget::Release(id) => self.patch_home(move |home| release_liked_on_home(home, id, on)),
            LikeTarget::Artist(id) => self.patch_home(move |home| {
                home.artist_mixes.iter_mut().filter(|a| a.id == id).for_each(|a| a.liked = on);
            }),
            _ => self.home_input_changed(),
        }
        if let LikeTarget::Track(t) = target {
            let releases = self.releases_of(&[t]);
            self.after_like_change(before, releases);
        } else if self.library_load_pending() {
            // The load on its way may have read the hearts before.
            self.load_library();
        }
    }
}

fn patch_search_liked(
    search: &mut nikgob_view::SearchView,
    fix: &impl Fn(&mut nikgob_view::TrackRow),
    fix_stream: &impl Fn(&mut nikgob_view::StreamTrackRow),
    fix_release: &impl Fn(&mut nikgob_view::ReleaseCard),
    fix_artist: &impl Fn(&mut nikgob_view::ArtistCard),
) {
    search.tracks.iter_mut().for_each(fix);
    search.releases.iter_mut().for_each(fix_release);
    search.artists.iter_mut().for_each(fix_artist);
    for sec in &mut search.streams {
        sec.tracks.iter_mut().for_each(fix_stream);
    }
    for t in &mut search.answer.tracks {
        match t {
            nikgob_view::SearchTrack::Library(r) => fix(r),
            nikgob_view::SearchTrack::Stream(r) => fix_stream(r),
        }
    }
    for a in &mut search.answer.albums {
        if let nikgob_view::SearchAlbum::Library(r) = a {
            fix_release(r);
        }
    }
    for a in &mut search.answer.artists {
        if let nikgob_view::SearchArtist::Library(c) = a {
            fix_artist(c);
        }
    }
    if let Some(top) = search.answer.top.as_mut() {
        match top {
            nikgob_view::TopResult::Track(nikgob_view::SearchTrack::Library(r)) => fix(r),
            nikgob_view::TopResult::Track(nikgob_view::SearchTrack::Stream(r)) => fix_stream(r),
            nikgob_view::TopResult::Album(nikgob_view::SearchAlbum::Library(r)) => fix_release(r),
            nikgob_view::TopResult::Artist(nikgob_view::SearchArtist::Library(c)) => fix_artist(c),
            _ => {}
        }
    }
}

impl Worker {
    /// Copies the picture next to the library data and points the playlist at it.
    pub(crate) fn set_playlist_cover(&mut self, id: PlaylistId, path: PathBuf) {
        const COVERS: &str = ".musiclib/covers";
        let dir = self.res.path(COVERS);
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .filter(|e| !e.is_empty() && e.len() <= 5 && e.chars().all(|c| c.is_ascii_alphanumeric()))
            .unwrap_or_else(|| "jpg".into());
        // A fresh name per cover: the UI caches images by URI, so reusing one
        // file name would keep showing the previous picture.
        let name = format!("playlist-{id}-{}.{ext}", nikgob_library::now_ms());
        let dest = dir.join(&name);
        let copied = std::fs::create_dir_all(&dir).and_then(|()| std::fs::copy(&path, &dest));
        if let Err(e) = copied {
            log::warn!("playlist cover {}: {e}", path.display());
            self.toast_err(nikgob_core::i18n::errors_in(self.locale()).cover_load_failed, None);
            return;
        }
        let previous = self.lib.get_playlist(id).ok().flatten().and_then(|p| p.cover);
        // Relative to the library folder, so the cover survives moving it and
        // the playlist export stays valid on other devices.
        let stored = format!("{COVERS}/{name}");
        if self.log_err("обложка", self.lib.set_playlist_cover(id, Some(&stored))).is_some() {
            if let Some(old) = previous {
                let old = self.res.path(&old);
                let ours = old
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with(&format!("playlist-{id}")));
                if ours && old.parent() == Some(dir.as_path()) && old != dest {
                    let _ = std::fs::remove_file(old);
                }
            }
            self.res.invalidate();
            self.load_library();
            self.load_playlist(id);
        }
    }
}

/// The hearts of album `id` on the home page's cards say `on`.
fn release_liked_on_home(home: &mut nikgob_view::HomeView, id: nikgob_core::model::ReleaseId, on: bool) {
    for r in [&mut home.long_unplayed, &mut home.recently_added] {
        r.iter_mut().filter(|c| c.id == id).for_each(|c| c.liked = on);
    }
    for c in &mut home.continue_listening {
        if let nikgob_view::ContinueCard::Release(c) = c
            && c.id == id
        {
            c.liked = on;
        }
    }
}
