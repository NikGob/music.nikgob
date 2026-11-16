//! The taste model: what the listening history, the
//! likes and the playlists say about every track and artist of the library.
//!
//! * **Track affinity**: the sum of event weights, each decayed with a
//!   45-day half-life from its moment: a complete +1.0, a start that neither
//!   completed nor was skipped +0.3, a skip in the first 30 s −0.7 (half of
//!   it from the third early skip running in a playlist or the library: that
//!   is leafing through), a later skip +0.15 times the share
//!   heard (the song was listened to), a repeat (started again within a day
//!   of completing) +0.5, a playlist entry +1.5 (decayed from when it was
//!   added); a like is +3, a state without decay, and a song saved in a
//!   service's library (an import's «music library songs», not liked) +1.5
//!   the same way. Imported events (Takeout,
//!   Spotify; [`EventOrigin::Import`]) count the same here, but never in
//!   the hot affinity, the completes of «На повторе» or the co-listening:
//!   those are this app's own sessions.
//! * **Hot affinity** ([`Taste::hot_at`]): the same event weights with a
//!   2-day half-life, what is on repeat now («На повторе», «Слушать
//!   снова»; [`hot_affinity`] for a history without a catalog).
//! * **Artist affinity**: the affinities of the artist's tracks summed and
//!   divided by the square root of the artist's track count in the catalog,
//!   so a big discography does not win by size alone. A song saved in a
//!   service's library that the catalog lacks ([`TasteInput::saved_elsewhere`])
//!   counts as one more track of each of its artists with [`W_SAVED`]; an
//!   artist the catalog has nothing of gets the affinity of those songs
//!   alone ([`Taste::elsewhere_artists`]), for the new music and the new
//!   releases only.
//! * **Co-listening**: plays of one session (gaps under 30 minutes) link
//!   each play's main artist with the main artists of the
//!   [`CO_WINDOW`] plays before it, a pair once per session: the fallback
//!   similarity for tracks without a sound vector.
//! * **Day parts**: per track, how many plays fell into the morning (6–11),
//!   the day (11–18), the evening (18–23) and the night (23–6), local time,
//!   and the positive weights of its outcomes there halving in two months
//!   ([`Taste::part_lift_at`]).
//! * **Who and where** ([`crate::context`]): every weight above is times
//!   the weight of the play's context — who started it, on what kind of
//!   device, for which device the taste is built; a guest's plays are not
//!   read at all, and the windows of the tiles count a play weighing at
//!   least [`crate::context::FULL_PLAY`]. The co-listening keeps the
//!   sessions of each kind of device apart.
//! * **Service suggestions** ([`crate::suggest`]): a
//!   track YouTube Music or Spotify suggests gets up to
//!   [`crate::suggest::SUGGESTED_CAP`] more affinity, fading out over
//!   [`crate::suggest::SUGGESTION_FADE_MS`] since it was suggested; a
//!   suggested artist up to [`crate::suggest::SUGGESTED_ARTIST_CAP`] more
//!   artist affinity. Suggestions alone are not a history
//!   ([`Taste::has_history`]).
//! * **Dislikes** («Не рекомендовать»): tracks, and every track of an
//!   artist, kept out of everything.
//! * **Windows** for the home's tiles: per track, the completes of the last
//!   [`ON_REPEAT_MS`], the plays within [`YEAR_AGO_SPAN_MS`] of this day a
//!   year ago and when it was liked.
//!
//! One pass over the events sorted by time, with per-track state in dense
//! vectors indexed by catalog position: 100 000 events over 50 000 tracks
//! take a few milliseconds.

use std::collections::{HashMap, HashSet, VecDeque};

use nikgob_core::model::{ArtistId, EndReason, Event, EventContext, EventOrigin, EventType, ReleaseId, TrackId, UnixMs};
use serde::{Deserialize, Serialize};

use crate::context::{FULL_PLAY, PlayActor, PlayDevice, SLOT_HALF_LIFE_MS, event_weights};
use crate::suggest;

pub const HOUR_MS: i64 = 60 * 60 * 1000;
pub const DAY_MS: i64 = 24 * HOUR_MS;

/// Affinity halves every this long.
pub const HALF_LIFE_MS: i64 = 45 * DAY_MS;
/// The hot affinity halves every this long.
pub const HOT_HALF_LIFE_MS: i64 = 2 * DAY_MS;
/// A mix lets a played track rest about this long ([`Taste::lately_at`]
/// halves in it).
pub const REST_MS: i64 = 36 * HOUR_MS;
pub const W_COMPLETE: f32 = 1.0;
pub const W_START_ONLY: f32 = 0.3;
pub const W_SKIP_EARLY: f32 = -0.7;
/// A later skip: this times the share of the track heard.
pub const W_SKIP_LATE_HEARD: f32 = 0.15;
/// From this early skip running in a playlist or the library…
pub const SKIP_STREAK_FROM: u32 = 3;
/// …an early skip weighs this much of its weight.
pub const SKIP_STREAK_FACTOR: f32 = 0.5;
pub const W_REPEAT: f32 = 0.5;
pub const W_PLAYLIST: f32 = 1.5;
pub const W_LIKE: f32 = 3.0;
/// A song saved in a service's library: a state like a like, lighter.
pub const W_SAVED: f32 = 1.5;
/// A skip before this position is an early one.
pub const EARLY_SKIP_MS: u32 = 30_000;
/// A start this soon after a complete of the same track is a repeat.
pub const REPEAT_WITHIN_MS: i64 = DAY_MS;
/// A pause this long starts a new session.
pub const SESSION_GAP_MS: i64 = 30 * 60 * 1000;
/// A play links its artist with the artists of this many plays before it.
pub const CO_WINDOW: usize = 8;
/// Plays a track needs before its own day-part profile counts (each as
/// much as it weighs).
const DAYPART_MIN_PLAYS: f32 = 3.0;
/// «На повторе» and «Свежие лайки» look this far back.
pub const ON_REPEAT_MS: i64 = 30 * DAY_MS;
/// A track played to the end more than [`SATURATED_FROM`] times within
/// this is worn: the streams let it cool a little ([`Taste::saturation_at`])…
pub const SATURATION_MS: i64 = 4 * DAY_MS;
pub const SATURATED_FROM: u8 = 3;
/// …all they do from this many completes more.
const SATURATED_SPAN: f32 = 4.0;
/// «Год назад»: this long before now…
pub const YEAR_MS: i64 = 365 * DAY_MS;
/// …give or take this much.
pub const YEAR_AGO_SPAN_MS: i64 = 14 * DAY_MS;

/// One track of the catalog: what the engine may pick (library tracks and
/// liked streams), with its artists (main first) and release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackInfo {
    pub id: TrackId,
    pub artists: Vec<ArtistId>,
    pub release: Option<ReleaseId>,
    /// Has a file (a liked or playlist stream without one plays from its
    /// service): «Миксы только из скачанного» picks only these.
    pub local: bool,
    /// The track standing for its song when the song has several tracks
    /// (one per album it is on): a mix takes one of them.
    pub song: Option<TrackId>,
    /// The world it belongs to ([`world_token`] of the key of Главная's
    /// world: a soundtrack franchise, else its main artist): the diversity
    /// rules keep one world from filling a mix.
    pub world: Option<u64>,
    /// How long it plays (a late skip counts by how much was heard; mixes
    /// leave out the longest).
    pub duration_ms: Option<u32>,
    /// Its place on its release: disc × 1000 + number (the next on the
    /// album).
    pub position: u32,
    /// When it came to the library (0: not known).
    pub added_at: UnixMs,
}

impl TrackInfo {
    /// A track with a file.
    pub fn new(id: TrackId, artists: Vec<ArtistId>, release: Option<ReleaseId>) -> Self {
        Self {
            id,
            artists,
            release,
            local: true,
            song: None,
            world: None,
            duration_ms: None,
            position: 0,
            added_at: 0,
        }
    }

    /// The track standing for the song this one is (see [`Self::song`]).
    #[must_use]
    pub fn with_song(mut self, song: TrackId) -> Self {
        self.song = (song != self.id).then_some(song);
        self
    }

    /// The same track, with a file or without one.
    #[must_use]
    pub fn with_local(mut self, local: bool) -> Self {
        self.local = local;
        self
    }

    /// The same track in the world of `key` ([`world_token`]).
    #[must_use]
    pub fn with_world(mut self, key: &str) -> Self {
        self.world = Some(world_token(key));
        self
    }

