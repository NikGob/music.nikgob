//! Listeners made up for the evaluation, each with a hidden taste the
//! engine never sees ([`super::Dataset::truth`]).
//!
//! The library: kinds of music (sung in Russian, in Japanese, in English
//! twice, game soundtracks of franchises, ambient, electronic, classical),
//! each a region of sound with its artists around it and their albums
//! around them. A listener loves some kinds, some artists of them more,
//! some songs most; a song's appeal is all of that (0…1). Sessions start
//! at the listener's hours: an album in order, songs picked one by one
//! (one like the last, a favourite of the week again), a playlist leafed
//! through; whether a start is heard out or skipped early follows its
//! appeal. A listener who keeps music on in the background has long runs
//! of autoplay heard out with no action. Likes follow songs heard out
//! often. One listener brought a history from another service: imported
//! plays and likes of an older taste. One listens to rap and has three calm
//! songs of one artist (docs/recommendations.md §1.2): the open pool it
//! carries holds calm new songs linked to them, rap new songs (some of them
//! on the calm songs' radios) and songs of one of its rappers.

use std::collections::HashMap;

use nikgob_core::model::{ArtistId, EventContext, EventOrigin, EventType, ReleaseId, TrackId, UnixMs, Uuid};

use super::{DNew, DPlay, DTrack, Dataset};
use nikgob_core::streaming::{ArtistRef, RemoteId, Service, StreamTrack};
use crate::analysis::DIM;
use crate::rng::Rng;
use crate::songtype::{Lang, SongType, SongTypes, Voice, VoiceSource};
use crate::taste::{DAY_MS, HOUR_MS, world_token};

/// Who listens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// One kind of music, little else.
    OneGenre,
    /// Everything, by the hour: songs by day, calm music at night.
    Omnivore,
    /// Songs by hand, and hours of ambient and soundtracks on in the
    /// background.
    Background,
    /// Leafs through playlists and skips a lot.
    Skipper,
    /// Ten events.
    Cold,
    /// A big library and 135 000 events over two years (the timing).
    Big,
    /// Brought a long history from another service (some 5 000 imported
    /// events of an older taste and 300 of its songs liked at once at the
    /// import) and listens to other music here.
    Imported,
    /// Rap, and a calm niche of three songs of one artist (the open world).
    NicheCalm,
}

impl Kind {
    pub const ALL: [Self; 8] = [Self::OneGenre, Self::Omnivore, Self::Background, Self::Skipper, Self::Cold, Self::Big, Self::Imported, Self::NicheCalm];

    pub fn name(self) -> &'static str {
        match self {
            Self::OneGenre => "synthetic: one kind of music",
            Self::Omnivore => "synthetic: omnivore",
            Self::Background => "synthetic: background listener",
            Self::Skipper => "synthetic: skipper",
            Self::Cold => "synthetic: cold start (10 events)",
            Self::Big => "synthetic: big (135k events)",
            Self::Imported => "synthetic: imported history",
            Self::NicheCalm => "synthetic: rap and a calm niche of three songs",
        }
    }
}

/// The kinds of music of the library.
const GENRES: usize = 8;
/// Russian, Japanese, English, English rock, game soundtracks, ambient,
/// electronic, classical.
const SUNG: [Option<Lang>; GENRES] = [Some(Lang::Russian), Some(Lang::Japanese), Some(Lang::English), Some(Lang::English), None, None, None, None];
const VOICE: [Voice; GENRES] = [Voice::Sung, Voice::Sung, Voice::Sung, Voice::Sung, Voice::Instrumental, Voice::Instrumental, Voice::Unknown, Voice::Instrumental];
const OST: usize = 4;
const AMBIENT: usize = 5;

fn uuid(kind: u128, i: u128) -> Uuid {
    Uuid::from_u128((kind << 96) | i)
}

fn gauss(rng: &mut Rng) -> f32 {
    // Irwin–Hall: close enough to a normal for made-up sound.
    (0..6).map(|_| rng.f32()).sum::<f32>() - 3.0
}

