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
//!   «Обновить» picks again), the mood cards with their covers and the
//!   style mixes (their centres kept in `recs.styles` as the next start).
//! * **Mixes and radio** ([`radio`]), **«Новое для тебя» and new releases**
//!   ([`discover`]), **«Не рекомендовать»** with its undo (below).
//!
//! [`StreamSchedule::recs_debounce`]: crate::StreamSchedule::recs_debounce
//! [`StreamSchedule::recs_every`]: crate::StreamSchedule::recs_every

mod discover;
mod radio;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use nikgob_core::model::{ArtistId, EventContext, ReleaseId, TrackId, UnixMs};
use nikgob_core::streaming::{RemoteId, StreamTrack};
use nikgob_library::{Library, now_ms};
use nikgob_recs::{
    Catalog, DailyParams, DiscoveryFilter, Item, Mood, NewTrack, Pick, Recs, StyleMix, Taste, TasteInput, TrackInfo,
    VectorIndex, album_key, artist_key, song_key, stream_song_key,
};
use nikgob_view::{self as v, Command};
use serde::{Deserialize, Serialize};

use crate::Worker;
use crate::analysis::{SoundIndex, lower_priority};
use crate::import::Internal;

pub(crate) use radio::RadioSave;

/// Plays after which the taste is built again.
const PLAYS_PER_BUILD: u32 = 20;
/// The analysis added this many vectors (or a tenth of the index, or
/// doubled it) since the last build: built again.
const NEW_VECTORS: usize = 200;
/// Library cache keys.
const DAILY_KEY: &str = "recs.daily";
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
}

impl Snapshot {
    pub fn recs(&self) -> Recs<'_> {
        Recs::new(&self.catalog, &self.taste, &self.index)
    }

    /// A new-music candidate that is new indeed: not in the library, not
    /// disliked.
    pub fn is_new(&self, t: &StreamTrack) -> bool {
        let f = &self.filter;
        let key = stream_song_key(t);
        !f.known_ids.contains(&t.id)
            && !f.known_songs.contains(&key)
            && !f.disliked_songs.contains(&key)
            && !t.artists.iter().any(|a| f.disliked_artists.contains(&artist_key(&a.name)))
            && t.playable
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
    /// The style mixes with their titles.
    pub styles: Vec<(StyleMix, String)>,
}

/// What a build hands back.
pub(crate) struct Built {
    snap: Arc<Snapshot>,
    home: RecHome,
    index_len: usize,
}

/// From the builder, the radio and the background calls to the worker.
pub(crate) enum Msg {
    Built(Box<Result<Built, String>>),
    Radio(radio::RadioMsg),
    Discover(discover::DiscoverMsg),
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
    /// A mix asked for before the first build.
    pending_play: Option<v::MixId>,
    pub(crate) radio: radio::RadioState,
    pub(crate) discover: discover::DiscoverState,
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
            pending_play: None,
            radio: radio::RadioState::default(),
            discover: discover::DiscoverState::default(),
        }
    }
}

/// The local day and the minutes east of UTC.
pub(crate) fn local_day() -> (String, i32) {
    let now = jiff::Zoned::now();
    (now.date().to_string(), now.offset().seconds() / 60)
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

/// What the builder thread needs.
struct BuildInput {
    lib: Library,
    index: SoundIndex,
    day: String,
    utc_offset_min: i32,
    now: UnixMs,
    refresh_daily: bool,
    new_share: f32,
    /// New music for the daily mix (the discoveries).
    new: Vec<StreamTrack>,
}

fn build(input: BuildInput) -> Result<Built, String> {
    let started = Instant::now();
    let lib = &input.lib;
    let err = |what: &str, e: nikgob_library::LibraryError| format!("{what}: {e}");
    let rows = lib.rec_tracks().map_err(|e| err("tracks", e))?;
    let events = lib.listening_events().map_err(|e| err("events", e))?;
    let likes = lib.liked_track_ids().map_err(|e| err("likes", e))?;
    let entries = lib.playlist_entry_times().map_err(|e| err("playlists", e))?;
    let feedback = lib.rec_dislikes().map_err(|e| err("feedback", e))?;
    let artists = lib.artist_names().map_err(|e| err("artists", e))?;
    let remote = lib.track_remote_ids().map_err(|e| err("remote ids", e))?;
    let releases = lib.release_names().map_err(|e| err("releases", e))?;

    let catalog = Catalog::new(
        rows.iter()
            .filter(|r| r.in_library)
            .map(|r| TrackInfo::new(r.id, r.artists.clone(), r.release)),
    );
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

    let taste = Taste::build(
        &catalog,
        &TasteInput {
            events: events.iter().map(nikgob_recs::Play::from).collect(),
            likes,
            playlist_entries: entries,
            disliked_tracks,
            disliked_artists,
            utc_offset_min: input.utc_offset_min,
        },
        input.now,
    );
    // Vectors of tracks that are gone (or left the library) stay in the
    // shared index until a restart; not in the snapshot.
    let mut index = input.index.read().clone();
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
    };
    let home = build_home(&snap, &input);
    log::info!(
        "recs: {} tracks, {} vectors, {} events in {:?}",
        snap.catalog.len(),
        snap.index.len(),
        events.len(),
        started.elapsed()
    );
    Ok(Built {
        snap: Arc::new(snap),
        home,
        index_len,
    })
}

