//! An in-memory [`StreamService`] for tests and demo screenshots.
//!
//! The catalog is fixed and the same on every run: four artists (one with a
//! Cyrillic name, one with a Japanese one), five albums with track numbers,
//! ISRCs on most tracks, two playlists (one editable, one followed), a few
//! liked tracks and a short listening history. Likes and playlists are
//! mutable in memory, so sync logic can be exercised against it.
//!
//! The Spotify and YouTube flavours share the songs and their ISRCs (so
//! cross-service matching can be tested) but not the ids; the YouTube one
//! knows ISRCs only for "Discovery" (YouTube rarely exposes them) and has an
//! extra music video. The Deezer flavour is a catalogue only
//! ([`StreamService::catalogue_only`]): never logged in, no sound, and like
//! the real one it gives ISRCs only for a single track (`track`, `tracks`,
//! `find_isrc`), not in search results or album listings. Every future is
//! ready on its first poll. Audio is a
//! short sine as 16-bit mono WAV, generated on the first read; its pitch
//! depends on the track.
//!
//! Discovery (`related`, `radio`, `similar_artists`, `artist_top`,
//! `artist_radio`) follows a fixed table of similar artists that leads to
//! five more artists outside the listings (Аквариум, Наутилус Помпилиус,
//! 椎名林檎, Justice, Sigur Rós; [`FakeService::discovery_tracks`]), and
//! supports what the real flavour does: everything on YouTube, no track
//! radio on Deezer, nothing on Spotify. The YouTube flavour keeps every
//! track it opened "on disk" for `cached_audio`.
//! [`FakeService::with_new_release`] adds a fresh single for new releases.
//!
//! Suggestions come from fixed lists too: the
//! YouTube flavour's home (quick picks, listen again) and history, the
//! Spotify flavour's tops and recently played (each a mix of catalog songs
//! and songs only discovery leads to); Deezer has none.
//! [`FakeService::without_top_scope`] makes Spotify's tops answer as a
//! token without `user-top-read` does.

use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use super::{
    Account, AlbumRef, ArtistRef, AudioStream, BoxFuture, EntityKind, LikedHead, LoginInput,
    Played, RemoteId, SearchKind, Service, StreamAlbum, StreamArtist, StreamError, StreamPlaylist,
    StreamSearch, StreamService, StreamTrack, SuggestedArtist, SuggestedTrack, SuggestionFeed, SuggestionKind,
    SoloistAction, SoloistPhase, SoloistStatus, Suggestions, TrackKind, format, only_kind, rank_weight,
};
use crate::model::{ReleaseType, UnixMs};

/// Display name of the fake account.
const USER: &str = "Fake User";
/// Sample rate of the generated audio.
const AUDIO_RATE: u32 = 22_050;
/// Start of the fake listening history (2025-10-09).
const HISTORY_T0: UnixMs = 1_760_000_000_000;
const HOUR_MS: UnixMs = 3_600_000;
/// Tracks in the first page of [`StreamService::liked_head`] (small, so
/// tests see a head that is not the whole list).
const LIKED_PAGE: usize = 2;

struct AlbumData<'a> {
    artist: &'a str,
    key: &'a str,
    title: &'a str,
    year: i32,
    /// The day of release (Spotify and Deezer give it, YouTube only the year).
    date: &'a str,
    release_type: ReleaseType,
    /// Title, length in seconds, ISRC.
    tracks: &'a [(&'a str, u64, Option<&'a str>)],
}

/// `(key, name)`.
const ARTISTS: &[(&str, &str)] = &[
    ("kino", "Кино"),
    ("utada", "宇多田ヒカル"),
    ("daft", "Daft Punk"),
    ("bjork", "Björk"),
];

const ALBUMS: &[AlbumData<'static>] = &[
    AlbumData {
        artist: "kino",
        key: "gruppa",
        title: "Группа крови",
        year: 1988,
        date: "1988-01-05",
        release_type: ReleaseType::Album,
        tracks: &[
            ("Группа крови", 285, Some("ZZFAK8800001")),
            ("Закрой за мной дверь, я ухожу", 257, Some("ZZFAK8800002")),
            ("Война", 236, None),
            ("Спокойная ночь", 365, Some("ZZFAK8800004")),
        ],
    },
    AlbumData {
        artist: "utada",
        key: "firstlove",
        title: "First Love",
        year: 1999,
        date: "1999-03-10",
        release_type: ReleaseType::Album,
        tracks: &[
            ("Automatic", 306, Some("ZZFAK9900001")),
            ("Movin' on without you", 279, Some("ZZFAK9900002")),
            ("First Love", 257, Some("ZZFAK9900003")),
        ],
    },
    AlbumData {
        artist: "utada",
        key: "michi",
        title: "道",
        year: 2016,
        date: "2016-09-28",
        release_type: ReleaseType::Single,
        tracks: &[("道", 240, Some("ZZFAK1600001"))],
    },
    AlbumData {
        artist: "daft",
        key: "discovery",
        title: "Discovery",
        year: 2001,
        date: "2001-03-12",
        release_type: ReleaseType::Album,
        tracks: &[
            ("One More Time", 320, Some("ZZFAK0100001")),
            ("Aerodynamic", 212, Some("ZZFAK0100002")),
            ("Digital Love", 301, Some("ZZFAK0100003")),
            (
                "Harder, Better, Faster, Stronger",
                224,
                Some("ZZFAK0100004"),
            ),
        ],
    },
    AlbumData {
        artist: "bjork",
        key: "homogenic",
        title: "Homogenic",
        year: 1997,
        date: "1997-09-22",
        release_type: ReleaseType::Album,
        tracks: &[
            ("Hunter", 255, None),
            ("Jóga", 305, Some("ZZFAK9700002")),
            ("Bachelorette", 312, Some("ZZFAK9700003")),
        ],
    },
];

/// Artists only discovery leads to (they are not in [`FakeService::tracks`]
/// or [`FakeService::albums`], but search, track and ISRC lookups find
/// them, so their songs resolve and play): `(key, name)`.
const NEW_ARTISTS: &[(&str, &str)] = &[
    ("aquarium", "Аквариум"),
    ("nautilus", "Наутилус Помпилиус"),
    ("shiina", "椎名林檎"),
    ("justice", "Justice"),
    ("sigur", "Sigur Rós"),
];

/// Their albums (every track with an ISRC, on YouTube too).
const NEW_ALBUMS: &[AlbumData<'static>] = &[
    AlbumData {
        artist: "aquarium",
        key: "afrika",
        title: "Радио Африка",
        year: 1983,
        date: "1983-06-01",
        release_type: ReleaseType::Album,
        tracks: &[
            ("Рок-н-ролл мёртв", 236, Some("ZZNEW8300001")),
            ("Капитан Африка", 391, Some("ZZNEW8300002")),
        ],
    },
    AlbumData {
        artist: "nautilus",
        key: "razluka",
        title: "Разлука",
        year: 1986,
        date: "1986-11-01",
        release_type: ReleaseType::Album,
        tracks: &[
            ("Скованные одной цепью", 270, Some("ZZNEW8600001")),
            ("Казанова", 231, Some("ZZNEW8600002")),
        ],
    },
    AlbumData {
        artist: "shiina",
        key: "muzai",
        title: "無罪モラトリアム",
        year: 1999,
        date: "1999-02-24",
        release_type: ReleaseType::Album,
        tracks: &[
            ("丸の内サディスティック", 234, Some("ZZNEW9900001")),
            ("歌舞伎町の女王", 254, Some("ZZNEW9900002")),
        ],
    },
    AlbumData {
        artist: "justice",
        key: "cross",
        title: "Cross",
        year: 2007,
        date: "2007-06-11",
        release_type: ReleaseType::Album,
        tracks: &[
            ("Genesis", 234, Some("ZZNEW0700001")),
            ("D.A.N.C.E.", 242, Some("ZZNEW0700002")),
        ],
    },
    AlbumData {
        artist: "sigur",
        key: "agaetis",
        title: "Ágætis byrjun",
        year: 1999,
        date: "1999-06-12",
        release_type: ReleaseType::Album,
        tracks: &[
            ("Svefn-g-englar", 604, Some("ZZNEW9900011")),
            ("Starálfur", 407, Some("ZZNEW9900012")),
        ],
    },
];

/// Who is like whom, most alike first (`similar_artists`).
const SIMILAR: &[(&str, &[&str])] = &[
    ("kino", &["aquarium", "nautilus", "daft"]),
    ("utada", &["shiina", "bjork"]),
    ("daft", &["justice", "kino"]),
    ("bjork", &["sigur", "utada"]),
    ("aquarium", &["kino", "nautilus"]),
    ("nautilus", &["kino", "aquarium"]),
    ("shiina", &["utada"]),
    ("justice", &["daft"]),
    ("sigur", &["bjork"]),
];

/// Tracks in an artist's top, a related list and a radio.
const TOP_LEN: usize = 10;
const RELATED_LEN: usize = 10;
const RADIO_LEN: usize = 25;

/// `(key, name, editable, tracks as album key + number)`.
type PlaylistData = (
    &'static str,
    &'static str,
    bool,
    &'static [(&'static str, u32)],
);

const PLAYLISTS: &[PlaylistData] = &[
    (
        "evening",
        "Вечер",
        true,
        &[
            ("gruppa", 1),
            ("firstlove", 3),
            ("homogenic", 2),
            ("discovery", 3),
        ],
    ),
    (
        "mix",
        "Fake Mix",
        false,
        &[("discovery", 1), ("firstlove", 1), ("gruppa", 2)],
    ),
];

/// Liked at start, most recent first.
const LIKED: &[(&str, u32)] = &[("gruppa", 1), ("discovery", 1), ("firstlove", 1)];

/// Listening history, newest first: `(album, number, hours after T0)`.
const RECENT: &[(&str, u32, i64)] = &[("discovery", 2, 3), ("gruppa", 1, 2), ("homogenic", 2, 1)];

