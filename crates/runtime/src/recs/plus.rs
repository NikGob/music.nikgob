//! «✦ С дополнением» in the queue: while a
//! playlist plays with its chip on, a guest of the open pool joins it
//! after every three of its own tracks ([`nikgob_recs::plus`]).
//!
//! * A playlist's queue knows its own entries from the moment it is made
//!   ([`Worker::plus_queued`]): tracks queued by hand or by autoplay later
//!   are not the playlist's, whatever they are. What each entry is (own,
//!   guest) survives a restart (`session.json`) and «Очистить» → «Отменить»,
//!   by its place in the queue.
//! * The guests are ranked on a thread when the chip is on and again when
//!   the snapshot or the discoveries change; a guest takes its place only
//!   when that place comes within the live buffer
//!   ([`nikgob_recs::plus::AHEAD`]), one ahead at a most. A place that came
//!   and went without a guest (none ranked yet) is owed: the guest comes
//!   right after the next own track. Shuffled away from its place, a guest
//!   ahead goes back to the queue's pool of guests and the place gets one
//!   again; one the user dragged stays where it was put (and no other comes
//!   while it is ahead). Repeat one plays no guests; under repeat all a
//!   guest that played is not played again on the next pass.
//! * A guest plays from its service like any new song (never downloaded on
//!   its own). ♡ or adding it to a playlist keeps it: it stays where it is
//!   (still a guest for the count). A skip of a guest («Дальше» at any
//!   point of it) takes every guest not kept out of the queue at once and no
//!   other comes while this queue plays (the chip stays on: the next play
//!   of the playlist, or the chip switched off and on, brings them back,
//!   never of the skipped guest's artist in this queue); turning the chip
//!   off takes them out too, those that played included. The playlist's
//!   own entries are never touched, nor the playlist itself.
//! * A guest that cannot play (no network, no service) is passed over in
//!   silence and no other comes while this queue plays.
//! * The chip of each playlist is remembered in the prefs
//!   ([`nikgob_view::Prefs::plus_playlists`]).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use nikgob_core::model::{PlaylistId, TrackId};
use nikgob_recs::open_world::WaveContext;
use nikgob_recs::plus::{Guest, Slot};
use nikgob_view::Repeat;
use serde::{Deserialize, Serialize};

use super::Msg;
use super::niche::NicheFound;
use crate::Worker;
use crate::import::Internal;
use crate::queue::Entry;

/// What an entry of the playlist's queue is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Tag {
    /// One of the playlist's own.
    Own,
    /// A guest: its song key and artist key; `kept` (♡, added to a
    /// playlist: it stays when the guests go), `placed` (dragged by the
    /// user: it stays where it was put).
    Guest { key: String, artist: String, kept: bool, placed: bool },
}

/// The playlist the guests join.
pub(crate) struct PlusRun {
    id: u64,
    playlist: PlaylistId,
    /// The queue it plays in.
    queue_gen: u64,
    /// The entry the count starts at: the one playing when the guests
    /// began (`None` while the chip is off).
    from: Option<u64>,
    /// The entries that are the playlist's own or its guests.
    tags: HashMap<u64, Tag>,
    /// Its tracks in its order (the seeds of the ranking).
    tracks: Vec<TrackId>,
    /// The guests that came (song keys).
    served: HashSet<String>,
    /// The guests ranked, and what for: the snapshot and the discoveries.
    list: Vec<Guest>,
    ranked_for: Option<(usize, usize)>,
    ranking: bool,
    /// A guest was skipped or could not play: no more in this queue.
    stopped: bool,
}

/// What `session.json` keeps of the playlist's queue (and «Отменить» of
/// «Очистить»): each tagged entry by its place in play order and its track.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct PlusSave {
    playlist: PlaylistId,
    tags: Vec<(usize, TrackId, Tag)>,
    /// The place the count starts at.
    #[serde(default)]
    from: Option<usize>,
    #[serde(default)]
    served: Vec<String>,
    #[serde(default)]
    stopped: bool,
    #[serde(default)]
    away: Vec<String>,
}

