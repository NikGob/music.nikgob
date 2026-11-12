//! The open world (docs/recommendations.md §1): new music beyond the
//! library, not a few songs between loved ones.
//!
//! * **A niche** ([`is_niche`], §1.2): a filter (a mood, a scene, the
//!   background, a mix of the home) the library has fewer than
//!   [`NICHE_TRACKS`] fitting tracks or fewer than [`NICHE_ARTISTS`]
//!   artists for. Its library tracks are seeds, not a closed list: new
//!   music takes [`crate::mix_new::NICHE_SHARE`] and more of what it plays.
//! * **The niche's sound** ([`NicheSound`]): what a new song must be to
//!   join it — near the niche's sound by its analysed preview and in its
//!   band of energy; without a preview, linked by the graph or by the
//!   radio that found it to the niche's songs, of a kind of song the niche
//!   has (an instrumental niche takes no sung song) and, when its artist is
//!   in the library, not of an artist whose own songs are far louder (an
//!   aggressive rap song of a rapper who has one calm song stays out).
//! * **The next of a wave** ([`next`]): what the coming «Моя волна» asks —
//!   given a filter (novelty, character, scene) and the session, the next
//!   `n` of the library and of the open pool ranked together: the walk of
//!   the graph from the session's tracks ([`crate::graph::SimGraph::walk`])
//!   links both, an outside song reached from two or three independent
//!   seeds ([`crate::discover::multi_seed`]) stands far above a lone
//!   answer, and new music takes the filter's share (a niche's at least).

use std::collections::{HashMap, HashSet};

use nikgob_core::model::ArtistId;
use nikgob_core::streaming::StreamTrack;

use crate::analysis::Vector;
use crate::index::euclid;
use crate::model::UserModel;
use crate::pick::Item;
use crate::profiles::LaneKey;
use crate::session::{Ended, Session};
use crate::taste::HOUR_MS;

/// A filter with fewer fitting library tracks than this is a niche…
pub const NICHE_TRACKS: usize = 25;
/// …and one of fewer artists than this.
pub const NICHE_ARTISTS: usize = 4;
/// A niche's sound reaches this many median distances from its centre at
/// least (a niche of three tracks has a narrow centre)…
const NICHE_RADIUS: f32 = 1.4;
/// …and its energy band this far beyond its own songs' (z units).
const NICHE_ENERGY_SLACK: f32 = 0.35;
/// An artist of the library whose median energy stands this far above the
/// niche's band plays no part in it.
const ARTIST_LOUDER: f32 = 0.5;
/// The character of the wave on the mood axes (z units of the library)…
const CALM_Z: f32 = -0.8;
/// …and calm is quiet in itself too, not only beside the rest of the
/// library: bliss's loudness (absolute; about −14 dB) at most. A library of
/// rap has its quieter rap, and that is no calm music.
const CALM_LOUDNESS: f32 = 0.68;
const LIVELY_Z: f32 = 0.8;
const DARK_Z: f32 = -0.5;
const BACKGROUND_Z: f32 = 0.6;
/// The session's tracks the walk starts from, and the niche's best loved
/// when the session has none.
const WALK_SEEDS: usize = 5;
const NICHE_SEEDS: usize = 8;
/// A library track played this recently waits.
const RESTED_MS: i64 = 3 * HOUR_MS;

/// Whether a filter with `tracks` fitting library tracks of `artists`
/// artists is a niche.
pub fn is_niche(tracks: usize, artists: usize) -> bool {
    tracks < NICHE_TRACKS || artists < NICHE_ARTISTS
}

/// What a new song must be to join a niche (see the module docs).
#[derive(Clone, Debug, Default)]
pub struct NicheSound {
    /// The niche's songs (catalog positions).
    pub tracks: Vec<usize>,
    /// The mean of their embedded sound and how far it reaches.
    centre: Option<Vector>,
    radius: f32,
    /// Their band of energy, widened.
    band: Option<(f32, f32)>,
    /// The kinds of song among them.
    lanes: HashSet<LaneKey>,
}

