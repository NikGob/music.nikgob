//! The open world: new music beyond the
//! library, not a few songs between loved ones.
//!
//! * **A niche** ([`is_niche`]): a filter (a mood, a scene, the
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
//!   aggressive rap song of a rapper who has one calm song stays out). A
//!   niche of a sound (calm, lively, dark, the background: `strict`) takes
//!   no song on one link alone — a calm song's radio holds loud rap too:
//!   without a preview it needs its artist's own songs in the niche's band,
//!   or the lists of [`STRICT_LINKS`] of the niche's songs.
//! * **The niches of the home** ([`niches`]): the calm songs, each kind of
//!   song and each family of genres the library has a few loved songs of
//!   ([`NICHE_MIN_SEEDS`]) but a niche's worth only — each a card of its
//!   own even below a mix's least length, its new music the rest.
//! * **The next of a wave** ([`next`]): what the coming «Моя волна» asks —
//!   given a filter (novelty, character, scene) and the session, the next
//!   `n` of the library and of the open pool ranked together: the walk of
//!   the graph from the session's tracks ([`crate::graph::SimGraph::walk`])
//!   links both, an outside song reached from two or three independent
//!   seeds ([`crate::discover::multi_seed`]) stands far above a lone
//!   answer, and new music takes the filter's share (a niche's at least).

use std::collections::{HashMap, HashSet};

use nikgob_core::model::{ArtistId, TrackId};
use nikgob_core::streaming::StreamTrack;
use serde::{Deserialize, Serialize};

use crate::analysis::Vector;
use crate::embedding::Compact;
use crate::index::{NICHE_FITS, NicheProbe, VectorIndex, euclid};
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
/// A song without a preview joins a niche of a sound when the lists of
/// this many of its songs hold it.
pub const STRICT_LINKS: usize = 2;
/// A niche of the home has this many songs at least, two of them loved…
pub const NICHE_MIN_SEEDS: usize = 3;
/// …and the home holds this many of the niches the row lacks at most.
pub const NICHE_CARDS: usize = 2;

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
    /// A niche of a sound: one link is no evidence (see the module docs).
    strict: bool,
    /// The niche measured by the library's index, for previews the neural
    /// sound model heard ([`Self::heard`]).
    probe: Option<NicheProbe<TrackId>>,
}

impl NicheSound {
    /// The sound of the niche of `tracks`, `strict` for a niche of a sound.
    pub fn of(model: &UserModel, tracks: &[usize], strict: bool) -> Self {
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
        Self { tracks: tracks.to_vec(), centre, radius, band, lanes, strict, probe: None }
    }

    /// [`Self::of`], measured by the library's `index` too: a preview with
    /// the neural model's vector is judged by the blend of its sound
    /// ([`Self::fits_heard`]).
    pub fn heard(model: &UserModel, tracks: &[usize], strict: bool, index: Option<&VectorIndex<TrackId>>) -> Self {
        let mut s = Self::of(model, tracks, strict);
        s.probe = index.filter(|ix| ix.embedded_len() > 0).and_then(|ix| ix.niche_probe(&tracks.iter().filter(|&&i| i < model.len()).map(|&i| model.id(i)).collect::<Vec<_>>()));
        s
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
        self.fits_by(model, vector, None, lane, near, artist)
    }

    /// [`Self::fits`] with the preview's neural model vector (`embedding`)
    /// and the library's `index`: a preview the model heard is near the
    /// niche by the blend of both sounds ([`VectorIndex::probe_fit`])
    /// rather than by bliss's alone.
    #[allow(clippy::too_many_arguments)]
    pub fn fits_heard(
        &self,
        model: &UserModel,
        index: Option<&VectorIndex<TrackId>>,
        vector: Option<&Vector>,
        embedding: Option<&Compact>,
        lane: Option<LaneKey>,
        near: &[usize],
        artist: &[f32],
    ) -> bool {
        let blend = match (index, &self.probe, vector, embedding) {
            (Some(ix), Some(p), Some(v), Some(e)) => ix.probe_fit(p, v, Some(e)),
            _ => None,
        };
        self.fits_by(model, vector, blend, lane, near, artist)
    }

