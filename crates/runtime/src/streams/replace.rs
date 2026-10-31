//! «Найти песню…» / «Другая версия…»: a stream track replaced by hand with
//! the song the user picked (docs/stage3.md §7 «Клипы в лайках»).
//!
//! * [`Command::FindSong`] opens [`v::FindSongView`] and searches songs
//!   only (one songs request to YouTube Music, one to Deezer), the merged
//!   list ranked by the query; a song both have shows once, as YouTube's.
//! * [`Command::ReplaceTrack`] makes the library track the picked song
//!   ([`nikgob_library::Library::replace_with_song`]); a Deezer pick plays
//!   from its YouTube song (its ISRC, else its title), looked up first.
//!   «Только здесь»: the remote like stays the clip's, exactly as the
//!   automatic conversion leaves it. «И в YouTube Music»: through the like
//!   queue the clip is unliked there and the song liked, and the like's
//!   identity becomes the song; for a while the like checks take neither
//!   the clip back as news nor the song as gone (YouTube's list lags).
//! * A track replaced by hand is marked (`replaced.{track}` and its liked
//!   clip as `yt_clip.{id} = manual` in the library cache): the background
//!   repair never touches it again, undone or not.
//! * The toast's «Отменить» ([`Command::UndoReplace`]) puts the track back
//!   as it was and, when the like went to YouTube, sends it back the other
//!   way (a change still in the queue just cancels out).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use nikgob_core::model::TrackId;
use nikgob_core::streaming::{RemoteId, SearchKind, Service, StreamError, StreamTrack, TrackKind};
use nikgob_library::{RemoteLike, TrackSnapshot};
use nikgob_view::{self as v, Command};

use super::Msg;
use super::rank::Query;
use super::resolve::{ResolveQuery, search_youtube};
use super::search::SongKey;
use crate::Worker;

/// Results shown at most.
const SHOWN: usize = 20;
/// How long the like checks keep trusting a like change sent from here
/// over what the service's list still says.
pub(crate) const ECHO_TTL: Duration = Duration::from_secs(15 * 60);
/// Library cache key of a track replaced by hand.
pub(crate) fn replaced_key(track: TrackId) -> String {
    format!("replaced.{track}")
}
/// The value a liked clip replaced by hand gets under its `yt_clip.` key.
pub(crate) const MANUAL: &str = "manual";

/// One replacement, to take back.
pub(crate) struct Undo {
    snapshot: TrackSnapshot,
    /// What went to YouTube Music: the ids unliked there and the one liked.
    remote: Option<(Vec<RemoteId>, RemoteId)>,
}

/// The dialog's searches and the replacements that can still be undone.
#[derive(Default)]
pub(crate) struct ReplaceState {
    /// The search the dialog shows (answers of older ones are dropped).
    seq: u64,
    opened: u64,
    youtube: Option<Result<Vec<StreamTrack>, StreamError>>,
    deezer: Option<Result<Vec<StreamTrack>, StreamError>>,
    asked: Vec<Service>,
    undo: HashMap<u64, Undo>,
    undo_seq: u64,
    /// Like changes sent from here lately: (on, when).
    echo: HashMap<RemoteId, (bool, Instant)>,
}

impl ReplaceState {
    /// A like change sent from here for `remote`, if it is recent.
    pub(crate) fn echo(&self, remote: &RemoteId) -> Option<bool> {
        self.echo
            .get(remote)
            .filter(|(_, at)| at.elapsed() < ECHO_TTL)
            .map(|(on, _)| *on)
    }
}

/// «Eve» of «Eve feat. X», «Eve, X».
fn main_artist(credit: &str) -> &str {
    let cut = [", ", " feat. ", " feat ", " ft. "]
        .iter()
        .filter_map(|sep| credit.find(sep))
        .min()
        .unwrap_or(credit.len());
    credit[..cut].trim()
}

/// How a library row is titled: a clip by the song title inside its own.
pub(crate) fn shown_title(row: &nikgob_library::TrackRow) -> String {
    if row.is_clip() {
        nikgob_core::clip_title::display_title(&row.title, main_artist(&row.artist))
    } else {
        row.title.clone()
    }
}

