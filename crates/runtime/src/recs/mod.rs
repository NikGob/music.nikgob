//! Recommendations in the runtime (docs/stage4.md §3–§6): the engine of
//! `nikgob-recs` over the library, kept off the worker thread.
//!
//! * **Snapshot.** The [`Catalog`] (library tracks and liked or playlist
//!   stream tracks), the [`Taste`] of the event log, likes, playlists and
//!   «Не рекомендовать», and a copy of the sound index without vectors of
//!   tracks that are gone, plus what the discovery filter needs. Built on a
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
//!   **«Не рекомендовать»** with its undo (below).
//!
//! [`StreamSchedule::recs_debounce`]: crate::StreamSchedule::recs_debounce
//! [`StreamSchedule::recs_every`]: crate::StreamSchedule::recs_every

mod discover;
mod radio;
mod suggest;
mod tiles;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use nikgob_core::model::{ArtistId, EventContext, ReleaseId, TrackId, UnixMs};
use nikgob_core::streaming::{RemoteId, StreamTrack};
use nikgob_library::{Library, now_ms};
use nikgob_recs::{
    Catalog, DAYLIST_LEN, DailyParams, DayPart, DiscoveryFilter, Item, Mood, NewTrack, Pick, RadioParams,
    RadioState as Engine, Recs, ServiceSuggestions, StyleMix, Taste, TasteInput, TrackInfo, VectorIndex, album_key,
    artist_key, daylist_title, daylist_until, match_suggestions, song_key, stream_song_key, style_words,
};
use nikgob_view::{self as v, Command};
use serde::{Deserialize, Serialize};

use crate::Worker;
use crate::analysis::{SoundIndex, lower_priority};
use crate::import::Internal;

pub(crate) use radio::RadioSave;
pub use tiles::{PreviewTile, preview_ready_tiles, preview_tile_days};

/// Plays after which the taste is built again.
const PLAYS_PER_BUILD: u32 = 20;
/// The analysis added this many vectors (or a tenth of the index, or
/// doubled it) since the last build: built again.
const NEW_VECTORS: usize = 200;
/// Library cache keys.
const DAILY_KEY: &str = "recs.daily";
const DAYLIST_KEY: &str = "recs.daylist";
const STYLES_KEY: &str = "recs.styles";
/// Covers of a mix card…
const COVERS: usize = 4;
/// …from this many releases (some have no cover).
const COVER_CANDIDATES: usize = 8;
/// Top tracks of a mood looked at for its covers.
const MOOD_COVER_POOL: usize = 60;

/// `rec_feedback` target kinds beyond the library's `track` and `artist`:
/// a song of the services by [`song_key`], an artist of the services by
/// [`artist_key`].
pub(crate) const SONG: &str = "song";
pub(crate) const ARTIST_NAME: &str = "artist_name";

/// The engine's inputs at one moment.
pub(crate) struct Snapshot {
    pub catalog: Catalog,
    pub taste: Taste,
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
        Recs::new(&self.catalog, &self.taste, &self.index).local_only(self.local_only)
    }

    /// A new-music candidate that is new indeed: not in the library, not
    /// disliked.
    pub fn is_new(&self, t: &StreamTrack) -> bool {
        is_new(&self.filter, t)
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
}

/// The daylist of the home.
pub(crate) struct DaylistHome {
    /// «НА ВЕЧЕР».
    pub kicker: &'static str,
    pub title: String,
    /// «что ты слушаешь по вечерам · до 23:00».
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
    Discover(discover::DiscoverMsg),
    Suggest(suggest::SuggestMsg),
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
}

/// The worker's recommendation state.
pub(crate) struct RecsState {
    tx: Sender<Internal>,
    snap: Option<Arc<Snapshot>>,
    home: Option<RecHome>,
    building: bool,
    /// A build is due then.
    due: Option<Instant>,
    built_at: Option<Instant>,
    built_index_len: usize,
    plays: u32,
    /// The next build picks «Твой микс» again.
    refresh_daily: bool,
    /// The next build picks the daylist again.
    refresh_daylist: bool,
    /// A mix asked for before the first build.
    pending_play: Option<v::MixId>,
    pub(crate) radio: radio::RadioState,
    pub(crate) discover: discover::DiscoverState,
    pub(crate) suggest: suggest::SuggestState,
    /// The services' suggestions changed: the discoveries are ranked again
    /// after the next build.
    discover_rerank: bool,
}

