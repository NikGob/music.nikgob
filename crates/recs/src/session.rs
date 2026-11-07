//! The listening session (recs-v2 §5.1): what played since the last pause
//! of [`SESSION_GAP_MS`] or more, how each ended, and what that says —
//! the sound to continue (the anchor), the sounds skipped, an album or a
//! world being listened through, one track on repeat, a run of skips.
//!
//! The runtime feeds every start and end ([`Session::start`],
//! [`Session::end`]); a radio keeps one of its own for its tracks
//! ([`crate::RadioState::feedback`]) that the runtime replaces with the
//! real one before it picks.

use std::collections::{HashMap, VecDeque};

use nikgob_core::model::{EventContext, EventOrigin, TrackId, UnixMs};
use serde::{Deserialize, Serialize};

use crate::analysis::{DIM, Vector};
use crate::songtype::SongLane;
use crate::taste::SESSION_GAP_MS;

/// Tracks a session remembers.
pub const SESSION_LEN: usize = 20;
/// The anchor is the last this many tracks not skipped early…
const ANCHOR_TRACKS: usize = 3;
/// …each this much less than the one after it.
const ANCHOR_DECAY: f32 = 0.6;
/// Early skips whose sound pushes picks away.
const PUSHES: usize = 6;
/// The lane of a session is the commonest of this many last tracks.
const LANE_TRACKS: usize = 5;

/// How a start of the session ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ended {
    /// Still playing, or the app closed on it.
    Open,
    Complete,
    /// Skipped in its first 30 s.
    Early,
    Late,
}

/// One start of the session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Played {
    pub track: TrackId,
    pub ts: UnixMs,
    pub ended: Ended,
    pub context: EventContext,
    pub origin: Option<EventOrigin>,
}

/// The session (see the module docs).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Session {
    tracks: VecDeque<Played>,
    last_ts: Option<UnixMs>,
}

/// What [`Session`] needs to know of a track beyond its id.
pub trait TrackLookup {
    fn vector(&self, id: TrackId) -> Option<Vector>;
    /// Its release and its place there (disc × 1000 + number).
    fn album(&self, id: TrackId) -> Option<(u64, u32)>;
    fn world(&self, id: TrackId) -> Option<u64>;
    fn lane(&self, id: TrackId) -> Option<SongLane>;
}

impl Session {
    /// A start at `ts`; after a pause of [`SESSION_GAP_MS`] a new session
    /// begins (returns true then).
    pub fn start(&mut self, track: TrackId, ts: UnixMs, context: EventContext, origin: Option<EventOrigin>) -> bool {
        let new = self.last_ts.is_none_or(|l| ts - l >= SESSION_GAP_MS);
        if new {
            self.tracks.clear();
        }
        self.last_ts = Some(ts);
        self.tracks.push_back(Played { track, ts, ended: Ended::Open, context, origin });
        while self.tracks.len() > SESSION_LEN {
            self.tracks.pop_front();
        }
        new
    }