impl NicheSound {
    /// The sound of the niche of `tracks`.
    pub fn of(model: &UserModel, tracks: &[usize]) -> Self {
        let embedded: Vec<&Vector> = tracks.iter().filter_map(|&i| model.embedded_at(i)).collect();
        let centre = (!embedded.is_empty()).then(|| {
            let mut c = [0.0f32; crate::analysis::DIM];
            for v in &embedded {
                for (a, b) in c.iter_mut().zip(v.iter()) {
                    *a += b;
                }
            }
            for a in &mut c {
                *a /= embedded.len() as f32;
            }
            c
        });
        let scale = model.scale().max(1e-6);
        let radius = centre.as_ref().map_or(0.0, |c| {
            let far = embedded.iter().map(|v| euclid(&v[..], &c[..])).fold(0.0f32, f32::max);
            (far * 1.25).max(NICHE_RADIUS * scale)
        });
        let mut e: Vec<f32> = tracks.iter().filter_map(|&i| model.energy_at(i)).collect();
        e.sort_by(f32::total_cmp);
        let band = (!e.is_empty()).then(|| (e[0] - NICHE_ENERGY_SLACK, e[e.len() - 1] + NICHE_ENERGY_SLACK));
        let lanes = tracks.iter().map(|&i| LaneKey::of(model.song_type_at(i))).collect();
        Self { tracks: tracks.to_vec(), centre, radius, band, lanes }
    }

    /// Whether a sung song of `lane` (or one without words) is of the
    /// niche's kinds; an unknown kind on either side says nothing.
    fn kind_fits(&self, lane: Option<LaneKey>) -> bool {
        match lane {
            None | Some(LaneKey::Unknown) => true,
            Some(l) => self.lanes.is_empty() || self.lanes.contains(&LaneKey::Unknown) || self.lanes.contains(&l),
        }
    }

    /// Whether a new song joins the niche: its analysed preview `vector`
    /// (raw), its kind `lane`, the niche's songs it is linked to (`near`,
    /// by the graph or by the radio that found it) and the energies of its
    /// artist's songs in the library (`artist`, empty for a new artist).
    pub fn fits(&self, model: &UserModel, vector: Option<&Vector>, lane: Option<LaneKey>, near: &[usize], artist: &[f32]) -> bool {
        if !self.kind_fits(lane) {
            return false;
        }
        if let Some((lo, hi)) = self.band
            && !artist.is_empty()
        {
            let mut a = artist.to_vec();
            a.sort_by(f32::total_cmp);
            let median = a[a.len() / 2];
            if median > hi + ARTIST_LOUDER || median < lo - ARTIST_LOUDER {
                return false;
            }
        }
        match vector {
            Some(v) => {
                let e = model.embed(v);
                let near_sound = self.centre.as_ref().is_none_or(|c| euclid(&e, c) <= self.radius);
                let energy = self.band.is_none_or(|(lo, hi)| (lo..=hi).contains(&model.energy_of(&e)));
                near_sound && energy
            }
            None => near.iter().any(|i| self.tracks.contains(i)),
        }
    }
}

/// The character of a wave («Характер»).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Character {
    #[default]
    Any,
    Lively,
    Calm,
    Dark,
    Background,
}

/// The scene of a wave («Сцена / Жанр»).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scene {
    #[default]
    All,
    /// A taste profile by its key.
    Profile(u32),
    /// A kind of song (a language, songs without words).
    Kind(LaneKey),
    /// A family of genres ([`crate::mix_names`]).
    Genre(u8),
}

/// What a wave plays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaveFilter {
    /// The share of new music asked for (0 «Знакомое» … 1 «Только новое»).
    pub novelty: f32,
    pub character: Character,
    pub scene: Scene,
}

impl Default for WaveFilter {
    fn default() -> Self {
        Self { novelty: 0.2, character: Character::Any, scene: Scene::All }
    }
}

/// A song of the open pool: a discovery, a song the graph found beside the
/// library's, a niche's top-up.
#[derive(Clone, Debug)]
pub struct OpenCandidate {
    pub track: StreamTrack,
    /// [`crate::stream_song_key`].
    pub key: String,
    /// Its analysed preview (raw), if any.
    pub vector: Option<Vector>,
    /// Its kind of song, if told.
    pub lane: Option<LaneKey>,
    /// The library's songs the graph or the radio that found it link it to.
    pub near: Vec<usize>,
    /// How much it is believed (its rank among the finds and its source).
    pub weight: f32,
}