impl RecsState {
    pub(crate) fn new(tx: Sender<Internal>) -> Self {
        Self {
            tx,
            snap: None,
            home: None,
            building: false,
            due: Some(Instant::now()),
            built_at: None,
            built_index_len: 0,
            plays: 0,
            refresh_daily: false,
            refresh_daylist: false,
            pending_play: None,
            radio: radio::RadioState::default(),
            discover: discover::DiscoverState::default(),
            suggest: suggest::SuggestState::default(),
            discover_rerank: false,
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

/// How much new music radios and mixes mix in: «Новое в радио», none under
/// «Миксы только из скачанного».
pub(crate) fn new_share(prefs: &v::Prefs) -> f32 {
    if prefs.mixes_downloaded_only { 0.0 } else { prefs.radio_new_share.fraction() }
}

/// Now, for the recommendations.
pub(crate) fn recs_now() -> UnixMs {
    now_ms() + day_shift_ms()
}

/// The local day and the minutes east of UTC.
pub(crate) fn local_day() -> (String, i32) {
    let now = jiff::Zoned::now();
    let now = now.checked_add(jiff::SignedDuration::from_millis(day_shift_ms())).unwrap_or(now);
    (now.date().to_string(), now.offset().seconds() / 60)
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

/// «что ты слушаешь по вечерам · до 23:00»: what the daylist is made of.
fn daylist_subtitle(part: DayPart) -> String {
    let when = match part {
        DayPart::Morning => "по утрам",
        DayPart::Day => "днём",
        DayPart::Evening => "по вечерам",
        DayPart::Night => "по ночам",
    };
    format!("что ты слушаешь {when} · {}", daylist_until(part))
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
            .is_some_and(|t| !snap.taste.blocks(t) && (t.local || !snap.local_only)),
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
    utc_offset_min: i32,
    now: UnixMs,
    refresh_daily: bool,
    refresh_daylist: bool,
    new_share: f32,
    /// «Миксы только из скачанного».
    local_only: bool,
    /// New music for the daily mix (the discoveries).
    new: Vec<StreamTrack>,
    /// The services' suggestions, of the services switched on.
    suggested: Vec<suggest::Kept>,
    /// They changed: the discoveries are ranked again after this build.
    rerank: bool,
}

fn build(input: BuildInput) -> Result<Built, String> {
    let started = Instant::now();
    let (snap, tiles_input, index_len) =
        read_snapshot(&input.lib, &input.index, input.now, input.utc_offset_min, &input.suggested, input.local_only)?;
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
    let err = |what: &str, e: nikgob_library::LibraryError| format!("{what}: {e}");
    let rows = lib.rec_tracks().map_err(|e| err("tracks", e))?;
    let events = lib.listening_events().map_err(|e| err("events", e))?;
    let like_times = lib.liked_track_times().map_err(|e| err("likes", e))?;
    let entries = lib.playlist_entry_times().map_err(|e| err("playlists", e))?;
    let feedback = lib.rec_dislikes().map_err(|e| err("feedback", e))?;
    let artists = lib.artist_names().map_err(|e| err("artists", e))?;
    let remote = lib.track_remote_ids().map_err(|e| err("remote ids", e))?;
    let releases = lib.release_names().map_err(|e| err("releases", e))?;

    // One song on two albums: the track with a file (the oldest) stands for it.
    let mut songs: HashMap<&str, (bool, TrackId)> = HashMap::new();
    for r in rows.iter().filter(|r| r.in_library) {
        let e = songs.entry(r.song.as_str()).or_insert((r.has_local, r.id));
        if r.has_local && !e.0 {
            *e = (true, r.id);
        }
    }
    let catalog = Catalog::new(rows.iter().filter(|r| r.in_library).map(|r| {
        let song = songs.get(r.song.as_str()).map_or(r.id, |s| s.1);
        TrackInfo::new(r.id, r.artists.clone(), r.release).with_local(r.has_local).with_song(song)
    }));
    let mut artist_keys: HashMap<String, ArtistId> = HashMap::new();
    let mut names: HashMap<ArtistId, String> = HashMap::new();
    for (id, name) in &artists {
        artist_keys.entry(artist_key(name)).or_insert(*id);
        names.insert(*id, name.clone());
    }

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
    filter.known_ids = remote.into_iter().collect();
    filter.known_songs = rows
        .iter()
        .filter(|r| r.in_library)
        .map(|r| song_key(&r.title, &r.artist))
        .collect();
    filter.known_artists = artist_keys.keys().cloned().collect();
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
    let suggested_new: Vec<(StreamTrack, f32)> = matched.unmatched.into_iter().filter(|(t, _)| is_new(&filter, t)).collect();

    let taste = Taste::build(
        &catalog,
        &TasteInput {
            events: events.iter().map(nikgob_recs::Play::from).collect(),
            likes: like_times.iter().map(|l| l.0).collect(),
            like_times,
            playlist_entries: entries,
            disliked_tracks,
            disliked_artists,
            utc_offset_min,
            suggested_tracks: matched.tracks,
            suggested_artists: matched.artists,
        },
        now,
    );
    // Vectors of tracks that are gone (or left the library) stay in the
    // shared index until a restart; not in the snapshot.
    let mut index = index.read().clone();
    let index_len = index.len();
    let stale: Vec<TrackId> = index.iter().map(|(id, _)| id).filter(|id| catalog.position(*id).is_none()).collect();
    for id in stale {
        index.remove(id);
    }
    let snap = Snapshot {
        catalog,
        taste,
        index,
        filter,
        artist_keys,
        known_albums,
        played_songs,
        suggested_new,
        local_only,
    };
    let tiles_input = tiles::input(lib, &rows, &events, names);
    Ok((snap, tiles_input, index_len))
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
                && (c.history || !snap.taste.has_history())
                && c.local_only == input.local_only
        });
    let (picks, streams) = match cached {
        Some(c) => (c.picks, c.streams),
        None => {
            let new: Vec<&StreamTrack> = input.new.iter().filter(|t| !input.local_only && snap.is_new(t)).collect();
            let candidates: Vec<NewTrack> = new.iter().map(|t| NewTrack::from_stream(t, None)).collect();
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
                history: snap.taste.has_history(),
                local_only: input.local_only,
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

    // Moods: covers of the releases most liked among each mood's top.
    let mut moods = Vec::new();
    if !snap.index.is_empty() {
        for m in recs.mood_order() {
            let mut top: Vec<(TrackId, f32)> = recs.mood_pool(m).into_iter().take(MOOD_COVER_POOL).collect();
            let aff = |id: TrackId| snap.catalog.position(id).map_or(0.0, |i| snap.taste.affinity_at(i));
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
        daily,
        daily_streams,
        daily_covers,
        history: snap.taste.has_history(),
        moods,
        styles,
        daylist_key,
        daylist,
        tiles,
    }
}

/// The daylist of now's part of the day: the one kept for this part,
/// unless «Обновить» or a history appeared since.
fn build_daylist(snap: &Snapshot, input: &BuildInput) -> (String, Option<DaylistHome>) {
    let lib = &input.lib;
    let history = snap.taste.has_history();
    let (part, day, key) = daylist_key(input.now, input.utc_offset_min);
    let cached = cache_read::<DaylistCache>(lib, DAYLIST_KEY)
        .filter(|c| c.key == key && !input.refresh_daylist && (c.history || !history) && c.local_only == input.local_only);
    let c = match cached {
        Some(c) => c,
        None => {
            let new: Vec<&StreamTrack> = input.new.iter().filter(|t| !input.local_only && snap.is_new(t)).collect();
            let candidates: Vec<NewTrack> = new.iter().map(|t| NewTrack::from_stream(t, None)).collect();
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
    let subtitle = if history {
        daylist_subtitle(part)
    } else {
        "Слушай — подборка соберётся из того, что нравится".to_owned()
    };
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

/// The recommendation parts of the home view, ready to be drawn.
pub(crate) struct HomeParts {
    hero: Option<(String, String, Vec<CoverRef>)>,
    mix: Vec<nikgob_library::TrackRow>,
    /// Kicker, title, subtitle, covers.
    daylist: Option<(String, String, String, Vec<CoverRef>)>,
    /// Id, kicker, title, subtitle, covers.
    tiles: Vec<(v::MixId, String, String, String, Vec<CoverRef>)>,
    moods: Vec<(v::Mood, Vec<CoverRef>)>,
    styles: Vec<(String, String, Vec<CoverRef>)>,
    discover: Vec<v::StreamTrackRow>,
    new_releases: Vec<v::StreamAlbumCard>,
    /// «Только скачанное»: nothing on the home may play without a file.
    local_only: bool,
}

impl HomeParts {
    /// The parts of `home` (none before the first build).
    fn of(
        lib: &Library,
        home: Option<&RecHome>,
        discover: Vec<v::StreamTrackRow>,
        new_releases: Vec<v::StreamAlbumCard>,
        local_only: bool,
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
                local_only,
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
                Item::Track(id) => lib.get_track_row(id).ok().flatten(),
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
            .map(|t| (t.mix_id(), t.kicker().to_owned(), t.title.clone(), t.subtitle.clone(), t.covers.clone()))
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
            local_only,
        }
    }

    /// Fills the recommendation fields of `home`.
    pub(crate) fn apply(self, home: &mut v::HomeView, res: &mut crate::Resolver) {
        self.resolve(res)(home);
    }

    /// Resolves covers and rows now and returns what fills `home` with them
    /// (it touches neither the library nor the resolver, so it can run
    /// under the state lock).
    pub(crate) fn resolve(self, res: &mut crate::Resolver) -> impl FnOnce(&mut v::HomeView) + use<> {
        let local_only = self.local_only;
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
        let mix: Option<Vec<_>> = (!self.mix.is_empty()).then(|| self.mix.iter().map(|t| res.track(t)).collect());
        let (discover, new_releases) = (self.discover, self.new_releases);
        move |home: &mut v::HomeView| {
            home.hero = hero;
            home.daylist = daylist;
            home.tiles = tiles;
            home.moods = moods;
            home.style_mixes = style_mixes;
            if let Some(mix) = mix {
                home.mix = mix;
            }
            home.discover = discover;
            home.new_releases = new_releases;
            if local_only {
                only_downloaded(home);
            }
            dedupe_rows(home);
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
    let input = BuildInput {
        lib: lib.clone(),
        index,
        day,
        utc_offset_min,
        now: recs_now(),
        refresh_daily: false,
        refresh_daylist: false,
        new_share: new_share(prefs),
        local_only: prefs.mixes_downloaded_only,
        new: discoveries.clone(),
        suggested: suggest::cached(lib),
        rerank: false,
    };
    let built = build(input)?;
    let h = &built.home;
    let mut res = crate::Resolver::new(lib.clone(), library_dir.to_owned());
    let mut home = res.home(&crate::home::build(lib));
    HomeParts::of(lib, Some(h), Vec::new(), Vec::new(), prefs.mixes_downloaded_only).apply(&mut home, &mut res);
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
        suggested_library: (0..built.snap.catalog.len()).filter(|&i| built.snap.taste.suggested_at(i) > 0.0).count(),
        suggested_new: built.snap.suggested_new.len(),
        took: started.elapsed(),
        home,
    })
}

impl Worker {
    /// The recommendation parts of the home page as they stand.
    pub(crate) fn recs_home_parts(&mut self) -> HomeParts {
        let (discover, new_releases) = self.discover_rows();
        let local_only = self.state.read().prefs.mixes_downloaded_only;
        HomeParts::of(&self.lib, self.recs.home.as_ref(), discover, new_releases, local_only)
    }

    /// Draws the recommendation parts of the home page again.
    pub(crate) fn publish_recs_home(&mut self) {
        let parts = self.recs_home_parts();
        let fill = parts.resolve(&mut self.res);
        self.publish(|s| fill(&mut s.home));
    }

    /// A build is due after `after` (sooner if one is due already).
    pub(crate) fn recs_dirty(&mut self, after: Duration) {
        let at = Instant::now() + after;
        let r = &mut self.recs;
        r.due = Some(r.due.map_or(at, |d| d.min(at)));
    }

    /// «Миксы только из скачанного» changed: the home is built again now
    /// (its kept picks are picked again under the new setting).
    pub(crate) fn recs_prefs_changed(&mut self) {
        self.recs_dirty(Duration::ZERO);
        if !self.recs.building {
            self.recs_build();
        }
    }

    /// The library changed (likes, playlists, imports).
    pub(crate) fn recs_library_changed(&mut self) {
        let d = self.streaming.schedule.recs_debounce;
        self.recs_dirty(d);
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

    /// Every tick: a build when one is due, the discovery job.
    pub(crate) fn recs_tick(&mut self) {
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
        if !r.building && (r.due.is_some_and(|d| now >= d) || stale) {
            self.recs_build();
        }
        self.discover_tick();
        self.suggest_tick();
    }

    fn recs_build(&mut self) {
        let (day, utc_offset_min) = local_day();
        let prefs = self.state.read().prefs.clone();
        let input = BuildInput {
            lib: self.lib.clone(),
            index: self.analysis.index().clone(),
            day,
            utc_offset_min,
            now: recs_now(),
            refresh_daily: std::mem::take(&mut self.recs.refresh_daily),
            refresh_daylist: std::mem::take(&mut self.recs.refresh_daylist),
            new_share: new_share(&prefs),
            local_only: prefs.mixes_downloaded_only,
            new: self.discoveries(),
            suggested: self.suggestions_kept(),
            rerank: std::mem::take(&mut self.recs.discover_rerank),
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
                self.recs.due = None;
            }
            Err(e) => log::warn!("recs: cannot build: {e}"),
        }
    }

    pub(crate) fn recs_msg(&mut self, m: Msg) {
        match m {
            Msg::Built(b) => self.recs_built(*b),
            Msg::Radio(m) => self.radio_msg(m),
            Msg::Discover(m) => self.discover_msg(m),
            Msg::Suggest(m) => self.suggest_msg(m),
        }
    }

    fn recs_built(&mut self, b: Result<Built, String>) {
        self.recs.building = false;
        self.recs.built_at = Some(Instant::now());
        let rerank = match b {
            Ok(b) => {
                let rerank = b.rerank;
                self.recs.built_index_len = b.index_len;
                self.recs.snap = Some(b.snap);
                self.recs.home = Some(b.home);
                rerank
            }
            Err(e) => {
                log::warn!("recs: {e}");
                return;
            }
        };
        self.publish_recs_home();
        if let Some(id) = self.recs.pending_play.take() {
            self.play_mix(id);
        }
        self.radio_snapshot_ready();
        if rerank {
            self.discover_rerank();
        }
        self.discover_tick();
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
            Command::RefreshMix { id } => self.refresh_mix(id),
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
            v::MixId::Tile(v::TileKind::Lottery) => self.play_lottery(),
            v::MixId::Tile(kind) => match self.home_tile(kind).map(|t| (t.picks.clone(), t.title.clone())) {
                Some((picks, title)) => self.play_picks(&picks, &HashMap::new(), &title),
                None => self.toast("Этой подборки уже нет — главная обновилась", None),
            },
        }
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
        for p in picks {
            match &p.item {
                Item::Track(id) => {
                    if radio::radio_allows(*id, false, false, &prefs, snap.as_deref()) {
                        ids.push(*id);
                    }
                }
                Item::Stream(_) if prefs.mixes_downloaded_only => {}
                Item::Stream(r) => {
                    if let Some(t) = streams.get(r) {
                        self.streaming.learn(std::slice::from_ref(t));
                        ids.extend(self.upsert_streams(std::slice::from_ref(t)));
                    }
                }
            }
        }
        if ids.is_empty() {
            self.toast("Нечего играть", None);
            return;
        }
        self.queue.shuffle = false;
        self.queue.set(&ids, 0, Some(format!("микса «{title}»")));
        self.play_context = EventContext::HomeSection;
        if let Some(e) = self.queue.current() {
            self.start(e);
        }
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
            self.toast(format!("«{name}» снова в рекомендациях"), None);
        }
        // Discoveries follow at once; the mixes after the next build.
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