/// One list of a suggestion feed: its kind, tracks as album key + number
/// (the albums of [`NEW_ALBUMS`] are songs new to a library of the
/// catalog) and artists by key.
type SuggestionList = (SuggestionKind, &'static [(&'static str, u32)], &'static [&'static str]);

/// What each feed suggests.
const SUGGESTIONS: &[(SuggestionFeed, &[SuggestionList])] = &[
    (
        SuggestionFeed::Home,
        &[
            (
                SuggestionKind::QuickPicks,
                &[("gruppa", 4), ("discovery", 3), ("agaetis", 1), ("cross", 2)],
                &[],
            ),
            (SuggestionKind::ListenAgain, &[("homogenic", 2)], &["bjork", "kino"]),
        ],
    ),
    (
        SuggestionFeed::History,
        &[(SuggestionKind::Recent, &[("discovery", 2), ("gruppa", 1), ("homogenic", 2)], &[])],
    ),
    (
        SuggestionFeed::TopTracksMonth,
        &[(SuggestionKind::TopMonth, &[("gruppa", 3), ("discovery", 4), ("muzai", 1)], &[])],
    ),
    (
        SuggestionFeed::TopTracksHalfYear,
        &[(SuggestionKind::TopHalfYear, &[("firstlove", 1), ("gruppa", 1)], &[])],
    ),
    (SuggestionFeed::TopArtistsMonth, &[(SuggestionKind::TopMonth, &[], &["daft", "kino"])]),
    (SuggestionFeed::TopArtistsHalfYear, &[(SuggestionKind::TopHalfYear, &[], &["utada", "bjork"])]),
    (
        SuggestionFeed::RecentlyPlayed,
        &[(SuggestionKind::Recent, &[("discovery", 2), ("gruppa", 1), ("homogenic", 2)], &[])],
    ),
];

const YOUTUBE_FEEDS: &[SuggestionFeed] = &[SuggestionFeed::Home, SuggestionFeed::History];
const SPOTIFY_FEEDS: &[SuggestionFeed] = &[
    SuggestionFeed::TopTracksMonth,
    SuggestionFeed::TopTracksHalfYear,
    SuggestionFeed::TopArtistsMonth,
    SuggestionFeed::TopArtistsHalfYear,
    SuggestionFeed::RecentlyPlayed,
];

fn prefix(service: Service) -> &'static str {
    match service {
        Service::Spotify => "sp",
        Service::Youtube => "yt",
        Service::Deezer => "dz",
        Service::Soundcloud => "sc",
    }
}

fn track_id(service: Service, album: &str, n: u32) -> String {
    format!("{}-{album}-{n}", prefix(service))
}

/// The fixed catalog of one flavour.
struct Catalog {
    artists: Vec<StreamArtist>,
    albums: Vec<StreamAlbum>,
    tracks: Vec<StreamTrack>,
    /// Found only by discovery (and search, track and ISRC lookups): the
    /// artists of [`NEW_ARTISTS`] with their albums.
    more_artists: Vec<StreamArtist>,
    more_albums: Vec<StreamAlbum>,
    more_tracks: Vec<StreamTrack>,
}

/// The artist of `key` in either table, as a credit.
fn artist_ref(service: Service, key: &str) -> ArtistRef {
    let name = ARTISTS
        .iter()
        .chain(NEW_ARTISTS)
        .find(|(k, _)| *k == key)
        .map_or("", |(_, n)| n);
    ArtistRef::new(Some(RemoteId::new(service, format!("{}-art-{key}", prefix(service)))), name)
}

/// One album and its tracks. YouTube gives no release day and knows ISRCs
/// only where `isrc_on_youtube`.
fn build_album(service: Service, a: &AlbumData<'_>, isrc_on_youtube: bool) -> StreamAlbum {
    let p = prefix(service);
    let album_id = RemoteId::new(service, format!("{p}-alb-{}", a.key));
    let cover = Some(format!("https://fake.invalid/{p}/{}.jpg", a.key));
    let mut album_ref = AlbumRef::new(Some(album_id.clone()), a.title);
    album_ref.artists = vec![artist_ref(service, a.artist)];
    album_ref.year = Some(a.year);
    let mut tracks = Vec::new();
    for (i, &(title, secs, isrc)) in a.tracks.iter().enumerate() {
        let n = i as u32 + 1;
        let mut t = StreamTrack::new(RemoteId::new(service, track_id(service, a.key, n)), title);
        t.artists = vec![artist_ref(service, a.artist)];
        t.album = Some(album_ref.clone());
        t.duration_ms = Some(secs * 1000);
        t.isrc = isrc
            .filter(|_| service != Service::Youtube || isrc_on_youtube)
            .map(str::to_owned);
        t.disc_no = Some(1);
        t.track_no = Some(n);
        t.cover_url = cover.clone();
        tracks.push(t);
    }
    StreamAlbum {
        id: album_id,
        title: a.title.to_owned(),
        artists: vec![artist_ref(service, a.artist)],
        year: Some(a.year),
        release_date: (service != Service::Youtube).then(|| a.date.to_owned()),
        release_type: a.release_type,
        cover_url: cover,
        total_tracks: Some(a.tracks.len() as u32),
        tracks,
    }
}

/// The artists of `table` with their discographies (from `albums`, newest
/// first, without tracks).
fn build_artists(service: Service, table: &[(&str, &str)], albums: &[StreamAlbum]) -> Vec<StreamArtist> {
    let p = prefix(service);
    table
        .iter()
        .map(|&(key, name)| {
            let id = RemoteId::new(service, format!("{p}-art-{key}"));
            let mut discography: Vec<StreamAlbum> = albums
                .iter()
                .filter(|al| al.artists.iter().any(|ar| ar.id.as_ref() == Some(&id)))
                .map(|al| StreamAlbum {
                    tracks: Vec::new(),
                    ..al.clone()
                })
                .collect();
            discography.sort_by_key(|al| std::cmp::Reverse(al.year));
            StreamArtist {
                id,
                name: name.to_owned(),
                image_url: Some(format!("https://fake.invalid/{p}/artist-{key}.jpg")),
                albums: discography,
            }
        })
        .collect()
}

impl Catalog {
    fn build(service: Service) -> Self {
        let p = prefix(service);
        let albums: Vec<StreamAlbum> = ALBUMS
            .iter()
            .map(|a| build_album(service, a, a.key == "discovery"))
            .collect();
        let mut tracks: Vec<StreamTrack> = albums.iter().flat_map(|a| a.tracks.iter().cloned()).collect();
        if service == Service::Youtube {
            let mut v = StreamTrack::new(
                RemoteId::new(service, format!("{p}-video-kino")),
                "Группа крови (клип)",
            );
            v.artists = vec![artist_ref(service, "kino")];
            v.duration_ms = Some(290_000);
            v.kind = TrackKind::Video;
            tracks.push(v);
        }
        let artists = build_artists(service, ARTISTS, &albums);
        let more_albums: Vec<StreamAlbum> = NEW_ALBUMS.iter().map(|a| build_album(service, a, true)).collect();
        let more_tracks = more_albums.iter().flat_map(|a| a.tracks.iter().cloned()).collect();
        let more_artists = build_artists(service, NEW_ARTISTS, &more_albums);
        Self {
            artists,
            albums,
            tracks,
            more_artists,
            more_albums,
            more_tracks,
        }
    }

    /// Every track, the listed ones first.
    fn all_tracks(&self) -> impl Iterator<Item = &StreamTrack> {
        self.tracks.iter().chain(&self.more_tracks)
    }

    fn track(&self, id: &str) -> Option<&StreamTrack> {
        self.all_tracks().find(|t| t.id.id == id)
    }

    fn album(&self, id: &str) -> Option<&StreamAlbum> {
        self.albums.iter().chain(&self.more_albums).find(|a| a.id.id == id)
    }

    fn artist(&self, id: &str) -> Option<&StreamArtist> {
        self.artists.iter().chain(&self.more_artists).find(|a| a.id.id == id)
    }

    /// The songs credited first to `artist_id`, most popular first (a
    /// stable order derived from the ids), at most [`TOP_LEN`].
    fn top_of(&self, artist_id: &str) -> Vec<StreamTrack> {
        let mut out: Vec<StreamTrack> = self
            .all_tracks()
            .filter(|t| t.kind == TrackKind::Song)
            .filter(|t| t.artists.first().and_then(|a| a.id.as_ref()).is_some_and(|i| i.id == artist_id))
            .cloned()
            .collect();
        out.sort_by_key(|t| (popularity(&t.id.id), t.id.id.clone()));
        out.truncate(TOP_LEN);
        out
    }

    /// Ids of the artists like `artist_id`, most alike first: [`SIMILAR`]
    /// for the fixed artists, two of [`NEW_ARTISTS`] picked by the id for
    /// any other known one. `None`: no such artist.
    fn similar_to(&self, service: Service, artist_id: &str) -> Option<Vec<String>> {
        let p = prefix(service);
        let key = artist_id.strip_prefix(&format!("{p}-art-"))?;
        if let Some((_, like)) = SIMILAR.iter().find(|(k, _)| *k == key) {
            return Some(like.iter().map(|k| format!("{p}-art-{k}")).collect());
        }
        let known = self.artist(artist_id).is_some()
            || self
                .all_tracks()
                .any(|t| t.artists.iter().any(|a| a.id.as_ref().is_some_and(|i| i.id == artist_id)));
        known.then(|| {
            let h = popularity(artist_id) as usize;
            (0..2)
                .map(|i| format!("{p}-art-{}", NEW_ARTISTS[(h + i) % NEW_ARTISTS.len()].0))
                .collect()
        })
    }

