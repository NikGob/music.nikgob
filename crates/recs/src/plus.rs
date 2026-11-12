//! «✦ С дополнением» (docs/recommendations.md §4): a playlist of the
//! user's plays as it is, and after every [`EVERY`] of its own tracks one
//! song of the open pool joins it as a guest.
//!
//! * **Who may come** ([`guests`]): the songs of the open pool
//!   ([`crate::open_world::OpenCandidate`]) the playlist's own songs reach
//!   by the graph (the walk from them, or a song whose list holds it) and
//!   that sound like the playlist: by the analysed preview, within the
//!   playlist's sound ([`crate::open_world::NicheSound`]) and near one of
//!   its songs; without a preview, of a kind of song the playlist has and
//!   linked to it. A song reached from songs of several artists stands
//!   above a lone answer ([`crate::discover::multi_seed`]). When the graph
//!   knows nothing of the pool, songs near the playlist's sound alone come
//!   after all the linked ones.
//! * **Who comes next** ([`pick`]): of those, the best that has not come
//!   yet and is not of an artist turned away this time, closest to the
//!   playlist's songs just before its place.
//! * **Where** ([`due`]): after every [`EVERY`] own tracks in play order,
//!   only a few places ahead of the track playing ([`AHEAD`]), never all
//!   at once.

use std::collections::{HashMap, HashSet};

use nikgob_core::model::ArtistId;
use nikgob_core::streaming::StreamTrack;

use crate::analysis::Vector;
use crate::index::euclid;
use crate::model::UserModel;
use crate::open_world::{NicheSound, OpenCandidate, WaveContext};

/// One guest after this many own tracks.
pub const EVERY: usize = 3;
/// A guest's place is filled once it is this many places ahead of the
/// track playing or nearer (the queue's live buffer).
pub const AHEAD: usize = 3;
/// The playlist's songs the walk starts from, spread over the playlist.
const SEEDS: usize = 64;
/// The guests kept to choose from, and how many of one artist.
pub const KEPT: usize = 48;
const PER_ARTIST: usize = 2;
/// A preview this near one of the playlist's songs (in the library's
/// median distances) sounds like it.
const NEAR_REACH: f32 = 1.0;
/// The sound term of a song without a preview.
const NO_PREVIEW: f32 = 0.35;

/// A song that may join the playlist.
#[derive(Clone, Debug)]
pub struct Guest {
    pub track: StreamTrack,
    /// [`crate::stream_song_key`].
    pub key: String,
    /// Its main artist ([`crate::artist_key`]).
    pub artist: String,
    /// The library's songs the graph or the radio that found it link it to.
    pub near: Vec<usize>,
    /// Its preview, embedded.
    pub embedded: Option<Vector>,
    /// Reached by the graph from the playlist (else by its sound alone).
    pub linked: bool,
    pub score: f32,
}

/// The guests of the playlist of `playlist` (catalog positions of its
/// songs) from `pool`, the best first, [`KEPT`] at most (see the module
/// docs).
pub fn guests(ctx: &WaveContext<'_>, pool: &[OpenCandidate], playlist: &[usize]) -> Vec<Guest> {
    let m = ctx.model;
    if playlist.is_empty() || pool.is_empty() {
        return Vec::new();
    }
    let step = playlist.len().div_ceil(SEEDS).max(1);
    let sample: Vec<usize> = playlist.iter().step_by(step).copied().collect();
    let weight = 1.0 / (sample.len() as f32).sqrt();
    let seeds: Vec<(usize, f32)> = sample.iter().map(|&i| (i, weight)).collect();
    let walk = m.graph.walk(&seeds);
    let own: HashSet<usize> = playlist.iter().copied().collect();
    let sound = NicheSound::of(m, &sample, false);
    let scale = m.scale().max(1e-6);
    let own_sound: Vec<&Vector> = sample.iter().filter_map(|&i| m.embedded_at(i)).collect();
    let mut out: Vec<Guest> = Vec::new();
    for c in pool {
        let artist_tracks = (ctx.artist_tracks)(&c.track);
        let energies: Vec<f32> = artist_tracks.iter().filter_map(|&i| m.energy_at(i)).collect();
        if !sound.fits(m, c.vector.as_ref(), c.lane, &c.near, &energies) {
            continue;
        }
        let direct = c.near.iter().filter(|i| own.contains(i)).count() as f32;
        let link = (c.near.iter().map(|i| walk.get(i).copied().unwrap_or(0.0)).sum::<f32>() + 0.5 * direct).min(2.0);
        let embedded = c.vector.as_ref().map(|v| m.embed(v));
        // How near it sounds to the playlist's nearest song.
        let near_sound = match &embedded {
            Some(e) => {
                let d = own_sound.iter().map(|o| euclid(&e[..], &o[..])).fold(f32::INFINITY, f32::min);
                if own_sound.is_empty() {
                    Some(NO_PREVIEW)
                } else if d <= NEAR_REACH * scale {
                    Some((-d / scale).exp())
                } else {
                    None
                }
            }
            None => Some(NO_PREVIEW),
        };
        let Some(near_sound) = near_sound else { continue };
        let linked = link > 0.0;
        if !linked && embedded.is_none() {
            // Nothing says it belongs here.
            continue;
        }
        let independent: HashSet<Option<ArtistId>> = c.near.iter().filter(|i| own.contains(i) || walk.contains_key(i)).map(|&i| m.main_artist_at(i)).collect();
        let score = (c.weight + link) * crate::discover::multi_seed(independent.len()) * (0.5 + near_sound);
        let artist = c.track.artists.first().map_or_else(String::new, |a| crate::artist_key(&a.name));
        out.push(Guest { track: c.track.clone(), key: c.key.clone(), artist, near: c.near.clone(), embedded, linked, score });
    }
    out.sort_by(|a, b| b.linked.cmp(&a.linked).then(b.score.total_cmp(&a.score)).then(a.key.cmp(&b.key)));
    let mut per: HashMap<String, usize> = HashMap::new();
    out.retain(|g| {
        let n = per.entry(g.artist.clone()).or_default();
        *n += 1;
        *n <= PER_ARTIST
    });
    out.truncate(KEPT);
    out
}

