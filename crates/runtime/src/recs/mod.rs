//! Recommendations in the runtime: the engine of
//! `nikgob-recs` over the library, kept off the worker thread.
//!
//! * **Snapshot.** The [`Catalog`] (library tracks and liked or playlist
//!   stream tracks), the [`UserModel`] of the event log, likes, playlists
//!   and «Не рекомендовать» (its [`Taste`] and what the next start is
//!   scored by), the [`SongTypes`] of the tracks (kept in
//!   `recs.song_types` while the lyrics, the sound and the rules are the
//!   same), and a copy of the sound index without vectors of tracks that
//!   are gone, plus what the discovery filter needs. Built on a
//!   low-priority thread of its own (about half a second for 50 000
//!   tracks): at start, a little after the library changes (likes,
//!   playlists, imports, feedback: [`StreamSchedule::recs_debounce`]),
//!   after 20 plays, when the analysis added many vectors, on a new day and
//!   every [`StreamSchedule::recs_every`] anyway. The worker only swaps the
//!   finished [`Snapshot`] in.
//! * **Home.** The same thread picks «Твой микс» (kept for the day in the
//!   library's cache, `recs.daily`, so it does not change on every look;
//!   «Обновить» picks again), the daylist of the part of the day (kept for
//!   the part in `recs.daylist`, built again when the part changes;
//!   «Обновить» picks again), the bento's tiles (kept for the day in
//!   `recs.tiles`, each only when it has enough songs), the mood cards with
//!   their covers and the style mixes (their centres kept in `recs.styles`
//!   as the next start).
//! * **Mixes and radio** ([`radio`]), **«Новое для тебя» and new releases**
//!   ([`discover`]), **the services' own suggestions** as a signal of taste
//!   ([`suggest`]: matched against the library here, a bonus of affinity
//!   for its tracks and artists, discovery votes for the rest),
//!   **«Не рекомендовать»** with its undo (below), **the niches' top-up**
//!   ([`niche`]) and **the open pool of the waves** ([`open`]).
//! * **The session and the model online.** Every start and its end go on
//!   in the [`Session`] of the moment (what a track's radio reads); a start
//!   the user chose teaches the model of the next start, and every one the
//!   skip model, between builds; their weights are kept in `recs.model`
//!   (learnt from the whole history again when the features change).
//!
//! [`StreamSchedule::recs_debounce`]: crate::StreamSchedule::recs_debounce
//! [`StreamSchedule::recs_every`]: crate::StreamSchedule::recs_every

mod discover;
mod dislikes;
mod graph;
mod lanes;
mod niche;
mod novelty;
mod open;
mod picker;
mod plus;
mod radio;
mod suggest;
mod tiles;
mod wave;
pub(crate) use wave::WAVE_REFILL;
mod top;
mod home;
mod preview;
mod snapshot;
mod worker;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use nikgob_core::i18n::activity::fmt_quoted_in;
use nikgob_core::i18n::{Locale, PerLocale, TileStatic, activity_in, home_recs_in, tracks_count_in};
use nikgob_core::model::{ArtistId, EventContext, ReleaseId, TrackId, UnixMs};
use nikgob_core::streaming::{RemoteId, StreamTrack};
use nikgob_library::{Library, now_ms};
use nikgob_recs::{
    Axes, Catalog, DAYLIST_LEN, DailyParams, DayPart, DiscoveryFilter, Ended, FEATURE_VERSION, Item, ModelInput, Mood,
    NewTrack, Pick, RadioParams, RadioState as Engine, Reason, Recs, SONGTYPE_RULES, ServiceSuggestions, Session, SongInput,
    SongTypes, StyleMix, TasteInput, TrackInfo, UserModel, VectorIndex, Weights, album_key, artist_key, daylist_title_in,
    match_suggestions, song_key, stream_song_key, style_words_in,
};
use nikgob_view::{self as v, Command};
use serde::{Deserialize, Serialize};

use crate::Worker;
use crate::analysis::SoundIndex;
use crate::quiet::lower_priority;
use crate::import::Internal;

pub(crate) use plus::PlusSave;
pub(crate) use radio::RadioSave;
pub(crate) use discover::{DiscoverInput, HOME_NEW_MORE};
pub use tiles::{PreviewTile, preview_ready_tiles, preview_tile_days};