    /// The same track, `ms` long.
    #[must_use]
    pub fn with_duration(mut self, ms: Option<u32>) -> Self {
        self.duration_ms = ms;
        self
    }

    /// The same track at `position` of its release.
    #[must_use]
    pub fn with_position(mut self, position: u32) -> Self {
        self.position = position;
        self
    }

    /// The same track, come to the library at `at`.
    #[must_use]
    pub fn with_added(mut self, at: UnixMs) -> Self {
        self.added_at = at;
        self
    }

    pub fn main_artist(&self) -> Option<ArtistId> {
        self.artists.first().copied()
    }
}

/// The token of a world's key (`made in abyss`, `artist:<id>`): stable
/// across builds.
pub fn world_token(key: &str) -> u64 {
    crate::pick::token(key.as_bytes())
}

/// The tracks the engine knows, with lookups by id and by artist.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    tracks: Vec<TrackInfo>,
    pos: HashMap<TrackId, u32>,
    by_artist: HashMap<ArtistId, Vec<u32>>,
    /// Tracks that are another copy of a catalog track (one recording on
    /// two albums): not picked themselves, looked up as that track.
    alias: HashMap<TrackId, u32>,
}

impl Catalog {
    /// A catalog of `tracks`; a repeated id keeps its first entry.
    pub fn new(tracks: impl IntoIterator<Item = TrackInfo>) -> Self {
        let mut c = Self::default();
        for t in tracks {
            if c.pos.contains_key(&t.id) {
                continue;
            }
            let i = c.tracks.len() as u32;
            c.pos.insert(t.id, i);
            let mut seen = Vec::with_capacity(t.artists.len());
            for a in &t.artists {
                if !seen.contains(a) {
                    seen.push(*a);
                    c.by_artist.entry(*a).or_default().push(i);
                }
            }
            c.tracks.push(t);
        }
        c
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    pub fn tracks(&self) -> &[TrackInfo] {
        &self.tracks
    }

    /// The same catalog with each `(copy, track)`: `copy` is another copy
    /// of the catalog's `track` (one recording on two albums). It is never
    /// picked, and its plays, likes and lookups are `track`'s, so a list
    /// never has both and «Не повторять сыгранное» covers both. A copy that
    /// is in the catalog itself, or whose track is not, is left alone.
    #[must_use]
    pub fn with_aliases(mut self, aliases: impl IntoIterator<Item = (TrackId, TrackId)>) -> Self {
        for (copy, track) in aliases {
            if self.pos.contains_key(&copy) {
                continue;
            }
            if let Some(&i) = self.pos.get(&track) {
                self.alias.insert(copy, i);
            }
        }
        self
    }

    /// The copies looked up as catalog tracks ([`Self::with_aliases`]),
    /// with the position of each one's track.
    pub fn aliases(&self) -> impl Iterator<Item = (TrackId, usize)> + '_ {
        self.alias.iter().map(|(id, &i)| (*id, i as usize))
    }

    /// The catalog position of a track (of the track it is a copy of).
    pub fn position(&self, id: TrackId) -> Option<usize> {
        self.pos.get(&id).or_else(|| self.alias.get(&id)).map(|&i| i as usize)
    }

    pub fn get(&self, id: TrackId) -> Option<&TrackInfo> {
        self.position(id).map(|i| &self.tracks[i])
    }

    /// The work the catalog track at `i` is of: the track standing for its
    /// song ([`TrackInfo::song`]: its copy on another album, the plain song
    /// of an off vocal or a TV size), else itself. Two tracks of one work
    /// never both belong in one session or mix. O(1).
    pub fn work_at(&self, i: usize) -> TrackId {
        let t = &self.tracks[i];
        t.song.unwrap_or(t.id)
    }

    /// [`Self::work_at`] by id (a copy's is its track's); a track the
    /// catalog lacks is its own.
    pub fn work_of(&self, id: TrackId) -> TrackId {
        self.position(id).map_or(id, |i| self.work_at(i))
    }

    /// Catalog positions of an artist's tracks (any credit, not only main).
    pub fn of_artist(&self, artist: ArtistId) -> &[u32] {
        self.by_artist.get(&artist).map_or(&[], Vec::as_slice)
    }

    /// Every artist with at least one track.
    pub fn artists(&self) -> impl Iterator<Item = ArtistId> + '_ {
        self.by_artist.keys().copied()
    }
}

/// The four parts of a day (local time).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DayPart {
    /// 6–11.
    Morning,
    /// 11–18.
    Day,
    /// 18–23.
    Evening,
    /// 23–6.
    Night,
}

impl DayPart {
    pub const ALL: [Self; 4] = [Self::Morning, Self::Day, Self::Evening, Self::Night];

    pub fn of_hour(hour: u32) -> Self {
        match hour {
            6..=10 => Self::Morning,
            11..=17 => Self::Day,
            18..=22 => Self::Evening,
            _ => Self::Night,
        }
    }

    /// The part of the day of a moment, `utc_offset_min` east of UTC.
    pub fn at(ts: UnixMs, utc_offset_min: i32) -> Self {
        let local = ts + i64::from(utc_offset_min) * 60_000;
        Self::of_hour((local.rem_euclid(DAY_MS) / HOUR_MS) as u32)
    }

    pub const fn index(self) -> usize {
        self as usize
    }

    /// The hour the part ends at (local time).
    pub const fn end_hour(self) -> u32 {
        match self {
            Self::Morning => 11,
            Self::Day => 18,
            Self::Evening => 23,
            Self::Night => 6,
        }
    }

    /// The local day (days since 1970-01-01) the part of the day at `ts`
    /// belongs to: a night belongs to the day it started, so 02:00 on a
    /// Friday is Thursday's night.
    pub fn day_of(ts: UnixMs, utc_offset_min: i32) -> i64 {
        let local = ts + i64::from(utc_offset_min) * 60_000;
        let day = local.div_euclid(DAY_MS);
        let hour = (local.rem_euclid(DAY_MS) / HOUR_MS) as u32;
        if hour < Self::Night.end_hour() { day - 1 } else { day }
    }
}

/// One event of the listening history, as much of it as the taste needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Play {
    pub track: TrackId,
    pub ts: UnixMs,
    pub kind: EventType,
    pub position_ms: Option<u32>,
    /// Where it played from (a run of early skips in a playlist is leafing).
    pub context: EventContext,
    /// Who chose it (a start).
    pub origin: Option<EventOrigin>,
    /// Why it ended (an end).
    pub end_reason: Option<EndReason>,
    /// How far the window was while it played (an end): 0 focused, 0.5 in
    /// view behind another window, 1 hidden.
    pub attention: Option<f32>,
    /// Who started it ([`crate::context`]).
    pub actor: PlayActor,
    /// The kind of device it sounded on (the runtime tells it from the
    /// event's device; personal when not known).
    pub device: PlayDevice,
}

impl Play {
    /// An event played from nowhere in particular, the owner's, at a
    /// personal device.
    pub fn new(track: TrackId, ts: UnixMs, kind: EventType, position_ms: Option<u32>) -> Self {
        Self {
            track,
            ts,
            kind,
            position_ms,
            context: EventContext::Other,
            origin: None,
            end_reason: None,
            attention: None,
            actor: PlayActor::Owner,
            device: PlayDevice::Personal,
        }
    }
}

impl From<&Event> for Play {
    /// The event as it is, its device taken for a personal one (the
    /// runtime knows the devices: [`Play::device`]).
    fn from(e: &Event) -> Self {
        Self {
            track: e.track_id,
            ts: e.ts,
            kind: e.event_type,
            position_ms: e.position_ms,
            context: e.context,
            origin: e.origin,
            end_reason: e.end_reason,
            attention: e.attention,
            actor: PlayActor::of(e.actor),
            device: PlayDevice::Personal,
        }
    }
}

/// The weight of each event of a history in order of time (see the module
/// docs): what a start, a complete and a skip add to a track, and from
/// when it decays; each times the weight of the play's context
/// ([`crate::context::event_weights`]).
struct Weigher {
    /// A start not ended yet, by track, and its context's weight.
    open: Vec<Option<(UnixMs, f32)>>,
    last_complete: Vec<Option<UnixMs>>,
    /// Early skips running, and when the last event was (a session).
    streak: u32,
    last_ts: Option<UnixMs>,
}