    /// [`Self::fits`], the preview near the niche by `blend` when it is
    /// known.
    fn fits_by(&self, model: &UserModel, vector: Option<&Vector>, blend: Option<f32>, lane: Option<LaneKey>, near: &[usize], artist: &[f32]) -> bool {
        if !self.kind_fits(lane) {
            return false;
        }
        // The artist's own songs in the library, when it has them: in the
        // band, or out.
        let artist_in_band = (!artist.is_empty()).then(|| {
            let mut a = artist.to_vec();
            a.sort_by(f32::total_cmp);
            let median = a[a.len() / 2];
            self.band.is_none_or(|(lo, hi)| median <= hi + ARTIST_LOUDER && median >= lo - ARTIST_LOUDER)
        });
        if artist_in_band == Some(false) {
            return false;
        }
        match vector {
            Some(v) => {
                let e = model.embed(v);
                let near_sound = match blend {
                    Some(fit) => fit >= NICHE_FITS,
                    None => self.centre.as_ref().is_none_or(|c| euclid(&e, c) <= self.radius),
                };
                let energy = self.band.is_none_or(|(lo, hi)| (lo..=hi).contains(&model.energy_of(&e)));
                near_sound && energy
            }
            None => {
                let mut links: Vec<usize> = near.iter().copied().filter(|i| self.tracks.contains(i)).collect();
                links.sort_unstable();
                links.dedup();
                if self.strict {
                    // A radio's word alone is no sound: its artist's songs
                    // or several of the niche's lists.
                    artist_in_band == Some(true) || links.len() >= STRICT_LINKS
                } else {
                    !links.is_empty()
                }
            }
        }
    }
}

/// The character of a wave («Характер»).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Character {
    #[default]
    Any,
    Lively,
    Calm,
    Dark,
    Background,
}

/// The scene of a wave («Сцена / Жанр»).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
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
    /// The neural sound model's vector of its preview, if the model heard it.
    pub embedding: Option<Compact>,
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
    /// The library's sound index, with the neural model's vectors: a new
    /// song's preview the model heard is judged by them too.
    pub index: Option<&'a VectorIndex<TrackId>>,
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

/// Whether a new song is of `scene`: by its kind for a kind; for a profile
/// by its preview's region, else by the library's songs it is linked to;
/// for a family of genres by its artist's songs in the library, else by
/// the songs it is linked to (most of them of the family).
pub(crate) fn scene_of_new(ctx: &WaveContext<'_>, scene: Scene, c: &OpenCandidate, artist_tracks: &[usize]) -> bool {
    let m = ctx.model;
    let most = |of: &dyn Fn(usize) -> bool, list: &[usize]| !list.is_empty() && 2 * list.iter().filter(|&&i| of(i)).count() > list.len();
    match scene {
        Scene::All => true,
        Scene::Kind(l) => c.lane == Some(l),
        Scene::Profile(k) => {
            let is = |j: Option<usize>| j.and_then(|j| m.profiles.list.get(j)).is_some_and(|p| p.key == k);
            match &c.vector {
                Some(v) => is(m.regions.of(&m.embed(v), None)),
                None => most(&|i| is(m.profile_at(i)), &c.near),
            }
        }
        Scene::Genre(f) => {
            let Some(g) = ctx.genre else { return false };
            let of = |i: usize| g(i) == Some(f);
            if artist_tracks.is_empty() { most(&of, &c.near) } else { most(&of, artist_tracks) }
        }
    }
}

/// A niche the home holds a card of ([`niches`]).
#[derive(Clone, Debug, PartialEq)]
pub struct NicheCard {
    pub key: crate::mixes::MixKey,
    /// Its songs, the best loved first: the card's own and its seeds.
    pub seeds: Vec<usize>,
    /// A niche of a sound ([`NicheSound`]'s `strict`).
    pub strict: bool,
}

