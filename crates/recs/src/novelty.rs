//! How new music is received (recs-v2 §4.7, §11.4, §11.5): every new track
//! of a service that started from a mix, a radio or «Новое для тебя» and
//! how it ended, kept as a short log ([`NewLog`], the library's cache keeps
//! it) and read four ways.
//!
//! * **Taste profiles** (profiles-v1 §8). Each event keeps the key of the
//!   profile it was heard for ([`NewEvent::profile`]; a log of an older
//!   [`NOVELTY_VERSION`] has them told again from its sounds,
//!   [`NewLog::upgrade`]). A complete is `α + 1` of its profile, an early
//!   skip `β + 1`, from `Beta(2, 14)` (about an eighth new); a background
//!   profile starts from `Beta(2, 18)` and a song heard out in the
//!   background counts half (heard out, not listened to). The share of new
//!   music of a mix is drawn from it (Thompson), between [`MIN_SHARE`] and
//!   [`MAX_SHARE`] (every fifth at most, as «С новым» always said): it
//!   grows where the new is heard out and shrinks where it is skipped.
//! * **Sources.** The same counts per source of discovery, `Beta(completes
//!   + 1, early skips + 1)`; a pass gives each source the share of its
//!   requests its draw says ([`NewLog::source_shares`]).
//! * **Artists.** An early skip of a new song puts its artist
//!   [`ARTIST_PENALTY`] lower for a week; two in a month keep the artist
//!   out of new music for the month.
//! * **«Оставить у себя».** A new song played to the end twice (or liked)
//!   is offered once to be kept: a like, which downloads it.

use std::collections::{BTreeMap, HashSet};

use nikgob_core::model::{TrackId, UnixMs};
use serde::{Deserialize, Serialize};

use crate::analysis::Vector;
use crate::rng::Rng;
use crate::taste::DAY_MS;

/// The prior of a profile's appetite: about an eighth new…
pub const PRIOR: (f32, f32) = (2.0, 14.0);
/// …of a background profile's, about a tenth; a song heard out in the
/// background counts this much.
pub const BG_PRIOR: (f32, f32) = (2.0, 18.0);
pub const BG_COMPLETE: f32 = 0.5;
/// The log keyed by the taste profiles.
pub const NOVELTY_VERSION: u32 = 1;
pub const MIN_SHARE: f32 = 0.05;
pub const MAX_SHARE: f32 = 0.2;
/// An early skip of a new song: its artist this much lower…
pub const ARTIST_PENALTY: f32 = 0.5;
/// …for this long;
pub const PENALTY_MS: i64 = 7 * DAY_MS;
/// two of them within this long keep the artist out.
pub const BAN_MS: i64 = 30 * DAY_MS;
/// Completes of a new song before it is offered to keep.
pub const KEEP_AFTER: u32 = 2;
/// Events kept.
const LOG_CAP: usize = 2_000;

/// Where a discovery came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Source {
    /// YouTube Music's radio of a seed song.
    YoutubeRadio,
    /// The similar artists of Deezer or YouTube Music and their tops,
    /// Deezer's related lists.
    Similar,
    /// A song a service suggests itself.
    Service,
    /// Another album of a world's composer or franchise.
    Doppelganger,
}

pub const SOURCES: [Source; 4] = [Source::YoutubeRadio, Source::Similar, Source::Service, Source::Doppelganger];

/// How a start of a new song ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Heard {
    Complete,
    Early,
    Liked,
}

/// One start of a new song that ended (or a like of one).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NewEvent {
    /// [`crate::song_key`] and [`crate::artist_key`].
    pub song: String,
    pub artist: String,
    pub source: Option<Source>,
    /// Its sound (an analysed preview, embedded in the model's space), for
    /// its region.
    pub vector: Option<Vector>,
    /// The track it played as (a seed of the next pass when heard out).
    pub track: Option<TrackId>,
    pub at: UnixMs,
    pub heard: Heard,
    /// The key of the taste profile it was heard for
    /// ([`crate::profiles::Profile::key`]).
    #[serde(default)]
    pub profile: Option<u32>,
    /// It ended in a background session.
    #[serde(default)]
    pub background: bool,
}

/// The log of new music heard (see the module docs).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NewLog {
    pub events: Vec<NewEvent>,
    /// Songs offered to keep (never twice).
    #[serde(default)]
    pub offered: HashSet<String>,
    /// [`NOVELTY_VERSION`] of its keys.
    #[serde(default)]
    pub version: u32,
}

/// A draw of `Beta(a, b)` (two gammas, Marsaglia and Tsang).
pub fn beta(rng: &mut Rng, a: f32, b: f32) -> f32 {
    let x = gamma(rng, f64::from(a.max(1e-3)));
    let y = gamma(rng, f64::from(b.max(1e-3)));
    if x + y <= 0.0 { 0.5 } else { (x / (x + y)) as f32 }
}

