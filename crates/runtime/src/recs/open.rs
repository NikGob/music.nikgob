//! The open pool of the waves (docs/recommendations.md §1, §7 phase 2):
//! what the coming «Моя волна» draws from beyond the library, and the call
//! it makes for its next songs ([`Worker::wave_next`] over
//! [`nikgob_recs::open_world::next`]).
//!
//! The pool ([`open_pool`]): the discoveries the last pass kept (the open
//! world's [`super::discover::DISCOVER_POOL`]), the songs the graph of
//! similar music found beside the library's, and what the niches' top-ups
//! found ([`super::niche`]), each once, new indeed, with its preview, its
//! kind of song and the library's songs it is linked to. Everything is in
//! memory: no request, no read of the library.

use std::collections::{HashMap, HashSet};

use nikgob_core::streaming::{RemoteId, StreamTrack};
use nikgob_recs::open_world::{OpenCandidate, WaveContext, WaveFilter, WaveNext};
use nikgob_recs::{NewLog, stream_song_key};

use super::Snapshot;
use super::niche::NicheFound;
use crate::Worker;

/// The open pool of `snap` (see the module docs): the discoveries `pool`
/// with their `sources`, the graph's songs, the niches' `topped` songs.
pub(crate) fn open_pool(snap: &Snapshot, pool: &[StreamTrack], sources: &HashMap<String, nikgob_recs::Source>, topped: &[NicheFound]) -> Vec<OpenCandidate> {
    let mut cands = super::lanes::candidates(snap, pool, sources, &NewLog::default(), 1);
    for f in topped {
        cands.extend(super::lanes::niche_candidates(snap, f));
    }
    let mut seen: HashSet<String> = HashSet::new();
    cands
        .into_iter()
        .filter_map(|c| {
            let key = stream_song_key(&c.track);
            seen.insert(key.clone()).then(|| OpenCandidate { key, track: c.track, vector: c.vector, lane: c.lane, near: c.near, weight: c.weight })
        })
        .collect()
}

impl Worker {
    /// The next `n` of a wave of `filter` after the session of the moment,
    /// none of `served` (track ids and [`stream_song_key`]s), with the new
    /// songs' tracks by their remote ids (they play from the services):
    /// what «Моя волна» calls for its next songs. Without a snapshot,
    /// nothing. Under «Только скачанное» the pool is empty and the library
    /// plays alone.
    #[allow(dead_code)]
    pub(crate) fn wave_next(&self, filter: &WaveFilter, n: usize, served: &HashSet<String>) -> Option<(WaveNext, HashMap<RemoteId, StreamTrack>)> {
        let snap = self.snapshot()?;
        let topped: Vec<NicheFound> = match self.recs.home.as_ref() {
            Some(h) if !snap.local_only => h.lanes.iter().filter(|l| !l.niche.is_empty()).filter_map(|l| super::niche::cached(&self.lib, l.key)).collect(),
            _ => Vec::new(),
        };
        let pool = if snap.local_only { Vec::new() } else { open_pool(&snap, &self.discoveries_all(), &self.discovery_sources(), &topped) };
        let ok = |i: usize| {
            let t = &snap.catalog.tracks()[i];
            (t.local || !snap.local_only) && !snap.model.taste.blocks(t)
        };
        let background = |i: usize| snap.background.get(i).copied().unwrap_or(false);
        let artist_tracks = |t: &StreamTrack| super::lanes::artist_tracks(&snap, t);
        let ctx = WaveContext { model: &snap.model, ok: &ok, genre: None, background: &background, artist_tracks: &artist_tracks };
        let next = nikgob_recs::open_world::next(&ctx, &pool, filter, &self.recs.session, n, served);
        let streams = pool.into_iter().map(|c| (c.track.id.clone(), c.track)).collect();
        Some((next, streams))
    }
}
