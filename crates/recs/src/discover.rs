//! New music (docs/stage4.md §5).
//!
//! **«Новое для тебя».** The engine picks the seeds ([`Recs::discovery_seeds`]:
//! the [`SEED_TRACKS`] tracks with the highest affinity among those played
//! in the last 30 days and the [`SEED_ARTISTS`] artists with the highest
//! affinity); the runtime asks the services around each seed (YouTube Music
//! related / radio for a track, similar artists of YouTube Music and Deezer
//! and their Deezer top tracks for an artist) and hands the lists back
//! ([`SeedFinds`]). [`rank_discoveries`] folds one song from several lists
//! and services into one candidate ([`song_key`]) and scores it: the sum
//! over the distinct seeds that found it of the seed's weight (0.75…1 by
//! its affinity, a little more when the song stood high in the seed's
//! list; a service suggesting it itself, [`SeedRef::Service`], weighs
//! [`crate::suggest::SERVICE_VOTE`]), plus [`NEW_ARTIST_BONUS`] for an
//! artist the library does not
//! know, times [`CLIP_FACTOR`] for a music video. Known songs, disliked
//! songs and artists, and songs shown [`IGNORED_AFTER`] times and never
//! played are out. The best [`DISCOVERIES`], at most [`PER_ARTIST`] per
//! artist, no artist twice in a row of three.
//!
//! **«Новые релизы».** For the artists with affinity
//! ([`Recs::release_artists`]) the runtime fetches discographies; albums
//! dated (to the day) within the last [`RELEASE_WINDOW_MS`] that the library
//! does not have are ranked by the artist's affinity and freshness
//! ([`Recs::new_releases`]). A year without a day (YouTube Music) cannot
//! say "the last 60 days" and is left out.

use std::collections::{HashMap, HashSet};

use nikgob_core::clip_title;
use nikgob_core::model::{ArtistId, ReleaseType, TrackId, UnixMs};
use nikgob_core::streaming::{RemoteId, Service, StreamAlbum, StreamTrack, TrackKind};

use crate::pick::Recs;
use crate::taste::DAY_MS;

pub const SEED_TRACKS: usize = 10;
pub const SEED_ARTISTS: usize = 8;
/// Seed tracks were played within this long.
pub const SEED_RECENT_MS: i64 = 30 * DAY_MS;
pub const DISCOVERIES: usize = 30;
pub const PER_ARTIST: usize = 2;
pub const NEW_ARTIST_BONUS: f32 = 0.25;
pub const CLIP_FACTOR: f32 = 0.7;
/// A song's place in a seed's list adds up to this share of the seed.
const RANK_BONUS: f32 = 0.15;
/// Shown this many times and never played: ignored.
pub const IGNORED_AFTER: u32 = 3;
pub const RELEASE_WINDOW_MS: i64 = 60 * DAY_MS;
pub const RELEASE_ARTISTS: usize = 30;
pub const NEW_RELEASES: usize = 20;
const RELEASES_PER_ARTIST: usize = 2;

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

/// Words a bracket or a " - " tail may consist of and still be release
/// noise (as the library's same-song rule).
const NOISE: &[&str] = &[
    "remaster", "remastered", "remastering", "ремастер", "ремастеринг", "digital", "digitally", "explicit",
    "clean", "mono", "stereo", "album", "version", "single", "original", "deluxe", "edition", "bonus",
    "track", "anniversary", "expanded", "reissue",
];

fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| if c == 'ё' { 'е' } else { c })
        .collect()
}

fn words(s: &str) -> impl Iterator<Item = &str> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty())
}

fn is_feat(s: &str) -> bool {
    let s = s.trim_start();
    ["feat.", "feat ", "ft.", "ft ", "featuring ", "with ", "при участии "]
        .iter()
        .any(|p| s.starts_with(p))
}

fn is_noise(body: &str) -> bool {
    words(body).all(|w| NOISE.contains(&w) || w.starts_with(|c: char| c.is_ascii_digit()))
}

