//! New music.
//!
//! **«Новое для тебя».** The engine picks the seeds ([`Recs::discovery_seeds`]:
//! [`SEED_TRACKS_MAX`] tracks shared among the taste profiles by their
//! shares, [`SEEDS_PER_PROFILE`] at least for each active one — new
//! Japanese songs are looked for while ambient fills the days — the best
//! loved of each as the profile sees them, those played in the last 30
//! days first; [`SEED_TRACKS`] by affinity without profiles; and
//! [`ELSEWHERE_SEEDS`] songs saved in a service's library that the library
//! lacks, others each day, every other one among the first, so the taste
//! a user keeps elsewhere looks for new music too, a user with nothing
//! else as well; the [`SEED_ARTISTS`] artists with the highest affinity,
//! those of the songs saved elsewhere among them, spread over the
//! profiles); the
//! runtime asks the services around each seed (YouTube Music
//! related / radio for a track, similar artists of YouTube Music and Deezer
//! and their Deezer top tracks for an artist) and hands the lists back
//! ([`SeedFinds`]). [`rank_discoveries`] folds one song from several lists
//! and services into one candidate ([`song_key`]) and scores it: the sum
//! over the distinct seeds that found it of the seed's weight (0.75…1 by
//! its affinity, a little more when the song stood high in the seed's
//! list; a service suggesting it itself, [`SeedRef::Service`], weighs
//! [`crate::suggest::SERVICE_VOTE`]), times [`multi_seed`] of the
//! independent seeds (a song two or three
//! of the user's seeds lead to stands far above a lone answer of one
//! request; the services' own suggestions count as one), plus
//! [`NEW_ARTIST_BONUS`] for an
//! artist the library does not
//! know, times [`CLIP_FACTOR`] for a music video (a song copy of it wins
//! the place; a video stays only when no song was found, and the player
//! asks for its song before it plays). Known songs, disliked
//! songs and artists, junk ([`crate::junk`], against the canonical
//! release among the finds) and songs shown [`IGNORED_AFTER`] times and
//! never played are out; one song by ISRC once. With the user model
//! ([`NewScoring`], recs-v2 §11.2) the score adds [`W_REGION_FIT`] times
//! how well its sound fits a taste profile (its analysed preview, else
//! half its seed's). A find is a profile's (profiles-v1 §8) when its
//! preview has [`PROFILE_MEMBER`] of it and its kind of song — the
//! language its artist or title tells — is the profile's; one of a
//! background profile also must not get in the way: no words told, a
//! preview in the middle of the profile's energy, two minutes or more.
//! Then [`W_KNOWN_ARTIST`] times a known artist's affinity,
//! minus [`W_SKIP_NEW`] times the chance it is skipped early (the skip
//! model with «new from a service»), minus the dislikes near it, minus a
//! week's [`crate::novelty::ARTIST_PENALTY`] after an early skip of its
//! artist, and leaves an artist skipped twice in a month out. The best
//! [`DISCOVERIES`], spread over the profiles by their shares
//! ([`crate::regions::spread`]), at most [`PER_ARTIST`] per artist, no
//! artist twice in a row of three; a larger pool (the open world's,
//! §1.1) goes on past them with [`POOL_PER_ARTIST`] of an artist at most,
//! the first [`DISCOVERIES`] what they were.
//!
//! **«Новые релизы».** For the artists with affinity
//! ([`Recs::release_artists`]) the runtime fetches discographies; albums
//! dated (to the day) within the last [`RELEASE_WINDOW_MS`] that the library
//! does not have are ranked by the artist's affinity and freshness
//! ([`Recs::new_releases`]). A year without a day (YouTube Music) cannot
//! say "the last 90 days" and is left out.

use std::collections::{HashMap, HashSet};

use nikgob_core::clip_title;
use nikgob_core::model::{ArtistId, EventContext, ReleaseType, TrackId, UnixMs};
use nikgob_core::streaming::{RemoteId, Service, StreamAlbum, StreamTrack, TrackKind};

use crate::analysis::Vector;
use crate::model::UserModel;
use crate::novelty::NewLog;
use crate::pick::Recs;
use crate::taste::DAY_MS;