/// The guest of `guests` to come next after the playlist's songs `before`
/// (catalog positions, the nearest last): the best linked one before any
/// that only sounds alike, by its score and how near it is to them; none
/// of `served` ([`Guest::key`]s) nor of the artists in `away`.
pub fn pick(model: &UserModel, guests: &[Guest], before: &[usize], served: &HashSet<String>, away: &HashSet<String>) -> Option<usize> {
    let scale = model.scale().max(1e-6);
    let around: Vec<&Vector> = before.iter().filter_map(|&i| model.embedded_at(i)).collect();
    let mut best: Option<(bool, f32, usize)> = None;
    for (k, g) in guests.iter().enumerate() {
        if served.contains(&g.key) || (!g.artist.is_empty() && away.contains(&g.artist)) {
            continue;
        }
        let linked_here = g.near.iter().any(|i| before.contains(i));
        let sounds_here = match &g.embedded {
            Some(e) => around.iter().map(|a| (-euclid(&e[..], &a[..]) / scale).exp()).fold(0.0f32, f32::max),
            None => 0.0,
        };
        let here = if linked_here { 1.0 } else { sounds_here };
        let value = g.score * (1.0 + 0.5 * here);
        if best.is_none_or(|(l, v, _)| (g.linked, value) > (l, v)) {
            best = Some((g.linked, value, k));
        }
    }
    best.map(|b| b.2)
}

/// What an entry of the queue is to the playlist playing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    /// One of the playlist's own tracks.
    Own,
    /// A guest.
    Guest,
    /// Anything else (a track the user queued, autoplay's).
    Other,
}