/// What [`next`] reads besides the pool.
pub struct WaveContext<'a> {
    pub model: &'a UserModel,
    /// A library track it may play (a file under «Только скачанное», not
    /// junk, not disliked).
    pub ok: &'a dyn Fn(usize) -> bool,
    /// The family of genres of a library track, when the tags tell it.
    pub genre: Option<&'a dyn Fn(usize) -> Option<u8>>,
    /// Whether a library track suits the background.
    pub background: &'a dyn Fn(usize) -> bool,
    /// The library's songs of the main artist of a new song (catalog
    /// positions; empty for a new artist).
    pub artist_tracks: &'a dyn Fn(&StreamTrack) -> Vec<usize>,
}

/// One pick of a wave.
#[derive(Clone, Debug, PartialEq)]
pub struct WavePick {
    pub item: Item,
    pub score: f32,
    /// A song of the open pool.
    pub new: bool,
    /// Its main artist: a library artist, or a new one by name.
    pub artist: String,
}

/// The next of a wave and what it found of its filter.
#[derive(Clone, Debug, Default)]
pub struct WaveNext {
    pub picks: Vec<WavePick>,
    /// The library tracks fitting the filter, and their artists.
    pub fitting: usize,
    pub artists: usize,
    /// It is a niche ([`is_niche`]): its tracks seeded the pool.
    pub niche: bool,
    /// The share of new music it took.
    pub share: f32,
}

/// Whether the library track at `i` fits `filter`.
pub fn fits_filter(ctx: &WaveContext<'_>, filter: &WaveFilter, i: usize) -> bool {
    let m = ctx.model;
    let character = match filter.character {
        Character::Any => true,
        Character::Calm => m.axes_at(i).is_some_and(|a| a.energy <= CALM_Z) && m.loudness_at(i).is_none_or(|l| l <= CALM_LOUDNESS),
        Character::Lively => m.axes_at(i).is_some_and(|a| a.energy >= LIVELY_Z),
        Character::Dark => m.axes_at(i).is_some_and(|a| a.brightness <= DARK_Z),
        Character::Background => (ctx.background)(i) || (m.song_type_at(i).lane() == crate::songtype::SongLane::Instrumental && m.axes_at(i).is_some_and(|a| a.energy.abs() <= BACKGROUND_Z)),
    };
    let scene = match filter.scene {
        Scene::All => true,
        Scene::Profile(k) => m.profile_at(i).and_then(|j| m.profiles.list.get(j)).is_some_and(|p| p.key == k),
        Scene::Kind(l) => LaneKey::of(m.song_type_at(i)) == l,
        Scene::Genre(f) => ctx.genre.is_some_and(|g| g(i) == Some(f)),
    };
    character && scene
}

/// Whether a new song of the analysed preview `v` (raw) is of `character`
/// (an unknown sound says nothing).
fn character_of_new(m: &UserModel, character: Character, v: Option<&Vector>) -> bool {
    let Some(v) = v else { return true };
    let a = m.axes_of(&m.embed(v));
    match character {
        Character::Any | Character::Background => true,
        Character::Calm => a.energy <= CALM_Z + NICHE_ENERGY_SLACK && v[crate::mood::LOUDNESS] <= CALM_LOUDNESS,
        Character::Lively => a.energy >= LIVELY_Z - NICHE_ENERGY_SLACK,
        Character::Dark => a.brightness <= DARK_Z + NICHE_ENERGY_SLACK,
    }
}

/// The share of new music a wave of `filter` takes: the filter's, at least
/// a niche's when new music is asked for at all.
pub fn share_of(filter: &WaveFilter, niche: bool) -> f32 {
    let asked = filter.novelty.clamp(0.0, 1.0);
    if niche && asked > 0.0 { asked.max(crate::mix_new::NICHE_SHARE) } else { asked }
}

