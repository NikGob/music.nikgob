//! Recommendations in the runtime (docs/stage4.md §3–§6): the engine of
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
mod plus;
mod radio;
mod suggest;
mod tiles;
mod wave;
mod top;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use nikgob_core::model::{ArtistId, EventContext, ReleaseId, TrackId, UnixMs};
use nikgob_core::streaming::{RemoteId, StreamTrack};
use nikgob_library::{Library, now_ms};
use nikgob_recs::{
    Catalog, DAYLIST_LEN, DailyParams, DayPart, DiscoveryFilter, Ended, FEATURE_VERSION, Item, ModelInput, Mood,
    NewTrack, Pick, RadioParams, RadioState as Engine, Reason, Recs, SONGTYPE_RULES, ServiceSuggestions, Session, SongInput,
    SongTypes, StyleMix, TasteInput, TrackInfo, UserModel, VectorIndex, Weights, album_key, artist_key, daylist_title,
    match_suggestions, song_key, song_types, stream_song_key, style_words,
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
    /// The style mixes with their titles and subtitles.
    pub styles: Vec<(StyleMix, String, String)>,
    /// The part of the day the daylist is for ([`daylist_key`]).
    pub daylist_key: String,
    /// `None` with an empty library.
    pub daylist: Option<DaylistHome>,
    /// The day's tiles in their order (docs/stage4.md §12.9).
    pub tiles: Vec<tiles::TileHome>,
    /// «Миксы для тебя».
    pub lanes: Vec<lanes::LaneHome>,
    /// The taste profiles «Смешать с…» offers.
    pub blend_profiles: Vec<v::BlendProfile>,
    /// The mix tiles of the card at the top (recs-v2 §7), picked only
    /// while the app has the card ([`v::HOME_CARD`]); not on the page until
    /// the card comes back redesigned.
    #[allow(dead_code)]
    pub top: Vec<top::TopMixHome>,
}

/// The daylist of the home.
pub(crate) struct DaylistHome {
    /// «НА ВЕЧЕР».
    pub kicker: &'static str,
    pub title: String,
    /// «25 треков».
    pub subtitle: String,
    /// The radio that plays on after the picks.
    pub state: Engine,
    pub picks: Vec<Pick>,
    pub streams: HashMap<RemoteId, StreamTrack>,
    pub covers: Vec<CoverRef>,
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
    /// Why a queue entry was picked (recs-v2 §12), in its row's tooltip.
    pub(crate) why: HashMap<u64, String>,
    /// The number of this run of the app: the home's epochs are its
    /// ([`crate::home_v2::Epoch`]).
    run: u64,
}

/// The mixes of a world's page: its key, and each mix with the name the
/// queue says and its tracks.
type WorldMixes = (String, Vec<(v::WorldMix, String, Vec<TrackId>)>);

impl RecsState {
    /// A discovery request is on its way.
    pub(crate) fn discover_busy(&self) -> bool {
        self.discover.is_busy()
    }