/// The niches the home may hold a card of (see the module docs): the calm
/// songs ([`Character::Calm`]), each kind of song, each family of genres
/// `ctx` tells — those with [`NICHE_MIN_SEEDS`] songs and two loved but a
/// niche's worth only ([`is_niche`]; a kind or genre below a mix's least
/// length, [`crate::mixes::MIN_SONGS`]), the most loved first, every song
/// once (a calm niche first), [`NICHE_CARDS`] at most.
pub fn niches(ctx: &WaveContext<'_>) -> Vec<NicheCard> {
    use crate::mixes::{MixKey, lane_code};
    let m = ctx.model;
    let now = m.now();
    let mut filters: Vec<(crate::mixes::MixKey, WaveFilter, bool)> = vec![(MixKey::Calm, WaveFilter { character: Character::Calm, ..WaveFilter::default() }, true)];
    let mut lanes: Vec<LaneKey> = (0..m.len()).map(|i| LaneKey::of(m.song_type_at(i))).filter(|l| *l != LaneKey::Unknown).collect();
    lanes.sort_by_key(|l| lane_code(*l));
    lanes.dedup();
    for l in lanes {
        filters.push((MixKey::Kind(lane_code(l)), WaveFilter { scene: Scene::Kind(l), ..WaveFilter::default() }, false));
    }
    if let Some(g) = ctx.genre {
        let mut fams: Vec<u8> = (0..m.len()).filter_map(g).collect();
        fams.sort_unstable();
        fams.dedup();
        for f in fams {
            filters.push((MixKey::Genre(f), WaveFilter { scene: Scene::Genre(f), ..WaveFilter::default() }, false));
        }
    }
    let mut found: Vec<(f32, NicheCard)> = Vec::new();
    for (key, filter, strict) in filters {
        let mut seeds: Vec<usize> = (0..m.len()).filter(|&i| (ctx.ok)(i) && fits_filter(ctx, &filter, i)).collect();
        let artists: HashSet<ArtistId> = seeds.iter().filter_map(|&i| m.main_artist_at(i)).collect();
        let loved = seeds.iter().filter(|&&i| m.tracks[i].aff_long(now) > 0.0).count();
        if seeds.len() < NICHE_MIN_SEEDS || loved < 2 || !is_niche(seeds.len(), artists.len()) {
            continue;
        }
        // A kind or a genre of a mix's length is the row's to draw (judged
        // a niche by its pool then): only one too small for a card is
        // given one. The calm songs take the calm card's place whatever.
        if !strict && seeds.len() >= crate::mixes::MIN_SONGS {
            continue;
        }
        seeds.sort_by(|a, b| m.tracks[*b].aff_long(now).total_cmp(&m.tracks[*a].aff_long(now)).then(a.cmp(b)));
        let love: f32 = seeds.iter().map(|&i| m.tracks[i].aff_long(now).max(0.0)).sum();
        // The calm niche before the others whatever its love.
        let rank = if strict { f32::INFINITY } else { love };
        found.push((rank, NicheCard { key, seeds, strict }));
    }
    found.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut taken: HashSet<usize> = HashSet::new();
    let mut out = Vec::new();
    for (_, card) in found {
        if out.len() == NICHE_CARDS {
            break;
        }
        // A niche the one before already holds is not another.
        if card.seeds.iter().filter(|i| taken.contains(i)).count() * 2 > card.seeds.len() {
            continue;
        }
        taken.extend(card.seeds.iter().copied());
        out.push(card);
    }
    out
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
    let sound = (filtered || niche).then(|| NicheSound::heard(m, &fitting, filter.character != Character::Any, ctx.index));
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
        if !scene_of_new(ctx, filter.scene, c, &artist_tracks) {
            continue;
        }
        if let Some(s) = &sound {
            let energies: Vec<f32> = artist_tracks.iter().filter_map(|&i| m.energy_at(i)).collect();
            if !s.fits_heard(m, ctx.index, c.vector.as_ref(), c.embedding.as_ref(), c.lane, &c.near, &energies) {
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

    // As many new as the share asks and the pool has, spread evenly; a
    // side short of its share is made up by the other, except at the ends:
    // «Знакомое» (0) plays no new song and «Только новое» (1) no library
    // one, fewer than `n` when that side runs out.
    let (want_lib, want_new) = if share <= 0.0 {
        (n.min(lib.len()), 0)
    } else if share >= 1.0 {
        (0, n.min(ext.len()))
    } else {
        let want_new = ((n as f32 * share).round() as usize).min(ext.len());
        let want_lib = (n - want_new).min(lib.len());
        (want_lib, (n - want_lib).min(ext.len()))
    };
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