/// Where guests are due in `order` (the queue in play order from where the
/// guests began; `current` its place of the track playing): the places
/// after which a guest is missing — every [`EVERY`] own tracks counted
/// since the last guest — no further than [`AHEAD`] places ahead of the
/// track playing, the nearest first. A place right after the track playing
/// counts too, and a place already behind it that got no guest (none was
/// ranked in time) is owed: the guest comes right after the next own track
/// that is not behind.
pub fn due(order: &[Slot], current: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut own = 0usize;
    for (i, s) in order.iter().enumerate() {
        match s {
            Slot::Guest => own = 0,
            Slot::Other => {}
            Slot::Own => {
                own += 1;
                let next_is_guest = order.get(i + 1) == Some(&Slot::Guest);
                // Behind the track playing a missing guest is owed (the
                // count goes on); from it on the place is filled.
                if own >= EVERY && !next_is_guest && i >= current {
                    if i < current + AHEAD {
                        out.push(i);
                    }
                    own = 0;
                }
            }
        }
        if i >= current + AHEAD {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::{open_pool, synthetic};

    /// One guest after every three own tracks, filled only a few places
    /// ahead; a guest there already counts; tracks the user queued do not.
    #[test]
    fn one_guest_after_every_three_own() {
        use Slot::{Guest as G, Other as O, Own as P};
        let list = vec![P; 12];
        // Playing the first: the place after the third is two ahead.
        assert_eq!(due(&list, 0), vec![2]);
        // Playing the third: the place right after it.
        assert_eq!(due(&list, 2), vec![2]);
        // Its guest came: nothing more until the sixth is near.
        let with = [P, P, P, G, P, P, P, P];
        assert_eq!(due(&with, 1), Vec::<usize>::new());
        assert_eq!(due(&with, 4), vec![6]);
        // A track the user queued is not counted (it takes a place).
        assert_eq!(due(&[P, O, P, P, P, P], 2), vec![3]);
        assert_eq!(due(&[P, O, P, P, P, P], 0), Vec::<usize>::new());
        // Never more than the live buffer ahead.
        assert_eq!(due(&[P, P, P, G, P, P, P, G, P, P, P, P], 9), vec![10]);
        assert!(due(&list, 3).iter().all(|&i| i < 3 + AHEAD));
        // A place behind that got no guest (ranked too late): owed, right
        // after the track playing.
        assert_eq!(due(&list, 3), vec![3]);
        assert_eq!(due(&list, 6), vec![6]);
        // A skipped guest is a guest that came: the count goes on from it.
        assert_eq!(due(&[P, P, P, G, P, P], 3), Vec::<usize>::new());
    }

    /// A playlist of the three calm songs of a rap listener has calm songs
    /// as guests — the new calm songs its songs' lists hold — never the rap
    /// on those lists nor its rapper's songs, never a rap song; a playlist
    /// of rap has rap.
    #[test]
    fn the_guests_are_of_the_playlist() {
        let d = synthetic::user(synthetic::Kind::NicheCalm, 5);
        let p = d.parts();
        let last = p.input.events.iter().map(|e| e.ts).max().unwrap();
        let model = p.model_at(last + 1);
        let pool = open_pool(&p, &model);
        let title = |g: &Guest| g.track.title.clone();
        let calm: Vec<usize> = d.new.iter().filter(|n| n.track.title.starts_with("Drift")).flat_map(|n| n.near.iter().filter_map(|t| model.position(*t))).collect::<HashSet<_>>().into_iter().collect();
        assert_eq!(calm.len(), 3);
        let ok = |_: usize| true;
        let no = |_: usize| false;
        // The library's songs of a new song's artist, as the runtime tells.
        let artist_of: HashMap<String, ArtistId> = d.new.iter().filter_map(|n| Some((crate::stream_song_key(&n.track), n.artist?))).collect();
        let artist_tracks = |t: &StreamTrack| artist_of.get(&crate::stream_song_key(t)).map(|a| model.of_artist(*a).iter().map(|&i| i as usize).collect()).unwrap_or_default();
        let ctx = WaveContext { model: &model, ok: &ok, genre: None, background: &no, artist_tracks: &artist_tracks };
        let g = guests(&ctx, &pool, &calm);
        assert!(g.len() >= 4, "{}", g.len());
        assert!(g.iter().all(|g| title(g).starts_with("Drift")), "{:?}", g.iter().map(title).collect::<Vec<_>>());
        let artists: HashSet<&str> = g.iter().map(|g| g.artist.as_str()).collect();
        assert!(artists.len() * PER_ARTIST >= g.len());
        // Rap: the rap of the pool.
        let rap: Vec<usize> = d.new.iter().filter(|n| n.track.title.starts_with("Улица")).flat_map(|n| n.near.iter().filter_map(|t| model.position(*t))).take(20).collect();
        let g = guests(&ctx, &pool, &rap);
        assert!(!g.is_empty());
        assert!(g.iter().take(5).all(|g| !title(g).starts_with("Drift")), "{:?}", g.iter().map(title).collect::<Vec<_>>());
    }

    /// The next guest is the best not served yet, not of an artist turned
    /// away; linked ones before those that only sound alike.
    #[test]
    fn the_next_guest() {
        let d = synthetic::user(synthetic::Kind::NicheCalm, 5);
        let p = d.parts();
        let last = p.input.events.iter().map(|e| e.ts).max().unwrap();
        let model = p.model_at(last + 1);
        let track = |n: &str| StreamTrack::new(nikgob_core::streaming::RemoteId::new(nikgob_core::streaming::Service::Youtube, n), n);
        let guest = |n: &str, artist: &str, linked: bool, score: f32| Guest { track: track(n), key: n.into(), artist: artist.into(), near: Vec::new(), embedded: None, linked, score };
        let list = vec![guest("a", "x", false, 9.0), guest("b", "x", true, 1.0), guest("c", "y", true, 2.0)];
        let none = HashSet::new();
        assert_eq!(pick(&model, &list, &[], &none, &none), Some(2), "the best linked");
        let served: HashSet<String> = ["c".to_owned()].into();
        assert_eq!(pick(&model, &list, &[], &served, &none), Some(1));
        let away: HashSet<String> = ["x".to_owned()].into();
        assert_eq!(pick(&model, &list, &[], &served, &away), None);
        // Linked to the songs just before its place: ahead of a better one.
        let mut here = guest("d", "z", true, 1.5);
        here.near = vec![7];
        let list = vec![guest("c", "y", true, 2.0), here];
        assert_eq!(pick(&model, &list, &[7], &none, &none), Some(1));
    }
}