pub(crate) use snapshot::{
    Devices, ReadExtra, play_of, profile_lines, set_own_kind, whole,
};
#[cfg(test)]
use snapshot::{compose_songs, embedding_fingerprint, style_families, work_heads};
use snapshot::{BuildInput, read_snapshot};
pub(crate) use home::{HomeParts, RecsFill, play_lane_mix_title};
#[cfg(test)]
use home::{dedupe_rows, lane_mix_title};
pub use preview::{HomePreview, ModelPreview, preview_home, preview_home_with, preview_replay, preview_replay_with};

/// Plays after which the taste is built again.
const PLAYS_PER_BUILD: u32 = 20;
/// After a build the next one the user did not ask for waits this many
/// times as long as the build took ([`MAX_REST`] at most): a library whose
/// builds are long (a history of years) is not built without a pause while
/// imports and background passes keep changing it.
const REST: u32 = 8;
const MAX_REST: Duration = Duration::from_secs(300);
/// The analysis added this many vectors (or a tenth of the index, or
/// doubled it) since the last build: built again.
const NEW_VECTORS: usize = 200;
/// A sync brought vectors to a device that does not analyse: built again
/// this soon (the pulls of one sync come together).
const SYNCED_SOUND_REST: Duration = Duration::from_secs(60);
/// Library cache keys.
const DAILY_KEY: &str = "recs.daily";
const DAYLIST_KEY: &str = "recs.daylist";
const STYLES_KEY: &str = "recs.styles";
const MODEL_KEY: &str = "recs.model";
const SONG_TYPES_KEY: &str = "recs.song_types";
/// The taste profiles of the builds before ([`nikgob_recs::profiles::ProfileHistory`]).
const PROFILES_KEY: &str = "recs.profiles";
/// Covers of a mix card…
const COVERS: usize = 4;
/// …from this many releases (some have no cover).
const COVER_CANDIDATES: usize = 8;
/// Top tracks of a mood looked at for its covers.
const MOOD_COVER_POOL: usize = 60;
/// The weights learnt online are written at most this often.
const KEEP_WEIGHTS_EVERY: Duration = Duration::from_secs(30);
/// «Миксы для тебя» gets this many more cards at its row's end.
const ROW_MORE: usize = 6;

/// `rec_feedback` target kinds beyond the library's `track` and `artist`:
/// a song of the services by [`song_key`], an artist of the services by
/// [`artist_key`].
pub(crate) const SONG: &str = "song";
pub(crate) const ARTIST_NAME: &str = "artist_name";

/// The engine's inputs at one moment.
pub(crate) struct Snapshot {
    pub catalog: Catalog,
    /// The model of the listener, its taste among it.
    pub model: UserModel,
    pub types: SongTypes,
    /// The sound vectors of catalog tracks only.
    pub index: VectorIndex<TrackId>,
    /// Known songs and artists of the library and the dislikes (discovery
    /// and the new music of radios).
    pub filter: DiscoveryFilter,
    /// Library artists by [`artist_key`] (a stream radio's seed artists).
    pub artist_keys: HashMap<String, ArtistId>,
    /// [`album_key`]s of the library's releases (new releases it has).
    pub known_albums: HashSet<String>,
    /// [`song_key`]s of every track ever played (a discovery that played
    /// is not ignored).
    pub played_songs: HashSet<String>,
    /// Songs the services suggest that the library does not have, new
    /// indeed ([`Snapshot::is_new`]), strongest first.
    pub suggested_new: Vec<(StreamTrack, f32)>,
    /// «Миксы только из скачанного» when it was built: the engine picks
    /// only tracks with a file, and no new music.
    pub local_only: bool,
    /// The worlds of the catalog's tracks by their token: key and title.
    pub worlds: HashMap<u64, (String, String)>,
    /// The analysed previews of the new music found, by [`song_key`]…
    pub new_vectors: HashMap<String, nikgob_recs::Vector>,
    /// …and the neural sound model's vectors of those it heard (the index
    /// keeps the catalog's only; as many as the previews, 260 bytes each).
    pub new_embeddings: HashMap<String, nikgob_recs::Compact>,
    /// The library artists' affinities by [`artist_key`], 0…1 (a known
    /// artist of new music).
    pub artist_affinity: HashMap<String, f32>,
    /// The taste profile of each track and the profiles' shares (the home's
    /// shelves draw from each).
    pub profiles: Arc<TrackProfiles>,
    /// Whether each catalog track suits the background ([`background_fit`]).
    pub background: Vec<bool>,
    /// Songs the graph of similar music found beside the library's that it
    /// lacks, new indeed, the strongest first ([`graph::finds`]).
    pub graph_new: Vec<graph::GraphFind>,
    /// The family of genres of each library artist the tags tell
    /// ([`nikgob_recs::mix_names::leading`]): a wave's scene.
    pub genres: HashMap<ArtistId, u8>,
}