impl Weigher {
    fn new(n: usize) -> Self {
        Self {
            open: vec![None; n],
            last_complete: vec![None; n],
            streak: 0,
            last_ts: None,
        }
    }

    /// Weighs one event of track `i` (`duration` its length, `w` the
    /// weight of its context), handing each weight and its moment to
    /// `add`; true when the event is a play.
    #[allow(clippy::too_many_arguments)]
    fn event(
        &mut self,
        i: usize,
        ts: UnixMs,
        kind: EventType,
        pos: Option<u32>,
        context: EventContext,
        duration: Option<u32>,
        w: f32,
        add: &mut impl FnMut(usize, UnixMs, f32),
    ) -> bool {
        if self.last_ts.is_none_or(|l| ts - l >= SESSION_GAP_MS) {
            self.streak = 0;
        }
        self.last_ts = Some(ts);
        match kind {
            EventType::Start => {
                if let Some((t0, w0)) = self.open[i] {
                    add(i, t0, W_START_ONLY * w0);
                }
                if self.last_complete[i].is_some_and(|c| ts - c <= REPEAT_WITHIN_MS) {
                    add(i, ts, W_REPEAT * w);
                }
                self.open[i] = Some((ts, w));
                true
            }
            EventType::Complete => {
                self.streak = 0;
                // A complete without its start (some imports) is a play too.
                let counted = self.open[i].take().is_none();
                add(i, ts, W_COMPLETE * w);
                self.last_complete[i] = Some(ts);
                counted
            }
            EventType::Skip => {
                self.open[i] = None;
                match pos {
                    Some(p) if p < EARLY_SKIP_MS => {
                        self.streak += 1;
                        let leafing = self.streak >= SKIP_STREAK_FROM && matches!(context, EventContext::Playlist | EventContext::Library);
                        add(i, ts, w * if leafing { W_SKIP_EARLY * SKIP_STREAK_FACTOR } else { W_SKIP_EARLY });
                    }
                    // A skip without a position counts as a late one; how
                    // much was heard, when both are known.
                    _ => {
                        self.streak = 0;
                        let heard = match (pos, duration) {
                            (Some(p), Some(d)) if d > 0 => (p as f32 / d as f32).clamp(0.0, 1.0),
                            _ => 0.0,
                        };
                        add(i, ts, W_SKIP_LATE_HEARD * heard * w);
                    }
                }
                false
            }
            _ => false,
        }
    }

    /// The starts never ended.
    fn finish(self, add: &mut impl FnMut(usize, UnixMs, f32)) {
        for (i, open) in self.open.iter().enumerate() {
            if let Some((t0, w0)) = open {
                add(i, *t0, W_START_ONLY * *w0);
            }
        }
    }
}

/// The hot affinity ([`Taste::hot_at`]) of every track in `events` at
/// `now`, `duration` telling a track's length: for a history read without
/// a catalog (Главная's «Слушать снова»), weighed for a personal device
/// ([`crate::context`]). Imported plays are not this app's listening: they
/// are left out.
pub fn hot_affinity(events: &[Play], duration: impl Fn(TrackId) -> Option<u32>, now: UnixMs) -> HashMap<TrackId, f32> {
    let weights = event_weights(events, PlayDevice::Personal);
    let mut sorted: Vec<usize> = (0..events.len())
        .filter(|&k| {
            let p = &events[k];
            matches!(p.kind, EventType::Start | EventType::Complete | EventType::Skip) && p.origin != Some(EventOrigin::Import) && weights[k] > 0.0
        })
        .collect();
    sorted.sort_by_key(|&k| events[k].ts);
    let mut ids: Vec<TrackId> = Vec::new();
    let mut pos: HashMap<TrackId, usize> = HashMap::new();
    for &k in &sorted {
        let p = &events[k];
        pos.entry(p.track).or_insert_with(|| {
            ids.push(p.track);
            ids.len() - 1
        });
    }
    let mut hot = vec![0.0f32; ids.len()];
    let mut add = |i: usize, ts: UnixMs, w: f32| hot[i] += w * decay(now, ts, HOT_HALF_LIFE_MS);
    let mut weigher = Weigher::new(ids.len());
    for k in sorted {
        let p = &events[k];
        let i = pos[&p.track];
        weigher.event(i, p.ts, p.kind, p.position_ms, p.context, duration(p.track), weights[k], &mut add);
    }
    weigher.finish(&mut add);
    ids.into_iter().zip(hot).collect()
}

/// What the runtime reads from the library for [`Taste::build`].
#[derive(Clone, Debug, Default)]
pub struct TasteInput {
    /// The whole event log, any order. `like`, `unlike`, `add_to_playlist`
    /// and `seek` events are ignored (the likes and the playlist entries
    /// below are the state they led to).
    pub events: Vec<Play>,
    /// Tracks liked now.
    pub likes: Vec<TrackId>,
    /// Songs saved in a service's library (an import's «music library
    /// songs»): long taste and the profiles' mass, never a like.
    pub saved: Vec<TrackId>,
    /// Those of them the catalog lacks (not in the library), with their
    /// artists: the long taste of the artists and seeds of the new music,
    /// never a track to pick.
    pub saved_elsewhere: Vec<(TrackId, Vec<ArtistId>)>,
    /// When the likes were set (a like missing here is an old one).
    pub like_times: Vec<(TrackId, UnixMs)>,
    /// Playlist entries: a track and when it was added (one per entry).
    pub playlist_entries: Vec<(TrackId, UnixMs)>,
    /// «Не рекомендовать» tracks and artists.
    pub disliked_tracks: Vec<TrackId>,
    pub disliked_artists: Vec<ArtistId>,
    /// The local time zone, minutes east of UTC (day parts).
    pub utc_offset_min: i32,
    /// Catalog tracks the services suggest: the strength of the suggestion
    /// ([`crate::suggest::strength`]) and when it was made (one entry per
    /// suggestion; see [`crate::suggest::match_suggestions`]).
    pub suggested_tracks: Vec<(TrackId, f32, UnixMs)>,
    /// Library artists the services suggest, the same way.
    pub suggested_artists: Vec<(ArtistId, f32, UnixMs)>,
    /// A disliked track's nearest sound neighbours stand a little lower
    /// ([`crate::dislike`]; «Учитывать дизлайки в рекомендациях»). Off, a
    /// dislike keeps only its own track out.
    pub dislike_neighbours: bool,
    /// The personal graph of similar music ([`crate::graph::personal`]: an
    /// imported history's sessions, the order of the playlists) is a
    /// feature of the next start («Мои плейлисты и импорт истории»).
    pub personal_graph: bool,
    /// While the history is short the library itself is the taste
    /// profiles' mass, its tracks by their prior in the graph («Без
    /// истории — по самой библиотеке»).
    pub library_prior: bool,
    /// The tracks of each playlist (hints of the taste profiles).
    pub playlists: Vec<Vec<TrackId>>,
    /// How the taste profiles are built ([`crate::profiles`]).
    pub profiles: crate::profiles::Settings,
    /// The kind of device the recommendations are for: the plays weigh
    /// by who played them where ([`crate::context::event_weights`]).
    pub target_device: PlayDevice,
}

/// Per-track and per-artist signals at one moment (`now`).
#[derive(Clone, Debug)]
pub struct Taste {
    now: UnixMs,
    utc_offset_min: i32,
    /// By catalog position.
    affinity: Vec<f32>,
    /// The same weights with [`HOT_HALF_LIFE_MS`].
    hot: Vec<f32>,
    plays: Vec<u32>,
    last_played: Vec<Option<UnixMs>>,
    liked: Vec<bool>,
    liked_time: Vec<Option<UnixMs>>,
    /// Plays by the part of the day, each by its context's weight.
    daypart: Vec<[f32; 4]>,
    /// The outcomes' positive weights by the part of the day, halving in
    /// [`SLOT_HALF_LIFE_MS`] ([`Self::part_lift_at`]), and their sums.
    part_mass: Vec<[f32; 4]>,
    part_total: [f32; 4],
    /// The part of the affinity the services' suggestions gave.
    suggested: Vec<f32>,
    /// Anything listened, liked or put in a playlist (suggestions aside).
    history: bool,
    /// Completes within [`ON_REPEAT_MS`].
    recent_completes: Vec<u16>,
    /// Completes within [`SATURATION_MS`].
    saturated: Vec<u8>,
    /// Plays within [`YEAR_AGO_SPAN_MS`] of now − [`YEAR_MS`].
    year_ago: Vec<u16>,
    artist_affinity: HashMap<ArtistId, f32>,
    /// Artists the catalog has nothing of, by the songs saved elsewhere.
    elsewhere_artists: HashMap<ArtistId, f32>,
    /// [`TasteInput::saved_elsewhere`], each song once, none disliked.
    saved_elsewhere: Vec<(TrackId, Vec<ArtistId>)>,
    co_pairs: HashMap<(ArtistId, ArtistId), f32>,
    co: HashMap<ArtistId, Vec<(ArtistId, f32)>>,
    profile: [f32; 4],
    disliked_tracks: HashSet<TrackId>,
    disliked_artists: HashSet<ArtistId>,
    /// How much lower each track stands for the dislikes near it in sound
    /// (by catalog position; empty: nothing), set by the user model.
    near_dislike: Vec<f32>,
}

