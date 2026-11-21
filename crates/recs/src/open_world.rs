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
//!   The library's songs are ranked by the wave's own pull (loved, linked
//!   to the session, near its sound), and by the user model too as a radio
//!   ranks them ([`library_value`]) when [`WAVE_MODEL_RANKING`] is on; the
//!   new songs it is about to play without a
//!   preview ([`WaveNext::unheard`]) and a filter with few new songs left
//!   ([`WaveNext::short`]) are told, so the runtime can ask for more.

use std::collections::{HashMap, HashSet};

use nikgob_core::model::{ArtistId, TrackId};
use nikgob_core::streaming::{RemoteId, StreamTrack};
use serde::{Deserialize, Serialize};

use crate::analysis::Vector;
use crate::embedding::Compact;
use crate::index::{NICHE_FITS, NicheProbe, VectorIndex, euclid};
use crate::model::{F_ALBUM_FIRST, F_IN_SESSION, F_IS_PREV, F_NEXT_ALBUM, F_SAME_ALBUM, Moment, UserModel, Weights};
use crate::pick::{Diversity, Item, Seen};
use crate::profiles::LaneKey;
use crate::rng::Rng;
use crate::session::{Ended, Session};
use crate::taste::HOUR_MS;
use nikgob_core::model::{EventContext, EventOrigin, UnixMs};
use std::sync::atomic::{AtomicU8, Ordering};

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
pub const CALM_Z: f32 = -0.8;
/// …and calm is quiet in itself too, not only beside the rest of the
/// library: bliss's loudness (absolute; about −14 dB) at most. A library of
/// rap has its quieter rap, and that is no calm music.
const CALM_LOUDNESS: f32 = 0.68;
const LIVELY_Z: f32 = 0.8;
const DARK_Z: f32 = -0.5;
/// The feeling of a song's cached lyrics moves it on the brightness axis
/// by this much at most (z units; sad words darker, happy ones brighter,
/// a song without lyrics where its sound puts it).
pub const VALENCE_NUDGE: f32 = 0.3;
const BACKGROUND_Z: f32 = 0.6;
/// The session's tracks the walk starts from, and the niche's best loved
/// when the session has none.
const WALK_SEEDS: usize = 5;
const NICHE_SEEDS: usize = 8;
/// A library track played this recently waits whatever the user's habit
/// ([`rest_of`]).
const RESTED_FLOOR_MS: i64 = 45 * 60_000;
/// A song rests [`REST_BASE_MS`] scaled by the user's own time between two
/// plays of a song ([`UserModel::repeat_rest`]) against [`REST_HABIT_MS`]
/// (its square root: a user who hears songs again within hours rests them
/// less, one who waits days more), within [`REST_RANGE_MS`]…
const REST_BASE_MS: i64 = 3 * HOUR_MS;
const REST_HABIT_MS: i64 = 24 * HOUR_MS;
const REST_RANGE_MS: (i64, i64) = (90 * 60_000, 6 * HOUR_MS);
/// …times this when it is an old one (heard long ago more than lately)…
const STAPLE_REST: f32 = 1.5;
/// …and this when it is new love (heard as much lately as ever).
const FALLEN_REST: f32 = 0.35;
/// The wave ranks the library's songs by the user model as a radio does
/// ([`LibStepper`]: one pick at a time, each by the model's value at the
/// session's moment as if the picks before it played) instead of by its
/// own pull alone.
pub const WAVE_MODEL_RANKING: bool = false;
/// [`WAVE_MODEL_RANKING`] as set for this process ([`set_model_ranking`]):
/// 0 the constant, 1 off, 2 on.
static MODEL_RANKING: AtomicU8 = AtomicU8::new(0);
/// The model's candidates of the moment are gathered with this many of the
/// filter's songs best by the wave's own pull (a wave starting with no
/// session has the user's loves, the walk of the graph from them).
const GATHER: usize = 200;
/// The seeded jitter of a pick's value ([`WaveTurn::seed`]), as a radio
/// picked by the model has: a wave started again does not open the same.
const WAVE_JITTER: f32 = 0.15;
/// Relative seeded tie-break jitter (±half this share) on the pull path when
/// [`WaveTurn::seed`] is set.
const PULL_JITTER: f32 = 0.18;
/// Cross-call artist fatigue window (last served picks) and half-life in picks.
pub(crate) const ARTIST_FATIGUE_WINDOW: usize = 24;
const ARTIST_FATIGUE_HALF: f32 = 8.0;
/// Progressive relative pull damping for recent wave plays of the same artist:
/// light after a single play, steeper on repeated plays to break ping-pong.
const ARTIST_FATIGUE_BASE: f32 = 0.14;
const ARTIST_FATIGUE_EXTRA: f32 = 0.30;
const ARTIST_FATIGUE_KNEE: f32 = 0.7;
/// Additional temporary relative damping for an artist rejected in the session.
const SKIP_ARTIST_PENALTY: f32 = 0.55;
/// Session lookback for skipped-artist cooldown and its decay half-life.
const SKIP_ARTIST_WINDOW: usize = 12;
const SKIP_ARTIST_HALF: f32 = 6.0;
/// A simulated pick plays this long ([`LibStepper`]).
const SIM_MS: i64 = 200_000;
/// A user whose own next starts are of the same artist this often
/// ([`UserModel::artist_again`]) has the last artist alone kept out of the
/// wave's next pick, across its calls too; any other the last two (the
/// radio's rules: no artist twice within three)…
const ARTIST_AGAIN: f32 = 0.45;
/// …the share shrunk to this with few starts.
const ARTIST_AGAIN_PRIOR: f32 = 0.3;
/// The pull of a song worn out ([`UserModel::worn`]) is this much lower per
/// unit of its chance of an early skip.
const WORN_PULL: f32 = 1.0;
/// Its filter's new songs are short when fewer than the share asks of a
/// pick and this many more are left: a block of the wave.
pub const NEW_RESERVE: usize = crate::wave::BLOCK_MAX as usize;
/// The new songs without a preview next in line told beside the picks.
const UNHEARD_AHEAD: usize = 3;
/// A new song without a preview found beside the same library songs as one
/// the session rejected unheard stands this many times lower…
pub const BLIND_DAMP: f32 = 0.25;
/// …and every song near where a rejected new song sounded (its preview, or
/// the mean of its seeds' sounds), or a library song the session rejected
/// sounds (by its push), this much lower at that very spot, the new ones by
/// this share of their score; library songs by [`NEW_PUSH_LIB`] of the
/// pull.
pub const NEW_PUSH: f32 = 0.6;
pub const NEW_PUSH_LIB: f32 = 0.55;
/// A song without a preview joins a niche of a sound when the lists of
/// this many of its songs hold it.
pub const STRICT_LINKS: usize = 2;
/// A niche of the home has this many songs at least, two of them loved…
pub const NICHE_MIN_SEEDS: usize = 3;
/// …and the home holds this many of the niches the row lacks at most.
pub const NICHE_CARDS: usize = 2;

/// The parts of the wave's picking the offline evaluation turns off one at
/// a time ([`set_off`]) to measure each; all are on in the player.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// One version of a song a session ([`UserModel::work_at`]).
    Works,
    /// The rest by the user's habit ([`rest_of`]; off: 3 hours).
    Rest,
    /// Short blocks while the mood is looked for ([`crate::wave::PROBE_MAX`]).
    Probe,
    /// A calm or background batch ordered by its transitions
    /// ([`crate::wave::sequence`]).
    Sequence,
    /// Rejected songs teach at once ([`BLIND_DAMP`], [`NEW_PUSH`], the
    /// halved share of new music).
    Rejected,
    /// Files come lately and never played lead the unheard of the mixes
    /// ([`UserModel::arrival_at`]).
    Arrivals,
}

static PARTS_OFF: AtomicU8 = AtomicU8::new(0);

fn part_bit(p: Part) -> u8 {
    1 << (p as u8)
}

/// Turns `parts` off for this process (the rest on).
pub fn set_off(parts: &[Part]) {
    PARTS_OFF.store(parts.iter().fold(0, |b, p| b | part_bit(*p)), Ordering::Relaxed);
}

/// Whether `p` is on ([`set_off`]).
pub fn part_on(p: Part) -> bool {
    PARTS_OFF.load(Ordering::Relaxed) & part_bit(p) == 0
}

/// Whether the wave ranks the library by the user model now
/// ([`WAVE_MODEL_RANKING`], or as [`set_model_ranking`] set it).
pub fn model_ranking() -> bool {
    match MODEL_RANKING.load(Ordering::Relaxed) {
        1 => false,
        2 => true,
        _ => WAVE_MODEL_RANKING,
    }
}

/// Sets [`model_ranking`] for this process (`None`: the constant): the
/// offline evaluation measures both ways in one run.
pub fn set_model_ranking(on: Option<bool>) {
    MODEL_RANKING.store(on.map_or(0, |o| if o { 2 } else { 1 }), Ordering::Relaxed);
}

/// Who a song is by, one identity for the library's songs and the
/// services' alike: the library's artist when the library knows them (a
/// new song through the library's songs of its artist), else the artist's
/// name, else nobody known (each such song an artist of its own).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ArtistKey {
    Library(ArtistId),
    /// The hash of [`crate::artist_key`] of the name.
    Named(u64),
    Unknown,
}

impl ArtistKey {
    /// The library's song at `i`.
    pub fn of_track(m: &UserModel, i: usize) -> Self {
        m.main_artist_at(i).map_or(Self::Unknown, Self::Library)
    }

    /// A new song of the services.
    pub fn of_new(ctx: &WaveContext<'_>, c: &OpenCandidate) -> Self {
        let m = ctx.model;
        if let Some(a) = (ctx.artist_tracks)(&c.track).into_iter().find_map(|i| m.main_artist_at(i)) {
            return Self::Library(a);
        }
        match c.track.artists.first().map(|a| crate::artist_key(&a.name)).filter(|k| !k.is_empty()) {
            Some(k) => {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                k.hash(&mut h);
                Self::Named(h.finish())
            }
            None => Self::Unknown,
        }
    }

    /// As text; `apart` names an unknown one apart from every other.
    pub fn label(self, apart: impl FnOnce() -> String) -> String {
        match self {
            Self::Library(a) => a.to_string(),
            Self::Named(h) => format!("name:{h:016x}"),
            Self::Unknown => apart(),
        }
    }
}

/// A song queued ahead of a wave's next picks ([`WaveTurn::queued`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ahead {
    /// A library song.
    Track(TrackId),
    /// A new song of the services, by its artist ([`ArtistKey::of_new`]).
    New(ArtistKey),
}

impl Ahead {
    /// A new song of `pool` ([`Self::New`]).
    pub fn of_new(ctx: &WaveContext<'_>, c: &OpenCandidate) -> Self {
        Self::New(ArtistKey::of_new(ctx, c))
    }
}

