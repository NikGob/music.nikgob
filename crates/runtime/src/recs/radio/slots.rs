use super::*;

/// The library songs an ordinary radio's new finds are found around: a
/// track radio's track, a list's first songs (a mix's are not of one).
pub(super) fn ordinary_seeds(seed: &RadioSeed) -> Vec<TrackId> {
    match seed {
        RadioSeed::Track(t) => vec![*t],
        RadioSeed::Tracks(ts) => ts.iter().take(nikgob_recs::novelty::SEEDS_KEPT).copied().collect(),
        _ => Vec::new(),
    }
}

/// Every entry of the queue in play order (played, current, upcoming).
pub(super) fn queue_order(q: &crate::queue::Queue) -> Vec<Entry> {
    let mut out = q.history().to_vec();
    out.extend(q.current());
    out.extend_from_slice(q.upcoming());
    out
}

/// The place of the entry `misfit` of the upcoming `up`: after the entry
/// before it, else before the one after it (none: the end, where picks go
/// anyway).
pub(super) fn misfit_slot(up: &[Entry], misfit: u64) -> Option<(u64, bool)> {
    let at = up.iter().position(|e| e.id == misfit)?;
    match at.checked_sub(1) {
        Some(before) => Some((up[before].id, true)),
        None => up.get(at + 1).map(|e| (e.id, false)),
    }
}

/// `gone` (a misfit) leaves the queue with `own` its place: the places
/// anchored on it move to its own, then its own joins them.
pub(super) fn reanchor(slots: &mut Vec<(u64, bool)>, gone: u64, own: Option<(u64, bool)>) {
    match own {
        Some(o) => {
            for s in slots.iter_mut().filter(|s| s.0 == gone) {
                *s = o;
            }
            slots.push(o);
        }
        None => slots.retain(|s| s.0 != gone),
    }
    let over = slots.len().saturating_sub(SLOTS_MAX);
    slots.drain(..over);
}

/// The picks `entries` (in their order) take the `slots` in the queue's
/// order, one each, while their anchors are still upcoming; the places
/// left without a pick come back.
pub(super) fn into_slots(queue: &mut crate::queue::Queue, entries: &[u64], mut slots: Vec<(u64, bool)>) -> Vec<(u64, bool)> {
    let at = |q: &crate::queue::Queue, s: &(u64, bool)| q.upcoming().iter().position(|e| e.id == s.0).map(|k| 2 * k + usize::from(s.1));
    slots.retain(|s| at(queue, s).is_some());
    slots.sort_by_key(|s| at(queue, s));
    let left = slots.split_off(entries.len().min(slots.len()));
    for (&entry, slot) in entries.iter().zip(slots) {
        into_slot(queue, entry, slot);
    }
    left
}

/// Moves `entry` into `slot` ([`misfit_slot`]) while its anchor is still
/// upcoming.
pub(super) fn into_slot(queue: &mut crate::queue::Queue, entry: u64, (anchor, after): (u64, bool)) -> bool {
    queue.upcoming().iter().any(|e| e.id == anchor) && queue.move_next_to(entry, anchor, after)
}

/// The places in `order` of the entries `of` and their tracks, as
/// `session.json` keeps them (entry ids are a run's own).
pub(crate) fn places_of(order: &[Entry], of: &HashSet<u64>) -> Vec<(usize, TrackId)> {
    order.iter().enumerate().filter(|(_, e)| of.contains(&e.id)).map(|(k, e)| (k, e.track)).collect()
}

/// The entries of `order` at `places` that still hold their tracks (a
/// track that moved is the user's now).
pub(crate) fn entries_at(order: &[Entry], places: &[(usize, TrackId)]) -> HashSet<u64> {
    places.iter().filter_map(|(k, t)| order.get(*k).filter(|e| e.track == *t).map(|e| e.id)).collect()
}

/// The seed of a list's radio: the catalog's tracks of a library list
/// ([`RadioSeed::Tracks`]); a service's list the catalog lacks songs of,
/// blended whole as a service song's seed ([`RadioSeed::Stream`] under its
/// first song): the mean sound of its analysed songs and every library
/// artist its songs match. None: nothing to grow from.
pub(super) fn list_seed(
    ids: &[TrackId],
    streams: &[StreamTrack],
    in_catalog: impl Fn(TrackId) -> bool,
    vector: impl Fn(TrackId) -> Option<nikgob_recs::Vector>,
    artist: impl Fn(&str) -> Option<ArtistId>,
) -> Option<RadioSeed> {
    if streams.is_empty() || ids.iter().all(|t| in_catalog(*t)) {
        let known: Vec<TrackId> = ids.iter().copied().filter(|t| in_catalog(*t)).collect();
        return (!known.is_empty()).then_some(RadioSeed::Tracks(known));
    }
    let first = streams.first()?;
    let vs: Vec<nikgob_recs::Vector> = ids.iter().filter_map(|t| vector(*t)).collect();
    let vector = (!vs.is_empty()).then(|| std::array::from_fn(|k| vs.iter().map(|v| v[k]).sum::<f32>() / vs.len() as f32));
    let mut artists: Vec<ArtistId> = Vec::new();
    for a in streams.iter().flat_map(|t| &t.artists).filter_map(|a| artist(&a.name)) {
        if !artists.contains(&a) {
            artists.push(a);
        }
    }
    Some(RadioSeed::Stream { id: first.id.clone(), vector, artists })
}