    /// Ids of the artists like the artist of `track` (the first credit);
    /// a track without an artist id gets two of [`NEW_ARTISTS`] picked by
    /// its own id.
    fn similar_to_track(&self, service: Service, track: &StreamTrack) -> Vec<String> {
        let p = prefix(service);
        track
            .artists
            .first()
            .and_then(|a| a.id.as_ref())
            .and_then(|a| self.similar_to(service, &a.id))
            .unwrap_or_else(|| {
                let h = popularity(&track.id.id) as usize;
                (0..2)
                    .map(|i| format!("{p}-art-{}", NEW_ARTISTS[(h + i) % NEW_ARTISTS.len()].0))
                    .collect()
            })
    }
}

/// A stable pseudo-popularity rank of an id (lower = more popular): FNV-1a.
fn popularity(id: &str) -> u64 {
    id.bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// The civil date `(year, month, day)` of a day number since 1970-01-01
/// (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// The lists taken in turn, one item of each, skipping `seed` and repeats,
/// up to `max`: how the fake mixes an artist with similar ones.
fn interleave(lists: &[Vec<StreamTrack>], seed: Option<&str>, max: usize) -> Vec<StreamTrack> {
    let mut out: Vec<StreamTrack> = Vec::new();
    let longest = lists.iter().map(Vec::len).max().unwrap_or(0);
    for i in 0..longest {
        for l in lists {
            if let Some(t) = l.get(i)
                && Some(t.id.id.as_str()) != seed
                && !out.iter().any(|o| o.id == t.id)
            {
                out.push(t.clone());
            }
        }
    }
    out.truncate(max);
    out
}

struct State {
    account: Account,
    liked: Vec<String>,
    playlists: Vec<StreamPlaylist>,
    created: u32,
    unavailable: HashSet<String>,
    opened: Vec<String>,
    audio_secs: f32,
    /// Calls per trait method, for request-counting tests.
    calls: HashMap<&'static str, usize>,
    /// The next calls of a method that answer [`StreamError::RateLimited`]:
    /// how many are left and the `retry_after` they carry.
    /// The third field: a pause the service keeps itself, set from the
    /// moment such an answer is given (see [`FakeService::rate_limit_next_paused`]).
    limited: HashMap<&'static str, (usize, Option<Duration>, Option<Duration>)>,
    /// What [`StreamService::paused_until`] answers.
    paused_until: Option<std::time::SystemTime>,
    /// Calls of a method that take this long ([`FakeService::delay`]).
    delays: HashMap<&'static str, Duration>,
    /// A login finds no Premium ([`FakeService::without_premium`]).
    no_premium: bool,
    /// The song of each music video ([`StreamService::song_for_video`]),
    /// by video id.
    songs: HashMap<String, String>,
    /// Tracks whose whole audio the YouTube flavour has "on disk": those
    /// opened so far ([`StreamService::cached_audio`]).
    cached: HashSet<String>,
    /// Spotify's tops answer as a token without `user-top-read`
    /// ([`FakeService::without_top_scope`]).
    no_top_scope: bool,
    /// Its Spotify Soloist ([`FakeService::with_soloist`]).
    soloist: Option<SoloistStatus>,
}

/// Deterministic fake of a streaming service (feature `fake`).
pub struct FakeService {
    service: Service,
    catalog: Catalog,
    state: Mutex<State>,
}

impl FakeService {
    /// The fake of `service`, logged in.
    pub fn new(service: Service) -> Self {
        let catalog = Catalog::build(service);
        let p = prefix(service);
        let owner = service.display_name();
        let playlists = PLAYLISTS
            .iter()
            .map(|&(key, name, editable, items)| {
                let tracks: Vec<StreamTrack> = items
                    .iter()
                    .filter_map(|&(album, n)| catalog.track(&track_id(service, album, n)).cloned())
                    .collect();
                StreamPlaylist {
                    id: RemoteId::new(service, format!("{p}-pl-{key}")),
                    name: name.to_owned(),
                    owner: Some(if editable { USER } else { owner }.to_owned()),
                    cover_url: None,
                    version: Some("1".into()),
                    editable,
                    total_tracks: Some(tracks.len() as u32),
                    tracks,
                }
            })
            .collect();
        let liked = LIKED
            .iter()
            .map(|&(album, n)| track_id(service, album, n))
            .collect();
        // A catalogue has no account.
        let account = if service == Service::Deezer {
            Account::LoggedOut
        } else {
            Account::LoggedIn {
                name: USER.into(),
                premium: Some(true),
                audio: true,
            }
        };
        Self {
            service,
            catalog,
            state: Mutex::new(State {
                account,
                liked,
                playlists,
                created: 0,
                unavailable: HashSet::new(),
                opened: Vec::new(),
                audio_secs: 2.0,
                calls: HashMap::new(),
                limited: HashMap::new(),
                paused_until: None,
                delays: HashMap::new(),
                no_premium: false,
                // The clip of «Группа крови» is the album's song.
                songs: if service == Service::Youtube {
                    HashMap::from([(format!("{p}-video-kino"), track_id(service, "gruppa", 1))])
                } else {
                    HashMap::new()
                },
                cached: HashSet::new(),
                no_top_scope: false,
                soloist: None,
            }),
        }
    }

    pub fn spotify() -> Self {
        Self::new(Service::Spotify)
    }

    pub fn youtube() -> Self {
        Self::new(Service::Youtube)
    }

    pub fn deezer() -> Self {
        Self::new(Service::Deezer)
    }

    /// What a listing of this flavour gives of a track: Deezer leaves the
    /// ISRC out of search results and album tracks.
    fn listed(&self, t: &StreamTrack) -> StreamTrack {
        let mut t = t.clone();
        if self.service == Service::Deezer {
            t.isrc = None;
        }
        t
    }

    /// Has a Spotify Soloist at `phase` (screenshots of its setup); its
    /// steps move it along as the real one would, at once.
    #[must_use]
    pub fn with_soloist(self, phase: SoloistPhase, offer: bool) -> Self {
        self.lock().soloist = Some(SoloistStatus {
            phase,
            device_name: "music.nikgob (DEMO-PC)".into(),
            size: Some(9_780_408),
            expires_at: Some(1_801_699_311),
            has_key: false,
            offer,
        });
        self
    }

    /// Starts logged out instead.
    #[must_use]
    pub fn logged_out(self) -> Self {
        self.lock().account = Account::LoggedOut;
        self
    }

    /// Logged in without the right to stream sound (Spotify Free, or no
    /// playback authorization): catalog calls work, `open_audio` answers
    /// [`StreamError::PremiumRequired`].
    pub fn without_audio(self) -> Self {
        self.lock().account = Account::LoggedIn {
            name: USER.into(),
            premium: Some(false),
            audio: false,
        };
        self
    }

    /// Spotify Premium signed in, the playback login not given yet.
    pub fn premium_without_audio(self) -> Self {
        self.lock().account = Account::LoggedIn {
            name: USER.into(),
            premium: Some(true),
            audio: false,
        };
        self
    }

    /// Logged out, and a login finds no Premium (Spotify is not connected
    /// without it): the login answers [`StreamError::PremiumRequired`] and
    /// the account reads [`super::SPOTIFY_PREMIUM_REQUIRED`].
    pub fn without_premium(self) -> Self {
        {
            let mut st = self.lock();
            st.account = Account::LoggedOut;
            st.no_premium = true;
        }
        self
    }

    /// Spotify signed in with a token from before `user-top-read` was
    /// asked: its top feeds answer [`Suggestions::needs_reconnect`].
    #[must_use]
    pub fn without_top_scope(self) -> Self {
        self.lock().no_top_scope = true;
        self
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Every track of the catalog (album tracks in album order, then extras).
    pub fn tracks(&self) -> Vec<StreamTrack> {
        self.catalog.tracks.clone()
    }

    /// Every album of the catalog, with tracks.
    pub fn albums(&self) -> Vec<StreamAlbum> {
        self.catalog.albums.clone()
    }

    /// The songs only discovery leads to (artists of [`NEW_ARTISTS`]).
    pub fn discovery_tracks(&self) -> Vec<StreamTrack> {
        self.catalog.more_tracks.clone()
    }

    /// Adds a single by one of the fixed artists (`artist`: `kino`,
    /// `utada`, `daft`, `bjork`) released `days_ago` days before today
    /// (UTC): an album `{prefix}-alb-{key}` with one song `{prefix}-{key}-1`,
    /// first in the artist's discography, so new releases have something to
    /// find. The one thing of the fake that depends on the clock; YouTube
    /// gives only its year, as the real one does.
    #[must_use]
    pub fn with_new_release(mut self, artist: &str, key: &str, title: &str, days_ago: u32) -> Self {
        let service = self.service;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        let day = now_ms.div_euclid(86_400_000) - i64::from(days_ago);
        let (y, m, d) = civil(day);
        let date = format!("{y:04}-{m:02}-{d:02}");
        let data = AlbumData {
            artist,
            key,
            title,
            year: y as i32,
            date: &date,
            release_type: ReleaseType::Single,
            tracks: &[(title, 215, None)],
        };
        let album = build_album(service, &data, false);
        self.catalog.tracks.extend(album.tracks.iter().cloned());
        let artist_id = format!("{}-art-{artist}", prefix(service));
        if let Some(a) = self.catalog.artists.iter_mut().find(|a| a.id.id == artist_id) {
            a.albums.insert(
                0,
                StreamAlbum {
                    tracks: Vec::new(),
                    ..album.clone()
                },
            );
        }
        self.catalog.albums.push(album);
        self
    }

    /// Ids of the liked tracks, most recent first.
    pub fn liked_ids(&self) -> Vec<String> {
        self.lock().liked.clone()
    }

    /// Makes the audio of `id` fail on its first read (region lock, withdrawn).
    pub fn set_unavailable(&self, id: &str, unavailable: bool) {
        let mut st = self.lock();
        if unavailable {
            st.unavailable.insert(id.to_owned());
        } else {
            st.unavailable.remove(id);
        }
    }

    /// Length of the generated audio (default 2 s), for every later open.
    pub fn set_audio_secs(&self, secs: f32) {
        self.lock().audio_secs = secs.max(0.01);
    }

    /// Ids passed to [`StreamService::open_audio`] so far, in order.
    pub fn opened(&self) -> Vec<String> {
        self.lock().opened.clone()
    }

    /// Adds an album of `n` tracks (`{prefix}-{key}-1` …), each with an
    /// ISRC, to the catalog (for tests that need a big one).
    #[must_use]
    pub fn with_album(mut self, key: &str, title: &str, n: u32) -> Self {
        let service = self.service;
        let p = prefix(service);
        let album_id = RemoteId::new(service, format!("{p}-alb-{key}"));
        let artist = ArtistRef::new(Some(RemoteId::new(service, format!("{p}-art-{key}"))), "Big Band");
        let mut album_ref = AlbumRef::new(Some(album_id.clone()), title);
        album_ref.artists = vec![artist.clone()];
        let tracks: Vec<StreamTrack> = (1..=n)
            .map(|i| {
                let mut t = StreamTrack::new(RemoteId::new(service, track_id(service, key, i)), format!("{title} {i}"));
                t.artists = vec![artist.clone()];
                t.album = Some(album_ref.clone());
                t.duration_ms = Some(200_000 + u64::from(i) * 1000);
                t.isrc = Some(format!("ZZBIG{:07}", i));
                t.track_no = Some(i);
                t
            })
            .collect();
        self.catalog.tracks.extend(tracks.iter().cloned());
        self.catalog.albums.push(StreamAlbum {
            id: album_id,
            title: title.to_owned(),
            artists: vec![artist],
            year: Some(2020),
            release_date: (service != Service::Youtube).then(|| "2020-01-01".into()),
            release_type: ReleaseType::Album,
            cover_url: None,
            total_tracks: Some(n),
            tracks,
        });
        self
    }

    /// Adds one single track `{prefix}-x-{key}` (no album, no ISRC) to the
    /// catalog. Its search matches the text as written (lower case only):
    /// «лед» does not find «Лёд», as on the real services.
    #[must_use]
    pub fn with_track(mut self, key: &str, title: &str, artist: &str, secs: u64) -> Self {
        let p = prefix(self.service);
        let mut t = StreamTrack::new(RemoteId::new(self.service, format!("{p}-x-{key}")), title);
        t.artists = vec![ArtistRef::new(None, artist)];
        t.duration_ms = Some(secs * 1000);
        self.catalog.tracks.push(t);
        self
    }

    /// Adds one song `{prefix}-x-{key}` on its own album `{prefix}-alb-{key}`
    /// (titled `album`, with a cover), no ISRC.
    #[must_use]
    pub fn with_song(mut self, key: &str, title: &str, artist: &str, album: &str, secs: u64) -> Self {
        let p = prefix(self.service);
        let mut t = StreamTrack::new(RemoteId::new(self.service, format!("{p}-x-{key}")), title);
        t.artists = vec![ArtistRef::new(None, artist)];
        let mut a = AlbumRef::new(Some(RemoteId::new(self.service, format!("{p}-alb-{key}"))), album);
        a.artists = t.artists.clone();
        t.album = Some(a);
        t.duration_ms = Some(secs * 1000);
        t.cover_url = Some(format!("https://fake.invalid/{p}/{key}.jpg"));
        self.catalog.tracks.push(t);
        self
    }

    /// Adds a music video `{prefix}-video-{key}` (no album, no ISRC) whose
    /// song is `song` (a catalog id) or none, to the catalog.
    #[must_use]
    pub fn with_video(mut self, key: &str, title: &str, artist: &str, secs: u64, song: Option<&str>) -> Self {
        let p = prefix(self.service);
        let id = format!("{p}-video-{key}");
        let mut t = StreamTrack::new(RemoteId::new(self.service, id.clone()), title);
        t.artists = vec![ArtistRef::new(None, artist)];
        t.duration_ms = Some(secs * 1000);
        t.kind = TrackKind::Video;
        t.cover_url = Some(format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg"));
        self.catalog.tracks.push(t);
        if let Some(song) = song {
            self.lock().songs.insert(id, song.to_owned());
        }
        self
    }

    /// Likes (or unlikes) `id` on the service side, as if the user did it
    /// in another app. Not counted as a call.
    pub fn set_liked_remotely(&self, id: &str, liked: bool) {
        let mut st = self.lock();
        st.liked.retain(|l| l != id);
        if liked {
            st.liked.insert(0, id.to_owned());
        }
    }

    /// How many times the trait method `name` was called (`"search"`,
    /// `"liked"`, `"liked_head"`, `"set_liked_many"`, …).
    pub fn calls(&self, name: &str) -> usize {
        self.lock().calls.get(name).copied().unwrap_or(0)
    }

    /// Every call so far, by method, sorted by name.
    pub fn all_calls(&self) -> Vec<(&'static str, usize)> {
        let mut v: Vec<_> = self.lock().calls.iter().map(|(k, v)| (*k, *v)).collect();
        v.sort_unstable();
        v
    }

    /// Calls in all (every call stands for at least one request of the
    /// real service).
    pub fn total_calls(&self) -> usize {
        self.lock().calls.values().sum()
    }

    pub fn reset_calls(&self) {
        self.lock().calls.clear();
    }

    /// The next `n` calls of the trait method `name` (`"search"`,
    /// `"liked"`, `"liked_head"`, `"my_playlists"`, `"playlist"`,
    /// `"playlist_version"`, `"search_kind"`, `"open_audio"`) answer
    /// [`StreamError::RateLimited`] with `retry_after` (they are still
    /// counted as calls). `n = 0` lifts it.
    pub fn rate_limit_next(&self, name: &'static str, n: usize, retry_after: Option<Duration>) {
        let mut st = self.lock();
        if n == 0 {
            st.limited.remove(name);
        } else {
            st.limited.insert(name, (n, retry_after, None));
        }
    }

    /// Like [`Self::rate_limit_next`] without a `retry_after`, the way
    /// YouTube Music answers: each such answer pauses the service for
    /// `pause` from then on ([`StreamService::paused_until`]).
    pub fn rate_limit_next_paused(&self, name: &'static str, n: usize, pause: Duration) {
        self.lock().limited.insert(name, (n, None, Some(pause)));
    }

    /// Every call of the trait method `name` that goes through its limits
    /// (see [`Self::rate_limit_next`]) takes `d` before it answers (it
    /// blocks the calling thread: the answer of a slow network). Zero lifts it.
    pub fn delay(&self, name: &'static str, d: Duration) {
        let mut st = self.lock();
        if d.is_zero() {
            st.delays.remove(name);
        } else {
            st.delays.insert(name, d);
        }
    }

    fn count(&self, name: &'static str) {
        *self.lock().calls.entry(name).or_default() += 1;
        let delay = self.lock().delays.get(name).copied();
        if let Some(d) = delay {
            std::thread::sleep(d);
        }
    }

    /// Counts the call; a rate limit set by [`Self::rate_limit_next`]
    /// answers instead.
    fn gate(&self, name: &'static str) -> Result<(), StreamError> {
        self.count(name);
        let mut st = self.lock();
        let Some((left, retry_after, pause)) = st.limited.get_mut(name) else {
            return Ok(());
        };
        let (retry_after, pause) = (*retry_after, *pause);
        *left -= 1;
        if *left == 0 {
            st.limited.remove(name);
        }
        if let Some(p) = pause {
            st.paused_until = Some(std::time::SystemTime::now() + p);
        }
        Err(StreamError::RateLimited { retry_after })
    }

    fn need_login(&self) -> Result<(), StreamError> {
        if self.lock().account.is_logged_in() {
            Ok(())
        } else {
            Err(StreamError::NotLoggedIn)
        }
    }

    fn track_now(&self, id: &str) -> Result<StreamTrack, StreamError> {
        self.catalog
            .track(id)
            .cloned()
            .ok_or(StreamError::Unavailable)
    }

    fn search_now(&self, query: &str) -> StreamSearch {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return StreamSearch::default();
        }
        let hit = |s: &str| s.to_lowercase().contains(&q);
        let tracks = self
            .catalog
            .all_tracks()
            .filter(|t| {
                let fields: Vec<String> = std::iter::once(t.title.to_lowercase())
                    .chain(t.artists.iter().map(|a| a.name.to_lowercase()))
                    .chain(t.album.iter().map(|a| a.title.to_lowercase()))
                    .collect();
                // The query in one field, or each of its words in some
                // field («aiyou eve»: the title and the artist).
                fields.iter().any(|f| f.contains(&q))
                    || q.split_whitespace().all(|w| fields.iter().any(|f| f.contains(w)))
            })
            .map(|t| self.listed(t))
            .collect();
        let albums = self
            .catalog
            .albums
            .iter()
            .chain(&self.catalog.more_albums)
            .filter(|a| {
                // The query in its title or artist, or its words over both
                // («кино группа крови»).
                let fields: Vec<String> = std::iter::once(a.title.to_lowercase())
                    .chain(a.artists.iter().map(|ar| ar.name.to_lowercase()))
                    .collect();
                hit(&a.title)
                    || a.artists.iter().any(|ar| hit(&ar.name))
                    || q.split_whitespace().all(|w| fields.iter().any(|f| f.contains(w)))
            })
            .map(|a| StreamAlbum {
                tracks: Vec::new(),
                ..a.clone()
            })
            .collect();
        let artists = self
            .catalog
            .artists
            .iter()
            .chain(&self.catalog.more_artists)
            .filter(|a| hit(&a.name))
            .map(|a| StreamArtist {
                albums: Vec::new(),
                ..a.clone()
            })
            .collect();
        let playlists = self
            .lock()
            .playlists
            .iter()
            .filter(|p| hit(&p.name))
            .map(|p| StreamPlaylist {
                tracks: Vec::new(),
                ..p.clone()
            })
            .collect();
        StreamSearch {
            tracks,
            albums,
            artists,
            playlists,
        }
    }

    fn set_liked_now(&self, id: &str, liked: bool) -> Result<(), StreamError> {
        self.need_login()?;
        self.track_now(id)?;
        let mut st = self.lock();
        st.liked.retain(|l| l != id);
        if liked {
            st.liked.insert(0, id.to_owned());
        }
        Ok(())
    }

    fn create_now(&self, name: &str) -> Result<String, StreamError> {
        self.need_login()?;
        let mut st = self.lock();
        st.created += 1;
        let id = format!("{}-pl-new{}", prefix(self.service), st.created);
        st.playlists.push(StreamPlaylist {
            id: RemoteId::new(self.service, id.clone()),
            name: name.to_owned(),
            owner: Some(USER.into()),
            cover_url: None,
            version: Some("1".into()),
            editable: true,
            total_tracks: Some(0),
            tracks: Vec::new(),
        });
        Ok(id)
    }

    fn edit_now(
        &self,
        id: &str,
        add: &[String],
        remove: &[String],
    ) -> Result<Option<String>, StreamError> {
        self.need_login()?;
        let added = add
            .iter()
            .map(|t| self.track_now(t))
            .collect::<Result<Vec<_>, _>>()?;
        let mut st = self.lock();
        let p = st
            .playlists
            .iter_mut()
            .find(|p| p.id.id == id)
            .ok_or(StreamError::Unavailable)?;
        if !p.editable {
            return Err(StreamError::Http { status: 403 });
        }
        p.tracks.retain(|t| !remove.contains(&t.id.id));
        p.tracks.extend(added);
        p.total_tracks = Some(p.tracks.len() as u32);
        let version = p
            .version
            .as_deref()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
            + 1;
        p.version = Some(version.to_string());
        Ok(p.version.clone())
    }

    fn open_now(&self, id: &str) -> Result<FakeAudio, StreamError> {
        if self.service == Service::Deezer {
            return Err(StreamError::Unsupported);
        }
        if self.service == Service::Spotify {
            self.need_login()?;
        }
        let (index, track) = self
            .catalog
            .all_tracks()
            .enumerate()
            .find(|(_, t)| t.id.id == id)
            .ok_or(StreamError::Unavailable)?;
        let mut st = self.lock();
        st.opened.push(id.to_owned());
        let fail = st.unavailable.contains(id) || !track.playable;
        // A played YouTube track is kept whole "on disk".
        if self.service == Service::Youtube && !fail {
            st.cached.insert(id.to_owned());
        }
        let mut audio = FakeAudio::new(
            220.0 + 20.0 * index as f32,
            st.audio_secs,
            fail,
            (self.service == Service::Spotify).then_some(-2.0),
        );
        audio.quality = Some(self.service.nominal_quality());
        Ok(audio)
    }

    /// Ids of the tracks whose whole audio [`StreamService::cached_audio`]
    /// gives (YouTube: every track opened and not failing).
    pub fn cached_ids(&self) -> Vec<String> {
        let mut v: Vec<String> = self.lock().cached.iter().cloned().collect();
        v.sort_unstable();
        v
    }

    /// Discovery as the real service does it: Spotify can do none of it,
    /// Deezer has no track radio (its "related" is the radio of the track's
    /// artist). Counts the call and obeys [`Self::rate_limit_next`].
    fn discover<T>(&self, name: &'static str, f: impl FnOnce() -> Result<T, StreamError>) -> Result<T, StreamError> {
        let supported = match self.service {
            Service::Spotify => false,
            Service::Deezer => name != "radio",
            Service::Youtube | Service::Soundcloud => true,
        };
        self.gate(name)?;
        if supported { f() } else { Err(StreamError::Unsupported) }
    }

    /// [`Self::listed`] of each.
    fn listed_all(&self, v: Vec<StreamTrack>) -> Vec<StreamTrack> {
        v.iter().map(|t| self.listed(t)).collect()
    }

    /// Tops of the similar artists of `ids`, in that order.
    fn tops(&self, ids: &[String]) -> Vec<Vec<StreamTrack>> {
        ids.iter().map(|a| self.catalog.top_of(a)).collect()
    }

    fn related_now(&self, track_id: &str) -> Result<Vec<StreamTrack>, StreamError> {
        let seed = self.track_now(track_id)?;
        if self.service == Service::Deezer {
            // The radio of the track's artist, without the track.
            let artist = seed.artists.first().and_then(|a| a.id.clone());
            let mut out = match artist {
                Some(a) => self.artist_radio_now(&a.id)?,
                None => interleave(&self.tops(&self.catalog.similar_to_track(self.service, &seed)), None, RADIO_LEN),
            };
            out.retain(|t| t.id != seed.id);
            out.truncate(RELATED_LEN);
            return Ok(out);
        }
        let similar = self.catalog.similar_to_track(self.service, &seed);
        Ok(interleave(&self.tops(&similar), Some(track_id), RELATED_LEN))
    }

    fn radio_now(&self, track_id: &str) -> Result<Vec<StreamTrack>, StreamError> {
        let seed = self.track_now(track_id)?;
        let own = seed
            .artists
            .first()
            .and_then(|a| a.id.as_ref())
            .map(|a| self.catalog.top_of(&a.id))
            .unwrap_or_default();
        let mut lists = vec![own];
        lists.extend(self.tops(&self.catalog.similar_to_track(self.service, &seed)));
        Ok(interleave(&lists, Some(track_id), RADIO_LEN))
    }

    fn similar_now(&self, artist_id: &str) -> Result<Vec<StreamArtist>, StreamError> {
        let ids = self
            .catalog
            .similar_to(self.service, artist_id)
            .ok_or(StreamError::Unavailable)?;
        Ok(ids
            .iter()
            .filter_map(|id| self.catalog.artist(id))
            .map(|a| StreamArtist {
                albums: Vec::new(),
                ..a.clone()
            })
            .collect())
    }

    fn top_now(&self, artist_id: &str) -> Result<Vec<StreamTrack>, StreamError> {
        let top = self.catalog.top_of(artist_id);
        if top.is_empty() && self.catalog.artist(artist_id).is_none() {
            return Err(StreamError::Unavailable);
        }
        Ok(top)
    }

    fn suggestions_now(&self, feed: SuggestionFeed) -> Result<Suggestions, StreamError> {
        if !self.suggestion_feeds().contains(&feed) {
            return Err(StreamError::Unsupported);
        }
        self.need_login()?;
        let top = matches!(
            feed,
            SuggestionFeed::TopTracksMonth
                | SuggestionFeed::TopTracksHalfYear
                | SuggestionFeed::TopArtistsMonth
                | SuggestionFeed::TopArtistsHalfYear
        );
        if top && self.lock().no_top_scope {
            return Ok(Suggestions {
                needs_reconnect: true,
                ..Suggestions::default()
            });
        }
        let mut out = Suggestions::default();
        let lists = SUGGESTIONS.iter().find(|(f, _)| *f == feed).map_or(&[][..], |(_, l)| *l);
        for &(kind, tracks, artists) in lists {
            for (rank, &(album, n)) in tracks.iter().enumerate() {
                if let Some(t) = self.catalog.track(&track_id(self.service, album, n)) {
                    out.tracks.push(SuggestedTrack {
                        track: t.clone(),
                        kind,
                        weight: rank_weight(rank, tracks.len()),
                    });
                }
            }
            for (rank, key) in artists.iter().enumerate() {
                out.artists.push(SuggestedArtist {
                    artist: artist_ref(self.service, key),
                    kind,
                    weight: rank_weight(rank, artists.len()),
                });
            }
        }
        Ok(out)
    }

    fn artist_radio_now(&self, artist_id: &str) -> Result<Vec<StreamTrack>, StreamError> {
        let similar = self
            .catalog
            .similar_to(self.service, artist_id)
            .ok_or(StreamError::Unavailable)?;
        let mut lists = vec![self.catalog.top_of(artist_id)];
        lists.extend(self.tops(&similar));
        Ok(interleave(&lists, None, RADIO_LEN))
    }
}

fn ready<'a, T: Send + 'a>(value: T) -> BoxFuture<'a, T> {
    Box::pin(std::future::ready(value))
}

impl StreamService for FakeService {
    fn service(&self) -> Service {
        self.service
    }

    fn catalogue_only(&self) -> bool {
        self.service == Service::Deezer
    }

    fn account(&self) -> Account {
        self.lock().account.clone()
    }

    fn login(&self, input: LoginInput) -> BoxFuture<'_, Result<(), StreamError>> {
        if self.service == Service::Deezer {
            return ready(Err(StreamError::Unsupported));
        }
        let mut st = self.lock();
        let result = match input {
            LoginInput::Cookies(c) if c.trim().is_empty() => {
                st.account = Account::Failed("empty cookies".into());
                Err(StreamError::NotLoggedIn)
            }
            LoginInput::Playback | LoginInput::PersonalApp { .. }
                if self.service != Service::Spotify =>
            {
                Err(StreamError::Unsupported)
            }
            _ if st.no_premium => {
                st.account = Account::Failed(super::SPOTIFY_PREMIUM_REQUIRED.into());
                Err(StreamError::PremiumRequired)
            }
            _ => {
                st.account = Account::LoggedIn {
                    name: USER.into(),
                    premium: Some(true),
                    audio: true,
                };
                Ok(())
            }
        };
        ready(result)
    }

    fn logout(&self) {
        self.lock().account = Account::LoggedOut;
    }

    fn paused_until(&self) -> Option<std::time::SystemTime> {
        let st = self.lock();
        st.paused_until.filter(|t| *t > std::time::SystemTime::now())
    }

    fn search(&self, query: &str) -> BoxFuture<'_, Result<StreamSearch, StreamError>> {
        if let Err(e) = self.gate("search") {
            return ready(Err(e));
        }
        ready(Ok(self.search_now(query)))
    }

    fn search_kind(
        &self,
        query: &str,
        kind: SearchKind,
    ) -> BoxFuture<'_, Result<StreamSearch, StreamError>> {
        if let Err(e) = self.gate("search_kind") {
            return ready(Err(e));
        }
        if self.service != Service::Youtube && kind == SearchKind::Videos {
            return ready(Ok(StreamSearch::default()));
        }
        ready(Ok(only_kind(self.search_now(query), kind)))
    }

    fn track(&self, id: &str) -> BoxFuture<'_, Result<StreamTrack, StreamError>> {
        self.count("track");
        ready(self.track_now(id))
    }

    fn tracks(&self, ids: &[String]) -> BoxFuture<'_, Result<Vec<Option<StreamTrack>>, StreamError>> {
        self.count("tracks");
        ready(Ok(ids.iter().map(|id| self.track_now(id).ok()).collect()))
    }

    fn album(&self, id: &str) -> BoxFuture<'_, Result<StreamAlbum, StreamError>> {
        self.count("album");
        ready(
            self.catalog
                .album(id)
                .map(|a| StreamAlbum {
                    tracks: a.tracks.iter().map(|t| self.listed(t)).collect(),
                    ..a.clone()
                })
                .ok_or(StreamError::Unavailable),
        )
    }

    fn artist(&self, id: &str) -> BoxFuture<'_, Result<StreamArtist, StreamError>> {
        self.count("artist");
        ready(
            self.catalog
                .artist(id)
                .cloned()
                .ok_or(StreamError::Unavailable),
        )
    }

    fn playlist(&self, id: &str) -> BoxFuture<'_, Result<StreamPlaylist, StreamError>> {
        if let Err(e) = self.gate("playlist") {
            return ready(Err(e));
        }
        ready(
            self.lock()
                .playlists
                .iter()
                .find(|p| p.id.id == id)
                .cloned()
                .ok_or(StreamError::Unavailable),
        )
    }

    fn playlist_version(&self, id: &str) -> BoxFuture<'_, Result<Option<String>, StreamError>> {
        if let Err(e) = self.gate("playlist_version") {
            return ready(Err(e));
        }
        ready(
            self.lock()
                .playlists
                .iter()
                .find(|p| p.id.id == id)
                .map(|p| p.version.clone())
                .ok_or(StreamError::Unavailable),
        )
    }

    fn find_isrc(&self, isrc: &str) -> BoxFuture<'_, Result<Option<StreamTrack>, StreamError>> {
        self.count("find_isrc");
        let isrc = isrc.trim().to_uppercase();
        ready(Ok(self
            .catalog
            .all_tracks()
            .find(|t| t.isrc.as_deref() == Some(isrc.as_str()))
            .cloned()))
    }

    fn song_for_video(&self, video: &StreamTrack) -> BoxFuture<'_, Result<Option<StreamTrack>, StreamError>> {
        if let Err(e) = self.gate("song_for_video") {
            return ready(Err(e));
        }
        let id = video.id.id.as_str();
        if let Some(t) = self.catalog.track(id).filter(|t| t.kind == TrackKind::Song) {
            return ready(Ok(Some(t.clone())));
        }
        let song = self.lock().songs.get(id).cloned();
        ready(Ok(song.and_then(|s| self.catalog.track(&s).cloned())))
    }

    fn liked(&self) -> BoxFuture<'_, Result<Vec<StreamTrack>, StreamError>> {
        if let Err(e) = self.gate("liked") {
            return ready(Err(e));
        }
        let result = self.need_login().map(|()| {
            self.liked_ids()
                .iter()
                .filter_map(|id| self.catalog.track(id).cloned())
                .collect()
        });
        ready(result)
    }

    fn liked_head(&self) -> BoxFuture<'_, Result<LikedHead, StreamError>> {
        if let Err(e) = self.gate("liked_head") {
            return ready(Err(e));
        }
        let result = self.need_login().map(|()| {
            let ids = self.liked_ids();
            LikedHead {
                tracks: ids
                    .iter()
                    .take(LIKED_PAGE)
                    .filter_map(|id| self.catalog.track(id).cloned())
                    .collect(),
                total: u32::try_from(ids.len()).ok(),
                complete: ids.len() <= LIKED_PAGE,
            }
        });
        ready(result)
    }

    fn set_liked(&self, id: &str, liked: bool) -> BoxFuture<'_, Result<(), StreamError>> {
        self.count("set_liked");
        ready(self.set_liked_now(id, liked))
    }

    fn set_liked_many(&self, ids: &[String], liked: bool) -> BoxFuture<'_, Result<(), StreamError>> {
        self.count("set_liked_many");
        ready(ids.iter().try_for_each(|id| self.set_liked_now(id, liked)))
    }

    fn my_playlists(&self) -> BoxFuture<'_, Result<Vec<StreamPlaylist>, StreamError>> {
        if let Err(e) = self.gate("my_playlists") {
            return ready(Err(e));
        }
        let result = self.need_login().map(|()| {
            self.lock()
                .playlists
                .iter()
                .map(|p| StreamPlaylist {
                    tracks: Vec::new(),
                    ..p.clone()
                })
                .collect()
        });
        ready(result)
    }

    fn create_playlist(&self, name: &str) -> BoxFuture<'_, Result<String, StreamError>> {
        self.count("create_playlist");
        ready(self.create_now(name))
    }

    fn playlist_edit(
        &self,
        id: &str,
        add: &[String],
        remove: &[String],
    ) -> BoxFuture<'_, Result<Option<String>, StreamError>> {
        self.count("playlist_edit");
        ready(self.edit_now(id, add, remove))
    }

    fn recent(&self) -> BoxFuture<'_, Result<Vec<Played>, StreamError>> {
        self.count("recent");
        let result = self.need_login().map(|()| {
            RECENT
                .iter()
                .filter_map(|&(album, n, hours)| {
                    let track = self
                        .catalog
                        .track(&track_id(self.service, album, n))?
                        .clone();
                    Some(Played {
                        track,
                        at_ms: HISTORY_T0 + hours * HOUR_MS,
                    })
                })
                .collect()
        });
        ready(result)
    }

    /// Not counted as a call (the real one asks nothing until the first
    /// read) unless [`FakeService::rate_limit_next`] for `"open_audio"`
    /// makes it answer a rate limit.
    fn open_audio(&self, id: &str) -> Result<Box<dyn AudioStream>, StreamError> {
        if matches!(self.lock().account, Account::LoggedIn { audio: false, .. }) {
            return Err(StreamError::PremiumRequired);
        }
        if self.lock().limited.contains_key("open_audio") {
            self.gate("open_audio")?;
        }
        self.open_now(id)
            .map(|a| Box::new(a) as Box<dyn AudioStream>)
    }

    fn save_audio(&self, id: &str, dir: &Path) -> BoxFuture<'_, Result<PathBuf, StreamError>> {
        if self.service != Service::Youtube {
            return ready(Err(StreamError::Unsupported));
        }
        let path = dir.join(format!("{id}.wav"));
        let result = self.open_now(id).and_then(|mut audio| {
            let mut bytes = Vec::new();
            audio
                .read_to_end(&mut bytes)
                .map_err(|_| StreamError::Unavailable)?;
            std::fs::create_dir_all(dir)
                .and_then(|()| std::fs::write(&path, &bytes))
                .map_err(|e| StreamError::Network(e.to_string()))?;
            Ok(path)
        });
        ready(result)
    }

    /// YouTube: the WAV of a track opened before (kept whole "on disk")
    /// until [`StreamService::clear_cache`]; the others keep nothing.
    fn cached_audio(&self, id: &str) -> Option<Box<dyn AudioStream>> {
        if !self.lock().cached.contains(id) {
            return None;
        }
        let index = self.catalog.all_tracks().position(|t| t.id.id == id)?;
        let mut audio = FakeAudio::new(220.0 + 20.0 * index as f32, self.lock().audio_secs, false, None);
        audio.quality = Some(self.service.nominal_quality());
        Some(Box::new(audio))
    }

    fn clear_cache(&self) {
        self.lock().cached.clear();
    }

    /// Songs of the artists like the track's (a fixed table, two of
    /// [`NEW_ARTISTS`] by the id for others), taken in turn, at most 10.
    /// Deezer: the radio of the track's artist without it. Spotify:
    /// unsupported.
    fn related(&self, track_id: &str) -> BoxFuture<'_, Result<Vec<StreamTrack>, StreamError>> {
        ready(self.discover("related", || self.related_now(track_id)).map(|v| self.listed_all(v)))
    }

    /// The track's artist and the artists like it, taken in turn, at most
    /// 25, without the track. YouTube only.
    fn radio(&self, track_id: &str) -> BoxFuture<'_, Result<Vec<StreamTrack>, StreamError>> {
        ready(self.discover("radio", || self.radio_now(track_id)).map(|v| self.listed_all(v)))
    }

    fn similar_artists(&self, artist_id: &str) -> BoxFuture<'_, Result<Vec<StreamArtist>, StreamError>> {
        ready(self.discover("similar_artists", || self.similar_now(artist_id)))
    }

    /// The artist's songs in a stable "popularity" order, at most 10.
    fn artist_top(&self, artist_id: &str) -> BoxFuture<'_, Result<Vec<StreamTrack>, StreamError>> {
        ready(self.discover("artist_top", || self.top_now(artist_id)).map(|v| self.listed_all(v)))
    }

    /// The artist's top and the tops of the artists like it, taken in
    /// turn, at most 25.
    fn artist_radio(&self, artist_id: &str) -> BoxFuture<'_, Result<Vec<StreamTrack>, StreamError>> {
        ready(self.discover("artist_radio", || self.artist_radio_now(artist_id)).map(|v| self.listed_all(v)))
    }

    /// YouTube: home and history; Spotify: the tops and recently played;
    /// Deezer: none.
    fn suggestion_feeds(&self) -> &'static [SuggestionFeed] {
        match self.service {
            Service::Youtube => YOUTUBE_FEEDS,
            Service::Spotify => SPOTIFY_FEEDS,
            Service::Deezer | Service::Soundcloud => &[],
        }
    }

    /// The fixed lists of the feed (counted as `"suggestions"`, obeys
    /// [`FakeService::rate_limit_next`]); needs a login.
    fn suggestions(&self, feed: SuggestionFeed) -> BoxFuture<'_, Result<Suggestions, StreamError>> {
        ready(self.gate("suggestions").and_then(|()| self.suggestions_now(feed)))
    }

    fn soloist(&self) -> Option<SoloistStatus> {
        self.lock().soloist.clone()
    }

    fn soloist_act(&self, action: SoloistAction) {
        let mut st = self.lock();
        let Some(s) = st.soloist.as_mut() else { return };
        match action {
            SoloistAction::Download => {
                s.offer = false;
                s.phase = SoloistPhase::NeedsKey;
            }
            SoloistAction::SetKey(_) => {
                s.has_key = true;
                s.phase = SoloistPhase::Pairing;
            }
            SoloistAction::Enable | SoloistAction::Retry => s.phase = SoloistPhase::Pairing,
            SoloistAction::Disable => s.phase = SoloistPhase::Off,
            SoloistAction::AnswerOffer => s.offer = false,
        }
    }

    fn web_url(&self, kind: EntityKind, id: &str) -> String {
        match self.service {
            Service::Spotify => {
                let kind = match kind {
                    EntityKind::Track => "track",
                    EntityKind::Album => "album",
                    EntityKind::Artist => "artist",
                    EntityKind::Playlist => "playlist",
                };
                format!("https://open.spotify.com/{kind}/{id}")
            }
            Service::Youtube => match kind {
                EntityKind::Track => format!("https://music.youtube.com/watch?v={id}"),
                EntityKind::Album => format!("https://music.youtube.com/browse/{id}"),
                EntityKind::Artist => format!("https://music.youtube.com/channel/{id}"),
                EntityKind::Playlist => format!("https://music.youtube.com/playlist?list={id}"),
            },
            Service::Deezer => {
                let kind = match kind {
                    EntityKind::Track => "track",
                    EntityKind::Album => "album",
                    EntityKind::Artist => "artist",
                    EntityKind::Playlist => "playlist",
                };
                format!("https://www.deezer.com/{kind}/{id}")
            }
            Service::Soundcloud => format!("https://soundcloud.com/{}/{id}", kind.as_str()),
        }
    }
}

