//! The services' own suggestions as a signal of taste (docs/stage4.md
//! §12.3): YouTube Music's «Быстрый выбор» and «Слушать снова», its
//! history, Spotify's tops and recently played. They never show as cards
//! of their own; they move our mixes.
//!
//! * A suggested track the library has (by the service's id or the same
//!   song, [`match_suggestions`]) gets more affinity in the [`Taste`]: the
//!   suggestion's [`strength`] times [`W_SUGGESTED`], fading linearly to
//!   nothing over [`SUGGESTION_FADE_MS`] since it was suggested, at most
//!   [`SUGGESTED_CAP`] (a complete play's worth) however often it is
//!   suggested. A suggested artist gets more artist affinity the same way
//!   ([`W_SUGGESTED_ARTIST`], at most [`SUGGESTED_ARTIST_CAP`]).
//! * A suggested track the library does not have is new music: a
//!   discovery candidate with the service's own vote ([`suggestion_finds`],
//!   [`crate::SeedRef::Service`] weighs [`SERVICE_VOTE`]) and, the
//!   strongest few, a seed of the discovery pass ([`SUGGESTION_SEEDS`]).
//!   Like all new music it reaches the mixes and radios only through their
//!   new-music share («Новое в радио и миксах»), never more.
//!
//! [`Taste`]: crate::Taste

use std::collections::HashMap;

use nikgob_core::model::{ArtistId, TrackId, UnixMs};
use nikgob_core::streaming::{Service, StreamTrack, SuggestedArtist, SuggestedTrack, SuggestionKind};

use crate::discover::{SeedFinds, SeedRef, stream_song_key};
use crate::taste::DAY_MS;

/// A suggestion fades out over this long.
pub const SUGGESTION_FADE_MS: i64 = 14 * DAY_MS;
/// Affinity of a fresh suggestion of strength 1.
pub const W_SUGGESTED: f32 = 0.6;
/// The most affinity suggestions give one track.
pub const SUGGESTED_CAP: f32 = 1.0;
/// Artist affinity of a fresh suggestion of strength 1.
pub const W_SUGGESTED_ARTIST: f32 = 0.3;
/// The most artist affinity suggestions give one artist.
pub const SUGGESTED_ARTIST_CAP: f32 = 0.6;
/// The weight of a service's vote for a discovery (a seed weighs 0.75…1).
pub const SERVICE_VOTE: f32 = 1.0;
/// Suggested songs new to the library that seed the discovery pass.
pub const SUGGESTION_SEEDS: usize = 4;

/// How much a kind of suggestion says about the user's taste.
pub const fn kind_weight(kind: SuggestionKind) -> f32 {
    match kind {
        SuggestionKind::QuickPicks | SuggestionKind::TopMonth => 1.0,
        SuggestionKind::ListenAgain => 0.8,
        SuggestionKind::TopHalfYear => 0.7,
        SuggestionKind::Recent => 0.5,
    }
}

/// The strength of one suggestion: its kind's weight times its place.
pub fn strength(kind: SuggestionKind, weight: f32) -> f32 {
    kind_weight(kind) * weight.clamp(0.0, 1.0)
}

/// 1 for a suggestion made now, down to 0 at [`SUGGESTION_FADE_MS`].
pub fn fade(now: UnixMs, at: UnixMs) -> f32 {
    let age = (now - at).max(0) as f32;
    (1.0 - age / SUGGESTION_FADE_MS as f32).max(0.0)
}

/// One service's suggestions as the runtime keeps them.
#[derive(Clone, Copy, Debug)]
pub struct ServiceSuggestions<'a> {
    /// When they were read.
    pub at: UnixMs,
    pub tracks: &'a [SuggestedTrack],
    pub artists: &'a [SuggestedArtist],
}

/// Suggestions sorted out against the library.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Matched {
    /// For [`crate::TasteInput::suggested_tracks`].
    pub tracks: Vec<(TrackId, f32, UnixMs)>,
    /// For [`crate::TasteInput::suggested_artists`].
    pub artists: Vec<(ArtistId, f32, UnixMs)>,
    /// Songs the library does not have, one per song ([`stream_song_key`]),
    /// strongest first (a song suggested more than once sums up).
    pub unmatched: Vec<(StreamTrack, f32)>,
}

