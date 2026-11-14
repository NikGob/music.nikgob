//! The listening session: what played since the last pause
//! of [`SESSION_GAP_MS`] or more, how each ended, and what that says —
//! the sound to continue (the anchor), the sounds skipped, an album or a
//! world being listened through, one track on repeat, a run of skips.
//!
//! The runtime feeds every start and end ([`Session::start`],
//! [`Session::end_at`]: where it ended, its length, whether the bar was
//! dragged first, [`Session::seek`]) and every like ([`Session::like`]); a
//! radio keeps one of its own for its tracks
//! ([`crate::RadioState::feedback`]) that the runtime replaces with the
//! real one before it picks.
//!
//! What a start says ([`Verdict`]): it is approved when it played to the
//! end, was skipped only at its outro ([`APPROVED_SHARE`] of its length or
//! later, the bar untouched), or was liked; only those make the anchor (the
//! track still playing has said nothing yet: a radio's seed leads its own
//! picks, not the session's anchor). With the length known a skip is
//! judged by its share, however soon it came (a short track's outro may
//! come before 30 s). A skip in its first half, or after the bar was dragged (a
//! scrub-skip: the drop heard, then off), rejects it: its sound pushes the
//! picks away. A fast skip ([`NOT_NOW_MS`]) of a long-loved track in a
//! stream is «not now»: a strong turn of the session (it counts twice in a
//! run of skips) and nothing against the track in the long run; a rejected
//! start of an unfamiliar track «did not land»: a light push of its sound
//! ([`MISSED_PUSH`]). A skip past the half, the bar untouched, says
//! neither. A start whose length nobody knows is rejected only by an early
//! skip (or a scrub).

use std::collections::{HashMap, VecDeque};

use nikgob_core::model::{EventContext, EventOrigin, TrackId, UnixMs};
use serde::{Deserialize, Serialize};

use crate::analysis::{DIM, Vector};
use crate::songtype::SongLane;
use crate::taste::SESSION_GAP_MS;

/// Tracks a session remembers.
pub const SESSION_LEN: usize = 20;
/// The anchor is the last this many approved tracks…
const ANCHOR_TRACKS: usize = 3;
/// …each this much less than the one after it.
const ANCHOR_DECAY: f32 = 0.6;
/// Rejected starts whose sound pushes the picks away.
const PUSHES: usize = 6;
/// The lane of a session is the commonest of this many last tracks.
const LANE_TRACKS: usize = 5;
/// A skip at this share of a track's length or later, the bar untouched,
/// approves it (its outro)…
pub const APPROVED_SHARE: f32 = 0.8;
/// …and one before this share rejects it.
pub const REJECTED_SHARE: f32 = 0.5;
/// A skip this soon of a long-loved track in a stream: «not now».
pub const NOT_NOW_MS: u32 = 10_000;
/// The push of a track that did not land, against a rejection's 1.
pub const MISSED_PUSH: f32 = 0.5;
/// A seek this far into a track or farther drags the bar (back to its
/// start is a restart).
pub const SCRUB_FROM_MS: u32 = 5_000;

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
    /// Where it ended, when known.
    #[serde(default)]
    pub position_ms: Option<u32>,
    /// Its length, when the one who ended it knew it.
    #[serde(default)]
    pub duration_ms: Option<u32>,
    /// The bar was dragged into it while it played.
    #[serde(default)]
    pub scrubbed: bool,
    /// Liked while it played (or after, in the session).
    #[serde(default)]
    pub liked: bool,
}

/// What a start says of its track (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Played to the end or to its outro, or liked.
    Approved,
    /// Still playing (or the app closed on it): nothing yet.
    Playing,
    /// Skipped past its half, the bar untouched.
    Neutral,
    /// Skipped in its first half, or after the bar was dragged.
    Rejected,
    /// A fast skip of a long-loved track in a stream.
    NotNow,
    /// A rejected start of an unfamiliar track.
    Missed,
}

impl Verdict {
    /// It turns the picks away.
    pub fn rejects(self) -> bool {
        matches!(self, Self::Rejected | Self::NotNow | Self::Missed)
    }