/// The next `n` of a wave of `filter` in `session` (see the module docs):
/// library tracks and songs of `pool`, none of `served` (track ids and
/// [`crate::stream_song_key`]s as strings).
pub fn next(ctx: &WaveContext<'_>, pool: &[OpenCandidate], filter: &WaveFilter, session: &Session, n: usize, served: &HashSet<String>) -> WaveNext {
    let m = ctx.model;
    let now = m.now();
    let fitting: Vec<usize> = (0..m.len()).filter(|&i| (ctx.ok)(i) && fits_filter(ctx, filter, i)).collect();
    let artists: HashSet<ArtistId> = fitting.iter().filter_map(|&i| m.main_artist_at(i)).collect();
    let niche = is_niche(fitting.len(), artists.len());
    let share = share_of(filter, niche);
    // The walk from the session's tracks heard out; else from the
    // filter's best loved (a niche's tracks are its seeds).
    let mut seeds: Vec<(usize, f32)> = session
        .tracks()
        .rev()
        .filter(|p| p.ended != Ended::Early)
        .filter_map(|p| m.position(p.track))
        .take(WALK_SEEDS)
        .enumerate()
        .map(|(k, i)| (i, 0.6f32.powi(k as i32)))
        .collect();
    let mut loved: Vec<usize> = fitting.clone();
    loved.sort_by(|a, b| m.tracks[*b].aff_long(now).total_cmp(&m.tracks[*a].aff_long(now)).then(a.cmp(b)));
    loved.truncate(NICHE_SEEDS);
    if seeds.is_empty() {
        seeds = loved.iter().enumerate().map(|(k, &i)| (i, 0.8f32.powi(k as i32))).collect();
    }
    let walk = m.graph.walk(&seeds);
    let seed_set: HashSet<usize> = seeds.iter().map(|s| s.0).chain(loved.iter().copied()).collect();
    let filtered = filter.character != Character::Any || filter.scene != Scene::All;
    let fitting_set: HashSet<usize> = fitting.iter().copied().collect();
    let sound = (filtered || niche).then(|| NicheSound::of(m, &fitting));
    let anchor = session.anchor(m).map(|a| m.embed(&a));
    let scale = m.scale().max(1e-6);

    // The library's: loved, linked to the session, near its sound, rested.
    let mut lib: Vec<(f32, usize)> = fitting
        .iter()
        .copied()
        .filter(|&i| !served.contains(&m.id(i).to_string()))
        .filter(|&i| m.taste.last_played_at(i).is_none_or(|t| now - t >= RESTED_MS))
        .map(|i| {
            let aff = (m.tracks[i].aff_long(now).max(0.0) / 3.0).tanh();
            let link = walk.get(&i).copied().unwrap_or(0.0).min(1.0);
            let near = match (m.embedded_at(i), anchor.as_ref()) {
                (Some(e), Some(a)) => (-euclid(e, a) / scale).exp(),
                _ => 0.0,
            };
            (0.6 * aff + link + 0.5 * near + 0.2 * m.graph.prior(i), i)
        })
        .collect();
    lib.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));

    // The pool's: its weight and its links, times the independent seeds
    // that reach it (its linked songs' distinct artists among the seeds).
    let mut ext: Vec<(f32, usize)> = Vec::new();
    for (k, c) in pool.iter().enumerate() {
        if served.contains(&c.key) {
            continue;
        }
        if !character_of_new(m, filter.character, c.vector.as_ref()) {
            continue;
        }
        let artist_tracks = (ctx.artist_tracks)(&c.track);
        if let Some(s) = &sound {
            let energies: Vec<f32> = artist_tracks.iter().filter_map(|&i| m.energy_at(i)).collect();
            if !s.fits(m, c.vector.as_ref(), c.lane, &c.near, &energies) {
                continue;
            }
        }
        // A new song of a library artist off the filter is not the
        // filter's (a rapper's song in «Спокойное»).
        if filtered && !artist_tracks.is_empty() && !artist_tracks.iter().any(|i| fitting_set.contains(i)) && c.vector.is_none() {
            continue;
        }
        let link: f32 = c.near.iter().map(|i| walk.get(i).copied().unwrap_or(0.0)).sum::<f32>().min(2.0);
        let independent: HashSet<Option<ArtistId>> = c.near.iter().filter(|i| seed_set.contains(i) || walk.contains_key(i)).map(|&i| m.main_artist_at(i)).collect();
        let score = (c.weight + 0.5 * link) * crate::discover::multi_seed(independent.len());
        ext.push((score, k));
    }
    ext.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));

    // As many new as the share asks and the pool has, spread evenly.
    let want_new = ((n as f32 * share).round() as usize).min(ext.len());
    let want_lib = (n - want_new).min(lib.len());
    let want_new = (n - want_lib).min(ext.len());
    let lib_artist = |i: usize| m.main_artist_at(i).map_or_else(|| format!("track:{i}"), |a| a.to_string());
    let ext_artist = |k: usize| pool[k].track.artists.first().map_or_else(String::new, |a| crate::artist_key(&a.name));
    let mut picks: Vec<WavePick> = Vec::with_capacity(want_lib + want_new);
    let (mut took_lib, mut took_new) = (0usize, 0usize);
    let mut per_artist: HashMap<String, usize> = HashMap::new();
    let cap = (n / 3).max(2);
    while took_lib + took_new < want_lib + want_new {
        // The new one's turn when it is behind its even share.
        let new_turn = took_new < want_new
            && (took_lib >= want_lib || (took_new as f32 + 0.5) * (want_lib + want_new) as f32 <= want_new as f32 * (took_lib + took_new + 1) as f32);
        let recent: Vec<String> = picks.iter().rev().take(2).map(|p| p.artist.clone()).collect();
        let pick = if new_turn {
            take_next(&mut ext, &ext_artist, &recent, &per_artist, cap).map(|(s, k, a)| WavePick { item: Item::Stream(pool[k].track.id.clone()), score: s, new: true, artist: a })
        } else {
            take_next(&mut lib, &lib_artist, &recent, &per_artist, cap).map(|(s, i, a)| WavePick { item: Item::Track(m.id(i)), score: s, new: false, artist: a })
        };
        let Some(p) = pick else {
            if new_turn {
                took_new = want_new;
            } else {
                took_lib = want_lib;
            }
            continue;
        };
        if p.new {
            took_new += 1;
        } else {
            took_lib += 1;
        }
        *per_artist.entry(p.artist.clone()).or_default() += 1;
        picks.push(p);
    }
    WaveNext { picks, fitting: fitting.len(), artists: artists.len(), niche, share }
}