/// A title folded: lower case, `ё` → `е`, noise brackets and tails
/// ("Remastered 2011", "feat. X") dropped, punctuation gone. "Live",
/// "Remix" and the like stay: another version is another song.
pub fn title_key(raw: &str) -> String {
    let lower = fold(raw);
    let mut kept = String::with_capacity(lower.len());
    let mut rest = lower.as_str();
    while let Some(open) = rest.find(['(', '[']) {
        kept.push_str(&rest[..open]);
        let close_ch = if rest[open..].starts_with('(') { ')' } else { ']' };
        let after = &rest[open + 1..];
        let (body, next) = match after.find(close_ch) {
            Some(close) => (&after[..close], &after[close + 1..]),
            None => (after, ""),
        };
        if !(is_feat(body) || is_noise(body)) {
            kept.push(' ');
            kept.push_str(body);
            kept.push(' ');
        }
        rest = next;
    }
    kept.push_str(rest);
    let mut parts = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    for dash in [" - ", " – ", " — "] {
        let segs: Vec<&str> = parts.split(dash).collect();
        if segs.len() > 1 {
            let mut out = segs[0].to_owned();
            for s in &segs[1..] {
                if !(is_feat(s) || is_noise(s)) {
                    out.push(' ');
                    out.push_str(s);
                }
            }
            parts = out;
        }
    }
    for cut in [" feat. ", " feat ", " ft. ", " featuring "] {
        if let Some(at) = parts.find(cut) {
            parts.truncate(at);
        }
    }
    words(&parts).collect::<Vec<_>>().join(" ")
}

/// The main artist of a name or credit line folded ("A, B", "A & B",
/// "A feat. B" → "a"; " - Topic" of YouTube channels dropped).
pub fn artist_key(name: &str) -> String {
    let lower = fold(name);
    let lower = lower.strip_suffix(" - topic").unwrap_or(&lower);
    let mut end = lower.len();
    for sep in [", ", "、", "，", " & ", " feat", " ft.", " featuring ", " x "] {
        if let Some(at) = lower.find(sep)
            && at > 0
        {
            end = end.min(at);
        }
    }
    words(&lower[..end]).collect::<Vec<_>>().join(" ")
}

/// One song across services and versions of its upload: the main artist
/// and the title, folded. The runtime builds the set of known songs with
/// the same function from library tracks.
pub fn song_key(title: &str, artist: &str) -> String {
    format!("{}\u{1f}{}", artist_key(artist), title_key(title))
}

/// One album across services.
pub fn album_key(title: &str, artist: &str) -> String {
    song_key(title, artist)
}

/// [`song_key`] of a stream track (a clip's title without what clips add).
pub fn stream_song_key(t: &StreamTrack) -> String {
    let artist = t.artists.first().map_or("", |a| a.name.as_str());
    if t.kind == TrackKind::Video {
        song_key(&clip_title::display_title(&t.title, artist), artist)
    } else {
        song_key(&t.title, artist)
    }
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// What a seed is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SeedRef {
    Track(TrackId),
    Artist(ArtistId),
    /// The service suggests the song itself («сервис советует»,
    /// [`crate::suggest`]).
    Service(Service),
}

/// The seeds of a discovery pass with their affinities.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiscoverySeeds {
    pub tracks: Vec<(TrackId, f32)>,
    pub artists: Vec<(ArtistId, f32)>,
}

impl DiscoverySeeds {
    fn affinity(&self, seed: SeedRef) -> f32 {
        match seed {
            SeedRef::Track(id) => self.tracks.iter().find(|(t, _)| *t == id).map_or(0.0, |x| x.1),
            SeedRef::Artist(id) => self.artists.iter().find(|(a, _)| *a == id).map_or(0.0, |x| x.1),
            SeedRef::Service(_) => 0.0,
        }
    }
}

/// What the services gave for one seed, in their order (several lists of
/// one seed may be passed as several finds of the same seed).
#[derive(Clone, Debug)]
pub struct SeedFinds {
    pub seed: SeedRef,
    pub tracks: Vec<StreamTrack>,
}

/// What the runtime knows about the user beyond the library's ids.
#[derive(Clone, Debug, Default)]
pub struct DiscoveryFilter {
    /// Remote ids of library sources.
    pub known_ids: HashSet<RemoteId>,
    /// [`song_key`]s of library tracks.
    pub known_songs: HashSet<String>,
    /// [`artist_key`]s of library artists (the others are new to the user).
    pub known_artists: HashSet<String>,
    /// [`song_key`]s and [`artist_key`]s of «Не рекомендовать».
    pub disliked_songs: HashSet<String>,
    pub disliked_artists: HashSet<String>,
    /// [`song_key`] → (times shown, ever played) of earlier discoveries.
    pub shown: HashMap<String, (u32, bool)>,
}