    pub(crate) fn new(tx: Sender<Internal>) -> Self {
        Self {
            tx,
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

/// «A, B и другие» (two artists: «A и B»; one: its name).
fn style_title(names: &[String], artists: usize) -> String {
    match names {
        [] => "Твой микс".to_owned(),
        [a] => a.clone(),
        [a, b, ..] if artists > 2 => format!("{a}, {b} и другие"),
        [a, b, ..] => format!("{a} и {b}"),
    }
}

/// «25 треков».
fn tracks_count(n: usize) -> String {
    format!("{n} {}", crate::streams::plural(n, "трек", "трека", "треков"))
}

/// What binds a style mix, under its artists' names: «по звуку: спокойное и
/// мрачное», or «12 исполнителей похожего звука» for a style in the middle
/// of everything.
fn style_subtitle(m: &StyleMix) -> String {
    match style_words(m.axes) {
        Some(words) => format!("по звуку: {words}"),
        None => {
            let n = m.artists.len();
            format!("{n} {} похожего звука", crate::streams::plural(n, "исполнитель", "исполнителя", "исполнителей"))
        }
    }
}

/// What a mood card plays, in a few words.
pub(crate) const fn mood_subtitle(m: v::Mood) -> &'static str {
    match m {
        v::Mood::Calm => "тихое и неспешное",
        v::Mood::Energetic => "быстрое и громкое",
        v::Mood::Dark => "минорное, тёмное по звуку",
        v::Mood::Bright => "мажорное, светлое по звуку",
        v::Mood::Focus => "ровное и негромкое",
        v::Mood::Drive => "громкое и ровное, в дорогу",
    }
}

/// The daylist's kicker: the part of the day it is for.
const fn daylist_kicker(part: DayPart) -> &'static str {
    match part {
        DayPart::Morning => "НА УТРО",
        DayPart::Day => "НА ДЕНЬ",
        DayPart::Evening => "НА ВЕЧЕР",
        DayPart::Night => "НА НОЧЬ",
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

/// What the builder thread needs.
struct BuildInput {
    lib: Library,
    index: SoundIndex,
    day: String,
    /// The home's epoch: «Миксы для тебя» are its.
    epoch: crate::home_v2::Epoch,
    utc_offset_min: i32,
    now: UnixMs,
    refresh_daily: bool,
    refresh_daylist: bool,
    new_share: f32,
    /// «Миксы только из скачанного».
    local_only: bool,
    /// New music for the daily mix (the discoveries).
    new: Vec<StreamTrack>,
    /// When the pass that found them ended (0: none): the day's «Твой
    /// микс» and daylist of older finds are picked again.
    new_at: UnixMs,
    /// Every discovery the pass kept, best first: «С новым» of the cards
    /// draws from them all (and from the graph's finds).
    new_pool: Vec<StreamTrack>,
    /// Where each of them came from, by [`stream_song_key`].
    new_sources: HashMap<String, nikgob_recs::Source>,
    /// The services' suggestions, of the services switched on.
    suggested: Vec<suggest::Kept>,
    /// They changed: the discoveries are ranked again after this build.
    rerank: bool,
    /// [`RecsState::refresh_mix`].
    refresh_mix: Option<v::MixKey>,
    /// [`RecsState::row_more`].
    row_more: usize,
    /// The blends made today.
    blends: Vec<v::MixKey>,
    /// The session of the moment: the order of the mixes follows it.
    session: Session,
    /// The settings of «Миксы для тебя».
    mix_prefs: lanes::MixPrefs,
    extra: ReadExtra,
    /// How new music was received: «С новым» takes its share by it.
    new_log: nikgob_recs::NewLog,
}

fn build(input: BuildInput) -> Result<Built, String> {
    let started = Instant::now();
    let (snap, tiles_input, index_len) =
        read_parts(&input.lib, &input.index, input.now, input.utc_offset_min, &input.suggested, input.local_only, &input.extra)
            .map(|(snap, tiles, len, _)| (snap, tiles, len))?;
    let home = build_home(&snap, &input, &tiles_input);
    log::info!(
        "recs: {} tracks, {} vectors, {} events in {:?}",
        snap.catalog.len(),
        snap.index.len(),
        tiles_input.events.len(),
        started.elapsed()
    );
    Ok(Built {
        snap: Arc::new(snap),
        home,
        index_len,
        rerank: input.rerank,
    })
}

/// The snapshot of the library at `now`, what the tiles read besides it,
/// and the size of the shared sound index it was cut from.
fn read_snapshot(
    lib: &Library,
    index: &SoundIndex,
    now: UnixMs,
    utc_offset_min: i32,
    suggested: &[suggest::Kept],
    local_only: bool,
) -> Result<(Snapshot, nikgob_recs::TilesInput, usize), String> {
    read_parts(lib, index, now, utc_offset_min, suggested, local_only, &ReadExtra::default()).map(|(snap, tiles, len, _)| (snap, tiles, len))
}

/// What a snapshot reads besides the library: the new music found (the
/// analysed previews of it are kept with the snapshot), «Учитывать
/// дизлайки в рекомендациях» and the settings of the taste profiles.
#[derive(Clone)]
pub(crate) struct ReadExtra {
    pub new: Vec<StreamTrack>,
    pub dislike_neighbours: bool,
    pub background_less: bool,
    pub split_by_sound: bool,
    pub names: v::ProfileNames,
    /// The graph of similar music: used at all, the personal graph, the
    /// library as the taste while the history is short.
    pub graph: bool,
    pub personal_graph: bool,
    pub library_prior: bool,
}

impl Default for ReadExtra {
    fn default() -> Self {
        Self::of(Vec::new(), &v::Prefs::default())
    }
}

impl ReadExtra {
    pub(crate) fn of(new: Vec<StreamTrack>, prefs: &v::Prefs) -> Self {
        Self {
            new,
            dislike_neighbours: prefs.dislike_in_recs,
            background_less: prefs.background_counts_less,
            split_by_sound: prefs.profiles_split,
            names: prefs.profile_names,
            graph: prefs.graph_use,
            personal_graph: prefs.graph_personal,
            library_prior: prefs.library_as_taste,
        }
    }
}

/// [`read_snapshot`] and the history its model was built of.
fn read_parts(
    lib: &Library,
    index: &SoundIndex,
    now: UnixMs,
    utc_offset_min: i32,
    suggested: &[suggest::Kept],
    local_only: bool,
    extra: &ReadExtra,
) -> Result<(Snapshot, nikgob_recs::TilesInput, usize, TasteInput), String> {
    let err = |what: &str, e: nikgob_library::LibraryError| format!("{what}: {e}");
    // Where the time goes, stage by stage (a long history makes it felt).
    let mut stage = Instant::now();
    let mut stages: Vec<(&str, u128)> = Vec::new();
    let mut lap = |name: &'static str, stages: &mut Vec<(&str, u128)>| {
        stages.push((name, stage.elapsed().as_millis()));
        stage = Instant::now();
    };
    let rows = lib.rec_tracks().map_err(|e| err("tracks", e))?;
    let mut events = lib.listening_events().map_err(|e| err("events", e))?;
    let mut like_times = lib.liked_track_times().map_err(|e| err("likes", e))?;
    // One song on two albums is one recording: the copy standing for it
    // takes the other copies' plays and likes, and only it is picked.
    let spares = lib.recording_spares().unwrap_or_default();
    let in_library: HashSet<TrackId> = rows.iter().filter(|r| r.in_library).map(|r| r.id).collect();
    let spares: HashMap<TrackId, TrackId> = spares.into_iter().filter(|(_, canon)| in_library.contains(canon)).collect();
    fold_recordings(&spares, &mut events, &mut like_times);
    let saved = lib.saved_tracks().map_err(|e| err("saved songs", e))?;
    let entries = lib.playlist_entry_times().map_err(|e| err("playlists", e))?;
    let feedback = lib.rec_dislikes().map_err(|e| err("feedback", e))?;
    let artists = lib.artist_names().map_err(|e| err("artists", e))?;
    let remote = lib.track_remote_ids().map_err(|e| err("remote ids", e))?;
    let releases = lib.release_names().map_err(|e| err("releases", e))?;
    let release_rows = lib
        .releases(0, 20_000, nikgob_library::Sort::desc(nikgob_library::ReleaseSortKey::Added))
        .map_err(|e| err("albums", e))?;
    let mut artist_keys: HashMap<String, ArtistId> = HashMap::new();
    let mut names: HashMap<ArtistId, String> = HashMap::new();
    for (id, name) in &artists {
        artist_keys.entry(artist_key(name)).or_insert(*id);
        names.insert(*id, name.clone());
    }
    lap("rows", &mut stages);
    let tiles_input = tiles::input(lib, &rows, &events, names.clone());
    lap("tiles", &mut stages);
    // The songs saved in a service's library that the library lacks: the
    // taste of their artists and seeds of the new music, no catalog track.
    let saved_set: HashSet<TrackId> = saved.iter().copied().collect();
    let elsewhere: Vec<&nikgob_library::RecTrack> = rows.iter().filter(|r| !r.in_library && saved_set.contains(&r.id)).collect();
    let mut elsewhere_names: HashMap<ArtistId, String> = HashMap::new();
    for a in elsewhere.iter().flat_map(|r| &r.artists) {
        if !names.contains_key(a) && !elsewhere_names.contains_key(a)
            && let Some(artist) = lib.get_artist(*a).ok().flatten()
        {
            elsewhere_names.insert(*a, artist.name);
        }
    }

    // One song on two albums: the track with a file (the oldest) stands for it.
    let mut songs: HashMap<&str, (bool, TrackId)> = HashMap::new();
    for r in rows.iter().filter(|r| r.in_library && !spares.contains_key(&r.id)) {
        let e = songs.entry(r.song.as_str()).or_insert((r.has_local, r.id));
        if r.has_local && !e.0 {
            *e = (true, r.id);
        }
    }
    // The worlds of Главная: one of them does not fill a mix.
    let album_worlds = crate::home_v2::worlds_of_releases(&release_rows);
    let mut worlds: HashMap<u64, (String, String)> = HashMap::new();
    let catalog = Catalog::new(rows.iter().filter(|r| r.in_library && !spares.contains_key(&r.id)).map(|r| {
        let song = songs.get(r.song.as_str()).map_or(r.id, |s| s.1);
        let world = r.release.and_then(|a| album_worlds.get(&a));
        let mut t = TrackInfo::new(r.id, r.artists.clone(), r.release)
            .with_local(r.has_local)
            .with_song(song)
            .with_duration(tiles_input.facts.get(&r.id).and_then(|f| f.duration_ms))
            .with_position(tiles_input.facts.get(&r.id).map_or(0, |f| f.position));
        let main = r.artists.first().copied();
        if let Some(key) = crate::home_v2::track_world_key(world.map(|w| w.0.as_str()), main) {
            t = t.with_world(&key);
            let token = nikgob_recs::world_token(&key);
            worlds.entry(token).or_insert_with(|| {
                let title = world.map_or_else(|| main.and_then(|a| names.get(&a)).cloned().unwrap_or_default(), |w| w.1.clone());
                (key, title)
            });
        }
        t
    }))
    .with_aliases(spares.iter().map(|(copy, canon)| (*copy, *canon)));

    // «Не рекомендовать»: library tracks and artists, songs and artists of
    // the services (an artist disliked by name is the library's too).
    let mut filter = DiscoveryFilter::default();
    let mut disliked_tracks = Vec::new();
    let mut disliked_artists = Vec::new();
    for f in &feedback {
        match f.target_kind.as_str() {
            "track" => disliked_tracks.extend(f.target_id.parse::<TrackId>().ok()),
            "artist" => disliked_artists.extend(f.target_id.parse::<ArtistId>().ok()),
            SONG => {
                filter.disliked_songs.insert(f.target_id.clone());
            }
            ARTIST_NAME => {
                filter.disliked_artists.insert(f.target_id.clone());
                disliked_artists.extend(artist_keys.get(&f.target_id).copied());
            }
            _ => {}
        }
    }
    for a in &disliked_artists {
        if let Some(n) = names.get(a) {
            filter.disliked_artists.insert(artist_key(n));
        }
    }
    let by_id: HashMap<TrackId, &nikgob_library::RecTrack> = rows.iter().map(|r| (r.id, r)).collect();
    for t in &disliked_tracks {
        if let Some(r) = by_id.get(t) {
            filter.disliked_songs.insert(song_key(&r.title, &r.artist));
        }
    }
    // A song the user deleted from an album is a strong «not for me» for
    // that song (not its artist): new music never brings it back.
    for u in lib.unwanted_songs().unwrap_or_default() {
        filter.disliked_songs.insert(song_key(&u.title, &u.artist));
    }
    filter.known_ids = remote.into_iter().collect();
    filter.known_songs = rows
        .iter()
        .filter(|r| r.in_library)
        .map(|r| song_key(&r.title, &r.artist))
        .collect();
    filter.known_artists = artist_keys.keys().cloned().collect();
    // A song the user keeps elsewhere is no new music, nor its artist.
    for r in &elsewhere {
        filter.known_songs.insert(song_key(&r.title, &r.artist));
        filter.known_ids.extend(lib.external_ids(nikgob_core::streaming::EntityKind::Track, *r.id.as_uuid()).unwrap_or_default());
    }
    filter.known_artists.extend(elsewhere_names.values().map(|n| artist_key(n)));
    let played: HashSet<TrackId> = events.iter().map(|e| e.track_id).collect();
    let played_songs = played
        .iter()
        .filter_map(|t| by_id.get(t))
        .map(|r| song_key(&r.title, &r.artist))
        .collect();
    let known_albums = releases.iter().map(|(t, a)| album_key(t, a)).collect();

    // The services' suggestions: a library track by the service's id or
    // the same song, a library artist by name; the rest is new music.
    let lists: Vec<ServiceSuggestions<'_>> = suggested
        .iter()
        .map(|k| ServiceSuggestions {
            at: k.at,
            tracks: &k.tracks,
            artists: &k.artists,
        })
        .collect();
    let mut by_song: HashMap<String, TrackId> = HashMap::new();
    for r in rows.iter().filter(|r| r.in_library) {
        by_song.entry(song_key(&r.title, &r.artist)).or_insert(r.id);
    }
    let matched = match_suggestions(
        &lists,
        |t| {
            lib.track_by_remote(&t.id)
                .ok()
                .flatten()
                .filter(|id| catalog.position(*id).is_some())
                .or_else(|| by_song.get(&stream_song_key(t)).copied())
        },
        |name| artist_keys.get(&artist_key(name)).copied(),
    );
    let suggested_new: Vec<(StreamTrack, f32)> =
        matched.unmatched.into_iter().filter(|(t, _)| is_new(&filter, t) && nikgob_recs::junk(t, None).is_none()).collect();

    // The analysed previews of the new music found and the model's vectors
    // of them (their tracks are not the catalog's: the snapshot's index
    // drops them below).
    let (new_vectors, new_embeddings) = {
        let shared = index.read();
        let mut vectors: HashMap<String, nikgob_recs::Vector> = HashMap::new();
        let mut embeddings: HashMap<String, nikgob_recs::Compact> = HashMap::new();
        for t in &extra.new {
            let Some(id) = lib.track_by_remote(&t.id).ok().flatten() else { continue };
            let Some(v) = shared.get(id) else { continue };
            let key = stream_song_key(t);
            if let Some(c) = shared.embedding(id) {
                embeddings.insert(key.clone(), c);
            }
            vectors.insert(key, *v);
        }
        (vectors, embeddings)
    };
    // Vectors of tracks that are gone (or left the library) stay in the
    // shared index until a restart; not in the snapshot.
    let mut index = index.read().clone();
    let index_len = index.len();
    let stale: Vec<TrackId> = index.iter().map(|(id, _)| id).filter(|id| catalog.position(*id).is_none()).collect();
    for id in stale {
        index.remove(id);
    }
    lap("catalog", &mut stages);
    let types = read_song_types(lib, &catalog, &index, &tiles_input, &names);
    lap("types", &mut stages);
    let cached: Option<Weights> = lib
        .cache_get(MODEL_KEY)
        .ok()
        .flatten()
        .and_then(|(_, json)| serde_json::from_str(&json).ok())
        .filter(|w: &Weights| w.version == FEATURE_VERSION);
    // The seeks are touches of the plays (how the user listens).
    let seeks = lib.seek_events().unwrap_or_default();
    let kept: Option<nikgob_recs::profiles::ProfileHistory> =
        lib.cache_get(PROFILES_KEY).ok().flatten().and_then(|(_, json)| serde_json::from_str(&json).ok());
    let history = TasteInput {
        events: events.iter().chain(&seeks).map(play_of).collect(),
        playlists: tiles_input.playlists.iter().map(|p| p.tracks.clone()).collect(),
        profiles: nikgob_recs::profiles::Settings {
            background_less: extra.background_less,
            split_by_sound: extra.split_by_sound,
            history: kept.clone(),
        },
        likes: like_times.iter().map(|l| l.0).collect(),
        saved,
        saved_elsewhere: elsewhere.iter().map(|r| (r.id, r.artists.clone())).collect(),
        like_times,
        playlist_entries: entries,
        disliked_tracks,
        disliked_artists,
        utc_offset_min,
        suggested_tracks: matched.tracks,
        suggested_artists: matched.artists,
        dislike_neighbours: extra.dislike_neighbours,
        personal_graph: extra.personal_graph,
        library_prior: extra.library_prior,
    };
    // The graph of similar music the library keeps, resolved to it now.
    let similar = if extra.graph { graph::read(lib, &rows, &artist_keys) } else { graph::Resolved::default() };
    lap("graph", &mut stages);
    let model = UserModel::build(
        &catalog,
        &history,
        ModelInput { index: &index, types: Some(&types), weights: cached, graph: (!similar.input.is_empty()).then_some(&similar.input) },
        now,
    );
    lap("model", &mut stages);
    log::debug!(
        "recs: snapshot of {} events: {}",
        history.events.len(),
        stages.iter().map(|(n, ms)| format!("{n} {ms} ms")).collect::<Vec<_>>().join(", ")
    );
    if cached.is_none()
        && let Ok(json) = serde_json::to_string(&model.weights)
        && let Err(e) = lib.cache_put(MODEL_KEY, now, &json)
    {
        log::warn!("recs: cannot keep the model: {e}");
    }
    if kept.as_ref() != Some(&model.profiles.history)
        && let Ok(json) = serde_json::to_string(&model.profiles.history)
        && let Err(e) = lib.cache_put(PROFILES_KEY, now, &json)
    {
        log::warn!("recs: cannot keep the profiles: {e}");
    }
    log_profiles(&model, &tiles_input, &names, extra.names);
    let artist_affinity: HashMap<String, f32> = model
        .taste
        .all_artist_affinities()
        .filter_map(|(a, s)| Some((artist_key(names.get(&a).or_else(|| elsewhere_names.get(&a))?), (s.max(0.0) / 3.0).tanh())))
        .collect();
    // The artists beyond the library the graph links to the loved ones:
    // their new music counts as a little known.
    let artist_affinity = graph::with_votes(artist_affinity, &similar.votes, &|a| (model.taste.artist_affinity(a).max(0.0) / 3.0).tanh());
    // The songs the graph found beside the library's: new music, by how
    // loved the tracks whose radios hold them are.
    let graph_new = graph::finds(
        &similar.beyond,
        &|t| is_new(&filter, t) && nikgob_recs::junk(t, None).is_none(),
        &|t| model.position(t).map(|i| (model.taste.affinity_at(i).max(0.0) / 3.0).tanh()),
        &|t| model.position(t).and_then(|i| model.main_artist_at(i)),
    );
    let genres = {
        let mut tags: HashMap<ArtistId, Vec<(String, f32)>> = HashMap::new();
        for (a, t, w) in &similar.input.tags {
            tags.entry(*a).or_default().push((t.clone(), *w));
        }
        tags.into_iter().filter_map(|(a, t)| Some((a, nikgob_recs::mix_names::leading(&t)?))).collect()
    };
    let mut snap = Snapshot {
        genres,
        graph_new,
        background: Vec::new(),
        profiles: Arc::new(TrackProfiles::from_model(&model)),
        new_vectors,
        new_embeddings,
        artist_affinity,
        catalog,
        model,
        types,
        index,
        filter,
        artist_keys,
        known_albums,
        played_songs,
        suggested_new,
        local_only,
        worlds,
    };
    snap.background = background_fit(&snap, &tiles_input, &events);
    Ok((snap, tiles_input, index_len, history))
}

/// Whether each catalog track suits the background
/// ([`nikgob_recs::background::Basic`]): by its type and names, and
/// whether it is in a playlist the user plays in their background sessions
/// ([`nikgob_recs::background::background_playlists`]: the plays from it,
/// by the history's `context_id`, else every playlist of the track played
/// from a playlist before the history had one).
pub(crate) fn background_fit(snap: &Snapshot, tiles: &nikgob_recs::TilesInput, events: &[nikgob_core::model::Event]) -> Vec<bool> {
    use nikgob_recs::background::{BackgroundScore, BackgroundTrack, Basic};
    let (kept, holding) = played_in_background(snap, tiles, events);
    if !kept.is_empty() {
        let names: Vec<&str> = kept.iter().filter_map(|&k| tiles.playlists.get(k)).map(|p| p.name.as_str()).collect();
        log::info!("recs: played in the background: {}", names.join(", "));
    }
    snap.catalog
        .tracks()
        .iter()
        .map(|t| {
            let (Some(song), Some(f)) = (snap.types.get(t.id), tiles.facts.get(&t.id)) else { return false };
            let background_playlist = holding.get(&t.id).is_some_and(|l| l.iter().any(|k| kept.contains(k)));
            Basic.suits(&BackgroundTrack { song, title: &f.title, album: f.album.as_deref(), artist: &f.artist, background_playlist })
        })
        .collect()
}

/// The playlists of `tiles` the user plays in the background (their
/// places), and the playlists of each track.
fn played_in_background(
    snap: &Snapshot,
    tiles: &nikgob_recs::TilesInput,
    events: &[nikgob_core::model::Event],
) -> (HashSet<usize>, HashMap<TrackId, Vec<usize>>) {
    use nikgob_core::model::EventType;
    let by_key: HashMap<&str, usize> = tiles.playlists.iter().enumerate().map(|(k, p)| (p.key.as_str(), k)).collect();
    let mut holding: HashMap<TrackId, Vec<usize>> = HashMap::new();
    for (k, p) in tiles.playlists.iter().enumerate() {
        for t in &p.tracks {
            holding.entry(*t).or_default().push(k);
        }
    }
    let mut plays: Vec<(usize, UnixMs)> = Vec::new();
    for e in events.iter().filter(|e| e.event_type == EventType::Start) {
        match e.context_id.as_deref().and_then(|c| c.strip_prefix("playlist:")) {
            Some(id) => plays.extend(by_key.get(id).map(|&k| (k, e.ts))),
            None if e.context == EventContext::Playlist && e.context_id.is_none() => {
                plays.extend(holding.get(&e.track_id).into_iter().flatten().map(|&k| (k, e.ts)));
            }
            None => {}
        }
    }
    (nikgob_recs::background::background_playlists(&plays, &snap.model.listening), holding)
}

/// One song on two albums ([`Library::recording_spares`]: each other copy
/// with the copy standing for it): the plays of a copy are the standing
/// copy's, and its like is that one's (once, the newest time).
pub(crate) fn fold_recordings(
    spares: &HashMap<TrackId, TrackId>,
    events: &mut [nikgob_core::model::Event],
    likes: &mut Vec<(TrackId, UnixMs)>,
) {
    if spares.is_empty() {
        return;
    }
    for e in events.iter_mut() {
        if let Some(c) = spares.get(&e.track_id) {
            e.track_id = *c;
        }
    }
    for l in likes.iter_mut() {
        if let Some(c) = spares.get(&l.0) {
            l.0 = *c;
        }
    }
    // Newest first: the first of each track stays.
    likes.sort_by_key(|l| std::cmp::Reverse(l.1));
    let mut seen = HashSet::new();
    likes.retain(|l| seen.insert(l.0));
}

/// An event as the recommendations read it: a play imported before
/// imports had their origin (the import's device) is an import.
pub(crate) fn play_of(e: &nikgob_core::model::Event) -> nikgob_recs::Play {
    let mut p = nikgob_recs::Play::from(e);
    if p.origin.is_none() && e.device.as_str() == crate::streams::IMPORT_DEVICE {
        p.origin = Some(nikgob_core::model::EventOrigin::Import);
    }
    p
}

/// The taste profiles of a build in the log (profiles-v1 §11.4): a line
/// of them all, then each, the largest share first — its name and artists,
/// its share of the recent plays, how much of it is background, three of
/// its best loved songs; and how the user listens.
fn log_profiles(model: &UserModel, tiles: &nikgob_recs::TilesInput, names: &HashMap<ArtistId, String>, style: v::ProfileNames) {
    log::info!("{}", listening_line(model));
    let lines = profile_lines(model, tiles, names, style);
    let short: Vec<String> = profile_order(model).into_iter().map(|j| format!("{} {:.2}", profile_name(model, names, style, j).0, model.profile_share.get(j).copied().unwrap_or(0.0))).collect();
    let born = model.profiles.list.iter().filter(|p| !p.visible).count();
    log::info!("profiles: {} ({}), not shown yet {born}", model.profiles.len(), short.join(", "));
    for line in lines {
        log::info!("recs: profile {line}");
    }
}

/// How the user listens, in a line: the sessions, the background ones and
/// the threshold that told them.
fn listening_line(model: &UserModel) -> String {
    let l = &model.listening;
    format!(
        "listening: {} sessions, {} background{}",
        l.sessions.len(),
        l.background_sessions(),
        match (l.has_background, l.threshold) {
            (false, _) => " (no background listening)".to_owned(),
            (true, Some(t)) => format!(" (threshold {t:.2}, separation {:.2})", l.eta),
            (true, None) => format!(" (the fixed band, separation {:.2})", l.eta),
        }
    )
}

/// The profiles, the largest share first.
fn profile_order(model: &UserModel) -> Vec<usize> {
    let mut order: Vec<usize> = (0..model.profiles.len()).collect();
    order.sort_by(|a, b| model.profile_share[*b].total_cmp(&model.profile_share[*a]).then(a.cmp(b)));
    order
}

/// The name and the subtitle of profile `j`.
fn profile_name(model: &UserModel, names: &HashMap<ArtistId, String>, style: v::ProfileNames, j: usize) -> (String, String) {
    let style = match style {
        v::ProfileNames::Kind => nikgob_recs::profiles::NameStyle::Kind,
        v::ProfileNames::Artists => nikgob_recs::profiles::NameStyle::Artists,
    };
    model.profiles.name(j, style, &|a| names.get(&a).cloned())
}

/// [`log_profiles`]' lines of each profile.
pub(crate) fn profile_lines(model: &UserModel, tiles: &nikgob_recs::TilesInput, names: &HashMap<ArtistId, String>, style: v::ProfileNames) -> Vec<String> {
    let now = model.now();
    let mut out = Vec::new();
    for j in profile_order(model) {
        let p = &model.profiles.list[j];
        let mut members: Vec<usize> = (0..model.len()).filter(|&i| model.profile_at(i) == Some(j) && model.tracks[i].aff_long(now) > 0.0).collect();
        members.sort_by(|a, b| model.tracks[*b].aff_long(now).total_cmp(&model.tracks[*a].aff_long(now)).then(a.cmp(b)));
        let songs: Vec<String> = members
            .iter()
            .take(3)
            .filter_map(|&i| tiles.facts.get(&model.id(i)).map(|f| format!("«{}» {}", f.title, f.artist)))
            .collect();
        let (name, sub) = profile_name(model, names, style, j);
        out.push(format!(
            "{name} ({sub}): {:?}, key {}{}, {:.0} % of the plays, background {:.2}, {} members; {}",
            p.lane,
            p.key,
            if p.visible { "" } else { ", not shown yet" },
            100.0 * model.profile_share.get(j).copied().unwrap_or(0.0),
            model.profile_bg.get(j).copied().unwrap_or(0.0),
            p.members,
            songs.join(", ")
        ));
    }
    out
}

/// What `recs.song_types` keeps: the types and what they were told from.
#[derive(Serialize, Deserialize)]
struct SongTypesCache {
    rules: u32,
    /// [`fingerprint`] of the inputs.
    of: u64,
    types: SongTypes,
}

/// A stable hash of everything the song types are told from.
fn fingerprint(inputs: &[SongInput]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h = (h ^ u64::from(*b)).wrapping_mul(0x100_0000_01b3);
        }
        h = (h ^ 0xff).wrapping_mul(0x100_0000_01b3);
    };
    for t in inputs {
        eat(t.id.as_uuid().as_bytes());
        eat(t.main_artist.map(|a| *a.as_uuid().as_bytes()).unwrap_or_default().as_slice());
        eat(t.artist_name.as_bytes());
        eat(t.title.as_bytes());
        eat(t.credit.as_bytes());
        eat(t.album.as_deref().unwrap_or_default().as_bytes());
        eat(&[u8::from(t.ost), u8::from(t.synced_lyrics)]);
        match &t.lyrics {
            Some((flag, text)) => eat(format!("{flag}{text}").as_bytes()),
            None => eat(b"-"),
        }
        if let Some(z) = &t.z {
            for x in z {
                eat(&x.to_le_bytes());
            }
        }
    }
    h
}