/// Takes the best of `list` (best first) whose artist is not among the
/// last two picks (`recent`) nor at `cap`, looking a little way down;
/// else the best whose artist is under `cap`; else the best.
fn take_next(list: &mut Vec<(f32, usize)>, artist: &dyn Fn(usize) -> String, recent: &[String], per: &HashMap<String, usize>, cap: usize) -> Option<(f32, usize, String)> {
    const LOOK: usize = 48;
    let under = |k: usize| per.get(&artist(k)).copied().unwrap_or(0) < cap;
    let at = list
        .iter()
        .take(LOOK)
        .position(|&(_, k)| !recent.contains(&artist(k)) && under(k))
        .or_else(|| list.iter().take(LOOK).position(|&(_, k)| under(k)))
        .or_else(|| (!list.is_empty()).then_some(0))?;
    let (s, k) = list.remove(at);
    Some((s, k, artist(k)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A niche is few songs or few artists; it raises the share of new
    /// music only when new music is asked for at all.
    #[test]
    fn a_niche_is_few_songs_or_artists() {
        assert!(is_niche(3, 1) && is_niche(24, 10) && is_niche(200, 3));
        assert!(!is_niche(25, 4));
        let calm = WaveFilter { novelty: 0.2, character: Character::Calm, ..WaveFilter::default() };
        assert_eq!(share_of(&calm, false), 0.2);
        assert_eq!(share_of(&calm, true), crate::mix_new::NICHE_SHARE);
        assert_eq!(share_of(&WaveFilter { novelty: 0.0, ..calm }, true), 0.0, "«Знакомое» stays familiar");
        assert_eq!(share_of(&WaveFilter { novelty: 1.0, ..calm }, true), 1.0);
    }
}
