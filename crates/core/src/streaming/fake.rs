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

use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use super::{
    Account, AlbumRef, ArtistRef, AudioStream, BoxFuture, EntityKind, LikedHead, LoginInput,
    Played, RemoteId, SearchKind, Service, StreamAlbum, StreamArtist, StreamError, StreamPlaylist,
    StreamSearch, StreamService, StreamTrack, TrackKind, format, only_kind,
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

struct AlbumData {
    artist: &'static str,
    key: &'static str,
    title: &'static str,
    year: i32,
    release_type: ReleaseType,
    /// Title, length in seconds, ISRC.
    tracks: &'static [(&'static str, u64, Option<&'static str>)],
}

/// `(key, name)`.
const ARTISTS: &[(&str, &str)] = &[
    ("kino", "Кино"),
    ("utada", "宇多田ヒカル"),
    ("daft", "Daft Punk"),
    ("bjork", "Björk"),
];

const ALBUMS: &[AlbumData] = &[
    AlbumData {
        artist: "kino",
        key: "gruppa",
        title: "Группа крови",
        year: 1988,
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
        release_type: ReleaseType::Single,
        tracks: &[("道", 240, Some("ZZFAK1600001"))],
    },
    AlbumData {
        artist: "daft",
        key: "discovery",
        title: "Discovery",
        year: 2001,
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
        release_type: ReleaseType::Album,
        tracks: &[
            ("Hunter", 255, None),
            ("Jóga", 305, Some("ZZFAK9700002")),
            ("Bachelorette", 312, Some("ZZFAK9700003")),
        ],
    },
];

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

fn prefix(service: Service) -> &'static str {
    match service {
        Service::Spotify => "sp",
        Service::Youtube => "yt",
        Service::Deezer => "dz",
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
}

impl Catalog {
    fn build(service: Service) -> Self {
        let p = prefix(service);
        let artist_ref = |key: &str| {
            let name = ARTISTS
                .iter()
                .find(|(k, _)| *k == key)
                .map_or("", |(_, n)| n);
            ArtistRef::new(Some(RemoteId::new(service, format!("{p}-art-{key}"))), name)
        };
        let mut albums = Vec::new();
        let mut tracks = Vec::new();
        for a in ALBUMS {
            let album_id = RemoteId::new(service, format!("{p}-alb-{}", a.key));
            let cover = Some(format!("https://fake.invalid/{p}/{}.jpg", a.key));
            let mut album_ref = AlbumRef::new(Some(album_id.clone()), a.title);
            album_ref.artists = vec![artist_ref(a.artist)];
            album_ref.year = Some(a.year);
            let mut album_tracks = Vec::new();
            for (i, &(title, secs, isrc)) in a.tracks.iter().enumerate() {
                let n = i as u32 + 1;
                let mut t =
                    StreamTrack::new(RemoteId::new(service, track_id(service, a.key, n)), title);
                t.artists = vec![artist_ref(a.artist)];
                t.album = Some(album_ref.clone());
                t.duration_ms = Some(secs * 1000);
                t.isrc = match service {
                    Service::Spotify | Service::Deezer => isrc,
                    Service::Youtube => isrc.filter(|_| a.key == "discovery"),
                }
                .map(str::to_owned);
                t.disc_no = Some(1);
                t.track_no = Some(n);
                t.cover_url = cover.clone();
                album_tracks.push(t);
            }
            tracks.extend(album_tracks.iter().cloned());
            albums.push(StreamAlbum {
                id: album_id,
                title: a.title.to_owned(),
                artists: vec![artist_ref(a.artist)],
                year: Some(a.year),
                release_type: a.release_type,
                cover_url: cover,
                total_tracks: Some(a.tracks.len() as u32),
                tracks: album_tracks,
            });
        }
        if service == Service::Youtube {
            let mut v = StreamTrack::new(
                RemoteId::new(service, format!("{p}-video-kino")),
                "Группа крови (клип)",
            );
            v.artists = vec![artist_ref("kino")];
            v.duration_ms = Some(290_000);
            v.kind = TrackKind::Video;
            tracks.push(v);
        }
        let artists = ARTISTS
            .iter()
            .map(|&(key, name)| {
                let mut discography: Vec<StreamAlbum> = albums
                    .iter()
                    .filter(|al| al.artists.iter().any(|ar| ar.name == name))
                    .map(|al| StreamAlbum {
                        tracks: Vec::new(),
                        ..al.clone()
                    })
                    .collect();
                discography.sort_by_key(|al| std::cmp::Reverse(al.year));
                StreamArtist {
                    id: RemoteId::new(service, format!("{p}-art-{key}")),
                    name: name.to_owned(),
                    image_url: Some(format!("https://fake.invalid/{p}/artist-{key}.jpg")),
                    albums: discography,
                }
            })
            .collect();
        Self {
            artists,
            albums,
            tracks,
        }
    }

    fn track(&self, id: &str) -> Option<&StreamTrack> {
        self.tracks.iter().find(|t| t.id.id == id)
    }
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
            release_type: ReleaseType::Album,
            cover_url: None,
            total_tracks: Some(n),
            tracks,
        });
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
    /// `"playlist_version"`, `"search_kind"`) answer
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

    fn count(&self, name: &'static str) {
        *self.lock().calls.entry(name).or_default() += 1;
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
            .tracks
            .iter()
            .filter(|t| {
                hit(&t.title)
                    || t.artists.iter().any(|a| hit(&a.name))
                    || t.album.as_ref().is_some_and(|a| hit(&a.title))
            })
            .map(|t| self.listed(t))
            .collect();
        let albums = self
            .catalog
            .albums
            .iter()
            .filter(|a| hit(&a.title) || a.artists.iter().any(|ar| hit(&ar.name)))
            .map(|a| StreamAlbum {
                tracks: Vec::new(),
                ..a.clone()
            })
            .collect();
        let artists = self
            .catalog
            .artists
            .iter()
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
        let index = self
            .catalog
            .tracks
            .iter()
            .position(|t| t.id.id == id)
            .ok_or(StreamError::Unavailable)?;
        let mut st = self.lock();
        st.opened.push(id.to_owned());
        let fail = st.unavailable.contains(id) || !self.catalog.tracks[index].playable;
        let mut audio = FakeAudio::new(
            220.0 + 20.0 * index as f32,
            st.audio_secs,
            fail,
            (self.service == Service::Spotify).then_some(-2.0),
        );
        audio.quality = Some(self.service.nominal_quality());
        Ok(audio)
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
                .albums
                .iter()
                .find(|a| a.id.id == id)
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
                .artists
                .iter()
                .find(|a| a.id.id == id)
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
            .tracks
            .iter()
            .find(|t| t.isrc.as_deref() == Some(isrc.as_str()))
            .cloned()))
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

    fn open_audio(&self, id: &str) -> Result<Box<dyn AudioStream>, StreamError> {
        if matches!(self.lock().account, Account::LoggedIn { audio: false, .. }) {
            return Err(StreamError::PremiumRequired);
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
}