/// The taste profiles as the home reads them.
#[derive(Debug, Default)]
pub struct TrackProfiles {
    pub of: HashMap<TrackId, usize>,
    pub shares: Vec<f32>,
}

impl TrackProfiles {
    fn from_model(model: &UserModel) -> Self {
        Self {
            of: (0..model.len()).filter_map(|i| Some((model.id(i), model.profile_at(i)?))).collect(),
            shares: model.profile_share.clone(),
        }
    }
}


/// A new-music candidate that is new indeed by `f`: not in the library,
/// not disliked, playable.
fn is_new(f: &DiscoveryFilter, t: &StreamTrack) -> bool {
    let key = stream_song_key(t);
    !f.known_ids.contains(&t.id)
        && !f.known_songs.contains(&key)
        && !f.known_songs.contains(&nikgob_recs::discover::stream_work_key(t))
        && !f.disliked_songs.contains(&key)
        && !t.artists.iter().any(|a| f.disliked_artists.contains(&artist_key(&a.name)))
        && t.playable
}

impl Snapshot {
    pub fn recs(&self) -> Recs<'_> {
        Recs::new(&self.catalog, &self.model.taste, &self.index)
            .local_only(self.local_only)
            .with_model(&self.model)
            .with_background(&self.background)
    }

    /// A new-music candidate that is new indeed: not in the library, not
    /// disliked, not junk ([`nikgob_recs::junk`]).
    pub fn is_new(&self, t: &StreamTrack) -> bool {
        is_new(&self.filter, t) && nikgob_recs::junk(t, None).is_none()
    }

    /// A new track as the engine places it: with its analysed preview.
    pub fn new_track(&self, t: &StreamTrack) -> NewTrack {
        NewTrack::from_stream(t, self.new_vectors.get(&stream_song_key(t)).copied())
    }

    /// The neural sound model's vector of a new track's preview, if heard.
    pub fn new_embedding(&self, t: &StreamTrack) -> Option<nikgob_recs::Compact> {
        self.new_embeddings.get(&stream_song_key(t)).copied()
    }
}

/// Where a mix card's cover comes from.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CoverRef {
    Release(ReleaseId),
    Web(String),
}

/// A style mix with its artist names for synchronous in-memory relocalization.
pub(crate) struct StyleHome {
    pub mix: StyleMix,
    pub artist_names: Vec<String>,
    pub title: String,
    pub subtitle: String,
}

impl StyleHome {
    /// Updates `title` and `subtitle` for `loc` synchronously in memory.
    pub fn relocalize(&mut self, loc: Locale) {
        self.title = style_title(&self.artist_names, self.mix.artists.len(), loc);
        self.subtitle = style_subtitle(&self.mix, loc);
    }
}

/// The home's picks of one build.
pub(crate) struct RecHome {
    /// The local day they are for (`YYYY-MM-DD`).
    pub day: String,
    /// The home's epoch they are for.
    pub epoch: String,
    pub daily: Vec<Pick>,
    /// Stream picks of the daily mix, by remote id.
    pub daily_streams: HashMap<RemoteId, StreamTrack>,
    pub daily_covers: Vec<CoverRef>,
    /// Anything listened, liked or put in a playlist.
    pub history: bool,
    /// The six moods in the order of the day part, with covers (empty
    /// without analysis).
    pub moods: Vec<(Mood, Vec<ReleaseId>)>,
    /// The style mixes with their titles, subtitles, and metadata.
    pub styles: Vec<StyleHome>,
    /// The part of the day the daylist is for ([`daylist_key`]).
    pub daylist_key: String,
    /// `None` with an empty library.
    pub daylist: Option<DaylistHome>,
    /// The day's tiles in their order.
    pub tiles: Vec<tiles::TileHome>,
    /// «Миксы для тебя».
    pub lanes: Vec<lanes::LaneHome>,
    /// The taste profiles «Смешать с…» offers.
    pub blend_profiles: Vec<v::BlendProfile>,
    /// All localized titles for each profile in `blend_profiles`.
    blend_profile_titles: Vec<(u32, lanes::PerLocale<String>)>,
    /// The mix tiles of the card at the top, picked only
    /// while the app has the card ([`v::HOME_CARD`]); not on the page until
    /// the card comes back redesigned.
    #[allow(dead_code)]
    pub top: Vec<top::TopMixHome>,
}