/// A sine as 16-bit mono WAV, generated on the first read.
struct FakeAudio {
    freq: f32,
    frames: u64,
    fail: bool,
    gain_db: Option<f32>,
    bytes: Option<Vec<u8>>,
    pos: u64,
    /// What the real service would play (for the player's label).
    quality: Option<String>,
}

impl FakeAudio {
    fn new(freq: f32, secs: f32, fail: bool, gain_db: Option<f32>) -> Self {
        Self {
            freq,
            frames: (f64::from(secs) * f64::from(AUDIO_RATE)).round() as u64,
            fail,
            gain_db,
            bytes: None,
            pos: 0,
            quality: None,
        }
    }

    fn len(&self) -> u64 {
        44 + self.frames * 2
    }

    fn bytes(&mut self) -> io::Result<&[u8]> {
        if self.fail {
            return Err(io::Error::other("fake: the track is not available"));
        }
        let (freq, frames) = (self.freq, self.frames);
        Ok(self.bytes.get_or_insert_with(|| wav(freq, frames)))
    }
}

/// RIFF WAVE, PCM 16-bit mono at [`AUDIO_RATE`], amplitude 0.25.
fn wav(freq: f32, frames: u64) -> Vec<u8> {
    let data_len = (frames * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&AUDIO_RATE.to_le_bytes());
    out.extend_from_slice(&(AUDIO_RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    let step = std::f64::consts::TAU * f64::from(freq) / f64::from(AUDIO_RATE);
    for n in 0..frames {
        let s = (0.25 * (step * n as f64).sin() * f64::from(i16::MAX)).round() as i16;
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

impl Read for FakeAudio {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let pos = self.pos;
        let bytes = self.bytes()?;
        let start = (pos as usize).min(bytes.len());
        let n = buf.len().min(bytes.len() - start);
        buf[..n].copy_from_slice(&bytes[start..start + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for FakeAudio {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let target = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.len().checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = target.ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        Ok(self.pos)
    }
}

impl AudioStream for FakeAudio {
    fn byte_len(&self) -> Option<u64> {
        Some(self.len())
    }

    fn format_hint(&self) -> &'static str {
        format::WAV
    }

    fn gain_db(&self) -> Option<f32> {
        self.gain_db
    }

    fn duration(&self) -> Option<Duration> {
        Some(Duration::from_secs_f64(
            self.frames as f64 / f64::from(AUDIO_RATE),
        ))
    }

    fn quality(&self) -> Option<String> {
        self.quality.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::task::{Context, Poll, Waker};

    /// Every fake future is ready on its first poll.
    fn now<T>(f: BoxFuture<'_, T>) -> T {
        let mut f = f;
        match f.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(v) => v,
            Poll::Pending => panic!("fake futures are always ready"),
        }
    }

    #[test]
    fn suggestions_follow_the_flavour() {
        let yt = FakeService::youtube();
        let home = now(yt.suggestions(SuggestionFeed::Home)).unwrap();
        assert_eq!(home.tracks[0].track.title, "Спокойная ночь");
        assert_eq!(home.tracks[0].kind, SuggestionKind::QuickPicks);
        assert!(home.tracks.iter().any(|t| t.track.artist_line() == "Sigur Rós"), "a song new to the catalog");
        assert!(home.artists.iter().any(|a| a.artist.name == "Björk" && a.kind == SuggestionKind::ListenAgain));
        assert_eq!(yt.calls("suggestions"), 1);
        assert_eq!(now(yt.suggestions(SuggestionFeed::TopTracksMonth)), Err(StreamError::Unsupported));
        let sp = FakeService::spotify();
        assert_eq!(sp.suggestion_feeds().len(), 5);
        let top = now(sp.suggestions(SuggestionFeed::TopTracksMonth)).unwrap();
        assert_eq!(top.tracks[0].weight, 1.0);
        assert!(top.tracks.iter().all(|t| t.track.id.service == Service::Spotify));
        let old = FakeService::spotify().without_top_scope();
        let top = now(old.suggestions(SuggestionFeed::TopArtistsMonth)).unwrap();
        assert!(top.needs_reconnect && top.artists.is_empty());
        assert!(!now(old.suggestions(SuggestionFeed::RecentlyPlayed)).unwrap().needs_reconnect);
        let out = FakeService::youtube().logged_out();
        assert_eq!(now(out.suggestions(SuggestionFeed::Home)), Err(StreamError::NotLoggedIn));
        assert!(FakeService::deezer().suggestion_feeds().is_empty());
    }

    #[test]
    fn catalog_is_deterministic_and_varied() {
        let a = FakeService::spotify();
        let b = FakeService::spotify();
        assert_eq!(a.tracks(), b.tracks());
        let names: Vec<String> = a.tracks().iter().map(StreamTrack::artist_line).collect();
        assert!(names.iter().any(|n| n == "Кино"));
        assert!(names.iter().any(|n| n == "宇多田ヒカル"));
        assert!(a.tracks().iter().any(|t| t.isrc.is_none()));
        assert!(a.tracks().iter().all(|t| t.track_no.is_some()));
        let album = now(a.album("sp-alb-gruppa")).unwrap();
        assert_eq!(album.tracks.len(), 4);
        assert_eq!(album.tracks[2].track_no, Some(3));
        let artist = now(a.artist("sp-art-utada")).unwrap();
        assert_eq!(artist.albums.len(), 2);
        assert_eq!(artist.albums[0].year, Some(2016), "newest first");
        assert!(artist.albums.iter().all(|al| al.tracks.is_empty()));
    }

    #[test]
    fn flavours_share_isrcs_not_ids() {
        let sp = FakeService::spotify();
        let yt = FakeService::youtube();
        let on_yt = now(yt.find_isrc("zzfak0100001")).unwrap().unwrap();
        let on_sp = now(sp.find_isrc("ZZFAK0100001")).unwrap().unwrap();
        assert_eq!(on_yt.title, on_sp.title);
        assert_ne!(on_yt.id, on_sp.id);
        assert_eq!(on_yt.id.service, Service::Youtube);
        assert!(now(yt.find_isrc("ZZFAK8800001")).unwrap().is_none());
        assert!(yt.tracks().iter().any(|t| t.kind == TrackKind::Video));
    }

    #[test]
    fn search_finds_cyrillic_and_japanese() {
        let sp = FakeService::spotify();
        let r = now(sp.search("группа")).unwrap();
        assert!(r.tracks.iter().any(|t| t.title == "Группа крови"));
        assert_eq!(r.albums.len(), 1);
        let r = now(sp.search("宇多田")).unwrap();
        assert_eq!(r.artists.len(), 1);
        assert_eq!(r.tracks.len(), 4);
        assert!(now(sp.search("  ")).unwrap().tracks.is_empty());
        assert_eq!(now(sp.search("вечер")).unwrap().playlists.len(), 1);
    }

    #[test]
    fn likes_and_playlists_are_mutable() {
        let sp = FakeService::spotify();
        let liked = now(sp.liked()).unwrap();
        assert_eq!(liked.len(), 3);
        now(sp.set_liked("sp-homogenic-3", true)).unwrap();
        assert_eq!(sp.liked_ids()[0], "sp-homogenic-3");
        now(sp.set_liked("sp-gruppa-1", false)).unwrap();
        assert!(!sp.liked_ids().contains(&"sp-gruppa-1".to_owned()));
        assert_eq!(
            now(sp.set_liked("nope", true)),
            Err(StreamError::Unavailable)
        );

        let mine = now(sp.my_playlists()).unwrap();
        assert_eq!(mine.len(), 2);
        assert!(mine.iter().all(|p| p.tracks.is_empty()));
        let id = now(sp.create_playlist("Новый")).unwrap();
        let v =
            now(sp.playlist_edit(&id, &["sp-gruppa-2".into(), "sp-michi-1".into()], &[])).unwrap();
        assert_eq!(v.as_deref(), Some("2"));
        let v = now(sp.playlist_edit(&id, &[], &["sp-gruppa-2".into()])).unwrap();
        assert_eq!(v.as_deref(), Some("3"));
        let p = now(sp.playlist(&id)).unwrap();
        assert_eq!(p.tracks.len(), 1);
        assert_eq!(p.tracks[0].title, "道");
        assert_eq!(
            now(sp.playlist_edit("sp-pl-mix", &[], &[])),
            Err(StreamError::Http { status: 403 })
        );
        let recent = now(sp.recent()).unwrap();
        assert_eq!(recent.len(), 3);
        assert!(recent[0].at_ms > recent[1].at_ms, "newest first");
    }

    #[test]
    fn login_state_gates_personal_data() {
        let sp = FakeService::spotify().logged_out();
        assert_eq!(sp.account(), Account::LoggedOut);
        assert_eq!(now(sp.liked()), Err(StreamError::NotLoggedIn));
        assert!(matches!(
            sp.open_audio("sp-gruppa-1"),
            Err(StreamError::NotLoggedIn)
        ));
        assert!(now(sp.search("кино")).is_ok(), "the catalog needs no login");
        now(sp.login(LoginInput::Browser)).unwrap();
        assert!(sp.account().is_logged_in());
        sp.logout();
        assert!(!sp.account().is_logged_in());

        let yt = FakeService::youtube().logged_out();
        assert!(
            yt.open_audio("yt-gruppa-1").is_ok(),
            "YouTube plays without login"
        );
    }

    #[test]
    fn audio_is_a_lazy_seekable_wav() {
        let sp: Arc<dyn StreamService> = Arc::new(FakeService::spotify());
        let mut a = sp.open_audio("sp-discovery-2").unwrap();
        assert_eq!(a.format_hint(), "wav");
        assert_eq!(a.gain_db(), Some(-2.0));
        assert_eq!(a.duration(), Some(Duration::from_secs(2)));
        let len = a.byte_len().unwrap();
        assert_eq!(a.seek(SeekFrom::End(0)).unwrap(), len);
        a.seek(SeekFrom::Start(0)).unwrap();
        let mut bytes = Vec::new();
        a.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes.len() as u64, len);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert!(matches!(
            sp.open_audio("missing"),
            Err(StreamError::Unavailable)
        ));

        let fake = FakeService::youtube();
        fake.set_unavailable("yt-gruppa-3", true);
        let mut a = fake
            .open_audio("yt-gruppa-3")
            .expect("opening needs no network");
        assert!(a.read(&mut [0; 16]).is_err(), "fails on the first read");
        assert_eq!(fake.opened(), vec!["yt-gruppa-3".to_owned()]);
    }

    #[test]
    fn save_audio_only_on_youtube_and_web_urls() {
        let dir = std::env::temp_dir().join(format!("nikgob-fake-{}", uuid::Uuid::now_v7()));
        let sp = FakeService::spotify();
        assert_eq!(
            now(sp.save_audio("sp-gruppa-1", &dir)),
            Err(StreamError::Unsupported)
        );
        let yt = FakeService::youtube();
        let path = now(yt.save_audio("yt-gruppa-1", &dir)).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > 44);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            sp.web_url(EntityKind::Album, "x"),
            "https://open.spotify.com/album/x"
        );
        assert_eq!(
            yt.web_url(EntityKind::Track, "v"),
            "https://music.youtube.com/watch?v=v"
        );
    }

    #[test]
    fn discovery_is_stable_and_leads_to_new_artists() {
        let yt = FakeService::youtube();
        let similar = now(yt.similar_artists("yt-art-kino")).unwrap();
        let names: Vec<&str> = similar.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Аквариум", "Наутилус Помпилиус", "Daft Punk"]);
        assert!(similar.iter().all(|a| a.albums.is_empty()));

        let related = now(yt.related("yt-gruppa-1")).unwrap();
        assert_eq!(related, now(FakeService::youtube().related("yt-gruppa-1")).unwrap(), "stable");
        assert!(!related.is_empty() && related.len() <= RELATED_LEN);
        assert!(related.iter().all(|t| t.id.id != "yt-gruppa-1"));
        let listed: Vec<String> = yt.tracks().iter().map(|t| t.id.id.clone()).collect();
        assert!(related.iter().any(|t| !listed.contains(&t.id.id)), "something new");
        // A track from discovery is a real track: it resolves and plays.
        let new = related.iter().find(|t| !listed.contains(&t.id.id)).unwrap();
        assert_eq!(now(yt.track(&new.id.id)).unwrap(), *new);
        assert!(yt.open_audio(&new.id.id).is_ok());
        assert!(now(yt.search("аквариум")).unwrap().artists.len() == 1);

        let radio = now(yt.radio("yt-gruppa-1")).unwrap();
        assert!(radio.len() > related.len());
        assert!(radio.iter().all(|t| t.id.id != "yt-gruppa-1"));
        assert_eq!(radio[0].artist_line(), "Кино", "the artist first");
        let mut ids: Vec<&str> = radio.iter().map(|t| t.id.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), radio.len(), "no repeats");

        let top = now(yt.artist_top("yt-art-daft")).unwrap();
        assert_eq!(top.len(), 4);
        assert!(top.iter().all(|t| t.artist_line() == "Daft Punk"));
        let radio = now(yt.artist_radio("yt-art-bjork")).unwrap();
        assert!(radio.iter().any(|t| t.artist_line() == "Sigur Rós"));
        assert_eq!(now(yt.artist_top("nope")), Err(StreamError::Unavailable));
        assert_eq!(now(yt.related("nope")), Err(StreamError::Unavailable));
        assert_eq!(yt.calls("related"), 2);

        // Any other artist still gets two similar ones.
        let big = FakeService::youtube().with_album("big", "Big", 3);
        assert_eq!(now(big.similar_artists("yt-art-big")).unwrap().len(), 2);
        assert_eq!(now(big.artist_top("yt-art-big")).unwrap().len(), 3);
    }

    #[test]
    fn discovery_supports_what_the_real_services_do() {
        let sp = FakeService::spotify();
        assert_eq!(now(sp.related("sp-gruppa-1")), Err(StreamError::Unsupported));
        assert_eq!(now(sp.similar_artists("sp-art-kino")), Err(StreamError::Unsupported));
        let dz = FakeService::deezer();
        assert_eq!(now(dz.radio("dz-gruppa-1")), Err(StreamError::Unsupported));
        let related = now(dz.related("dz-gruppa-1")).unwrap();
        assert!(!related.is_empty() && related.iter().all(|t| t.id.id != "dz-gruppa-1"));
        assert!(related.iter().all(|t| t.isrc.is_none()), "Deezer listings carry no ISRC");
        assert!(now(dz.artist_top("dz-art-utada")).unwrap().len() == 4);
        // Rate limits apply like to any call.
        dz.rate_limit_next("artist_top", 1, None);
        assert!(matches!(now(dz.artist_top("dz-art-utada")), Err(StreamError::RateLimited { .. })));
        assert!(now(dz.artist_top("dz-art-utada")).is_ok());
    }

    #[test]
    fn release_days_and_a_new_release() {
        let dz = FakeService::deezer().with_new_release("utada", "badmode", "BADモード", 12);
        let artist = now(dz.artist("dz-art-utada")).unwrap();
        let first = &artist.albums[0];
        assert_eq!(first.title, "BADモード");
        let age_days = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
            - first.released_at_ms().unwrap())
            / 86_400_000;
        assert_eq!(age_days, 12);
        assert_eq!(artist.albums[1].release_date.as_deref(), Some("2016-09-28"));
        let yt = FakeService::youtube().with_new_release("utada", "badmode", "BADモード", 12);
        let first = &now(yt.artist("yt-art-utada")).unwrap().albums[0];
        assert_eq!((first.release_date.as_deref(), first.year.is_some()), (None, true), "YouTube: the year only");
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_743), (2026, 10, 17));
    }

    #[test]
    fn played_youtube_audio_is_cached_whole() {
        let yt = FakeService::youtube();
        assert!(yt.cached_audio("yt-gruppa-1").is_none());
        yt.set_unavailable("yt-gruppa-2", true);
        let _ = yt.open_audio("yt-gruppa-1").unwrap();
        let _ = yt.open_audio("yt-gruppa-2").unwrap();
        let mut a = yt.cached_audio("yt-gruppa-1").expect("played: on disk");
        assert_eq!(a.format_hint(), format::WAV);
        let mut bytes = Vec::new();
        a.read_to_end(&mut bytes).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert!(yt.cached_audio("yt-gruppa-2").is_none(), "a failed one is not kept");
        assert_eq!(yt.cached_ids(), ["yt-gruppa-1"]);
        yt.clear_cache();
        assert!(yt.cached_audio("yt-gruppa-1").is_none());
        let sp = FakeService::spotify();
        let _ = sp.open_audio("sp-gruppa-1").unwrap();
        assert!(sp.cached_audio("sp-gruppa-1").is_none());
    }
}