#[derive(Default)]
pub(crate) struct PlusState {
    run: Option<PlusRun>,
    seq: u64,
    /// The artists of the guests skipped in the queue of that generation.
    away: (u64, HashSet<String>),
    /// What the queue was before «Очистить» (for «Отменить»).
    cleared: Option<PlusSave>,
}

pub(crate) enum PlusMsg {
    Ranked { id: u64, guests: Vec<Guest>, ranked_for: (usize, usize) },
}

impl PlusState {
    /// The guest entries in the queue.
    pub(crate) fn guests(&self) -> HashSet<u64> {
        self.run
            .as_ref()
            .map(|r| r.tags.iter().filter(|(_, t)| matches!(t, Tag::Guest { .. })).map(|(e, _)| *e).collect())
            .unwrap_or_default()
    }
}

/// Every entry of `q` in play order (played, current, upcoming) and the
/// place of the current one.
fn queue_order(q: &crate::queue::Queue) -> (Vec<Entry>, Option<usize>) {
    let mut out = q.history().to_vec();
    let at = q.current().map(|_| out.len());
    out.extend(q.current());
    out.extend_from_slice(q.upcoming());
    (out, at)
}

/// What the cadence counts an entry as.
fn slot_of(tags: &HashMap<u64, Tag>, entry: u64) -> Slot {
    match tags.get(&entry) {
        Some(Tag::Own) => Slot::Own,
        Some(Tag::Guest { .. }) => Slot::Guest,
        None => Slot::Other,
    }
}

impl Worker {
    /// A playlist's queue was just made: its entries are its own (whether
    /// the chip is on or not: it may be turned on while it plays).
    pub(crate) fn plus_queued(&mut self, playlist: PlaylistId) {
        let (order, _) = queue_order(&self.queue);
        let (tracks, _) = self.playlist_tracks(playlist, None);
        self.recs.plus.seq += 1;
        self.recs.plus.run = Some(PlusRun {
            id: self.recs.plus.seq,
            playlist,
            queue_gen: self.queue.generation(),
            from: None,
            tags: order.iter().map(|e| (e.id, Tag::Own)).collect(),
            tracks,
            served: HashSet::new(),
            list: Vec::new(),
            ranked_for: None,
            ranking: false,
            stopped: false,
        });
        self.recs.plus.away = (self.queue.generation(), HashSet::new());
        self.publish_player();
    }

    /// The run still plays its queue (any other queue ended it).
    fn plus_alive(&mut self) -> bool {
        let generation = self.queue.generation();
        if self.recs.plus.run.as_ref().is_some_and(|r| r.queue_gen != generation) {
            self.recs.plus.run = None;
        }
        self.recs.plus.run.is_some()
    }

    /// Before the player is published: with the chip on, the places that
    /// came near get their guests.
    pub(crate) fn plus_top_up(&mut self) {
        if !self.plus_alive() {
            return;
        }
        let current = self.queue.current().map(|e| e.id);
        let Some(run) = self.recs.plus.run.as_mut() else { return };
        let on = self.state.read().prefs.plus_playlists.contains(&run.playlist);
        if !on {
            return;
        }
        if run.from.is_none() {
            run.from = current;
            if current.is_some() {
                log::info!("plus: guests join the playlist {}", run.playlist);
            }
        }
        // «Только скачанное» of the recommendations: nothing from the
        // services. Repeat one plays one song.
        let local_only = self.state.read().prefs.mixes_downloaded_only;
        if local_only || run.stopped || self.queue.repeat == Repeat::One {
            return;
        }
        self.plus_rank();
        self.plus_fill();
    }