/// One track of the made-up library.
struct Song {
    id: TrackId,
    genre: usize,
    artist: usize,
    album: usize,
    duration: u32,
    vector: [f32; DIM],
}

struct Library {
    songs: Vec<Song>,
    tracks: Vec<DTrack>,
    /// The tracks of each album in order, of each artist.
    albums: Vec<Vec<usize>>,
    artist_albums: Vec<Vec<usize>>,
    artist_genre: Vec<usize>,
}

fn library(rng: &mut Rng, artists_per_genre: usize, albums_per_artist: usize, tracks_per_album: (usize, usize)) -> Library {
    let centres: Vec<[f32; DIM]> = (0..GENRES).map(|_| std::array::from_fn(|_| 1.6 * gauss(rng))).collect();
    let mut songs = Vec::new();
    let mut tracks = Vec::new();
    let mut albums: Vec<Vec<usize>> = Vec::new();
    let mut artist_albums: Vec<Vec<usize>> = Vec::new();
    let mut artist_genre = Vec::new();
    for g in 0..GENRES {
        for _ in 0..artists_per_genre {
            let a = artist_genre.len();
            artist_genre.push(g);
            let at: [f32; DIM] = std::array::from_fn(|d| centres[g][d] + 0.6 * gauss(rng));
            let mut mine = Vec::new();
            for k in 0..albums_per_artist {
                let album = albums.len();
                mine.push(album);
                let n = tracks_per_album.0 + rng.below(tracks_per_album.1 - tracks_per_album.0 + 1);
                // A soundtrack's franchise: two albums of one composer.
                let world = if g == OST { format!("franchise {a} {}", k / 2) } else { format!("artist:{a}") };
                let mut list = Vec::new();
                for p in 0..n {
                    let i = songs.len();
                    let id = TrackId(uuid(1, i as u128));
                    let calm = matches!(g, OST | AMBIENT | 7);
                    let duration = if rng.f32() < 0.04 {
                        40_000 + rng.below(30_000) as u32
                    } else if calm {
                        150_000 + rng.below(330_000) as u32
                    } else {
                        140_000 + rng.below(140_000) as u32
                    };
                    let vector = std::array::from_fn(|d| at[d] + 0.45 * gauss(rng));
                    songs.push(Song { id, genre: g, artist: a, album, duration, vector });
                    tracks.push(DTrack {
                        id,
                        artists: vec![ArtistId(uuid(2, a as u128))],
                        release: Some(ReleaseId(uuid(3, album as u128))),
                        local: true,
                        song: None,
                        world: Some(world_token(&world)),
                        duration_ms: Some(duration),
                        position: 1 + p as u32,
                    });
                    list.push(i);
                }
                albums.push(list);
            }
            artist_albums.push(mine);
        }
    }
    Library { songs, tracks, albums, artist_albums, artist_genre }
}

/// The hidden taste: how much each kind and artist is loved, and the
/// appeal of each song.
struct Taste {
    appeal: Vec<f32>,
    /// What plays in the background (a background listener's).
    background: Vec<f32>,
    /// How much each kind is wanted at each part of the day (0 morning, 1
    /// day, 2 evening, 3 night).
    hours: [[f32; GENRES]; 4],
}

fn taste(rng: &mut Rng, lib: &Library, genres: [f32; GENRES], background: [f32; GENRES], hours: [[f32; GENRES]; 4]) -> Taste {
    let artist: Vec<f32> = lib.artist_genre.iter().map(|_| (0.7 * gauss(rng)).exp()).collect();
    let raw = |g: &[f32; GENRES], rng: &mut Rng| -> Vec<f32> {
        let mut v: Vec<f32> = lib.songs.iter().map(|s| g[s.genre] * artist[s.artist] * (0.45 * gauss(rng)).exp()).collect();
        let top = v.iter().copied().fold(1e-6f32, f32::max);
        for x in &mut v {
            *x = (*x / top).sqrt();
        }
        v
    };
    let appeal = raw(&genres, &mut *rng);
    let background = if background.iter().any(|x| *x > 0.0) { raw(&background, &mut *rng) } else { vec![0.0; lib.songs.len()] };
    Taste { appeal, background, hours }
}