    /// How the last start of `track` ended.
    pub fn end(&mut self, track: TrackId, ended: Ended) {
        if let Some(p) = self.tracks.iter_mut().rev().find(|p| p.track == track && p.ended == Ended::Open) {
            p.ended = ended;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    /// Oldest first.
    pub fn tracks(&self) -> impl DoubleEndedIterator<Item = &Played> + ExactSizeIterator {
        self.tracks.iter()
    }

    pub fn last(&self) -> Option<&Played> {
        self.tracks.back()
    }

    pub fn last_ts(&self) -> Option<UnixMs> {
        self.last_ts
    }

    /// Early skips running at the end.
    pub fn skip_streak(&self) -> u8 {
        self.tracks.iter().rev().take_while(|p| p.ended == Ended::Early).count().min(255) as u8
    }

    /// One track twice running (Greymoor five times in a night).
    pub fn repeat_one(&self) -> bool {
        let mut last = self.tracks.iter().rev();
        matches!((last.next(), last.next()), (Some(a), Some(b)) if a.track == b.track)
    }

    /// An album listened through: the last track played from an album, or
    /// right after the one before it on its album.
    pub fn album_session(&self, look: &impl TrackLookup) -> bool {
        let mut last = self.tracks.iter().rev();
        let Some(a) = last.next() else { return false };
        if a.context == EventContext::Album {
            return true;
        }
        let Some(b) = last.next() else { return false };
        matches!((look.album(b.track), look.album(a.track)), (Some((ra, pa)), Some((rb, pb))) if ra == rb && pb > pa)
    }

    /// Two of the last three of one world: that world.
    pub fn world_session(&self, look: &impl TrackLookup) -> Option<u64> {
        let worlds: Vec<u64> = self.tracks.iter().rev().take(3).filter_map(|p| look.world(p.track)).collect();
        worlds.iter().copied().find(|w| worlds.iter().filter(|x| *x == w).count() >= 2)
    }

    /// The sound to continue: the last [`ANCHOR_TRACKS`] not skipped early,
    /// weighted 1, 0.6, 0.36 from the last; else the last track's own.
    pub fn anchor(&self, look: &impl TrackLookup) -> Option<Vector> {
        let mut sum = [0.0f32; DIM];
        let (mut w_total, mut w, mut k) = (0.0, 1.0, 0);
        for p in self.tracks.iter().rev() {
            if p.ended == Ended::Early {
                continue;
            }
            let Some(v) = look.vector(p.track) else { continue };
            for (s, x) in sum.iter_mut().zip(v) {
                *s += w * x;
            }
            w_total += w;
            w *= ANCHOR_DECAY;
            k += 1;
            if k == ANCHOR_TRACKS {
                break;
            }
        }
        if w_total > 0.0 {
            return Some(sum.map(|s| s / w_total));
        }
        self.last().and_then(|p| look.vector(p.track))
    }

    /// The sounds skipped early in the session (the last [`PUSHES`]).
    pub fn pushes(&self, look: &impl TrackLookup) -> Vec<Vector> {
        let all: Vec<Vector> = self.tracks.iter().filter(|p| p.ended == Ended::Early).filter_map(|p| look.vector(p.track)).collect();
        all[all.len().saturating_sub(PUSHES)..].to_vec()
    }

    /// The commonest song lane of the last [`LANE_TRACKS`] (the earliest
    /// of them on a tie).
    pub fn lane(&self, look: &impl TrackLookup) -> Option<SongLane> {
        let from = self.tracks.len().saturating_sub(LANE_TRACKS);
        let lanes: Vec<SongLane> = self.tracks.iter().skip(from).filter_map(|p| look.lane(p.track)).collect();
        let mut count: HashMap<SongLane, usize> = HashMap::new();
        for l in &lanes {
            *count.entry(*l).or_default() += 1;
        }
        let best = count.values().copied().max()?;
        lanes.into_iter().find(|l| count[l] == best)
    }

    /// How many times `track` started in the session.
    pub fn count(&self, track: TrackId) -> usize {
        self.tracks.iter().filter(|p| p.track == track).count()
    }

    /// What the session is (profiles-v1 §4.2): pure in one taste profile
    /// or mixed, by the profiles (`profile` tells a track's) of what was
    /// not skipped early; `None` before a start of a known profile. A
    /// track of another profile picked by hand makes it mixed; two early
    /// skips take nothing in.
    pub fn mode(&self, profile: impl Fn(TrackId) -> Option<usize>) -> Option<crate::listening::Mode> {
        let profiles: Vec<Option<usize>> = self.tracks.iter().filter(|p| p.ended != Ended::Early).map(|p| profile(p.track)).collect();
        crate::listening::infer_mode(&profiles)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::*;

    struct Look;

    impl TrackLookup for Look {
        fn vector(&self, id: TrackId) -> Option<Vector> {
            Some(line(id.0.as_u128() as f32))
        }
        fn album(&self, id: TrackId) -> Option<(u64, u32)> {
            let i = id.0.as_u128() as u32;
            Some((u64::from(i / 10), i % 10))
        }
        fn world(&self, id: TrackId) -> Option<u64> {
            Some(u64::from(id.0.as_u128() as u32 / 100))
        }
        fn lane(&self, id: TrackId) -> Option<SongLane> {
            Some(if id.0.as_u128() < 50 { SongLane::Russian } else { SongLane::Instrumental })
        }
    }

    const T: UnixMs = 1_000_000_000;

    #[test]
    fn repeat_one_detected() {
        let mut s = Session::default();
        s.start(tid(7), T, EventContext::Album, None);
        s.end(tid(7), Ended::Complete);
        assert!(!s.repeat_one());
        s.start(tid(7), T + 200_000, EventContext::Album, None);
        assert!(s.repeat_one(), "the same track twice running");
        s.start(tid(8), T + 400_000, EventContext::Album, None);
        assert!(!s.repeat_one());
    }

    #[test]
    fn a_pause_starts_a_new_session_and_skips_run() {
        let mut s = Session::default();
        assert!(s.start(tid(1), T, EventContext::Playlist, None));
        s.end(tid(1), Ended::Early);
        s.start(tid(2), T + 10_000, EventContext::Playlist, None);
        s.end(tid(2), Ended::Early);
        assert_eq!(s.skip_streak(), 2);
        s.start(tid(3), T + 20_000, EventContext::Playlist, None);
        assert_eq!(s.skip_streak(), 0, "the third still plays");
        assert_eq!(s.pushes(&Look), vec![line(1.0), line(2.0)]);
        assert_eq!(s.anchor(&Look), Some(line(3.0)), "skips are no anchor");
        assert!(s.start(tid(4), T + 20_000 + SESSION_GAP_MS, EventContext::Album, None));
        assert_eq!(s.len(), 1);
    }

    /// Three of one profile: pure; one of another picked: mixed; an early
    /// skip of a third counts for nothing.
    #[test]
    fn the_mode_of_a_session() {
        let profile = |t: TrackId| Some((t.0.as_u128() / 100) as usize);
        let mut s = Session::default();
        assert_eq!(s.mode(profile), None);
        for k in 0..3 {
            s.start(tid(10 + k), T + k as i64 * 200_000, EventContext::Library, None);
            s.end(tid(10 + k), Ended::Complete);
        }
        assert_eq!(s.mode(profile), Some(crate::listening::Mode::Pure(0)));
        s.start(tid(250), T + 700_000, EventContext::Library, None);
        s.end(tid(250), Ended::Early);
        assert_eq!(s.mode(profile), Some(crate::listening::Mode::Pure(0)), "skipped early");
        s.start(tid(120), T + 800_000, EventContext::Library, None);
        assert_eq!(s.mode(profile), Some(crate::listening::Mode::Mixed));
    }

    #[test]
    fn albums_worlds_and_lanes_of_a_session() {
        let mut s = Session::default();
        s.start(tid(11), T, EventContext::Radio, None);
        s.start(tid(12), T + 1, EventContext::Radio, None);
        assert!(s.album_session(&Look), "the next one on its album");
        s.start(tid(130), T + 2, EventContext::Radio, None);
        assert!(!s.album_session(&Look));
        assert_eq!(s.world_session(&Look), Some(0), "two of three in world 0");
        s.start(tid(140), T + 3, EventContext::Radio, None);
        assert_eq!(s.world_session(&Look), Some(1));
        assert_eq!(s.lane(&Look), Some(SongLane::Russian), "a tie: the earlier");
        let a = s.anchor(&Look).unwrap()[0];
        let want = (140.0 + 0.6 * 130.0 + 0.36 * 12.0) / (1.0 + 0.6 + 0.36);
        assert!((a - want).abs() < 1e-3, "{a}");
    }
}