/// The types of the catalog's tracks: kept in `recs.song_types` while
/// their lyrics, sound, names and the rules are the same.
fn read_song_types(
    lib: &Library,
    catalog: &Catalog,
    index: &VectorIndex<TrackId>,
    tiles: &nikgob_recs::TilesInput,
    names: &HashMap<ArtistId, String>,
) -> SongTypes {
    let samples = lib.lyrics_samples().unwrap_or_else(|e| {
        log::warn!("recs: lyrics for the song types: {e}");
        Vec::new()
    });
    let lyrics: HashMap<TrackId, (bool, String)> = samples.into_iter().map(|s| (s.id, (s.instrumental, s.text))).collect();
    let norm = index.normalizer();
    let mut inputs: Vec<SongInput> = catalog
        .tracks()
        .iter()
        .map(|t| {
            let f = tiles.facts.get(&t.id);
            let main = t.artists.first().copied();
            let album = f.and_then(|f| f.album.clone());
            SongInput {
                id: t.id,
                main_artist: main,
                artist_name: main.and_then(|a| names.get(&a)).cloned().unwrap_or_default(),
                title: f.map(|f| f.title.clone()).unwrap_or_default(),
                credit: f.map(|f| f.artist.clone()).unwrap_or_default(),
                ost: album.as_deref().and_then(nikgob_recs::tiles::franchise).is_some(),
                album,
                lyrics: lyrics.get(&t.id).cloned(),
                synced_lyrics: f.is_some_and(|f| matches!(f.lyrics, nikgob_recs::LyricsKind::Synced | nikgob_recs::LyricsKind::Words)),
                z: index.get(t.id).map(|v| norm.z(v)),
            }
        })
        .collect();
    inputs.sort_by_key(|t| t.id);
    let of = fingerprint(&inputs);
    let cached: Option<SongTypesCache> = lib
        .cache_get(SONG_TYPES_KEY)
        .ok()
        .flatten()
        .and_then(|(_, json)| serde_json::from_str(&json).ok())
        .filter(|c: &SongTypesCache| c.rules == SONGTYPE_RULES && c.of == of);
    if let Some(c) = cached {
        return c.types;
    }
    let types = song_types(&inputs);
    let cache = SongTypesCache { rules: SONGTYPE_RULES, of, types };
    if let Ok(json) = serde_json::to_string(&cache)
        && let Err(e) = lib.cache_put(SONG_TYPES_KEY, now_ms(), &json)
    {
        log::warn!("recs: cannot keep the song types: {e}");
    }
    cache.types
}

fn build_home(snap: &Snapshot, input: &BuildInput, tiles_input: &nikgob_recs::TilesInput) -> RecHome {
    let lib = &input.lib;
    let recs = snap.recs();

    // «Твой микс»: the day's, unless it is a new day or «Обновить».
    let cached: Option<DailyCache> = lib
        .cache_get(DAILY_KEY)
        .ok()
        .flatten()
        .and_then(|(_, json)| serde_json::from_str(&json).ok())
        .filter(|c: &DailyCache| {
            c.day == input.day
                && !input.refresh_daily
                && (c.history || !snap.model.taste.has_history())
                && c.local_only == input.local_only
                && new_kept(c.new_at, input.new_at, input.local_only)
        });
    let (picks, streams) = match cached {
        Some(c) => (c.picks, c.streams),
        None => {
            let new: Vec<&StreamTrack> = input.new.iter().filter(|t| !input.local_only && snap.is_new(t)).collect();
            let candidates: Vec<NewTrack> = new.iter().map(|t| snap.new_track(t)).collect();
            let seed = if input.refresh_daily { fastrand::u64(..) } else { day_seed(&input.day) };
            let picks = recs.daily_mix(
                &candidates,
                &DailyParams {
                    new_share: input.new_share,
                    seed,
                    ..DailyParams::default()
                },
            );
            let streams: Vec<StreamTrack> = new
                .into_iter()
                .filter(|t| picks.iter().any(|p| p.item == Item::Stream(t.id.clone())))
                .cloned()
                .collect();
            let cache = DailyCache {
                day: input.day.clone(),
                picks: picks.clone(),
                streams: streams.clone(),
                history: snap.model.taste.has_history(),
                local_only: input.local_only,
                new_at: input.new_at,
            };
            if let Ok(json) = serde_json::to_string(&cache)
                && let Err(e) = lib.cache_put(DAILY_KEY, input.now, &json)
            {
                log::warn!("recs: keeping the daily mix: {e}");
            }
            (picks, streams)
        }
    };
    let daily_streams: HashMap<RemoteId, StreamTrack> = streams.into_iter().map(|t| (t.id.clone(), t)).collect();
    // A pick disliked since (or gone) leaves the day's mix; its undo brings
    // it back.
    let daily: Vec<Pick> = picks.into_iter().filter(|p| still_fits(snap, p, &daily_streams)).collect();
    let daily_covers = covers_of(snap, &daily, &daily_streams);
    let (daylist_key, daylist) = build_daylist(snap, input);
    let tiles = tiles::build(snap, input, tiles_input);
    let (lanes, blend_profiles) = lanes::build(snap, input, tiles_input);
    let top = if v::HOME_CARD { top::build(snap, input, tiles_input, None) } else { Vec::new() };

    // Moods: covers of the releases most liked among each mood's top.
    let mut moods = Vec::new();
    if !snap.index.is_empty() {
        for m in recs.mood_order() {
            let mut top: Vec<(TrackId, f32)> = recs.mood_pool(m).into_iter().take(MOOD_COVER_POOL).collect();
            let aff = |id: TrackId| snap.catalog.position(id).map_or(0.0, |i| snap.model.taste.affinity_at(i));
            top.sort_by(|a, b| aff(b.0).total_cmp(&aff(a.0)));
            let mut covers: Vec<ReleaseId> = Vec::new();
            for (id, _) in top {
                if let Some(r) = snap.catalog.get(id).and_then(|t| t.release)
                    && !covers.contains(&r)
                {
                    covers.push(r);
                    if covers.len() == COVER_CANDIDATES {
                        break;
                    }
                }
            }
            moods.push((m, covers));
        }
    }

    // Style mixes from the last centres.
    let previous: Vec<Vec<f32>> = lib
        .cache_get(STYLES_KEY)
        .ok()
        .flatten()
        .and_then(|(_, json)| serde_json::from_str(&json).ok())
        .unwrap_or_default();
    let mixes = recs.style_mixes(&previous, day_seed(&input.day));
    if !mixes.is_empty() {
        let centres: Vec<&Vec<f32>> = mixes.iter().map(|m| &m.centroid).collect();
        if let Ok(json) = serde_json::to_string(&centres) {
            let _ = lib.cache_put(STYLES_KEY, input.now, &json);
        }
    }
    let styles = mixes
        .into_iter()
        .map(|m| {
            let names: Vec<String> = m
                .name_artists
                .iter()
                .filter_map(|a| lib.get_artist(*a).ok().flatten().map(|a| a.name))
                .collect();
            let title = style_title(&names, m.artists.len());
            let subtitle = style_subtitle(&m);
            (m, title, subtitle)
        })
        .collect();

    RecHome {
        day: input.day.clone(),
        epoch: input.epoch.key.clone(),
        daily,
        daily_streams,
        daily_covers,
        history: snap.model.taste.has_history(),
        moods,
        styles,
        daylist_key,
        daylist,
        tiles,
        lanes,
        blend_profiles,
        top,
    }
}

/// The daylist of now's part of the day: the one kept for this part,
/// unless «Обновить» or a history appeared since.
fn build_daylist(snap: &Snapshot, input: &BuildInput) -> (String, Option<DaylistHome>) {
    let lib = &input.lib;
    let history = snap.model.taste.has_history();
    let (part, day, key) = daylist_key(input.now, input.utc_offset_min);
    let cached = cache_read::<DaylistCache>(lib, DAYLIST_KEY)
        .filter(|c| c.key == key && !input.refresh_daylist && (c.history || !history) && c.local_only == input.local_only && new_kept(c.new_at, input.new_at, input.local_only));
    let c = match cached {
        Some(c) => c,
        None => {
            let new: Vec<&StreamTrack> = input.new.iter().filter(|t| !input.local_only && snap.is_new(t)).collect();
            let candidates: Vec<NewTrack> = new.iter().map(|t| snap.new_track(t)).collect();
            let seed = if input.refresh_daylist { fastrand::u64(..) } else { day_seed(&key) };
            let params = RadioParams {
                new_share: input.new_share,
                seed,
            };
            let (centre, state, picks) = snap.recs().daylist(part, &candidates, DAYLIST_LEN, &params);
            let streams = new
                .into_iter()
                .filter(|t| picks.iter().any(|p| p.item == Item::Stream(t.id.clone())))
                .cloned()
                .collect();
            let c = DaylistCache {
                key: key.clone(),
                title: daylist_title(centre.axes, part, day),
                state,
                picks,
                streams,
                history,
                local_only: input.local_only,
                new_at: input.new_at,
            };
            cache_write(lib, DAYLIST_KEY, input.now, &c);
            c
        }
    };
    let streams: HashMap<RemoteId, StreamTrack> = c.streams.into_iter().map(|t| (t.id.clone(), t)).collect();
    let picks: Vec<Pick> = c.picks.into_iter().filter(|p| still_fits(snap, p, &streams)).collect();
    if picks.is_empty() {
        return (key, None);
    }
    // The number of songs only: no «what it is made of», no «until 23:00».
    let subtitle = tracks_count(picks.len());
    let covers = covers_of(snap, &picks, &streams);
    let home = DaylistHome {
        kicker: daylist_kicker(part),
        title: c.title,
        subtitle,
        state: c.state,
        picks,
        streams,
        covers,
    };
    (key, Some(home))
}

/// A mix of «Миксы для тебя» as [`HomeParts`] keeps it.
struct LaneParts {
    key: v::MixKey,
    title: String,
    short: Vec<String>,
    artists: Vec<String>,
    /// The new songs' artists.
    new_artists: Vec<String>,
    covers: Vec<CoverRef>,
    new_cover: Option<String>,
    /// Its tracks, read when the parts are resolved.
    rows: Vec<TrackId>,
    /// «С новым»: the new songs it mixes in, in the order they play.
    new_rows: Vec<StreamTrack>,
    blend_base: Option<u32>,
    why: Option<String>,
}

/// The recommendation parts of the home view, ready to be drawn: owned
/// copies of the build's picks, so they can be resolved on another thread
/// (the home's builder, [`crate::home_build`]).
pub(crate) struct HomeParts {
    hero: Option<(String, String, Vec<CoverRef>)>,
    /// The library tracks of «Твой микс», read when resolved.
    mix: Vec<TrackId>,
    /// Kicker, title, subtitle, covers.
    daylist: Option<(String, String, String, Vec<CoverRef>)>,
    /// Id, kicker, title, subtitle, covers.
    tiles: Vec<(v::MixId, String, String, String, Vec<CoverRef>)>,
    moods: Vec<(v::Mood, Vec<CoverRef>)>,
    styles: Vec<(String, String, Vec<CoverRef>)>,
    discover: Vec<v::StreamTrackRow>,
    new_releases: Vec<v::StreamAlbumCard>,
    lanes: Vec<LaneParts>,
    blend_profiles: Vec<v::BlendProfile>,
    history: bool,
    /// «Только скачанное»: nothing on the home may play without a file.
    local_only: bool,
    /// «Карточка наверху главной»: what its world shows is not repeated
    /// below.
    card: bool,
}