impl Worker {
    /// `Command::FindSong`.
    pub(crate) fn find_song(&mut self, track: TrackId, query: Option<String>) {
        let Some(row) = self.lib.get_track_row(track).ok().flatten() else { return };
        let clip = row.is_clip();
        let title = shown_title(&row);
        let fresh = self.state.read().find_song.as_ref().is_none_or(|f| f.track != track) || query.is_none();
        let query = query
            .map(|q| q.trim().to_owned())
            .unwrap_or_else(|| format!("{title} {}", main_artist(&row.artist)).trim().to_owned());
        let youtube_off = self.youtube_off(track, clip);
        let r = &mut self.streaming.replace;
        r.seq += 1;
        if fresh {
            r.opened += 1;
        }
        r.youtube = None;
        r.deezer = None;
        r.asked.clear();
        let (seq, opened) = (r.seq, r.opened);
        let mut asked = Vec::new();
        if !query.is_empty() {
            for service in [Service::Youtube, Service::Deezer] {
                let Some(svc) = self.service(service) else { continue };
                asked.push(service);
                let q = query.clone();
                self.streaming.spawn(async move {
                    let result = svc.search_kind(&q, SearchKind::Songs).await.map(|s| s.tracks);
                    Msg::SongsFound { seq, service, result }
                });
            }
        }
        let error = (asked.is_empty() && !query.is_empty()).then(|| "YouTube Music и Deezer выключены".to_owned());
        self.streaming.replace.asked = asked;
        let view = v::FindSongView {
            track,
            clip,
            title,
            artist: row.artist.clone(),
            duration: row.duration_ms.map(|ms| Duration::from_millis(u64::from(ms))),
            query,
            opened,
            loading: !self.streaming.replace.asked.is_empty(),
            results: Vec::new(),
            error,
            youtube_off,
            busy: false,
        };
        self.publish(|s| s.find_song = Some(view));
    }

    /// Why the like cannot move to YouTube Music for `track`, if it cannot.
    fn youtube_off(&self, track: TrackId, clip: bool) -> Option<String> {
        let signed_in = self
            .service(Service::Youtube)
            .is_some_and(|s| s.account().is_logged_in());
        if !signed_in {
            return Some("Нужен вход в YouTube Music".into());
        }
        let liked = self
            .lib
            .remote_likes_of(track)
            .unwrap_or_default()
            .iter()
            .any(|l| l.remote.service == Service::Youtube);
        (!liked).then(|| if clip { "Клип не лайкнут в YouTube Music" } else { "Трек не лайкнут в YouTube Music" }.into())
    }

    /// An answer of the dialog's search.
    pub(crate) fn songs_found(&mut self, seq: u64, service: Service, result: Result<Vec<StreamTrack>, StreamError>) {
        if seq != self.streaming.replace.seq || self.state.read().find_song.is_none() {
            return;
        }
        if let Ok(tracks) = &result {
            self.streaming.learn(tracks.iter());
        }
        match service {
            Service::Deezer => self.streaming.replace.deezer = Some(result),
            _ => self.streaming.replace.youtube = Some(result),
        }
        self.publish_found();
    }

    fn publish_found(&mut self) {
        let Some(view) = self.state.read().find_song.clone() else { return };
        let r = &self.streaming.replace;
        let playing_now = self
            .lib
            .get_track_row(view.track)
            .ok()
            .flatten()
            .and_then(|row| row.youtube_id);
        let ok = |a: &Option<Result<Vec<StreamTrack>, StreamError>>| a.as_ref().and_then(|r| r.as_ref().ok()).cloned().unwrap_or_default();
        let yt: Vec<StreamTrack> = ok(&r.youtube)
            .into_iter()
            .filter(|t| t.kind == TrackKind::Song && t.playable && Some(&t.id.id) != playing_now.as_ref())
            .collect();
        let keys: Vec<SongKey> = yt.iter().map(SongKey::of_stream).collect();
        let dz: Vec<StreamTrack> = ok(&r.deezer)
            .into_iter()
            .filter(|t| t.playable)
            .filter(|t| {
                let k = SongKey::of_stream(t);
                !keys.iter().any(|y| y.same(&k))
            })
            .collect();
        let q = Query::new(&view.query);
        let mut all: Vec<(f64, usize, StreamTrack)> = yt
            .into_iter()
            .chain(dz)
            .enumerate()
            .map(|(i, t)| {
                let artists: Vec<&str> = t.artists.iter().map(|a| a.name.as_str()).collect();
                let album = t.album.as_ref().map(|a| a.title.as_str());
                (q.track(&t.title, &artists, album, t.kind), i, t.clone())
            })
            .collect();
        all.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let results: Vec<v::StreamTrackRow> = all
            .iter()
            .take(SHOWN)
            .map(|(_, _, t)| self.stream_row(t, self.library_track_of(t)))
            .collect();
        let answered = |s: Service| match s {
            Service::Deezer => r.deezer.is_some(),
            _ => r.youtube.is_some(),
        };
        let loading = r.asked.iter().any(|s| !answered(*s));
        let failed: Vec<&StreamError> = [&r.youtube, &r.deezer]
            .into_iter()
            .filter_map(|a| a.as_ref().and_then(|r| r.as_ref().err()))
            .collect();
        let error = (!loading && results.is_empty() && !failed.is_empty() && failed.len() == r.asked.len())
            .then(|| super::error_text(failed[0]));
        self.publish(|s| {
            if let Some(f) = s.find_song.as_mut().filter(|f| f.track == view.track) {
                f.results = results;
                f.loading = loading;
                f.error = error;
            }
        });
    }