/// What a call of [`next_turn`] knows of the wave beyond its filter.
#[derive(Clone, Copy, Debug, Default)]
pub struct WaveTurn<'a> {
    /// The songs picked and queued after the one playing, oldest first:
    /// they play first, so the model's moment and the artists heard last
    /// go on from them.
    pub queued: &'a [Ahead],
    /// The run's seed (the model's jitter).
    pub seed: u64,
    /// The new songs the session rejected: the songs found beside the same
    /// library songs stand lower ([`BLIND_DAMP`]), and everything near
    /// where they sounded (their preview, else their seeds) a little lower
    /// ([`NEW_PUSH`]).
    pub skipped_new: &'a [RemoteId],
    /// Songs served before that may play again now (track ids and
    /// [`crate::stream_song_key`]s, [`crate::wave::repeatable`]): the very
    /// recordings, never another version of their songs.
    pub again: &'a [String],
    /// The works ([`crate::discover::stream_work_key`]) of the new songs
    /// served, queued or played this session, kept as they were (the pool
    /// may no longer hold them): no other version of them is picked.
    pub works: &'a [String],
    /// Artists served recently in this wave (oldest first, [`ArtistKey::label`]),
    /// used for smooth cross-call artist fatigue.
    pub recent_artists: &'a [String],
    /// Whether the wave is picking immediately after a rejection: a balanced
    /// wave stabilizes on a familiar song before dropping a new one.
    pub after_skip: bool,
}

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