impl HomeParts {
    /// The parts of `home` (none before the first build). It reads nothing:
    /// the rows are read when the parts are resolved.
    pub(crate) fn of(
        home: Option<&RecHome>,
        discover: Vec<v::StreamTrackRow>,
        new_releases: Vec<v::StreamAlbumCard>,
        local_only: bool,
        card: bool,
    ) -> Self {
        let Some(h) = home else {
            return Self {
                hero: None,
                mix: Vec::new(),
                daylist: None,
                tiles: Vec::new(),
                moods: Vec::new(),
                styles: Vec::new(),
                discover,
                new_releases,
                lanes: Vec::new(),
                blend_profiles: Vec::new(),
                history: false,
                local_only,
                card,
            };
        };
        let n = h.daily.len();
        let subtitle = if h.history {
            format!("{} · любимое и похожее", tracks_count(n))
        } else {
            "Слушай — микс соберётся из того, что нравится".to_owned()
        };
        let hero = (n > 0).then(|| ("Твой микс".to_owned(), subtitle, h.daily_covers.clone()));
        let mix = h
            .daily
            .iter()
            .filter_map(|p| match p.item {
                Item::Track(id) => Some(id),
                Item::Stream(_) => None,
            })
            .collect();
        let daylist = h
            .daylist
            .as_ref()
            .map(|d| (d.kicker.to_owned(), d.title.clone(), d.subtitle.clone(), d.covers.clone()));
        let tiles = h
            .tiles
            .iter()
            .map(|t| (t.mix_id(), t.kicker().to_owned(), t.title.clone(), t.duration.clone(), t.covers.clone()))
            .collect();
        let lanes = h
            .lanes
            .iter()
            .map(|l| {
                let mut new_artists: Vec<String> = Vec::new();
                for t in &l.new {
                    if let Some(a) = t.artists.first()
                        && !new_artists.contains(&a.name)
                    {
                        new_artists.push(a.name.clone());
                    }
                }
                let new_cover = l.new.iter().find_map(|t| t.cover_url.clone());
                let rows = l.tracks.clone();
                // What «Слушать» plays of them: those that find a place.
                let new_rows: Vec<StreamTrack> = l
                    .picks(true)
                    .into_iter()
                    .filter_map(|p| match p.item {
                        Item::Stream(r) => l.new.iter().find(|t| t.id == r).cloned(),
                        Item::Track(_) => None,
                    })
                    .collect();
                LaneParts {
                    key: l.key,
                    title: l.title.clone(),
                    short: l.short.clone(),
                    artists: l.artists.clone(),
                    new_artists,
                    covers: l.covers.clone(),
                    new_cover,
                    rows,
                    new_rows,
                    blend_base: l.blend_base,
                    why: l.why.clone(),
                }
            })
            .collect();
        let moods = h
            .moods
            .iter()
            .map(|(m, c)| (to_view_mood(*m), c.iter().copied().map(CoverRef::Release).collect()))
            .collect();
        let styles = h
            .styles
            .iter()
            .map(|(m, title, subtitle)| (title.clone(), subtitle.clone(), m.covers.iter().copied().map(CoverRef::Release).collect()))
            .collect();
        Self {
            hero,
            mix,
            daylist,
            tiles,
            moods,
            styles,
            discover,
            new_releases,
            lanes,
            blend_profiles: h.blend_profiles.clone(),
            history: h.history,
            local_only,
            card,
        }
    }

    /// Fills the recommendation fields of `home`.
    pub(crate) fn apply(self, home: &mut v::HomeView, res: &mut crate::Resolver) {
        self.resolve(res).apply(home);
    }

    /// The new-music rows, made after the parts were taken (the home's
    /// builder makes them, [`super::discover::DiscoverInput::rows`]).
    pub(crate) fn with_discover(mut self, discover: Vec<v::StreamTrackRow>, new_releases: Vec<v::StreamAlbumCard>) -> Self {
        self.discover = discover;
        self.new_releases = new_releases;
        self
    }

    /// Reads the rows and resolves the covers now, into what fills `home`
    /// with them ([`RecsFill::apply`] touches neither the library nor the
    /// resolver, so it can run under the state lock).
    pub(crate) fn resolve(self, res: &mut crate::Resolver) -> RecsFill {
        let local_only = self.local_only;
        let card = self.card;
        let lib = res.library().clone();
        let rows = |ids: &[TrackId]| -> Vec<nikgob_library::TrackRow> {
            ids.iter().filter_map(|id| lib.get_track_row(*id).ok().flatten()).collect()
        };
        let mix_rows = rows(self.mix.as_slice());
        let lane_rows: Vec<Vec<nikgob_library::TrackRow>> = self.lanes.iter().map(|l| rows(l.rows.as_slice())).collect();
        let lane_new: Vec<Vec<v::StreamTrackRow>> = self
            .lanes
            .iter()
            .map(|l| l.new_rows.iter().map(|t| crate::streams::stream_row_of(&lib, t, lib.track_for_stream(t).ok().flatten())).collect())
            .collect();
        let mut covers = |c: Vec<CoverRef>| -> Vec<v::Cover> {
            c.into_iter()
                .filter_map(|c| match c {
                    CoverRef::Release(r) => res.release_cover(r),
                    CoverRef::Web(url) => Some(v::Cover::web(url)),
                })
                .take(COVERS)
                .collect()
        };
        let hero = self.hero.map(|(title, subtitle, c)| v::MixCard {
            id: v::MixId::Daily,
            title,
            kicker: "НА СЕГОДНЯ".to_owned(),
            subtitle,
            covers: covers(c),
        });
        let daylist = self.daylist.map(|(kicker, title, subtitle, c)| v::MixCard {
            id: v::MixId::Daylist,
            title,
            kicker,
            subtitle,
            covers: covers(c),
        });
        let tiles: Vec<_> = self
            .tiles
            .into_iter()
            .map(|(id, kicker, title, subtitle, c)| v::MixCard {
                id,
                title,
                kicker,
                subtitle,
                covers: covers(c),
            })
            .collect();
        let moods: Vec<_> = self
            .moods
            .into_iter()
            .map(|(m, c)| v::MixCard {
                id: v::MixId::Mood(m),
                title: m.title().to_owned(),
                kicker: String::new(),
                subtitle: mood_subtitle(m).to_owned(),
                covers: covers(c),
            })
            .collect();
        let style_mixes: Vec<_> = self
            .styles
            .into_iter()
            .enumerate()
            .map(|(i, (title, subtitle, c))| v::MixCard {
                id: v::MixId::Style(i as u8),
                title,
                kicker: String::new(),
                subtitle,
                covers: covers(c),
            })
            .collect();
        let mut lane_covers: Vec<Vec<v::Cover>> = self.lanes.iter().map(|l| covers(l.covers.clone())).collect();
        distinct_leads(&mut lane_covers);
        let lanes: Vec<v::LaneCard> = self
            .lanes
            .into_iter()
            .zip(lane_covers)
            .zip(lane_rows)
            .zip(lane_new)
            .map(|(((l, covers), rows), new_tracks)| v::LaneCard {
                key: l.key,
                title: l.title,
                short_titles: l.short,
                artists: l.artists,
                new_artists: l.new_artists,
                covers,
                new_cover: l.new_cover.map(v::Cover::web),
                tracks: rows.iter().map(|t| res.track(t)).collect(),
                new_tracks,
                blend_base: l.blend_base,
                queue: None,
                why: l.why,
            })
            .collect();
        let blend_profiles = self.blend_profiles;
        let history = self.history;
        let mix: Option<Vec<_>> = (!mix_rows.is_empty()).then(|| mix_rows.iter().map(|t| res.track(t)).collect());
        let (discover, new_releases) = (self.discover, self.new_releases);
        RecsFill {
            hero,
            daylist,
            tiles,
            moods,
            style_mixes,
            mix,
            discover,
            new_releases,
            lanes,
            blend_profiles,
            history,
            local_only,
            card,
        }
    }
}

/// The recommendation parts of the home view resolved ([`HomeParts::resolve`]):
/// what fills the page, made off the state lock and, by the home's builder,
/// off the worker's thread.
pub(crate) struct RecsFill {
    hero: Option<v::MixCard>,
    daylist: Option<v::MixCard>,
    tiles: Vec<v::MixCard>,
    moods: Vec<v::MixCard>,
    style_mixes: Vec<v::MixCard>,
    /// `None`: no library track of «Твой микс» (the page keeps its own).
    mix: Option<Vec<v::TrackRow>>,
    discover: Vec<v::StreamTrackRow>,
    new_releases: Vec<v::StreamAlbumCard>,
    lanes: Vec<v::LaneCard>,
    blend_profiles: Vec<v::BlendProfile>,
    history: bool,
    local_only: bool,
    card: bool,
}

impl RecsFill {
    /// Fills the recommendation fields of `home` (and drops what «Только
    /// скачанное», the rows' doubles and the card at the top leave out).
    pub(crate) fn apply(self, home: &mut v::HomeView) {
        home.hero = self.hero;
        home.daylist = self.daylist;
        home.tiles = self.tiles;
        home.moods = self.moods;
        home.style_mixes = self.style_mixes;
        if let Some(mix) = self.mix {
            home.mix = mix;
        }
        home.discover = self.discover;
        home.new_releases = self.new_releases;
        home.lanes = self.lanes;
        home.blend_profiles = self.blend_profiles;
        home.history |= self.history;
        if self.local_only {
            only_downloaded(home);
        }
        dedupe_rows(home);
        if self.card {
            card_first(home);
        }
    }
}

/// A mix's card shows the first of its covers: each list of `covers` (one
/// for each card of the row, in the row's order) starts with a picture no
/// card before it starts with, while it has one.
fn distinct_leads<T: PartialEq + Clone>(covers: &mut [Vec<T>]) {
    let mut leads: Vec<T> = Vec::new();
    for list in covers.iter_mut() {
        if let Some(at) = list.iter().position(|c| !leads.contains(c)) {
            list[..=at].rotate_right(1);
        }
        if let Some(first) = list.first() {
            leads.push(first.clone());
        }
    }
}

/// «Только скачанное» on the home: whatever would play without a file
/// goes, the rows of new music («Новое для тебя», «Новые релизы») and
/// albums streamed without one; the mixes are picked so already.
pub(crate) fn only_downloaded(home: &mut v::HomeView) {
    home.discover.clear();
    home.new_releases.clear();
    home.mix.retain(|t| t.has_local);
    home.continue_listening.retain(|c| matches!(c, v::ContinueCard::Release(r) if r.local_tracks > 0));
    home.long_unplayed.retain(|r| r.local_tracks > 0);
    home.recently_added.retain(|r| r.local_tracks > 0);
    home.listen_again.retain(|t| t.has_local);
    home.from_library.retain(|a| a.card.local_tracks > 0);
    home.shortcuts.retain(|s| !matches!(s, v::Shortcut::Release(r) if r.local_tracks == 0));
    for l in &mut home.lanes {
        l.tracks.retain(|t| t.has_local);
        l.new_artists.clear();
        l.new_cover = None;
        l.new_tracks.clear();
    }
    for w in &mut home.worlds {
        w.cards.retain(|c| !matches!(c.go, v::WorldGo::Stream(_)));
    }
}

/// With the card at the top: its world gets the new album of the services
/// as its third card, and what the world opened on shows (its albums on
/// the tiles and the stage, that new album) do not show again below; the
/// next in line takes their place (home v2 §14.5, §15.3).
pub(crate) fn card_first(home: &mut v::HomeView) {
    if home.worlds.is_empty() {
        return;
    }
    if !home.new_releases.is_empty() {
        for k in 0..home.worlds.len() {
            if home.worlds[k].cards.iter().any(|c| matches!(c.go, v::WorldGo::Stream(_))) {
                continue;
            }
            if let Some(i) = crate::home_v2::world_release(&home.worlds[k], &home.new_releases) {
                let a = &home.new_releases[i];
                let mut name = crate::home_v2::short_album(&a.title, &home.worlds[k].title);
                // «FILM CONCERT LIVE RECORDING CD» reads «FILM CONCERT».
                for tail in [" LIVE RECORDING CD", " LIVE RECORDING", " CD"] {
                    if name.len() > tail.len() && name.get(name.len() - tail.len()..).is_some_and(|t| t.eq_ignore_ascii_case(tail)) {
                        name.truncate(name.len() - tail.len());
                    }
                }
                let card = v::WorldCard {
                    icon: v::WorldIcon::Album,
                    kicker: "Новый альбом".into(),
                    title: name,
                    go: v::WorldGo::Stream(a.remote.clone()),
                };
                let w = &mut home.worlds[k];
                w.cards.truncate(2);
                w.cards.push(card);
            }
        }
    }
    let first = &home.worlds[0];
    let mut shown: HashSet<v::ReleaseId> = home.worlds.iter().filter_map(|w| w.release).collect();
    shown.extend(first.release);
    let stream: Option<v::RemoteId> = first.cards.iter().find_map(|c| match &c.go {
        v::WorldGo::Stream(r) => Some(r.clone()),
        _ => None,
    });
    home.shortcuts.retain(|s| !matches!(s, v::Shortcut::Release(r) if shown.contains(&r.id)));
    home.from_library.retain(|a| !shown.contains(&a.card.id));
    if let Some(r) = stream {
        home.new_releases.retain(|a| a.remote != r);
    }
}

/// Keeps the first of each item in every row of the home: a row shows an
/// album, a song, an artist or a mix once (the UI keys its cards by them).
pub(crate) fn dedupe_rows(home: &mut v::HomeView) {
    fn keep_first<T, K: std::hash::Hash + Eq>(row: &mut Vec<T>, key: impl Fn(&T) -> K) {
        let mut seen = HashSet::new();
        row.retain(|x| seen.insert(key(x)));
    }
    keep_first(&mut home.mix, |t| t.id);
    keep_first(&mut home.tiles, |c| c.id);
    keep_first(&mut home.moods, |c| c.id);
    keep_first(&mut home.style_mixes, |c| c.id);
    keep_first(&mut home.discover, |r| r.remote.clone());
    keep_first(&mut home.new_releases, |a| a.remote.clone());
    keep_first(&mut home.continue_listening, |c| match c {
        v::ContinueCard::Release(r) => (Some(r.id), None),
        v::ContinueCard::Stream(a) => (None, Some(a.remote.clone())),
    });
    keep_first(&mut home.artist_mixes, |a| a.id);
    keep_first(&mut home.long_unplayed, |r| r.id);
    keep_first(&mut home.recently_added, |r| r.id);
}