/// One discovery.
#[derive(Clone, Debug, PartialEq)]
pub struct Discovery {
    /// The best version found (a song over a clip, a playable one).
    pub track: StreamTrack,
    pub key: String,
    pub score: f32,
    /// How many distinct seeds pointed at it.
    pub votes: u32,
    /// The strongest of them (for "because you listen to X").
    pub via: SeedRef,
    pub new_artist: bool,
}

impl Recs<'_> {
    /// The seeds of a discovery pass.
    pub fn discovery_seeds(&self) -> DiscoverySeeds {
        let now = self.now();
        let mut tracks: Vec<(TrackId, f32)> = (0..self.catalog.len())
            // Streams without a file are seeds too, whatever the mixes pick.
            .filter(|&i| {
                !self.taste.blocks(self.info(i))
                    && self.taste.affinity_at(i) > 0.0
                    && self.taste.last_played_at(i).is_some_and(|t| now - t <= SEED_RECENT_MS)
            })
            .map(|i| (self.info(i).id, self.taste.affinity_at(i)))
            .collect();
        tracks.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        tracks.truncate(SEED_TRACKS);
        let mut artists: Vec<(ArtistId, f32)> = self
            .taste
            .artist_affinities()
            .filter(|(a, s)| *s > 0.0 && !self.taste.is_disliked_artist(*a))
            .collect();
        artists.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        artists.truncate(SEED_ARTISTS);
        DiscoverySeeds { tracks, artists }
    }

    /// The artists whose discographies to check for new releases.
    pub fn release_artists(&self) -> Vec<ArtistId> {
        let mut artists: Vec<(ArtistId, f32)> = self
            .taste
            .artist_affinities()
            .filter(|(a, s)| *s > 0.0 && !self.taste.is_disliked_artist(*a))
            .collect();
        artists.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        artists.into_iter().take(RELEASE_ARTISTS).map(|(a, _)| a).collect()
    }

    /// New releases of `artists` (their library id and their discography
    /// from a service), best first; `known_albums` are [`album_key`]s of
    /// the library's releases.
    pub fn new_releases(&self, artists: &[(ArtistId, Vec<StreamAlbum>)], known_albums: &HashSet<String>) -> Vec<NewRelease> {
        let now = self.now();
        let max_aff = artists
            .iter()
            .map(|(a, _)| self.taste.artist_affinity(*a))
            .fold(0.0f32, f32::max)
            .max(1e-6);
        // An album twice: by its name, or by its id (a shared album is in
        // the discography of each of its artists, credited to each).
        let mut seen: HashSet<String> = HashSet::new();
        let mut seen_ids: HashSet<&RemoteId> = HashSet::new();
        let mut out: Vec<NewRelease> = Vec::new();
        for (artist, albums) in artists {
            if self.taste.is_disliked_artist(*artist) {
                continue;
            }
            let aff = self.taste.artist_affinity(*artist).max(0.0) / max_aff;
            for album in albums {
                let Some(at) = album.released_at_ms() else { continue };
                let age = now - at;
                if !(0..=RELEASE_WINDOW_MS).contains(&age) {
                    continue;
                }
                let name = album.artists.first().map_or("", |a| a.name.as_str());
                let key = album_key(&album.title, name);
                if known_albums.contains(&key) || !seen.insert(key) || !seen_ids.insert(&album.id) {
                    continue;
                }
                let kind = match album.release_type {
                    ReleaseType::Album => 1.0,
                    ReleaseType::Ep => 0.8,
                    ReleaseType::Single => 0.6,
                    _ => 0.5,
                };
                let fresh = 1.0 - age as f32 / RELEASE_WINDOW_MS as f32;
                out.push(NewRelease {
                    artist: *artist,
                    album: album.clone(),
                    released_at: at,
                    score: (0.5 + aff) * (0.5 + 0.5 * fresh) * kind,
                });
            }
        }
        out.sort_by(|a, b| b.score.total_cmp(&a.score).then(b.released_at.cmp(&a.released_at)).then(a.album.id.id.cmp(&b.album.id.id)));
        let mut per: HashMap<ArtistId, usize> = HashMap::new();
        out.retain(|r| {
            let c = per.entry(r.artist).or_default();
            *c += 1;
            *c <= RELEASES_PER_ARTIST
        });
        out.truncate(NEW_RELEASES);
        out
    }
}

