//! «Не нравится» and «Не рекомендовать исполнителя»:
//! the `rec_feedback` marks of tracks and artists, kept here as sets for
//! playback and the view.
//!
//! * A dislike is the track's `rec_feedback` mark: out of every
//!   recommendation at once, its nearest sound neighbours a little lower
//!   after the next build ([`nikgob_recs::dislike`]), never its artist. A
//!   like and a dislike exclude each other. A track the user loves (liked,
//!   or played to the end as often as their most played) is asked about
//!   first. «Отменить» takes it back, with the like.
//! * The playing track disliked is skipped at once
//!   ([`v::Prefs::dislike_skips`]). A disliked track, or one of an artist
//!   not recommended, is skipped when the app picked it (mixes, radios,
//!   autoplay), and in the user's own albums and playlists too with
//!   [`v::Prefs::dislike_skip_everywhere`]; a track the user picks plays.
//! * The second and the third dislike of one artist offer «Не рекомендовать
//!   исполнителя» with a snackbar; it is never done without asking.

use std::collections::HashSet;

use nikgob_core::model::{ArtistId, EventContext, EventOrigin, EventType, LikeTarget, TrackId};
use nikgob_view::{self as v, Command};

use crate::Worker;
use crate::queue::Entry;

/// A track played to the end at least this many times, and as often as
/// the top [`LOVED_SHARE`] of the user's played tracks, is loved: its
/// dislike is asked about first.
const LOVED_COMPLETES: u32 = 3;
const LOVED_SHARE: f32 = 0.05;
/// The dislikes of one artist that offer leaving the artist out.
const OFFER_AT: [usize; 2] = [2, 3];

/// The marks as playback reads them.
#[derive(Default)]
pub(crate) struct Dislikes {
    loaded: bool,
    pub tracks: HashSet<TrackId>,
    pub artists: HashSet<ArtistId>,
}

impl Worker {
    /// The marks from the library (once, and again after each change).
    pub(crate) fn dislikes_load(&mut self, again: bool) {
        if self.recs.dislikes.loaded && !again {
            return;
        }
        let marks = self.lib.rec_dislikes().unwrap_or_default();
        let d = &mut self.recs.dislikes;
        d.loaded = true;
        d.tracks = marks.iter().filter(|m| m.target_kind == "track").filter_map(|m| m.target_id.parse().ok()).collect();
        d.artists = marks.iter().filter(|m| m.target_kind == "artist").filter_map(|m| m.target_id.parse().ok()).collect();
        let mut tracks: Vec<(i64, v::DislikedTrack)> = Vec::new();
        let mut artists: Vec<(i64, (ArtistId, String))> = Vec::new();
        for m in &marks {
            match m.target_kind.as_str() {
                "track" => {
                    let Ok(id) = m.target_id.parse::<TrackId>() else { continue };
                    let Some(t) = self.lib.get_track(id).ok().flatten() else { continue };
                    tracks.push((m.updated_at, v::DislikedTrack { id, title: t.title, artist: t.artist_credit.display() }));
                }
                "artist" => {
                    let Ok(id) = m.target_id.parse::<ArtistId>() else { continue };
                    let Some(a) = self.lib.get_artist(id).ok().flatten() else { continue };
                    artists.push((m.updated_at, (id, a.name)));
                }
                _ => {}
            }
        }
        tracks.sort_by(|a, b| b.0.cmp(&a.0));
        artists.sort_by(|a, b| b.0.cmp(&a.0));
        let view = v::DislikesView {
            tracks: tracks.into_iter().map(|x| x.1).collect(),
            artists: artists.into_iter().map(|x| x.1).collect(),
        };
        if self.state.read().dislikes != view {
            self.publish(|s| s.dislikes = view);
        }
    }

    /// The main artists of `track`.
    fn artists_of(&self, track: TrackId) -> Vec<ArtistId> {
        self.lib.get_track(track).ok().flatten().map(|t| t.artist_credit.artist_ids().collect()).unwrap_or_default()
    }

    /// `e` is disliked (or by an artist not recommended) and is to be
    /// skipped now rather than played (see the module docs).
    pub(crate) fn skips_disliked(&mut self, e: Entry) -> bool {
        self.dislikes_load(false);
        let d = &self.recs.dislikes;
        if d.tracks.is_empty() && d.artists.is_empty() {
            return false;
        }
        let out = d.tracks.contains(&e.track) || self.artists_of(e.track).iter().any(|a| self.recs.dislikes.artists.contains(a));
        if !out || self.next_origin == Some(EventOrigin::Manual) {
            return false;
        }
        let everywhere = self.state.read().prefs.dislike_skip_everywhere;
        let picked_by_app = matches!(self.play_context, EventContext::HomeSection | EventContext::Radio)
            || self.autoplay_owns(e.id)
            || self.radio_origin(e.id).is_some();
        everywhere || picked_by_app
    }