/// The home of [`preview_home`] and what it was built from.
pub struct HomePreview {
    /// As the player would show it, but «Новое для тебя» and «Новые
    /// релизы» (their rows need the services; see the counts below).
    pub home: v::HomeView,
    /// Tracks the engine may pick (library tracks and liked streams).
    pub catalog: usize,
    /// Sound vectors of the analysis version in use.
    pub vectors: usize,
    pub events: usize,
    pub likes: usize,
    /// Something was listened, liked or put in a playlist.
    pub history: bool,
    /// The daylist's song count.
    pub daylist_tracks: usize,
    /// Each tile's song count.
    pub tile_tracks: Vec<(v::MixId, usize)>,
    /// Discoveries and new releases the last discovery pass left.
    pub discover_cached: usize,
    pub new_releases_cached: usize,
    /// Library tracks the services' kept suggestions give affinity now,
    /// and suggested songs new to the library (docs/stage4.md §12.3).
    pub suggested_library: usize,
    pub suggested_new: usize,
    pub took: Duration,
}

/// The home exactly as the runtime builds it, without a player: the
/// recommendation snapshot of `lib` now and the home over it (checks on a
/// copy of a real library, docs/stage4.md §12.7). Like a build, it keeps
/// the day's picks in the library's cache.
pub fn preview_home(lib: &Library, library_dir: &std::path::Path) -> Result<HomePreview, String> {
    preview_home_with(lib, library_dir, &v::Prefs::default())
}

/// [`preview_home`] under `prefs` («Новое в радио», «Миксы только из
/// скачанного»).
pub fn preview_home_with(lib: &Library, library_dir: &std::path::Path, prefs: &v::Prefs) -> Result<HomePreview, String> {
    let started = Instant::now();
    let index = SoundIndex::default();
    crate::analysis::load_index(lib, &index);
    let vectors = index.read().len();
    let (day, utc_offset_min) = local_day();
    let (discoveries, new_releases_cached) = discover::cached_found(lib);
    let epoch = crate::home_v2::Epoch::of(crate::home_v2::Epoch::run(), recs_now(), utc_offset_min);
    let input = BuildInput {
        lib: lib.clone(),
        index: index.clone(),
        day,
        epoch: epoch.clone(),
        utc_offset_min,
        now: recs_now(),
        refresh_daily: false,
        refresh_daylist: false,
        new_share: new_share(prefs, v::RadioKind::Mix),
        local_only: prefs.mixes_downloaded_only,
        new: discoveries.clone(),
        new_at: 0,
        new_pool: discoveries.clone(),
        new_sources: HashMap::new(),
        suggested: suggest::cached(lib),
        rerank: false,
        refresh_mix: None,
        row_more: 0,
        blends: Vec::new(),
        session: Session::default(),
        mix_prefs: lanes::MixPrefs::of(prefs),
        extra: ReadExtra::of(discoveries.clone(), prefs),
        new_log: novelty::log_of(lib),
    };
    let built = build(input)?;
    let h = &built.home;
    let mut res = crate::Resolver::new(lib.clone(), library_dir.to_owned());
    let mut visit = crate::home_v2::Visit::now(None, prefs);
    visit.sound = Some(index.clone());
    visit.profiles = Some(built.snap.profiles.clone());
    visit.epoch = epoch;

    let mut home = res.home(&crate::home::build_with(lib, &visit));
    HomeParts::of(Some(h), Vec::new(), Vec::new(), prefs.mixes_downloaded_only, prefs.home_card_shown()).apply(&mut home, &mut res);
    Ok(HomePreview {
        catalog: built.snap.catalog.len(),
        vectors,
        events: lib.listening_events().map_or(0, |e| e.len()),
        likes: lib.liked_track_ids().map_or(0, |l| l.len()),
        history: h.history,
        daylist_tracks: h.daylist.as_ref().map_or(0, |d| d.picks.len()),
        tile_tracks: h.tiles.iter().map(|t| (t.mix_id(), t.picks.len())).collect(),
        discover_cached: discoveries.len(),
        new_releases_cached,
        suggested_library: (0..built.snap.catalog.len()).filter(|&i| built.snap.model.taste.suggested_at(i) > 0.0).count(),
        suggested_new: built.snap.suggested_new.len(),
        took: started.elapsed(),
        home,
    })
}

/// The prequential replay of a library's history (recs-v2 §13): every
/// start predicted by the model learnt from the starts before it, against
/// the affinity of phase 1 and the hand weights. The build keeps its
/// caches in `lib`: run it on a copy.
pub fn preview_replay(lib: &Library) -> Result<nikgob_recs::model::ReplayReport, String> {
    let preview = ModelPreview::of(lib)?;
    let snap = &preview.snap;
    Ok(nikgob_recs::model::replay(&snap.catalog, &preview.history, ModelInput { index: &snap.index, types: Some(&snap.types), weights: None, graph: None }))
}

/// [`preview_replay`] with the recommendations' settings of `prefs`: the
/// graph of similar music the library keeps, the personal graph.
pub fn preview_replay_with(lib: &Library, prefs: &v::Prefs) -> Result<nikgob_recs::model::ReplayReport, String> {
    let preview = ModelPreview::of_with(lib, prefs)?;
    let snap = &preview.snap;
    let graph = (!preview.graph.is_empty()).then_some(&preview.graph);
    Ok(nikgob_recs::model::replay(&snap.catalog, &preview.history, ModelInput { index: &snap.index, types: Some(&snap.types), weights: None, graph }))
}

/// A snapshot of a library to try the radio on, as the runtime would
/// (its caches are kept in the library: a copy).
pub struct ModelPreview {
    snap: Snapshot,
    history: TasteInput,
    /// The graph of similar music it was built with.
    graph: nikgob_recs::GraphInput,
    /// The last pass's discoveries and where each came from.
    new: Vec<StreamTrack>,
    new_sources: HashMap<String, nikgob_recs::Source>,
}

impl ModelPreview {
    pub fn of(lib: &Library) -> Result<Self, String> {
        Self::of_with(lib, &v::Prefs::default())
    }

    /// The snapshot with the recommendations' settings of `prefs`.
    pub fn of_with(lib: &Library, prefs: &v::Prefs) -> Result<Self, String> {
        let index = SoundIndex::default();
        crate::analysis::load_index(lib, &index);
        let (_, utc_offset_min) = local_day();
        // The last pass's finds: their analysed previews come with the
        // snapshot.
        let (new, new_sources) = discover::cached_pool(lib);
        let extra = ReadExtra::of(new.clone(), prefs);
        let (snap, _, _, history) = read_parts(lib, &index, recs_now(), utc_offset_min, &[], false, &extra)?;
        let graph = if extra.graph {
            let rows = lib.rec_tracks().map_err(|e| e.to_string())?;
            graph::read(lib, &rows, &snap.artist_keys).input
        } else {
            nikgob_recs::GraphInput::default()
        };
        Ok(Self { snap, history, graph, new, new_sources })
    }

    /// What the snapshot was built of, for the offline evaluation
    /// ([`nikgob_recs::eval`]): the catalog, its sound and song types, the
    /// history and the graph, and the new songs of the services the cards
    /// of «Миксы для тебя» could take (the discoveries and the graph's
    /// finds, weighed as a build weighs them).
    pub fn dataset(&self, name: &str) -> nikgob_recs::eval::Dataset {
        let graph = (!self.graph.is_empty()).then_some(&self.graph);
        let mut d = nikgob_recs::eval::Dataset::of(name, &self.snap.catalog, &self.snap.index, &self.snap.types, &self.history, graph);
        let snap = &self.snap;
        d.new = lanes::candidates(snap, &self.new, &self.new_sources, &nikgob_recs::NewLog::default(), 1)
            .into_iter()
            .map(|c| nikgob_recs::eval::DNew {
                vector: snap.new_vectors.get(&stream_song_key(&c.track)).map(|v| v.to_vec()),
                near: c.near.iter().map(|&i| snap.model.id(i)).collect(),
                weight: c.weight,
                artist: c.artist_tracks.first().and_then(|&i| snap.model.main_artist_at(i)),
                track: c.track,
            })
            .collect();
        d
    }

    /// The edges of tracks and of artists of the graph of similar music in
    /// the model.
    pub fn graph_size(&self) -> (usize, usize) {
        self.snap.model.graph.len()
    }

    /// The model the snapshot scores by.
    pub fn model(&self) -> &UserModel {
        &self.snap.model
    }

    /// The catalog and the sound index of the snapshot.
    pub fn catalog(&self) -> (&Catalog, &VectorIndex<TrackId>) {
        (&self.snap.catalog, &self.snap.index)
    }

    /// The seeds of a pass of «Новое» now: the tracks, then the artists,
    /// a line each, and the artists whose new releases are looked for.
    pub fn new_music_seeds(&self, lib: &Library) -> (Vec<String>, Vec<String>) {
        let recs = self.snap.recs();
        let seeds = recs.discovery_seeds();
        let track = |t: TrackId| lib.get_track(t).ok().flatten().map_or_else(|| t.to_string(), |t| format!("«{}» {}", t.title, t.artist_credit.display()));
        let artist = |a: ArtistId| lib.get_artist(a).ok().flatten().map_or_else(|| a.to_string(), |a| a.name);
        let elsewhere: HashSet<TrackId> = self.snap.model.taste.saved_elsewhere().iter().map(|s| s.0).collect();
        let mut lines: Vec<String> = seeds
            .tracks
            .iter()
            .map(|(t, a)| format!("{} {a:.2}{}", track(*t), if elsewhere.contains(t) { " (saved elsewhere)" } else { "" }))
            .collect();
        lines.extend(seeds.artists.iter().map(|(a, s)| format!("artist {} {s:.2}", artist(*a))));
        (lines, recs.release_artists().into_iter().map(artist).collect())
    }

    /// The playlists the user plays in the background, by name.
    pub fn background_playlists(&self, lib: &Library) -> Vec<String> {
        let names: HashMap<ArtistId, String> = lib.artist_names().unwrap_or_default().into_iter().collect();
        let rows = lib.rec_tracks().unwrap_or_default();
        let events = lib.listening_events().unwrap_or_default();
        let tiles = tiles::input(lib, &rows, &events, names);
        let (kept, _) = played_in_background(&self.snap, &tiles, &events);
        kept.into_iter().filter_map(|k| tiles.playlists.get(k)).map(|p| p.name.clone()).collect()
    }

    /// The worlds by title (their tokens).
    pub fn world_named(&self, title: &str) -> Option<u64> {
        self.snap.worlds.iter().find(|(_, (_, t))| t.eq_ignore_ascii_case(title)).map(|(w, _)| *w)
    }

    /// The mixes of a world's page now, with their tracks and why each is
    /// offered.
    pub fn world_mixes(&self, lib: &Library, world: u64) -> Vec<(nikgob_recs::WorldMixKind, Vec<TrackId>, Option<String>)> {
        let (part, _, _) = daylist_key(recs_now(), local_day().1);
        let recs = self.snap.recs();
        let name = self.world_title(world).unwrap_or_default();
        let title = |i: usize| lib.get_track(self.snap.model.id(i)).ok().flatten().map(|t| t.title);
        recs.world_mixes(world, part)
            .into_iter()
            .map(|m| {
                let why = recs.why_world_mix(&m, &name, &title);
                (m.kind, m.tracks.iter().map(|&i| self.snap.model.id(i)).collect(), why)
            })
            .collect()
    }

    /// Why a radio's pick `p` after `before` ([`nikgob_recs::explain`]).
    pub fn why_pick(&self, lib: &Library, p: &Pick, before: Option<TrackId>) -> Option<String> {
        let title = |t: TrackId| lib.get_track(t).ok().flatten().map(|t| t.title);
        self.snap.recs().why_pick(p, before, &title)
    }

    /// The track suits the background ([`background_fit`]).
    pub fn suits_background(&self, id: TrackId) -> bool {
        self.snap.catalog.position(id).is_some_and(|i| self.snap.background.get(i).copied().unwrap_or(false))
    }

    /// The name of the taste profile of a track.
    pub fn profile_name(&self, lib: &Library, id: TrackId) -> Option<String> {
        let names: HashMap<ArtistId, String> = lib.artist_names().unwrap_or_default().into_iter().collect();
        let j = self.snap.catalog.position(id).and_then(|i| self.snap.model.profile_at(i))?;
        Some(profile_name(&self.snap.model, &names, v::ProfileNames::Kind, j).0)
    }

    /// The next `n` of a radio.
    pub fn extend(&self, radio: &mut Engine, n: usize) -> Vec<Pick> {
        radio.extend(&self.snap.recs(), n, &RadioParams { new_share: 0.0, seed: 1 })
    }

    /// The next `n` of a radio with `new_share` of new music.
    pub fn extend_new(&self, radio: &mut Engine, n: usize, new_share: f32) -> Vec<Pick> {
        radio.extend(&self.snap.recs(), n, &RadioParams { new_share, seed: 1 })
    }

    /// The new music a service's answer kept in the library's cache under
    /// `key` (`radio_new.youtube.radio.<id>`) holds, new indeed.
    pub fn cached_new(&self, lib: &Library, key: &str) -> Vec<StreamTrack> {
        let tracks: Vec<StreamTrack> = cache_read(lib, key).unwrap_or_default();
        tracks.into_iter().filter(|t| self.snap.is_new(t)).collect()
    }

    /// The world of a catalog track.
    pub fn world(&self, id: TrackId) -> Option<u64> {
        self.snap.catalog.get(id).and_then(|t| t.world)
    }

    /// The world's title.
    pub fn world_title(&self, world: u64) -> Option<String> {
        self.snap.worlds.get(&world).map(|w| w.1.clone())
    }


    /// The taste profiles, the largest share first (as the log has them).
    pub fn profiles(&self, lib: &Library) -> Vec<String> {
        let names: HashMap<ArtistId, String> = lib.artist_names().unwrap_or_default().into_iter().collect();
        let rows = lib.rec_tracks().unwrap_or_default();
        let events = lib.listening_events().unwrap_or_default();
        let tiles = tiles::input(lib, &rows, &events, names.clone());
        profile_lines(&self.snap.model, &tiles, &names, v::ProfileNames::Kind)
    }

    /// How the user listens ([`Listening`](nikgob_recs::listening::Listening)), in a line.
    pub fn listening(&self) -> String {
        listening_line(&self.snap.model)
    }