impl RecHome {
    /// Returns [`v::BlendProfile`]s rendered in `loc` without touching SQLite.
    pub fn blend_profiles_in(&self, loc: Locale) -> Vec<v::BlendProfile> {
        self.blend_profiles
            .iter()
            .map(|p| {
                let title = self
                    .blend_profile_titles
                    .iter()
                    .find(|(k, _)| *k == p.key)
                    .map_or_else(|| p.title.clone(), |(_, t)| t.get(loc).clone());
                v::BlendProfile { key: p.key, title }
            })
            .collect()
    }

    /// Recomputes every visible localized string in memory (< 0.2 ms, no I/O).
    pub fn relocalize(&mut self, loc: Locale) {
        if let Some(d) = &mut self.daylist {
            d.relocalize(loc);
        }
        for t in &mut self.tiles {
            t.relocalize(loc);
        }
        for l in &mut self.lanes {
            l.relocalize(loc);
        }
        for s in &mut self.styles {
            s.relocalize(loc);
        }
        for p in &mut self.blend_profiles {
            if let Some((_, titles)) = self.blend_profile_titles.iter().find(|(k, _)| *k == p.key) {
                p.title.clone_from(titles.get(loc));
            }
        }
    }
}

/// The daylist of the home.
pub(crate) struct DaylistHome {
    pub part: DayPart,
    pub day: i64,
    pub centre: Option<[f32; 3]>,
    /// «НА ВЕЧЕР» / «EVENING».
    pub kicker: &'static str,
    pub title: String,
    /// «25 треков» / «25 tracks».
    pub subtitle: String,
    /// The radio that plays on after the picks.
    pub state: Engine,
    pub picks: Vec<Pick>,
    pub streams: HashMap<RemoteId, StreamTrack>,
    pub covers: Vec<CoverRef>,
}

impl DaylistHome {
    /// Updates `kicker`, `title`, and `subtitle` synchronously for `loc`.
    pub fn relocalize(&mut self, loc: Locale) {
        self.kicker = daylist_kicker(self.part, loc);
        self.subtitle = tracks_count(self.picks.len(), loc);
        if let Some(c) = self.centre {
            self.title = daylist_title_in(loc, Axes::from_array(c), self.part, self.day);
        } else if let nikgob_recs::RadioSeed::Daylist { centre, .. } = &self.state.seed {
            self.title = daylist_title_in(loc, Axes::from_array(*centre), self.part, self.day);
        }
    }
}

/// What a build hands back.
pub(crate) struct Built {
    snap: Arc<Snapshot>,
    home: RecHome,
    index_len: usize,
    /// [`BuildInput::rerank`].
    rerank: bool,
}

/// From the builder, the radio and the background calls to the worker.
pub(crate) enum Msg {
    Built(Box<Result<Built, String>>),
    Radio(radio::RadioMsg),
    Plus(plus::PlusMsg),
    Discover(discover::DiscoverMsg),
    Suggest(suggest::SuggestMsg),
    Graph(graph::GraphMsg),
    Niche(niche::NicheMsg),
}

impl Msg {
    pub(crate) const fn turn_name(&self) -> &'static str {
        match self {
            Self::Built(_) => "a build came",
            Self::Radio(_) => "the radio",
            Self::Plus(_) => "the guests",
            Self::Discover(_) => "new music",
            Self::Suggest(_) => "suggestions",
            Self::Graph(_) => "the graph",
            Self::Niche(_) => "the niches",
        }
    }
}

/// «Твой микс» of a day as the library's cache keeps it.
#[derive(Serialize, Deserialize)]
struct DailyCache {
    day: String,
    picks: Vec<Pick>,
    /// The stream picks' tracks (they play from the service).
    #[serde(default)]
    streams: Vec<StreamTrack>,
    /// Picked from a history: a mix of random tracks is picked again once
    /// there is one.
    #[serde(default)]
    history: bool,
    /// Picked under «Миксы только из скачанного»: picked again when it
    /// changes.
    #[serde(default)]
    local_only: bool,
    /// When the new music it took was found ([`BuildInput::new_at`]): a
    /// later pass picks it again.
    #[serde(default)]
    new_at: UnixMs,
}

