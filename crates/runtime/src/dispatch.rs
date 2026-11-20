use super::*;

use nikgob_core::model::{EndReason, EventMeta, EventOrigin, EventType, LikeTarget};
use nikgob_view::LIKED_PLAYLIST;

/// The toast of «Добавить в плейлист»: what was added and what the
/// playlist had already.
fn added_to_playlist(name: &str, added: usize, already: usize) -> String {
    match (added, already) {
        (0, 0) => format!("Добавлено в «{name}»"),
        (0, _) => format!("Уже есть в «{name}»"),
        (_, 0) => format!("Добавлено в «{name}»"),
        (_, 1) => format!("Добавлено {added}, 1 уже был в «{name}»"),
        _ => format!("Добавлено {added}, {already} уже были в «{name}»"),
    }
}

impl Worker {
    // ---------------------------------------------------------------- commands

    /// [`Self::command_now`]; what it adds to the queue plays as whoever
    /// added it when that is not whose the queue is (a guest's song in the
    /// owner's queue is the guest's, the owner's in an assistant's queue the
    /// owner's).
    pub(crate) fn command(&mut self, c: Command) {
        if !adds_to_queue(&c) {
            return self.command_now(c);
        }
        let before: std::collections::HashSet<u64> = self.queue.upcoming().iter().map(|e| e.id).collect();
        self.command_now(c);
        let by = self.api_actor();
        if by != self.queue_actor {
            let added: Vec<u64> = self.queue.upcoming().iter().map(|e| e.id).filter(|id| !before.contains(id)).collect();
            for id in added {
                self.entry_actor.insert(id, by);
            }
        }
    }

