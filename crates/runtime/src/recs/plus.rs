//! «✦ С дополнением» in the queue (docs/recommendations.md §4): while a
//! playlist plays with its chip on, a guest of the open pool joins it
//! after every three of its own tracks ([`nikgob_recs::plus`]).
//!
//! * The guests of the playlist are ranked on a thread when it starts and
//!   again when the snapshot or the discoveries change; a guest takes its
//!   place only when that place comes within the live buffer
//!   ([`nikgob_recs::plus::AHEAD`]), one at a time, from what is ranked.
//! * A guest plays from its service like any new song (never downloaded on
//!   its own); ♡ or adding it to a playlist keeps it where it is. A skip of
//!   a guest takes every guest ahead out of the queue at once and no other
//!   comes while this queue plays (the chip stays on: the next play of the
//!   playlist, or the chip switched off and on, brings them back, never of
//!   the skipped guest's artist in this queue); turning the chip off takes
//!   them out too. The playlist's own entries are never touched, nor the
//!   playlist itself.
//! * A guest that cannot play (no network, no service) is passed over in
//!   silence and no other comes until the queue is replaced.
//! * The chip of each playlist is remembered in the prefs
//!   ([`nikgob_view::Prefs::plus_playlists`]).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use nikgob_core::model::{PlaylistId, TrackId};
use nikgob_recs::open_world::WaveContext;
use nikgob_recs::plus::{Guest, Slot};

use super::Msg;
use super::niche::NicheFound;
use crate::Worker;
use crate::import::Internal;
use crate::queue::Entry;

/// The playlist the guests join.
pub(crate) struct PlusRun {
    id: u64,
    playlist: PlaylistId,
    /// The queue it plays in.
    queue_gen: u64,
    /// The entry playing when the guests began: the count starts there.
    from: u64,
    /// The playlist's own entries.
    own: HashSet<u64>,
    /// Its tracks in its order (the seeds of the ranking).
    tracks: Vec<TrackId>,
    /// The guests in the queue: entry → (track, song key, artist key).
    guests: HashMap<u64, (TrackId, String, String)>,
    /// The guests that came (song keys).
    served: HashSet<String>,
    /// The guests ranked, and what for: the snapshot and the discoveries.
    list: Vec<Guest>,
    ranked_for: Option<(usize, usize)>,
    ranking: bool,
    /// A guest was skipped or could not play: no more in this queue.
    stopped: bool,
}

#[derive(Default)]
pub(crate) struct PlusState {
    run: Option<PlusRun>,
    seq: u64,
    /// The queue the guests were looked at for already (once a queue).
    tried: Option<u64>,
    /// The artists of the guests skipped in that queue.
    away: (u64, HashSet<String>),
}

pub(crate) enum PlusMsg {
    Ranked { id: u64, guests: Vec<Guest>, ranked_for: (usize, usize) },
}

/// The playlist a queue's context id names (`playlist:<id>`).
fn playlist_of(context: &str) -> Option<PlaylistId> {
    context.strip_prefix("playlist:")?.parse().ok()
}

impl PlusState {
    /// The guest entries in the queue.
    pub(crate) fn guests(&self) -> HashSet<u64> {
        self.run.as_ref().map(|r| r.guests.keys().copied().collect()).unwrap_or_default()
    }
}

impl Worker {
    fn plus_on(&self, playlist: PlaylistId) -> bool {
        self.state.read().prefs.plus_playlists.contains(&playlist)
    }

    /// Before the player is published: the guests' run follows the queue,
    /// starts on a playlist with its chip on, and fills the places that
    /// came near.
    pub(crate) fn plus_top_up(&mut self) {
        let generation = self.queue.generation();
        if self.recs.plus.run.as_ref().is_some_and(|r| r.queue_gen != generation || !self.plus_on(r.playlist)) {
            self.recs.plus.run = None;
        }
        if self.recs.plus.run.is_none() {
            if self.recs.plus.tried == Some(generation) {
                return;
            }
            self.recs.plus.tried = Some(generation);
            let Some(playlist) = self.context_page_id().and_then(playlist_of) else { return };
            if !self.plus_on(playlist) {
                return;
            }
            self.plus_start(playlist);
        }
        // «Только скачанное» of the recommendations: nothing from the services.
        let local_only = self.state.read().prefs.mixes_downloaded_only;
        if local_only || self.recs.plus.run.as_ref().is_none_or(|r| r.stopped) {
            return;
        }
        self.plus_rank();
        self.plus_fill();
    }

    /// The guests begin with the track playing.
    fn plus_start(&mut self, playlist: PlaylistId) {
        let Some(current) = self.queue.current() else { return };
        let (tracks, _) = self.playlist_tracks(playlist, None);
        let set: HashSet<TrackId> = tracks.iter().copied().collect();
        let mut order = self.queue.history().to_vec();
        order.push(current);
        order.extend_from_slice(self.queue.upcoming());
        let own = order.iter().filter(|e| set.contains(&e.track)).map(|e| e.id).collect();
        self.recs.plus.seq += 1;
        log::info!("plus: guests join the playlist {playlist}");
        self.recs.plus.run = Some(PlusRun {
            id: self.recs.plus.seq,
            playlist,
            queue_gen: self.queue.generation(),
            from: current.id,
            own,
            tracks,
            guests: HashMap::new(),
            served: HashSet::new(),
            list: Vec::new(),
            ranked_for: None,
            ranking: false,
            stopped: false,
        });
        if self.recs.plus.away.0 != self.queue.generation() {
            self.recs.plus.away = (self.queue.generation(), HashSet::new());
        }
    }