    /// The home's cards of each profile out of `slots`, the largest share
    /// first: its name, its cards and, for a background profile, the sizes
    /// of its «Фон» cards (the basic background rule).
    pub fn home_profiles(&self, lib: &Library, slots: usize) -> Vec<String> {
        let names: HashMap<ArtistId, String> = lib.artist_names().unwrap_or_default().into_iter().collect();
        let model = &self.snap.model;
        let fit = &self.snap.background;
        let home = model.home_profiles(slots, nikgob_recs::profiles::BG_CARDS, |i| fit[i]);
        profile_order(model)
            .into_iter()
            .map(|j| {
                let (name, _) = profile_name(model, &names, v::ProfileNames::Kind, j);
                let bg: Vec<usize> = home.background[j].iter().map(Vec::len).collect();
                format!("{name}: {} cards{}", home.cards[j], if bg.is_empty() { String::new() } else { format!(", «Фон» of {bg:?} tracks") })
            })
            .collect()
    }

    pub fn song_type(&self, id: TrackId) -> Option<nikgob_recs::SongType> {

        self.snap.types.get(id).copied()
    }

    /// The replay before and after `n` tracks with plays are disliked
    /// (drawn by `seed`): the same starts scored both times, the disliked
    /// tracks' own left out of the scoring.
    pub fn replay_dislikes(&self, n: usize, seed: u64) -> (nikgob_recs::model::ReplayReport, nikgob_recs::model::ReplayReport, Vec<TrackId>) {
        let m = &self.snap.model;
        let mut played: Vec<TrackId> = (0..m.len()).filter(|&i| m.tracks[i].starts > 0).map(|i| m.id(i)).collect();
        let mut rng = nikgob_recs::rng::Rng::new(seed);
        let mut dislikes = Vec::new();
        while dislikes.len() < n && !played.is_empty() {
            let k = (rng.next_u64() % played.len() as u64) as usize;
            dislikes.push(played.swap_remove(k));
        }
        let unscored: HashSet<TrackId> = dislikes.iter().copied().collect();
        let input = || ModelInput { index: &self.snap.index, types: Some(&self.snap.types), weights: None, graph: None };
        let before = nikgob_recs::model::replay_with(&self.snap.catalog, &self.history, input(), &[], &unscored);
        let after = nikgob_recs::model::replay_with(&self.snap.catalog, &self.history, input(), &dislikes, &unscored);
        (before, after, dislikes)
    }

    /// What a dislike of `track` does: how much lower each track stands
    /// (its nearest sound neighbours), and its artist's other tracks with
    /// theirs (0 unless one is a neighbour).
    pub fn dislike_effect(&self, track: TrackId) -> (Vec<(TrackId, f32)>, Vec<(TrackId, f32)>) {
        let m = &self.snap.model;
        let Some(d) = m.position(track) else { return (Vec::new(), Vec::new()) };
        let emb: Vec<Option<nikgob_recs::Vector>> = (0..m.len()).map(|i| m.embedded_at(i).copied()).collect();
        let near = nikgob_recs::dislike::near_dislikes(&emb, &[d], m.scale());
        let mut lower: Vec<(TrackId, f32)> = near.iter().enumerate().filter(|(_, w)| **w > 0.0).map(|(i, w)| (m.id(i), *w)).collect();
        lower.sort_by(|a, b| b.1.total_cmp(&a.1));
        let artist = m.main_artist_at(d);
        let same: Vec<(TrackId, f32)> =
            (0..m.len()).filter(|&i| i != d && artist.is_some() && m.main_artist_at(i) == artist).map(|i| (m.id(i), near[i])).collect();
        (lower, same)
    }

    /// The finds of the last discovery pass and the new music the services'
    /// answers under `keys` keep, that the junk filter leaves out, with why.
    pub fn junk_found(&self, lib: &Library, keys: &[&str]) -> Vec<(StreamTrack, nikgob_recs::Junk)> {
        let mut pool = discover::cached_finds(lib);
        for k in keys {
            pool.extend(cache_read::<Vec<StreamTrack>>(lib, k).unwrap_or_default());
        }
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for t in &pool {
            if !seen.insert(t.id.clone()) {
                continue;
            }
            let canonical = nikgob_recs::junk::canonical_in(t, &pool);
            if let Some(j) = nikgob_recs::junk(t, canonical.as_ref()) {
                out.push((t.clone(), j));
            }
        }
        out
    }

    /// How far apart two tracks sound, in the model's median distances.

    pub fn distance(&self, a: TrackId, b: TrackId) -> Option<f32> {
        let m = &self.snap.model;
        let (a, b) = (m.embedded_at(m.position(a)?)?, m.embedded_at(m.position(b)?)?);
        Some(nikgob_recs::index::euclid(a, b) / m.scale())
    }
}

impl Worker {
    /// The home's epoch now: this run's, of this part of the day.
    pub(crate) fn home_epoch(&self) -> crate::home_v2::Epoch {
        crate::home_v2::Epoch::of(self.recs.run, recs_now(), local_day().1)
    }

    /// The recommendation parts of the home page as they stand, without
    /// their rows: the home's builder reads those, resolves the covers and
    /// makes the new-music rows ([`Self::discover_input`]).
    pub(crate) fn recs_home_parts(&self) -> HomeParts {
        let (local_only, card) = {
            let p = &self.state.read().prefs;
            (p.mixes_downloaded_only, p.home_card_shown())
        };
        HomeParts::of(self.recs.home.as_ref(), Vec::new(), Vec::new(), local_only, card)
    }

    /// Draws the recommendation parts of the home page again (built off
    /// the loop, [`crate::home_build`]).
    pub(crate) fn publish_recs_home(&mut self) {
        self.ask_home(false);
    }

    /// «Не предлагать этот микс»: its card goes at once (another takes its
    /// place with the build), «Отменить» brings it back.
    pub(crate) fn mix_hidden(&mut self, m: v::HiddenMix) {
        if let Some(h) = self.recs.home.as_mut() {
            h.lanes.retain(|l| l.key != m.key);
        }
        let key = m.key;
        self.patch_home(move |home| home.lanes.retain(|l| l.key != key));
        self.toast(format!("Не буду предлагать «{}»", m.title), Some(("Отменить".into(), Command::SetPref(v::PrefPatch::ShowMix(m.key)))));
    }

    /// «Миксы для тебя» scrolled to its end: [`ROW_MORE`] more cards, built
    /// now, while the last build filled the row it was asked for (else
    /// there are no more).
    pub(crate) fn recs_more_mixes(&mut self) {
        let Some(home) = self.recs.home.as_ref() else { return };
        let shown = home.lanes.iter().filter(|l| l.key != v::MixKey::OnRepeat).count();
        if shown < nikgob_recs::mixes::ROW_MIXES + self.recs.row_more {
            return;
        }
        self.recs.row_more += ROW_MORE;
        self.recs_dirty(Duration::ZERO);
        if !self.recs.building {
            self.recs_build();
        }
    }

    /// A build is due after `after` (sooner if one is due already).
    pub(crate) fn recs_dirty(&mut self, after: Duration) {
        let at = Instant::now() + after;
        let r = &mut self.recs;
        r.due = Some(r.due.map_or(at, |d| d.min(at)));
        r.urgent |= after.is_zero();
    }

    /// «Миксы только из скачанного» changed: the home is built again now
    /// (its kept picks are picked again under the new setting).
    pub(crate) fn recs_prefs_changed(&mut self) {
        self.recs_dirty(Duration::ZERO);
        if !self.recs.building {
            self.recs_build();
        }
    }

    /// The library changed (likes, playlists, imports): a build once it has
    /// been quiet for the debounce, so a run of likes or playlist edits
    /// builds once, yet at most ten debounces after the first of them.
    pub(crate) fn recs_library_changed(&mut self) {
        let d = self.streaming.schedule.recs_debounce;
        let now = Instant::now();
        let r = &mut self.recs;
        let first = *r.changed_since.get_or_insert(now);
        let at = (now + d).min(first + d * 10);
        // A build already due (or due before this run began) stays so.
        if !r.due.is_some_and(|due| due <= now || due < first) {
            r.due = Some(at);
        }
    }

    /// A track ended: a build after enough of them.
    pub(crate) fn recs_played(&mut self) {
        self.recs.plays += 1;
        if self.recs.plays >= PLAYS_PER_BUILD {
            self.recs.plays = 0;
            let d = self.streaming.schedule.recs_debounce;
            self.recs_dirty(d);
        }
    }

    /// A start, `origin` its chooser: the session goes on, and a start the
    /// user chose teaches the model of the next start.
    pub(crate) fn recs_started(&mut self, track: TrackId, origin: nikgob_core::model::EventOrigin) {
        let track = self.recs_song_of(track);
        let now = recs_now();
        let ctx = self.play_context;
        let r = &mut self.recs;
        if r.session.last_ts().is_none_or(|l| now - l >= nikgob_recs::taste::SESSION_GAP_MS) {
            r.session = Session::default();
        }
        r.skip_x = None;
        if let Some(snap) = r.snap.clone()
            && let Some(i) = snap.model.position(track)
        {
            let model = &snap.model;
            let m = model.moment(&r.session, now);
            r.skip_x = Some((track, model.skip_features(&m, i, ctx)));
            if m.prev.is_some() && nikgob_recs::model::teaches(Some(origin), ctx) {
                let w = r.weights.get_or_insert(model.weights);
                model.teach_next(&mut w.next, &m, i);
                self.recs_keep_weights();
            }
        }
        if self.recs.skip_x.is_none()
            && let Some(x) = self.new_started(track, origin)
        {
            self.recs.skip_x = Some((track, x));
        }
        self.recs.session.start(track, now, ctx, Some(origin));
    }

    /// How a start ended (not by closing the app), where, and its length
    /// when known: the session and the skip model learn it.
    pub(crate) fn recs_ended(&mut self, track: TrackId, complete: bool, position_ms: u32, duration_ms: Option<u32>) {
        let track = self.recs_song_of(track);
        // A short track's outro is no early skip.
        let ended = if complete { Ended::Complete } else { nikgob_recs::session::skip_ended(position_ms, duration_ms) };
        self.new_ended(track, ended);
        let r = &mut self.recs;
        r.session.end_at(track, ended, position_ms, duration_ms, false);
        let Some((t, x)) = r.skip_x.take() else { return };
        let Some(snap) = r.snap.clone() else { return };
        // What it teaches the skip model («not now» of a loved track:
        // nothing, it was the mood).
        let lesson = r.session.last_of(track).and_then(|p| Session::skip_lesson(p, &snap.model));
        let Some(skipped) = lesson.filter(|_| t == track) else { return };
        r.weights.get_or_insert(snap.model.weights).skip.observe(&x, skipped);
        self.recs_keep_weights();
    }

    /// The bar of the playing `track` was dragged to `to_ms`: a skip after
    /// it is a scrub-skip ([`nikgob_recs::session::Session::seek`]).
    pub(crate) fn recs_seeked(&mut self, track: TrackId, to_ms: u32) {
        let track = self.recs_song_of(track);
        self.recs.session.seek(track, to_ms);
    }

    /// The user liked `track`: its start in the session is approved, and a
    /// radio playing it takes its neighbours in.
    pub(crate) fn recs_liked(&mut self, track: TrackId) {
        let song = self.recs_song_of(track);
        self.recs.session.like(song);
        self.radio_liked(track);
    }

    /// The catalog's track for `track`: itself, or the copy standing for
    /// its recording when it is another copy of one song on two albums
    /// (the session holds one id per song: «Не повторять сыгранное»
    /// covers both).
    fn recs_song_of(&self, track: TrackId) -> TrackId {
        self.recs.snap.as_ref().and_then(|s| s.catalog.get(track)).map_or(track, |t| t.id)
    }

    /// The weights learnt online, into `recs.model`: at most every
    /// [`KEEP_WEIGHTS_EVERY`] (a start and an end write nothing more on the
    /// loop); what is learnt between goes with the next write, a tick or
    /// the app's close ([`Self::recs_flush_weights`]).
    fn recs_keep_weights(&mut self) {
        if self.recs.weights_kept.is_some_and(|t| t.elapsed() < KEEP_WEIGHTS_EVERY) {
            self.recs.weights_unkept = true;
            return;
        }
        self.recs_write_weights();
    }

    fn recs_write_weights(&mut self) {
        self.recs.weights_kept = Some(Instant::now());
        self.recs.weights_unkept = false;
        if let Some(w) = &self.recs.weights
            && let Ok(json) = serde_json::to_string(w)
            && let Err(e) = self.lib.cache_put(MODEL_KEY, now_ms(), &json)
        {
            log::warn!("recs: cannot keep the model: {e}");
        }
    }

    /// Writes the weights learnt since the last write (`now`: whenever they
    /// wait; else once [`KEEP_WEIGHTS_EVERY`] is over).
    pub(crate) fn recs_flush_weights(&mut self, now: bool) {
        if self.recs.weights_unkept && (now || self.recs.weights_kept.is_none_or(|t| t.elapsed() >= KEEP_WEIGHTS_EVERY)) {
            self.recs_write_weights();
        }
    }

    /// Every tick: a build when one is due, the discovery job.
    pub(crate) fn recs_tick(&mut self) {
        self.recs_flush_weights(false);
        let now = Instant::now();
        let every = self.streaming.schedule.recs_every;
        let (day, utc_offset_min) = local_day();
        let (_, _, daylist) = daylist_key(recs_now(), utc_offset_min);
        let r = &self.recs;
        let grown = {
            let (len, built) = (self.analysis.index().read().len(), r.built_index_len);
            len > built && (len - built >= NEW_VECTORS.max(built / 10) || len >= 2 * built)
        };
        let stale = r.built_at.is_some_and(|t| t.elapsed() >= every)
            || r.home.as_ref().is_some_and(|h| h.day != day || h.daylist_key != daylist)
            || (r.built_at.is_some() && grown);
        // Nobody looks at the recommendations while a game is in front:
        // they are built, and new music looked for, when it is over.
        if crate::quiet::busy() {
            return;
        }
        let rested = r.urgent || r.built_at.is_none_or(|t| t.elapsed() >= (r.build_took * REST).min(MAX_REST));
        if !r.building && rested && (r.due.is_some_and(|d| now >= d) || stale) {
            self.recs_build();
        }
        self.discover_tick();
        self.suggest_tick();
        self.graph_tick();
        self.niche_tick();
    }

    /// The run's first build at once, once the sound vectors are in memory
    /// (no network pass with it: those wait for the tick), so the mixes do
    /// not wait for the first tick. Only in a run that keeps its state (the
    /// app's; a test's builds keep to its ticks), and not while a game is in
    /// front.
    pub(crate) fn recs_first_build(&mut self) {
        if self.prefs_path.is_none() || self.recs.built_at.is_some() || self.recs.building || crate::quiet::busy() {
            return;
        }
        log::info!("recs: the run's first build, before the first tick");
        self.recs_build();
    }