/// The daylist of a part of the day as the library's cache keeps it.
#[derive(Serialize, Deserialize)]
struct DaylistCache {
    /// [`daylist_key`].
    key: String,
    title: String,
    #[serde(default)]
    centre: Option<[f32; 3]>,
    state: Engine,
    picks: Vec<Pick>,
    streams: Vec<StreamTrack>,
    history: bool,
    /// [`DailyCache::local_only`].
    #[serde(default)]
    local_only: bool,
    /// [`DailyCache::new_at`].
    #[serde(default)]
    new_at: UnixMs,
}

/// A day's pick kept in the cache still stands: the new music it took is
/// as new as the build's (under «Миксы только из скачанного» none counts).
fn new_kept(kept_at: UnixMs, new_at: UnixMs, local_only: bool) -> bool {
    local_only || kept_at >= new_at
}

/// The worker's recommendation state.
pub(crate) struct RecsState {
    tx: Sender<Internal>,
    /// The radio's picks and the guests, on one thread of their own.
    picker: picker::Picker,
    snap: Option<Arc<Snapshot>>,
    home: Option<RecHome>,
    building: bool,
    /// A build is due then.
    due: Option<Instant>,
    /// The first change of the library since the last build
    /// ([`Worker::recs_library_changed`]).
    changed_since: Option<Instant>,
    built_at: Option<Instant>,
    /// When the build going on began, and how long the last one took: the
    /// next one waits [`REST`] times that ([`Worker::recs_tick`]).
    build_began: Option<Instant>,
    build_took: Duration,
    /// The user asked for the build that is due: it does not wait.
    urgent: bool,
    built_index_len: usize,
    plays: u32,
    /// The next build picks «Твой микс» again.
    refresh_daily: bool,
    /// The next build picks the daylist again.
    refresh_daylist: bool,
    /// A mix asked for before the first build.
    pending_play: Option<v::MixId>,
    /// «Другой состав»: the next build draws this mix of «Миксы для тебя»
    /// again.
    refresh_mix: Option<v::MixKey>,
    /// Cards of «Миксы для тебя» asked for past its ten (the row scrolled
    /// to its end, [`Worker::recs_more_mixes`]).
    row_more: usize,
    /// The blends made, with the day they are for: they stand first in
    /// «Миксы для тебя» until its end.
    blends: Vec<(String, v::MixKey)>,
    pub(crate) radio: radio::RadioState,
    /// «✦ С дополнением»: the guests of the playlist playing.
    pub(crate) plus: plus::PlusState,
    pub(crate) discover: discover::DiscoverState,
    /// The background pass of the graph of similar music.
    pub(crate) graph: graph::GraphState,
    /// The top-up of the niches of «Миксы для тебя».
    pub(crate) niche: niche::NicheState,
    pub(crate) suggest: suggest::SuggestState,
    pub(crate) dislikes: dislikes::Dislikes,
    pub(crate) newlog: novelty::NewState,
    /// The services' suggestions changed: the discoveries are ranked again
    /// after the next build.
    discover_rerank: bool,
    /// The session of the moment, its starts and how they ended.
    pub(crate) session: Session,
    /// The weights learnt since the snapshot's (`None`: the snapshot's).
    pub(crate) weights: Option<Weights>,
    /// When they were last written, and whether some learnt since wait
    /// ([`Worker::recs_keep_weights`]).
    weights_kept: Option<Instant>,
    weights_unkept: bool,
    /// The skip model's features of the playing track at its start.
    skip_x: Option<(TrackId, [f32; nikgob_recs::model::G])>,
    /// The mixes of the world's page open.
    world: Option<WorldMixes>,
    /// Why a queue entry was picked, in its row's tooltip (bilingual so language switches republish without recomputing).
    pub(crate) why: HashMap<u64, nikgob_core::i18n::PerLocale<String>>,
    /// The number of this run of the app: the home's epochs are its
    /// ([`crate::home_v2::Epoch`]).
    run: u64,
}

/// The mixes of a world's page: its key, and each mix with its invariant
/// Russian context key, localized queue display string, and tracks.
type WorldMixes = (String, Vec<(v::WorldMix, String, String, Vec<TrackId>)>);