    /// The guests are ranked again when the snapshot or the discoveries
    /// changed since (on a thread).
    fn plus_rank(&mut self) {
        let Some(snap) = self.snapshot() else { return };
        let ranked_for = (Arc::as_ptr(&snap) as usize, self.discoveries_kept());
        let Some(run) = self.recs.plus.run.as_ref() else { return };
        if run.ranking || run.ranked_for == Some(ranked_for) {
            return;
        }
        let (id, tracks) = (run.id, run.tracks.clone());
        let topped: Vec<NicheFound> = match self.recs.home.as_ref() {
            Some(h) if !snap.local_only => h.lanes.iter().filter(|l| !l.niche.is_empty()).filter_map(|l| super::niche::cached(&self.lib, l.key, &l.niche)).collect(),
            _ => Vec::new(),
        };
        let (discoveries, sources, seeds) = (self.discoveries_all(), self.discovery_sources(), self.discovery_seeds());
        let tx = self.recs.tx.clone();
        let spawned = std::thread::Builder::new().name("nikgob-plus".into()).spawn(move || {
            let mut pool = super::open::open_pool(&snap, &discoveries, &sources, &topped);
            let playlist: Vec<usize> = tracks.iter().filter_map(|t| snap.model.position(*t)).collect();
            // A discovery the radio of one of the playlist's songs (or a
            // list of one of its artists) found is linked to those songs.
            let m = &snap.model;
            for c in &mut pool {
                let Some((by_tracks, by_artists)) = seeds.get(&c.key) else { continue };
                let linked = by_tracks
                    .iter()
                    .filter_map(|t| m.position(*t))
                    .chain(playlist.iter().copied().filter(|&i| m.main_artist_at(i).is_some_and(|a| by_artists.contains(&a))));
                for i in linked {
                    if !c.near.contains(&i) {
                        c.near.push(i);
                    }
                }
            }
            let ok = |_: usize| true;
            let background = |i: usize| snap.background.get(i).copied().unwrap_or(false);
            let artist_tracks = |t: &nikgob_core::streaming::StreamTrack| super::lanes::artist_tracks(&snap, t);
            let ctx = WaveContext { model: &snap.model, ok: &ok, genre: None, background: &background, artist_tracks: &artist_tracks };
            let guests = nikgob_recs::plus::guests(&ctx, &pool, &playlist);
            let _ = tx.send(Internal::Recs(Msg::Plus(PlusMsg::Ranked { id, guests, ranked_for })));
        });
        match spawned {
            Ok(_) => {
                if let Some(run) = self.recs.plus.run.as_mut() {
                    run.ranking = true;
                }
            }
            Err(e) => log::warn!("plus: cannot rank: {e}"),
        }
    }

    /// A new snapshot or new discoveries: the guests are ranked again.
    pub(crate) fn plus_ready(&mut self) {
        if self.recs.plus.run.is_some() {
            self.plus_top_up();
        }
    }

    pub(crate) fn plus_msg(&mut self, m: PlusMsg) {
        match m {
            PlusMsg::Ranked { id, guests, ranked_for } => {
                let Some(run) = self.recs.plus.run.as_mut().filter(|r| r.id == id) else { return };
                log::info!("plus: {} guests for the playlist {}", guests.len(), run.playlist);
                run.ranking = false;
                run.ranked_for = Some(ranked_for);
                run.list = guests;
                self.publish_player();
            }
        }
    }

    /// Every entry of the queue in play order and the place of the one
    /// playing.
    fn plus_order(&self) -> Option<(Vec<Entry>, usize)> {
        let current = self.queue.current()?;
        let mut order = self.queue.history().to_vec();
        let at = order.len();
        order.push(current);
        order.extend_from_slice(self.queue.upcoming());
        Some((order, at))
    }