pub const SEED_TRACKS: usize = 10;
/// Seed tracks of each active taste profile at least…
pub const SEEDS_PER_PROFILE: usize = 2;
/// …and of all of them, shared by the profiles' shares.
pub const SEED_TRACKS_MAX: usize = 30;
/// A find is a profile's with this membership in it…
pub const PROFILE_MEMBER: f32 = 0.35;
/// …and, for a background profile, in this band of its energy and this
/// long at least.
const QUIET_BAND: (f32, f32) = (0.15, 0.7);
const QUIET_MS: u64 = 120_000;
/// The weights of the score with the user model (recs-v2 §11.2).
pub const W_REGION_FIT: f32 = 1.2;
pub const W_KNOWN_ARTIST: f32 = 0.6;
pub const W_SKIP_NEW: f32 = 1.5;
pub const SEED_ARTISTS: usize = 8;
/// Songs saved in a service's library that the library lacks seed this
/// many tracks of a pass besides the others, in turn day by day.
pub const ELSEWHERE_SEEDS: usize = 4;
/// Seed tracks were played within this long.
pub const SEED_RECENT_MS: i64 = 30 * DAY_MS;
pub const DISCOVERIES: usize = 30;
pub const PER_ARTIST: usize = 2;
/// An artist's songs at most in a pool past the first [`DISCOVERIES`].
pub const POOL_PER_ARTIST: usize = 4;
/// What each independent seed past the first adds to a song's votes.
pub const MULTI_SEED: f32 = 0.75;
pub const NEW_ARTIST_BONUS: f32 = 0.25;
pub const CLIP_FACTOR: f32 = 0.5;
/// A song's place in a seed's list adds up to this share of the seed.
const RANK_BONUS: f32 = 0.15;
/// Shown this many times and never played: ignored.
pub const IGNORED_AFTER: u32 = 3;
/// A release is new for this long (three months: an artist puts something
/// out a few times a year, and two months left most of them with nothing).
pub const RELEASE_WINDOW_MS: i64 = 90 * DAY_MS;
/// The artists whose discographies are checked: the best loved.
pub const RELEASE_ARTISTS: usize = 50;
pub const NEW_RELEASES: usize = 36;
const RELEASES_PER_ARTIST: usize = 3;

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
    /// The taste profile of each seed track that has one.
    pub profiles: Vec<(TrackId, usize)>,
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

/// What scores a discovery beyond its votes (recs-v2 §11.2): the user
/// model, the analysed previews of candidates (raw bliss, by
/// [`song_key`]), the log of new music heard, the known artists'
/// affinities (by [`artist_key`], 0…1).
#[derive(Clone, Copy)]
pub struct NewScoring<'a> {
    pub model: &'a UserModel,
    pub vectors: &'a HashMap<String, Vector>,
    pub log: &'a NewLog,
    pub artist_affinity: &'a HashMap<String, f32>,
    pub now: UnixMs,
    /// The languages the library knows of artists (a find's kind of song).
    pub types: Option<&'a crate::SongTypes>,
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
    /// The taste profile it fits (its preview's, else its seed's).
    pub profile: Option<usize>,
}