fn normal(rng: &mut Rng) -> f64 {
    let u = rng.f64().max(1e-12);
    let v = rng.f64();
    (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
}

fn gamma(rng: &mut Rng, k: f64) -> f64 {
    if k < 1.0 {
        let u = rng.f64().max(1e-12);
        return gamma(rng, k + 1.0) * u.powf(1.0 / k);
    }
    let d = k - 1.0 / 3.0;
    let c = 1.0 / (9.0 * d).sqrt();
    loop {
        let x = normal(rng);
        let v = (1.0 + c * x).powi(3);
        if v <= 0.0 {
            continue;
        }
        let u = rng.f64();
        if u.ln() < 0.5 * x * x + d - d * v + d * v.ln() {
            return d * v;
        }
    }
}

impl NewLog {
    pub fn push(&mut self, e: NewEvent) {
        self.events.push(e);
        if self.events.len() > LOG_CAP {
            let over = self.events.len() - LOG_CAP;
            self.events.drain(..over);
        }
    }

    /// Keys a log of an older [`NOVELTY_VERSION`] by the taste profiles:
    /// each event without a profile gets the one `key_of` tells from its
    /// sound or track. Returns whether it changed.
    pub fn upgrade(&mut self, key_of: impl Fn(&NewEvent) -> Option<u32>) -> bool {
        if self.version >= NOVELTY_VERSION {
            return false;
        }
        for e in &mut self.events {
            if e.profile.is_none() {
                e.profile = key_of(e);
            }
        }
        self.version = NOVELTY_VERSION;
        true
    }

    /// The appetite `Beta(α, β)` of the profile keyed `profile` (`None`:
    /// the whole taste), a background one's (`background`) from its own
    /// prior.
    pub fn appetite(&self, profile: Option<u32>, background: bool) -> (f32, f32) {
        let mut ab = if background { BG_PRIOR } else { PRIOR };
        for e in self.events.iter().filter(|e| profile.is_none() || e.profile == profile) {
            match e.heard {
                Heard::Complete => ab.0 += if e.background { BG_COMPLETE } else { 1.0 },
                Heard::Liked => ab.0 += 1.0,
                Heard::Early => ab.1 += 1.0,
            }
        }
        ab
    }

    /// The share of new music of a mix of the profile keyed `profile`
    /// (`None`: the whole taste), drawn by Thompson from its appetite.
    pub fn share(&self, profile: Option<u32>, background: bool, rng: &mut Rng) -> f32 {
        let (a, b) = self.appetite(profile, background);
        beta(rng, a, b).clamp(MIN_SHARE, MAX_SHARE)
    }

    /// `Beta(completes + 1, early skips + 1)` of each source.
    pub fn sources(&self) -> BTreeMap<Source, (f32, f32)> {
        let mut out: BTreeMap<Source, (f32, f32)> = SOURCES.iter().map(|s| (*s, (1.0, 1.0))).collect();
        for e in &self.events {
            let Some(s) = e.source else { continue };
            let ab = out.entry(s).or_insert((1.0, 1.0));
            match e.heard {
                Heard::Complete | Heard::Liked => ab.0 += 1.0,
                Heard::Early => ab.1 += 1.0,
            }
        }
        out
    }

    /// The share of a pass's requests of each source: one draw of each
    /// source's Beta, normalised, at least a tenth each (no source is ever
    /// left untried).
    pub fn source_shares(&self, rng: &mut Rng) -> BTreeMap<Source, f32> {
        let draws: Vec<(Source, f32)> = self.sources().into_iter().map(|(s, (a, b))| (s, beta(rng, a, b))).collect();
        let total: f32 = draws.iter().map(|d| d.1).sum::<f32>().max(1e-6);
        let floor = 0.1;
        let n = draws.len() as f32;
        draws.into_iter().map(|(s, d)| (s, floor + (1.0 - floor * n) * d / total)).collect()
    }

    /// How much lower `artist` stands in new music at `now`: −∞ while kept
    /// out, [`ARTIST_PENALTY`] for a week after an early skip, else 0.
    pub fn artist_penalty(&self, artist: &str, now: UnixMs) -> f32 {
        let early: Vec<UnixMs> = self
            .events
            .iter()
            .filter(|e| e.heard == Heard::Early && e.artist == artist && now - e.at < BAN_MS)
            .map(|e| e.at)
            .collect();
        if early.len() >= 2 {
            return f32::INFINITY;
        }
        if early.iter().any(|t| now - t < PENALTY_MS) { ARTIST_PENALTY } else { 0.0 }
    }

    /// Times `song` was played to the end.
    pub fn completes(&self, song: &str) -> u32 {
        self.events.iter().filter(|e| e.song == song && e.heard == Heard::Complete).count() as u32
    }

    /// `song` is to be offered to keep now: played to the end
    /// [`KEEP_AFTER`] times, or liked, and never offered.
    pub fn keep_offer_due(&self, song: &str) -> bool {
        !self.offered.contains(song)
            && (self.completes(song) >= KEEP_AFTER || self.events.iter().any(|e| e.song == song && e.heard == Heard::Liked))
    }

    /// New songs heard out lately, the most recent first (seeds of the next
    /// pass).
    pub fn heard_out(&self, n: usize) -> Vec<TrackId> {
        let mut out: Vec<TrackId> = Vec::new();
        for e in self.events.iter().rev() {
            if e.heard == Heard::Complete
                && let Some(t) = e.track
                && !out.contains(&t)
            {
                out.push(t);
                if out.len() == n {
                    break;
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::point;

    fn ev(song: &str, artist: &str, source: Option<Source>, v: Option<Vector>, at: UnixMs, heard: Heard) -> NewEvent {
        NewEvent { song: song.into(), artist: artist.into(), source, vector: v, track: None, at, heard, profile: None, background: false }
    }

    /// Thompson over the sources: the one heard out wins most passes'
    /// budget, the skipped one keeps its floor.
    #[test]
    fn prefers_source_with_completes() {
        let mut log = NewLog::default();
        for k in 0..12 {
            log.push(ev(&format!("a{k}"), "x", Some(Source::YoutubeRadio), None, 0, Heard::Complete));
            log.push(ev(&format!("b{k}"), "y", Some(Source::Similar), None, 0, Heard::Early));
        }
        let mut rng = Rng::new(9);
        let mut wins = 0;
        for _ in 0..200 {
            let s = log.source_shares(&mut rng);
            assert!(s.values().all(|v| *v >= 0.1 - 1e-6));
            assert!((s.values().sum::<f32>() - 1.0).abs() < 1e-3);
            if s[&Source::YoutubeRadio] > s[&Source::Similar] {
                wins += 1;
            }
        }
        assert!(wins > 190, "{wins}");
    }

    /// A profile heard out takes more new music than one skipped, both
    /// within the bounds; a background profile starts lower and counts a
    /// song heard out in the background half; an old log is keyed from its
    /// sounds once; a skipping artist rests a week, two skips keep it out a
    /// month.
    #[test]
    fn appetite_and_artists_follow_reactions() {
        let mut log = NewLog::default();
        for k in 0..10 {
            log.push(ev(&format!("a{k}"), "a", None, Some(point(0.1, 0.0)), 0, Heard::Complete));
            log.push(ev(&format!("b{k}"), "b", None, Some(point(10.1, 0.0)), 0, Heard::Early));
        }
        // An old log: its events keyed by their sound, once.
        assert!(log.upgrade(|e| e.vector.map(|v| if v[0] < 5.0 { 1 } else { 2 })));
        assert!(!log.upgrade(|_| Some(9)));
        assert_eq!(log.events.iter().filter(|e| e.profile == Some(1)).count(), 10);
        let mut rng = Rng::new(1);
        let mean = |p: u32, bg: bool, rng: &mut Rng| (0..300).map(|_| log.share(Some(p), bg, rng)).sum::<f32>() / 300.0;
        let (a, b) = (mean(1, false, &mut rng), mean(2, false, &mut rng));
        assert!(a > b + 0.05, "{a} {b}");
        assert!(a <= MAX_SHARE + 1e-4 && b >= MIN_SHARE - 1e-4, "{a} {b}");
        let fresh = NewLog::default();
        let start = (0..500).map(|_| fresh.share(Some(1), false, &mut rng)).sum::<f32>() / 500.0;
        assert!((0.1..=0.15).contains(&start), "about 10–15 % at first: {start}");
        let calm = (0..500).map(|_| fresh.share(Some(1), true, &mut rng)).sum::<f32>() / 500.0;
        assert!(calm < start, "a background profile takes less: {calm} {start}");
        let mut bg = NewLog::default();
        for k in 0..6 {
            bg.push(NewEvent { profile: Some(3), background: true, ..ev(&format!("c{k}"), "c", None, None, 0, Heard::Complete) });
        }
        assert_eq!(bg.appetite(Some(3), true), (BG_PRIOR.0 + 3.0, BG_PRIOR.1), "heard out in the background: half");

        let mut log = NewLog::default();
        log.push(ev("s1", "artist", None, None, 0, Heard::Early));
        assert_eq!(log.artist_penalty("artist", DAY_MS), ARTIST_PENALTY);
        assert_eq!(log.artist_penalty("artist", 8 * DAY_MS), 0.0);
        log.push(ev("s2", "artist", None, None, 10 * DAY_MS, Heard::Early));
        assert!(log.artist_penalty("artist", 11 * DAY_MS).is_infinite());
        assert_eq!(log.artist_penalty("artist", 45 * DAY_MS), 0.0, "a month later");
    }

    /// Two completes of a new song: offered to keep, once.
    #[test]
    fn keep_is_offered_after_two_completes() {
        let mut log = NewLog::default();
        log.push(ev("song", "a", None, None, 0, Heard::Complete));
        assert!(!log.keep_offer_due("song"));
        log.push(ev("song", "a", None, None, 1, Heard::Early));
        assert!(!log.keep_offer_due("song"));
        log.push(ev("song", "a", None, None, 2, Heard::Complete));
        assert!(log.keep_offer_due("song"));
        log.offered.insert("song".into());
        assert!(!log.keep_offer_due("song"));
        log.push(ev("liked", "b", None, None, 3, Heard::Liked));
        assert!(log.keep_offer_due("liked"));
    }
}