struct Listener {
    /// The appeal at which a start is heard out half the time, and how
    /// sharply it turns.
    threshold: f32,
    sharp: f32,
    /// Early skips beyond the appeal (leafing).
    leaf: f32,
    /// Shares of the kinds of session: an album, songs one by one, a
    /// playlist.
    modes: [f32; 3],
    /// Sessions a day and their length (mean).
    per_day: f32,
    length: f32,
    /// The share of sessions in the background.
    background: f32,
    /// The hours sessions start at, by part of the day.
    parts: [f32; 4],
}

fn pick_weighted(rng: &mut Rng, items: &[(usize, f32)]) -> Option<usize> {
    let total: f32 = items.iter().map(|x| x.1.max(0.0)).sum();
    if total <= 0.0 {
        return items.first().map(|x| x.0);
    }
    let mut r = rng.f32() * total;
    for (i, w) in items {
        r -= w.max(0.0);
        if r <= 0.0 {
            return Some(*i);
        }
    }
    items.last().map(|x| x.0)
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// The events of a listener over `days` up to `end`, `max_starts` at most.
#[allow(clippy::too_many_arguments)]
fn history(rng: &mut Rng, lib: &Library, t: &Taste, who: &Listener, days: i64, end: UnixMs, max_starts: usize) -> (Vec<DPlay>, Vec<(TrackId, UnixMs)>, Vec<Vec<TrackId>>) {
    let n = lib.songs.len();
    let mut events: Vec<DPlay> = Vec::new();
    let mut likes: Vec<(TrackId, UnixMs)> = Vec::new();
    let mut completes = vec![0u32; n];
    let mut recent: Vec<(usize, UnixMs)> = Vec::new();
    // A playlist of loved songs and a few others.
    let mut by_appeal: Vec<usize> = (0..n).collect();
    by_appeal.sort_by(|a, b| t.appeal[*b].total_cmp(&t.appeal[*a]));
    let playlist: Vec<usize> = by_appeal.iter().take(40).copied().chain(by_appeal.iter().skip(40).step_by(9).take(20).copied()).collect();
    // Sound neighbours within a kind, for picks like the last.
    let near = |i: usize, rng: &mut Rng| -> Vec<usize> {
        let g = lib.songs[i].genre;
        let drawn: Vec<(usize, f32)> = (0..24).map(|_| (rng.below(n), rng.f32())).collect();
        let mut c: Vec<(f32, usize)> = drawn
            .into_iter()
            .chain(lib.albums[lib.songs[i].album].iter().map(|&j| (j, 1.0)))
            .filter(|&(j, chance)| j != i && (lib.songs[j].genre == g || chance < 0.1))
            .map(|(j, _)| j)
            .map(|j| (crate::index::euclid(&lib.songs[i].vector, &lib.songs[j].vector), j))
            .collect();
        c.sort_by(|a, b| a.0.total_cmp(&b.0));
        c.into_iter().take(12).map(|x| x.1).collect()
    };
    let mut starts = 0usize;
    for day in 0..days {
        let day_start = end - (days - day) * DAY_MS;
        let sessions = {
            let mut k = 0;
            let mut p = who.per_day;
            while p > 0.0 {
                if rng.f32() < p.min(1.0) {
                    k += 1;
                }
                p -= 1.0;
            }
            k
        };
        let mut hours: Vec<i64> = (0..sessions)
            .map(|_| {
                let part = pick_weighted(rng, &[(0, who.parts[0]), (1, who.parts[1]), (2, who.parts[2]), (3, who.parts[3])]).unwrap_or(1);
                let (from, len) = [(6, 5), (11, 7), (18, 5), (23, 7)][part];
                (from + rng.below(len) as i64) % 24
            })
            .collect();
        hours.sort_unstable();
        let mut free_from = day_start;
        for h in hours {
            let mut ts = (day_start + h * HOUR_MS + rng.below(3_000_000) as i64).max(free_from + 40 * 60_000);
            let part = match h {
                6..=10 => 0,
                11..=17 => 1,
                18..=22 => 2,
                _ => 3,
            };
            let want = |i: usize| t.appeal[i] * t.hours[part][lib.songs[i].genre];
            let background = rng.f32() < who.background;
            let len = if background { 30 + rng.below(50) } else { 2 + (who.length * -(1.0 - rng.f32()).max(1e-6).ln()) as usize };
            let mode = if background { 3 } else { pick_weighted(rng, &[(0, who.modes[0]), (1, who.modes[1]), (2, who.modes[2])]).unwrap_or(1) };
            let mut queue: Vec<usize> = Vec::new();
            let mut current: Option<usize> = None;
            let mut shuffled = playlist.clone();
            rng.shuffle(&mut shuffled);
            for k in 0..len {
                if starts >= max_starts {
                    break;
                }
                let (i, context, origin) = match mode {
                    0 => {
                        if queue.is_empty() {
                            let artists: Vec<(usize, f32)> = (0..lib.artist_albums.len())
                                .map(|a| (a, lib.artist_albums[a].iter().flat_map(|&al| &lib.albums[al]).map(|&i| want(i)).sum::<f32>()))
                                .collect();
                            let a = pick_weighted(rng, &artists).unwrap_or(0);
                            let al = lib.artist_albums[a][rng.below(lib.artist_albums[a].len())];
                            queue = lib.albums[al].iter().rev().copied().collect();
                            let i = queue.pop().unwrap_or(0);
                            (i, EventContext::Album, EventOrigin::Manual)
                        } else {
                            (queue.pop().unwrap_or(0), EventContext::Album, EventOrigin::Queue)
                        }
                    }
                    2 => (shuffled[k % shuffled.len()], EventContext::Playlist, if k == 0 { EventOrigin::Manual } else { EventOrigin::Queue }),
                    3 => {
                        // The background: the app goes on in calm music.
                        let pool: Vec<(usize, f32)> = match current {
                            Some(c) => near(c, &mut *rng).into_iter().map(|j| (j, t.background[j].powi(2))).collect(),
                            None => (0..60).map(|_| rng.below(n)).map(|j| (j, t.background[j].powi(3))).collect(),
                        };
                        let i = pick_weighted(rng, &pool).unwrap_or(0);
                        (i, EventContext::Radio, if k == 0 { EventOrigin::Manual } else { EventOrigin::Autoplay })
                    }
                    _ => {
                        let again: Vec<(usize, f32)> = recent.iter().filter(|(_, at)| ts - at < 7 * DAY_MS).map(|(i, _)| (*i, want(*i).powi(2))).collect();
                        let i = if current.is_some() && !again.is_empty() && rng.f32() < 0.25 {
                            pick_weighted(rng, &again).unwrap_or(0)
                        } else if let Some(c) = current.filter(|_| rng.f32() < 0.7) {
                            let pool: Vec<(usize, f32)> = near(c, &mut *rng).into_iter().map(|j| (j, want(j).powi(2))).collect();
                            pick_weighted(rng, &pool).unwrap_or(0)
                        } else {
                            let pool: Vec<(usize, f32)> = (0..80).map(|_| rng.below(n)).map(|j| (j, want(j).powi(3))).collect();
                            pick_weighted(rng, &pool).unwrap_or(0)
                        };
                        let context = [EventContext::Library, EventContext::Search, EventContext::Artist, EventContext::HomeSection][rng.below(4)];
                        (i, context, EventOrigin::Manual)
                    }
                };
                current = Some(i);
                let s = &lib.songs[i];
                events.push(DPlay {
                    track: s.id,
                    ts,
                    kind: EventType::Start,
                    position_ms: Some(0),
                    context,
                    origin: Some(origin),
                    end_reason: None,
                    attention: None,
                });
                starts += 1;
                let u = if mode == 3 { t.background[i].max(t.appeal[i]) } else { t.appeal[i] };
                let leaf = if mode == 2 { who.leaf } else { who.leaf * 0.3 };
                let p_early = if mode == 3 { 0.03 } else { (sigmoid(-who.sharp * (u - who.threshold)) + leaf).min(0.95) };
                let (kind, pos) = if rng.f32() < p_early {
                    (EventType::Skip, 3_000 + rng.below(24_000) as u32)
                } else if mode != 3 && rng.f32() < 0.1 {
                    (EventType::Skip, (s.duration as f32 * (0.3 + 0.5 * rng.f32())) as u32)
                } else {
                    (EventType::Complete, s.duration)
                };
                ts += i64::from(pos) + 1_000 + rng.below(4_000) as i64;
                events.push(DPlay {
                    track: s.id,
                    ts,
                    kind,
                    position_ms: Some(pos),
                    context,
                    origin: None,
                    end_reason: None,
                    attention: if mode == 3 { Some(1.0) } else { None },
                });
                if kind == EventType::Complete {
                    completes[i] += 1;
                    recent.push((i, ts));
                    if recent.len() > 400 {
                        recent.drain(..100);
                    }
                    if completes[i] == 3 && t.appeal[i] > 0.6 && rng.f32() < 0.6 {
                        likes.push((s.id, ts));
                        events.push(DPlay { track: s.id, ts, kind: EventType::Like, position_ms: None, context, origin: None, end_reason: None, attention: None });
                    }
                }
            }
            free_from = ts;
        }
    }
    let playlists = vec![playlist.iter().map(|&i| lib.songs[i].id).collect()];
    (events, likes, playlists)
}

/// A listener of `kind`, made up from `seed`.
pub fn user(kind: Kind, seed: u64) -> Dataset {
    let mut rng = Rng::new(seed ^ 0x5eed);
    let end: UnixMs = 20_400 * DAY_MS;
    let lib = match kind {
        Kind::Big => library(&mut rng, 110, 2, (8, 14)),
        _ => library(&mut rng, 10, 2, (8, 14)),
    };
    let flat = [[1.0; GENRES]; 4];
    let (genres, background, hours) = match kind {
        Kind::OneGenre | Kind::Cold => {
            let g = (seed % 4) as usize;
            let mut v = [0.03; GENRES];
            v[g] = 1.0;
            (v, [0.0; GENRES], flat)
        }
        Kind::Omnivore | Kind::Big => {
            let v: [f32; GENRES] = std::array::from_fn(|_| 0.5 + 0.5 * rng.f32());
            // Songs by day, calm music at night.
            let mut hours = flat;
            for g in 0..GENRES {
                let calm = !matches!(VOICE[g], Voice::Sung);
                hours[3][g] = if calm { 1.6 } else { 0.4 };
                hours[1][g] = if calm { 0.6 } else { 1.3 };
            }
            (v, [0.0; GENRES], hours)
        }
        Kind::Background => {
            let mut v = [0.05; GENRES];
            v[2] = 1.0;
            v[3] = 0.8;
            v[0] = 0.4;
            let mut bg = [0.0; GENRES];
            bg[AMBIENT] = 1.0;
            bg[OST] = 0.8;
            (v, bg, flat)
        }
        Kind::Skipper => {
            let mut v = [0.08; GENRES];
            v[1] = 1.0;
            v[6] = 0.9;
            v[2] = 0.3;
            (v, [0.0; GENRES], flat)
        }
        Kind::NicheCalm => {
            // Russian rap, some Japanese songs; the calm songs set below.
            let mut v = [0.02; GENRES];
            v[0] = 1.0;
            v[1] = 0.3;
            (v, [0.0; GENRES], flat)
        }
        Kind::Imported => {
            // Now: Japanese songs, English rock, some ambient.
            let mut v = [0.05; GENRES];
            v[1] = 1.0;
            v[3] = 0.7;
            v[AMBIENT] = 0.4;
            (v, [0.0; GENRES], flat)
        }
    };
    let mut lib = lib;
    let mut t = taste(&mut rng, &lib, genres, background, hours);
    // The calm niche: the first ambient artist's first three songs, far
    // quieter than anything else, loved.
    let calm: Vec<usize> = if kind == Kind::NicheCalm { lib.albums[lib.artist_albums[AMBIENT * 10][0]].iter().take(3).copied().collect() } else { Vec::new() };
    for &i in &calm {
        for (d, x) in lib.songs[i].vector.iter_mut().enumerate() {
            *x -= CALM_SHIFT * (1.0 + 0.1 * d as f32 / DIM as f32);
        }
        t.appeal[i] = 0.9;
    }
    let who = match kind {
        Kind::Skipper => Listener { threshold: 0.5, sharp: 9.0, leaf: 0.25, modes: [0.15, 0.35, 0.5], per_day: 1.6, length: 14.0, background: 0.0, parts: [0.2, 0.4, 0.3, 0.1] },
        Kind::Background => Listener { threshold: 0.3, sharp: 7.0, leaf: 0.05, modes: [0.35, 0.5, 0.15], per_day: 1.8, length: 9.0, background: 0.4, parts: [0.2, 0.4, 0.3, 0.1] },
        Kind::Big => Listener { threshold: 0.3, sharp: 7.0, leaf: 0.05, modes: [0.35, 0.45, 0.2], per_day: 8.0, length: 11.0, background: 0.0, parts: [0.15, 0.4, 0.3, 0.15] },
        _ => Listener { threshold: 0.3, sharp: 7.0, leaf: 0.05, modes: [0.35, 0.45, 0.2], per_day: 1.6, length: 11.0, background: 0.0, parts: [0.15, 0.35, 0.3, 0.2] },
    };
    let (days, max_starts) = match kind {
        Kind::Cold => (3, 5),
        Kind::Big => (730, 67_500),
        _ => (60, usize::MAX),
    };
    let (mut events, mut like_times, playlists) = history(&mut rng, &lib, &t, &who, days, end, max_starts);
    if kind == Kind::Imported {
        // The other service's history: an older taste (Russian and English
        // songs, soundtracks) over five months before, imported the day before
        // the own listening began, its 300 favourites liked at once then.
        let mut before = [0.05; GENRES];
        before[0] = 1.0;
        before[2] = 0.8;
        before[OST] = 0.5;
        let past = taste(&mut rng, &lib, before, [0.0; GENRES], flat);
        let at = end - (days + 1) * DAY_MS;
        let (old, _, _) = history(&mut rng, &lib, &past, &who, 150, at, 2_500);
        events.extend(old.into_iter().filter(|e| e.kind != EventType::Like).map(|e| DPlay { origin: Some(EventOrigin::Import), ..e }));
        let mut fav: Vec<usize> = (0..lib.songs.len()).collect();
        fav.sort_by(|a, b| past.appeal[*b].total_cmp(&past.appeal[*a]).then(a.cmp(b)));
        let liked: std::collections::HashSet<TrackId> = like_times.iter().map(|l| l.0).collect();
        for (k, &i) in fav.iter().filter(|&&i| !liked.contains(&lib.songs[i].id)).take(300).enumerate() {
            like_times.push((lib.songs[i].id, at + k as i64 * 1_000));
        }
        events.sort_by_key(|e| e.ts);
    }
    if kind == Kind::NicheCalm {
        return niche_calm(&mut rng, lib, &t, &calm, events, like_times, playlists, end);
    }
    let mut by_track: HashMap<TrackId, SongType> = HashMap::new();
    for s in &lib.songs {
        let voice = VOICE[s.genre];
        by_track.insert(
            s.id,
            SongType {
                voice,
                voice_conf: if voice == Voice::Unknown { 0.0 } else { 0.9 },
                voice_by: if voice == Voice::Unknown { VoiceSource::Nothing } else { VoiceSource::Lyrics },
                lang: SUNG[s.genre],
                ost: s.genre == OST,
                ..SongType::default()
            },
        );
    }
    Dataset {
        name: kind.name().to_owned(),
        vectors: lib.songs.iter().map(|s| (s.id, s.vector.to_vec())).collect(),
        tracks: lib.tracks,
        types: SongTypes { by_track, artist_langs: HashMap::new() },
        likes: like_times.iter().map(|l| l.0).collect(),
        like_times,
        playlist_entries: playlists.iter().flatten().map(|id| (*id, end - 61 * DAY_MS)).collect(),
        playlists,
        events,
        utc_offset_min: 0,
        personal_graph: true,
        library_prior: true,
        dislike_neighbours: true,
        truth: lib.songs.iter().enumerate().map(|(i, s)| (s.id, t.appeal[i].max(t.background[i]))).collect(),
        ..Dataset::default()
    }
}

/// How much quieter the calm niche's songs and new songs are than their
/// kind (raw units of each feature).
const CALM_SHIFT: f32 = 2.5;
/// Where bliss keeps the loudness.
const LOUDNESS: usize = 8;

/// The song types of `songs` by their kinds.
fn types_of<'a>(songs: impl Iterator<Item = &'a Song>) -> HashMap<TrackId, SongType> {
    songs
        .map(|s| {
            let voice = VOICE[s.genre];
            (
                s.id,
                SongType {
                    voice,
                    voice_conf: if voice == Voice::Unknown { 0.0 } else { 0.9 },
                    voice_by: if voice == Voice::Unknown { VoiceSource::Nothing } else { VoiceSource::Lyrics },
                    lang: SUNG[s.genre],
                    ost: s.genre == OST,
                    ..SongType::default()
                },
            )
        })
        .collect()
}

