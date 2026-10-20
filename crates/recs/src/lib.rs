//! Recommendations (docs/stage4.md). Pure logic: no database, no network,
//! no decoding, no clock; the runtime feeds it samples, library rows and
//! `now`, and every selection is a function of its inputs and a seed.
//!
//! * [`analysis`]: bliss features of a track from its decoded PCM (mono
//!   22 050 Hz, the first [`MAX_SECONDS`]), tagged [`ANALYZED_WITH`].
//! * [`index`]: every vector of the library in memory, z-normalised over the
//!   library itself, with brute-force nearest neighbours.
//! * [`taste`]: the [`Catalog`] of pickable tracks and the [`Taste`] built
//!   from the event log, likes, playlists and dislikes.
//! * [`pick`]: the engine context [`Recs`], [`Pick`]s with [`Reason`]s,
//!   diversity and the smooth order shared by every selection.
//! * [`daily`]: «Твой микс»; [`mood`]: the six moods; [`style`]: «Твои
//!   миксы»; [`radio`]: endless selections (track, artist, mood, style,
//!   discoveries) with session learning; [`discover`]: «Новое для тебя» and
//!   new releases.

pub mod analysis;
pub mod daily;
pub mod discover;
pub mod index;
pub mod mood;
pub mod pick;
pub mod radio;
pub mod rng;
pub mod style;
pub mod taste;

pub use analysis::{ANALYZED_WITH, AnalysisError, DIM, MAX_SECONDS, PcmAnalyzer, SAMPLE_RATE, Vector, analyze_pcm};
pub use daily::DailyParams;
pub use discover::{
    Discovery, DiscoveryFilter, DiscoverySeeds, NewRelease, SeedFinds, SeedRef, album_key, artist_key, rank_discoveries,
    song_key, stream_song_key,
};
pub use index::{Normalizer, VectorIndex};
pub use mood::{Axes, Mood};
pub use pick::{Item, NewTrack, Pick, Reason, Recs};
pub use radio::{Outcome, RadioParams, RadioSeed, RadioState};
pub use style::StyleMix;
pub use taste::{Catalog, DayPart, Play, Taste, TasteInput, TrackInfo};

#[cfg(test)]
pub(crate) mod test_util {
    use nikgob_core::model::{ArtistId, EventType, ReleaseId, TrackId, UnixMs};
    use uuid::Uuid;

    use crate::analysis::{DIM, Vector};
    use crate::rng::Rng;
    use crate::taste::{Catalog, DAY_MS, Play, TasteInput, TrackInfo};

    pub fn tid(i: u128) -> TrackId {
        TrackId(Uuid::from_u128(i))
    }

    pub fn aid(i: u32) -> ArtistId {
        ArtistId(Uuid::from_u128(u128::from(i)))
    }

    pub fn rid(i: u128) -> ReleaseId {
        ReleaseId(Uuid::from_u128(i))
    }

    pub fn info(i: u128, artists: &[u32], release: Option<u128>) -> TrackInfo {
        TrackInfo::new(tid(i), artists.iter().map(|a| aid(*a)).collect(), release.map(rid))
    }

    /// A vector on the first feature only.
    pub fn line(x: f32) -> Vector {
        let mut v = [0.0; DIM];
        v[0] = x;
        v
    }

    /// A vector on the first two features.
    pub fn point(x: f32, y: f32) -> Vector {
        let mut v = [0.0; DIM];
        v[0] = x;
        v[1] = y;
        v
    }

    /// `tracks` tracks by `artists` artists and `events` random plays over
    /// two years.
    pub fn big_history(tracks: u128, events: usize, artists: u32) -> (Catalog, TasteInput, UnixMs) {
        let now = 20_000 * DAY_MS;
        let cat = Catalog::new((0..tracks).map(|i| info(i, &[(i % u128::from(artists)) as u32], Some(i / 10))));
        let mut rng = Rng::new(42);
        let kinds = [EventType::Start, EventType::Complete, EventType::Skip];
        let events = (0..events)
            .map(|_| Play {
                track: tid(rng.below(tracks as usize) as u128),
                ts: now - (rng.f64() * 730.0 * DAY_MS as f64) as i64,
                kind: kinds[rng.below(3)],
                position_ms: Some(rng.below(300_000) as u32),
            })
            .collect();
        let likes = (0..2_000).map(|_| tid(rng.below(tracks as usize) as u128)).collect();
        (cat, TasteInput { events, likes, ..Default::default() }, now)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::test_util::*;
    use crate::*;

    /// The whole engine over a library of 50 000 tracks and 100 000 events:
    /// no pass is quadratic in the library or the history.
    #[test]
    fn everything_over_a_big_library_is_quick() {
        let (cat, input, now) = big_history(50_000, 100_000, 5_000);
        let mut rng = rng::Rng::new(1);
        let mut idx = VectorIndex::new();
        for t in cat.tracks() {
            idx.insert(t.id, std::array::from_fn(|_| rng.f32()));
        }
        let start = std::time::Instant::now();
        let taste = Taste::build(&cat, &input, now);
        let recs = Recs::new(&cat, &taste, &idx);
        let daily = recs.daily_mix(&[], &DailyParams::default());
        assert_eq!(daily.len(), 30);
        for m in Mood::ALL {
            let mut r = RadioState::new(RadioSeed::Mood(m));
            assert_eq!(r.extend(&recs, 10, &RadioParams { new_share: 0.0, seed: 1 }).len(), 10);
        }
        let mut r = RadioState::new(RadioSeed::Track(tid(7)));
        assert_eq!(r.extend(&recs, 10, &RadioParams { new_share: 0.0, seed: 1 }).len(), 10);
        let mut r = RadioState::new(RadioSeed::Artist(aid(7)));
        assert_eq!(r.extend(&recs, 10, &RadioParams { new_share: 0.0, seed: 1 }).len(), 10);
        let styles = recs.style_mixes(&[], 1);
        let seeds = recs.discovery_seeds();
        assert_eq!(seeds.tracks.len(), 10);
        let took = start.elapsed();
        let unique: HashSet<&Item> = daily.iter().map(|p| &p.item).collect();
        assert_eq!(unique.len(), 30);
        let _ = styles;
        assert!(took < std::time::Duration::from_secs(10), "{took:?}");
        eprintln!("everything over 50k tracks / 100k events: {took:?}");
    }
}