    fn recs_build(&mut self) {
        // The last pass's finds, before the run's first build takes them.
        self.discover_load();
        let (day, utc_offset_min) = local_day();
        let prefs = self.state.read().prefs.clone();
        self.recs.blends.retain(|b| b.0 == day);
        let blends = self.recs.blends.iter().map(|b| b.1).collect();
        let input = BuildInput {
            lib: self.lib.clone(),
            index: self.analysis.index().clone(),
            day,
            epoch: self.home_epoch(),
            utc_offset_min,
            now: recs_now(),
            refresh_daily: std::mem::take(&mut self.recs.refresh_daily),
            refresh_daylist: std::mem::take(&mut self.recs.refresh_daylist),
            new_share: new_share(&prefs, v::RadioKind::Mix),
            local_only: prefs.mixes_downloaded_only,
            new: self.discoveries(),
            new_at: self.discoveries_at(),
            new_pool: self.discoveries_all(),
            new_sources: self.discovery_sources(),
            suggested: self.suggestions_kept(),
            rerank: std::mem::take(&mut self.recs.discover_rerank),
            refresh_mix: self.recs.refresh_mix.take(),
            row_more: self.recs.row_more,
            blends,
            session: self.recs.session.clone(),
            mix_prefs: lanes::MixPrefs::of(&prefs),
            extra: ReadExtra::of(self.discoveries_all(), &prefs),
            new_log: {
                self.new_log_load();
                self.recs.newlog.log.clone()
            },
        };

        let tx = self.recs.tx.clone();
        let spawned = std::thread::Builder::new().name("nikgob-recs".into()).spawn(move || {
            lower_priority();
            let built = build(input);
            let _ = tx.send(Internal::Recs(Msg::Built(Box::new(built))));
        });
        match spawned {
            Ok(_) => {
                self.recs.building = true;
                self.recs.build_began = Some(Instant::now());
                self.recs.urgent = false;
                self.recs.due = None;
                self.recs.changed_since = None;
            }
            Err(e) => log::warn!("recs: cannot build: {e}"),
        }
    }

    pub(crate) fn recs_msg(&mut self, m: Msg) {
        match m {
            Msg::Built(b) => self.recs_built(*b),
            Msg::Radio(m) => self.radio_msg(m),
            Msg::Plus(m) => self.plus_msg(m),
            Msg::Discover(m) => self.discover_msg(m),
            Msg::Suggest(m) => self.suggest_msg(m),
            Msg::Graph(m) => self.graph_msg(m),
            Msg::Niche(m) => self.niche_msg(m),
        }
    }

    fn recs_built(&mut self, b: Result<Built, String>) {
        self.recs.building = false;
        self.recs.built_at = Some(Instant::now());
        self.recs.build_took = self.recs.build_began.take().map_or(Duration::ZERO, |t| t.elapsed());
        let rerank = match b {
            Ok(b) => {
                let rerank = b.rerank;
                self.recs.built_index_len = b.index_len;
                let built = b.snap.model.weights;
                if self.recs.weights.is_none_or(|w| w.version != built.version || w.next.seen + w.skip.seen <= built.next.seen + built.skip.seen) {
                    self.recs.weights = None;
                }
                // The run's first, or a new epoch's: the home's shortcuts
                // and «Слушать снова» come with its mixes (the first drawn
                // again with the taste profiles they were drawn without).
                let fresh = self.recs.home.as_ref().is_none_or(|h| h.epoch != b.home.epoch);
                // The build before is let go of off the loop (a large
                // catalog takes a while to free).
                let old = (self.recs.snap.replace(b.snap), self.recs.home.replace(b.home));
                self.drop_off_loop(old);
                self.turn.lap("swap");
                // The whole page with its mixes, or the mixes on the page.
                if fresh {
                    self.load_home();
                } else {
                    self.publish_recs_home();
                }
                self.turn.lap("home asked");
                rerank
            }
            Err(e) => {
                log::warn!("recs: {e}");
                return;
            }
        };
        self.world_mixes_again();
        self.turn.lap("world");
        if let Some(id) = self.recs.pending_play.take() {
            self.play_mix(id);
            self.turn.lap("pending mix");
        }
        self.radio_snapshot_ready();
        self.plus_ready();
        self.turn.lap("radio");
        if rerank {
            self.discover_rerank();
            self.turn.lap("rerank");
        }
        self.discover_tick();
        self.turn.lap("discover");
    }

    /// Test hook: the online metadata the graph's pass asks.
    pub(crate) fn set_graph_metadata(&mut self, m: nikgob_metadata::Metadata) {
        self.recs.graph.metadata = Some(m);
        self.graph_now();
    }

    pub(crate) fn snapshot(&self) -> Option<Arc<Snapshot>> {
        self.recs.snap.clone()
    }

    /// Recommendation commands; anything else is handed back.
    pub(crate) fn recs_command(&mut self, c: Command) -> Option<Command> {
        match c {
            Command::PlayMix { id } => self.play_mix(id),
            Command::StartRadio { seed } => self.start_radio_cmd(seed),
            Command::RecFeedback { target, dislike } => self.rec_feedback(target, dislike),
            Command::Dislike { track, dislike, sure, relike } => self.dislike(track, dislike, sure, relike),
            Command::RefreshMix { id } => self.refresh_mix(id),
            Command::PlayBlend { profiles } => self.play_blend(&profiles),
            Command::PlayWave { preset } => self.play_wave(preset),
            Command::SetWave(filter) => self.set_wave(filter),
            Command::OtherVibe => self.other_vibe(),
            Command::DownloadMix { id } => self.download_mix(id),
            other => return Some(other),
        }
        None
    }

    /// `Command::PlayMix`.
    pub(crate) fn play_mix(&mut self, id: v::MixId) {
        if self.recs.snap.is_none() {
            // The first build is on its way.
            self.recs.pending_play = Some(id);
            self.recs_dirty(Duration::ZERO);
            return;
        }
        match id {
            v::MixId::Daily => self.play_daily(),
            v::MixId::Mood(m) => {
                let m = from_view_mood(m);
                let seed = nikgob_recs::RadioSeed::Mood(m);
                self.start_radio(seed, m.title().to_owned(), EventContext::HomeSection, None);
            }
            v::MixId::Style(i) => {
                let style = self
                    .recs
                    .home
                    .as_ref()
                    .and_then(|h| h.styles.get(usize::from(i)))
                    .map(|(m, title, _)| (m.seed(), title.clone()));
                match style {
                    Some((seed, title)) => self.start_radio(seed, title, EventContext::HomeSection, None),
                    None => self.toast("Этого микса уже нет — главная обновилась", None),
                }
            }
            v::MixId::Artist(a) => {
                let name = self.lib.get_artist(a).ok().flatten().map_or_else(String::new, |a| a.name);
                let seed = nikgob_recs::RadioSeed::Artist(a);
                self.start_radio(seed, format!("Радио · {name}"), EventContext::HomeSection, None);
            }
            v::MixId::Discover => {
                if self.state.read().prefs.mixes_downloaded_only {
                    self.toast("Включено «Только скачанное» — нового сейчас нет", None);
                    return;
                }
                if self.discoveries().is_empty() {
                    self.toast("Новое ещё ищется — загляни чуть позже", None);
                    return;
                }
                let seed = nikgob_recs::RadioSeed::Discover;
                self.start_radio(seed, "Новое для тебя".to_owned(), EventContext::HomeSection, None);
            }
            v::MixId::Daylist => {
                let daylist = self.recs.home.as_ref().and_then(|h| h.daylist.as_ref());
                let Some(d) = daylist else {
                    self.toast("Нечего играть", None);
                    return;
                };
                let (state, picks, title) = (d.state.clone(), d.picks.clone(), d.title.clone());
                let streams = d.streams.values().cloned().collect();
                self.start_radio_picked(state, picks, streams, title, EventContext::HomeSection);
            }
            v::MixId::Lane(v::MixKey::World(kind)) => {
                let mix = self.recs.world.as_ref().and_then(|(_, m)| m.iter().find(|m| m.0 == kind));
                match mix.map(|m| (m.1.clone(), m.2.iter().map(|&id| Pick { item: Item::Track(id), reason: Reason::Random }).collect::<Vec<_>>())) {
                    Some((title, picks)) => self.play_picks(&picks, &HashMap::new(), &title),
                    None => self.toast("Этого микса уже нет — страница мира обновилась", None),
                }
            }
            v::MixId::Lane(key) => {
                let with_new = {
                    let p = &self.state.read().prefs;
                    p.mixes_with_new && !p.mixes_downloaded_only
                };
                let lane = self.recs.home.as_ref().and_then(|h| h.lanes.iter().find(|l| l.key == key));
                match lane.map(|l| (l.picks(with_new), l.new.clone(), l.title.clone())) {
                    Some((picks, new, title)) => {
                        let streams = new.into_iter().map(|t| (t.id.clone(), t)).collect();
                        self.play_picks(&picks, &streams, &title);
                    }
                    None => self.toast("Этого микса уже нет — главная обновилась", None),
                }
            }
            v::MixId::Tile(v::TileKind::Lottery) => self.play_lottery(),
            v::MixId::Tile(kind) => match self.home_tile(kind).map(|t| (t.picks.clone(), t.title.clone())) {
                Some((picks, title)) => self.play_picks(&picks, &HashMap::new(), &title),
                None => self.toast("Этой подборки уже нет — главная обновилась", None),
            },
        }
    }

    /// Keeps why each of `entries` was picked (`whys` beside them).
    pub(crate) fn explain_entries(&mut self, entries: &[u64], whys: Vec<Option<String>>) {
        for (e, why) in entries.iter().zip(whys) {
            if let Some(why) = why {
                self.recs.why.insert(*e, why);
            }
        }
    }

    /// The mixes of the page of `world` (recs-v2 §8.5): «Лучшее», «Ещё не
    /// слышал», «В дорогу», then its calm and its lively side, those it has;
    /// none before the first build. Kept for [`v::MixKey::World`] to play.
    pub(crate) fn world_mix_cards(&mut self, world: &v::World) -> Vec<v::LaneCard> {
        let Some(snap) = self.recs.snap.clone() else {
            self.recs.world = None;
            return Vec::new();
        };
        let (part, _, _) = daylist_key(recs_now(), local_day().1);
        let token = nikgob_recs::world_token(&world.key);
        let local_only = self.state.read().prefs.mixes_downloaded_only;
        let started = Instant::now();
        let recs = snap.recs();
        let lib = &self.lib;
        let title = |i: usize| lib.get_track(snap.model.id(i)).ok().flatten().map(|t| t.title);
        let mut mixes: Vec<(v::WorldMix, Vec<TrackId>, Option<String>)> = recs
            .world_mixes(token, part)
            .into_iter()
            .map(|m| {
                let kind = match m.kind {
                    nikgob_recs::WorldMixKind::Best => v::WorldMix::Best,
                    nikgob_recs::WorldMixKind::Unheard => v::WorldMix::Unheard,
                    nikgob_recs::WorldMixKind::Road => v::WorldMix::Road,
                    nikgob_recs::WorldMixKind::Calm => v::WorldMix::Calm,
                    nikgob_recs::WorldMixKind::Moving => v::WorldMix::Moving,
                };
                let why = recs.why_world_mix(&m, &world.title, &title);
                (kind, m.tracks.iter().map(|&i| snap.model.id(i)).collect(), why)
            })
            .collect();
        mixes.sort_by_key(|m| m.0);
        log::debug!("world mixes of «{}» in {:?}", world.title, started.elapsed());
        let mut kept = Vec::new();
        let mut cards = Vec::new();
        for (kind, ids, why) in mixes {
            let rows: Vec<nikgob_library::TrackRow> =
                ids.iter().filter_map(|id| self.lib.get_track_row(*id).ok().flatten()).filter(|t| t.has_local || !local_only).collect();
            if rows.is_empty() {
                continue;
            }
            // Each credited artist once, the most frequent first (as a mix of
            // Главная says them).
            let mut count: Vec<(String, usize)> = Vec::new();
            let mut releases: Vec<ReleaseId> = Vec::new();
            for t in &rows {
                for part in t.credit.0.iter().filter(|p| !p.name.is_empty()) {
                    match count.iter_mut().find(|c| c.0 == part.name) {
                        Some(c) => c.1 += 1,
                        None => count.push((part.name.clone(), 1)),
                    }
                }
                if let Some(r) = t.release_id
                    && !releases.contains(&r)
                {
                    releases.push(r);
                }
            }
            count.sort_by(|a, b| b.1.cmp(&a.1));
            let artists: Vec<String> = count.into_iter().map(|c| c.0).collect();
            let covers = releases.into_iter().filter_map(|r| self.res.release_cover(r)).take(COVERS).collect();
            let name = kind.title();
            let mut rest = name.chars();
            let lower: String = rest.next().map(|c| c.to_lowercase().chain(rest).collect()).unwrap_or_default();
            let queue = format!("{}: {lower}", world.title);
            kept.push((kind, queue.clone(), rows.iter().map(|t| t.id).collect()));
            cards.push(v::LaneCard {
                key: v::MixKey::World(kind),
                title: name.to_owned(),
                short_titles: Vec::new(),
                artists,
                new_artists: Vec::new(),
                covers,
                new_cover: None,
                tracks: rows.iter().map(|t| self.res.track(t)).collect(),
                new_tracks: Vec::new(),
                blend_base: None,
                queue: Some(queue),
                why,
            });
        }
        self.recs.world = Some((world.key.clone(), kept));
        cards
    }

    /// The page of a world open gets its mixes again (a build is done).
    fn world_mixes_again(&mut self) {
        let world = self.state.read().world.as_ref().map(|w| w.world.clone());
        let Some(world) = world else { return };
        let cards = self.world_mix_cards(&world);
        self.publish(|s| {
            if let Some(w) = s.world.as_mut().filter(|w| w.world.key == world.key) {
                w.mixes = cards;
            }
        });
    }

    /// «Наугад»: a fresh draw at every click, so what plays (the first song
    /// too) cannot be seen before.
    fn play_lottery(&mut self) {
        let Some(snap) = self.recs.snap.clone() else { return };
        let picks = snap.recs().lottery(fastrand::u64(..));
        if picks.is_empty() {
            self.toast("Нечего играть", None);
            return;
        }
        let title = self.home_tile(v::TileKind::Lottery).map_or_else(|| "Лотерея".to_owned(), |t| t.title.clone());
        self.play_picks(&picks, &HashMap::new(), &title);
    }

