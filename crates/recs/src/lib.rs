//! Recommendations. Pure logic: no database, no network,
//! no decoding, no clock; the runtime feeds it samples, library rows and
//! `now`, and every selection is a function of its inputs and a seed.
//!
//! * [`analysis`]: bliss features of a track from a few decoded windows of
//!   it (mono 22 050 Hz), tagged [`ANALYZED_WITH`]; [`embedding`]: the
//!   neural sound model's input and its compact vectors, tagged
//!   [`EMBEDDED_WITH`].
//! * [`index`]: every vector of the library in memory, z-normalised over the
//!   library itself, with brute-force nearest neighbours.
//! * [`taste`]: the [`Catalog`] of pickable tracks and the [`Taste`] built
//!   from the event log, likes, playlists and dislikes.
//! * [`pick`]: the engine context [`Recs`], [`Pick`]s with [`Reason`]s,
//!   diversity and the smooth order shared by every selection.
//! * [`daily`]: «Твой микс»; [`mood`]: the six moods; [`style`]: «Твои
//!   миксы»; [`radio`]: endless selections (track, artist, mood, style,
//!   discoveries, daylist) with session learning; [`discover`]: «Новое для
//!   тебя» and new releases.
//! * [`daylist`]: «Дейлист» of each part of the day with its name;
//!   [`tiles`]: the bento's smaller tiles, 21 kinds and the daily choice
//!   of them.
//! * [`suggest`]: the services' own suggestions as a signal of taste and
//!   as discovery votes.
//! * [`model`]: the user model — the history's stats of tracks,
//!   worlds and track pairs, the online models of the next start and of
//!   the early skip; [`session`]: the session they score in;
//!   [`songtype`]: sung or instrumental, the language, a soundtrack's song.
//! * [`world_mixes`]: the mixes of a world's page; [`explain`]: why a mix
//!   or a pick, in one line.
//! * [`open_world`]: new music beyond the library — the niches, their
//!   sound, the next of a wave.
//! * [`graph`]: the graph of similar music other listeners know (the
//!   services' radios and similar artists, ListenBrainz, Last.fm, the tags
//!   of MusicBrainz) and the user's own (an imported history, the
//!   playlists), and the prior of each track of the library.
//! * [`wave`]: «Моя волна» — its filters and the bandit of the user's
//!   clusters that leads it.

pub mod analysis;
pub mod background;
pub mod daily;
pub mod daylist;
pub mod discover;
pub mod dislike;
pub mod embedding;
pub mod eval;
pub mod explain;
pub mod graph;
pub mod index;
pub mod junk;
pub mod listening;
pub mod lyric_valence;
pub mod mix_names;
pub mod mix_new;
pub mod mixes;
pub mod model;
pub mod mood;
pub mod novelty;
pub mod open_world;
pub mod pick;
pub mod plus;
pub mod profiles;
pub mod radio;
pub mod regions;
pub mod rng;
pub mod session;
pub mod songtype;
pub mod style;
pub mod suggest;
pub mod taste;
pub mod tiles;
pub mod wave;
pub mod world_mixes;