/// What is left of a weight from `ts` at `now` with a half-life of `half`.
fn decay(now: UnixMs, ts: UnixMs, half: i64) -> f32 {
    let age = (now - ts).max(0) as f64;
    (-age / half as f64).exp2() as f32
}

fn ordered(a: ArtistId, b: ArtistId) -> (ArtistId, ArtistId) {
    if a <= b { (a, b) } else { (b, a) }
}

impl Taste {
    /// The model of `input` over `catalog` at `now`. Events of tracks the
    /// catalog lacks are ignored; each weighs by its context
    /// ([`crate::context::event_weights`] for [`TasteInput::target_device`]).
    pub fn build(catalog: &Catalog, input: &TasteInput, now: UnixMs) -> Self {
        Self::build_weighted(catalog, input, &event_weights(&input.events, input.target_device), now)
    }

    /// [`Self::build`] with the weight of each event of `input` given
    /// (beside them, in their order).
    pub fn build_weighted(catalog: &Catalog, input: &TasteInput, weights: &[f32], now: UnixMs) -> Self {
        let n = catalog.len();
        let mut affinity = vec![0.0f32; n];
        let mut plays = vec![0u32; n];
        let mut last_played: Vec<Option<UnixMs>> = vec![None; n];
        let mut liked = vec![false; n];
        let mut daypart = vec![[0.0f32; 4]; n];
        let mut part_mass = vec![[0.0f32; 4]; n];
        let mut recent_completes = vec![0u16; n];
        let mut saturated = vec![0u8; n];
        let mut year_ago = vec![0u16; n];
        let mut hot = vec![0.0f32; n];
        let off = input.utc_offset_min;

        type Ev = (UnixMs, usize, EventType, Option<u32>, EventContext, f32, PlayDevice);
        let mut events: Vec<Ev> = Vec::new();
        let mut imported: Vec<Ev> = Vec::new();
        for (k, p) in input.events.iter().enumerate() {
            if !matches!(p.kind, EventType::Start | EventType::Complete | EventType::Skip) {
                continue;
            }
            let w = weights.get(k).copied().unwrap_or(1.0);
            // A guest's: nothing at all.
            if w <= 0.0 {
                continue;
            }
            let Some(i) = catalog.position(p.track) else { continue };
            let e = (p.ts, i, p.kind, p.position_ms, p.context, w, p.device);
            if p.origin == Some(EventOrigin::Import) { imported.push(e) } else { events.push(e) }
        }
        events.sort_by_key(|e| e.0);
        imported.sort_by_key(|e| e.0);
        // A play (or a complete) of either kind: when it was last played, a
        // play counted, by part of the day (as much as it weighs) and a
        // year ago (a whole one).
        let mut count = |i: usize, ts: UnixMs, play: bool, w: f32| {
            last_played[i] = last_played[i].max(Some(ts));
            if !play {
                return;
            }
            plays[i] += 1;
            let part = DayPart::at(ts, off).index();
            daypart[i][part] += w;
            if w >= FULL_PLAY && (ts - (now - YEAR_MS)).abs() <= YEAR_AGO_SPAN_MS {
                year_ago[i] = year_ago[i].saturating_add(1);
            }
        };

        // Sessions and co-listening, each kind of device its own sessions.
        let mut co_pairs: HashMap<(ArtistId, ArtistId), f32> = HashMap::new();
        let mut tail: [VecDeque<ArtistId>; 2] = [VecDeque::with_capacity(CO_WINDOW + 1), VecDeque::with_capacity(CO_WINDOW + 1)];
        let mut session_pairs: [HashSet<(ArtistId, ArtistId)>; 2] = [HashSet::new(), HashSet::new()];
        let mut last_play_ts: [Option<UnixMs>; 2] = [None, None];
        let mut weigher = Weigher::new(n);
        let mut add = |i: usize, ts: UnixMs, w: f32| {
            affinity[i] += w * decay(now, ts, HALF_LIFE_MS);
            hot[i] += w * decay(now, ts, HOT_HALF_LIFE_MS);
            if w > 0.0 {
                part_mass[i][DayPart::at(ts, off).index()] += w * decay(now, ts, SLOT_HALF_LIFE_MS);
            }
        };

        for &(ts, i, kind, pos, context, w, device) in &events {
            let counted_play = weigher.event(i, ts, kind, pos, context, catalog.tracks[i].duration_ms, w, &mut add);
            if kind == EventType::Complete {
                if w >= FULL_PLAY && now - ts < ON_REPEAT_MS {
                    recent_completes[i] = recent_completes[i].saturating_add(1);
                }
                if w >= FULL_PLAY && now - ts < SATURATION_MS {
                    saturated[i] = saturated[i].saturating_add(1);
                }
                count(i, ts, false, w);
            }
            if !counted_play {
                continue;
            }
            count(i, ts, true, w);

            let d = usize::from(device == PlayDevice::Ambient);
            if last_play_ts[d].is_none_or(|l| ts - l >= SESSION_GAP_MS) {
                tail[d].clear();
                session_pairs[d].clear();
            }
            last_play_ts[d] = Some(ts);
            let Some(a) = catalog.tracks[i].main_artist() else { continue };
            for &b in &tail[d] {
                if b != a {
                    let pair = ordered(a, b);
                    if session_pairs[d].insert(pair) {
                        *co_pairs.entry(pair).or_default() += w;
                    }
                }
            }
            tail[d].push_back(a);
            if tail[d].len() > CO_WINDOW {
                tail[d].pop_front();
            }
        }
        weigher.finish(&mut add);
        // Imported plays: the long affinity alone, weighed apart (another
        // service's sessions and skips are not this app's); their times a
        // little ([`crate::model::IMPORT_SHARE`]).
        let mut weigher = Weigher::new(n);
        let mut add_long = |i: usize, ts: UnixMs, w: f32| {
            affinity[i] += w * decay(now, ts, HALF_LIFE_MS);
            if w > 0.0 {
                part_mass[i][DayPart::at(ts, off).index()] += crate::model::IMPORT_SHARE * w * decay(now, ts, SLOT_HALF_LIFE_MS);
            }
        };
        for &(ts, i, kind, pos, context, w, _) in &imported {
            let play = weigher.event(i, ts, kind, pos, context, catalog.tracks[i].duration_ms, w, &mut add_long);
            if play || kind == EventType::Complete {
                count(i, ts, play, w);
            }
        }
        weigher.finish(&mut add_long);
        let mut part_total = [0.0f32; 4];
        for m in &part_mass {
            for (t, x) in part_total.iter_mut().zip(m) {
                *t += x;
            }
        }
        for id in &input.likes {
            if let Some(i) = catalog.position(*id)
                && !liked[i]
            {
                liked[i] = true;
                affinity[i] += W_LIKE;
            }
        }
        let mut saved = vec![false; n];
        for id in &input.saved {
            if let Some(i) = catalog.position(*id)
                && !liked[i]
                && !saved[i]
            {
                saved[i] = true;
                affinity[i] += W_SAVED;
            }
        }
        let mut liked_time: Vec<Option<UnixMs>> = vec![None; n];
        for (id, at) in &input.like_times {
            if let Some(i) = catalog.position(*id)
                && liked[i]
            {
                liked_time[i] = liked_time[i].max(Some(*at));
            }
        }
        for (id, added) in &input.playlist_entries {
            if let Some(i) = catalog.position(*id) {
                affinity[i] += W_PLAYLIST * decay(now, *added, HALF_LIFE_MS);
            }
        }
        let history = affinity.iter().any(|a| *a > 0.0);
        let mut suggested = vec![0.0f32; n];
        for (id, strength, at) in &input.suggested_tracks {
            if let Some(i) = catalog.position(*id) {
                suggested[i] += suggest::W_SUGGESTED * strength.max(0.0) * suggest::fade(now, *at);
            }
        }
        for (a, s) in affinity.iter_mut().zip(suggested.iter_mut()) {
            *s = s.min(suggest::SUGGESTED_CAP);
            *a += *s;
        }

        let mut artist_sum: HashMap<ArtistId, f32> = HashMap::new();
        for (i, t) in catalog.tracks.iter().enumerate() {
            if affinity[i] != 0.0 {
                for a in &t.artists {
                    *artist_sum.entry(*a).or_default() += affinity[i];
                }
            }
        }
        // The songs saved elsewhere: more tracks of their artists.
        let disliked_tracks: HashSet<TrackId> = input.disliked_tracks.iter().copied().collect();
        let disliked_artists: HashSet<ArtistId> = input.disliked_artists.iter().copied().collect();
        let mut saved_elsewhere: Vec<(TrackId, Vec<ArtistId>)> = Vec::new();
        let mut elsewhere_count: HashMap<ArtistId, usize> = HashMap::new();
        for (id, artists) in &input.saved_elsewhere {
            if catalog.position(*id).is_some()
                || disliked_tracks.contains(id)
                || artists.iter().any(|a| disliked_artists.contains(a))
                || saved_elsewhere.iter().any(|s| s.0 == *id)
            {
                continue;
            }
            for a in artists {
                *artist_sum.entry(*a).or_default() += W_SAVED;
                *elsewhere_count.entry(*a).or_default() += 1;
            }
            saved_elsewhere.push((*id, artists.clone()));
        }
        let mut elsewhere_artists: HashMap<ArtistId, f32> = HashMap::new();
        let mut artist_affinity: HashMap<ArtistId, f32> = HashMap::new();
        for (a, s) in artist_sum {
            let own = catalog.of_artist(a).len();
            let n = own + elsewhere_count.get(&a).copied().unwrap_or(0);
            let s = s / (n.max(1) as f32).sqrt();
            if own == 0 { elsewhere_artists.insert(a, s) } else { artist_affinity.insert(a, s) };
        }
        let mut artist_bonus: HashMap<ArtistId, f32> = HashMap::new();
        for (a, strength, at) in &input.suggested_artists {
            if !catalog.of_artist(*a).is_empty() {
                *artist_bonus.entry(*a).or_default() +=
                    suggest::W_SUGGESTED_ARTIST * strength.max(0.0) * suggest::fade(now, *at);
            }
        }
        for (a, b) in artist_bonus {
            if b > 0.0 {
                *artist_affinity.entry(a).or_default() += b.min(suggest::SUGGESTED_ARTIST_CAP);
            }
        }

        let mut co: HashMap<ArtistId, Vec<(ArtistId, f32)>> = HashMap::new();
        for (&(a, b), &c) in &co_pairs {
            co.entry(a).or_default().push((b, c));
            co.entry(b).or_default().push((a, c));
        }
        for list in co.values_mut() {
            list.sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.cmp(&y.0)));
        }

        let mut profile = [0.0f32; 4];
        for d in &daypart {
            for (p, c) in profile.iter_mut().zip(d) {
                *p += *c;
            }
        }
        let total: f32 = profile.iter().sum();
        if total > 0.0 {
            profile.iter_mut().for_each(|p| *p /= total);
        }

        Self {
            now,
            utc_offset_min: input.utc_offset_min,
            affinity,
            hot,
            plays,
            last_played,
            liked,
            liked_time,
            daypart,
            part_mass,
            part_total,
            suggested,
            history,
            recent_completes,
            saturated,
            year_ago,
            artist_affinity,
            elsewhere_artists,
            saved_elsewhere,
            co_pairs,
            co,
            profile,
            disliked_tracks,
            disliked_artists,
            near_dislike: Vec::new(),
        }
    }

    /// How much lower the track at `i` stands for the dislikes of its
    /// nearest sound neighbours ([`crate::dislike`]), 0 to
    /// [`crate::dislike::CAP`].
    pub fn dislike_near_at(&self, i: usize) -> f32 {
        self.near_dislike.get(i).copied().unwrap_or(0.0)
    }

    pub(crate) fn set_near_dislikes(&mut self, near: Vec<f32>) {
        self.near_dislike = near;
    }

    pub fn now(&self) -> UnixMs {
        self.now
    }

    pub fn utc_offset_min(&self) -> i32 {
        self.utc_offset_min
    }

    /// The part of the day at [`Self::now`].
    pub fn day_part(&self) -> DayPart {
        DayPart::at(self.now, self.utc_offset_min)
    }

    /// Affinity of the track at catalog position `i`.
    pub fn affinity_at(&self, i: usize) -> f32 {
        self.affinity.get(i).copied().unwrap_or(0.0)
    }

    /// The hot affinity of the track at `i`: the event weights halving in
    /// two days (likes and playlist entries aside), what plays on repeat now.
    pub fn hot_at(&self, i: usize) -> f32 {
        self.hot.get(i).copied().unwrap_or(0.0)
    }

    pub fn plays_at(&self, i: usize) -> u32 {
        self.plays.get(i).copied().unwrap_or(0)
    }

    pub fn last_played_at(&self, i: usize) -> Option<UnixMs> {
        self.last_played.get(i).copied().flatten()
    }

    /// How lately the track at `i` played: 1 now, halving every
    /// [`REST_MS`], 0 never (what a mix lets rest).
    pub fn lately_at(&self, i: usize) -> f32 {
        self.last_played_at(i).map_or(0.0, |t| decay(self.now, t, REST_MS))
    }

    pub fn liked_at(&self, i: usize) -> bool {
        self.liked.get(i).copied().unwrap_or(false)
    }

    /// When the track at `i` was liked (`None`: not liked, or not known).
    pub fn liked_time_at(&self, i: usize) -> Option<UnixMs> {
        self.liked_time.get(i).copied().flatten()
    }

    /// Completes of the track at `i` within [`ON_REPEAT_MS`].
    pub fn recent_completes_at(&self, i: usize) -> u32 {
        self.recent_completes.get(i).map_or(0, |c| u32::from(*c))
    }

    /// How worn the track at `i` is (0…1): its completes within
    /// [`SATURATION_MS`] beyond [`SATURATED_FROM`], all of it four more on.
    /// Only the streams (radio, autoplay, the endless mixes) let a worn
    /// track cool; the user's own lists never do.
    pub fn saturation_at(&self, i: usize) -> f32 {
        let c = self.saturated.get(i).copied().unwrap_or(0);
        (f32::from(c.saturating_sub(SATURATED_FROM)) / SATURATED_SPAN).min(1.0)
    }

    /// Plays of the track at `i` around this day a year ago.
    pub fn year_ago_plays_at(&self, i: usize) -> u32 {
        self.year_ago.get(i).map_or(0, |c| u32::from(*c))
    }

    /// Plays of the track at `i` in each part of the day, each as much as
    /// its context weighs ([`crate::context`]).
    pub fn daypart_plays_at(&self, i: usize) -> [f32; 4] {
        self.daypart.get(i).copied().unwrap_or_default()
    }

    /// The positive weights of the track at `i` in each part of the day,
    /// halving in [`SLOT_HALF_LIFE_MS`].
    pub fn part_mass_at(&self, i: usize) -> [f32; 4] {
        self.part_mass.get(i).copied().unwrap_or_default()
    }

    /// How much more than everyone's the track at `i` is played in `part`
    /// ([`crate::context::LIFT_MIN`]…[`crate::context::LIFT_MAX`]): its own
    /// share of the part leaning on the share of all plays by
    /// [`crate::context::KAPPA_TRACK_PART`], over that share. 0 before any
    /// play.
    pub fn part_lift_at(&self, i: usize, part: DayPart) -> f32 {
        let all: f32 = self.part_total.iter().sum();
        if all <= 0.0 {
            return 0.0;
        }
        let m = self.part_mass_at(i);
        let own: f32 = m.iter().sum();
        let p = part.index();
        let share = self.part_total[p] / all;
        let k = crate::context::KAPPA_TRACK_PART;
        let smoothed = (m[p] + k * share) / (own + k);
        (smoothed / share.max(0.05) - 1.0).clamp(crate::context::LIFT_MIN, crate::context::LIFT_MAX)
    }

    /// Whether anything was ever liked, played or put in a playlist with
    /// a positive outcome (the services' suggestions alone are not a
    /// history).
    pub fn has_history(&self) -> bool {
        self.history
    }

    /// The part of the affinity of the track at `i` that the services'
    /// suggestions gave (0…[`crate::suggest::SUGGESTED_CAP`]).
    pub fn suggested_at(&self, i: usize) -> f32 {
        self.suggested.get(i).copied().unwrap_or(0.0)
    }

    /// The artist's affinity: of the catalog, else of the songs saved
    /// elsewhere ([`Self::elsewhere_artists`]).
    pub fn artist_affinity(&self, artist: ArtistId) -> f32 {
        self.artist_affinity.get(&artist).or_else(|| self.elsewhere_artists.get(&artist)).copied().unwrap_or(0.0)
    }

    /// Every artist of the catalog with a non-zero affinity.
    pub fn artist_affinities(&self) -> impl Iterator<Item = (ArtistId, f32)> + '_ {
        self.artist_affinity.iter().map(|(a, s)| (*a, *s))
    }

    /// Artists the catalog has nothing of, with the affinity of their songs
    /// saved in a service's library ([`TasteInput::saved_elsewhere`]).
    pub fn elsewhere_artists(&self) -> impl Iterator<Item = (ArtistId, f32)> + '_ {
        self.elsewhere_artists.iter().map(|(a, s)| (*a, *s))
    }

    /// [`Self::artist_affinities`] and [`Self::elsewhere_artists`]: every
    /// artist the user's taste knows.
    pub fn all_artist_affinities(&self) -> impl Iterator<Item = (ArtistId, f32)> + '_ {
        self.artist_affinities().chain(self.elsewhere_artists())
    }

    /// The songs saved in a service's library that the catalog lacks, with
    /// their artists (none disliked), in the order given.
    pub fn saved_elsewhere(&self) -> &[(TrackId, Vec<ArtistId>)] {
        &self.saved_elsewhere
    }

    /// Artists played in the same sessions as `artist`, most often first.
    pub fn co_listened(&self, artist: ArtistId) -> &[(ArtistId, f32)] {
        self.co.get(&artist).map_or(&[], Vec::as_slice)
    }

    /// How many sessions linked two artists.
    pub fn co_count(&self, a: ArtistId, b: ArtistId) -> f32 {
        self.co_pairs.get(&ordered(a, b)).copied().unwrap_or(0.0)
    }

    /// The share of all plays in each day part.
    pub fn day_profile(&self) -> [f32; 4] {
        self.profile
    }

    /// How much more than evenly (−0.25…0.75) the track at `i` is played in
    /// `part`; 0 for tracks with fewer than three plays.
    pub fn daypart_bonus_at(&self, i: usize, part: DayPart) -> f32 {
        let Some(d) = self.daypart.get(i) else { return 0.0 };
        let total: f32 = d.iter().sum();
        if total < DAYPART_MIN_PLAYS {
            return 0.0;
        }
        d[part.index()] / total - 0.25
    }

    pub fn is_disliked_track(&self, id: TrackId) -> bool {
        self.disliked_tracks.contains(&id)
    }

    pub fn is_disliked_artist(&self, artist: ArtistId) -> bool {
        self.disliked_artists.contains(&artist)
    }

    /// Kept out of every pick: the track is disliked, or any of its artists.
    pub fn blocks(&self, track: &TrackInfo) -> bool {
        self.disliked_tracks.contains(&track.id) || track.artists.iter().any(|a| self.disliked_artists.contains(a))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::*;

    fn play(track: TrackId, ts: UnixMs, kind: EventType, pos: Option<u32>) -> Play {
        Play::new(track, ts, kind, pos)
    }

    /// A version's work is its song's track, a copy's its track's, the
    /// rest their own.
    #[test]
    fn tracks_know_their_work() {
        let cat = Catalog::new([info(0, &[1], None), info(1, &[1], None).with_song(tid(0)), info(2, &[1], None)]).with_aliases([(tid(9), tid(1))]);
        assert_eq!(cat.work_of(tid(0)), tid(0));
        assert_eq!(cat.work_of(tid(1)), tid(0));
        assert_eq!(cat.work_at(1), tid(0));
        assert_eq!(cat.work_of(tid(9)), tid(0), "a copy of the version");
        assert_eq!(cat.work_of(tid(2)), tid(2));
        assert_eq!(cat.work_of(tid(50)), tid(50));
    }

    /// A track played to the end five times in the last four days is worn
    /// (two past three of four more), one played so a week ago is not.
    #[test]
    fn worn_tracks() {
        let now = 1_000 * DAY_MS;
        let cat = Catalog::new((0..2).map(|i| info(i, &[1], Some(i))));
        let mut events = Vec::new();
        for k in 0..5 {
            events.push(play(tid(0), now - DAY_MS - k * 10 * HOUR_MS, EventType::Complete, None));
            events.push(play(tid(1), now - 7 * DAY_MS - k * HOUR_MS, EventType::Complete, None));
        }
        let taste = Taste::build(&cat, &TasteInput { events, ..TasteInput::default() }, now);
        assert!((taste.saturation_at(0) - 0.5).abs() < 1e-6, "{}", taste.saturation_at(0));
        assert_eq!(taste.saturation_at(1), 0.0);
    }

    #[test]
    fn event_weights_decay_and_repeats() {
        let now = 1_000 * DAY_MS;
        let cat = Catalog::new((0..6).map(|i| info(i, &[i as u32 % 3], Some(i))));
        let t = |i| tid(i);
        let input = TasteInput {
            events: vec![
                // 0: complete now, then started again within a day: +1 +0.5 +0.3.
                play(t(0), now - 3 * HOUR_MS, EventType::Start, None),
                play(t(0), now - 2 * HOUR_MS, EventType::Complete, None),
                play(t(0), now - HOUR_MS, EventType::Start, None),
                // 1: early skip −0.7, then a late one of unknown length: 0.
                play(t(1), now - 10 * HOUR_MS, EventType::Start, None),
                play(t(1), now - 10 * HOUR_MS, EventType::Skip, Some(5_000)),
                play(t(1), now - 9 * HOUR_MS, EventType::Start, None),
                play(t(1), now - 9 * HOUR_MS, EventType::Skip, Some(95_000)),
                // 2: one complete a half-life ago: half of +1.
                play(t(2), now - HALF_LIFE_MS, EventType::Complete, None),
                // An unknown track is ignored.
                play(tid(99), now, EventType::Complete, None),
            ],
            likes: vec![t(3), t(3)],
            like_times: vec![(t(3), now - DAY_MS), (t(4), now)],
            playlist_entries: vec![(t(4), now - 2 * HALF_LIFE_MS)],
            disliked_tracks: vec![t(5)],
            disliked_artists: vec![aid(1)],
            utc_offset_min: 0,
            ..Default::default()
        };
        let taste = Taste::build(&cat, &input, now);
        let a = |i| taste.affinity_at(cat.position(t(i)).unwrap());
        let close = |x: f32, y: f32| (x - y).abs() < 5e-3;
        assert!(close(a(0), 1.8), "{}", a(0));
        assert!(close(a(1), -0.7), "{}", a(1));
        assert!(close(a(2), 0.5), "{}", a(2));
        assert!(close(a(3), 3.0), "a like counts once, without decay: {}", a(3));
        assert!(close(a(4), 0.375), "{}", a(4));
        assert_eq!(taste.plays_at(0), 2);
        assert_eq!(taste.plays_at(2), 1, "a complete without a start is a play");
        assert_eq!(taste.last_played_at(0), Some(now - HOUR_MS));
        assert!(taste.liked_at(3));
        assert_eq!(taste.liked_time_at(3), Some(now - DAY_MS));
        assert_eq!(taste.liked_time_at(4), None, "a time without a like");
        assert_eq!(taste.recent_completes_at(0), 1);
        assert_eq!(taste.recent_completes_at(2), 0, "45 days ago");
        assert!(taste.blocks(&cat.tracks()[5]));
        assert!(taste.blocks(&cat.tracks()[1]), "artist 1 is disliked");
        assert!(!taste.blocks(&cat.tracks()[0]));
        assert!(taste.has_history());
        assert!(!Taste::build(&cat, &TasteInput::default(), now).has_history());
    }

    /// One recording on two albums: the copy is never in the catalog, it is
    /// looked up as its track, and its plays and like count for the track
    /// once («Не повторять сыгранное» covers both).
    #[test]
    fn a_copy_of_a_track_counts_as_the_track() {
        let now = 1_000 * DAY_MS;
        let (track, copy) = (tid(0), tid(50));
        let cat = Catalog::new((0..3).map(|i| info(i, &[i as u32], None))).with_aliases([(copy, track), (tid(1), tid(2)), (tid(60), tid(99))]);
        assert_eq!(cat.len(), 3, "the copy is no catalog track");
        assert_eq!(cat.position(copy), cat.position(track));
        assert_eq!(cat.get(copy).map(|t| t.id), Some(track));
        assert_eq!(cat.position(tid(1)), Some(1), "a catalog track is no copy");
        assert_eq!(cat.position(tid(60)), None, "a copy of nothing in the catalog");
        assert_eq!(cat.aliases().count(), 1);
        let input = TasteInput {
            events: vec![
                Play::new(track, now - 3 * HOUR_MS, EventType::Start, None),
                Play::new(copy, now - HOUR_MS, EventType::Start, None),
            ],
            likes: vec![track, copy],
            ..Default::default()
        };
        let taste = Taste::build(&cat, &input, now);
        let i = cat.position(track).unwrap();
        assert_eq!(taste.plays_at(i), 2, "the plays of both copies");
        assert_eq!(taste.last_played_at(i), Some(now - HOUR_MS), "the copy played last");
        assert!(taste.liked_at(i));
    }

    /// A song saved in a service's library is long taste without decay,
    /// lighter than a like and no like; liked as well, it is the like alone.
    #[test]
    fn saved_songs_are_taste_not_likes() {
        let now = 1_000 * DAY_MS;
        let cat = Catalog::new((0..3).map(|i| info(i, &[0], Some(i))));
        let input = TasteInput { likes: vec![tid(1)], saved: vec![tid(0), tid(0), tid(1)], ..Default::default() };
        let taste = Taste::build(&cat, &input, now);
        let a = |i| taste.affinity_at(cat.position(tid(i)).unwrap());
        assert!((a(0) - W_SAVED).abs() < 1e-6, "{}", a(0));
        assert!((a(1) - W_LIKE).abs() < 1e-6, "{}", a(1));
        assert_eq!(a(2), 0.0);
        assert!(!taste.liked_at(0));
        assert_eq!(taste.hot_at(0), 0.0, "nothing on repeat");
        assert!(taste.has_history());
    }

    /// A song saved in a service's library that the catalog lacks is one
    /// more song of its artists: the long taste of a library artist grows,
    /// an artist the catalog lacks gets its own (apart from the catalog's
    /// artists); no track of the catalog changes, and nothing of it is a
    /// history. A disliked one teaches nothing.
    #[test]
    fn songs_saved_elsewhere_teach_their_artists() {
        let now = 1_000 * DAY_MS;
        let cat = Catalog::new((0..2).map(|i| info(i, &[0], Some(i))));
        let before = Taste::build(&cat, &TasteInput { likes: vec![tid(0)], ..Default::default() }, now);
        let elsewhere = vec![(tid(10), vec![aid(0)]), (tid(11), vec![aid(5)]), (tid(11), vec![aid(5)]), (tid(12), vec![aid(6)])];
        let input = TasteInput {
            likes: vec![tid(0)],
            saved_elsewhere: elsewhere.clone(),
            disliked_artists: vec![aid(6)],
            ..Default::default()
        };
        let taste = Taste::build(&cat, &input, now);
        let close = |x: f32, y: f32| (x - y).abs() < 1e-5;
        assert!(close(before.artist_affinity(aid(0)), W_LIKE / 2f32.sqrt()));
        assert!(close(taste.artist_affinity(aid(0)), (W_LIKE + W_SAVED) / 3f32.sqrt()), "{}", taste.artist_affinity(aid(0)));
        assert!(close(taste.artist_affinity(aid(5)), W_SAVED), "once: {}", taste.artist_affinity(aid(5)));
        assert!(taste.artist_affinities().all(|(a, _)| a == aid(0)), "the catalog's artists apart");
        assert_eq!(taste.elsewhere_artists().map(|x| x.0).collect::<Vec<_>>(), [aid(5)]);
        assert_eq!(taste.all_artist_affinities().count(), 2);
        assert_eq!(taste.saved_elsewhere().iter().map(|s| s.0).collect::<Vec<_>>(), [tid(10), tid(11)]);
        assert_eq!(taste.affinity_at(1), before.affinity_at(1));
        let only = Taste::build(&cat, &TasteInput { saved_elsewhere: elsewhere, ..Default::default() }, now);
        assert!(!only.has_history(), "the catalog has no history of them");
    }

    /// A song skipped after three quarters was heard: a little plus, not a
    /// minus; the third early skip running in a playlist is leafing and
    /// weighs half; the hot affinity is the same weights over two days.
    #[test]
    fn late_skip_is_not_negative() {
        let now = 1_000 * DAY_MS;
        let cat = Catalog::new((0..5).map(|i| info(i, &[0], Some(i)).with_duration(Some(200_000))));
        let t = |i| tid(i);
        let skip = |i, at, pos, context| Play { context, ..play(t(i), at, EventType::Skip, Some(pos)) };
        let events = vec![
            play(t(0), now - HOUR_MS, EventType::Start, None),
            play(t(0), now - HOUR_MS, EventType::Skip, Some(150_000)),
            // Three early skips running in a playlist, a minute apart.
            skip(1, now - 30 * 60_000, 5_000, EventContext::Playlist),
            skip(2, now - 29 * 60_000, 5_000, EventContext::Playlist),
            skip(3, now - 28 * 60_000, 5_000, EventContext::Playlist),
            // A complete two days ago: half of it is hot.
            play(t(4), now - 2 * DAY_MS, EventType::Complete, None),
        ];
        let taste = Taste::build(&cat, &TasteInput { events: events.clone(), ..Default::default() }, now);
        let a = |i| taste.affinity_at(cat.position(t(i)).unwrap());
        let close = |x: f32, y: f32| (x - y).abs() < 5e-3;
        assert!(close(a(0), 0.15 * 0.75), "{}", a(0));
        assert!(close(a(1), -0.7) && close(a(2), -0.7), "{} {}", a(1), a(2));
        assert!(close(a(3), -0.35), "leafing: {}", a(3));
        assert!(close(taste.hot_at(cat.position(t(4)).unwrap()), 0.5));
        let hot = hot_affinity(&events, |_| Some(200_000), now);
        assert!(close(hot[&t(4)], 0.5) && close(hot[&t(3)], -0.35), "{hot:?}");
    }

    #[test]
    fn artist_affinity_is_normalised_by_discography() {
        // Artist 0: 16 tracks, each completed once; artist 1: one track
        // completed 4 times. Sum 16 / √16 = 4 against 4 / √1 = 4.
        let now = 100 * DAY_MS;
        let mut tracks: Vec<TrackInfo> = (0..16).map(|i| info(i, &[0], None)).collect();
        tracks.push(info(100, &[1], None));
        let cat = Catalog::new(tracks);
        let mut events: Vec<Play> = (0..16).map(|i| play(tid(i), now, EventType::Complete, None)).collect();
        events.extend((0..4).map(|k| play(tid(100), now - k * 2 * DAY_MS, EventType::Complete, None)));
        let taste = Taste::build(&cat, &TasteInput { events, ..Default::default() }, now);
        let (a0, a1) = (taste.artist_affinity(aid(0)), taste.artist_affinity(aid(1)));
        assert!((a0 - 4.0).abs() < 1e-3, "{a0}");
        assert!(a1 > 3.8 && a1 < 4.0, "{a1}");
    }

    #[test]
    fn sessions_link_artists() {
        let now = 100 * DAY_MS;
        let cat = Catalog::new((0..4).map(|i| info(i, &[i as u32], None)));
        let p = |i: u128, ts| play(tid(i), ts, EventType::Start, None);
        let events = vec![
            p(0, now - 10 * HOUR_MS),
            p(1, now - 10 * HOUR_MS + 4 * 60_000),
            p(0, now - 10 * HOUR_MS + 8 * 60_000),
            // An hour later: a new session.
            p(2, now - 8 * HOUR_MS),
            p(3, now - 8 * HOUR_MS + 60_000),
            p(0, now - 5 * HOUR_MS),
            p(1, now - 5 * HOUR_MS + 60_000),
        ];
        let taste = Taste::build(&cat, &TasteInput { events, ..Default::default() }, now);
        assert_eq!(taste.co_count(aid(0), aid(1)), 2.0, "once per session");
        assert_eq!(taste.co_count(aid(2), aid(3)), 1.0);
        assert_eq!(taste.co_count(aid(1), aid(2)), 0.0);
        assert_eq!(taste.co_listened(aid(0))[0].0, aid(1));
    }

    #[test]
    fn day_parts() {
        assert_eq!(DayPart::of_hour(6), DayPart::Morning);
        assert_eq!(DayPart::of_hour(10), DayPart::Morning);
        assert_eq!(DayPart::of_hour(11), DayPart::Day);
        assert_eq!(DayPart::of_hour(18), DayPart::Evening);
        assert_eq!(DayPart::of_hour(23), DayPart::Night);
        assert_eq!(DayPart::of_hour(5), DayPart::Night);
        // 21:00 UTC is midnight in Moscow (+3).
        assert_eq!(DayPart::at(21 * HOUR_MS, 180), DayPart::Night);
        assert_eq!(DayPart::at(21 * HOUR_MS, 0), DayPart::Evening);

        let now = 100 * DAY_MS;
        let cat = Catalog::new([info(0, &[0], None)]);
        let events = (0..4)
            .map(|d| play(tid(0), now - d * DAY_MS - 2 * HOUR_MS, EventType::Start, None))
            .collect();
        let taste = Taste::build(&cat, &TasteInput { events, ..Default::default() }, now);
        // now - 2h is 22:00 UTC: evening.
        assert!((taste.daypart_bonus_at(0, DayPart::Evening) - 0.75).abs() < 1e-6);
        assert!((taste.daypart_bonus_at(0, DayPart::Morning) + 0.25).abs() < 1e-6);
        assert_eq!(taste.day_profile()[DayPart::Evening.index()], 1.0);
    }

    #[test]
    fn windows_of_the_tiles() {
        let now = 1_000 * DAY_MS;
        let cat = Catalog::new((0..3).map(|i| info(i, &[0], None)));
        let c = |i: u128, ts| play(tid(i), ts, EventType::Complete, None);
        let events = vec![
            c(0, now - 10 * DAY_MS),
            c(0, now - 29 * DAY_MS),
            c(0, now - 31 * DAY_MS),
            c(1, now - YEAR_MS + 10 * DAY_MS),
            c(1, now - YEAR_MS - 13 * DAY_MS),
            c(1, now - YEAR_MS - 20 * DAY_MS),
            play(tid(2), now - YEAR_MS, EventType::Start, None),
        ];
        let taste = Taste::build(&cat, &TasteInput { events, ..Default::default() }, now);
        assert_eq!(taste.recent_completes_at(0), 2);
        assert_eq!(taste.year_ago_plays_at(0), 0);
        assert_eq!(taste.year_ago_plays_at(1), 2, "±14 days");
        assert_eq!(taste.year_ago_plays_at(2), 1, "a start is a play");
    }

    #[test]
    fn a_night_belongs_to_the_day_it_started() {
        let day = 3 * DAY_MS;
        assert_eq!(DayPart::day_of(day + 2 * HOUR_MS, 0), 2);
        assert_eq!(DayPart::day_of(day + 6 * HOUR_MS, 0), 3);
        assert_eq!(DayPart::day_of(day + 23 * HOUR_MS, 0), 3);
        // 21:00 UTC is midnight in Moscow: still the night of that day.
        assert_eq!(DayPart::day_of(day + 21 * HOUR_MS, 180), 3);
        assert_eq!(DayPart::day_of(day + 4 * HOUR_MS, 180), 3, "07:00 in Moscow");
        assert_eq!(DayPart::day_of(day + 2 * HOUR_MS, 180), 2, "05:00 in Moscow");
    }

    /// «Включи X» to the assistant is the owner's own pick, a little less
    /// (six of them stand between four and five picks at the computer); two
    /// early «дальше» on the speaker push a song down; the radio going on
    /// for hours after a command weighs a few songs at the computer (more
    /// on the speaker itself), and none of them counts as on repeat.
    #[test]
    fn a_voice_request_is_intent_and_a_marathon_a_few_songs() {
        use crate::context::{PlayActor, PlayDevice};
        let now = 1_000 * DAY_MS;
        let cat = Catalog::new((0..80).map(|i| info(i, &[i as u32 % 10], Some(i)).with_duration(Some(180_000))));
        let speaker = |p: Play| Play { actor: PlayActor::Assistant, device: PlayDevice::Ambient, ..p };
        let pick = |track: u128, ts: UnixMs| Play { origin: Some(EventOrigin::Manual), ..play(tid(track), ts, EventType::Start, Some(0)) };
        let done = |track: u128, ts: UnixMs| play(tid(track), ts + 180_000, EventType::Complete, Some(180_000));
        let mut events = Vec::new();
        // Every other day (no repeats within a day).
        for d in (3..15i64).step_by(2) {
            let day = now - d * DAY_MS;
            // The voice: «включи» song 1 in the morning, on the speaker.
            events.push(speaker(pick(1, day + 9 * HOUR_MS)));
            events.push(speaker(done(1, day + 9 * HOUR_MS)));
            // The computer: song 2 four evenings, song 3 five.
            if d < 10 {
                events.push(pick(2, day + 20 * HOUR_MS));
                events.push(done(2, day + 20 * HOUR_MS));
            }
            if d < 12 {
                events.push(pick(3, day + 21 * HOUR_MS));
                events.push(done(3, day + 21 * HOUR_MS));
            }
        }
        // A command, then the radio fifty songs on, untouched.
        let t0 = now - DAY_MS + 8 * HOUR_MS;
        events.push(speaker(pick(5, t0)));
        events.push(speaker(done(5, t0)));
        for k in 0..50u128 {
            let ts = t0 + (k as i64 + 1) * 181_000;
            events.push(speaker(Play { origin: Some(EventOrigin::Radio), ..play(tid(10 + k), ts, EventType::Start, Some(0)) }));
            events.push(speaker(play(tid(10 + k), ts + 179_000, EventType::Complete, Some(179_000))));
        }
        // Two early «дальше» by voice.
        for h in [12, 11] {
            let ts = now - h * HOUR_MS;
            events.push(speaker(Play { origin: Some(EventOrigin::Radio), ..play(tid(4), ts, EventType::Start, Some(0)) }));
            events.push(speaker(Play { end_reason: Some(EndReason::UserNext), ..play(tid(4), ts + 8_000, EventType::Skip, Some(8_000)) }));
        }
        let taste = Taste::build(&cat, &TasteInput { events: events.clone(), ..TasteInput::default() }, now);
        let a = |t: &Taste, i: u128| t.affinity_at(cat.position(tid(i)).unwrap());
        assert!(a(&taste, 2) < a(&taste, 1) && a(&taste, 1) < a(&taste, 3), "{} {} {}", a(&taste, 2), a(&taste, 1), a(&taste, 3));
        assert!(a(&taste, 4) < -1.0, "{}", a(&taste, 4));
        let radio = |t: &Taste| (10..60u128).map(|i| a(t, i)).sum::<f32>();
        assert!(radio(&taste) > 1.0 && radio(&taste) < 5.0, "{}", radio(&taste));
        let i10 = cat.position(tid(10)).unwrap();
        assert_eq!(taste.plays_at(i10), 1, "heard");
        assert_eq!(taste.recent_completes_at(i10), 0, "not on repeat");
        assert_eq!(taste.recent_completes_at(cat.position(tid(1)).unwrap()), 6, "the asked-for song is");
        // On the speaker itself its own radio weighs more.
        let room = Taste::build(&cat, &TasteInput { events, target_device: PlayDevice::Ambient, ..TasteInput::default() }, now);
        assert!(radio(&room) > 2.0 * radio(&taste), "{} {}", radio(&room), radio(&taste));
    }

    #[test]
    fn a_big_history_is_quick() {
        let (cat, input, now) = big_history(50_000, 100_000, 5_000);
        let t = std::time::Instant::now();
        let taste = Taste::build(&cat, &input, now);
        let took = t.elapsed();
        assert!(taste.has_history());
        assert!(taste.artist_affinities().count() > 1_000);
        assert!(took < std::time::Duration::from_secs(2), "{took:?}");
    }
}