    /// How hard its sound pushes the picks away.
    pub fn push(self) -> f32 {
        match self {
            Self::Rejected | Self::NotNow => 1.0,
            Self::Missed => MISSED_PUSH,
            Self::Approved | Self::Playing | Self::Neutral => 0.0,
        }
    }

    /// What it counts in a run of skips: «not now» turns the session as
    /// two skips do.
    fn streak(self) -> u8 {
        match self {
            Self::NotNow => 2,
            Self::Rejected | Self::Missed => 1,
            Self::Approved | Self::Playing | Self::Neutral => 0,
        }
    }
}

/// What an end says of its track, whoever it is: approved, playing,
/// neutral or rejected ([`Session::verdict`] tells the rejections apart).
/// A skip of a known length is judged by its share, however soon it came;
/// of an unknown one, an early skip rejects and a later one says nothing.
pub fn judge(ended: Ended, position_ms: Option<u32>, duration_ms: Option<u32>, scrubbed: bool) -> Verdict {
    let share = match (position_ms, duration_ms) {
        (Some(at), Some(d)) if d > 0 => Some(at as f32 / d as f32),
        _ => None,
    };
    let rejected = match ended {
        Ended::Complete => return Verdict::Approved,
        Ended::Open => return Verdict::Playing,
        Ended::Early | Ended::Late if scrubbed => true,
        Ended::Early | Ended::Late => match share {
            Some(s) if s >= APPROVED_SHARE => return Verdict::Approved,
            Some(s) => s < REJECTED_SHARE,
            None => ended == Ended::Early,
        },
    };
    if rejected { Verdict::Rejected } else { Verdict::Neutral }
}

/// A skip at `position_ms` of a track `duration_ms` long (when known):
/// early (a skip in its first 30 s) unless it came at the outro of a short
/// track ([`APPROVED_SHARE`] of it heard).
pub fn skip_ended(position_ms: u32, duration_ms: Option<u32>) -> Ended {
    let outro = duration_ms.is_some_and(|d| d > 0 && position_ms as f32 >= APPROVED_SHARE * d as f32);
    if position_ms < crate::taste::EARLY_SKIP_MS && !outro { Ended::Early } else { Ended::Late }
}

/// A start a radio, autoplay or a mix chose (not the user's queue).
pub fn in_stream(context: EventContext, origin: Option<EventOrigin>) -> bool {
    context == EventContext::Radio || matches!(origin, Some(EventOrigin::Radio | EventOrigin::Autoplay | EventOrigin::Mix))
}