impl RecsState {
    /// A discovery request is on its way.
    pub(crate) fn discover_busy(&self) -> bool {
        self.discover.is_busy()
    }

    pub(crate) fn new(tx: Sender<Internal>) -> Self {
        Self {
            tx,
            picker: picker::Picker::default(),
            snap: None,
            home: None,
            building: false,
            due: Some(Instant::now()),
            changed_since: None,
            built_at: None,
            build_began: None,
            build_took: Duration::ZERO,
            urgent: false,
            built_index_len: 0,
            plays: 0,
            refresh_daily: false,
            refresh_daylist: false,
            pending_play: None,
            refresh_mix: None,
            row_more: 0,
            blends: Vec::new(),
            radio: radio::RadioState::default(),
            plus: plus::PlusState::default(),
            discover: discover::DiscoverState::default(),
            graph: graph::GraphState::default(),
            niche: niche::NicheState::default(),
            suggest: suggest::SuggestState::default(),
            dislikes: dislikes::Dislikes::default(),
            newlog: novelty::NewState::default(),
            discover_rerank: false,
            session: Session::default(),
            weights: None,
            weights_kept: None,
            weights_unkept: false,
            skip_x: None,
            world: None,
            why: HashMap::new(),
            run: crate::home_v2::Epoch::run(),
        }
    }
}

/// `NIKGOB_DEMO_DAYS=n` in the demo: the recommendations live `n` days
/// ahead (screenshots of another day's board).
fn day_shift_ms() -> i64 {
    if std::env::var_os("NIKGOB_DEMO").is_none() {
        return 0;
    }
    std::env::var("NIKGOB_DEMO_DAYS")
        .ok()
        .and_then(|d| d.trim().parse::<i64>().ok())
        .map_or(0, |d| d * 24 * 3600 * 1000)
}

/// How much new music a radio of `kind` mixes in: what was chosen for that
/// kind, else «Новое в радио»; none under «Миксы только из скачанного».
pub(crate) fn new_share(prefs: &v::Prefs, kind: v::RadioKind) -> f32 {
    if prefs.mixes_downloaded_only { 0.0 } else { prefs.new_share_of(kind).fraction() }
}

/// Now, for the recommendations.
pub(crate) fn recs_now() -> UnixMs {
    now_ms() + day_shift_ms()
}

/// The home's day begins at 06:00 (a night belongs to the day before).
pub(crate) const DAY_STARTS_HOUR: i64 = 6;

/// The local day of the home (it begins at [`DAY_STARTS_HOUR`]) and the
/// minutes east of UTC.
pub(crate) fn local_day() -> (String, i32) {
    let now = jiff::Zoned::now();
    let shift = day_shift_ms() - DAY_STARTS_HOUR * 3_600_000;
    let day = now.checked_add(jiff::SignedDuration::from_millis(shift)).unwrap_or_else(|_| now.clone());
    (day.date().to_string(), now.offset().seconds() / 60)
}

/// The part of the day at `now` and the key of its daylist: the local day
/// the part belongs to (a night, the day it started) and the part.
pub(crate) fn daylist_key(now: UnixMs, utc_offset_min: i32) -> (DayPart, i64, String) {
    let part = DayPart::at(now, utc_offset_min);
    let day = DayPart::day_of(now, utc_offset_min);
    (part, day, format!("{day}.{part:?}"))
}

/// A number of the day for seeds.
fn day_seed(day: &str) -> u64 {
    day.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, c| (h ^ u64::from(c)).wrapping_mul(0x100_0000_01b3))
}

pub(crate) fn to_view_mood(m: Mood) -> v::Mood {
    match m {
        Mood::Calm => v::Mood::Calm,
        Mood::Energetic => v::Mood::Energetic,
        Mood::Dark => v::Mood::Dark,
        Mood::Bright => v::Mood::Bright,
        Mood::Focus => v::Mood::Focus,
        Mood::Drive => v::Mood::Drive,
    }
}

pub(crate) fn from_view_mood(m: v::Mood) -> Mood {
    match m {
        v::Mood::Calm => Mood::Calm,
        v::Mood::Energetic => Mood::Energetic,
        v::Mood::Dark => Mood::Dark,
        v::Mood::Bright => Mood::Bright,
        v::Mood::Focus => Mood::Focus,
        v::Mood::Drive => Mood::Drive,
    }
}