fn build_home(snap: &Snapshot, input: &BuildInput) -> RecHome {
    let lib = &input.lib;
    let recs = snap.recs();
    let blocked = |p: &Pick| match &p.item {
        Item::Track(id) => snap.catalog.get(*id).is_none_or(|t| snap.taste.blocks(t)),
        Item::Stream(_) => false,
    };

    // «Твой микс»: the day's, unless it is a new day or «Обновить».
    let cached: Option<DailyCache> = lib
        .cache_get(DAILY_KEY)
        .ok()
        .flatten()
        .and_then(|(_, json)| serde_json::from_str(&json).ok())
        .filter(|c: &DailyCache| c.day == input.day && !input.refresh_daily && (c.history || !snap.taste.has_history()));
    let (picks, streams) = match cached {
        Some(c) => (c.picks, c.streams),
        None => {
            let new: Vec<&StreamTrack> = input.new.iter().filter(|t| snap.is_new(t)).collect();
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
    let daily: Vec<Pick> = picks
        .into_iter()
        .filter(|p| !blocked(p))
        .filter(|p| match &p.item {
            Item::Stream(r) => daily_streams.get(r).is_some_and(|t| snap.is_new(t)),
            Item::Track(_) => true,
        })
        .collect();
    let mut daily_covers: Vec<CoverRef> = Vec::new();
    for p in &daily {
        let c = match &p.item {
            Item::Track(id) => snap.catalog.get(*id).and_then(|t| t.release).map(CoverRef::Release),
            Item::Stream(r) => daily_streams.get(r).and_then(|t| t.cover_url.clone()).map(CoverRef::Web),
        };
        if let Some(c) = c
            && !daily_covers.contains(&c)
        {
            daily_covers.push(c);
            if daily_covers.len() == COVER_CANDIDATES {
                break;
            }
        }
    }

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
            (m, title)
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
    }
}

/// The recommendation parts of the home view, ready to be drawn.
pub(crate) struct HomeParts {
    hero: Option<(String, String, Vec<CoverRef>)>,
    mix: Vec<nikgob_library::TrackRow>,
    moods: Vec<(v::Mood, Vec<CoverRef>)>,
    styles: Vec<(String, Vec<CoverRef>)>,
    discover: Vec<v::StreamTrackRow>,
    new_releases: Vec<v::StreamAlbumCard>,
}

impl HomeParts {
    /// Fills the recommendation fields of `home`.
    pub(crate) fn apply(self, home: &mut v::HomeView, res: &mut crate::Resolver) {
        let mut covers = |c: Vec<CoverRef>| -> Vec<v::Cover> {
            c.into_iter()
                .filter_map(|c| match c {
                    CoverRef::Release(r) => res.release_cover(r),
                    CoverRef::Web(url) => Some(v::Cover::web(url)),
                })
                .take(COVERS)
                .collect()
        };
        home.hero = self.hero.map(|(title, subtitle, c)| v::MixCard {
            id: v::MixId::Daily,
            title,
            subtitle,
            covers: covers(c),
        });
        home.moods = self
            .moods
            .into_iter()
            .map(|(m, c)| v::MixCard {
                id: v::MixId::Mood(m),
                title: m.title().to_owned(),
                subtitle: String::new(),
                covers: covers(c),
            })
            .collect();
        home.style_mixes = self
            .styles
            .into_iter()
            .enumerate()
            .map(|(i, (title, c))| v::MixCard {
                id: v::MixId::Style(i as u8),
                title,
                subtitle: String::new(),
                covers: covers(c),
            })
            .collect();
        if !self.mix.is_empty() {
            home.mix = self.mix.iter().map(|t| res.track(t)).collect();
        }
        home.discover = self.discover;
        home.new_releases = self.new_releases;
    }
}

impl Worker {
    /// The recommendation parts of the home page as they stand.
    pub(crate) fn recs_home_parts(&mut self) -> HomeParts {
        let (discover, new_releases) = self.discover_rows();
        let Some(h) = &self.recs.home else {
            return HomeParts {
                hero: None,
                mix: Vec::new(),
                moods: Vec::new(),
                styles: Vec::new(),
                discover,
                new_releases,
            };
        };
        let n = h.daily.len();
        let subtitle = if h.history {
            format!("{n} {} · обновлён сегодня", crate::streams::plural(n, "трек", "трека", "треков"))
        } else {
            "Слушай — микс соберётся из того, что нравится".to_owned()
        };
        let hero = (n > 0).then(|| ("Твой микс".to_owned(), subtitle, h.daily_covers.clone()));
        let mix = h
            .daily
            .iter()
            .filter_map(|p| match p.item {
                Item::Track(id) => self.lib.get_track_row(id).ok().flatten(),
                Item::Stream(_) => None,
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
            .map(|(m, title)| (title.clone(), m.covers.iter().copied().map(CoverRef::Release).collect()))
            .collect();
        HomeParts {
            hero,
            mix,
            moods,
            styles,
            discover,
            new_releases,
        }
    }

    /// Draws the recommendation parts of the home page again.
    pub(crate) fn publish_recs_home(&mut self) {
        let parts = self.recs_home_parts();
        self.publish(|s, res| parts.apply(&mut s.home, res));
    }

    /// A build is due after `after` (sooner if one is due already).
    pub(crate) fn recs_dirty(&mut self, after: Duration) {
        let at = Instant::now() + after;
        let r = &mut self.recs;
        r.due = Some(r.due.map_or(at, |d| d.min(at)));
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
        let (day, _) = local_day();
        let r = &self.recs;
        let grown = {
            let (len, built) = (self.analysis.index().read().len(), r.built_index_len);
            len > built && (len - built >= NEW_VECTORS.max(built / 10) || len >= 2 * built)
        };
        let stale = r.built_at.is_some_and(|t| t.elapsed() >= every)
            || r.home.as_ref().is_some_and(|h| h.day != day)
            || (r.built_at.is_some() && grown);
        if !r.building && (r.due.is_some_and(|d| now >= d) || stale) {
            self.recs_build();
        }
        self.discover_tick();
    }

    fn recs_build(&mut self) {
        let (day, utc_offset_min) = local_day();
        let prefs = self.state.read().prefs.clone();
        let input = BuildInput {
            lib: self.lib.clone(),
            index: self.analysis.index().clone(),
            day,
            utc_offset_min,
            now: now_ms(),
            refresh_daily: std::mem::take(&mut self.recs.refresh_daily),
            new_share: prefs.radio_new_share.fraction(),
            new: self.discoveries(),
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
        }
    }

    fn recs_built(&mut self, b: Result<Built, String>) {
        self.recs.building = false;
        self.recs.built_at = Some(Instant::now());
        match b {
            Ok(b) => {
                self.recs.built_index_len = b.index_len;
                self.recs.snap = Some(b.snap);
                self.recs.home = Some(b.home);
            }
            Err(e) => {
                log::warn!("recs: {e}");
                return;
            }
        }
        self.publish_recs_home();
        if let Some(id) = self.recs.pending_play.take() {
            self.play_mix(id);
        }
        self.radio_snapshot_ready();
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
                    .map(|(m, title)| (m.seed(), title.clone()));
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
                if self.discoveries().is_empty() {
                    self.toast("Новое ещё ищется — загляни чуть позже", None);
                    return;
                }
                let seed = nikgob_recs::RadioSeed::Discover;
                self.start_radio(seed, "Новое для тебя".to_owned(), EventContext::HomeSection, None);
            }
            v::MixId::Daylist
            | v::MixId::OnRepeat
            | v::MixId::TimeCapsule
            | v::MixId::Rediscover
            | v::MixId::DeepCuts
            | v::MixId::FreshLikes => self.toast("Эта подборка ещё собирается", None),
        }
    }

    /// «Твой микс» as it is today: its tracks, not endless.
    fn play_daily(&mut self) {
        let Some(h) = &self.recs.home else { return };
        let picks = h.daily.clone();
        let streams = h.daily_streams.clone();
        let mut ids = Vec::with_capacity(picks.len());
        for p in &picks {
            match &p.item {
                Item::Track(id) => ids.push(*id),
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
        self.queue.set(&ids, 0, Some("микса «Твой микс»".to_owned()));
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
            // Moods, styles and artists are endless: nothing to pick again.
            v::MixId::Mood(_) | v::MixId::Style(_) | v::MixId::Artist(_) => {}
            v::MixId::Daylist
            | v::MixId::OnRepeat
            | v::MixId::TimeCapsule
            | v::MixId::Rediscover
            | v::MixId::DeepCuts
            | v::MixId::FreshLikes => {}
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

    #[test]
    fn moods_map_both_ways() {
        for m in Mood::ALL {
            assert_eq!(from_view_mood(to_view_mood(m)), m);
            assert_eq!(to_view_mood(m).title(), m.title());
        }
    }
}