    /// `Command::ReplaceTrack`.
    pub(crate) fn replace_track(&mut self, track: TrackId, with: RemoteId, youtube: bool) {
        if self.state.read().prefs.replace_in_youtube != youtube {
            self.publish(|s| s.prefs.replace_in_youtube = youtube);
            self.save_prefs();
        }
        let Some(pick) = self.streaming.known.get(&with).cloned() else {
            self.toast("Песня устарела — найди её заново", None);
            return;
        };
        if pick.id.service != Service::Deezer {
            self.apply_replace(track, pick, None, youtube);
            return;
        }
        // A Deezer song plays from its YouTube song: found first.
        let Some(yt) = self.service(Service::Youtube) else {
            self.apply_replace(track, pick, None, youtube);
            return;
        };
        let dz = self.service(Service::Deezer);
        self.publish(|s| {
            if let Some(f) = s.find_song.as_mut() {
                f.busy = true;
            }
        });
        self.streaming.spawn(async move {
            let mut q = ResolveQuery {
                title: pick.title.clone(),
                artist: pick.artist_line(),
                duration_ms: pick.duration_ms,
                isrc: pick.isrc.clone(),
                exclude: Vec::new(),
                aliases: Vec::new(),
            };
            if q.isrc.is_none()
                && let Some(dz) = dz
                && let Ok(full) = dz.track(&pick.id.id).await
            {
                q.isrc = full.isrc;
            }
            let mut found = None;
            if let Some(isrc) = &q.isrc
                && let Ok(Some(hit)) = yt.find_isrc(isrc).await
                && hit.kind == TrackKind::Song
            {
                found = Some(hit);
            }
            if found.is_none() {
                found = search_youtube(yt.as_ref(), &q).await.filter(|h| h.kind == TrackKind::Song);
            }
            let mut pick = pick;
            if pick.isrc.is_none() {
                pick.isrc = q.isrc;
            }
            Msg::ReplaceResolved {
                track,
                pick: Box::new(pick),
                youtube_song: found.map(Box::new),
                youtube,
            }
        });
    }

    /// The YouTube song of a Deezer pick came (or none): the replacement.
    pub(crate) fn replace_resolved(&mut self, track: TrackId, pick: StreamTrack, youtube_song: Option<StreamTrack>, youtube: bool) {
        if let Some(s) = &youtube_song {
            self.streaming.learn(std::slice::from_ref(s));
        }
        self.apply_replace(track, pick, youtube_song, youtube);
    }