/// The listener of rap with a calm niche of three songs ([`Kind::NicheCalm`]):
/// the library without the other calm kinds, and the open pool.
#[allow(clippy::too_many_arguments)]
fn niche_calm(
    rng: &mut Rng,
    lib: Library,
    t: &Taste,
    calm: &[usize],
    events: Vec<DPlay>,
    like_times: Vec<(TrackId, UnixMs)>,
    playlists: Vec<Vec<TrackId>>,
    end: UnixMs,
) -> Dataset {
    // Loudness as bliss gives it: mastered songs loud, the calm ones quiet.
    let mut lib = lib;
    for (i, s) in lib.songs.iter_mut().enumerate() {
        s.vector[LOUDNESS] = if calm.contains(&i) { 0.5 + 0.03 * gauss(rng) } else { 0.76 + 0.02 * gauss(rng) };
    }
    let quiet = |g: usize| matches!(g, OST | AMBIENT | 7);
    let keep: Vec<bool> = (0..lib.songs.len()).map(|i| !quiet(lib.songs[i].genre) || calm.contains(&i)).collect();
    let kept: std::collections::HashSet<TrackId> = lib.songs.iter().zip(&keep).filter(|(_, k)| **k).map(|(s, _)| s.id).collect();
    let mean = |idx: &[usize]| -> [f32; DIM] {
        let mut c = [0.0f32; DIM];
        for &i in idx {
            for (a, b) in c.iter_mut().zip(lib.songs[i].vector.iter()) {
                *a += b / idx.len() as f32;
            }
        }
        c
    };
    let rap: Vec<usize> = (0..lib.songs.len()).filter(|&i| lib.songs[i].genre == 0).collect();
    let (calm_c, rap_c) = (mean(calm), mean(&rap));
    let id = |i: usize| lib.songs[i].id;
    let new = |rng: &mut Rng, k: usize, title: String, artist: String, centre: Option<&[f32; DIM]>, near: Vec<TrackId>, weight: f32, by: Option<ArtistId>| {
        let mut track = StreamTrack::new(RemoteId::new(Service::Youtube, format!("n{k}")), title);
        track.artists.push(ArtistRef::new(None, artist));
        track.duration_ms = Some(200_000);
        let vector = centre.map(|c| {
            let mut v: Vec<f32> = c.iter().map(|x| x + 0.45 * gauss(rng)).collect();
            v[LOUDNESS] = c[LOUDNESS] + 0.02 * gauss(rng);
            v
        });
        DNew { track, vector, near, weight, artist: by }
    };
    let mut pool = Vec::new();
    // Calm new songs of eight new artists, linked to the calm songs.
    for k in 0..40 {
        let near = vec![id(calm[k % 3]), id(calm[(k + 1) % 3])].into_iter().take(1 + k % 2).collect();
        pool.push(new(rng, k, format!("Drift {k}"), format!("Still Water {}", k % 8), Some(&calm_c), near, 0.9 - 0.005 * k as f32, None));
    }
    // Rap new songs, believed more, linked to the rap.
    for k in 0..30 {
        let near = vec![id(rap[rng.below(rap.len())]), id(rap[rng.below(rap.len())])];
        pool.push(new(rng, 100 + k, format!("Улица {k}"), format!("МС {}", k % 10), Some(&rap_c), near, 0.95 - 0.005 * k as f32, None));
    }
    // Rap on the calm songs' radios.
    for k in 0..6 {
        pool.push(new(rng, 200 + k, format!("Район {k}"), format!("Бит {k}"), Some(&rap_c), vec![id(calm[k % 3])], 0.97, None));
    }
    // A rapper of the library's songs on them, without a preview.
    let rapper = ArtistId(uuid(2, 0));
    for k in 0..4 {
        pool.push(new(rng, 300 + k, format!("Night {k}"), "Rapper".into(), None, vec![id(calm[k % 3])], 0.97, Some(rapper)));
    }
    // The radios of the calm songs name the same songs the library lacks;
    // the rap's name theirs.
    let mut outside = Vec::new();
    for (n, &i) in calm.iter().enumerate() {
        for x in 0..6u32 {
            outside.push((id(i), x + n as u32 % 2, 0.9));
        }
    }
    for (n, &i) in rap.iter().enumerate() {
        outside.push((id(i), 1_000 + (n % 40) as u32, 0.8));
    }
    Dataset {
        name: Kind::NicheCalm.name().to_owned(),
        vectors: lib.songs.iter().filter(|s| kept.contains(&s.id)).map(|s| (s.id, s.vector.to_vec())).collect(),
        tracks: lib.tracks.iter().filter(|x| kept.contains(&x.id)).cloned().collect(),
        types: SongTypes { by_track: types_of(lib.songs.iter().filter(|s| kept.contains(&s.id))), artist_langs: HashMap::new() },
        likes: like_times.iter().filter(|l| kept.contains(&l.0)).map(|l| l.0).collect(),
        like_times: like_times.into_iter().filter(|l| kept.contains(&l.0)).collect(),
        playlist_entries: playlists.iter().flatten().filter(|id| kept.contains(id)).map(|id| (*id, end - 61 * DAY_MS)).collect(),
        playlists: playlists.into_iter().map(|p| p.into_iter().filter(|id| kept.contains(id)).collect()).collect(),
        events: events.into_iter().filter(|e| kept.contains(&e.track)).collect(),
        utc_offset_min: 0,
        personal_graph: true,
        library_prior: true,
        dislike_neighbours: true,
        truth: lib.songs.iter().enumerate().filter(|(_, s)| kept.contains(&s.id)).map(|(i, s)| (s.id, t.appeal[i])).collect(),
        graph_outside_tracks: outside,
        new: pool,
        ..Dataset::default()
    }
}
