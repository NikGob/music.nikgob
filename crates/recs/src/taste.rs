//! The taste model (docs/stage4.md §3): what the listening history, the
//! likes and the playlists say about every track and artist of the library.
//!
//! * **Track affinity**: the sum of event weights, each decayed with a
//!   90-day half-life from its moment: a complete +1.0, a start that neither
//!   completed nor was skipped +0.3, a skip in the first 30 s −0.7 and a
//!   later one −0.1, a repeat (started again within a day of completing)
//!   +0.5, a playlist entry +1.5 (decayed from when it was added); a like is
//!   +3, a state without decay. Imported events (Takeout, Spotify) count the
//!   same.
//! * **Artist affinity**: the affinities of the artist's tracks summed and
//!   divided by the square root of the artist's track count in the catalog,
//!   so a big discography does not win by size alone.
//! * **Co-listening**: plays of one session (gaps under 30 minutes) link
//!   each play's main artist with the main artists of the
//!   [`CO_WINDOW`] plays before it, a pair once per session: the fallback
//!   similarity for tracks without a sound vector.
//! * **Day parts**: per track, how many plays fell into the morning (6–11),
//!   the day (11–18), the evening (18–23) and the night (23–6), local time.
//! * **Service suggestions** (docs/stage4.md §12.3, [`crate::suggest`]): a
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

use nikgob_core::model::{ArtistId, Event, EventType, ReleaseId, TrackId, UnixMs};
use serde::{Deserialize, Serialize};

use crate::suggest;

pub const HOUR_MS: i64 = 60 * 60 * 1000;
pub const DAY_MS: i64 = 24 * HOUR_MS;

/// Affinity halves every this long.
pub const HALF_LIFE_MS: i64 = 90 * DAY_MS;
pub const W_COMPLETE: f32 = 1.0;
pub const W_START_ONLY: f32 = 0.3;
pub const W_SKIP_EARLY: f32 = -0.7;
pub const W_SKIP_LATE: f32 = -0.1;
pub const W_REPEAT: f32 = 0.5;
pub const W_PLAYLIST: f32 = 1.5;
pub const W_LIKE: f32 = 3.0;
/// A skip before this position is an early one.
pub const EARLY_SKIP_MS: u32 = 30_000;
/// A start this soon after a complete of the same track is a repeat.
pub const REPEAT_WITHIN_MS: i64 = DAY_MS;
/// A pause this long starts a new session.
pub const SESSION_GAP_MS: i64 = 30 * 60 * 1000;
/// A play links its artist with the artists of this many plays before it.
pub const CO_WINDOW: usize = 8;
/// Plays a track needs before its own day-part profile counts.
const DAYPART_MIN_PLAYS: u16 = 3;
/// «На повторе» and «Свежие лайки» look this far back.
pub const ON_REPEAT_MS: i64 = 30 * DAY_MS;
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

    pub fn main_artist(&self) -> Option<ArtistId> {
        self.artists.first().copied()
    }
}