    fn apply_replace(&mut self, track: TrackId, pick: StreamTrack, youtube_song: Option<StreamTrack>, youtube: bool) {
        let clip = self.lib.get_track_row(track).ok().flatten().is_some_and(|r| r.is_clip());
        let allowed = youtube && self.youtube_off(track, clip).is_none();
        let liked_there: Vec<RemoteId> = self
            .lib
            .remote_likes_of(track)
            .unwrap_or_default()
            .into_iter()
            .map(|l| l.remote)
            .filter(|r| r.service == Service::Youtube)
            .collect();
        let mut songs = vec![pick.clone()];
        songs.extend(youtube_song.clone());
        let Some(snapshot) = self.log_err("замена трека", self.lib.replace_with_song(track, &songs)) else {
            self.publish(|s| {
                if let Some(f) = s.find_song.as_mut() {
                    f.busy = false;
                }
            });
            return;
        };
        log::info!("{track} replaced by hand with {}", pick.id);
        let now = nikgob_library::now_ms();
        let _ = self.lib.cache_put(&replaced_key(track), now, MANUAL);
        for r in &liked_there {
            let _ = self.lib.cache_put(&super::clips::clip_key(&r.id), now, MANUAL);
        }
        // The like there follows: the clip unliked, the song liked.
        let song_there = if pick.id.service == Service::Youtube { Some(pick.id.clone()) } else { youtube_song.map(|s| s.id) };
        let mut remote = None;
        let mut text = format!("Заменено на «{}»", pick.title);
        if allowed {
            match song_there {
                Some(to) => {
                    let from: Vec<RemoteId> = liked_there.into_iter().filter(|r| *r != to).collect();
                    self.move_like(track, &from, &to);
                    remote = Some((from, to));
                }
                None => text.push_str(" · только здесь: в YouTube Music нет этой песни"),
            }
        }
        self.streaming.replace.undo_seq += 1;
        let id = self.streaming.replace.undo_seq;
        self.streaming.replace.undo.insert(id, Undo { snapshot, remote });
        self.publish(|s| s.find_song = None);
        self.track_replaced(track);
        self.toast(text, Some(("Отменить".into(), Command::UndoReplace(id))));
    }

    /// The like of `track` on YouTube Music goes from `from` to `to`
    /// (through the like queue).
    fn move_like(&mut self, track: TrackId, from: &[RemoteId], to: &RemoteId) {
        let now = nikgob_library::now_ms();
        for r in from {
            let _ = self.lib.delete_remote_like(r);
            self.queue_like(r.clone(), false);
        }
        let _ = self.lib.put_remote_like(&RemoteLike {
            remote: to.clone(),
            track_id: track,
            pulled: false,
            seen_at: now,
        });
        self.queue_like(to.clone(), true);
    }

    /// Queues one like change for its service, remembered as sent from here.
    fn queue_like(&mut self, remote: RemoteId, on: bool) {
        self.streaming.replace.echo.retain(|_, (_, at)| at.elapsed() < ECHO_TTL);
        self.streaming.replace.echo.insert(remote.clone(), (on, Instant::now()));
        self.push_like_remote(remote, on);
    }

    /// `Command::UndoReplace`.
    pub(crate) fn undo_replace(&mut self, id: u64) {
        let Some(undo) = self.streaming.replace.undo.remove(&id) else { return };
        let track = undo.snapshot.track;
        if self.log_err("отмена замены", self.lib.restore_track(&undo.snapshot)).is_none() {
            return;
        }
        if let Some((from, to)) = undo.remote {
            // The remote like rows came back with the track's; the change
            // goes back the other way.
            self.queue_like(to, false);
            for r in from {
                self.queue_like(r, true);
            }
        }
        self.track_replaced(track);
        self.toast("Замена отменена", None);
    }

    /// The track's metadata and audio changed by hand: what shows or plays
    /// it follows, its lyrics are looked up anew.
    fn track_replaced(&mut self, track: TrackId) {
        if let Some(svc) = self.import.as_mut() {
            svc.lyrics_cache.remove(&track);
        }
        self.wish_track_changed(track);
        self.streaming.search.forget_links();
        self.after_like_change(crate::Held::Tracks(None), Vec::new());
        let open = self.state.read().playlist.as_ref().map(|p| p.card.id);
        if let Some(p) = open.filter(|p| *p != nikgob_view::LIKED_PLAYLIST) {
            self.load_playlist(p);
        }
        match self.playing.filter(|e| e.track == track) {
            // What plays now is the picked song from its start.
            Some(e) => {
                let paused = self.status == v::Status::Paused;
                self.start(e);
                if paused {
                    self.pause();
                }
            }
            None => {
                self.preload_next();
                self.publish_now_playing();
            }
        }
    }

    /// A track replaced by hand: the clip repair leaves it alone.
    pub(crate) fn replaced_by_hand(&self, track: TrackId) -> bool {
        self.lib.cache_get(&replaced_key(track)).ok().flatten().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::main_artist;

    #[test]
    fn the_main_artist_of_a_credit() {
        assert_eq!(main_artist("Eve"), "Eve");
        assert_eq!(main_artist("Eve feat. Suis"), "Eve");
        assert_eq!(main_artist("Eve, Suis"), "Eve");
        assert_eq!(main_artist("Simon & Garfunkel"), "Simon & Garfunkel");
    }
}