    fn command_now(&mut self, c: Command) {
        for i in 0..self.services.len() {
            let mut handled = false;
            let mut st = self.state.write();
            if self.services[i].handle(&c, &mut st) {
                handled = true;
            }
            drop(st);
            if handled {
                self.revision += 1;
                self.state.write().revision = self.revision;
                (self.wake)();
                return;
            }
        }
        // The user edits the queue: an entry waiting out a service's pause
        // no longer starts by itself.
        if edits_queue(&c) {
            self.cancel_pause_wake();
        }
        match c {
            Command::TogglePlay => match self.status {
                Status::Playing | Status::Buffering => self.pause(),
                Status::Paused => self.resume(),
                _ => {
                    if let Some(e) = self.queue.current() {
                        self.start(e);
                    }
                }
            },
            Command::Play => self.resume(),
            Command::Pause => self.pause(),
            Command::Next => {
                self.record_end(true, EndReason::UserNext);
                self.turn.lap("end recorded");
                match self.queue.skip_forward() {
                    Some(e) => self.start(e),
                    // The radio replaced its tail: its picks play when
                    // they come.
                    None if self.radio_holds() => self.wait_for_radio(),
                    None => self.stop(),
                }
            }
            Command::Previous => {
                if self.position_now() > Duration::from_secs(3) || self.queue.current().is_none() {
                    self.seek(Duration::ZERO);
                } else if let Some(e) = self.queue.previous() {
                    self.user_pick(EventOrigin::Manual, e.id);
                    self.start(e);
                }
            }
            Command::Seek(p) => self.seek(p),
            Command::SleepTimer(choice) => self.sleep_set(choice),
            // The app opens and resets the developer profile itself.
            Command::DevProfile(_) => {}
            Command::PickLyricsSource(t, src) => self.pick_lyrics_source(t, src),
            Command::SearchLyricsAgain(t) => self.search_lyrics_again(t),
            Command::CraftLyrics { track, source, target, deep } => self.craft_lyrics(track, source, target, deep),
            Command::ClearCraft => self.clear_craft(),
            Command::CancelCraft => self.cancel_craft(),
            Command::ShowLyricsAs(t, sync) => self.show_lyrics_as(t, sync),
            Command::SaveLyrics { track, source } => self.save_lyrics(track, source),
            Command::SaveOwnLyrics { track, lines, timed } => self.save_own_lyrics(track, lines, timed),
            Command::DropOwnLyrics(track) => self.drop_own_lyrics(track),
            Command::RestoreOwnLyrics(track) => self.restore_own_lyrics(track),
            Command::AlignModel { key, download } => self.align_model(&key, download),
            Command::SeekBy(ms) => {
                // Seconds as the listener counts them: more of the track
                // when it plays faster.
                let ms = (ms as f64 * self.speed).round() as i64;
                let p = self.position_now().as_millis() as i64 + ms;
                self.seek(Duration::from_millis(p.max(0) as u64));
            }
            Command::SetTrackSpeed { track, variant } => self.set_track_speed(track, variant),
            Command::ResetTrackSpeed(track) => self.reset_track_speed(track),
            Command::SetVolume(v) => {
                let v = v.clamp(0.0, 1.0);
                if v == self.volume && !self.muted {
                    return;
                }
                self.heard_tick();
                self.volume = v;
                self.muted = false;
                self.playback.set_volume(self.volume);
                self.publish_volume();
            }
            Command::ToggleMute => {
                self.heard_tick();
                self.muted = !self.muted;
                self.playback
                    .set_volume(if self.muted { 0.0 } else { self.volume });
                self.publish_volume();
            }
            Command::ToggleShuffle => {
                let on = !self.queue.shuffle;
                // The button changes the queue only: the rules of the
                // kinds of lists are the settings' («Перемешивание»).
                self.queue.set_shuffle(on);
                self.preload_next();
                self.publish_player();
            }
            Command::CycleRepeat => {
                self.queue.repeat = self.queue.repeat.next();
                self.preload_next();
                self.publish_player();
            }
            Command::PlayRelease { id, start, shuffle } => self.play_release(id, start, shuffle, None),
            Command::PlayReleaseWhole { id, start, shuffle } => self.play_release(id, start, shuffle, Some(true)),
            Command::PlayReleaseMine { id, start, shuffle } => self.play_release(id, start, shuffle, Some(false)),
            Command::PlayPlaylist { id, start, shuffle } => {
                let (ids, name) = self.playlist_tracks(id, start);
                let origin = if id == LIKED_PLAYLIST {
                    nikgob_view::QueueOrigin::Liked
                } else {
                    nikgob_view::QueueOrigin::Playlist(name)
                };
                let kind = self.playlist_kind(id);
                let n = ids.len();
                self.play_list(ids, start, shuffle, kind, origin, EventContext::Playlist, Some(format!("playlist:{id}")));
                // Its entries are its own («✦ С дополнением» counts them).
                if n > 0 {
                    self.plus_queued(id);
                }
            }
            Command::PlayArtist { id, shuffle } => {
                let rows = self
                    .log_err(nikgob_core::i18n::LibOp::Artist.label_in(self.locale()), self.lib.tracks_of_artist(id, 0, 500))
                    .unwrap_or_default();
                let origin = self
                    .lib
                    .get_artist(id)
                    .ok()
                    .flatten()
                    .map_or(nikgob_view::QueueOrigin::None, |a| nikgob_view::QueueOrigin::Artist(a.name));
                let ids = self.playable(&rows, None);
                // Without «Перемешать» it is the artist's «Слушать микс».
                self.play_list(ids, None, shuffle, nikgob_view::ListKind::Mix, origin, EventContext::Artist, Some(format!("artist:{id}")));
            }
            Command::PlayTracks {
                tracks,
                start,
                context,
            } => self.play_tracks(&tracks, Some(start), context),
            Command::PlayAll { tracks, context } => self.play_tracks(&tracks, None, context),
            Command::PlayNext(t) => {
                self.queue.play_next(&t);
                self.preload_next();
                self.publish_player();
                let msg = nikgob_core::i18n::player_in(self.locale()).queued_next;
                self.toast_ok(msg, None);
            }
            Command::Enqueue(t) => {
                self.queue.enqueue(&t);
                self.preload_next();
                self.publish_player();
                let msg = nikgob_core::i18n::player_in(self.locale()).added_to_queue;
                self.toast_ok(msg, None);
            }
            Command::PlayReleaseNext(id) => {
                let ids = self.release_tracks(id);
                if ids.is_empty() {
                    return;
                }
                self.queue.play_next(&ids);
                self.preload_next();
                self.publish_player();
                let title = self.lib.get_release(id).ok().flatten().map(|r| r.title).unwrap_or_default();
                let msg = nikgob_core::i18n::PlayerStrings::next_up_quoted_in(self.locale(), &title);
                self.toast_ok(msg, None);
            }
            Command::EnqueueRelease(id) => {
                let ids = self.release_tracks(id);
                self.command(Command::Enqueue(ids));
            }
            Command::AddAlbumToPlaylist(playlist, id) => {
                let ids = self.release_tracks(id);
                if ids.is_empty() {
                    return;
                }
                self.command(Command::AddToPlaylist(playlist, ids));
            }
            Command::PlayPlaylistNext(id) => {
                let (ids, name) = self.playlist_tracks(id, None);
                if ids.is_empty() {
                    return;
                }
                self.queue.play_next(&ids);
                self.preload_next();
                self.publish_player();
                let msg = nikgob_core::i18n::PlayerStrings::next_up_quoted_in(self.locale(), &name);
                self.toast_ok(msg, None);
            }
            Command::EnqueuePlaylist(id) => {
                let (ids, _) = self.playlist_tracks(id, None);
                if ids.is_empty() {
                    return;
                }
                self.command(Command::Enqueue(ids));
            }
            Command::PlayArtistNext(id) => {
                let rows = self
                    .log_err(nikgob_core::i18n::LibOp::Artist.label_in(self.locale()), self.lib.tracks_of_artist(id, 0, 2000))
                    .unwrap_or_default();
                let ids = self.playable(&rows, None);
                if ids.is_empty() {
                    return;
                }
                self.queue.play_next(&ids);
                self.preload_next();
                self.publish_player();
                let name = self.lib.get_artist(id).ok().flatten().map(|a| a.name).unwrap_or_default();
                let loc = self.locale();
                let msg = if name.is_empty() {
                    nikgob_core::i18n::player_in(loc).queued_next.to_owned()
                } else {
                    nikgob_core::i18n::PlayerStrings::next_up_in(loc, &name)
                };
                self.toast_ok(msg, None);
            }
            Command::EnqueueArtist(id) => {
                let rows = self
                    .log_err(nikgob_core::i18n::LibOp::Artist.label_in(self.locale()), self.lib.tracks_of_artist(id, 0, 2000))
                    .unwrap_or_default();
                let ids = self.playable(&rows, None);
                self.command(Command::Enqueue(ids));
            }
            Command::PlayQueueEntry(entry) => {
                if let Some(e) = self.queue.jump(entry) {
                    self.user_pick(EventOrigin::Manual, e.id);
                    self.start(e);
                }
            }
            Command::RemoveFromQueue(entry) => {
                let track = self.queue.upcoming().iter().find(|e| e.id == entry).map(|e| e.track);
                self.sleep_removing(entry);
                self.queue.remove(entry);
                if let Some(t) = track {
                    self.radio_removed(entry, t);
                }
                self.preload_next();
                self.publish_player();
            }
            Command::MoveInQueue { entry, anchor, after } => {
                self.queue.move_next_to(entry, anchor, after);
                self.plus_moved(entry);
                self.preload_next();
                self.publish_player();
            }
            Command::ClearQueue => {
                let before: Vec<TrackId> = self.queue.history().iter().map(|e| e.track).collect();
                let after: Vec<TrackId> = self.queue.upcoming().iter().map(|e| e.track).collect();
                let playing = self.queue.current().map(|e| e.id);
                self.plus_cleared();
                self.queue.clear_upcoming();
                self.preload_next();
                self.publish_player();
                if !before.is_empty() || !after.is_empty() {
                    self.cleared_queue = Some((playing, before, after));
                    let msg = nikgob_core::i18n::player_in(self.locale()).queue_cleared;
                    let undo = self.undo_action(Command::UndoClearQueue);
                    self.toast_info(msg, Some(undo));
                }
            }
            Command::UndoClearQueue => {
                // Only onto the queue it was taken from: another list since
                // then is not this one's to fill.
                if let Some((playing, before, after)) = self.cleared_queue.take()
                    && self.queue.current().map(|e| e.id) == playing
                {
                    self.queue.restore_around(&before, &after);
                    self.plus_uncleared();
                    self.preload_next();
                    self.publish_player();
                }
            }
            Command::SaveTracksAsPlaylist { name, tracks } => {
                let before = self.library_tracks();
                if let Some(p) = self.log_err(nikgob_core::i18n::LibOp::Playlist.label_in(self.locale()), self.lib.create_playlist(&name)) {
                    let _ = self.log_err(nikgob_core::i18n::LibOp::Playlist.label_in(self.locale()), self.lib.playlist_add_many(p.id, &tracks));
                    self.toast_ok(format!("Плейлист «{name}» сохранён"), None);
                    self.library_edited(before, Vec::new());
                }
            }
            Command::HideHomeCard => {
                self.publish(|s| s.prefs.home_card = false);
                self.save_prefs();
                self.load_home();
                self.push_toast(
                    nikgob_view::ToastKind::Info,
                    "Карточка скрыта. Включить её снова можно в Настройках → Внешний вид → Главная.".into(),
                    Some(("Вернуть".into(), Command::SetPref(nikgob_view::PrefPatch::HomeCard(true)))),
                    None,
                    true,
                );
            }
            Command::SaveQueueAsPlaylist(name) => {
                let tracks = self.queue.tracks();
                let before = self.library_tracks();
                if let Some(p) = self.log_err(nikgob_core::i18n::LibOp::Playlist.label_in(self.locale()), self.lib.create_playlist(&name)) {
                    let _ = self.log_err(nikgob_core::i18n::LibOp::Playlist.label_in(self.locale()), self.lib.playlist_add_many(p.id, &tracks));
                    self.toast_ok(format!("Плейлист «{name}» сохранён"), None);
                    self.library_edited(before, Vec::new());
                }
            }
            // A guest's like would fetch files and write to the owner's
            // services: the API never lets one through, and it stops here too.
            Command::Like(..) if self.api.actor == Some(nikgob_core::settings::ApiActor::Guest) => {
                log::info!("api: a guest's like is not kept");
            }
            Command::Like(LikeTarget::Release(id), on) => self.like_release(id, on),
            Command::Like(target, on) => {
                let before = self.library_tracks();
                let r = if on {
                    self.lib.like(target)
                } else {
                    self.lib.unlike(target)
                };
                if self.log_err(nikgob_core::i18n::LibOp::Like.label_in(self.locale()), r).is_some() {
                    let mut wished = false;
                    if let LikeTarget::Track(t) = target {
                        if on {
                            self.plus_kept(t);
                            self.liked_undislikes(t);
                            self.new_liked(t);
                            self.recs_liked(t);
                        }
                        // The like's actor: the voice assistant's «нравится»
                        // is its own in the history, as its plays are.
                        let actor = self.api_actor();
                        let _ = self.lib.record_event_with(
                            t,
                            if on { EventType::Like } else { EventType::Unlike },
                            None,
                            None,
                            EventContext::Other,
                            EventMeta { actor, ..EventMeta::default() },
                        );
                        wished = self.track_liked(t, on);
                    }
                    // A like that downloads says so (a like of a track
                    // without a file puts it into «Хочу скачать»).
                    let text = match (on, wished) {
                        (true, true) if self.can_download() => "В Любимых · скачаю в фоне",
                        (true, true) => "В Любимых · скачается, когда подключишь торренты",
                        (true, false) => "Добавлено в Любимые",
                        (false, _) => "Убрано из Любимых",
                    };
                    let undo = Some(self.undo_action(Command::Like(target, !on)));
                    if on {
                        self.toast_ok(text, undo);
                    } else {
                        self.toast_info(text, undo);
                    }
                    // Its album's cards are read again below.
                    if let LikeTarget::Track(t) = target {
                        let _ = self.single_follows_like(t, on);
                    }
                    self.refresh_liked(target, on, before);
                }
            }
            Command::CreatePlaylist(name) => {
                if self.log_err(nikgob_core::i18n::LibOp::Playlist.label_in(self.locale()), self.lib.create_playlist(&name)).is_some() {
                    self.library_edited(Held::Same, Vec::new());
                }
            }
            Command::RenamePlaylist(id, name) => {
                if self.log_err(nikgob_core::i18n::LibOp::Playlist.label_in(self.locale()), self.lib.rename_playlist(id, &name)).is_some() {
                    self.library_edited(Held::Same, Vec::new());
                    self.load_playlist(id);
                }
            }
            Command::DeletePlaylist(id) => {
                let before = self.library_tracks();
                if self.log_err(nikgob_core::i18n::LibOp::Playlist.label_in(self.locale()), self.lib.delete_playlist(id)).is_some() {
                    self.toast_ok("Плейлист удалён", None);
                    self.library_edited(before, Vec::new());
                }
            }
            Command::AddToPlaylist(id, tracks) => {
                for t in &tracks {
                    self.plus_kept(*t);
                }
                let before = self.library_tracks();
                // Only what it does not have yet: never a track twice.
                let (mut added, mut already) = (0, 0);
                if id == LIKED_PLAYLIST {
                    let mut seen = std::collections::HashSet::new();
                    let mut liked_ids = Vec::new();
                    for t in tracks.iter().filter(|t| seen.insert(**t)) {
                        if self.lib.is_liked(*t).unwrap_or(false) {
                            already += 1;
                        } else if self.lib.like(*t).is_ok() {
                            added += 1;
                            self.track_liked(*t, true);
                            liked_ids.push(*t);
                        }
                    }
                    if !liked_ids.is_empty() {
                        self.publish_liked_state(&liked_ids, None, None, true);
                    }
                } else if let Some(n) = self.log_err(nikgob_core::i18n::LibOp::Playlist.label_in(self.locale()), self.lib.playlist_add_missing(id, &tracks)) {
                    (added, already) = n;
                }
                let name = if id == LIKED_PLAYLIST {
                    "Любимые".to_owned()
                } else {
                    self.lib
                        .get_playlist(id)
                        .ok()
                        .flatten()
                        .map_or_else(String::new, |p| p.name)
                };
                let msg = added_to_playlist(&name, added, already);
                if added == 0 && already > 0 {
                    self.toast_info(msg, None);
                } else {
                    self.toast_ok(msg, None);
                }
                let releases = if id == LIKED_PLAYLIST { self.releases_of(&tracks) } else { Vec::new() };
                self.library_edited(before, releases);
                self.playlist_edited(id);
            }
            Command::RemoveFromPlaylist(id, track) => {
                let before = self.library_tracks();
                // The way back, in its toast.
                let mut undo = None;
                if id == LIKED_PLAYLIST {
                    if self.log_err(nikgob_core::i18n::LibOp::Like.label_in(self.locale()), self.lib.unlike(track)).is_some() {
                        self.publish_liked_state(&[track], None, None, false);
                        undo = Some(("Убрано из Любимых".to_owned(), Command::Like(LikeTarget::Track(track), true)));
                    }
                } else if let Ok(entries) = self.lib.playlist_entries(id, 0, u32::MAX)
                    && let Some(at) = entries.iter().position(|e| e.track.id == track)
                    && self.log_err(nikgob_core::i18n::LibOp::Playlist.label_in(self.locale()), self.lib.playlist_remove(entries[at].entry.id)).is_some()
                {
                    let name = self.lib.get_playlist(id).ok().flatten().map_or_else(String::new, |p| p.name);
                    undo = Some((format!("Убрано из «{name}»"), Command::RestoreToPlaylist { playlist: id, track, at }));
                }
                self.load_playlist(id);
                let releases = if id == LIKED_PLAYLIST { self.releases_of(&[track]) } else { Vec::new() };
                self.library_edited(before, releases);
                self.playlist_edited(id);
                if let Some((text, cmd)) = undo {
                    let undo = self.undo_action(cmd);
                    self.toast_info(text, Some(undo));
                }
            }
            Command::RestoreToPlaylist { playlist, track, at } => {
                let before = self.library_tracks();
                if self.log_err(nikgob_core::i18n::LibOp::Playlist.label_in(self.locale()), self.lib.playlist_insert(playlist, track, at)).is_some() {
                    self.load_playlist(playlist);
                    self.library_edited(before, Vec::new());
                    self.playlist_edited(playlist);
                }
            }
            Command::MoveInPlaylist {
                playlist,
                track,
                to,
            } => {
                if let Ok(entries) = self.lib.playlist_entries(playlist, 0, u32::MAX)
                    && let Some(e) = entries.iter().find(|e| e.track.id == track)
                {
                    let _ = self.log_err(nikgob_core::i18n::LibOp::Playlist.label_in(self.locale()), self.lib.playlist_move(e.entry.id, to));
                }
                self.load_playlist(playlist);
            }
            Command::TrashTrack(t) => self.delete_tracks(vec![t], "Трек"),
            Command::UndoTrash(t) => self.restore_tracks(vec![t]),
            Command::TrashRelease(id) => {
                let ids = self.release_track_ids(id);
                self.delete_tracks(ids, "Альбом");
            }
            Command::AddWholeAlbum { id, on } => self.add_whole_album(id, on),
            Command::UndoWholeAlbum { id, mine, excluded } => {
                let before = self.library_tracks();
                let undo = nikgob_library::WholeUndo { mine, excluded };
                if self.log_err(nikgob_core::i18n::LibOp::Album.label_in(self.locale()), self.lib.undo_release_whole(id, &undo)).is_some() {
                    self.library_edited(before, vec![id]);
                    self.album_changed(id);
                }
            }
            Command::AlbumVersions { id, on } => {
                // On is kept for the album (karaoke plays only so); off
                // only against the setting.
                let versions = self.album_rules().versions_choice(on);
                if self.log_err(nikgob_core::i18n::LibOp::Album.label_in(self.locale()), self.lib.set_release_versions(id, versions)).is_some() {
                    self.album_changed(id);
                }
            }
            Command::TrackInLibrary { track, on } => self.track_in_library(track, Some(on), None),
            Command::TrackLibraryRestore { track, put, choice } => {
                self.track_in_library(track, None, Some(nikgob_library::TrackLibraryWas { put, choice }));
            }
            Command::AlbumTrackMine { release, track, mine } => self.album_track_mine(release, track, mine),
            Command::AlbumTrackChoice { release, track, choice } => {
                let before = self.library_tracks();
                if self.log_err(nikgob_core::i18n::LibOp::Album.label_in(self.locale()), self.lib.set_track_choice(release, track, choice)).is_some() {
                    self.library_edited(before, vec![release]);
                    self.album_changed(release);
                }
            }
            Command::RestoreTracks(ids) => self.restore_tracks(ids),
            Command::DeleteForever(ids) => self.purge_tracks(ids),
            Command::DeleteReleaseForever(id) => {
                let ids = self.release_track_ids(id);
                self.purge_tracks(ids);
            }
            Command::EmptyTrash => self.empty_trash(),
            Command::LoadTrash => self.load_trash(),
            Command::LoadDownloaded => self.load_downloaded(0, import::DOWNLOADED_PAGE),
            Command::LoadMoreDownloaded => self.load_downloaded(self.downloaded_shown(), import::DOWNLOADED_PAGE),
            Command::SetPlaylistCoverMode(id, mode) => self.set_playlist_cover_mode(id, mode),
            Command::ImportFiles(paths) => self.import_paths(paths),
            Command::ImportFolder(dir) => self.import_paths(vec![dir]),
            Command::MergeEditions(id) => self.merge_editions(id),
            Command::KeepEditionsApart(id) => self.keep_editions_apart(id),
            Command::ConfirmChoice { id, option } if id & api::API_CONFIRM_ID != 0 => self.api_confirm(id, option),
            Command::ConfirmChoice { id, option } if id & replica::REPLICA_ID != 0 => self.replica_choice(id, option),
            Command::ConfirmChoice { id, option } if id & streams::WISH_ID != 0 => {
                self.wish_confirm(id, option);
            }
            Command::ConfirmChoice { id, option } if id & IMPORT_ID != 0 => {
                self.confirm_choice(id, option);
            }
            Command::ConfirmChoice { id, .. } if id & quality::QUALITY_ID != 0 => {
                // A fakes card is answered song by song (`ConfirmPicked`),
                // never as a whole.
                log::warn!("quality: a whole fakes card answered, ignored");
            }
            Command::ConfirmPicked { id, sources, accept } if id & quality::QUALITY_ID != 0 => {
                self.quality_picked(sources, accept);
            }
            Command::ConfirmChoice { id, option } if id & same_audio::SAME_AUDIO_ID != 0 => {
                self.same_audio_choice(id, option);
            }
            Command::ConfirmPicked { .. } => {
                log::warn!("picked songs of an unknown card");
            }
            Command::WrongFile(track) => self.wrong_file(track, None),
            Command::UndoWrongFile(track) => self.undo_wrong_file(track),
            Command::FindBetter(track) => self.find_better(track),
            Command::TrackInfo(track) => self.track_info(track),
            Command::FindBetterAlbum(release) => self.find_better_album(release),
            Command::UndoBetterAlbum(release) => self.undo_better_album(release),
            Command::RefreshReleaseCover(release) => self.refresh_release_cover(release),
            Command::RefreshStreamAlbumCover(album) => self.refresh_stream_album_cover(album),
            Command::FakeToMp3(track) => self.fake_to_mp3(track),
            Command::UndoFakeToMp3(pairs) => self.undo_fake_to_mp3(pairs),
            Command::RecordingApart(track) => self.recording_apart(track),
            Command::RestoreConvertedFlac => self.restore_converted_flac(),
            Command::LoadLibrary => self.load_library(),
            Command::DownloadsShown(shown) => self.torrents.shown = shown,
            Command::LoadTracks {
                offset,
                limit,
                sort,
            } => self.load_tracks(offset, limit, sort),
            Command::FilterTracks(q) => self.filter_tracks(&q),
            Command::SortReleases(sort) => {
                self.publish(|s| s.library.release_sort = sort);
                self.load_library();
            }
            Command::LoadRelease(id) => self.load_release(id),
            Command::LoadArtist(id) => self.load_artist(id),
            Command::LoadWorld(key) => self.load_world(&key),
            Command::LoadPlaylist(id) => {
                self.load_playlist(id);
                self.sync_on_open(id);
            }
            Command::LoadHome => self.load_home(),
            Command::HomeMore(row) => self.home_more(row),
            Command::Search(q) => self.search(q),
            Command::SetPrefs(p) => self.set_prefs(p),
            Command::SetPref(patch) => {
                let mut p = self.state.read().prefs.clone();
                patch.apply(&mut p);
                self.set_prefs(Box::new(p));
                if let nikgob_view::PrefPatch::Plus { playlist, on } = &patch {
                    self.plus_switched(*playlist, *on);
                }
                if let nikgob_view::PrefPatch::HideMix(m) = patch {
                    self.mix_hidden(m);
                }
            }
            Command::MoveMastersDir(dir) => self.move_masters_dir(dir),
            Command::SetIndexer { url, api_key } => self.set_indexer(url, api_key),
            Command::SetSoulseek(form) => self.set_soulseek(form),
            Command::SetSoulseekShare { share, upload_limit_kbps } => self.set_soulseek_share(share, upload_limit_kbps),
            Command::SetSoulseekSlots(n) => self.set_soulseek_slots(n),
            Command::CheckIndexer { url, api_key } => self.check_indexer(url, api_key),
            Command::DismissToast(id) => {
                self.publish(|s| s.toasts.retain(|t| t.id != id));
            }
            Command::OpenTagEditor(t) => self.open_tag_editor(t),
            Command::LookupTags { track, query } => self.lookup_tags(track, query),
            Command::SaveTags { track, tags } => self.save_tags(track, tags),
            Command::CloseTagEditor => self.publish(|s| s.tag_editor = Default::default()),
            Command::SetPlaylistCover(id, path) => self.set_playlist_cover(id, path),
            Command::SetMetadata { contact, acoustid_key } => self.set_metadata(contact, acoustid_key),
            Command::SetLastfmKey(key) => self.set_lastfm_key(&key),
            Command::SetListenbrainzToken(token) => self.set_listenbrainz_token(&token),
            Command::OpenSourcesConfig => self.open_sources_config(),
            Command::RealCovers(urls) => self.real_covers_wanted(urls),
            Command::SearchTorrents(q) => self.search_torrents(q),
            c @ (Command::DownloadTorrent { .. }
            | Command::ListTorrentFiles(_)
            | Command::CloseTorrentFiles
            | Command::CleanSeeding
            | Command::AddTorrentFile(_)
            | Command::AddMagnet(_)
            | Command::PauseJob(..)
            | Command::CancelJob(_)) => self.torrent_command(c),
            Command::ConfirmChoice { .. } => {
                log::warn!("confirm choice for an unknown item");
            }
            c @ (Command::StreamLogin { .. }
            | Command::StreamLogout(_)
            | Command::RetryStreamAudio(_)
            | Command::Soloist(_)
            | Command::SetStreamSecrets { .. }
            | Command::SetSearchSources(_)
            | Command::LoadStream(_)
            | Command::PlayStreamTracks { .. }
            | Command::PlaySearch { .. }
            | Command::PlayStreamAlbum { .. }
            | Command::PlayStreamPlaylist { .. }
            | Command::PlayStreamNext(_)
            | Command::EnqueueStream(_)
            | Command::PlayStreamAlbumNext(_)
            | Command::EnqueueStreamAlbum(_)
            | Command::PlayStreamPlaylistNext(_)
            | Command::EnqueueStreamPlaylist(_)
            | Command::PlayStreamArtist(_)
            | Command::PlayStreamArtistNext(_)
            | Command::EnqueueStreamArtist(_)
            | Command::LikeStream { .. }
            | Command::StreamInLibrary { .. }
            | Command::LikeStreamAlbum { .. }
            | Command::DownloadStream { .. }
            | Command::DownloadStreamAlbum(_)
            | Command::DownloadTracks(_)
            | Command::DownloadRelease(_)
            | Command::DownloadWholeRelease(_)
            | Command::FollowStreamPlaylist(_)
            | Command::AddStreamToPlaylist(..)
            | Command::AddStreamAlbumToPlaylist(..)
            | Command::Share(_)
            | Command::LoadWishlist
            | Command::WishlistDownload(_)
            | Command::WishlistDismiss(_)
            | Command::WishlistRestore(_)
            | Command::WishlistUndoDismiss { .. }
            | Command::WishlistRetry(_)
            | Command::WishlistRetryAll
            | Command::WishlistSaveFromYoutube(_)
            | Command::WishlistAttach { .. }
            | Command::ImportFromService(_)
            | Command::ImportExportFiles(_)
            | Command::ClearStreamCache
            | Command::WindowState { .. }
            | Command::PlayFromYoutube
            | Command::SetPlaylistSync { .. }
            | Command::LoadDiscography(_)
            | Command::FindSong { .. }
            | Command::CloseFindSong
            | Command::ReplaceTrack { .. }
            | Command::UndoReplace(_)) => {
                if let Some(c) = self.stream_command(c) {
                    log::warn!("unhandled {c:?}");
                }
            }
            c @ (Command::PlayMix { .. }
            | Command::StartRadio { .. }
            | Command::RecFeedback { .. }
            | Command::Dislike { .. }
            | Command::RefreshMix { .. }
            | Command::PlayBlend { .. }
            | Command::PlayWave { .. }
            | Command::SetWave(_)
            | Command::OtherVibe
            | Command::DownloadMix { .. }
            | Command::PlayMixNext { .. }
            | Command::EnqueueMix { .. }) => {
                if let Some(c) = self.recs_command(c) {
                    log::warn!("unhandled {c:?}");
                }
            }
            c @ (Command::SetDuck { .. } | Command::ClearDuck { .. }) => self.duck_command(c),
            Command::SetOutputDevice(device) => self.set_output_device(device),
            c @ (Command::SetApiEnabled(_)
            | Command::SetApiAccess(_)
            | Command::SetApiPort(_)
            | Command::ApiCreateToken(_)
            | Command::ApiRevokeToken(_)
            | Command::ApiRestoreToken(_)
            | Command::ApiStatus { .. }
            | Command::ApiTokensUsed(_)
            | Command::ApiRun { .. }
            | Command::ApiShuffle(_)
            | Command::ApiShare { .. }
            | Command::ApiReadTags { .. }
            | Command::ApiRetag { .. }
            | Command::ApiTrash { .. }
            | Command::ApiSourceSearch { .. }
            | Command::ApiConfirmDownload { .. }
            | Command::ApiKnow(_)
            | Command::ApiCreatePlaylist { .. }) => self.api_command(c),
            c @ (Command::MeshSetEnabled(_)
            | Command::MeshPairNearby { .. }
            | Command::MeshCreateInvite { .. }
            | Command::MeshCancelInvite
            | Command::MeshJoin { .. }
            | Command::MeshRename { .. }
            | Command::MeshForget { .. }
            | Command::MeshSetProfile { .. }
            | Command::MeshSetScope { .. }
            | Command::MeshUpdate(_)
            | Command::MeshRestore { .. }
            | Command::MeshClearPairing
            | Command::MeshForgetFailed { .. }
            | Command::MeshSetKeepMasters(_)
            | Command::MeshSetPins { .. }
            | Command::MeshPauseTransfer { .. }
            | Command::MeshForgetAnswer { .. }
            | Command::MeshSetSync { .. }
            | Command::MeshSyncReport { .. }) => self.mesh_command(c),
            c @ (Command::OutputTransfer { .. }
            | Command::OutputAttach { .. }
            | Command::OutputDetach
            | Command::OutputContinueHere
            | Command::OutputUndoTransfer
            | Command::ApiSnapshot { .. }
            | Command::ApiLoad { .. }
            | Command::ConnectRun { .. }
            | Command::ConnectReport(_)
            | Command::ApiTransfer { .. }) => self.connect_command(c),
            Command::ReplicaRechecked { card, ok, failed } => self.replica_rechecked(card, ok, failed),
            Command::ReplicaUndoTrash { ids } => self.replica_undo(ids),
            Command::ReplicaCopiesChanged => self.replica_soon(),
            Command::ReplicaFileKept => self.storage_changed(),
            Command::ReplicaRecheck { .. } => log::warn!("replica: a recheck goes to the mesh, not to the worker"),
        }
    }

    /// A track was liked / unliked here: the services hear about it, and a
    /// new like of a track without a file starts its download.
    /// A like / unlike of a track: to its services, and to «Хочу скачать».
    /// True when the like put the track there.
    pub(crate) fn track_liked(&mut self, track: TrackId, on: bool) -> bool {
        self.push_like(track, on);
        let wished = if on {
            self.wish_for_like(track, nikgob_core::model::WishlistOrigin::Like)
        } else {
            self.wish_drop_unstarted(track);
            false
        };
        self.publish_wishlist();
        self.wish_tick();
        wished
    }
}