/// The tracks the engine knows, with lookups by id and by artist.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    tracks: Vec<TrackInfo>,
    pos: HashMap<TrackId, u32>,
    by_artist: HashMap<ArtistId, Vec<u32>>,
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

    /// The catalog position of a track.
    pub fn position(&self, id: TrackId) -> Option<usize> {
        self.pos.get(&id).map(|&i| i as usize)
    }

    pub fn get(&self, id: TrackId) -> Option<&TrackInfo> {
        self.position(id).map(|i| &self.tracks[i])
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Play {
    pub track: TrackId,
    pub ts: UnixMs,
    pub kind: EventType,
    pub position_ms: Option<u32>,
}

impl From<&Event> for Play {
    fn from(e: &Event) -> Self {
        Self {
            track: e.track_id,
            ts: e.ts,
            kind: e.event_type,
            position_ms: e.position_ms,
        }
    }
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
}

/// Per-track and per-artist signals at one moment (`now`).
#[derive(Clone, Debug)]
pub struct Taste {
    now: UnixMs,
    utc_offset_min: i32,
    /// By catalog position.
    affinity: Vec<f32>,
    plays: Vec<u32>,
    last_played: Vec<Option<UnixMs>>,
    liked: Vec<bool>,
    liked_time: Vec<Option<UnixMs>>,
    daypart: Vec<[u16; 4]>,
    /// The part of the affinity the services' suggestions gave.
    suggested: Vec<f32>,
    /// Anything listened, liked or put in a playlist (suggestions aside).
    history: bool,
    /// Completes within [`ON_REPEAT_MS`].
    recent_completes: Vec<u16>,
    /// Plays within [`YEAR_AGO_SPAN_MS`] of now − [`YEAR_MS`].
    year_ago: Vec<u16>,
    artist_affinity: HashMap<ArtistId, f32>,
    co_pairs: HashMap<(ArtistId, ArtistId), f32>,
    co: HashMap<ArtistId, Vec<(ArtistId, f32)>>,
    profile: [f32; 4],
    disliked_tracks: HashSet<TrackId>,
    disliked_artists: HashSet<ArtistId>,
}

fn decay(now: UnixMs, ts: UnixMs) -> f32 {
    let age = (now - ts).max(0) as f64;
    (-age / HALF_LIFE_MS as f64).exp2() as f32
}

fn ordered(a: ArtistId, b: ArtistId) -> (ArtistId, ArtistId) {
    if a <= b { (a, b) } else { (b, a) }
}

impl Taste {
    /// The model of `input` over `catalog` at `now`. Events of tracks the
    /// catalog lacks are ignored.
    pub fn build(catalog: &Catalog, input: &TasteInput, now: UnixMs) -> Self {
        let n = catalog.len();
        let mut affinity = vec![0.0f32; n];
        let mut plays = vec![0u32; n];
        let mut last_played: Vec<Option<UnixMs>> = vec![None; n];
        let mut liked = vec![false; n];
        let mut daypart = vec![[0u16; 4]; n];
        let mut recent_completes = vec![0u16; n];
        let mut year_ago = vec![0u16; n];
        let mut open: Vec<Option<UnixMs>> = vec![None; n];
        let mut last_complete: Vec<Option<UnixMs>> = vec![None; n];

        let mut events: Vec<(UnixMs, usize, EventType, Option<u32>)> = input
            .events
            .iter()
            .filter(|p| {
                matches!(p.kind, EventType::Start | EventType::Complete | EventType::Skip)
            })
            .filter_map(|p| Some((p.ts, catalog.position(p.track)?, p.kind, p.position_ms)))
            .collect();
        events.sort_by_key(|e| e.0);

        // Sessions and co-listening.
        let mut co_pairs: HashMap<(ArtistId, ArtistId), f32> = HashMap::new();
        let mut tail: VecDeque<ArtistId> = VecDeque::with_capacity(CO_WINDOW + 1);
        let mut session_pairs: HashSet<(ArtistId, ArtistId)> = HashSet::new();
        let mut last_play_ts: Option<UnixMs> = None;

        for &(ts, i, kind, pos) in &events {
            let mut counted_play = false;
            match kind {
                EventType::Start => {
                    if let Some(t0) = open[i] {
                        affinity[i] += W_START_ONLY * decay(now, t0);
                    }
                    if last_complete[i].is_some_and(|c| ts - c <= REPEAT_WITHIN_MS) {
                        affinity[i] += W_REPEAT * decay(now, ts);
                    }
                    open[i] = Some(ts);
                    counted_play = true;
                }
                EventType::Complete => {
                    // A complete without its start (some imports) is a play too.
                    counted_play = open[i].take().is_none();
                    affinity[i] += W_COMPLETE * decay(now, ts);
                    if now - ts < ON_REPEAT_MS {
                        recent_completes[i] = recent_completes[i].saturating_add(1);
                    }
                    last_complete[i] = Some(ts);
                    last_played[i] = last_played[i].max(Some(ts));
                }
                EventType::Skip => {
                    open[i] = None;
                    // A skip without a position counts as a late one.
                    let w = if pos.is_some_and(|p| p < EARLY_SKIP_MS) { W_SKIP_EARLY } else { W_SKIP_LATE };
                    affinity[i] += w * decay(now, ts);
                }
                _ => {}
            }
            if !counted_play {
                continue;
            }
            plays[i] += 1;
            last_played[i] = last_played[i].max(Some(ts));
            let part = DayPart::at(ts, input.utc_offset_min).index();
            daypart[i][part] = daypart[i][part].saturating_add(1);
            if (ts - (now - YEAR_MS)).abs() <= YEAR_AGO_SPAN_MS {
                year_ago[i] = year_ago[i].saturating_add(1);
            }

            if last_play_ts.is_none_or(|l| ts - l >= SESSION_GAP_MS) {
                tail.clear();
                session_pairs.clear();
            }
            last_play_ts = Some(ts);
            let Some(a) = catalog.tracks[i].main_artist() else { continue };
            for &b in &tail {
                if b != a {
                    let pair = ordered(a, b);
                    if session_pairs.insert(pair) {
                        *co_pairs.entry(pair).or_default() += 1.0;
                    }
                }
            }
            tail.push_back(a);
            if tail.len() > CO_WINDOW {
                tail.pop_front();
            }
        }
        for (i, t0) in open.iter().enumerate() {
            if let Some(t0) = t0 {
                affinity[i] += W_START_ONLY * decay(now, *t0);
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
                affinity[i] += W_PLAYLIST * decay(now, *added);
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
        let mut artist_affinity: HashMap<ArtistId, f32> = artist_sum
            .into_iter()
            .map(|(a, s)| (a, s / (catalog.of_artist(a).len().max(1) as f32).sqrt()))
            .collect();
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
                *p += f32::from(*c);
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
            plays,
            last_played,
            liked,
            liked_time,
            daypart,
            suggested,
            history,
            recent_completes,
            year_ago,
            artist_affinity,
            co_pairs,
            co,
            profile,
            disliked_tracks: input.disliked_tracks.iter().copied().collect(),
            disliked_artists: input.disliked_artists.iter().copied().collect(),
        }
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

    pub fn plays_at(&self, i: usize) -> u32 {
        self.plays.get(i).copied().unwrap_or(0)
    }

    pub fn last_played_at(&self, i: usize) -> Option<UnixMs> {
        self.last_played.get(i).copied().flatten()
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

    /// Plays of the track at `i` around this day a year ago.
    pub fn year_ago_plays_at(&self, i: usize) -> u32 {
        self.year_ago.get(i).map_or(0, |c| u32::from(*c))
    }

    /// Plays of the track at `i` in each part of the day.
    pub fn daypart_plays_at(&self, i: usize) -> [u16; 4] {
        self.daypart.get(i).copied().unwrap_or_default()
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

    pub fn artist_affinity(&self, artist: ArtistId) -> f32 {
        self.artist_affinity.get(&artist).copied().unwrap_or(0.0)
    }

    /// Every artist with a non-zero affinity.
    pub fn artist_affinities(&self) -> impl Iterator<Item = (ArtistId, f32)> + '_ {
        self.artist_affinity.iter().map(|(a, s)| (*a, *s))
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
        let total: u16 = d.iter().sum();
        if total < DAYPART_MIN_PLAYS {
            return 0.0;
        }
        f32::from(d[part.index()]) / f32::from(total) - 0.25
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
        Play {
            track,
            ts,
            kind,
            position_ms: pos,
        }
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
                // 1: early skip −0.7, then a late one −0.1.
                play(t(1), now - 10 * HOUR_MS, EventType::Start, None),
                play(t(1), now - 10 * HOUR_MS, EventType::Skip, Some(5_000)),
                play(t(1), now - 9 * HOUR_MS, EventType::Start, None),
                play(t(1), now - 9 * HOUR_MS, EventType::Skip, Some(95_000)),
                // 2: one complete 90 days ago: half of +1.
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
        assert!(close(a(1), -0.8), "{}", a(1));
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
        assert_eq!(taste.recent_completes_at(2), 0, "90 days ago");
        assert!(taste.blocks(&cat.tracks()[5]));
        assert!(taste.blocks(&cat.tracks()[1]), "artist 1 is disliked");
        assert!(!taste.blocks(&cat.tracks()[0]));
        assert!(taste.has_history());
        assert!(!Taste::build(&cat, &TasteInput::default(), now).has_history());
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