/// Sorts suggestions out: `track_of` finds a suggested track in the
/// catalog (the service's id, else the same song), `artist_of` a library
/// artist by name.
pub fn match_suggestions(
    lists: &[ServiceSuggestions<'_>],
    track_of: impl Fn(&StreamTrack) -> Option<TrackId>,
    artist_of: impl Fn(&str) -> Option<ArtistId>,
) -> Matched {
    let mut out = Matched::default();
    // Song key → (index in `unmatched`).
    let mut new_songs: HashMap<String, usize> = HashMap::new();
    for l in lists {
        for s in l.tracks {
            let w = strength(s.kind, s.weight);
            match track_of(&s.track) {
                Some(id) => out.tracks.push((id, w, l.at)),
                None => {
                    let key = stream_song_key(&s.track);
                    match new_songs.get(&key) {
                        Some(&k) => out.unmatched[k].1 += w,
                        None => {
                            new_songs.insert(key, out.unmatched.len());
                            out.unmatched.push((s.track.clone(), w));
                        }
                    }
                }
            }
        }
        for a in l.artists {
            if let Some(id) = artist_of(&a.artist.name) {
                out.artists.push((id, strength(a.kind, a.weight), l.at));
            }
        }
    }
    // Stable: equal strengths keep the order the services gave.
    out.unmatched.sort_by(|a, b| b.1.total_cmp(&a.1));
    out
}

/// The suggested songs new to the library as finds of the services' own
/// seeds ([`SeedRef::Service`]), one list per service in strength order.
pub fn suggestion_finds(unmatched: &[(StreamTrack, f32)]) -> Vec<SeedFinds> {
    let mut out: Vec<SeedFinds> = Vec::new();
    for &service in Service::ALL {
        let tracks: Vec<StreamTrack> = unmatched
            .iter()
            .filter(|(t, _)| t.id.service == service)
            .map(|(t, _)| t.clone())
            .collect();
        if !tracks.is_empty() {
            out.push(SeedFinds {
                seed: SeedRef::Service(service),
                tracks,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daily::DailyParams;
    use crate::discover::{DiscoveryFilter, DiscoverySeeds, rank_discoveries};
    use crate::index::VectorIndex;
    use crate::pick::{Item, NewTrack, Recs};
    use crate::radio::{RadioParams, RadioSeed, RadioState};
    use crate::taste::{Catalog, Play, Taste, TasteInput};
    use crate::test_util::*;
    use nikgob_core::model::EventType;
    use nikgob_core::streaming::{ArtistRef, RemoteId};

    fn st(service: Service, id: &str, title: &str, artist: &str) -> StreamTrack {
        let mut t = StreamTrack::new(RemoteId::new(service, id), title);
        t.artists = vec![ArtistRef::new(None, artist)];
        t
    }

    fn sugg(t: StreamTrack, kind: SuggestionKind, weight: f32) -> SuggestedTrack {
        SuggestedTrack { track: t, kind, weight }
    }

    #[test]
    fn strength_and_fade() {
        assert_eq!(strength(SuggestionKind::QuickPicks, 1.0), 1.0);
        assert_eq!(strength(SuggestionKind::Recent, 1.0), 0.5);
        assert_eq!(strength(SuggestionKind::TopMonth, 2.0), 1.0, "a weight is at most 1");
        let now = 100 * DAY_MS;
        assert_eq!(fade(now, now), 1.0);
        assert_eq!(fade(now, now + DAY_MS), 1.0, "from the future: fresh");
        assert!((fade(now, now - 7 * DAY_MS) - 0.5).abs() < 1e-6);
        assert_eq!(fade(now, now - 14 * DAY_MS), 0.0);
        assert_eq!(fade(now, now - 40 * DAY_MS), 0.0);
    }

    #[test]
    fn a_suggestion_adds_capped_fading_affinity() {
        let now = 1_000 * DAY_MS;
        let cat = Catalog::new((0..5).map(|i| info(i, &[i as u32], None)));
        let input = TasteInput {
            events: vec![Play {
                track: tid(4),
                ts: now - DAY_MS,
                kind: EventType::Complete,
                position_ms: None,
            }],
            suggested_tracks: vec![
                // 0: once, fresh: 0.6.
                (tid(0), 1.0, now),
                // 1: five times: capped at 1.
                (tid(1), 1.0, now),
                (tid(1), 1.0, now),
                (tid(1), 1.0, now),
                (tid(1), 1.0, now),
                (tid(1), 1.0, now),
                // 2: a week ago: half of 0.6.
                (tid(2), 1.0, now - 7 * DAY_MS),
                // 3: faded out.
                (tid(3), 1.0, now - 15 * DAY_MS),
                // Not in the catalog.
                (tid(77), 1.0, now),
            ],
            suggested_artists: vec![(aid(3), 1.0, now), (aid(2), 1.0, now), (aid(2), 1.0, now), (aid(2), 1.0, now)],
            ..Default::default()
        };
        let taste = Taste::build(&cat, &input, now);
        let a = |i: u128| taste.affinity_at(cat.position(tid(i)).unwrap());
        let close = |x: f32, y: f32| (x - y).abs() < 1e-3;
        assert!(close(a(0), 0.6), "{}", a(0));
        assert!(close(a(1), SUGGESTED_CAP), "{}", a(1));
        assert!(close(a(2), 0.3), "{}", a(2));
        assert_eq!(a(3), 0.0);
        assert!(close(taste.suggested_at(1), SUGGESTED_CAP));
        assert_eq!(taste.suggested_at(4), 0.0, "played, not suggested");
        // Artist 3 has only the artist bonus; artist 2 its track's
        // affinity plus a capped bonus.
        assert!(close(taste.artist_affinity(aid(3)), 0.3), "{}", taste.artist_affinity(aid(3)));
        assert!(close(taste.artist_affinity(aid(2)), 0.3 + SUGGESTED_ARTIST_CAP), "{}", taste.artist_affinity(aid(2)));
        assert!(taste.has_history(), "the complete is a history");

        let only = TasteInput {
            suggested_tracks: vec![(tid(0), 1.0, now)],
            ..Default::default()
        };
        let taste = Taste::build(&cat, &only, now);
        assert!(!taste.has_history(), "suggestions alone are not a history");
        assert!(taste.affinity_at(0) > 0.0);
    }

    #[test]
    fn matching_splits_library_songs_from_new_ones() {
        let now = 50 * DAY_MS;
        let yt = [
            sugg(st(Service::Youtube, "y1", "Known", "Band"), SuggestionKind::QuickPicks, 1.0),
            sugg(st(Service::Youtube, "y2", "New Song", "Stranger"), SuggestionKind::QuickPicks, 0.5),
            sugg(st(Service::Youtube, "y3", "Other New", "Stranger 2"), SuggestionKind::Recent, 1.0),
        ];
        let sp = [
            // The same new song from Spotify: one candidate, summed.
            sugg(st(Service::Spotify, "s1", "New Song (Remastered)", "Stranger"), SuggestionKind::TopMonth, 1.0),
            sugg(st(Service::Spotify, "s2", "Known", "Band"), SuggestionKind::TopMonth, 1.0),
        ];
        let artists = [SuggestedArtist {
            artist: ArtistRef::new(None, "Band"),
            kind: SuggestionKind::TopMonth,
            weight: 1.0,
        }];
        let lists = [
            ServiceSuggestions { at: now, tracks: &yt, artists: &[] },
            ServiceSuggestions { at: now - DAY_MS, tracks: &sp, artists: &artists },
        ];
        let m = match_suggestions(
            &lists,
            |t| (t.title == "Known").then_some(tid(1)),
            |name| (name == "Band").then_some(aid(1)),
        );
        assert_eq!(m.tracks, vec![(tid(1), 1.0, now), (tid(1), 1.0, now - DAY_MS)]);
        assert_eq!(m.artists, vec![(aid(1), 1.0, now - DAY_MS)]);
        let titles: Vec<&str> = m.unmatched.iter().map(|(t, _)| t.title.as_str()).collect();
        assert_eq!(titles, ["New Song", "Other New"], "summed 0.5 + 1 beats 0.5");
        assert!((m.unmatched[0].1 - 1.5).abs() < 1e-6);
        let finds = suggestion_finds(&m.unmatched);
        assert_eq!(finds.len(), 1, "only YouTube's ids are kept for a song");
        assert_eq!(finds[0].seed, SeedRef::Service(Service::Youtube));
    }

    #[test]
    fn a_service_vote_makes_a_discovery_and_adds_to_others() {
        let seeds = DiscoverySeeds {
            tracks: vec![(tid(1), 5.0)],
            artists: Vec::new(),
        };
        let mut finds = vec![SeedFinds {
            seed: SeedRef::Track(tid(1)),
            tracks: vec![
                st(Service::Youtube, "a", "Seed Found", "A"),
                st(Service::Youtube, "b", "Both Found", "B"),
            ],
        }];
        finds.extend(suggestion_finds(&[
            (st(Service::Youtube, "b", "Both Found", "B"), 1.0),
            (st(Service::Youtube, "c", "Only Suggested", "C"), 0.8),
            (st(Service::Spotify, "k", "Known", "K"), 0.8),
        ]));
        let filter = DiscoveryFilter {
            known_ids: [RemoteId::new(Service::Spotify, "k")].into(),
            ..Default::default()
        };
        let out = rank_discoveries(&seeds, &finds, &filter);
        let titles: Vec<&str> = out.iter().map(|d| d.track.title.as_str()).collect();
        assert_eq!(out[0].track.title, "Both Found", "{titles:?}");
        assert_eq!(out[0].votes, 2);
        assert!(titles.contains(&"Only Suggested"));
        assert!(!titles.contains(&"Known"), "the filter holds for suggestions too");
        let only = out.iter().find(|d| d.track.title == "Only Suggested").unwrap();
        assert_eq!(only.via, SeedRef::Service(Service::Youtube));
    }

    #[test]
    fn suggested_new_songs_never_pass_the_new_share() {
        let now = 1_000 * DAY_MS;
        let cat = Catalog::new((0..120u128).map(|i| info(i, &[(i % 12) as u32], Some(i / 6))));
        let events = (0..120u128)
            .map(|i| Play {
                track: tid(i),
                ts: now - (5 + (i % 40) as i64) * DAY_MS,
                kind: EventType::Complete,
                position_ms: None,
            })
            .collect();
        let taste = Taste::build(&cat, &TasteInput { events, ..Default::default() }, now);
        let idx = VectorIndex::new();
        let recs = Recs::new(&cat, &taste, &idx);
        // Forty suggested songs new to the library, of forty artists.
        let unmatched: Vec<(StreamTrack, f32)> = (0..40)
            .map(|k| (st(Service::Youtube, &format!("n{k}"), &format!("New {k}"), &format!("Stranger {k}")), 1.0))
            .collect();
        let new: Vec<NewTrack> = unmatched.iter().map(|(t, _)| NewTrack::from_stream(t, None)).collect();
        let streams = |picks: &[crate::pick::Pick]| picks.iter().filter(|p| matches!(p.item, Item::Stream(_))).count();
        for (share, most) in [(0.0, 0), (0.2, 6), (0.4, 12)] {
            let daily = recs.daily_mix(&new, &DailyParams { new_share: share, seed: 3, ..DailyParams::default() });
            assert_eq!(daily.len(), 30);
            assert!(streams(&daily) <= most, "{share}: {} new of 30", streams(&daily));
            let mut radio = RadioState::new(RadioSeed::Artist(aid(1)));
            radio.add_new(new.clone());
            let picks = radio.extend(&recs, 10, &RadioParams { new_share: share, seed: 3 });
            assert!(streams(&picks) <= (share * 10.0).round() as usize, "{share}: radio {}", streams(&picks));
        }
    }
}