pub use analysis::{
    ANALYZED_WITH, Analysed, AnalysisError, DIM, MAX_SECONDS, PcmAnalyzer, SAMPLE_RATE, Vector, Window, WindowedAnalyzer,
    analyze_pcm, analyze_windows, preview_windows, windows,
};
pub use embedding::{Compact, EMBEDDED_WITH, SoundModel};
pub use daily::DailyParams;
pub use daylist::{Centre, DAYLIST_LEN, daylist_title};
pub use discover::{
    Discovery, DiscoveryFilter, DiscoverySeeds, NewRelease, SeedFinds, SeedRef, album_key, artist_key, rank_discoveries,
    song_key, stream_song_key,
};
pub use graph::{Edge, GraphInput, GraphSource, SimGraph};
pub use index::{Normalizer, VectorIndex};
pub use junk::{Canonical, Junk, junk};
pub use novelty::{Heard, NewEvent, NewLog, Source};
pub use regions::Regions;
pub use model::{FEATURE_VERSION, HomeProfiles, Live, Model, ModelInput, Moment, SkipModel, UserModel, Weights};
pub use mood::{Axes, Mood};
pub use pick::{Item, NewTrack, Pick, Reason, Recs};
pub use radio::{NO_REPEAT_MS, Outcome, RadioParams, RadioSeed, RadioState, SeedFacts};
pub use session::{Ended, Familiarity, Session, TrackLookup, Verdict};
pub use songtype::{Lang, SONGTYPE_RULES, SongInput, SongLane, SongType, SongTypes, Voice, song_types};
pub use style::{StyleMix, style_words};
pub use suggest::{Matched, ServiceSuggestions, match_suggestions, suggestion_finds};
pub use taste::{Catalog, DayPart, Play, Taste, TasteInput, TrackInfo, hot_affinity, world_token};
pub use world_mixes::{WorldMix, WorldMixKind};
pub use tiles::{LyricsKind, like_title, PlaylistFacts, TILES_PER_DAY, TileGroup, TileHistory, TileKind, TileMix, TilesInput, TrackFacts};

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
                context: Default::default(),
                origin: None,
                end_reason: None,
                attention: None,
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

    /// 2 000 tracks by 40 artists, the odd ones streams without a file,
    /// analysed, with two years of plays and likes on both kinds.
    fn half_streams() -> (Catalog, VectorIndex<nikgob_core::model::TrackId>, TasteInput, nikgob_core::model::UnixMs) {
        let (cat, input, now) = big_history(2_000, 20_000, 40);
        let cat = Catalog::new(cat.tracks().iter().map(|t| t.clone().with_local(t.id.0.as_u128() % 2 == 0)));
        let mut rng = rng::Rng::new(3);
        let mut idx = VectorIndex::new();
        for t in cat.tracks() {
            idx.insert(t.id, std::array::from_fn(|_| rng.f32()));
        }
        (cat, idx, input, now)
    }

    fn new_music() -> Vec<NewTrack> {
        use nikgob_core::streaming::{RemoteId, Service};
        (0..20)
            .map(|k| NewTrack {
                id: RemoteId::new(Service::Deezer, format!("{k}")),
                artist: format!("someone {k}"),
                album: None,
                vector: None,
            })
            .collect()
    }

    /// «Миксы только из скачанного»: every selection picks only tracks with
    /// a file and no new music, however much new music there is.
    #[test]
    fn local_only_picks_only_files() {
        let (cat, idx, input, now) = half_streams();
        let taste = Taste::build(&cat, &input, now);
        let recs = Recs::new(&cat, &taste, &idx).local_only(true);
        let local = |p: &Pick| match &p.item {
            Item::Track(id) => cat.get(*id).is_some_and(|t| t.local),
            Item::Stream(_) => false,
        };
        let check = |what: &str, picks: &[Pick]| {
            assert!(!picks.is_empty(), "{what}: nothing");
            assert!(picks.iter().all(local), "{what}: a stream or new music");
        };
        let new = new_music();
        let params = RadioParams { new_share: 0.5, seed: 1 };
        check("daily", &recs.daily_mix(&new, &DailyParams { new_share: 0.5, ..DailyParams::default() }));
        check("daylist", &recs.daylist(DayPart::Evening, &new, 20, &params).2);
        let streamed = cat.tracks().iter().find(|t| !t.local).unwrap();
        let seeds = [
            RadioSeed::Mood(Mood::Calm),
            RadioSeed::Artist(streamed.artists[0]),
            RadioSeed::Track(streamed.id),
            RadioSeed::Stream {
                id: new[0].id.clone(),
                vector: None,
                artists: streamed.artists.clone(),
            },
        ];
        for seed in seeds.into_iter().chain(recs.style_mixes(&[], 1).iter().map(StyleMix::seed)) {
            let mut r = RadioState::new(seed.clone());
            r.add_new(new.iter().cloned());
            check(&format!("{seed:?}"), &r.extend(&recs, 20, &params));
        }
        let mut facts = std::collections::HashMap::new();
        for t in cat.tracks() {
            let fact = TrackFacts {
                title: format!("Song {}", t.id.0.as_u128()),
                duration_ms: Some(if t.id.0.as_u128() % 3 == 0 { 8 * 60_000 } else { 120_000 }),
                has_local: t.local,
                ..TrackFacts::default()
            };
            facts.insert(t.id, fact);
        }
        let tiles = TilesInput {
            facts,
            events: input.events.clone(),
            ..TilesInput::default()
        };
        let ready = recs.ready_tiles(&tiles, &TileHistory::default());
        assert!(ready.len() >= 5, "{:?}", ready.iter().map(|t| t.kind).collect::<Vec<_>>());
        for t in &ready {
            check(&format!("{:?}", t.kind), &t.picks);
        }
        assert!(ready.iter().all(|t| t.kind != TileKind::NotDownloaded));
        // Without the setting the same library has its streams back.
        let all = Recs::new(&cat, &taste, &idx);
        let kinds: Vec<TileKind> = all.ready_tiles(&tiles, &TileHistory::default()).iter().map(|t| t.kind).collect();
        assert!(kinds.contains(&TileKind::NotDownloaded), "{kinds:?}");
        let daily = all.daily_mix(&new, &DailyParams { new_share: 0.5, ..DailyParams::default() });
        assert!(!daily.iter().all(local));
        // Discovery still grows from everything liked.
        assert_eq!(recs.discovery_seeds(), all.discovery_seeds());
    }

    /// «Наугад» draws anew with every seed: other songs, another first.
    #[test]
    fn the_lottery_is_a_lottery() {
        let (cat, idx, input, now) = half_streams();
        let taste = Taste::build(&cat, &input, now);
        let recs = Recs::new(&cat, &taste, &idx);
        let draws: Vec<Vec<Pick>> = (0..20).map(|s| recs.lottery(s)).collect();
        assert!(draws.iter().all(|d| d.len() == tiles::TILE_LEN));
        let firsts: HashSet<&Item> = draws.iter().map(|d| &d[0].item).collect();
        assert!(firsts.len() >= 15, "the first song is known: {firsts:?}");
        let a: HashSet<&Item> = draws[0].iter().map(|p| &p.item).collect();
        let b: HashSet<&Item> = draws[1].iter().map(|p| &p.item).collect();
        assert!(a.intersection(&b).count() < 10, "two draws are alike");
        for d in &draws {
            pick::assert_diverse(&recs, d);
        }
        let only = Recs::new(&cat, &taste, &idx).local_only(true);
        assert!(only.lottery(7).iter().all(|p| matches!(p.item, Item::Track(id) if cat.get(id).unwrap().local)));
    }
}