    /// The places due a guest (see [`nikgob_recs::plus::due`]) get one.
    fn plus_fill(&mut self) {
        let Some(snap) = self.snapshot() else { return };
        let Some((order, at)) = self.plus_order() else { return };
        let Some(run) = self.recs.plus.run.as_ref() else { return };
        if run.list.is_empty() {
            return;
        }
        let start = order.iter().position(|e| e.id == run.from).filter(|&s| s <= at).unwrap_or(at);
        let slots: Vec<Slot> = order[start..]
            .iter()
            .map(|e| {
                if run.guests.contains_key(&e.id) {
                    Slot::Guest
                } else if run.own.contains(&e.id) {
                    Slot::Own
                } else {
                    Slot::Other
                }
            })
            .collect();
        let due = nikgob_recs::plus::due(&slots, at - start);
        if due.is_empty() {
            return;
        }
        let mut next_changed = false;
        for i in due {
            let anchor = order[start + i];
            // The playlist's songs just before the place, the nearest last.
            let mut before: Vec<usize> = order[start..=start + i]
                .iter()
                .rev()
                .filter(|e| run_own(self, e.id))
                .take(nikgob_recs::plus::EVERY)
                .filter_map(|e| snap.model.position(e.track))
                .collect();
            before.reverse();
            let Some(run) = self.recs.plus.run.as_ref() else { return };
            let Some(k) = nikgob_recs::plus::pick(&snap.model, &run.list, &before, &run.served, &self.recs.plus.away.1) else {
                return;
            };
            let guest = run.list[k].clone();
            self.streaming.learn(std::slice::from_ref(&guest.track));
            let Some(tid) = self.upsert_streams(std::slice::from_ref(&guest.track)).first().copied() else { return };
            let Some(entry) = self.queue.enqueue(&[tid]).first().copied() else { return };
            self.queue.move_next_to(entry.id, anchor.id, true);
            next_changed |= start + i == at;
            if let Some(why) = self.discovery_why(&guest.track) {
                self.recs.why.insert(entry.id, why);
            }
            if let Some(run) = self.recs.plus.run.as_mut() {
                run.served.insert(guest.key.clone());
                run.guests.insert(entry.id, (tid, guest.key, guest.artist));
            }
            log::info!("plus: a guest «{}» joins after place {}", guest.track.title, i + 1);
        }
        if next_changed {
            self.preload_next();
        }
    }

    /// The guests ahead leave the queue (the playlist's entries stay).
    fn plus_drop_ahead(&mut self) {
        let Some(run) = self.recs.plus.run.as_ref() else { return };
        let next = self.queue.upcoming().first().map(|e| e.id);
        let ahead: Vec<u64> = self.queue.upcoming().iter().map(|e| e.id).filter(|id| run.guests.contains_key(id)).collect();
        if ahead.is_empty() {
            return;
        }
        for id in &ahead {
            self.queue.remove(*id);
            self.recs.why.remove(id);
        }
        if let Some(run) = self.recs.plus.run.as_mut() {
            for id in &ahead {
                run.guests.remove(id);
            }
        }
        log::info!("plus: {} guests left the queue", ahead.len());
        if next.is_some_and(|n| ahead.contains(&n)) {
            self.preload_next();
        }
    }

    /// A track ended: a guest skipped takes the guests ahead with it and
    /// no other comes while this queue plays; its artist none at all in it.
    pub(crate) fn plus_ended(&mut self, entry: u64, complete: bool) {
        if complete {
            return;
        }
        let Some(run) = self.recs.plus.run.as_mut() else { return };
        let Some((_, _, artist)) = run.guests.get(&entry).cloned() else { return };
        log::info!("plus: a guest skipped, no more in this queue");
        run.stopped = true;
        if !artist.is_empty() {
            self.recs.plus.away.1.insert(artist);
        }
        self.plus_drop_ahead();
    }

    /// A guest could not play: it is passed over without a word (no
    /// network, no service), the guests ahead leave and no more come until
    /// the queue is replaced. Whether `entry` was a guest.
    pub(crate) fn plus_failed(&mut self, entry: u64) -> bool {
        let Some(run) = self.recs.plus.run.as_mut() else { return false };
        if !run.guests.contains_key(&entry) {
            return false;
        }
        log::info!("plus: a guest cannot play, no more in this queue");
        run.stopped = true;
        self.plus_drop_ahead();
        true
    }

    /// ♡ or «+» on a guest: it stays where it is, as one of the queue's
    /// own (the chip going off leaves it).
    pub(crate) fn plus_kept(&mut self, track: TrackId) {
        if let Some(run) = self.recs.plus.run.as_mut() {
            run.guests.retain(|_, (t, _, _)| *t != track);
        }
    }

    /// The chip of `playlist` was switched: off, the guests ahead leave the
    /// queue at once; on, they begin with the track playing if the
    /// playlist plays.
    pub(crate) fn plus_switched(&mut self, playlist: PlaylistId, on: bool) {
        if on {
            self.recs.plus.tried = None;
        } else if self.recs.plus.run.as_ref().is_some_and(|r| r.playlist == playlist) {
            self.plus_drop_ahead();
            self.recs.plus.run = None;
        }
        self.publish_player();
    }
}

/// Whether `entry` is one of the playing playlist's own.
fn run_own(w: &Worker, entry: u64) -> bool {
    w.recs.plus.run.as_ref().is_some_and(|r| r.own.contains(&entry))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_playlist_is_named_by_the_context() {
        let id = PlaylistId::new();
        assert_eq!(playlist_of(&format!("playlist:{id}")), Some(id));
        assert_eq!(playlist_of(&format!("playlist:{}", nikgob_view::LIKED_PLAYLIST)), Some(nikgob_view::LIKED_PLAYLIST));
        assert_eq!(playlist_of("artist:1"), None);
    }
}