    /// Played to the end as often as the user's most played tracks.
    fn loved(&self, track: TrackId) -> bool {
        let Some(snap) = self.snapshot() else { return false };
        let m = &snap.model;
        let Some(i) = m.position(track) else { return false };
        let mut completes: Vec<u32> = m.tracks.iter().map(|t| t.completes).filter(|c| *c > 0).collect();
        if completes.is_empty() {
            return false;
        }
        completes.sort_unstable_by(|a, b| b.cmp(a));
        let k = ((completes.len() as f32 * LOVED_SHARE) as usize).min(completes.len() - 1);
        let c = m.tracks[i].completes;
        c >= LOVED_COMPLETES && c >= completes[k]
    }

    /// Sets the like of `track` without a message (a dislike takes it).
    fn like_quietly(&mut self, track: TrackId, on: bool) {
        let before = self.library_tracks();
        let target = LikeTarget::Track(track);
        let r = if on { self.lib.like(target) } else { self.lib.unlike(target) };
        if self.log_err("лайк", r).is_some() {
            let _ = self.lib.record_event(track, if on { EventType::Like } else { EventType::Unlike }, None, None, EventContext::Other);
            self.track_liked(track, on);
            self.refresh_liked(target, on, before);
        }
    }

    /// A like of `track`: its dislike goes, and so does the «не нужна» of
    /// its song on its album (a song deleted from it, liked on purpose).
    pub(crate) fn liked_undislikes(&mut self, track: TrackId) {
        if self.lib.forget_unwanted_song(track).unwrap_or(false) {
            self.recs_dirty(std::time::Duration::ZERO);
        }
        self.dislikes_load(false);
        if self.recs.dislikes.tracks.contains(&track) {
            let _ = self.log_err("отзыв", self.lib.set_rec_feedback("track", &track.to_string(), false));
            self.dislikes_changed();
        }
    }

    fn dislikes_changed(&mut self) {
        self.dislikes_load(true);
        self.publish_recs_home();
        self.recs_dirty(std::time::Duration::ZERO);
    }

    /// `Command::Dislike`.
    pub(crate) fn dislike(&mut self, track: TrackId, dislike: bool, sure: bool, relike: bool) {
        let Some(row) = self.lib.get_track(track).ok().flatten() else { return };
        let title = row.title.clone();
        if !dislike {
            if self.log_err("отзыв", self.lib.set_rec_feedback("track", &track.to_string(), false)).is_none() {
                return;
            }
            if relike {
                self.like_quietly(track, true);
            }
            // Taken back by hand (not the undo of a dislike): «Отменить»
            // marks it again.
            let undo = (!relike).then(|| ("Отменить".to_owned(), Command::Dislike { track, dislike: true, sure: true, relike: false }));
            self.toast(format!("«{title}» снова в рекомендациях"), undo);
            self.dislikes_changed();
            return;
        }
        let liked = self.lib.is_liked(LikeTarget::Track(track)).unwrap_or(false);
        if !sure && (liked || self.loved(track)) {
            let text = if liked { format!("«{title}» в Любимых. Не нравится?") } else { format!("«{title}» ты часто дослушиваешь. Не нравится?") };
            self.toast(text, Some(("Не нравится".into(), Command::Dislike { track, dislike: true, sure: true, relike: false })));
            return;
        }
        if self.log_err("отзыв", self.lib.set_rec_feedback("track", &track.to_string(), true)).is_none() {
            return;
        }
        if liked {
            self.like_quietly(track, false);
        }
        self.toast(
            format!("Не нравится: «{title}»"),
            Some(("Отменить".into(), Command::Dislike { track, dislike: false, sure: true, relike: liked })),
        );
        self.dislikes_changed();
        let skips = self.state.read().prefs.dislike_skips;
        self.radio_disliked(track, skips);
        if self.playing.is_some_and(|e| e.track == track) && skips {
            self.command(Command::Next);
        }
        // A few of one artist: offer to leave the artist out.
        let artists = self.artists_of(track);
        if let Some(&a) = artists.first()
            && !self.recs.dislikes.artists.contains(&a)
        {
            let tracks: Vec<TrackId> = self.recs.dislikes.tracks.iter().copied().collect();
            let n = tracks.iter().filter(|t| self.artists_of(**t).first() == Some(&a)).count();
            if OFFER_AT.contains(&n)
                && let Some(name) = self.lib.get_artist(a).ok().flatten().map(|a| a.name)
            {
                self.toast(
                    format!("Не рекомендовать исполнителя «{name}»?"),
                    Some(("Не рекомендовать".into(), Command::RecFeedback { target: v::RecTarget::Artist(a), dislike: true })),
                );
            }
        }
    }
}