impl WaveFilter {
    /// What the filter plays (its character and scene, not its novelty) as
    /// a short stable key.
    pub fn token(&self) -> String {
        let scene = match self.scene {
            Scene::All => "all".to_owned(),
            Scene::Profile(k) => format!("p{k}"),
            Scene::Kind(l) => format!("k{}", crate::mixes::lane_code(l)),
            Scene::Genre(f) => format!("g{f}"),
        };
        format!("{:?}.{scene}", self.character).to_lowercase()
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
    /// The model's weights as learnt since `model` was built (a radio's,
    /// [`crate::RadioState::attune`]); none: the model's own.
    pub learned: Option<&'a Weights>,
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
    /// New songs of the pool without a preview (positions in it): those it
    /// picked, then the next in line.
    pub unheard: Vec<usize>,
    /// New music was asked for and few new songs of the filter are left
    /// (fewer than its share of the picks and [`NEW_RESERVE`] more).
    pub short: bool,
    /// The filter's library songs, the best loved first: where to look for
    /// more of its new music.
    pub seeds: Vec<usize>,
}

/// Whether the library track at `i` fits `filter`.
pub fn fits_filter(ctx: &WaveContext<'_>, filter: &WaveFilter, i: usize) -> bool {
    let m = ctx.model;
    let character = match filter.character {
        Character::Any => true,
        Character::Calm => {
            m.axes_at(i).is_some_and(|a| a.energy <= CALM_Z) && m.loudness_at(i).is_none_or(|l| l <= CALM_LOUDNESS) && m.song_type_at(i).calm_dynamics()
        }
        Character::Lively => m.axes_at(i).is_some_and(|a| a.energy >= LIVELY_Z),
        Character::Dark => m.axes_at(i).is_some_and(|a| a.brightness + VALENCE_NUDGE * m.song_type_at(i).lyric_valence() <= DARK_Z),
        Character::Background => {
            let t = m.song_type_at(i);
            (ctx.background)(i)
                || (t.lane() == crate::songtype::SongLane::Instrumental && t.background_dynamics() && m.axes_at(i).is_some_and(|a| a.energy.abs() <= BACKGROUND_Z))
        }
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

/// A new song whose preview stands this near the user's dislikes
/// ([`UserModel::dislike_near_of`]) is no longer played when it was picked
/// before its preview was heard ([`still_fits`]).
pub const MISFIT_DISLIKE: f32 = 0.35;

/// Whether a new song picked for a wave of `filter` before its preview was
/// heard still fits it now that `c` holds the preview: of the filter's
/// character and scene, of its niche's sound when it is one (or filtered),
/// not near the dislikes. A song without a preview fits.
pub fn still_fits(ctx: &WaveContext<'_>, filter: &WaveFilter, c: &OpenCandidate) -> bool {
    let m = ctx.model;
    let Some(v) = c.vector.as_ref() else { return true };
    if m.dislike_near_of(&m.embed(v)) >= MISFIT_DISLIKE {
        return false;
    }
    if !character_of_new(m, filter.character, Some(v)) {
        return false;
    }
    let artist_tracks = (ctx.artist_tracks)(&c.track);
    if !scene_of_new(ctx, filter.scene, c, &artist_tracks) {
        return false;
    }
    let filtered = filter.character != Character::Any || filter.scene != Scene::All;
    let fitting: Vec<usize> = (0..m.len()).filter(|&i| (ctx.ok)(i) && fits_filter(ctx, filter, i)).collect();
    let artists: HashSet<ArtistId> = fitting.iter().filter_map(|&i| m.main_artist_at(i)).collect();
    if filtered || is_niche(fitting.len(), artists.len()) {
        let sound = NicheSound::heard(m, &fitting, filter.character != Character::Any, ctx.index);
        let energies: Vec<f32> = artist_tracks.iter().filter_map(|&i| m.energy_at(i)).collect();
        if !sound.fits_heard(m, ctx.index, Some(v), c.embedding.as_ref(), c.lane, &c.near, &energies) {
            return false;
        }
    }
    true
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
    next_turn(ctx, pool, filter, session, n, served, WaveTurn::default())
}

/// [`next`] knowing the wave's `turn`: what is queued, its seed.
pub fn next_turn(ctx: &WaveContext<'_>, pool: &[OpenCandidate], filter: &WaveFilter, session: &Session, n: usize, served: &HashSet<String>, turn: WaveTurn<'_>) -> WaveNext {
    next_ranked(ctx, pool, filter, session, n, served, model_ranking(), turn)
}

/// [`next`], the library ranked `by_model` ([`WAVE_MODEL_RANKING`]) or by
/// the wave's own pull alone.
#[allow(clippy::too_many_arguments)]
fn next_ranked(
    ctx: &WaveContext<'_>,
    pool: &[OpenCandidate],
    filter: &WaveFilter,
    session: &Session,
    n: usize,
    served: &HashSet<String>,
    by_model: bool,
    turn: WaveTurn<'_>,
) -> WaveNext {
    let m = ctx.model;
    let now = m.now();
    let fitting: Vec<usize> = (0..m.len()).filter(|&i| (ctx.ok)(i) && fits_filter(ctx, filter, i)).collect();
    let artists: HashSet<ArtistId> = fitting.iter().filter_map(|&i| m.main_artist_at(i)).collect();
    let niche = is_niche(fitting.len(), artists.len());
    let share = share_of(filter, niche);
    // The walk from the session's tracks it approved and the one playing
    // ([`Session::seeds`]); else from the filter's best loved (a niche's
    // tracks are its seeds).
    let mut seeds: Vec<(usize, f32)> = session
        .seeds(m)
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

    // The library's, rested: by the wave's own pull (loved, linked to the
    // session by the walk, near its sound); with [`WAVE_MODEL_RANKING`]
    // by the user model, one pick at a time ([`LibStepper`]), from the
    // model's candidates and the best of the pull together.
    let rest = rest_by_habit(m.repeat_rest());

    let rest_at = |i: usize| if part_on(Part::Rest) { rest_of(m, i, rest) } else { 3 * HOUR_MS };
    let free = |i: usize| fitting_set.contains(&i) && !served.contains(&m.id(i).to_string()) && m.taste.last_played_at(i).is_none_or(|t| now - t >= rest_at(i));
    let anchor = session.anchor(m).map(|a| m.embed(&a));
    let scale = m.scale().max(1e-6);
    // The artists kept out of the next pick: the last one, or the last two
    // for a user who seldom plays an artist twice running — the session's
    // and the queue's before this call's picks.
    let avoid = if m.artist_again(ARTIST_AGAIN_PRIOR) >= ARTIST_AGAIN { 1 } else { 2 };
    // One identity of an artist for the library's songs and the new ones
    // ([`ArtistKey`]).
    let lib_artist = |i: usize| ArtistKey::of_track(m, i).label(|| format!("track:{i}"));
    let mut order: Vec<String> = session.tracks().filter_map(|p| m.position(p.track)).map(lib_artist).collect();
    for (k, q) in turn.queued.iter().enumerate() {
        match q {
            Ahead::Track(t) => order.extend(m.position(*t).map(lib_artist)),
            Ahead::New(a) => order.push(a.label(|| format!("queued:{k}"))),
        }
    }
    // Smooth cross-call artist fatigue from artists served by this wave so far
    // (never from the user's pre-wave session, which seeds the taste). Scaled
    // by the filter's loved-artist breadth so narrow repertoires stay rich.
    let loved_artists: HashSet<ArtistId> = fitting.iter().filter(|&&i| m.tracks[i].aff_long(now) > 0.0).filter_map(|&i| m.main_artist_at(i)).collect();
    let breadth_n = if loved_artists.is_empty() { artists.len() } else { loved_artists.len() };
    let breadth = ((breadth_n as f32 - 4.0) / 8.0).clamp(0.0, 1.0);
    let fatigue_scale = breadth * (1.0 - 0.4 * m.artist_again(ARTIST_AGAIN_PRIOR));
    let mut artist_load: HashMap<&str, f32> = HashMap::new();
    let from = turn.recent_artists.len().saturating_sub(ARTIST_FATIGUE_WINDOW);
    let tail = &turn.recent_artists[from..];
    for (pos, a) in tail.iter().enumerate() {
        if a.is_empty() {
            continue;
        }
        let ago = tail.len() - pos;
        *artist_load.entry(a.as_str()).or_default() += 0.5f32.powf((ago - 1) as f32 / ARTIST_FATIGUE_HALF);
    }
    // Where the new songs the session rejected sounded (their preview, else
    // the mean of their seeds' sounds), and the seeds of those rejected
    // unheard; and where its rejected library songs sound, each as hard as
    // its rejection pushes ([`crate::session::Verdict::push`]): a skip turns
    // the picks from its sound and its artist within the cluster too.
    let mut rejected_at: Vec<(Vector, f32)> = Vec::new();
    let mut blind_seeds: HashSet<usize> = HashSet::new();
    let mut skipped_artists: HashMap<String, f32> = HashMap::new();
    for r in turn.skipped_new.iter().filter(|_| part_on(Part::Rejected)) {
        let Some(c) = pool.iter().find(|c| c.track.id == *r) else { continue };
        let ak = ArtistKey::of_new(ctx, c);
        if ak != ArtistKey::Unknown {
            *skipped_artists.entry(ak.label(String::new)).or_default() += 1.0;
        }
        match &c.vector {
            Some(v) => rejected_at.push((m.embed(v), 1.0)),
            None => {
                blind_seeds.extend(c.near.iter().copied());
                rejected_at.extend(centroid(m, &c.near).map(|v| (v, 1.0)));
            }
        }
    }
    if part_on(Part::Rejected) {
        rejected_at.extend(session.pushed(m).into_iter().filter_map(|(t, w)| Some((*m.embedded_at(m.position(t)?)?, w))));
        for (ago, p) in session.tracks().rev().take(SKIP_ARTIST_WINDOW).enumerate() {
            let v = crate::session::Session::verdict(p, m);
            if !v.rejects() {
                continue;
            }
            if let Some(i) = m.position(p.track) {
                let ak = ArtistKey::of_track(m, i);
                if ak != ArtistKey::Unknown {
                    let w = v.push() * 0.5f32.powf(ago as f32 / SKIP_ARTIST_HALF);
                    *skipped_artists.entry(ak.label(String::new)).or_default() += w;
                }
            }
        }
    }
    // Gaussian repulsion around rejected sounds: strong within half a median
    // distance, tapering rapidly so genuinely different songs stay unaffected.
    let pushed = |e: &Vector| rejected_at.iter().map(|(r, w)| w * (-(euclid(e, r) / (0.75 * scale)).powi(2)).exp()).fold(0.0f32, f32::max);
    let pull = |i: usize| {
        let aff = (m.tracks[i].aff_long(now).max(0.0) / 3.0).tanh();
        let link = walk.get(&i).copied().unwrap_or(0.0).min(1.0);
        let near = match (m.embedded_at(i), anchor.as_ref()) {
            (Some(e), Some(a)) => (-euclid(e, a) / scale).exp(),
            _ => 0.0,
        };
        // The graph's prior whole: faded with a song's own starts (as the
        // model's feature is, in either of two ways measured) it lifts every
        // song never played over the played ones, and «Знакомое» fills with
        // the unheard — for a played song it is mostly the user's own
        // graph's (what was listened with it), not a stranger's popularity.
        0.6 * aff + link + 0.5 * near + 0.2 * m.graph.prior(i)
    };
    // The moment ([`crate::context`]): a song of this part of this weekday
    // stands higher, and one sounding as the user's music of it — unless a
    // character was chosen by hand, which then leads alone; in a room
    // (a speaker) a song of loudness jumping from a whisper to a blast
    // stands lower.
    let slot = m.slot();
    let free_character = filter.character == Character::Any;
    let room = m.device() == crate::context::PlayDevice::Ambient;
    let moment = |i: usize| {
        let mut c = crate::context::WAVE_TIME_WEIGHT * m.time_bonus_at(i, slot);
        if free_character {
            c += crate::context::WAVE_SOUND_WEIGHT * m.sound_fit_at(i, slot);
            if room && let Some(a) = m.axes_at(i) {
                c -= crate::context::AMBIENT_DYN_PENALTY * (crate::context::AMBIENT_STEADINESS_FLOOR - a.steadiness).clamp(0.0, 1.5);
            }
        }
        c
    };
    let artist_damp = |load: f32| ARTIST_FATIGUE_BASE * load + ARTIST_FATIGUE_EXTRA * (load - ARTIST_FATIGUE_KNEE).max(0.0);
    // A song worn out now (skipped early in most of its last plays) stands
    // lower whatever its love; artist fatigue, skipped sounds/artists, dislike
    // proximity and seeded tie-break jitter damp positive pull multiplicatively
    // so loved artists rotate cleanly without dropping below unheard filler.
    let pulled = |i: usize| {
        let base = pull(i) + moment(i);
        let scaled = if base > 0.0 {
            let ak = lib_artist(i);
            let load = artist_load.get(ak.as_str()).copied().unwrap_or(0.0);
            let skip_art = skipped_artists.get(ak.as_str()).copied().unwrap_or(0.0);
            let fatigue_factor = 1.0 / (1.0 + fatigue_scale * artist_damp(load) + breadth * SKIP_ARTIST_PENALTY * skip_art);
            let push_factor = m.embedded_at(i).map_or(1.0, |e| 1.0 - NEW_PUSH_LIB * pushed(e));
            let dislike_factor = 1.0 - 0.5 * m.taste.dislike_near_at(i);
            let jitter_factor = if turn.seed != 0 {
                1.0 + PULL_JITTER * (Rng::derive(turn.seed, i as u64).f32() - 0.5)
            } else {
                1.0
            };
            base * fatigue_factor * push_factor * dislike_factor * jitter_factor
        } else {
            base
        };
        scaled - WORN_PULL * m.worn(i)
    };
    // One version of a song a session ([`UserModel::work_at`]: an off
    // vocal or a TV size of a song played, queued or served is that song).
    let by_work = part_on(Part::Works);
    let again: HashSet<&str> = turn.again.iter().map(String::as_str).collect();
    let mut works: HashSet<TrackId> = served.iter().chain(turn.again).filter_map(|k| k.parse::<TrackId>().ok()).filter_map(|id| m.position(id)).map(|i| m.work_at(i)).collect();
    works.extend(session.tracks().filter_map(|p| m.position(p.track)).map(|i| m.work_at(i)));
    works.extend(turn.queued.iter().filter_map(|q| if let Ahead::Track(t) = q { m.position(*t) } else { None }).map(|i| m.work_at(i)));
    let mut new_works: HashSet<String> = pool.iter().filter(|c| served.contains(&c.key) || again.contains(c.key.as_str())).map(|c| crate::discover::stream_work_key(&c.track)).collect();
    // A song let in again keeps its own recording's work out of this.
    let again_works: HashSet<String> = pool.iter().filter(|c| again.contains(c.key.as_str())).map(|c| crate::discover::stream_work_key(&c.track)).collect();
    new_works.extend(turn.works.iter().filter(|w| !again_works.contains(*w)).cloned());
    if by_work {
        // One identity of a song across the library and the new songs: a
        // new song served is its library work too, and back.
        works.extend(new_works.iter().filter_map(|k| m.work_of_song(k)));
        new_works.extend(works.iter().filter_map(|w| m.song_of_work(*w)).map(str::to_owned));
    } else {
        works.clear();
        new_works.clear();
    }
    // A recording let in again may play; another version of its song not.
    let mut lib: Vec<(f32, usize)> =
        fitting.iter().copied().filter(|&i| free(i) && (!works.contains(&m.work_at(i)) || again.contains(m.id(i).to_string().as_str()))).map(|i| (pulled(i), i)).collect();
    lib.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut stepper = by_model.then(|| LibStepper::new(ctx, session, turn, &lib, avoid));

    // The pool's: its weight and its links, times the independent seeds
    // that reach it (its linked songs' distinct artists among the seeds).
    let skip_moment = (!pool.is_empty() && share > 0.0).then(|| m.moment_light(session, now));
    let mut ext: Vec<(f32, usize)> = Vec::new();
    let mut ext_work: HashMap<usize, String> = HashMap::new();
    for (k, c) in pool.iter().enumerate() {
        if served.contains(&c.key) {
            continue;
        }
        let emb = c.vector.as_ref().map(|v| m.embed(v));
        // A preview already known to sit next to the user's dislikes is kept
        // out before picking, matching [`still_fits`].
        let dislike = emb.as_ref().map_or(0.0, |e| m.dislike_near_of(e));
        if dislike >= MISFIT_DISLIKE {
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
        let work = crate::discover::stream_work_key(&c.track);
        if new_works.contains(&work) && !again.contains(c.key.as_str()) {
            continue;
        }

        let independent: HashSet<Option<ArtistId>> = c.near.iter().filter(|i| seed_set.contains(i) || walk.contains_key(i)).map(|&i| m.main_artist_at(i)).collect();
        let mut score = (c.weight + 0.5 * link) * crate::discover::multi_seed(independent.len());
        if dislike > 0.0 {
            score *= (1.0 - dislike).max(0.2);
        }
        // Previewed candidates near the session's current sound rise cleanly.
        if let (Some(e), Some(a)) = (emb.as_ref(), anchor.as_ref()) {
            let near = (-euclid(e, a) / scale).exp();
            score *= 0.8 + 0.45 * near;
        }
        // Weigh down new tracks with a high predicted early-skip risk (length,
        // interlude, acoustic jump from the previous song).
        if let Some(mo) = skip_moment.as_ref() {
            let dur = c.track.duration_ms.and_then(|d| u32::try_from(d).ok());
            let p_skip = m.p_skip_new(mo, emb.as_ref(), dur, EventContext::Radio).clamp(0.0, 0.9);
            score *= (1.0 - 0.5 * p_skip).max(0.35);
        }
        // A rejected new song teaches at once: unheard songs of its seeds
        // wait, and its sound's neighbourhood stands lower.
        if c.vector.is_none() && c.near.iter().any(|i| blind_seeds.contains(i)) {
            score *= BLIND_DAMP;
        }
        if !rejected_at.is_empty() {
            let at = emb.or_else(|| centroid(m, &c.near));
            if let Some(e) = at {
                score *= 1.0 - NEW_PUSH * pushed(&e);
            }
        }
        // A new song heard by its preview: how it sounds against the
        // user's music of this moment (no character chosen by hand).
        if free_character && let Some(e) = emb.as_ref() {
            score *= 1.0 + m.sound_fit_of(e, slot);
        }
        let ak = ArtistKey::of_new(ctx, c);
        if ak != ArtistKey::Unknown {
            let label = ak.label(String::new);
            let load = artist_load.get(label.as_str()).copied().unwrap_or(0.0);
            let skip_load = skipped_artists.get(label.as_str()).copied().unwrap_or(0.0);
            let damp = fatigue_scale * artist_damp(load) + breadth * SKIP_ARTIST_PENALTY * skip_load;
            if damp > 0.0 {
                score /= 1.0 + damp;
            }
        }
        if turn.seed != 0 {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            c.key.hash(&mut h);
            let j = Rng::derive(turn.seed ^ 0x9E37_79B9_7F4A_7C15, h.finish()).f32();
            score *= 1.0 + 0.14 * (j - 0.5);
        }
        ext.push((score, k));
        ext_work.insert(k, work);
    }
    ext.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    // Few new songs of the filter left: more are to be looked for.
    let asked = if share >= 1.0 { n } else { ((n as f32 * share).round() as usize).max(usize::from(share > 0.0)) };
    let short = share > 0.0 && ext.len() < asked + NEW_RESERVE;

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
    let ext_artist = |k: usize| ArtistKey::of_new(ctx, &pool[k]).label(|| format!("new:{}", pool[k].key));
    let mut picks: Vec<WavePick> = Vec::with_capacity(want_lib + want_new);
    let (mut took_lib, mut took_new) = (0usize, 0usize);
    let mut per_artist: HashMap<String, usize> = HashMap::new();
    let cap = (n / 3).max(2);
    while took_lib + took_new < want_lib + want_new {
        // The new one's turn when it is behind its even share — except right
        // after a skip, when a familiar song stabilizes the vibe first if one
        // is wanted in this batch.
        let stabilize = turn.after_skip && part_on(Part::Rejected) && took_lib == 0 && took_new == 0 && took_lib < want_lib;
        let new_turn = !stabilize
            && took_new < want_new
            && (took_lib >= want_lib || (took_new as f32 + 0.5) * (want_lib + want_new) as f32 <= want_new as f32 * (took_lib + took_new + 1) as f32);
        let recent: Vec<String> = order.iter().rev().take(avoid).cloned().collect();
        let pick = if new_turn {
            take_next(&mut ext, &ext_artist, &recent, &per_artist, cap).map(|(s, k, a)| {
                if let Some(st) = stepper.as_mut() {
                    st.play_new(Ahead::of_new(ctx, &pool[k]));
                }
                WavePick { item: Item::Stream(pool[k].track.id.clone()), score: s, new: true, artist: a }
            })
        } else if let Some(st) = stepper.as_mut() {
            st.pick(&per_artist, cap, &lib_artist).map(|(s, i, a)| WavePick { item: Item::Track(m.id(i)), score: s, new: false, artist: a })
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
        // Another version of the song picked is not picked after it.
        match &p.item {
            _ if !by_work => {}
            Item::Track(t) => {
                if let Some(w) = m.position(*t).map(|i| m.work_at(i)) {
                    works.insert(w);
                    lib.retain(|x| m.work_at(x.1) != w);
                    if let Some(st) = stepper.as_mut() {
                        st.free.retain(|&i| m.work_at(i) != w);
                    }
                    if let Some(k) = m.song_of_work(w) {
                        ext.retain(|x| ext_work.get(&x.1).map(String::as_str) != Some(k));
                        new_works.insert(k.to_owned());
                    }
                }
            }
            Item::Stream(_) => {
                if let Some(w) = picks_work(&p, pool, &ext_work) {
                    ext.retain(|x| ext_work.get(&x.1) != Some(&w));
                    if let Some(lw) = m.work_of_song(&w) {
                        works.insert(lw);
                        lib.retain(|x| m.work_at(x.1) != lw);
                        if let Some(st) = stepper.as_mut() {
                            st.free.retain(|&i| m.work_at(i) != lw);
                        }
                    }
                    new_works.insert(w);
                }
            }
        }
        if p.new {
            took_new += 1;
        } else {
            took_lib += 1;
        }
        *per_artist.entry(p.artist.clone()).or_default() += 1;
        order.push(p.artist.clone());
        picks.push(p);
    }
    // The new songs it is about to play without a preview: the picked, and
    // the next in line.
    let at: HashMap<&nikgob_core::streaming::RemoteId, usize> = pool.iter().enumerate().map(|(k, c)| (&c.track.id, k)).collect();
    let mut unheard: Vec<usize> = picks
        .iter()
        .filter_map(|p| match &p.item {
            Item::Stream(r) => at.get(r).copied(),
            Item::Track(_) => None,
        })
        .filter(|&k| pool[k].vector.is_none())
        .collect();
    if share > 0.0 {
        unheard.extend(ext.iter().map(|x| x.1).filter(|&k| pool[k].vector.is_none()).take(UNHEARD_AHEAD));
    }
    WaveNext { picks, fitting: fitting.len(), artists: artists.len(), niche, share, unheard, short, seeds: loved }
}

/// The mean embedded sound of the library's songs at `near` (none of them
/// analysed: none).
fn centroid(m: &UserModel, near: &[usize]) -> Option<Vector> {
    let vs: Vec<&Vector> = near.iter().filter_map(|&i| m.embedded_at(i)).collect();
    if vs.is_empty() {
        return None;
    }
    let mut c = [0.0f32; crate::analysis::DIM];
    for v in &vs {
        for (a, b) in c.iter_mut().zip(v.iter()) {
            *a += b;
        }
    }
    for a in &mut c {
        *a /= vs.len() as f32;
    }
    Some(c)
}

/// The rest of an ordinary song for a user whose songs are heard again
/// `habit` after they played as a rule (see [`REST_BASE_MS`]).
pub(crate) fn rest_by_habit(habit: i64) -> i64 {
    let f = (habit.max(1) as f64 / REST_HABIT_MS as f64).sqrt();
    ((REST_BASE_MS as f64 * f) as i64).clamp(REST_RANGE_MS.0, REST_RANGE_MS.1)
}

/// How long the library's song at `i` rests after it played: `rest` (the
/// user's, [`rest_by_habit`]), longer for an old staple and shorter for a
/// song fallen for lately (the share of its love that is of the last
/// days), never below [`RESTED_FLOOR_MS`].
pub(crate) fn rest_of(m: &UserModel, i: usize, rest: i64) -> i64 {
    let now = m.now();
    let t = &m.tracks[i];
    let long = t.aff_long(now).max(0.0);
    let lately = if long > 0.0 { (t.aff_hot(now).max(0.0) / long).min(1.0) } else { 0.0 };
    let f = STAPLE_REST - (STAPLE_REST - FALLEN_REST) * lately;
    ((rest as f32 * f) as i64).max(RESTED_FLOOR_MS)
}

/// The work key of a new song picked ([`crate::discover::stream_work_key`]).
fn picks_work(p: &WavePick, pool: &[OpenCandidate], known: &HashMap<usize, String>) -> Option<String> {
    let Item::Stream(r) = &p.item else { return None };
    let k = pool.iter().position(|c| c.track.id == *r)?;
    Some(known.get(&k).cloned().unwrap_or_else(|| crate::discover::stream_work_key(&pool[k].track)))
}

/// The library's picks of a wave by the user model ([`WAVE_MODEL_RANKING`]),
/// one at a time as a radio picked by the model takes them
/// ([`crate::radio`]): each the best by its value ([`library_value`]) at the
/// moment of the session as if the songs queued and the picks before it
/// played to the end, plus the wave's own pull (the love, the walk of the
/// graph, the sound of the session: measured on the owner's history, the
/// model's value alone lost a tenth of what the wave's picks are listened
/// to, and with the pull beside it most of that came back). The model's
/// candidates of each moment are gathered with the filter's best by the
/// pull ([`GATHER`]), and all are ranked by the one value; the rules of a radio keep them apart (an
/// artist within three, an album twice within ten, a world thrice running)
/// beside the wave's own artist cap. The album's own pull (the next on it,
/// another's first, the same album, the same song, a song of the session)
/// is left out: a wave is no album. A new song placed between them counts
/// as played (its time goes by, the rules see its artist when the library
/// knows it); an artist comes back after one other for a user who often
/// plays an artist twice running ([`ARTIST_AGAIN`]).
struct LibStepper<'a> {
    m: &'a UserModel,
    index: Option<&'a VectorIndex<TrackId>>,
    weights: Weights,
    sim: Session,
    ts: UnixMs,
    /// What the rules see placed before: its artist, album, world.
    seq: Vec<([u64; 1], Option<u64>, Option<u64>)>,
    rules: Diversity,
    /// The filter's free songs by the wave's own pull, the best first.
    by_pull: Vec<usize>,
    /// Each one's pull.
    pull: HashMap<usize, f32>,
    free: HashSet<usize>,
    taken: HashSet<usize>,
    seed: u64,
    /// New songs placed with no artist the library knows.
    unknown: u64,
}

impl<'a> LibStepper<'a> {
    /// `avoid`: the last this many artists are kept out of a pick.
    fn new(ctx: &WaveContext<'a>, session: &Session, turn: WaveTurn<'_>, lib: &[(f32, usize)], avoid: usize) -> Self {
        let m = ctx.model;
        let mut weights = ctx.learned.copied().unwrap_or(m.weights);
        for f in [F_NEXT_ALBUM, F_ALBUM_FIRST, F_IS_PREV, F_IN_SESSION, F_SAME_ALBUM] {
            weights.next.w[f] = 0.0;
        }
        let sim = session.clone();
        let ts = sim.last_ts().unwrap_or(m.now());
        let by_pull: Vec<usize> = lib.iter().map(|x| x.1).collect();
        let free = by_pull.iter().copied().collect();
        let pull = lib.iter().map(|&(p, i)| (i, p)).collect();
        let rules = Diversity { artist_window: avoid + 1, ..Diversity::default() };
        let mut s = Self { m, index: ctx.index, weights, sim, ts, seq: Vec::new(), rules, by_pull, pull, free, taken: HashSet::new(), seed: turn.seed, unknown: 0 };
        let placed: Vec<usize> = s.sim.tracks().filter_map(|p| m.position(p.track)).collect();
        for i in placed {
            s.seq.push(s.seen_of(i));
        }
        for &q in turn.queued {
            match q {
                Ahead::Track(t) => s.play(t),
                Ahead::New(_) => s.play_new(q),
            }
        }
        s
    }

    fn seen_of(&self, i: usize) -> ([u64; 1], Option<u64>, Option<u64>) {
        let a = ArtistKey::of_track(self.m, i);
        let apart = if a == ArtistKey::Unknown { i as u64 } else { 0 };
        ([Self::artist_hash(a, (0, apart))], self.m.release_at(i), self.m.world_at(i))
    }

    /// `a`'s hash; an unknown artist's by `apart`, one of its own.
    fn artist_hash(a: ArtistKey, apart: (u8, u64)) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        a.hash(&mut h);
        if a == ArtistKey::Unknown {
            apart.hash(&mut h);
        }
        h.finish()
    }

    /// A new song of the services plays in the simulated session: its time
    /// goes by, and the rules see its artist ([`ArtistKey`]).
    fn play_new(&mut self, q: Ahead) {
        let Ahead::New(a) = q else { return };
        self.ts += SIM_MS;
        self.unknown += 1;
        self.seq.push(([Self::artist_hash(a, (1, self.unknown))], None, None));
    }

    /// `t` plays to its end in the simulated session.
    fn play(&mut self, t: TrackId) {
        self.ts += SIM_MS;
        self.sim.start(t, self.ts, EventContext::Radio, Some(EventOrigin::Radio));
        self.sim.end(t, Ended::Complete);
        if let Some(i) = self.m.position(t) {
            let seen = self.seen_of(i);
            self.seq.push(seen);
        }
    }

    /// The next pick (its value, position, artist), none of the filter's
    /// free songs being left; `per` the picks of each artist so far in this
    /// call, `cap` the most of one.
    fn pick(&mut self, per: &HashMap<String, usize>, cap: usize, artist: &dyn Fn(usize) -> String) -> Option<(f32, usize, String)> {
        let m = self.m;
        let mut mo = m.moment(&self.sim, m.now().max(self.ts));
        mo.album_session = false;
        mo.world_session = None;
        let push = m.push_probe(&self.sim, self.index);
        let in_sim: HashSet<TrackId> = self.sim.tracks().map(|p| p.track).collect();
        let ok = |i: usize| self.free.contains(&i) && !self.taken.contains(&i) && !in_sim.contains(&m.id(i));
        let mut cands = m.candidates(&mo, &ok);
        cands.extend(self.by_pull.iter().copied().filter(|&i| ok(i)).take(GATHER));
        cands.sort_unstable();
        cands.dedup();
        let from = self.seq.len().saturating_sub(self.rules.album_window);
        let seen: Vec<Seen<'_>> = self.seq[from..].iter().map(|(a, b, w)| (&a[..], *b, *w)).collect();
        let mut best: Option<(usize, f32, usize)> = None;
        for c in cands {
            let value = library_value(m, &mo, &self.weights, c, push.as_ref()) + self.pull.get(&c).copied().unwrap_or(0.0) + WAVE_JITTER * Rng::derive(self.seed, c as u64).f32();
            let (a, b, w) = self.seen_of(c);
            let over = per.get(&artist(c)).copied().unwrap_or(0) >= cap;
            let broken = self.rules.weighted(&seen, (&a[..], b, w)) + 2 * usize::from(over);
            if best.is_none_or(|(k, v, _)| broken < k || (broken == k && value > v)) {
                best = Some((broken, value, c));
            }
        }
        let (_, value, c) = best?;
        self.taken.insert(c);
        self.play(m.id(c));
        Some((value, c, artist(c)))
    }
}

/// The user model's value of the library's song at `i` to a wave at
/// `moment` by `weights` (the album's features already left out of them),
/// as a radio picked by the model weighs it ([`crate::radio`]): the model's
/// relevance, the chance of an early skip (at least what the song's last
/// plays say, [`UserModel::skip_risk`]), the dislikes near it and how worn
/// it is; the session's rejections measured by both sounds when `push`
/// holds them. A radio's bonus for a song rested long is left out:
/// the wave rests a song by its own rule ([`rest_of`]), and the bonus
/// took the songs the user plays again over the ones the user loves now.
pub(crate) fn library_value(m: &UserModel, moment: &Moment, weights: &Weights, i: usize, push: Option<&crate::model::PushProbe<'_>>) -> f32 {
    use crate::radio::{COOL, SKIP_VALUE};
    let mut f = m.features(moment, i);
    if let Some(p) = push {
        f[crate::model::F_SKIP_PUSH] = m.skip_push_by(p, i);
    }
    let relevance = weights.next.score(&f);
    let p_skip = m.skip_risk(i, weights.skip.p(&m.skip_features(moment, i, EventContext::Radio))).min(0.99);
    relevance + SKIP_VALUE * (1.0 - p_skip).ln() - m.taste.dislike_near_at(i) - COOL * m.taste.saturation_at(i)
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

    use crate::eval::synthetic::{Kind, user};
    use crate::model::{F, FEATURES};
    use crate::radio::{RadioSeed, RadioState};
    use nikgob_core::streaming::{ArtistRef, RemoteId, Service};

    fn listener() -> UserModel {
        let p = user(Kind::Omnivore, 5).parts();
        let last = p.input.events.iter().map(|e| e.ts).max().unwrap();
        p.model_at(last + 1)
    }

    /// Weights that care for one feature alone, far over the rest of the
    /// value.
    fn caring_for(model: &UserModel, feature: &str, w: f32) -> Weights {
        let mut weights = model.weights;
        weights.next.w = [0.0; F];
        weights.next.w[FEATURES.iter().position(|f| *f == feature).unwrap()] = w;
        weights
    }

    /// A pool of `n` new songs of new artists without a preview, each
    /// beside one of the library's songs.
    fn pool_of(model: &UserModel, n: usize) -> Vec<OpenCandidate> {
        (0..n)
            .map(|k| {
                let mut track = StreamTrack::new(RemoteId::new(Service::Youtube, format!("n{k}")), format!("New {k}"));
                track.artists.push(ArtistRef::new(None, format!("Artist {k}")));
                OpenCandidate { key: crate::stream_song_key(&track), track, vector: None, embedding: None, lane: None, near: vec![(k * 7) % model.len()], weight: 0.8 }
            })
            .collect()
    }

    /// The wave's library songs are ranked by the user model, by the
    /// weights it was handed: weights that care for liked songs play those
    /// first, weights against them play none of them.
    #[test]
    fn the_library_is_ranked_by_the_model() {
        let model = listener();
        let cands = model.candidates(&model.moment(&Session::default(), model.now()), &|_| true);
        let liked = cands.iter().filter(|&&i| model.tracks[i].liked(model.now())).count();
        assert!(liked >= 8 && liked + 8 <= cands.len(), "{liked} liked of {} candidates", cands.len());
        let (ok, no) = (|_: usize| true, |_: usize| false);
        let none = |_: &StreamTrack| Vec::new();
        let familiar = WaveFilter { novelty: 0.0, ..WaveFilter::default() };
        for (w, want) in [(50.0, true), (-50.0, false)] {
            let weights = caring_for(&model, "liked", w);
            let ctx = WaveContext { model: &model, ok: &ok, genre: None, background: &no, artist_tracks: &none, index: None, learned: Some(&weights) };
            let next = next_ranked(&ctx, &[], &familiar, &Session::default(), 8, &HashSet::new(), true, WaveTurn::default());
            assert_eq!(next.picks.len(), 8);
            for p in &next.picks {
                let Item::Track(id) = p.item else { panic!("a library pick") };
                let i = model.position(id).unwrap();
                assert_eq!(model.tracks[i].liked(model.now()), want, "{w}");
            }
        }
    }

    /// A wave's radio ranks by the weights the runtime attuned it with
    /// when the model ranks the wave ([`WAVE_MODEL_RANKING`]); else the
    /// weights change nothing.
    #[test]
    fn a_wave_ranks_by_the_weights_learnt_since() {
        let model = listener();
        let (ok, no) = (|_: usize| true, |_: usize| false);
        let none = |_: &StreamTrack| Vec::new();
        let ctx = WaveContext { model: &model, ok: &ok, genre: None, background: &no, artist_tracks: &none, index: None, learned: None };
        let picks = |w: f32| {
            let mut radio = RadioState::new(RadioSeed::Wave(crate::wave::WaveSeed::plain(0.0)));
            radio.attune(Session::default(), Some(caring_for(&model, "liked", w)));
            radio.extend_wave(&ctx, &[], 6, 1)
        };
        let against = picks(-50.0);
        assert!(!against.is_empty());
        if model_ranking() {
            assert!(against.iter().all(|p| matches!(p.item, Item::Track(id) if !model.tracks[model.position(id).unwrap()].liked(model.now()))));
        } else {
            assert_eq!(against, picks(50.0));
        }
    }

    /// The new songs a wave is about to play without a preview are told
    /// (the picked and the next in line), and a pool with few new songs
    /// left says it is short, with the filter's songs to look around; a big
    /// pool is not short, and «Знакомое» never is.
    #[test]
    fn a_wave_tells_its_unheard_and_its_shortage() {
        let model = listener();
        let (ok, no) = (|_: usize| true, |_: usize| false);
        let none = |_: &StreamTrack| Vec::new();
        let ctx = WaveContext { model: &model, ok: &ok, genre: None, background: &no, artist_tracks: &none, index: None, learned: None };
        let balance = WaveFilter { novelty: 0.5, ..WaveFilter::default() };
        let small = pool_of(&model, 4);
        let w = next(&ctx, &small, &balance, &Session::default(), 4, &HashSet::new());
        let new: Vec<usize> = w.picks.iter().filter(|p| p.new).filter_map(|p| small.iter().position(|c| Item::Stream(c.track.id.clone()) == p.item)).collect();
        assert_eq!(new.len(), 2);
        assert!(new.iter().all(|k| w.unheard.contains(k)), "{new:?} {:?}", w.unheard);
        assert_eq!(w.unheard.len(), 4, "the picked two and the two left");
        assert!(w.short && !w.seeds.is_empty());
        let big = pool_of(&model, 40);
        let w = next(&ctx, &big, &balance, &Session::default(), 4, &HashSet::new());
        assert!(!w.short);
        assert_eq!(w.unheard.len(), 2 + UNHEARD_AHEAD);
        let w = next(&ctx, &small, &WaveFilter { novelty: 0.0, ..balance }, &Session::default(), 4, &HashSet::new());
        assert!(!w.short && w.unheard.is_empty());
        // The radio of the wave hands it on to the runtime, once: a wave
        // of one taste profile, its pool beside that profile's songs.
        let j = (0..model.profiles.list.len()).max_by_key(|&j| (0..model.len()).filter(|&i| model.profile_at(i) == Some(j)).count()).unwrap();
        let of_j: Vec<usize> = (0..model.len()).filter(|&i| model.profile_at(i) == Some(j)).collect();
        let mut seed = crate::wave::WaveSeed::plain(0.5);
        seed.filter.scene = Scene::Profile(model.profiles.list[j].key);
        let pool: Vec<OpenCandidate> = small.into_iter().zip(&of_j).map(|(c, &i)| OpenCandidate { near: vec![i], ..c }).collect();
        let mut radio = RadioState::new(RadioSeed::Wave(seed));
        radio.attune(Session::default(), None);
        radio.extend_wave(&ctx, &pool, 4, 1);
        let asks = radio.take_wave_asks();
        assert!(!asks.previews.is_empty() && asks.previews.len() <= 6);
        assert!(asks.short.as_ref().is_some_and(|(token, seeds)| !token.is_empty() && !seeds.is_empty()));
        assert_eq!(radio.take_wave_asks(), crate::wave::WaveAsks::default());
    }

    /// A filter's token is what it plays, not its novelty.
    #[test]
    fn a_filters_token_is_its_character_and_scene() {
        let a = WaveFilter { novelty: 0.3, character: Character::Calm, scene: Scene::Profile(7) };
        assert_eq!(a.token(), "calm.p7");
        assert_eq!(WaveFilter { novelty: 0.6, ..a }.token(), a.token());
        assert_ne!(WaveFilter { scene: Scene::Genre(7), ..a }.token(), a.token());
    }

    fn by_model(model: &UserModel, weights: Option<&Weights>, session: &Session, n: usize, turn: WaveTurn<'_>) -> Vec<usize> {
        let (ok, no) = (|_: usize| true, |_: usize| false);
        let none = |_: &StreamTrack| Vec::new();
        let ctx = WaveContext { model, ok: &ok, genre: None, background: &no, artist_tracks: &none, index: None, learned: weights };
        let familiar = WaveFilter { novelty: 0.0, ..WaveFilter::default() };
        next_ranked(&ctx, &[], &familiar, session, n, &HashSet::new(), true, turn)
            .picks
            .iter()
            .map(|p| match p.item {
                Item::Track(id) => model.position(id).unwrap(),
                Item::Stream(_) => panic!("a library pick"),
            })
            .collect()
    }

    /// The model's picks keep a radio's rules: no artist twice within
    /// three picks (two for a user who often plays an artist twice
    /// running), no album a third time within ten, no world thrice
    /// running — the queue before them counted.
    #[test]
    fn the_models_picks_keep_apart() {
        let model = listener();
        let avoid = if model.artist_again(ARTIST_AGAIN_PRIOR) >= ARTIST_AGAIN { 1 } else { 2 };
        let picks = by_model(&model, None, &Session::default(), 12, WaveTurn::default());
        assert_eq!(picks.len(), 12);
        for k in 1..picks.len() {
            let a = model.main_artist_at(picks[k]);
            for j in k.saturating_sub(avoid)..k {
                assert!(a.is_none() || model.main_artist_at(picks[j]) != a, "{picks:?}");
            }
            if k >= 2 {
                let w = model.world_at(picks[k]);
                assert!(w.is_none() || !(model.world_at(picks[k - 1]) == w && model.world_at(picks[k - 2]) == w), "a world thrice running");
            }
        }
        // A song queued: its artist does not come right after it.
        let queued = [Ahead::Track(model.id(picks[0]))];
        let after = by_model(&model, None, &Session::default(), 3, WaveTurn { queued: &queued, seed: 0, ..WaveTurn::default() });
        assert!(!after.contains(&picks[0]), "the queued song is not picked again");
        assert!(model.main_artist_at(picks[0]).is_none_or(|a| model.main_artist_at(after[0]) != Some(a)));
    }

    /// A new song queued (or placed between the model's picks) plays in its
    /// simulated session: the library's next pick is not of its artist.
    #[test]
    fn a_new_song_counts_in_the_models_session() {
        let model = listener();
        let picks = by_model(&model, None, &Session::default(), 1, WaveTurn::default());
        let a = model.main_artist_at(picks[0]).expect("an artist");
        let queued = [Ahead::New(ArtistKey::Library(a))];
        let after = by_model(&model, None, &Session::default(), 2, WaveTurn { queued: &queued, seed: 0, ..WaveTurn::default() });
        assert!(after.iter().all(|&i| model.main_artist_at(i) != Some(a)), "{after:?}");
        // One the library does not know keeps nobody out.
        let unknown = [Ahead::New(ArtistKey::Unknown)];
        assert_eq!(by_model(&model, None, &Session::default(), 1, WaveTurn { queued: &unknown, seed: 0, ..WaveTurn::default() }), picks);
    }

    fn by_pull(model: &UserModel, session: &Session, n: usize, turn: WaveTurn<'_>) -> Vec<usize> {
        let (ok, no) = (|_: usize| true, |_: usize| false);
        let none = |_: &StreamTrack| Vec::new();
        let ctx = WaveContext { model, ok: &ok, genre: None, background: &no, artist_tracks: &none, index: None, learned: None };
        let familiar = WaveFilter { novelty: 0.0, ..WaveFilter::default() };
        next_ranked(&ctx, &[], &familiar, session, n, &HashSet::new(), false, turn)
            .picks
            .iter()
            .map(|p| match p.item {
                Item::Track(id) => model.position(id).unwrap(),
                Item::Stream(_) => panic!("a library pick"),
            })
            .collect()
    }

    /// The wave by its pull keeps the last artist out of its next pick
    /// across its calls too: the queue's last song (or the session's), then
    /// the picks of this call.
    #[test]
    fn the_last_artist_waits_across_the_waves_calls() {
        let model = listener();
        let first = by_pull(&model, &Session::default(), 3, WaveTurn::default());
        let a = model.main_artist_at(first[0]).expect("an artist");
        let queued = [Ahead::Track(model.id(first[0]))];
        let next = by_pull(&model, &Session::default(), 3, WaveTurn { queued: &queued, seed: 0, ..WaveTurn::default() });
        assert_ne!(model.main_artist_at(next[0]), Some(a), "{first:?} {next:?}");
        let mut session = Session::default();
        let t0 = model.now() - 10 * 60_000;
        session.start(model.id(first[0]), t0, EventContext::Radio, Some(EventOrigin::Radio));
        session.end(model.id(first[0]), Ended::Complete);
        let after = by_pull(&model, &session, 3, WaveTurn::default());
        assert_ne!(model.main_artist_at(after[0]), Some(a), "{after:?}");
    }

    /// A library song and a new song of the same artist are one artist to
    /// the wave, either way round and across its calls; a new song of an
    /// artist the library lacks is its artist by name.
    #[test]
    fn a_library_artist_and_their_new_song_are_one_artist() {
        use nikgob_core::streaming::{ArtistRef, RemoteId, Service};
        let model = listener();
        let first = by_pull(&model, &Session::default(), 1, WaveTurn::default())[0];
        let a = model.main_artist_at(first).expect("an artist");
        let of_a: Vec<usize> = (0..model.len()).filter(|&i| model.main_artist_at(i) == Some(a)).collect();
        let song = |k: usize, artist: &str| {
            let mut track = StreamTrack::new(RemoteId::new(Service::Youtube, format!("n{k}")), format!("New {k}"));
            track.artists = vec![ArtistRef { name: artist.to_owned(), id: None }];
            OpenCandidate { key: format!("n{k}"), track, weight: 1.0, near: vec![], vector: None, embedding: None, lane: None }
        };
        let pool = vec![song(0, "Of A"), song(1, "Someone Else"), song(2, "")];
        let tracks_of = |t: &StreamTrack| if t.artists.first().is_some_and(|x| x.name == "Of A") { of_a.clone() } else { Vec::new() };
        let (ok, no) = (|_: usize| true, |_: usize| false);
        let ctx = WaveContext { model: &model, ok: &ok, genre: None, background: &no, artist_tracks: &tracks_of, index: None, learned: None };
        assert_eq!(ArtistKey::of_new(&ctx, &pool[0]), ArtistKey::Library(a));
        assert_eq!(ArtistKey::of_new(&ctx, &pool[1]), ArtistKey::of_new(&ctx, &song(9, "someone else")));
        assert_eq!(ArtistKey::of_new(&ctx, &pool[2]), ArtistKey::Unknown);
        assert_eq!(ArtistKey::of_new(&ctx, &pool[0]).label(String::new), ArtistKey::of_track(&model, first).label(String::new));
        // Library → new: A's new song waits after A's library song.
        let only = WaveFilter { novelty: 1.0, ..WaveFilter::default() };
        let queued = [Ahead::Track(model.id(first))];
        let w = next_ranked(&ctx, &pool, &only, &Session::default(), 1, &HashSet::new(), false, WaveTurn { queued: &queued, seed: 0, ..WaveTurn::default() });
        assert_ne!(w.picks[0].item, Item::Stream(pool[0].track.id.clone()), "A again right after A");
        // New → library: A's library song waits after A's new song queued.
        let queued = [Ahead::of_new(&ctx, &pool[0])];
        let after = by_pull(&model, &Session::default(), 1, WaveTurn { queued: &queued, seed: 0, ..WaveTurn::default() });
        assert_ne!(model.main_artist_at(after[0]), Some(a));
        // New → new across calls: a named artist queued keeps their next song out.
        let queued = [Ahead::of_new(&ctx, &pool[1])];
        let pool2 = vec![song(3, "Someone Else"), song(4, "Third")];
        let w = next_ranked(&ctx, &pool2, &only, &Session::default(), 1, &HashSet::new(), false, WaveTurn { queued: &queued, seed: 0, ..WaveTurn::default() });
        assert_eq!(w.picks[0].item, Item::Stream(pool2[1].track.id.clone()));
    }

    /// One pick at a time: each next pick follows the moment the picks
    /// before it make. Weights that care for the sound alone pick each next
    /// song near the one before it (the session's anchor moves with them),
    /// so the steps are short; songs queued lead the first pick so too.
    #[test]
    fn the_model_picks_one_at_a_time() {
        let model = listener();
        let weights = caring_for(&model, "sound", 20.0);
        let picks = by_model(&model, Some(&weights), &Session::default(), 6, WaveTurn::default());
        let d = |a: usize, b: usize| euclid(model.embedded_at(a).unwrap(), model.embedded_at(b).unwrap()) / model.scale();
        let steps: f32 = picks.windows(2).map(|w| d(w[0], w[1])).sum::<f32>() / (picks.len() - 1) as f32;
        // Random pairs of the library are about one median distance apart.
        assert!(steps < 0.8, "{steps}");
        let far = (0..model.len()).filter(|&i| model.embedded_at(i).is_some()).max_by(|&a, &b| d(a, picks[0]).total_cmp(&d(b, picks[0]))).unwrap();
        let queued = [Ahead::Track(model.id(far))];
        let after = by_model(&model, Some(&weights), &Session::default(), 1, WaveTurn { queued: &queued, seed: 0, ..WaveTurn::default() });
        assert!(d(after[0], far) < d(picks[0], far), "the queue leads the first pick");
    }

    /// A wave started again goes otherwise (the run's seed jitters the
    /// value a little), the same run the same way.
    #[test]
    fn a_wave_started_again_opens_otherwise() {
        let model = listener();
        let start = |seed: u64| -> Vec<usize> { by_model(&model, None, &Session::default(), 12, WaveTurn { queued: &[], seed, ..WaveTurn::default() }) };
        assert_eq!(start(3), start(3));
        let differ = (1..6u64).filter(|&s| start(s) != start(s + 100)).count();
        assert!(differ >= 3, "{differ}");
    }

    /// Two versions of one song (an off vocal, a TV size: one work) never
    /// play in one session of the wave: of the library's, the second is
    /// not picked beside the first, nor after it was served or played;
    /// of the new songs, «Song (Off Vocal)» is «Song».
    #[test]
    fn one_version_of_a_song_a_session() {
        let p = user(Kind::Omnivore, 5).parts();
        let last = p.input.events.iter().map(|e| e.ts).max().unwrap();
        let plain = p.model_at(last + 1);
        let first = by_pull(&plain, &Session::default(), 10, WaveTurn::default());
        // The second pick becomes a version of the first.
        let (a, b) = (plain.id(first[0]), plain.id(first[1]));
        let tracks: Vec<crate::taste::TrackInfo> = p.catalog.tracks().iter().map(|t| if t.id == b { t.clone().with_song(a) } else { t.clone() }).collect();
        let catalog = crate::taste::Catalog::new(tracks);
        let model = UserModel::build(&catalog, &p.before(last + 1), p.model_input(), last + 1);
        assert_eq!(model.work_at(model.position(b).unwrap()), a);
        let picks = by_pull(&model, &Session::default(), 10, WaveTurn::default());
        assert!(picks.contains(&model.position(a).unwrap()), "{picks:?}");
        assert!(!picks.contains(&model.position(b).unwrap()), "both versions in one call: {picks:?}");
        // Served before (a call earlier), or played in the session.
        let served: HashSet<String> = [a.to_string()].into_iter().collect();
        let (ok, no) = (|_: usize| true, |_: usize| false);
        let none = |_: &StreamTrack| Vec::new();
        let ctx = WaveContext { model: &model, ok: &ok, genre: None, background: &no, artist_tracks: &none, index: None, learned: None };
        let familiar = WaveFilter { novelty: 0.0, ..WaveFilter::default() };
        let again = next_ranked(&ctx, &[], &familiar, &Session::default(), 10, &served, false, WaveTurn::default());
        assert!(again.picks.iter().all(|p| p.item != Item::Track(b) && p.item != Item::Track(a)));
        let mut session = Session::default();
        session.start(a, model.now() - 4 * HOUR_MS, EventContext::Radio, Some(EventOrigin::Radio));
        session.end(a, Ended::Complete);
        let after = next_ranked(&ctx, &[], &familiar, &session, 10, &HashSet::new(), false, WaveTurn::default());
        assert!(after.picks.iter().all(|p| p.item != Item::Track(b)), "a version of a song the session played");
        // New songs: one work, two titles.
        let song = |k: usize, title: &str| {
            let mut track = StreamTrack::new(RemoteId::new(Service::Youtube, format!("v{k}")), title);
            track.artists.push(ArtistRef::new(None, "Singer"));
            OpenCandidate { key: crate::stream_song_key(&track), track, vector: None, embedding: None, lane: None, near: vec![first[2]], weight: 1.0 - k as f32 * 0.1 }
        };
        let pool = vec![song(0, "Sora"), song(1, "Sora (Off Vocal)"), song(2, "Umi")];
        let only = WaveFilter { novelty: 1.0, ..WaveFilter::default() };
        let w = next_ranked(&ctx, &pool, &only, &Session::default(), 3, &HashSet::new(), false, WaveTurn::default());
        let ids: Vec<&Item> = w.picks.iter().map(|p| &p.item).collect();
        assert_eq!(ids.len(), 2, "{ids:?}");
        assert!(!ids.contains(&&Item::Stream(pool[1].track.id.clone())));
        let served: HashSet<String> = [pool[0].key.clone()].into_iter().collect();
        let w = next_ranked(&ctx, &pool, &only, &Session::default(), 3, &served, false, WaveTurn::default());
        assert_eq!(w.picks.iter().map(|p| &p.item).collect::<Vec<_>>(), vec![&Item::Stream(pool[2].track.id.clone())]);
    }

    /// A library song and a new song of one work are one song: either
    /// served, the other is not picked after it, nor both in one call.
    #[test]
    fn a_library_song_and_its_new_version_are_one_song() {
        let p = user(Kind::Omnivore, 5).parts();
        let last = p.input.events.iter().map(|e| e.ts).max().unwrap();
        let mut model = p.model_at(last + 1);
        let first = by_pull(&model, &Session::default(), 10, WaveTurn::default());
        let a = model.id(first[0]);
        let song = |k: usize, title: &str| {
            let mut track = StreamTrack::new(RemoteId::new(Service::Youtube, format!("v{k}")), title);
            track.artists.push(ArtistRef::new(None, "Singer"));
            OpenCandidate { key: crate::stream_song_key(&track), track, vector: None, embedding: None, lane: None, near: vec![first[2]], weight: 1.0 - k as f32 * 0.1 }
        };
        let pool = vec![song(0, "Sora (TV Size)"), song(1, "Umi")];
        // The library's track `a` is «Sora» by «Singer».
        model.set_song_works([(crate::discover::stream_work_key(&song(9, "Sora").track), a)]);
        let (ok, no) = (|_: usize| true, |_: usize| false);
        let none = |_: &StreamTrack| Vec::new();
        let ctx = WaveContext { model: &model, ok: &ok, genre: None, background: &no, artist_tracks: &none, index: None, learned: None };
        let only = WaveFilter { novelty: 1.0, ..WaveFilter::default() };
        let familiar = WaveFilter { novelty: 0.0, ..WaveFilter::default() };
        let half = WaveFilter { novelty: 0.5, ..WaveFilter::default() };
        let stream = |i: usize| Item::Stream(pool[i].track.id.clone());
        // Unrelated: the new version may play, the library one leads.
        let w = next_ranked(&ctx, &pool, &only, &Session::default(), 2, &HashSet::new(), false, WaveTurn::default());
        assert!(w.picks.iter().any(|p| p.item == stream(0)));
        // The library's served (an extension before): not its new version.
        let served: HashSet<String> = [a.to_string()].into_iter().collect();
        let w = next_ranked(&ctx, &pool, &only, &Session::default(), 2, &served, false, WaveTurn::default());
        assert!(w.picks.iter().all(|p| p.item != stream(0)), "library then new");
        // Played in the session: the same.
        let mut session = Session::default();
        session.start(a, model.now() - 4 * HOUR_MS, EventContext::Radio, Some(EventOrigin::Radio));
        session.end(a, Ended::Complete);
        let w = next_ranked(&ctx, &pool, &only, &session, 2, &HashSet::new(), false, WaveTurn::default());
        assert!(w.picks.iter().all(|p| p.item != stream(0)), "session then new");
        // The new one served: not the library's.
        let served: HashSet<String> = [pool[0].key.clone()].into_iter().collect();
        let w = next_ranked(&ctx, &pool, &familiar, &Session::default(), 10, &served, false, WaveTurn::default());
        assert!(!w.picks.is_empty());
        assert!(w.picks.iter().all(|p| p.item != Item::Track(a)), "new then library");
        // One call of both kinds: one of the two.
        let w = next_ranked(&ctx, &pool, &half, &Session::default(), 6, &HashSet::new(), false, WaveTurn::default());
        let both = w.picks.iter().any(|p| p.item == Item::Track(a)) && w.picks.iter().any(|p| p.item == stream(0));
        assert!(!both, "{:?}", w.picks);
        assert!(w.picks.iter().any(|p| p.item == Item::Track(a)) || w.picks.iter().any(|p| p.item == stream(0)));
    }

    /// A stream queued or played that the pool no longer holds (the user's
    /// own, or one served before the pool was built again) keeps its work:
    /// no other version of it is picked, new or of the library, in this
    /// call or the next ones; its own recording let in again may play.
    #[test]
    fn a_stream_queued_outside_the_pool_keeps_its_work() {
        let p = user(Kind::Omnivore, 5).parts();
        let last = p.input.events.iter().map(|e| e.ts).max().unwrap();
        let mut model = p.model_at(last + 1);
        let first = by_pull(&model, &Session::default(), 10, WaveTurn::default());
        let a = model.id(first[0]);
        let song = |k: usize, title: &str, artist: &str| {
            let mut track = StreamTrack::new(RemoteId::new(Service::Youtube, format!("v{k}")), title);
            track.artists.push(ArtistRef::new(None, artist));
            OpenCandidate { key: crate::stream_song_key(&track), track, vector: None, embedding: None, lane: None, near: vec![first[2]], weight: 1.0 - k as f32 * 0.1 }
        };
        // Queued by the user, never in the pool: «Sora» by «Singer».
        let queued = crate::discover::stream_work_key(&song(9, "Sora", "Singer").track);
        model.set_song_works([(queued.clone(), a)]);
        let pool = vec![song(0, "Sora (Off Vocal)", "Singer"), song(1, "Umi", "Singer")];
        let (ok, no) = (|_: usize| true, |_: usize| false);
        let none = |_: &StreamTrack| Vec::new();
        let ctx = WaveContext { model: &model, ok: &ok, genre: None, background: &no, artist_tracks: &none, index: None, learned: None };
        let only = WaveFilter { novelty: 1.0, ..WaveFilter::default() };
        let familiar = WaveFilter { novelty: 0.0, ..WaveFilter::default() };
        let works = [queued];
        let turn = WaveTurn { works: &works, ..WaveTurn::default() };
        let w = next_ranked(&ctx, &pool, &only, &Session::default(), 2, &HashSet::new(), false, turn);
        assert!(w.picks.iter().all(|p| p.item != Item::Stream(pool[0].track.id.clone())), "a new version of the queued stream");
        let w = next_ranked(&ctx, &pool, &familiar, &Session::default(), 10, &HashSet::new(), false, turn);
        assert!(!w.picks.is_empty());
        assert!(w.picks.iter().all(|p| p.item != Item::Track(a)), "the library's version of the queued stream");
        // Without it both may play.
        let w = next_ranked(&ctx, &pool, &only, &Session::default(), 2, &HashSet::new(), false, WaveTurn::default());
        assert!(w.picks.iter().any(|p| p.item == Item::Stream(pool[0].track.id.clone())));
    }

    /// A wave's radio keeps the works of the new songs it served once its
    /// pool is built again without them, and of the streams the runtime
    /// tells it the session queued.
    #[test]
    fn a_wave_keeps_the_works_it_served_across_pools() {
        let model = listener();
        let (ok, no) = (|_: usize| true, |_: usize| false);
        let none = |_: &StreamTrack| Vec::new();
        let ctx = WaveContext { model: &model, ok: &ok, genre: None, background: &no, artist_tracks: &none, index: None, learned: None };
        let song = |k: usize, title: &str| {
            let mut track = StreamTrack::new(RemoteId::new(Service::Youtube, format!("w{k}")), title);
            track.artists.push(ArtistRef::new(None, "Singer"));
            OpenCandidate { key: crate::stream_song_key(&track), track, vector: None, embedding: None, lane: None, near: vec![0], weight: 1.0 - k as f32 * 0.1 }
        };
        let mut radio = RadioState::new(RadioSeed::Wave(crate::wave::WaveSeed::plain(1.0)));
        radio.attune(Session::default(), None);
        let first = radio.extend_wave(&ctx, &[song(0, "Sora")], 1, 1);
        assert_eq!(first.len(), 1);
        // The pool built again: the song gone, its off vocal there.
        let next = radio.extend_wave(&ctx, &[song(1, "Sora (Off Vocal)"), song(2, "Umi")], 2, 1);
        assert!(next.iter().all(|p| p.item != Item::Stream(song(1, "x").track.id)), "{next:?}");
        // Told by the runtime: a stream the user queued.
        radio.note_stream_works([crate::discover::stream_work_key(&song(3, "Kaze").track)]);
        let after = radio.extend_wave(&ctx, &[song(4, "Kaze (TV Size)"), song(5, "Hoshi")], 2, 1);
        assert!(after.iter().all(|p| p.item != Item::Stream(song(4, "x").track.id)), "{after:?}");
    }

    /// Picks the runtime did not queue (YouTube Music began to ask to wait
    /// while they were picked) are forgotten: the next extension may pick
    /// them again and the cluster's block has its places back; partly so
    /// too.
    #[test]
    fn picks_not_queued_are_forgotten() {
        let model = listener();
        let (ok, no) = (|_: usize| true, |_: usize| false);
        let none = |_: &StreamTrack| Vec::new();
        let ctx = WaveContext { model: &model, ok: &ok, genre: None, background: &no, artist_tracks: &none, index: None, learned: None };
        let mut radio = RadioState::new(RadioSeed::Wave(crate::wave::WaveSeed::plain(0.0)));
        radio.attune(Session::default(), None);
        let mut twin = radio.clone();
        let first = radio.extend_wave(&ctx, &[], 3, 1);
        assert_eq!(first.len(), 3);
        let left = radio.wave_bandit().unwrap().block_left();
        radio.forget(&first.iter().map(|p| p.item.clone()).collect::<Vec<_>>());
        assert!(radio.served().next().is_none(), "nothing stays served");
        assert!(radio.wave_bandit().unwrap().block_left() >= left);
        let again = radio.extend_wave(&ctx, &[], 3, 1);
        assert!(again.iter().any(|p| first.contains(p)), "the forgotten may come again");
        // One of three dropped: only it leaves the served.
        let picks = twin.extend_wave(&ctx, &[], 3, 1);
        twin.forget(std::slice::from_ref(&picks[1].item));
        let served: Vec<&Item> = twin.served().collect();
        assert_eq!(served, vec![&picks[0].item, &picks[2].item]);
    }

    /// The wave goes on from the queue as the runtime tells it: a song
    /// passed over (waiting for its service) still ahead keeps its artist
    /// out of the next pick, though in the served order it came before the
    /// song that played.
    #[test]
    fn the_wave_goes_on_from_the_queue_as_it_stands() {
        let model = listener();
        let (ok, no) = (|_: usize| true, |_: usize| false);
        let none = |_: &StreamTrack| Vec::new();
        let ctx = WaveContext { model: &model, ok: &ok, genre: None, background: &no, artist_tracks: &none, index: None, learned: None };
        let mut radio = RadioState::new(RadioSeed::Wave(crate::wave::WaveSeed::plain(0.0)));
        radio.attune(Session::default(), None);
        let first = radio.extend_wave(&ctx, &[], 2, 1);
        let (a, b) = (first[0].item.clone(), first[1].item.clone());
        // `b` overtook `a` and played; `a` waits ahead.
        radio.note_played(b);
        radio.set_upcoming(vec![a.clone()]);
        let Item::Track(at) = a else { panic!("a library pick") };
        let artist = model.main_artist_at(model.position(at).unwrap());
        let next = radio.extend_wave(&ctx, &[], 1, 1);
        let Item::Track(n) = next[0].item else { panic!("a library pick") };
        assert!(artist.is_none() || model.main_artist_at(model.position(n).unwrap()) != artist, "the waiting song's artist right after it");
    }

    /// A new song rejected unheard teaches at once: another new song found
    /// beside the same library song stands far lower for the rest of the
    /// session (last of the new picks here), one found elsewhere does not.
    #[test]
    fn a_rejected_unheard_song_damps_its_seeds_finds() {
        let model = listener();
        let (ok, no) = (|_: usize| true, |_: usize| false);
        let none = |_: &StreamTrack| Vec::new();
        let ctx = WaveContext { model: &model, ok: &ok, genre: None, background: &no, artist_tracks: &none, index: None, learned: None };
        let mut pool = pool_of(&model, 6);
        // The first two were found beside one library song.
        pool[1].near = pool[0].near.clone();
        for c in &mut pool {
            c.weight = 0.8;
        }
        let only = WaveFilter { novelty: 1.0, ..WaveFilter::default() };
        let served: HashSet<String> = [pool[0].key.clone()].into_iter().collect();
        let order = |skipped: &[RemoteId]| -> Vec<Item> {
            next_ranked(&ctx, &pool, &only, &Session::default(), 5, &served, false, WaveTurn { skipped_new: skipped, ..WaveTurn::default() }).picks.into_iter().map(|p| p.item).collect()
        };
        let plain = order(&[]);
        let taught = order(std::slice::from_ref(&pool[0].track.id));
        let sibling = Item::Stream(pool[1].track.id.clone());
        assert_eq!(plain.len(), 5);
        assert_eq!(taught.last(), Some(&sibling), "{taught:?}");
        assert!(plain.iter().position(|x| *x == sibling) < taught.iter().position(|x| *x == sibling));
    }

    /// A song rests the user's own time: a song fallen for lately less than
    /// an old staple, never below the floor.
    #[test]
    fn a_song_rests_by_the_users_habit() {
        let model = listener();
        let now = model.now();
        let loved: Vec<usize> = (0..model.len()).filter(|&i| model.tracks[i].aff_long(now) > 0.0).collect();
        let lately = |i: usize| model.tracks[i].aff_hot(now).max(0.0) / model.tracks[i].aff_long(now);
        let fresh = *loved.iter().max_by(|a, b| lately(**a).total_cmp(&lately(**b))).unwrap();
        let staple = *loved.iter().min_by(|a, b| lately(**a).total_cmp(&lately(**b))).unwrap();
        let day = 24 * HOUR_MS;
        assert!(rest_of(&model, fresh, day) < rest_of(&model, staple, day), "{} {}", rest_of(&model, fresh, day), rest_of(&model, staple, day));
        assert!(rest_of(&model, staple, day) <= (day as f32 * STAPLE_REST) as i64);
        assert_eq!(rest_of(&model, fresh, 60_000), RESTED_FLOOR_MS);
        // A user whose songs come back after a day rests them 3 hours, one
        // who plays them again within hours less, one who waits a week more.
        assert_eq!(rest_by_habit(24 * HOUR_MS), 3 * HOUR_MS);
        assert_eq!(rest_by_habit(2 * HOUR_MS), REST_RANGE_MS.0);
        assert_eq!(rest_by_habit(168 * HOUR_MS), REST_RANGE_MS.1);
    }

    /// A new song picked unheard for a calm wave and heard to be loud no
    /// longer fits; one heard calm does; unheard, it fits; a wave of any
    /// character takes the loud one.
    #[test]
    fn a_new_song_heard_is_looked_at_again() {
        let p = user(Kind::Omnivore, 5).parts();
        let last = p.input.events.iter().map(|e| e.ts).max().unwrap();
        let model = p.model_at(last + 1);
        let (ok, no) = (|_: usize| true, |_: usize| false);
        let none = |_: &StreamTrack| Vec::new();
        let ctx = WaveContext { model: &model, ok: &ok, genre: None, background: &no, artist_tracks: &none, index: Some(&p.index), learned: None };
        let calm = WaveFilter { novelty: 0.6, character: Character::Calm, scene: Scene::All };
        let quiet = (0..model.len()).find(|&i| fits_filter(&ctx, &calm, i)).expect("a calm song");
        let loud = (0..model.len()).filter(|&i| model.energy_at(i).is_some()).max_by(|a, b| model.energy_at(*a).unwrap().total_cmp(&model.energy_at(*b).unwrap())).unwrap();
        let heard = |i: usize| {
            let mut c = pool_of(&model, 1).remove(0);
            c.vector = p.index.get(model.id(i)).copied();
            c.near = vec![quiet];
            c
        };
        assert!(still_fits(&ctx, &calm, &heard(quiet)));
        assert!(!still_fits(&ctx, &calm, &heard(loud)));
        assert!(still_fits(&ctx, &calm, &pool_of(&model, 1).remove(0)), "unheard");
        let any = WaveFilter { character: Character::Any, ..calm };
        assert!(still_fits(&ctx, &any, &heard(loud)));
    }

    /// An ordinary radio's new song heard is looked at as the radio picks
    /// one heard: with «Подмешивать случайное» limiting what does not sound
    /// like the seed, one heard far from the seed no longer fits, one near
    /// it does; unheard, or with no limit, it stays.
    #[test]
    fn a_radios_new_song_heard_far_from_its_seed_no_longer_fits() {
        let p = user(Kind::Omnivore, 5).parts();
        let last = p.input.events.iter().map(|e| e.ts).max().unwrap();
        let model = p.model_at(last + 1);
        let (ok, no) = (|_: usize| true, |_: usize| false);
        let none = |_: &StreamTrack| Vec::new();
        let ctx = WaveContext { model: &model, ok: &ok, genre: None, background: &no, artist_tracks: &none, index: Some(&p.index), learned: None };
        let seed = (0..model.len()).find(|&i| model.embedded_at(i).is_some() && p.index.get(model.id(i)).is_some()).unwrap();
        let d = |i: usize| euclid(model.embedded_at(i).unwrap(), model.embedded_at(seed).unwrap());
        let far = (0..model.len()).filter(|&i| model.embedded_at(i).is_some() && p.index.get(model.id(i)).is_some()).max_by(|a, b| d(*a).total_cmp(&d(*b))).unwrap();
        let heard = |i: usize| {
            let mut c = pool_of(&model, 1).remove(0);
            c.vector = p.index.get(model.id(i)).copied();
            c
        };
        let mut radio = RadioState::new(RadioSeed::Track(model.id(seed)));
        assert!(radio.still_fits(&ctx, &heard(far)), "no limit on what does not sound like the seed");
        radio.set_explore(Some(0.2));
        assert!(!radio.still_fits(&ctx, &heard(far)));
        assert!(radio.still_fits(&ctx, &heard(seed)));
        assert!(radio.still_fits(&ctx, &pool_of(&model, 1).remove(0)), "unheard");
    }

    /// The active pull path opens differently on different run seeds and
    /// identically on the same seed.
    #[test]
    fn a_wave_by_pull_opens_otherwise_on_new_seeds() {
        let model = listener();
        let start = |seed: u64| -> Vec<usize> { by_pull(&model, &Session::default(), 10, WaveTurn { seed, ..WaveTurn::default() }) };
        assert_eq!(start(7), start(7));
        let differ = (1..=5u64).filter(|&s| start(s) != start(s + 50)).count();
        assert!(differ >= 3, "expected varied starts across seeds, got {differ}");
    }

    /// Repeated wave plays of an artist in `recent_artists` damp that artist
    /// across calls so other loved artists step in instead of ping-ponging.
    #[test]
    fn cross_call_artist_fatigue_breaks_ping_pong() {
        let model = listener();
        let first = by_pull(&model, &Session::default(), 3, WaveTurn::default());
        let top = model.main_artist_at(first[0]).expect("an artist");
        let top_label = ArtistKey::of_track(&model, first[0]).label(String::new);
        let other = ArtistKey::of_track(&model, first[1]).label(String::new);
        // Simulate top artist already played 5 times in the recent window,
        // NOT in the last two slots so `avoid` alone would pick them again.
        let recent = vec![
            top_label.clone(),
            other.clone(),
            top_label.clone(),
            other.clone(),
            top_label.clone(),
            other.clone(),
            top_label.clone(),
            top_label.clone(),
            "filler-a".to_owned(),
            "filler-b".to_owned(),
        ];
        let next = by_pull(&model, &Session::default(), 1, WaveTurn { recent_artists: &recent, ..WaveTurn::default() });
        assert_ne!(model.main_artist_at(next[0]), Some(top), "fatigued artist should yield to another loved artist");
    }

    /// Skipping a track in the session temporarily cools down its artist even
    /// after two intervening tracks clear the immediate `avoid` window.
    #[test]
    fn a_skipped_artist_cools_down_in_the_session() {
        let model = listener();
        let first = by_pull(&model, &Session::default(), 3, WaveTurn::default());
        let skipped = first[0];
        let a = model.main_artist_at(skipped).expect("an artist");
        let mut session = Session::default();
        let t0 = model.now() - 20 * 60_000;
        session.start(model.id(skipped), t0, EventContext::Radio, Some(EventOrigin::Radio));
        session.end_at(model.id(skipped), Ended::Early, 8_000, model.duration_at(skipped), None);
        // Two other artists played after so `avoid` no longer blocks `a`.
        for (k, &i) in first[1..3].iter().enumerate() {
            session.start(model.id(i), t0 + ((k + 1) as i64) * 200_000, EventContext::Radio, Some(EventOrigin::Radio));
            session.end(model.id(i), Ended::Complete);
        }
        let after = by_pull(&model, &session, 1, WaveTurn::default());
        assert_ne!(model.main_artist_at(after[0]), Some(a), "recently skipped artist should stay cooled down");
    }

    /// A previewed new song next to a user dislike is rejected before picking,
    /// and immediately after a skip a balanced wave stabilizes on a familiar
    /// track before introducing a new one.
    #[test]
    fn preview_near_dislike_is_skipped_and_after_skip_stabilizes_on_familiar() {
        let mut p = user(Kind::Omnivore, 5).parts();
        let last = p.input.events.iter().map(|e| e.ts).max().unwrap();
        let disliked = (0..p.catalog.len()).find(|&i| p.index.get(p.catalog.tracks()[i].id).is_some()).unwrap();
        let disliked_id = p.catalog.tracks()[disliked].id;
        p.input.disliked_tracks.push(disliked_id);
        p.input.dislike_neighbours = true;
        let model = p.model_at(last + 1);
        let (ok, no) = (|_: usize| true, |_: usize| false);
        let none = |_: &StreamTrack| Vec::new();
        let ctx = WaveContext { model: &model, ok: &ok, genre: None, background: &no, artist_tracks: &none, index: Some(&p.index), learned: None };
        let mut bad = pool_of(&model, 1).remove(0);
        bad.vector = p.index.get(disliked_id).copied();
        bad.weight = 10.0;
        let mut good = pool_of(&model, 2).remove(1);
        good.weight = 1.0;
        let only = WaveFilter { novelty: 1.0, ..WaveFilter::default() };
        let picked = next_ranked(&ctx, &[bad.clone(), good.clone()], &only, &Session::default(), 2, &HashSet::new(), false, WaveTurn::default());
        assert_eq!(picked.picks.len(), 1);
        assert_eq!(picked.picks[0].item, Item::Stream(good.track.id.clone()));

        // Right after a skip, a balanced wave with novelty 0.6 opens with a
        // familiar library song instead of dropping a new song on slot 0.
        let more = WaveFilter { novelty: 0.6, ..WaveFilter::default() };
        let pool = pool_of(&model, 6);
        let plain = next_ranked(&ctx, &pool, &more, &Session::default(), 3, &HashSet::new(), false, WaveTurn::default());
        assert!(plain.picks[0].new, "normally novelty 0.6 places a new song first");
        let after = next_ranked(&ctx, &pool, &more, &Session::default(), 3, &HashSet::new(), false, WaveTurn { after_skip: true, ..WaveTurn::default() });
        assert!(!after.picks[0].new, "after a skip, slot 0 stabilizes on a familiar song");
        assert_eq!(after.picks.iter().filter(|x| x.new).count(), plain.picks.iter().filter(|x| x.new).count(), "batch novelty share is preserved");
    }
}