    /// The guests are ranked again when the snapshot or the discoveries
    /// changed since (on the picks' thread, [`super::picker`]).
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
        self.new_log_load();
        let log = self.recs.newlog.log.clone();
        let tx = self.recs.tx.clone();
        let spawned = self.recs.picker.send(super::picker::Lane::Plus, move || {
            let mut pool = super::open::open_pool(&snap, &discoveries, &sources, &topped, &log, &|_| None);
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
            let ctx = WaveContext { model: &snap.model, ok: &ok, genre: None, background: &background, artist_tracks: &artist_tracks, index: Some(&snap.index), learned: None };
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

    /// Takes the guest entries `entries` out of the queue (never the one
    /// playing); `unserve`: their songs may come again.
    fn plus_take_out(&mut self, entries: &[u64], unserve: bool) {
        let next = self.queue.upcoming().first().map(|e| e.id);
        let mut gone = 0;
        for &id in entries {
            if self.queue.current().is_some_and(|c| c.id == id) || !self.queue.remove(id) {
                continue;
            }
            gone += 1;
            self.recs.why.remove(&id);
            if let Some(run) = self.recs.plus.run.as_mut()
                && let Some(Tag::Guest { key, .. }) = run.tags.remove(&id)
                && unserve
            {
                run.served.remove(&key);
            }
        }
        if gone > 0 {
            log::info!("plus: {gone} guests left the queue");
        }
        if next.is_some_and(|n| entries.contains(&n)) {
            self.preload_next();
        }
    }

    /// The guests not kept, `played` ones too or only those ahead.
    fn plus_unkept(&self, played: bool) -> Vec<u64> {
        let Some(run) = self.recs.plus.run.as_ref() else { return Vec::new() };
        let ahead: HashSet<u64> = self.queue.upcoming().iter().map(|e| e.id).collect();
        run.tags
            .iter()
            .filter(|(e, t)| matches!(t, Tag::Guest { kept: false, .. }) && (played || ahead.contains(e)))
            .map(|(e, _)| *e)
            .collect()
    }

    /// The places due a guest (see [`nikgob_recs::plus::due`]) get one: a
    /// guest ahead not at its place goes back, one at a time ahead.
    fn plus_fill(&mut self) {
        let Some(snap) = self.snapshot() else { return };
        // Under repeat all, the guests that played would come again on the
        // next pass: they leave before it.
        if self.queue.repeat == Repeat::All && self.queue.upcoming().len() < nikgob_recs::plus::AHEAD {
            let ahead: HashSet<u64> = self.queue.upcoming().iter().map(|e| e.id).collect();
            let played: Vec<u64> = self.plus_unkept(true).into_iter().filter(|e| !ahead.contains(e)).collect();
            if !played.is_empty() {
                self.plus_take_out(&played, false);
            }
        }
        let (order, Some(at)) = queue_order(&self.queue) else { return };
        let Some(run) = self.recs.plus.run.as_ref() else { return };
        let start = order.iter().position(|e| Some(e.id) == run.from).filter(|&s| s <= at).unwrap_or(at);
        // The places to come: on to the end, then (repeat all) the start.
        let mut line: Vec<Entry> = order[start..].to_vec();
        let end = line.len();
        if self.queue.repeat == Repeat::All {
            line.extend_from_slice(&order[..at]);
        }
        let cur = at - start;
        // The guests not kept ahead (before the end: the played ones the
        // next pass would bring go above).
        let ahead: Vec<(usize, Entry, bool)> = line[..end]
            .iter()
            .enumerate()
            .skip(cur + 1)
            .filter_map(|(i, e)| match run.tags.get(&e.id) {
                Some(Tag::Guest { kept: false, placed, .. }) => Some((i, *e, *placed)),
                _ => None,
            })
            .collect();
        // One the user put there stays, and none other comes meanwhile.
        if ahead.iter().any(|g| g.2) {
            return;
        }
        let waiting = ahead.first().map(|g| g.1.id);
        let slots: Vec<Slot> = line.iter().map(|e| if Some(e.id) == waiting { Slot::Other } else { slot_of(&run.tags, e.id) }).collect();
        let due = nikgob_recs::plus::due(&slots, cur);
        if let Some(w) = waiting {
            // At its place: right after the first place due.
            if due.first().is_some_and(|&i| line.get(i + 1).map(|e| e.id) == Some(w)) {
                return;
            }
            // Shuffled away from it: back to the guests, and the place
            // gets one again.
            let gone: Vec<u64> = ahead.iter().map(|g| g.1.id).collect();
            self.plus_take_out(&gone, true);
            return self.plus_fill();
        }
        let Some(&i) = due.first() else { return };
        if run.list.is_empty() {
            return;
        }
        let anchor = line[i];
        // The playlist's songs just before the place, the nearest last.
        let mut before: Vec<usize> = line[..=i]
            .iter()
            .rev()
            .filter(|e| run.tags.get(&e.id) == Some(&Tag::Own))
            .take(nikgob_recs::plus::EVERY)
            .filter_map(|e| snap.model.position(e.track))
            .collect();
        before.reverse();
        let Some(k) = nikgob_recs::plus::pick(&snap.model, &run.list, &before, &run.served, &self.recs.plus.away.1) else {
            return;
        };
        let guest = run.list[k].clone();
        self.streaming.learn(std::slice::from_ref(&guest.track));
        let Some(tid) = self.upsert_streams(std::slice::from_ref(&guest.track)).first().copied() else { return };
        let Some(entry) = self.queue.enqueue(&[tid]).first().copied() else { return };
        self.queue.move_next_to(entry.id, anchor.id, true);
        if let Some(why) = self.discovery_why(&guest.track) {
            self.recs.why.insert(entry.id, why);
        }
        if let Some(run) = self.recs.plus.run.as_mut() {
            run.served.insert(guest.key.clone());
            run.tags.insert(entry.id, Tag::Guest { key: guest.key, artist: guest.artist, kept: false, placed: false });
        }
        log::info!("plus: a guest «{}» joins after place {}", guest.track.title, i + 1);
        if i == cur {
            self.preload_next();
        }
    }

    /// A track ended: a guest skipped («Дальше», at any point of it) takes
    /// the guests not kept with it and no other comes while this queue
    /// plays; its artist none at all in it.
    pub(crate) fn plus_ended(&mut self, entry: u64, skipped: bool) {
        if !skipped {
            return;
        }
        let Some(run) = self.recs.plus.run.as_mut() else { return };
        let Some(Tag::Guest { artist, .. }) = run.tags.get(&entry).cloned() else { return };
        log::info!("plus: a guest skipped, no more in this queue");
        run.stopped = true;
        if !artist.is_empty() {
            self.recs.plus.away.1.insert(artist);
        }
        let gone = self.plus_unkept(false);
        self.plus_take_out(&gone, false);
    }

    /// A guest could not play: it is passed over without a word (no
    /// network, no service), the guests ahead leave and no more come while
    /// this queue plays. Whether `entry` was a guest.
    pub(crate) fn plus_failed(&mut self, entry: u64) -> bool {
        let Some(run) = self.recs.plus.run.as_mut() else { return false };
        if !matches!(run.tags.get(&entry), Some(Tag::Guest { .. })) {
            return false;
        }
        log::info!("plus: a guest cannot play, no more in this queue");
        run.stopped = true;
        let gone = self.plus_unkept(false);
        self.plus_take_out(&gone, false);
        true
    }

    /// ♡ or «+» on a guest: it stays where it is when the guests go (still
    /// a guest for the count).
    pub(crate) fn plus_kept(&mut self, track: TrackId) {
        let (order, _) = queue_order(&self.queue);
        let Some(run) = self.recs.plus.run.as_mut() else { return };
        for e in order.iter().filter(|e| e.track == track) {
            if let Some(Tag::Guest { kept, .. }) = run.tags.get_mut(&e.id) {
                *kept = true;
            }
        }
    }

    /// The user dragged `entry`: a guest stays where it was put.
    pub(crate) fn plus_moved(&mut self, entry: u64) {
        if let Some(Tag::Guest { placed, .. }) = self.recs.plus.run.as_mut().and_then(|r| r.tags.get_mut(&entry)) {
            *placed = true;
        }
    }

    /// The chip of `playlist` was switched: off, the guests not kept leave
    /// the queue at once (those that played too: repeat would bring them
    /// back); on, they begin with the track playing if the playlist plays.
    pub(crate) fn plus_switched(&mut self, playlist: PlaylistId, on: bool) {
        if self.plus_alive() && self.recs.plus.run.as_ref().is_some_and(|r| r.playlist == playlist) {
            if on {
                if let Some(run) = self.recs.plus.run.as_mut() {
                    run.stopped = false;
                }
            } else {
                let gone = self.plus_unkept(true);
                self.plus_take_out(&gone, false);
                if let Some(run) = self.recs.plus.run.as_mut() {
                    run.from = None;
                }
            }
        }
        self.publish_player();
    }

    /// What each entry of the playlist's queue is, by its place in play
    /// order (for `session.json` and «Отменить» of «Очистить»).
    pub(crate) fn plus_save(&mut self) -> Option<PlusSave> {
        if !self.plus_alive() {
            return None;
        }
        let run = self.recs.plus.run.as_ref()?;
        let (order, _) = queue_order(&self.queue);
        let tags = order.iter().enumerate().filter_map(|(k, e)| Some((k, e.track, run.tags.get(&e.id)?.clone()))).collect();
        let from = run.from.and_then(|f| order.iter().position(|e| e.id == f));
        Some(PlusSave {
            playlist: run.playlist,
            tags,
            from,
            served: run.served.iter().cloned().collect(),
            stopped: run.stopped,
            away: self.recs.plus.away.1.iter().cloned().collect(),
        })
    }

    /// The playlist's queue back (a restart, «Отменить» of «Очистить»): its
    /// entries tagged again by their places.
    pub(crate) fn plus_restore(&mut self, save: PlusSave) {
        let (order, _) = queue_order(&self.queue);
        let tags: HashMap<u64, Tag> = save
            .tags
            .into_iter()
            .filter_map(|(k, track, tag)| order.get(k).filter(|e| e.track == track).map(|e| (e.id, tag)))
            .collect();
        let from = save.from.and_then(|k| order.get(k)).map(|e| e.id);
        let keep = self.recs.plus.run.take().filter(|r| r.playlist == save.playlist && r.queue_gen == self.queue.generation());
        let (tracks, list, ranked_for) = match keep {
            Some(r) => (r.tracks, r.list, r.ranked_for),
            None => (self.playlist_tracks(save.playlist, None).0, Vec::new(), None),
        };
        self.recs.plus.seq += 1;
        self.recs.plus.run = Some(PlusRun {
            id: self.recs.plus.seq,
            playlist: save.playlist,
            queue_gen: self.queue.generation(),
            from,
            tags,
            tracks,
            served: save.served.into_iter().collect(),
            list,
            ranked_for,
            ranking: false,
            stopped: save.stopped,
        });
        self.recs.plus.away = (self.queue.generation(), save.away.into_iter().collect());
    }

    /// «Очистить»: what the queue was, for «Отменить».
    pub(crate) fn plus_cleared(&mut self) {
        self.recs.plus.cleared = self.plus_save();
    }

    /// «Отменить» of «Очистить»: the entries back are tagged again.
    pub(crate) fn plus_uncleared(&mut self) {
        if let Some(save) = self.recs.plus.cleared.take() {
            self.plus_restore(save);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A save reads back, and an old file without it reads as none.
    #[test]
    fn a_save_reads_back() {
        let save = PlusSave {
            playlist: PlaylistId::new(),
            tags: vec![(0, TrackId::new(), Tag::Own), (3, TrackId::new(), Tag::Guest { key: "a".into(), artist: "b".into(), kept: true, placed: false })],
            from: Some(0),
            served: vec!["a".into()],
            stopped: false,
            away: Vec::new(),
        };
        let json = serde_json::to_string(&save).unwrap();
        assert_eq!(serde_json::from_str::<PlusSave>(&json).unwrap(), save);
    }
}