/// How well the user knows a track ([`TrackLookup::familiarity`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Familiarity {
    /// Liked, or loved for long.
    Loved,
    #[default]
    Known,
    /// Never heard before (or this once).
    Unfamiliar,
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
    /// Its length.
    fn duration(&self, _id: TrackId) -> Option<u32> {
        None
    }
    /// How well the user knows it.
    fn familiarity(&self, _id: TrackId) -> Familiarity {
        Familiarity::Known
    }
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
        self.tracks.push_back(Played {
            track,
            ts,
            ended: Ended::Open,
            context,
            origin,
            position_ms: None,
            duration_ms: None,
            scrubbed: false,
            liked: false,
        });
        while self.tracks.len() > SESSION_LEN {
            self.tracks.pop_front();
        }
        new
    }

    /// How the last start of `track` ended.
    pub fn end(&mut self, track: TrackId, ended: Ended) {
        if let Some(p) = self.open_mut(track) {
            p.ended = ended;
        }
    }

    /// How the last start of `track` ended, where (`position_ms`), its
    /// length when known, and whether the bar was dragged (besides what
    /// [`Self::seek`] said).
    pub fn end_at(&mut self, track: TrackId, ended: Ended, position_ms: u32, duration_ms: Option<u32>, scrubbed: bool) {
        if let Some(p) = self.open_mut(track) {
            p.ended = ended;
            p.position_ms = Some(position_ms);
            p.duration_ms = duration_ms.filter(|d| *d > 0).or(p.duration_ms);
            p.scrubbed |= scrubbed;
        }
    }

    /// The bar was dragged to `to_ms` while `track` played: a scrub when
    /// it went into the track ([`SCRUB_FROM_MS`]).
    pub fn seek(&mut self, track: TrackId, to_ms: u32) {
        if to_ms < SCRUB_FROM_MS {
            return;
        }
        if let Some(p) = self.open_mut(track) {
            p.scrubbed = true;
        }
    }

    /// `track` was liked: its last start in the session is approved.
    pub fn like(&mut self, track: TrackId) {
        if let Some(p) = self.tracks.iter_mut().rev().find(|p| p.track == track) {
            p.liked = true;
        }
    }

    fn open_mut(&mut self, track: TrackId) -> Option<&mut Played> {
        self.tracks.iter_mut().rev().find(|p| p.track == track && p.ended == Ended::Open)
    }

    /// The last start of `track` in the session.
    pub fn last_of(&self, track: TrackId) -> Option<&Played> {
        self.tracks.iter().rev().find(|p| p.track == track)
    }

    /// What the start `p` says (see the module docs).
    pub fn verdict(p: &Played, look: &impl TrackLookup) -> Verdict {
        if p.liked {
            return Verdict::Approved;
        }
        let v = judge(p.ended, p.position_ms, p.duration_ms.or_else(|| look.duration(p.track)), p.scrubbed);
        if v != Verdict::Rejected {
            return v;
        }
        let stream = in_stream(p.context, p.origin);
        match look.familiarity(p.track) {
            Familiarity::Loved if stream && !p.scrubbed && p.position_ms.is_some_and(|x| x < NOT_NOW_MS) => Verdict::NotNow,
            Familiarity::Unfamiliar => Verdict::Missed,
            _ => Verdict::Rejected,
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

    /// Rejections running at the end (a «not now» counts twice).
    pub fn skip_streak(&self, look: &impl TrackLookup) -> u8 {
        let mut n = 0u8;
        for p in self.tracks.iter().rev() {
            let v = Self::verdict(p, look);
            if !v.rejects() {
                break;
            }
            n = n.saturating_add(v.streak());
        }
        n
    }

    /// The last start rejects its track.
    pub fn last_rejected(&self, look: &impl TrackLookup) -> bool {
        self.last().is_some_and(|p| Self::verdict(p, look).rejects())
    }

    /// The last `k` tracks of the session not rejected, oldest first.
    pub fn kept(&self, look: &impl TrackLookup, k: usize) -> Vec<TrackId> {
        let mut out: Vec<TrackId> = self.tracks.iter().rev().filter(|p| !Self::verdict(p, look).rejects()).map(|p| p.track).take(k).collect();
        out.reverse();
        out
    }

    /// What the start `p` teaches the early-skip model, which lives on past
    /// the session: a complete «not skipped», an early skip «skipped» unless
    /// it was «not now» (a mood, not the track); anything else nothing.
    pub fn skip_lesson(p: &Played, look: &impl TrackLookup) -> Option<bool> {
        match p.ended {
            Ended::Complete => Some(false),
            Ended::Early if Self::verdict(p, look) == Verdict::NotNow => None,
            Ended::Early => Some(true),
            Ended::Late | Ended::Open => None,
        }
    }

    /// The start `p` was not skipped: it played through (to the end or its
    /// outro), was liked, or plays still. A skip past the half was a skip.
    /// What the session goes on from, latest first: what it approved
    /// (played through or liked) and the song playing now; a song skipped,
    /// early or past its half, leads nowhere.
    pub fn seeds<'a>(&'a self, look: &'a impl TrackLookup) -> impl Iterator<Item = &'a Played> + 'a {
        let last = self.tracks.len().saturating_sub(1);
        self.tracks.iter().enumerate().rev().filter(move |(k, p)| match Self::verdict(p, look) {
            Verdict::Approved => true,
            Verdict::Playing => *k == last && p.ended == Ended::Open,
            _ => false,
        }).map(|(_, p)| p)
    }

    pub fn played_through(p: &Played, look: &impl TrackLookup) -> bool {
        matches!(Self::verdict(p, look), Verdict::Approved | Verdict::Playing)
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

    /// The sound to continue: the last [`ANCHOR_TRACKS`] approved, weighted
    /// 1, 0.6, 0.36 from the last; none before one was approved.
    pub fn anchor(&self, look: &impl TrackLookup) -> Option<Vector> {
        self.anchor_of(look, ANCHOR_TRACKS)
    }

    /// [`Self::anchor`] of the last `n` approved.
    pub fn anchor_of(&self, look: &impl TrackLookup, n: usize) -> Option<Vector> {
        let mut sum = [0.0f32; DIM];
        let (mut w_total, mut w, mut k) = (0.0, 1.0, 0);
        for p in self.tracks.iter().rev() {
            if Self::verdict(p, look) != Verdict::Approved {
                continue;
            }
            let Some(v) = look.vector(p.track) else { continue };
            for (s, x) in sum.iter_mut().zip(v) {
                *s += w * x;
            }
            w_total += w;
            w *= ANCHOR_DECAY;
            k += 1;
            if k == n {
                break;
            }
        }
        (w_total > 0.0).then(|| sum.map(|s| s / w_total))
    }

    /// The sounds the session rejected and how hard each pushes (the last
    /// [`PUSHES`]).
    pub fn pushes(&self, look: &impl TrackLookup) -> Vec<(Vector, f32)> {
        let all: Vec<(Vector, f32)> = self
            .tracks
            .iter()
            .filter_map(|p| {
                let v = Self::verdict(p, look);
                if !v.rejects() {
                    return None;
                }
                Some((look.vector(p.track)?, v.push()))
            })
            .collect();
        all[all.len().saturating_sub(PUSHES)..].to_vec()
    }

    /// The tracks of [`Self::pushes`] (those with a sound) and how hard each
    /// pushes: a sound index measures them by both sounds
    /// ([`crate::UserModel::push_probe`]).
    pub fn pushed(&self, look: &impl TrackLookup) -> Vec<(TrackId, f32)> {
        let all: Vec<(TrackId, f32)> = self
            .tracks
            .iter()
            .filter_map(|p| {
                let v = Self::verdict(p, look);
                if !v.rejects() {
                    return None;
                }
                look.vector(p.track)?;
                Some((p.track, v.push()))
            })
            .collect();
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

    /// What the session is: pure in one taste profile
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
        /// Every track lasts 200 s.
        fn duration(&self, _id: TrackId) -> Option<u32> {
            Some(200_000)
        }
        /// 900s are loved, 800s unfamiliar.
        fn familiarity(&self, id: TrackId) -> Familiarity {
            match id.0.as_u128() / 100 {
                9 => Familiarity::Loved,
                8 => Familiarity::Unfamiliar,
                _ => Familiarity::Known,
            }
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
        assert_eq!(s.skip_streak(&Look), 2);
        s.start(tid(3), T + 20_000, EventContext::Playlist, None);
        assert_eq!(s.skip_streak(&Look), 0, "the third still plays");
        assert_eq!(s.pushes(&Look), vec![(line(1.0), 1.0), (line(2.0), 1.0)]);
        assert_eq!(s.anchor(&Look), None, "skips are no anchor, nor the track still playing");
        s.end(tid(3), Ended::Complete);
        assert_eq!(s.anchor(&Look), Some(line(3.0)));
        assert!(s.start(tid(4), T + 20_000 + SESSION_GAP_MS, EventContext::Album, None));
        assert_eq!(s.len(), 1);
    }

    /// Only approved starts make the anchor: a skip past 30 s in the first
    /// half pushes, a scrub-skip pushes even late, the outro and a like
    /// approve, a skip past the half says nothing.
    #[test]
    fn the_anchor_takes_only_approved_tracks() {
        let mut s = Session::default();
        let mut ts = T;
        let mut play = |s: &mut Session, t: u128, ended: Ended, at: u32, scrub: bool| {
            ts += 200_000;
            s.start(tid(t), ts, EventContext::Playlist, None);
            if scrub {
                s.seek(tid(t), 70_000);
            }
            s.end_at(tid(t), ended, at, None, false);
        };
        play(&mut s, 10, Ended::Complete, 200_000, false);
        play(&mut s, 20, Ended::Late, 45_000, false);
        assert_eq!(s.anchor(&Look), Some(line(10.0)), "a skip at 45 s of 200 is no anchor");
        assert_eq!(s.pushes(&Look), vec![(line(20.0), 1.0)], "and it pushes");
        play(&mut s, 30, Ended::Late, 170_000, false);
        assert_eq!(s.anchor_of(&Look, 1), Some(line(30.0)), "the outro approves");
        play(&mut s, 40, Ended::Late, 170_000, true);
        assert_eq!(s.anchor_of(&Look, 1), Some(line(30.0)), "a scrub-skip does not");
        assert!(s.last_rejected(&Look));
        play(&mut s, 50, Ended::Late, 120_000, false);
        assert_eq!(Session::verdict(s.last().unwrap(), &Look), Verdict::Neutral);
        assert_eq!(s.skip_streak(&Look), 0, "a neutral skip ends the run");
        assert_eq!(s.anchor_of(&Look, 1), Some(line(30.0)));
        play(&mut s, 60, Ended::Early, 5_000, false);
        s.like(tid(60));
        assert_eq!(s.anchor_of(&Look, 1), Some(line(60.0)), "a like approves");
        assert_eq!(s.kept(&Look, 3), vec![tid(30), tid(50), tid(60)]);
        // A seek back to the start is a restart, not a scrub.
        s.start(tid(70), ts + 400_000, EventContext::Playlist, None);
        s.seek(tid(70), 0);
        s.end_at(tid(70), Ended::Late, 190_000, None, false);
        assert_eq!(Session::verdict(s.last().unwrap(), &Look), Verdict::Approved);
    }

    /// A short track skipped at its outro, before 30 s, is approved; the
    /// skip is not early either.
    #[test]
    fn a_short_tracks_outro_approves() {
        assert_eq!(skip_ended(17_000, Some(20_000)), Ended::Late);
        assert_eq!(skip_ended(17_000, Some(200_000)), Ended::Early);
        assert_eq!(skip_ended(17_000, None), Ended::Early);
        assert_eq!(judge(Ended::Early, Some(17_000), Some(20_000), false), Verdict::Approved, "an old label, judged by its share");
        assert_eq!(judge(Ended::Early, Some(12_000), Some(20_000), false), Verdict::Neutral);
        assert_eq!(judge(Ended::Early, Some(17_000), Some(20_000), true), Verdict::Rejected, "a scrub-skip");
        assert_eq!(judge(Ended::Early, Some(17_000), None, false), Verdict::Rejected, "length unknown: early rejects");
        assert_eq!(judge(Ended::Open, None, None, false), Verdict::Playing);
    }

    /// A fast skip of a loved track in a stream is «not now» (two in the
    /// run); a skip of an unfamiliar one pushes half as hard.
    #[test]
    fn not_now_and_did_not_land() {
        let mut s = Session::default();
        s.start(tid(900), T, EventContext::Radio, Some(EventOrigin::Autoplay));
        s.end_at(tid(900), Ended::Early, 4_000, None, false);
        assert_eq!(Session::verdict(s.last().unwrap(), &Look), Verdict::NotNow);
        assert_eq!(s.skip_streak(&Look), 2, "a strong turn");
        s.start(tid(800), T + 10_000, EventContext::Radio, Some(EventOrigin::Autoplay));
        s.end_at(tid(800), Ended::Early, 15_000, None, false);
        assert_eq!(s.pushes(&Look), vec![(line(900.0), 1.0), (line(800.0), MISSED_PUSH)]);
        assert_eq!(s.skip_streak(&Look), 3);
        assert_eq!(Session::skip_lesson(s.last_of(tid(900)).unwrap(), &Look), None, "«not now» teaches no skip");
        assert_eq!(Session::skip_lesson(s.last_of(tid(800)).unwrap(), &Look), Some(true));
        // The same fast skip from the user's own playlist is a rejection.
        let mut own = Session::default();
        own.start(tid(901), T, EventContext::Playlist, Some(EventOrigin::Manual));
        own.end_at(tid(901), Ended::Early, 4_000, None, false);
        assert_eq!(Session::verdict(own.last().unwrap(), &Look), Verdict::Rejected);
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
        assert_eq!(s.anchor(&Look), None, "nothing heard out yet");
        for t in [11, 12, 130, 140] {
            s.end(tid(t), Ended::Complete);
        }
        let a = s.anchor(&Look).unwrap()[0];
        let want = (140.0 + 0.6 * 130.0 + 0.36 * 12.0) / (1.0 + 0.6 + 0.36);
        assert!((a - want).abs() < 1e-3, "{a}");
    }
}