impl Recs<'_> {
    /// The seeds of a discovery pass (see the module docs).
    pub fn discovery_seeds(&self) -> DiscoverySeeds {
        let mut s = self.library_seeds();
        self.add_elsewhere_seeds(&mut s);
        s
    }

    /// Today's [`ELSEWHERE_SEEDS`] of the songs saved elsewhere, every
    /// other one among the first seed tracks, with the profile of their
    /// artist's sound when the catalog has it.
    fn add_elsewhere_seeds(&self, s: &mut DiscoverySeeds) {
        let all = self.taste.saved_elsewhere();
        if all.is_empty() {
            return;
        }
        let mut order: Vec<&(TrackId, Vec<ArtistId>)> = all.iter().collect();
        order.sort_by_key(|x| x.0);
        let n = order.len();
        let day = self.now().div_euclid(DAY_MS).max(0) as usize;
        let start = day.wrapping_mul(ELSEWHERE_SEEDS) % n;
        let mut picks = (0..ELSEWHERE_SEEDS.min(n)).map(|k| order[(start + k) % n]);
        let mut tracks = Vec::with_capacity(s.tracks.len() + ELSEWHERE_SEEDS);
        for t in std::mem::take(&mut s.tracks) {
            tracks.push(t);
            if let Some((id, artists)) = picks.next() {
                tracks.push((*id, crate::taste::W_SAVED));
                s.profiles.extend(artists.iter().find_map(|a| self.artist_profile(*a)).map(|j| (*id, j)));
            }
        }
        for (id, artists) in picks {
            tracks.push((*id, crate::taste::W_SAVED));
            s.profiles.extend(artists.iter().find_map(|a| self.artist_profile(*a)).map(|j| (*id, j)));
        }
        s.tracks = tracks;
    }

    /// The profile of the artist's best loved track of the catalog.
    fn artist_profile(&self, a: ArtistId) -> Option<usize> {
        let m = self.model.filter(|m| !m.regions.is_empty())?;
        let now = self.now();
        self.catalog
            .of_artist(a)
            .iter()
            .map(|&i| i as usize)
            .max_by(|x, y| m.tracks[*x].aff_long(now).total_cmp(&m.tracks[*y].aff_long(now)).then(y.cmp(x)))
            .and_then(|i| m.profile_at(i))
    }

    /// The seeds of the library's tracks and the artists.
    fn library_seeds(&self) -> DiscoverySeeds {
        let mut s = self.seeds_by_affinity();
        let Some(m) = self.model.filter(|m| !m.regions.is_empty()) else { return s };
        let now = self.now();
        let mut by_profile: Vec<Vec<(f32, usize)>> = vec![Vec::new(); m.regions.len()];
        for i in 0..self.catalog.len() {
            let Some(j) = m.profile_at(i) else { continue };
            if self.taste.blocks(self.info(i)) || m.tracks[i].aff_long(now) <= 0.0 {
                continue;
            }
            let recent = if self.taste.last_played_at(i).is_some_and(|t| now - t <= SEED_RECENT_MS) { 1.0 } else { 0.5 };
            by_profile[j].push((m.profile_affinity(i) * recent, i));
        }
        // The profiles by share, a small one its seeds too.
        let mut order: Vec<usize> = (0..by_profile.len()).filter(|&j| !by_profile[j].is_empty()).collect();
        order.sort_by(|a, b| m.profile_share[*b].total_cmp(&m.profile_share[*a]).then(a.cmp(b)));
        let quota = seed_quota(&order, &m.profile_share);
        let mut tracks: Vec<(TrackId, f32)> = Vec::new();
        let mut profiles: Vec<(TrackId, usize)> = Vec::new();
        for (j, q) in order.into_iter().zip(quota) {
            let list = &mut by_profile[j];
            list.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
            for &(_, i) in list.iter().take(q) {
                if tracks.len() == SEED_TRACKS_MAX {
                    break;
                }
                let id = self.info(i).id;
                tracks.push((id, self.taste.affinity_at(i)));
                profiles.push((id, j));
            }
        }
        // The best loved beyond the profiles' seeds after them (with their
        // profile, when they have one).
        for (id, a) in std::mem::take(&mut s.tracks) {
            if tracks.len() >= SEED_TRACKS_MAX.max(SEED_TRACKS) {
                break;
            }
            if !tracks.iter().any(|t| t.0 == id) {
                tracks.push((id, a));
                if let Some(j) = self.catalog.position(id).and_then(|i| m.profile_at(i)) {
                    profiles.push((id, j));
                }
            }
        }
        // Artists spread over the profiles of their best loved tracks.
        let mut artists: Vec<(ArtistId, f32)> =
            self.taste.all_artist_affinities().filter(|(a, s)| *s > 0.0 && !self.taste.is_disliked_artist(*a)).collect();
        artists.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let items: Vec<((ArtistId, f32), Option<usize>)> = artists.iter().map(|x| (*x, self.artist_profile(x.0))).collect();
        s.artists = crate::regions::spread(&items, &m.profile_share, SEED_ARTISTS);
        s.tracks = tracks;
        s.profiles = profiles;
        s
    }

    /// The seeds by affinity alone (no profiles).
    fn seeds_by_affinity(&self) -> DiscoverySeeds {
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
            .all_artist_affinities()
            .filter(|(a, s)| *s > 0.0 && !self.taste.is_disliked_artist(*a))
            .collect();
        artists.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        artists.truncate(SEED_ARTISTS);
        DiscoverySeeds { tracks, artists, profiles: Vec::new() }
    }

    /// The artists whose discographies to check for new releases (those of
    /// the songs saved elsewhere too).
    pub fn release_artists(&self) -> Vec<ArtistId> {
        let mut artists: Vec<(ArtistId, f32)> = self
            .taste
            .all_artist_affinities()
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

/// The factor of a song `seeds` independent seeds lead to: 1 for one, then
/// [`MULTI_SEED`] more for each of the second and the third.
pub fn multi_seed(seeds: usize) -> f32 {
    1.0 + MULTI_SEED * (seeds.clamp(1, 3) - 1) as f32
}

/// The discoveries of a pass (see the module docs), best first.
pub fn rank_discoveries(seeds: &DiscoverySeeds, finds: &[SeedFinds], filter: &DiscoveryFilter) -> Vec<Discovery> {
    rank_discoveries_with(seeds, finds, filter, None)
}

/// The seed tracks of each profile of `order` (the largest share first):
/// [`SEEDS_PER_PROFILE`] each that has a share, the rest of
/// [`SEED_TRACKS_MAX`] by the shares, the largest remainders first.
fn seed_quota(order: &[usize], shares: &[f32]) -> Vec<usize> {
    let share = |j: usize| shares.get(j).copied().unwrap_or(0.0);
    let active: Vec<bool> = order.iter().map(|&j| share(j) >= crate::profiles::ACTIVE_SHARE).collect();
    let mut q: Vec<usize> = active.iter().map(|a| if *a { SEEDS_PER_PROFILE } else { 0 }).collect();
    let left = SEED_TRACKS_MAX.saturating_sub(q.iter().sum());
    let total: f32 = order.iter().zip(&active).filter(|(_, a)| **a).map(|(&j, _)| share(j)).sum::<f32>().max(1e-6);
    let exact: Vec<f32> = order.iter().zip(&active).map(|(&j, a)| if *a { left as f32 * share(j) / total } else { 0.0 }).collect();
    for (k, e) in exact.iter().enumerate() {
        q[k] += e.floor() as usize;
    }
    let mut rest: Vec<(f32, usize)> = exact.iter().enumerate().map(|(k, e)| (e - e.floor(), k)).collect();
    rest.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let short = SEED_TRACKS_MAX.saturating_sub(q.iter().sum());
    for (_, k) in rest.into_iter().filter(|(_, k)| active[*k]).take(short) {
        q[k] += 1;
    }
    q
}

/// The profile a find of `t` with its preview `v` (embedded) is of (see
/// the module docs): the best of its kind of song by its preview, else its
/// seed's (`seed`) of that kind.
fn find_profile(sc: &NewScoring<'_>, t: &StreamTrack, v: Option<&Vector>, seed: Option<usize>) -> Option<usize> {
    use crate::profiles::LaneKey;
    use crate::songtype::{SongType, Voice};
    let m = sc.model;
    let artists: Vec<&str> = t.artists.iter().map(|a| a.name.as_str()).collect();
    let lang = sc.types.and_then(|ty| ty.lang_of_new(&t.title, &artists));
    let kind = match lang {
        Some(l) => SongType { voice: Voice::Sung, lang: Some(l), ..SongType::default() },
        None => SongType::default(),
    };
    let fits = |j: usize| m.profiles.list.get(j).is_some_and(|p| lang.is_none_or(|l| p.lane == LaneKey::Sung(l)));
    let quiet = |j: usize| {
        if !m.is_background_profile(j) {
            return true;
        }
        let Some(v) = v else { return false };
        let mut e: Vec<f32> = (0..m.len()).filter(|&i| m.profile_at(i) == Some(j)).filter_map(|i| m.energy_at(i)).collect();
        e.sort_by(f32::total_cmp);
        let band = (!e.is_empty()).then(|| {
            let at = |q: f32| e[((e.len() - 1) as f32 * q) as usize];
            (at(QUIET_BAND.0), at(QUIET_BAND.1))
        });
        lang.is_none() && t.duration_ms.is_none_or(|d| d >= QUIET_MS) && band.is_none_or(|(lo, hi)| (lo..=hi).contains(&m.energy_of(v)))
    };
    let by_sound = v.and_then(|v| {
        let mem = m.profiles.membership(v, &kind);
        (0..mem.len()).filter(|&j| mem[j] >= PROFILE_MEMBER && fits(j)).max_by(|a, b| mem[*a].total_cmp(&mem[*b]).then(b.cmp(a)))
    });
    by_sound.or(if v.is_none() { seed.filter(|&j| fits(j)) } else { None }).filter(|&j| quiet(j))
}

/// What the user model adds to a candidate's score, and its profile.
fn model_score(t: &StreamTrack, key: &str, via_seeds: &HashMap<SeedRef, f32>, seeds: &DiscoverySeeds, sc: &NewScoring<'_>) -> Option<(f32, Option<usize>)> {
    let m = sc.model;
    let artist = t.artists.first().map(|a| artist_key(&a.name)).unwrap_or_default();
    let penalty = sc.log.artist_penalty(&artist, sc.now);
    if penalty.is_infinite() {
        return None;
    }
    let v = sc.vectors.get(key).map(|v| m.embed(v));
    // The profile and the fit: the preview's, else half the seed's.
    let seed_profile = via_seeds
        .iter()
        .filter_map(|(s, w)| match s {
            SeedRef::Track(id) => seeds.profiles.iter().find(|p| p.0 == *id).map(|p| (p.1, *id, *w)),
            _ => None,
        })
        .max_by(|a, b| a.2.total_cmp(&b.2));
    let profile = find_profile(sc, t, v.as_ref(), seed_profile.map(|s| s.0));
    // A find of no profile (another kind of song, in the way of the
    // background) fits half.
    let lost = if profile.is_some() { 1.0 } else { 0.5 };
    let fit = match &v {
        Some(v) => lost * m.regions.fit(v),
        None => match seed_profile {
            Some((_, id, _)) => lost * 0.5 * m.position(id).and_then(|p| m.embedded_at(p)).map_or(0.5, |e| m.regions.fit(e)),
            None => 0.25,
        },
    };
    let known = sc.artist_affinity.get(&artist).copied().unwrap_or(0.0);
    let moment = m.moment(&crate::session::Session::default(), sc.now);
    let duration = t.duration_ms.map(|d| d.min(u64::from(u32::MAX)) as u32);
    let p_skip = m.p_skip_new(&moment, v.as_ref(), duration, EventContext::HomeSection);
    let near = v.as_ref().map_or(0.0, |v| m.dislike_near_of(v));
    Some((W_REGION_FIT * fit + W_KNOWN_ARTIST * known - W_SKIP_NEW * p_skip - near - penalty, profile))
}

/// [`rank_discoveries`] scored with the user model too (`scoring`).
pub fn rank_discoveries_with(seeds: &DiscoverySeeds, finds: &[SeedFinds], filter: &DiscoveryFilter, scoring: Option<&NewScoring<'_>>) -> Vec<Discovery> {
    rank_discoveries_upto(seeds, finds, filter, scoring, DISCOVERIES)
}

/// [`rank_discoveries_with`] keeping the best `keep` (the home shows the
/// first [`DISCOVERIES`] and more of them at the end of its row).
pub fn rank_discoveries_upto(
    seeds: &DiscoverySeeds,
    finds: &[SeedFinds],
    filter: &DiscoveryFilter,
    scoring: Option<&NewScoring<'_>>,
    keep: usize,
) -> Vec<Discovery> {
    let all_tracks: Vec<&StreamTrack> = finds.iter().flat_map(|f| f.tracks.iter()).collect();
    let mut by_isrc: HashMap<String, String> = HashMap::new();
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
            let canonical = crate::junk::canonical_in(t, all_tracks.iter().copied());
            if crate::junk::junk(t, canonical.as_ref()).is_some() {
                continue;
            }
            // One song under two titles: the same ISRC.
            let key = match t.isrc.as_ref() {
                Some(isrc) => by_isrc.entry(isrc.clone()).or_insert(key).clone(),
                None => key,
            };
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
        .filter_map(|(key, c)| {
            let artist = c.track.artists.first().map(|a| artist_key(&a.name)).unwrap_or_default();
            let new_artist = !filter.known_artists.contains(&artist);
            // The services' own suggestions count as one seed.
            let services = c.seeds.keys().any(|s| matches!(s, SeedRef::Service(_)));
            let independent = c.seeds.keys().filter(|s| !matches!(s, SeedRef::Service(_))).count() + usize::from(services);
            let mut score: f32 = c.seeds.values().sum::<f32>() * multi_seed(independent);
            if new_artist {
                score += NEW_ARTIST_BONUS;
            }
            let mut profile = None;
            if let Some(sc) = scoring {
                let (extra, p) = model_score(&c.track, &key, &c.seeds, seeds, sc)?;
                score += extra;
                profile = p;
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
            Some((
                Discovery {
                    votes: c.seeds.len() as u32,
                    track: c.track,
                    key,
                    score,
                    via,
                    new_artist,
                    profile,
                },
                c.first,
            ))
        })
        .collect();
    out.sort_by(|a, b| b.0.score.total_cmp(&a.0.score).then(a.1.cmp(&b.1)));
    // Every profile its share of the discoveries.
    if let Some(sc) = scoring
        && !sc.model.profile_share.is_empty()
    {
        let items: Vec<((Discovery, usize), Option<usize>)> = out.iter().map(|d| (d.clone(), d.0.profile)).collect();
        let n = items.len().min(keep * 2);
        out = crate::regions::spread(&items, &sc.model.profile_share, n);
    }


    // At most two per artist; no artist twice within three. A pool past
    // the first ones takes more of an artist after every artist's first.
    let cap = if keep > DISCOVERIES { POOL_PER_ARTIST } else { PER_ARTIST };
    let mut per: HashMap<String, usize> = HashMap::new();
    let mut pool: Vec<Discovery> = Vec::new();
    let mut more: Vec<Discovery> = Vec::new();
    for (d, _) in out {
        let a = artist_key(&d.track.artists[0].name);
        let c = per.entry(a).or_default();
        if *c < PER_ARTIST {
            pool.push(d);
        } else if *c < cap {
            more.push(d);
        }
        *c += 1;
    }
    let mut result: Vec<Discovery> = Vec::with_capacity(keep.min(pool.len() + more.len()));
    pick_spread(&mut result, pool, keep);
    pick_spread(&mut result, more, keep);
    result
}

/// Moves `pool` (best first) onto `result` up to `keep`, never an artist
/// within three.
fn pick_spread(result: &mut Vec<Discovery>, mut pool: Vec<Discovery>, keep: usize) {
    while result.len() < keep && !pool.is_empty() {
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
            ..DiscoverySeeds::default()
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
    fn more_discoveries_keep_the_first_ones_in_their_order() {
        let tracks: Vec<StreamTrack> = (0..60).map(|k| st(Service::Youtube, &format!("t{k}"), &format!("Song {k}"), &format!("Artist {k}"))).collect();
        let finds = vec![SeedFinds { seed: SeedRef::Artist(aid(1)), tracks }];
        let filter = DiscoveryFilter::default();
        let first = rank_discoveries(&seeds(), &finds, &filter);
        assert_eq!(first.len(), DISCOVERIES);
        let more = rank_discoveries_upto(&seeds(), &finds, &filter, None, 2 * DISCOVERIES);
        assert_eq!(more.len(), 2 * DISCOVERIES);
        let titles = |d: &[Discovery]| d.iter().map(|d| d.track.title.clone()).collect::<Vec<_>>();
        assert_eq!(titles(&more[..DISCOVERIES]), titles(&first));
    }

    /// A song three seeds lead to stands far above the lone answers of the
    /// most loved seed; the services' suggestions count as one seed.
    #[test]
    fn independent_seeds_multiply() {
        assert_eq!(multi_seed(1), 1.0);
        assert!(multi_seed(3) > multi_seed(2) && multi_seed(2) > 1.0);
        assert_eq!(multi_seed(9), multi_seed(3));
        let mut finds = vec![SeedFinds {
            seed: SeedRef::Track(tid(1)),
            tracks: (0..5).map(|k| st(Service::Youtube, &format!("l{k}"), &format!("Lone {k}"), &format!("Lone {k}"))).collect(),
        }];
        for s in [2, 3] {
            finds.push(SeedFinds { seed: SeedRef::Track(tid(s)), tracks: vec![st(Service::Youtube, "x", "Shared", "Bridge")] });
        }
        finds[0].tracks.push(st(Service::Youtube, "x", "Shared", "Bridge"));
        let out = rank_discoveries(&seeds(), &finds, &DiscoveryFilter::default());
        assert_eq!(out[0].track.title, "Shared");
        assert!(out[0].score > 3.0 * out[1].score, "{} vs {}", out[0].score, out[1].score);
        // Two services suggesting a song are one seed.
        let two = vec![
            SeedFinds { seed: SeedRef::Service(Service::Youtube), tracks: vec![st(Service::Youtube, "s", "Suggested", "S")] },
            SeedFinds { seed: SeedRef::Service(Service::Deezer), tracks: vec![st(Service::Deezer, "9", "Suggested", "S")] },
        ];
        let out = rank_discoveries(&seeds(), &two, &DiscoveryFilter::default());
        assert!((out[0].score - 2.0 * (1.0 + RANK_BONUS) * crate::suggest::SERVICE_VOTE - NEW_ARTIST_BONUS).abs() < 1e-4, "{}", out[0].score);
    }

    /// A pool larger than the home's row goes on past it with up to four
    /// songs of an artist, after every artist's first two.
    #[test]
    fn a_large_pool_takes_more_of_an_artist_after_the_others() {
        let mut tracks = Vec::new();
        for a in 0..10 {
            for k in 0..6 {
                tracks.push(st(Service::Youtube, &format!("{a}-{k}"), &format!("Song {a} {k}"), &format!("Artist {a}")));
            }
        }
        let finds = vec![SeedFinds { seed: SeedRef::Artist(aid(1)), tracks }];
        let out = rank_discoveries_upto(&seeds(), &finds, &DiscoveryFilter::default(), None, 400);
        assert_eq!(out.len(), 40, "four of each of ten artists");
        let firsts: HashSet<String> = out[..20].iter().map(|d| d.track.artists[0].name.clone()).collect();
        assert_eq!(firsts.len(), 10, "every artist's first two before anyone's third");
        let head = rank_discoveries(&seeds(), &finds, &DiscoveryFilter::default());
        assert_eq!(head.len(), 20);
        assert_eq!(head.iter().map(|d| &d.key).collect::<Vec<_>>(), out[..20].iter().map(|d| &d.key).collect::<Vec<_>>());
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
                    context: Default::default(),
                    origin: None,
                    end_reason: None,
                    attention: None,
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
                    album("3", "Too old", Some(100), ReleaseType::Album),
                    album("4", "Year only", None, ReleaseType::Album),
                    album("5", "Owned", Some(5), ReleaseType::Album),
                    album("6", "Single", Some(4), ReleaseType::Single),
                    album("8", "A fourth", Some(80), ReleaseType::Single),
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
        assert_eq!(sorted, ["Fresh", "Older", "Other band", "Single"], "three per artist, dated within three months, unknown: {titles:?}");
        let pos = |t: &str| titles.iter().position(|x| *x == t).unwrap();
        assert!(pos("Fresh") < pos("Single"), "{titles:?}");
    }

    /// Seeds from every taste profile, not only the one played most: a
    /// listener of much rap and a few soundtracks, Japanese pop and rock
    /// gets seeds of each, the soundtracks' too.
    #[test]
    fn seeds_cover_regions() {
        use crate::model::{ModelInput, UserModel};
        let (cat, idx, input, types) = crate::radio::tests::listener(true);
        let now = crate::model::tests::T0;
        let model = UserModel::build(&cat, &input, ModelInput { index: &idx, types: Some(&types), weights: None, graph: None }, now);
        let recs = Recs::new(&cat, &model.taste, &idx).with_model(&model);
        let s = recs.discovery_seeds();
        let kinds: HashSet<u128> = s.tracks.iter().map(|(t, _)| t.0.as_u128() / 1_000).collect();
        assert_eq!(kinds, [0, 1, 2, 3].into(), "{:?}", s.tracks);
        assert!(s.tracks.len() <= SEED_TRACKS_MAX);
        let rap = s.tracks.iter().filter(|(t, _)| t.0.as_u128() < 1_000).count();
        assert!(rap * 2 <= s.tracks.len() + 2, "rap does not fill the seeds: {rap} of {}", s.tracks.len());
        // Without the model: the best loved alone, as before.
        let plain = Recs::new(&cat, &model.taste, &idx).discovery_seeds();
        assert_eq!(plain.tracks.len(), SEED_TRACKS);
    }

    /// A user whose library is empty but who saved songs in a service's
    /// library: those songs (some each day, all in turn) and their artists
    /// seed the new music and the new releases. A user without them has
    /// the seeds of the library alone.
    #[test]
    fn songs_saved_elsewhere_seed_the_new_music() {
        let now = 1_000 * DAY_MS;
        let cat = Catalog::new(std::iter::empty());
        let idx = VectorIndex::new();
        let elsewhere: Vec<(TrackId, Vec<ArtistId>)> = (0..10).map(|i| (tid(100 + i), vec![aid(50 + (i % 3) as u32)])).collect();
        let input = TasteInput { saved_elsewhere: elsewhere.clone(), ..Default::default() };
        let mut seen: HashSet<TrackId> = HashSet::new();
        for day in 0..3 {
            let taste = Taste::build(&cat, &input, now + day * DAY_MS);
            let s = Recs::new(&cat, &taste, &idx).discovery_seeds();
            assert_eq!(s.tracks.len(), ELSEWHERE_SEEDS);
            assert!(s.tracks.iter().all(|(t, a)| elsewhere.iter().any(|e| e.0 == *t) && *a > 0.0));
            let mut artists: Vec<ArtistId> = s.artists.iter().map(|a| a.0).collect();
            artists.sort();
            assert_eq!(artists, [aid(50), aid(51), aid(52)]);
            seen.extend(s.tracks.iter().map(|t| t.0));
            let mut releases = Recs::new(&cat, &taste, &idx).release_artists();
            releases.sort();
            assert_eq!(releases, [aid(50), aid(51), aid(52)]);
        }
        assert_eq!(seen.len(), 10, "every song in turn over three days");

        // With a library: every other one among the first seeds.
        let cat = Catalog::new((0..5).map(|i| info(i, &[0], Some(i))));
        let events = (0..5)
            .map(|i| Play::new(tid(i), now - DAY_MS, EventType::Complete, None))
            .collect::<Vec<_>>();
        let with = TasteInput { events: events.clone(), saved_elsewhere: elsewhere, ..Default::default() };
        let taste = Taste::build(&cat, &with, now);
        let s = Recs::new(&cat, &taste, &idx).discovery_seeds();
        let ids: Vec<u128> = s.tracks.iter().map(|(t, _)| t.0.as_u128()).collect();
        assert_eq!(ids.len(), 5 + ELSEWHERE_SEEDS);
        assert!(ids[1] >= 100 && ids[3] >= 100 && ids[0] < 100 && ids[2] < 100, "{ids:?}");
        // Without: the library's seeds as they were.
        let taste = Taste::build(&cat, &TasteInput { events, ..Default::default() }, now);
        let s = Recs::new(&cat, &taste, &idx).discovery_seeds();
        assert_eq!(s.tracks.len(), 5);
        assert!(s.tracks.iter().all(|(t, _)| t.0.as_u128() < 100));
        assert_eq!(s.artists.iter().map(|a| a.0).collect::<Vec<_>>(), [aid(0)]);
        let empty = Taste::build(&Catalog::new(std::iter::empty()), &TasteInput::default(), now);
        assert_eq!(Recs::new(&Catalog::new(std::iter::empty()), &empty, &idx).discovery_seeds(), DiscoverySeeds::default());
    }

    /// The seeds go by the profiles' shares, two at least for each active
    /// one, thirty in all.
    #[test]
    fn seeds_follow_shares_with_a_floor() {
        let q = seed_quota(&[0, 1, 2, 3], &[0.7, 0.2, 0.08, 0.02]);
        assert_eq!(q.iter().sum::<usize>(), SEED_TRACKS_MAX);
        assert!(q.iter().all(|n| *n >= SEEDS_PER_PROFILE), "{q:?}");
        assert!(q[0] > q[1] && q[1] > q[2], "{q:?}");
        assert_eq!(seed_quota(&[0, 1], &[1.0, 0.0])[1], 0, "no share, no seeds");
    }

    /// A find is the profile's of its kind of song: a Japanese song that
    /// sounds like the rap is no rap find, a Russian one is.
    #[test]
    fn a_find_is_of_its_kind_of_songs_profile() {
        use crate::model::{ModelInput, UserModel};
        let (cat, idx, input, types) = crate::radio::tests::listener(true);
        let now = crate::model::tests::T0;
        let model = UserModel::build(&cat, &input, ModelInput { index: &idx, types: Some(&types), weights: None, graph: None }, now);
        let (vectors, known, log) = (HashMap::new(), HashMap::new(), NewLog::default());
        let sc = NewScoring { model: &model, vectors: &vectors, log: &log, artist_affinity: &known, now, types: Some(&types) };
        let rap = model.profile_at(cat.position(tid(5)).unwrap()).unwrap();
        let near_rap = *model.embedded_at(cat.position(tid(5)).unwrap()).unwrap();
        let ja = st(Service::Youtube, "ja", "東京の夜に歌う", "誰か");
        let ru = st(Service::Youtube, "ru", "Ночной город", "Кто-то");
        assert_ne!(find_profile(&sc, &ja, Some(&near_rap), Some(rap)), Some(rap));
        assert_eq!(find_profile(&sc, &ru, Some(&near_rap), None), Some(rap));
        assert_eq!(find_profile(&sc, &ja, None, Some(rap)), None, "the seed's profile only of its kind");
    }

    /// With the model: a junk upload never comes, an artist skipped twice
    /// stays out, the small profile's discoveries are among the best.
    #[test]
    fn discoveries_with_the_model_spread_and_filter() {
        use crate::model::{ModelInput, UserModel};
        use crate::novelty::{Heard, NewEvent, NewLog};
        let (cat, idx, input, types) = crate::radio::tests::listener(true);
        let now = crate::model::tests::T0;
        let model = UserModel::build(&cat, &input, ModelInput { index: &idx, types: Some(&types), weights: None, graph: None }, now);
        let recs = Recs::new(&cat, &model.taste, &idx).with_model(&model);
        let seeds = recs.discovery_seeds();
        // Each seed finds five songs of new artists of its own.
        let mut finds = Vec::new();
        for (t, _) in &seeds.tracks {
            let k = t.0.as_u128();
            let tracks = (0..5).map(|n| st(Service::Youtube, &format!("{k}-{n}"), &format!("Song {k} {n}"), &format!("Artist {k} {n}"))).collect();
            finds.push(SeedFinds { seed: SeedRef::Track(*t), tracks });
        }
        let seed0 = seeds.tracks[0].0.0.as_u128();
        let mut nightcore = st(Service::Youtube, "nc", "Song X (Nightcore)", "Nightcore Hub");
        nightcore.kind = TrackKind::Video;
        finds[0].tracks.insert(0, nightcore);
        let mut log = NewLog::default();
        for at in [now - DAY_MS, now - 2 * DAY_MS] {
            log.push(NewEvent {
                song: "x".into(),
                artist: artist_key(&format!("Artist {seed0} 0")),
                source: None,
                vector: None,
                track: None,
                at,
                heard: Heard::Early,
                profile: None,
                background: false,
            });
        }
        let vectors = HashMap::new();
        let known = HashMap::new();
        let sc = NewScoring { model: &model, vectors: &vectors, log: &log, artist_affinity: &known, now, types: Some(&types) };
        let out = rank_discoveries_with(&seeds, &finds, &DiscoveryFilter::default(), Some(&sc));
        assert!(!out.iter().any(|d| d.track.title.contains("Nightcore")));
        assert!(!out.iter().any(|d| d.track.artists[0].name == format!("Artist {seed0} 0")), "skipped twice: out for a month");
        let ost = out.iter().filter(|d| seeds.tracks.iter().any(|(t, _)| d.track.title.starts_with(&format!("Song {}", t.0.as_u128())) && (1_000..2_000).contains(&t.0.as_u128()))).count();
        assert!(ost >= 2, "the soundtracks' discoveries come too: {ost} of {}", out.len());
        assert!(out.iter().all(|d| d.profile.is_some()));
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