/// «A, B и другие» / «A, B & more» (two artists: «A и B» / «A & B»; one: its name).
fn style_title(names: &[String], artists: usize, loc: Locale) -> String {
    let s = activity_in(loc);
    match names {
        [] => home_recs_in(loc).your_mix.to_owned(),
        [a] => a.clone(),
        [a, b, ..] if artists > 2 => format!("{a}, {b} {}", s.and_more),
        [a, b, ..] => format!("{a} {} {b}", s.and_last),
    }
}

/// «25 треков» / «25 tracks».
fn tracks_count(n: usize, loc: Locale) -> String {
    tracks_count_in(loc, n as u64)
}

/// Hero subtitle under «Твой микс» / «Your Mix».
fn hero_subtitle(history: bool, n: usize, loc: Locale) -> String {
    let s = activity_in(loc);
    if history {
        format!("{} · {}", tracks_count(n, loc), s.hero_favorites)
    } else {
        s.hero_listen.to_owned()
    }
}

/// What binds a style mix, under its artists' names: «по звуку: спокойное и
/// мрачное» / «sound: calm and dark», or «12 исполнителей похожего звука» /
/// «12 artists with a similar sound» for a style in the middle of everything.
fn style_subtitle(m: &StyleMix, loc: Locale) -> String {
    let s = activity_in(loc);
    match style_words_in(m.axes, loc) {
        Some(words) => format!("{}: {words}", s.by_sound),
        None => {
            let n = m.artists.len();
            let unit = nikgob_core::i18n::plural_forms(loc, n as u64, &nikgob_core::i18n::common_in(loc).artists);
            format!("{n} {unit} {}", s.similar_sound)
        }
    }
}

/// What a mood card plays, in a few words.
pub(crate) const fn mood_subtitle(m: v::Mood, loc: Locale) -> &'static str {
    let s = activity_in(loc);
    match m {
        v::Mood::Calm => s.mood_calm,
        v::Mood::Energetic => s.mood_energetic,
        v::Mood::Dark => s.mood_dark,
        v::Mood::Bright => s.mood_bright,
        v::Mood::Focus => s.mood_focus,
        v::Mood::Drive => s.mood_drive,
    }
}

/// The daylist's kicker: the part of the day it is for.
const fn daylist_kicker(part: DayPart, loc: Locale) -> &'static str {
    let s = activity_in(loc);
    match part {
        DayPart::Morning => s.daylist_morning,
        DayPart::Day => s.daylist_day,
        DayPart::Evening => s.daylist_evening,
        DayPart::Night => s.daylist_night,
    }
}

/// Covers of the picks' releases (stream picks by their urls).
fn covers_of(snap: &Snapshot, picks: &[Pick], streams: &HashMap<RemoteId, StreamTrack>) -> Vec<CoverRef> {
    let mut covers: Vec<CoverRef> = Vec::new();
    for p in picks {
        let c = match &p.item {
            Item::Track(id) => snap.catalog.get(*id).and_then(|t| t.release).map(CoverRef::Release),
            Item::Stream(r) => streams.get(r).and_then(|t| t.cover_url.clone()).map(CoverRef::Web),
        };
        if let Some(c) = c
            && !covers.contains(&c)
        {
            covers.push(c);
            if covers.len() == COVER_CANDIDATES {
                break;
            }
        }
    }
    covers
}

/// A pick still allowed: a catalog track not disliked (with a file, under
/// «Миксы только из скачанного»), or a stream pick that is still new
/// (never under it).
fn still_fits(snap: &Snapshot, p: &Pick, streams: &HashMap<RemoteId, StreamTrack>) -> bool {
    match &p.item {
        Item::Track(id) => snap
            .catalog
            .get(*id)
            .is_some_and(|t| !snap.model.taste.blocks(t) && (t.local || !snap.local_only)),
        Item::Stream(r) => !snap.local_only && streams.get(r).is_some_and(|t| snap.is_new(t)),
    }
}

fn cache_read<T: serde::de::DeserializeOwned>(lib: &Library, key: &str) -> Option<T> {
    lib.cache_get(key).ok().flatten().and_then(|(_, json)| serde_json::from_str(&json).ok())
}

fn cache_write<T: Serialize>(lib: &Library, key: &str, now: UnixMs, value: &T) {
    if let Ok(json) = serde_json::to_string(value)
        && let Err(e) = lib.cache_put(key, now, &json)
    {
        log::warn!("recs: keeping {key}: {e}");
    }
}