/// One new release.
#[derive(Clone, Debug, PartialEq)]
pub struct NewRelease {
    pub artist: ArtistId,
    pub album: StreamAlbum,
    pub released_at: UnixMs,
    pub score: f32,
}

struct Cand {
    track: StreamTrack,
    seeds: HashMap<SeedRef, f32>,
    first: usize,
}

fn better_version(a: &StreamTrack, b: &StreamTrack) -> bool {
    let rank = |t: &StreamTrack| (t.playable, t.kind == TrackKind::Song);
    rank(b) > rank(a)
}

/// The discoveries of a pass (see the module docs), best first.
pub fn rank_discoveries(seeds: &DiscoverySeeds, finds: &[SeedFinds], filter: &DiscoveryFilter) -> Vec<Discovery> {
    let max_aff = seeds
        .tracks
        .iter()
        .map(|x| x.1)
        .chain(seeds.artists.iter().map(|x| x.1))
        .fold(0.0f32, f32::max)
        .max(1e-6);
    let mut cands: HashMap<String, Cand> = HashMap::new();
    let mut order = 0usize;
    for f in finds {
        let w_seed = match f.seed {
            SeedRef::Service(_) => crate::suggest::SERVICE_VOTE,
            seed => 0.75 + 0.25 * (seeds.affinity(seed).max(0.0) / max_aff).min(1.0),
        };
        let len = f.tracks.len().max(1) as f32;
        for (rank, t) in f.tracks.iter().enumerate() {
            let key = stream_song_key(t);
            let artist = t.artists.first().map(|a| artist_key(&a.name)).unwrap_or_default();
            if filter.known_ids.contains(&t.id)
                || filter.known_songs.contains(&key)
                || filter.disliked_songs.contains(&key)
                || t.artists.iter().any(|a| filter.disliked_artists.contains(&artist_key(&a.name)))
                || filter.shown.get(&key).is_some_and(|(n, played)| *n >= IGNORED_AFTER && !played)
            {
                continue;
            }
            if artist.is_empty() {
                continue;
            }
            let w = w_seed * (1.0 + RANK_BONUS * (1.0 - rank as f32 / len));
            let c = cands.entry(key).or_insert_with(|| {
                order += 1;
                Cand {
                    track: t.clone(),
                    seeds: HashMap::new(),
                    first: order,
                }
            });
            if better_version(&c.track, t) {
                c.track = t.clone();
            }
            let e = c.seeds.entry(f.seed).or_insert(0.0);
            *e = e.max(w);
        }
    }
    let mut out: Vec<(Discovery, usize)> = cands
        .into_iter()
        .filter(|(_, c)| c.track.playable)
        .map(|(key, c)| {
            let artist = c.track.artists.first().map(|a| artist_key(&a.name)).unwrap_or_default();
            let new_artist = !filter.known_artists.contains(&artist);
            let mut score: f32 = c.seeds.values().sum();
            if new_artist {
                score += NEW_ARTIST_BONUS;
            }
            if c.track.kind == TrackKind::Video {
                score *= CLIP_FACTOR;
            }
            let via = c
                .seeds
                .iter()
                .max_by(|a, b| a.1.total_cmp(b.1).then(b.0.cmp(a.0)))
                .map(|(s, _)| *s)
                .expect("a candidate has a seed");
            (
                Discovery {
                    votes: c.seeds.len() as u32,
                    track: c.track,
                    key,
                    score,
                    via,
                    new_artist,
                },
                c.first,
            )
        })
        .collect();
    out.sort_by(|a, b| b.0.score.total_cmp(&a.0.score).then(a.1.cmp(&b.1)));

    // At most two per artist; no artist twice within three.
    let mut per: HashMap<String, usize> = HashMap::new();
    let mut pool: Vec<Discovery> = Vec::new();
    for (d, _) in out {
        let a = artist_key(&d.track.artists[0].name);
        let c = per.entry(a).or_default();
        if *c < PER_ARTIST {
            *c += 1;
            pool.push(d);
        }
    }
    let mut result: Vec<Discovery> = Vec::with_capacity(DISCOVERIES);
    while result.len() < DISCOVERIES && !pool.is_empty() {
        let recent: Vec<String> = result
            .iter()
            .rev()
            .take(2)
            .map(|d| artist_key(&d.track.artists[0].name))
            .collect();
        let i = pool
            .iter()
            .position(|d| !recent.contains(&artist_key(&d.track.artists[0].name)))
            .unwrap_or(0);
        result.push(pool.remove(i));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::VectorIndex;
    use crate::taste::{Catalog, Play, Taste, TasteInput};
    use crate::test_util::*;
    use nikgob_core::model::EventType;
    use nikgob_core::streaming::{ArtistRef, Service};

    fn st(service: Service, id: &str, title: &str, artist: &str) -> StreamTrack {
        let mut t = StreamTrack::new(RemoteId::new(service, id), title);
        t.artists = vec![ArtistRef::new(None, artist)];
        t
    }

    #[test]
    fn keys_fold_what_services_add() {
        assert_eq!(title_key("Song (Remastered 2011)"), title_key("song"));
        assert_eq!(title_key("Song - 2011 Remaster"), "song");
        assert_eq!(title_key("Song (feat. Someone)"), "song");
        assert_ne!(title_key("Song (Live)"), title_key("Song"));
        assert_eq!(artist_key("Eve - Topic"), "eve");
        assert_eq!(artist_key("A & B"), "a");
        assert_eq!(artist_key("Ёлка feat. X"), "елка");
        assert_eq!(artist_key("Kevin Penkin、immi"), artist_key("Kevin Penkin"));
        let mut clip = st(Service::Youtube, "v", "AIYOU - Eve Music Video", "Eve");
        clip.kind = TrackKind::Video;
        assert_eq!(stream_song_key(&clip), song_key("AIYOU", "Eve"));
    }

    fn seeds() -> DiscoverySeeds {
        DiscoverySeeds {
            tracks: vec![(tid(1), 10.0), (tid(2), 1.0), (tid(3), 1.0)],
            artists: vec![(aid(1), 2.0)],
        }
    }

    #[test]
    fn votes_beat_single_hits_and_filters_hold() {
        let finds = vec![
            SeedFinds {
                seed: SeedRef::Track(tid(1)),
                tracks: vec![
                    st(Service::Youtube, "solo", "Solo Hit", "Known Artist"),
                    st(Service::Youtube, "known", "Known Song", "Known Artist"),
                    st(Service::Youtube, "bad", "Bad", "Disliked One"),
                    st(Service::Youtube, "ign", "Ignored", "Someone"),
                ],
            },
            SeedFinds {
                seed: SeedRef::Track(tid(2)),
                tracks: vec![
                    st(Service::Youtube, "x9", "filler", "Filler"),
                    st(Service::Youtube, "both", "Shared Song", "Known Artist B"),
                ],
            },
            SeedFinds {
                seed: SeedRef::Track(tid(3)),
                tracks: vec![
                    st(Service::Youtube, "y9", "filler 2", "Filler 2"),
                    // The same song from Deezer, remastered: one candidate.
                    st(Service::Deezer, "777", "Shared Song (Remastered)", "Known Artist B"),
                ],
            },
            // The same seed twice counts once.
            SeedFinds {
                seed: SeedRef::Track(tid(2)),
                tracks: vec![st(Service::Deezer, "778", "Shared Song", "Known Artist B")],
            },
        ];
        let filter = DiscoveryFilter {
            known_songs: [song_key("Known Song", "Known Artist")].into(),
            known_artists: ["known artist".to_owned(), "known artist b".to_owned()].into(),
            disliked_artists: ["disliked one".to_owned()].into(),
            shown: [(song_key("Ignored", "Someone"), (3, false))].into(),
            ..Default::default()
        };
        let out = rank_discoveries(&seeds(), &finds, &filter);
        let keys: Vec<&str> = out.iter().map(|d| d.track.title.as_str()).collect();
        assert_eq!(out[0].track.title, "Shared Song", "{keys:?}");
        assert_eq!(out[0].votes, 2);
        assert_eq!(out[1].track.title, "Solo Hit", "{keys:?}");
        assert!(!keys.contains(&"Known Song"));
        assert!(!keys.contains(&"Bad"));
        assert!(!keys.contains(&"Ignored"));
        assert!(out.iter().any(|d| d.track.title == "filler" && d.new_artist));
        assert!(!out[0].new_artist);
    }

    #[test]
    fn clips_lose_and_artists_spread() {
        let mut clip = st(Service::Youtube, "c", "Clip Song (Official Video)", "Clipper");
        clip.kind = TrackKind::Video;
        let mut tracks = vec![clip, st(Service::Youtube, "s", "Plain Song", "Plain")];
        for k in 0..6 {
            tracks.push(st(Service::Youtube, &format!("m{k}"), &format!("Many {k}"), "Prolific"));
        }
        tracks.push(st(Service::Youtube, "o", "Other", "Other"));
        let finds = vec![SeedFinds {
            seed: SeedRef::Artist(aid(1)),
            tracks,
        }];
        let out = rank_discoveries(&seeds(), &finds, &DiscoveryFilter::default());
        let pos = |t: &str| out.iter().position(|d| d.track.title.starts_with(t)).unwrap();
        assert!(pos("Plain") < pos("Clip"), "a clip ranks lower");
        assert_eq!(out.iter().filter(|d| d.track.artists[0].name == "Prolific").count(), 2);
        for w in out.windows(2) {
            assert_ne!(w[0].track.artists[0].name, w[1].track.artists[0].name);
        }
    }

    #[test]
    fn seeds_and_new_releases() {
        let now = 1_000 * DAY_MS;
        let cat = Catalog::new((0..30u128).map(|i| info(i, &[(i % 3) as u32], None)));
        let mut events = Vec::new();
        for i in 0..30u128 {
            // Track i played i+1 times, recent ones only below 20.
            let ago = if i < 20 { DAY_MS } else { 40 * DAY_MS };
            for _ in 0..=i {
                events.push(Play {
                    track: tid(i),
                    ts: now - ago,
                    kind: EventType::Complete,
                    position_ms: None,
                });
            }
        }
        let input = TasteInput {
            events,
            disliked_tracks: vec![tid(19)],
            ..Default::default()
        };
        let taste = Taste::build(&cat, &input, now);
        let idx = VectorIndex::new();
        let recs = Recs::new(&cat, &taste, &idx);
        let s = recs.discovery_seeds();
        let ids: Vec<u128> = s.tracks.iter().map(|(t, _)| t.0.as_u128()).collect();
        assert_eq!(ids, (9..19).rev().collect::<Vec<_>>(), "recent, by affinity, no dislikes");
        assert_eq!(s.artists.len(), 3);

        let date = |days_ago: i64| {
            let d = now - days_ago * DAY_MS;
            // Back to a date string through days since the epoch.
            let (y, m, dd) = civil(d / DAY_MS);
            format!("{y:04}-{m:02}-{dd:02}")
        };
        let album = |id: &str, title: &str, days_ago: Option<i64>, kind: ReleaseType| StreamAlbum {
            id: RemoteId::new(Service::Deezer, id),
            title: title.to_owned(),
            artists: vec![ArtistRef::new(None, "Band")],
            year: Some(1972),
            release_date: days_ago.map(date),
            release_type: kind,
            cover_url: None,
            total_tracks: None,
            tracks: Vec::new(),
        };
        let artists = vec![
            (
                aid(2),
                vec![
                    album("1", "Fresh", Some(3), ReleaseType::Album),
                    album("2", "Older", Some(55), ReleaseType::Album),
                    album("3", "Too old", Some(70), ReleaseType::Album),
                    album("4", "Year only", None, ReleaseType::Album),
                    album("5", "Owned", Some(5), ReleaseType::Album),
                    album("6", "Single", Some(4), ReleaseType::Single),
                ],
            ),
            (aid(0), vec![album("7", "Other band", Some(10), ReleaseType::Album)]),
            // «Fresh» is shared: the guest's discography credits it to the
            // guest, the same album by its id.
            (
                aid(1),
                vec![StreamAlbum {
                    artists: vec![ArtistRef::new(None, "Guest")],
                    ..album("1", "Fresh", Some(3), ReleaseType::Album)
                }],
            ),
        ];
        let known: HashSet<String> = [album_key("Owned", "Band")].into();
        let rel = recs.new_releases(&artists, &known);
        let titles: Vec<&str> = rel.iter().map(|r| r.album.title.as_str()).collect();
        let mut sorted = titles.clone();
        sorted.sort();
        assert_eq!(sorted, ["Fresh", "Other band", "Single"], "two per artist, dated, unknown: {titles:?}");
        let pos = |t: &str| titles.iter().position(|x| *x == t).unwrap();
        assert!(pos("Fresh") < pos("Single"), "{titles:?}");
    }

    /// Civil date of days since the epoch (Howard Hinnant).
    fn civil(z: i64) -> (i64, i64, i64) {
        let z = z + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        (if m <= 2 { y + 1 } else { y }, m, d)
    }
}