    /// A tile of today's board.
    fn home_tile(&self, kind: v::TileKind) -> Option<&tiles::TileHome> {
        self.recs.home.as_ref()?.tiles.iter().find(|t| t.mix_id() == v::MixId::Tile(kind))
    }

    /// `Command::DownloadMix`: every song of a tile without a file goes to
    /// «Хочу скачать».
    fn download_mix(&mut self, id: v::MixId) {
        let v::MixId::Tile(kind) = id else { return };
        let ids: Vec<TrackId> = self.home_tile(kind).map_or_else(Vec::new, |t| {
            t.picks
                .iter()
                .filter_map(|p| match p.item {
                    Item::Track(id) => Some(id),
                    Item::Stream(_) => None,
                })
                .collect()
        });
        if ids.is_empty() {
            self.toast("Этой подборки уже нет — главная обновилась", None);
            return;
        }
        self.download_tracks(&ids);
    }

    /// «Твой микс» as it is today: its tracks, not endless.
    fn play_daily(&mut self) {
        let Some(h) = &self.recs.home else { return };
        let picks = h.daily.clone();
        let streams = h.daily_streams.clone();
        self.play_picks(&picks, &streams, "Твой микс");
    }

    /// A mix of the home as it is (not endless), from its first track.
    fn play_picks(&mut self, picks: &[Pick], streams: &HashMap<RemoteId, StreamTrack>, title: &str) {
        // «Только скачанное» holds even for a mix picked before it was on.
        let prefs = self.state.read().prefs.clone();
        let snap = self.snapshot();
        let mut ids = Vec::with_capacity(picks.len());
        // New music says where it was found.
        let mut whys = Vec::with_capacity(picks.len());
        for p in picks {
            match &p.item {
                Item::Track(id) => {
                    if radio::radio_allows(*id, false, false, &prefs, snap.as_deref(), v::RadioKind::Mix) {
                        ids.push(*id);
                        whys.push(None);
                    }
                }
                Item::Stream(_) if prefs.mixes_downloaded_only => {}
                Item::Stream(r) => {
                    if let Some(t) = streams.get(r) {
                        self.streaming.learn(std::slice::from_ref(t));
                        for id in self.upsert_streams(std::slice::from_ref(t)) {
                            ids.push(id);
                            whys.push(self.discovery_why(t));
                        }
                    }
                }
            }
        }
        if ids.is_empty() {
            self.toast("Нечего играть", None);
            return;
        }
        self.user_switch(nikgob_core::model::EventOrigin::Mix);
        // Shuffled or in its order, as the setting of mixes says.
        self.new_list(Some(v::ListKind::Mix));
        self.queue.set_from(&ids, None, Some(format!("микса «{title}»")));
        // The entries in the mix's own order, as `whys` are.
        let entries: Vec<u64> = self.queue.context_order().iter().map(|e| e.id).collect();
        self.recs.why.clear();
        self.explain_entries(&entries, whys);
        self.play_context = EventContext::HomeSection;
        self.play_context_id = Some(format!("mix:{title}"));
        if let Some(e) = self.queue.current() {
            self.start(e);
        }
    }

    /// `Command::PlayBlend`: «Смешать с…». The blend is drawn now from the
    /// snapshot, plays, and stands first in «Миксы для тебя» until the
    /// day's end (the next builds draw it again the same).
    fn play_blend(&mut self, profiles: &[u32]) {
        let Some(snap) = self.recs.snap.clone() else {
            self.toast("Рекомендации ещё собираются — попробуй чуть позже", None);
            return;
        };
        let prefs = lanes::MixPrefs::of(&self.state.read().prefs);
        let lib = self.lib.clone();
        let names = |a: nikgob_core::model::ArtistId| lib.get_artist(a).ok().flatten().map(|a| a.name);
        let epoch = self.home_epoch();
        let Some(lane) = lanes::blend_now(&snap, &prefs, profiles, &names, &self.recs.session, epoch.seed) else {
            self.toast("Из этих профилей микса не собрать", None);
            return;
        };
        let (day, _) = local_day();
        lanes::remember_blend(&self.lib, &prefs, &day, &epoch.key, recs_now(), lane.key);
        self.recs.blends.retain(|b| b.0 == day && b.1 != lane.key);
        self.recs.blends.push((day, lane.key));
        let (picks, title) = (lane.picks(false), lane.title.clone());
        if let Some(h) = self.recs.home.as_mut() {
            h.lanes.retain(|l| l.key != lane.key);
            let at = lanes::blend_place(&h.lanes);
            h.lanes.insert(at, lane);
            // As the next build has it: the latest blends in the places of
            // the moments that fit now least.
            let blend = |l: &lanes::LaneHome| matches!(l.key, v::MixKey::Blend(_));
            while h.lanes.iter().filter(|l| blend(l)).count() > nikgob_recs::mixes::BLENDS_SHOWN {
                if let Some(k) = h.lanes.iter().rposition(|l| blend(l)) {
                    h.lanes.remove(k);
                }
            }
            while h.lanes.iter().filter(|l| lanes::is_moment(l.key)).count() > nikgob_recs::mixes::MOMENTS {
                if let Some(k) = h.lanes.iter().rposition(|l| lanes::is_moment(l.key) && !blend(l)) {
                    h.lanes.remove(k);
                } else {
                    break;
                }
            }
        }
        self.publish_recs_home();
        self.play_picks(&picks, &HashMap::new(), &title);
    }

    /// `Command::RefreshMix`.
    fn refresh_mix(&mut self, id: v::MixId) {
        match id {
            v::MixId::Daily => {
                self.recs.refresh_daily = true;
                self.recs_dirty(Duration::ZERO);
                if !self.recs.building {
                    self.recs_build();
                }
            }
            v::MixId::Discover => self.discover_now(),
            v::MixId::Daylist => {
                self.recs.refresh_daylist = true;
                self.recs_dirty(Duration::ZERO);
                if !self.recs.building {
                    self.recs_build();
                }
            }
            // «На повторе» is what played: nothing to draw again.
            v::MixId::Lane(v::MixKey::OnRepeat) => {}
            v::MixId::Lane(key) => {
                self.recs.refresh_mix = Some(key);
                self.recs_dirty(Duration::ZERO);
                if !self.recs.building {
                    self.recs_build();
                }
            }
            // Moods, styles and artists are endless: nothing to pick again.
            v::MixId::Mood(_) | v::MixId::Style(_) | v::MixId::Artist(_) => {}
            // The tiles are what the history says today.
            v::MixId::Tile(_) => {}
        }
    }

    /// `Command::RecFeedback`: «Не рекомендовать» (and its undo).
    fn rec_feedback(&mut self, target: v::RecTarget, dislike: bool) {
        let mut marks: Vec<(&'static str, String)> = Vec::new();
        let name = match &target {
            v::RecTarget::Track(t) => {
                marks.push(("track", t.to_string()));
                self.lib.get_track(*t).ok().flatten().map(|t| t.title)
            }
            v::RecTarget::Artist(a) => {
                marks.push(("artist", a.to_string()));
                self.lib.get_artist(*a).ok().flatten().map(|a| a.name)
            }
            v::RecTarget::StreamTrack(r) => {
                let t = self.stream_track(r);
                if let Some(t) = &t {
                    marks.push((SONG, stream_song_key(t)));
                }
                // The same song in the library is out too.
                if let Some(id) = self.lib.track_by_remote(r).ok().flatten() {
                    marks.push(("track", id.to_string()));
                }
                t.map(|t| t.title)
            }
            v::RecTarget::StreamArtist(name) => {
                let key = artist_key(name);
                if !key.is_empty() {
                    marks.push((ARTIST_NAME, key));
                }
                Some(name.clone())
            }
        };
        if marks.is_empty() {
            log::warn!("rec feedback for an unknown {target:?}");
            return;
        }
        for (kind, id) in &marks {
            if self.log_err("отзыв", self.lib.set_rec_feedback(kind, id, dislike)).is_none() {
                return;
            }
        }
        let artist = matches!(target, v::RecTarget::Artist(_) | v::RecTarget::StreamArtist(_));
        let name = name.unwrap_or_default();
        if dislike {
            let text = if artist {
                format!("Не буду рекомендовать исполнителя «{name}»")
            } else {
                format!("Не буду рекомендовать «{name}»")
            };
            self.toast(text, Some(("Отменить".into(), Command::RecFeedback { target, dislike: false })));
        } else {
            self.toast(format!("«{name}» снова в рекомендациях"), Some(("Отменить".into(), Command::RecFeedback { target, dislike: true })));
        }
        // Discoveries follow at once; the mixes after the next build.
        self.dislikes_load(true);
        self.publish_recs_home();
        self.recs_dirty(Duration::ZERO);
    }

    /// A stream track the runtime knows by its id (a search, a page, a
    /// discovery).
    pub(crate) fn stream_track(&self, r: &RemoteId) -> Option<StreamTrack> {
        self.streaming
            .known
            .get(r)
            .cloned()
            .or_else(|| self.discoveries().into_iter().find(|t| t.id == *r))
            .or_else(|| self.recs.home.as_ref().and_then(|h| h.daily_streams.get(r).cloned()))
            .or_else(|| {
                let d = self.recs.home.as_ref()?.daylist.as_ref()?;
                d.streams.get(r).cloned()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// «Твой микс» and the daylist kept for the day are picked again once
    /// a later pass found new music (not under «Только скачанное», where
    /// they take none).
    #[test]
    fn a_later_pass_redraws_the_days_picks() {
        assert!(new_kept(500, 500, false));
        assert!(new_kept(600, 500, false));
        assert!(!new_kept(0, 500, false), "kept before any pass");
        assert!(!new_kept(400, 500, false));
        assert!(new_kept(0, 500, true));
        assert!(new_kept(0, 0, false), "no pass yet");
    }

    /// The page of a mix lists the new songs «Слушать» plays, in their
    /// order, and only those that find a place between loved ones.
    #[test]
    fn a_mix_page_lists_its_new_songs() {
        use nikgob_core::streaming::Service;
        let new: Vec<StreamTrack> = (0..3).map(|k| StreamTrack::new(RemoteId::new(Service::Youtube, format!("v{k}")), format!("New {k}"))).collect();
        let lane = lanes::LaneHome {
            key: v::MixKey::Calm,
            title: "Спокойное".into(),
            short: Vec::new(),
            tracks: (0..12).map(|_| TrackId::new()).collect(),
            // Two gaps between loved ones only: two of the three play.
            anchors: (0..12).map(|k| !matches!(k, 0 | 1 | 4 | 5 | 6 | 7 | 10 | 11)).collect(),
            artists: Vec::new(),
            covers: Vec::new(),
            new,
            open: false,
            niche: Vec::new(),
            blend_base: None,
            why: None,
        };
        let home = RecHome {
            day: String::new(),
            epoch: String::new(),
            daily: Vec::new(),
            daily_streams: HashMap::new(),
            daily_covers: Vec::new(),
            history: true,
            moods: Vec::new(),
            styles: Vec::new(),
            daylist_key: String::new(),
            daylist: None,
            tiles: Vec::new(),
            lanes: vec![lane],
            blend_profiles: Vec::new(),
            top: Vec::new(),
        };
        let parts = HomeParts::of(Some(&home), Vec::new(), Vec::new(), false, false);
        let played: Vec<RemoteId> = home.lanes[0]
            .picks(true)
            .into_iter()
            .filter_map(|p| match p.item {
                Item::Stream(r) => Some(r),
                Item::Track(_) => None,
            })
            .collect();
        assert_eq!(played.len(), 2, "{played:?}");
        let rows: Vec<RemoteId> = parts.lanes[0].new_rows.iter().map(|t| t.id.clone()).collect();
        assert_eq!(rows, played);
    }

    /// Cards of one row lead with different pictures while they have them.
    #[test]
    fn the_cards_of_a_row_lead_with_different_covers() {
        let mut covers = vec![vec![1, 2, 3], vec![1, 2, 3], vec![2, 1], vec![1], Vec::new(), vec![3, 4]];
        distinct_leads(&mut covers);
        assert_eq!(covers, [vec![1, 2, 3], vec![2, 1, 3], vec![2, 1], vec![1], vec![], vec![3, 4]]);
    }


    #[test]
    fn style_titles_name_their_artists() {
        let n = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(style_title(&n(&["mzlff", "MORGENSHTERN"]), 7), "mzlff, MORGENSHTERN и другие");
        assert_eq!(style_title(&n(&["A", "B"]), 2), "A и B");
        assert_eq!(style_title(&n(&["A"]), 1), "A");
    }

    /// «ATM 4» of two artists' discographies, a tile twice: each row keeps
    /// the first.
    #[test]
    fn home_rows_show_each_item_once() {
        use nikgob_core::streaming::Service;
        let album = |id: &str, artist: &str| v::StreamAlbumCard {
            remote: RemoteId::new(Service::Deezer, id),
            title: "ATM 4".into(),
            artist: artist.into(),
            artists: v::StreamArtistLinks::default(),
            year: Some(2026),
            release_type: nikgob_core::model::ReleaseType::Album,
            cover_url: None,
            total_tracks: None,
            release: None,
            have: v::AlbumHave::default(),
        };
        let card = |id, title: &str| v::MixCard {
            id,
            title: title.into(),
            kicker: String::new(),
            subtitle: String::new(),
            covers: Vec::new(),
        };
        let mut home = v::HomeView {
            new_releases: vec![album("1", "Mzlff"), album("2", "Mzlff"), album("1", "Guest")],
            tiles: vec![
                card(v::MixId::Tile(v::TileKind::OnRepeat), "На повторе"),
                card(v::MixId::Tile(v::TileKind::OnRepeat), "На повторе"),
                card(v::MixId::Tile(v::TileKind::Lottery), "Лотерея"),
            ],
            style_mixes: vec![card(v::MixId::Style(0), "A и B"), card(v::MixId::Style(0), "A и B")],
            ..v::HomeView::default()
        };
        dedupe_rows(&mut home);
        let ids: Vec<(&str, &str)> = home.new_releases.iter().map(|a| (a.remote.id.as_str(), a.artist.as_str())).collect();
        assert_eq!(ids, [("1", "Mzlff"), ("2", "Mzlff")]);
        assert_eq!(home.tiles.len(), 2);
        assert_eq!(home.style_mixes.len(), 1);
    }

    #[test]
    fn moods_map_both_ways() {
        for m in Mood::ALL {
            assert_eq!(from_view_mood(to_view_mood(m)), m);
            assert_eq!(to_view_mood(m).title(), m.title());
        }
    }
}
