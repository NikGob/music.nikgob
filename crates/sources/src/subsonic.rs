//! A Subsonic server as a [`Source`]: the client behind
//! [`crate::squidify`] (squidify.org or the user's own Navidrome) and
//! [`crate::dogmazic`] (play.dogmazic.net with the user's login). Which
//! server it is, its default address, its guest login and the key its
//! offers carry come from a [`Server`].
//!
//! The API (`/rest/<method>`, JSON): `search3` finds songs and albums
//! by words of the title, the album and the artist, `getAlbum` lists an
//! album's songs with their format (`suffix`, `bitRate`, `bitDepth`,
//! `samplingRate`) and size, `download` gives the original file. A login
//! the server allows only to listen (squidify.org's guest: `downloadRole`
//! false, «downloads are disabled», checked 2026-11-09) gets the same
//! file through `stream?format=raw`, which sends it as it is, without
//! transcoding. The login goes as a salted token (`t` = md5(password +
//! salt), `s` = salt), as the server's guide asks; a server that takes no
//! token (an LDAP login, error 41) gets the password hex-encoded instead.
//!
//! One request at a time with a pause between, one file at a time, and a
//! pause of the whole source when it turns us away ([`Breaker`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use md5::{Digest as _, Md5};
use nikgob_core::indexer::BoxFuture;
use nikgob_core::model::{Codec, quality_score};
use nikgob_core::source::{Fetched, Format, Offer, OfferScope, Progress, Source, SourceError, SourceKey, Want, WantScope};
use nikgob_metadata::text::similarity;
use serde::{Deserialize, Serialize};

use crate::album::{Patience, fetch_each, left_out};
use crate::http::{Breaker, Cookies, Pace, client, download, get_page, safe_name, track_file_name};
use crate::matching::{judge, same_artist};
use crate::text::{album_core, album_keys, catalogue_queries, main_artist, title_keys};

/// The API version asked for: the one Navidrome speaks.
const API: &str = "1.16.1";
const CLIENT: &str = "nikgob";
/// Songs of one search looked at.
const SONGS: u32 = 20;
/// Albums of one search looked at.
const ALBUM_HITS: u32 = 10;
/// Albums opened for one wish.
const ALBUMS: usize = 2;
/// A song or an album title this alike is the one.
const TITLE: f64 = 0.85;
/// A file this small may be the server's error answer instead of audio.
const ANSWER_MAX: u64 = 64 * 1024;

/// Which server a [`Subsonic`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Server {
    /// The key its offers carry.
    pub key: SourceKey,
    /// The address an empty one stands for.
    pub default_url: &'static str,
    /// The guest login it publishes (name and password alike), used when
    /// the user gives none.
    pub guest: Option<&'static str>,
}

pub struct Subsonic {
    server: Server,
    base: String,
    user: String,
    password: String,
    /// The name offers show: the source's own at its default address, else
    /// the host of the user's server.
    provider: String,
    http: reqwest::Client,
    pace: Pace,
    breaker: Breaker,
    cookies: Cookies,
    /// The server said downloads are off for this login: files come
    /// through `stream?format=raw` from then on.
    no_download: AtomicBool,
    /// The server takes no token: the password goes hex-encoded.
    plain: AtomicBool,
}

/// The server's address as given (`host`, `https://host/`, `…/rest`) →
/// `https://host`; empty is `default`.
pub fn base_of(url: &str, default: &str) -> String {
    let u = url.trim().trim_end_matches('/');
    let u = if u.is_empty() { default } else { u };
    let u = u.strip_suffix("/rest").unwrap_or(u).trim_end_matches('/');
    if u.starts_with("http://") || u.starts_with("https://") { u.to_owned() } else { format!("https://{u}") }
}

fn md5_hex(s: &str) -> String {
    Md5::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// The token of a login: md5 of the password and the salt, in hex.
pub fn token(password: &str, salt: &str) -> String {
    md5_hex(&format!("{password}{salt}"))
}

/// A new salt for each request.
fn salt() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let n = N.fetch_add(1, Ordering::Relaxed);
    md5_hex(&format!("{nanos}:{n}:{}", std::process::id()))[..12].to_owned()
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

impl Subsonic {
    /// `server` at `url` (empty: its default address) with this login
    /// (empty: its guest's), one request per `every`.
    pub fn new(server: Server, url: &str, user: &str, password: &str, every: Duration) -> Self {
        let base = base_of(url, server.default_url);
        let (user, password) = match server.guest {
            Some(g) if user.trim().is_empty() => (g.to_owned(), g.to_owned()),
            _ => (user.trim().to_owned(), password.to_owned()),
        };
        let provider = if base == base_of("", server.default_url) {
            server.key.display_name().to_owned()
        } else {
            url::Url::parse(&base).ok().and_then(|u| u.host_str().map(str::to_owned)).unwrap_or_else(|| base.clone())
        };
        Self {
            server,
            base,
            user,
            password,
            provider,
            http: client(),
            pace: Pace::new(every),
            breaker: Breaker::default(),
            cookies: Cookies::default(),
            no_download: AtomicBool::new(false),
            plain: AtomicBool::new(false),
        }
    }

    /// The login part of a request's query.
    fn auth(&self) -> String {
        let user = enc(&self.user);
        if self.plain.load(Ordering::Relaxed) {
            let hex: String = self.password.bytes().map(|b| format!("{b:02x}")).collect();
            format!("u={user}&p=enc:{hex}&v={API}&c={CLIENT}&f=json")
        } else {
            let s = salt();
            format!("u={user}&t={}&s={s}&v={API}&c={CLIENT}&f=json", token(&self.password, &s))
        }
    }

    /// A request's URL (it holds the login: never logged).
    fn url(&self, method: &str, params: &[(&str, &str)]) -> String {
        let mut u = format!("{}/rest/{method}?{}", self.base, self.auth());
        for (k, v) in params {
            u.push('&');
            u.push_str(k);
            u.push('=');
            u.push_str(&enc(v));
        }
        u
    }

    /// One API call, at the pace, through the breaker; a server that
    /// takes no token is asked once more with the password.
    async fn call(&self, method: &str, params: &[(&str, &str)]) -> Result<Body, SourceError> {
        self.breaker.check()?;
        let mut r = self.answer(method, params).await;
        if let Ok(Err(e)) = &r
            && e.code == 41
            && !self.plain.swap(true, Ordering::Relaxed)
        {
            r = self.answer(method, params).await;
        }
        let r = match r {
            Ok(Ok(body)) => Ok(body),
            Ok(Err(e)) => Err(api_error(&e)),
            Err(e) => Err(e),
        };
        self.breaker.note(&r);
        r
    }

    async fn answer(&self, method: &str, params: &[(&str, &str)]) -> Result<Result<Body, ApiError>, SourceError> {
        self.pace.wait().await;
        let text = get_page(&self.http, &self.url(method, params), Some(&self.cookies)).await?;
        parse(&text)
    }

    async fn search3(&self, query: &str, albums: u32, songs: u32) -> Result<Found, SourceError> {
        let (albums, songs) = (albums.to_string(), songs.to_string());
        let body = self
            .call("search3", &[("query", query), ("artistCount", "0"), ("albumCount", albums.as_str()), ("songCount", songs.as_str())])
            .await?;
        Ok(body.search.unwrap_or_default())
    }

    async fn album(&self, id: &str) -> Result<Album, SourceError> {
        self.call("getAlbum", &[("id", id)])
            .await?
            .album
            .ok_or_else(|| SourceError::Parse("an album answer without the album".into()))
    }

    /// Albums named like `wanted`, the wish's artist's first: "artist
    /// album" searched, else the album alone (a soundtrack's album artist
    /// is often «Various Artists»).
    async fn albums_like(&self, want: &Want, wanted: &str) -> Result<Vec<Album>, SourceError> {
        let core = album_core(wanted);
        let artist = main_artist(want.artist());
        let mut queries = vec![format!("{artist} {core}").trim().to_owned(), core.clone()];
        queries.dedup();
        for q in queries.iter().filter(|q| !q.is_empty()) {
            let mut hits: Vec<Album> = self.search3(q, ALBUM_HITS, 0).await?.album.into_iter().filter(|a| album_named(&a.name, wanted)).collect();
            hits.sort_by_key(|a| !a.artist.as_deref().is_some_and(|n| same_artist(n, want)));
            if !hits.is_empty() {
                hits.truncate(ALBUMS);
                return Ok(hits);
            }
        }
        Ok(Vec::new())
    }

    fn track_offer(&self, s: &Song, album: Option<&str>) -> Offer {
        let artist = s.artist.clone().filter(|a| !a.trim().is_empty());
        let title = match &artist {
            Some(a) => format!("{a} — {}", s.title),
            None => s.title.clone(),
        };
        let handle = Handle {
            song: Some(s.id.clone()),
            file: Some(format!("{}.{}", safe_name(&s.title), ext_of(s))),
            album: None,
        };
        Offer {
            source: self.server.key,
            provider: self.provider.clone(),
            id: s.id.clone(),
            title,
            artist,
            track_title: Some(s.title.clone()),
            album: s.album.clone().or_else(|| album.map(str::to_owned)),
            duration_ms: s.duration.map(|d| d * 1000),
            isrc: isrc_of(&s.isrc),
            scope: OfferScope::Track,
            format: Some(format_of(s)),
            size_bytes: s.size.filter(|b| *b > 0),
            seeders: None,
            info_url: None,
            handle: serde_json::to_string(&handle).unwrap_or_default(),
        }
    }

    fn album_offer(&self, a: &Album) -> Offer {
        let songs: Vec<&Song> = a.song.iter().filter(|s| playable(s)).collect();
        let size: u64 = songs.iter().filter_map(|s| s.size).sum();
        let artist = a.artist.clone().filter(|n| !n.trim().is_empty());
        let handle = Handle { song: None, file: None, album: Some(a.id.clone()) };
        Offer {
            source: self.server.key,
            provider: self.provider.clone(),
            id: a.id.clone(),
            title: match &artist {
                Some(n) => format!("{n} — {}", a.name),
                None => a.name.clone(),
            },
            artist,
            track_title: None,
            album: Some(a.name.clone()),
            duration_ms: None,
            isrc: None,
            scope: OfferScope::Album,
            format: album_format(&songs),
            size_bytes: (size > 0).then_some(size),
            seeders: None,
            info_url: None,
            handle: serde_json::to_string(&handle).unwrap_or_default(),
        }
    }

    async fn search_track(&self, want: &Want) -> Result<Vec<Offer>, SourceError> {
        let mut out: Vec<Offer> = Vec::new();
        let sure = |out: &[Offer]| out.iter().any(|o| judge(o, want).is_some_and(|v| v.sure));
        for q in catalogue_queries(want).iter().filter(|q| !q.is_empty()) {
            let found = self.search3(q, 0, SONGS).await?;
            out.extend(found.song.iter().filter(|s| playable(s)).map(|s| self.track_offer(s, None)));
            if sure(out.as_slice()) {
                break;
            }
        }
        // The album's own songs: a soundtrack's are credited to whoever
        // plays them, not to the artist the wish names.
        if !sure(out.as_slice())
            && let (Some(album), Some(title)) = (want.album.as_deref(), want.title.as_deref())
        {
            let wanted = title_keys(title);
            for a in self.albums_like(want, album).await? {
                let full = self.album(&a.id).await?;
                for s in full.song.iter().filter(|s| playable(s)) {
                    let sim = title_keys(&s.title)
                        .iter()
                        .flat_map(|g| wanted.iter().map(move |w| similarity(g, w)))
                        .fold(0.0, f64::max);
                    let numbered = want.track_no > 0 && s.track == Some(want.track_no) && s.disc_number.unwrap_or(1) == want.disc_no.max(1);
                    if sim >= TITLE || (numbered && sim >= 0.5) {
                        out.push(self.track_offer(s, Some(full.name.as_str())));
                    }
                }
            }
        }
        let mut seen: Vec<String> = Vec::new();
        out.retain(|o| {
            let new = !seen.contains(&o.id);
            seen.push(o.id.clone());
            new
        });
        Ok(out)
    }

    async fn search_album(&self, want: &Want) -> Result<Vec<Offer>, SourceError> {
        let Some(wanted) = want.album.as_deref().or(want.title.as_deref()) else { return Ok(Vec::new()) };
        let mut out = Vec::new();
        for a in self.albums_like(want, wanted).await? {
            // The search's album says nothing of the format: its songs do.
            let full = self.album(&a.id).await?;
            if full.song.iter().any(|s| playable(&s)) {
                out.push(self.album_offer(&full));
            }
        }
        Ok(out)
    }

    /// One file into `dest`: `download`, or the raw stream once the
    /// server said downloads are off for this login.
    async fn file(&self, id: &str, dest: &Path, progress: &Progress) -> Result<PathBuf, SourceError> {
        self.breaker.check()?;
        if !self.no_download.load(Ordering::Relaxed) {
            match self.get_file("download", &[("id", id)], dest, progress).await {
                Ok(Err(e)) if e.code == 50 => {
                    log::info!("{}: downloads are off for this login, the original file comes through the stream", self.provider);
                    self.no_download.store(true, Ordering::Relaxed);
                }
                r => {
                    let r = flatten(r);
                    self.breaker.note(&r);
                    return r;
                }
            }
        }
        let r = flatten(self.get_file("stream", &[("id", id), ("format", "raw")], dest, progress).await);
        self.breaker.note(&r);
        r
    }

    async fn get_file(&self, method: &str, params: &[(&str, &str)], dest: &Path, progress: &Progress) -> Result<Result<PathBuf, ApiError>, SourceError> {
        self.pace.wait().await;
        let path = download(&self.http, &self.url(method, params), dest, progress).await?;
        // An error comes as a small answer of the API, not as audio.
        match answer_in(&path) {
            None => Ok(Ok(path)),
            Some(e) => {
                let _ = tokio::fs::remove_file(&path).await;
                Ok(Err(e))
            }
        }
    }
}

fn flatten(r: Result<Result<PathBuf, ApiError>, SourceError>) -> Result<PathBuf, SourceError> {
    match r {
        Ok(Ok(p)) => Ok(p),
        Ok(Err(e)) => Err(api_error(&e)),
        Err(e) => Err(e),
    }
}

/// What a fetch needs.
#[derive(Serialize, Deserialize)]
struct Handle {
    /// A song's id (a track offer) and the file name it gets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    song: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    /// An album's id (an album offer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    album: Option<String>,
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "subsonic-response")]
    body: Body,
}

/// The parts of an answer read here.
#[derive(Deserialize, Default, Debug)]
struct Body {
    #[serde(default)]
    status: String,
    #[serde(default)]
    error: Option<ApiError>,
    #[serde(default, rename = "searchResult3")]
    search: Option<Found>,
    #[serde(default)]
    album: Option<Album>,
}

/// An error the API answers with (HTTP 200, `status: failed`).
#[derive(Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct ApiError {
    #[serde(default)]
    pub code: u32,
    #[serde(default)]
    pub message: String,
}

#[derive(Deserialize, Default, Debug)]
struct Found {
    #[serde(default)]
    album: Vec<Album>,
    #[serde(default)]
    song: Vec<Song>,
}

/// A song as the API lists it.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Song {
    #[serde(deserialize_with = "id_of")]
    pub id: String,
    pub title: String,
    pub album: Option<String>,
    pub artist: Option<String>,
    pub track: Option<u32>,
    pub disc_number: Option<u32>,
    pub size: Option<u64>,
    pub content_type: Option<String>,
    pub suffix: Option<String>,
    /// Seconds.
    pub duration: Option<u32>,
    /// kbps.
    pub bit_rate: Option<u32>,
    /// Hz (OpenSubsonic).
    pub sampling_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    /// A string, or a list of them (OpenSubsonic).
    pub isrc: serde_json::Value,
    pub is_video: Option<bool>,
}

/// An album as the API lists it (with its songs from `getAlbum`).
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Album {
    #[serde(deserialize_with = "id_of")]
    pub id: String,
    pub name: String,
    pub artist: Option<String>,
    pub song_count: Option<u32>,
    pub year: Option<u32>,
    pub song: Vec<Song>,
}

/// An id, a string on most servers, a number on some.
fn id_of<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::String(s) => s,
        serde_json::Value::Null => String::new(),
        v => v.to_string(),
    })
}

/// The answer of `text`: its body when it is «ok», the error it says
/// otherwise.
fn parse(text: &str) -> Result<Result<Body, ApiError>, SourceError> {
    let env: Envelope = serde_json::from_str(text).map_err(|e| SourceError::Parse(format!("not a Subsonic answer: {e}")))?;
    let body = env.body;
    if body.status.eq_ignore_ascii_case("ok") {
        Ok(Ok(body))
    } else {
        Ok(Err(body.error.clone().unwrap_or_else(|| ApiError { code: 0, message: "failed".into() })))
    }
}

/// An API error as a source error: a login turned away is a block (the
/// source sits out), a right the login lacks a 403, a missing thing a 404.
fn api_error(e: &ApiError) -> SourceError {
    match e.code {
        40..=44 => SourceError::Blocked(format!("the server refused the login: {}", e.message)),
        50 => SourceError::Http { status: 403 },
        70 => SourceError::Http { status: 404 },
        _ => SourceError::Parse(format!("the server said no ({}): {}", e.code, e.message)),
    }
}

/// The API's answer a downloaded file holds instead of audio: the error
/// it says (an «ok» answer without audio counts as one too).
fn answer_in(path: &Path) -> Option<ApiError> {
    if std::fs::metadata(path).ok()?.len() > ANSWER_MAX {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let text = std::str::from_utf8(&bytes).ok()?.trim_start();
    if text.starts_with('{') {
        return match parse(text) {
            Ok(Err(e)) => Some(e),
            Ok(Ok(_)) => Some(ApiError { code: 0, message: "an answer instead of audio".into() }),
            Err(_) => None,
        };
    }
    if text.starts_with("<?xml") || text.starts_with("<subsonic-response") {
        let code = text
            .split("code=\"")
            .nth(1)
            .and_then(|r| r.split('"').next())
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        return Some(ApiError { code, message: "an answer instead of audio".into() });
    }
    None
}

fn playable(s: &&Song) -> bool {
    !s.id.is_empty() && !s.is_video.unwrap_or(false)
}

fn isrc_of(v: &serde_json::Value) -> Option<String> {
    let s = match v {
        serde_json::Value::String(s) => Some(s.as_str()),
        serde_json::Value::Array(a) => a.iter().find_map(|x| x.as_str()),
        _ => None,
    }?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_owned())
}

/// The codec of a song, from its suffix (else its content type).
fn codec_of(s: &Song) -> Codec {
    let suffix = s.suffix.as_deref().unwrap_or_default().trim().to_ascii_lowercase();
    let ct = s.content_type.as_deref().unwrap_or_default().to_ascii_lowercase();
    match suffix.as_str() {
        "flac" => Codec::Flac,
        "mp3" => Codec::Mp3,
        "opus" => Codec::Opus,
        "ogg" | "oga" if ct.contains("opus") => Codec::Opus,
        "ogg" | "oga" => Codec::Vorbis,
        "m4a" | "mp4" | "aac" | "alac" if ct.contains("alac") || s.bit_depth.is_some_and(|b| b > 0) => Codec::Alac,
        "m4a" | "mp4" | "aac" => Codec::Aac,
        "wav" => Codec::Wav,
        "" => match ct.rsplit('/').next().unwrap_or_default() {
            "x-flac" => Codec::Flac,
            "mpeg" | "mp3" => Codec::Mp3,
            other => Codec::parse(other),
        },
        other => Codec::parse(other),
    }
}

/// The audio a song is, as the server says.
fn format_of(s: &Song) -> Format {
    let codec = codec_of(s);
    if codec.is_lossless() {
        Format {
            bit_depth: s.bit_depth.filter(|b| *b > 0),
            sample_rate: s.sampling_rate.filter(|r| *r > 0),
            ..Format::new(codec)
        }
    } else {
        Format { bitrate_kbps: s.bit_rate.filter(|b| *b > 0), ..Format::new(codec) }
    }
}

/// An album's format: its worst song's (what the whole album is at least).
fn album_format(songs: &[&Song]) -> Option<Format> {
    songs
        .iter()
        .map(|s| format_of(s))
        .min_by_key(|f| quality_score(&f.codec, f.bitrate_kbps, f.sample_rate, f.bit_depth, false))
}

/// The extension a song's file gets.
fn ext_of(s: &Song) -> String {
    let suffix = s.suffix.as_deref().unwrap_or_default().trim().to_ascii_lowercase();
    if !suffix.is_empty() && suffix.len() <= 5 && suffix.chars().all(|c| c.is_ascii_alphanumeric()) {
        return suffix;
    }
    match codec_of(s) {
        Codec::Vorbis => "ogg".into(),
        Codec::Aac | Codec::Alac => "m4a".into(),
        Codec::Other(_) => "bin".into(),
        c => c.as_str().to_owned(),
    }
}

/// An album name alike the wished one.
fn album_named(name: &str, wanted: &str) -> bool {
    let want_keys = album_keys(wanted);
    album_keys(name).iter().any(|g| want_keys.iter().any(|w| similarity(g, w) >= TITLE))
}

impl Source for Subsonic {
    fn key(&self) -> SourceKey {
        self.server.key
    }

    fn name(&self) -> &str {
        &self.provider
    }

    fn search<'a>(&'a self, want: &'a Want) -> BoxFuture<'a, Result<Vec<Offer>, SourceError>> {
        Box::pin(async move {
            match want.scope {
                WantScope::Track => self.search_track(want).await,
                WantScope::Album => self.search_album(want).await,
            }
        })
    }

    fn fetch<'a>(&'a self, offer: &'a Offer, want: &'a Want, dest: &'a Path, progress: Progress) -> BoxFuture<'a, Result<Fetched, SourceError>> {
        Box::pin(async move {
            let h: Handle = serde_json::from_str(&offer.handle).map_err(|e| SourceError::Parse(e.to_string()))?;
            let files: Vec<(String, String)> = match (h.song, h.album) {
                (Some(id), _) => {
                    let name = h.file.unwrap_or_else(|| format!("{}.flac", safe_name(offer.track_title.as_deref().unwrap_or(&offer.title))));
                    vec![(id, name)]
                }
                (None, Some(id)) => {
                    let album = self.album(&id).await?;
                    let all: Vec<(String, String, String)> = album
                        .song
                        .iter()
                        .filter(|s| playable(s))
                        .map(|s| {
                            let name = track_file_name(s.disc_number.unwrap_or(1), s.track.unwrap_or(0), &s.title);
                            (s.id.clone(), format!("{name}.{}", ext_of(s)), s.title.clone())
                        })
                        .collect();
                    left_out(all, &want.got)
                }
                (None, None) => return Err(SourceError::NothingToFetch),
            };
            let before = Arc::new(AtomicU64::new(0));
            fetch_each(files, Patience::default(), |id, name| {
                let (before, progress) = (before.clone(), progress.clone());
                async move {
                    let base = before.load(Ordering::Relaxed);
                    let step: Progress = Arc::new(move |done, _| progress(base + done, None));
                    let path = self.file(&id, &dest.join(&name), &step).await?;
                    before.fetch_add(std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0), Ordering::Relaxed);
                    Ok(path)
                }
            })
            .await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQUIDIFY: Server = Server { key: SourceKey::Squidify, default_url: "https://www.squidify.org", guest: Some("Guest") };
    const DOGMAZIC: Server = Server { key: SourceKey::Dogmazic, default_url: "https://play.dogmazic.net", guest: None };

    #[test]
    fn the_address_is_made_whole() {
        let d = SQUIDIFY.default_url;
        assert_eq!(base_of("", d), d);
        assert_eq!(base_of("  https://www.squidify.org/ ", d), d);
        assert_eq!(base_of("https://music.example/rest/", d), "https://music.example");
        assert_eq!(base_of("navi.example:4533", d), "https://navi.example:4533");
        assert_eq!(base_of("http://192.168.1.5:4533", d), "http://192.168.1.5:4533");
    }

    /// The token as the Subsonic API defines it: md5("Guest" + salt).
    #[test]
    fn the_login_goes_as_a_salted_token() {
        assert_eq!(token("Guest", "abc123xyz"), "aa81028dd176fc876d6f7e48288d6e8f");
        let s = Subsonic::new(SQUIDIFY, "", "", "", Duration::ZERO);
        let auth = s.auth();
        assert!(auth.starts_with("u=Guest&t="), "{auth}");
        assert!(auth.contains("&s=") && auth.ends_with("&v=1.16.1&c=nikgob&f=json"), "{auth}");
        assert!(!auth.contains("p="), "no password in the URL");
        assert_ne!(salt(), salt(), "a new salt each time");
        assert_eq!(s.provider, "Squidify");
        assert_eq!(s.key(), SourceKey::Squidify);
        // A server that takes no token: the password hex-encoded.
        let own = Subsonic::new(SQUIDIFY, "https://navi.example", "me", "pw", Duration::ZERO);
        own.plain.store(true, Ordering::Relaxed);
        assert_eq!(own.auth(), "u=me&p=enc:7077&v=1.16.1&c=nikgob&f=json");
        assert_eq!(own.provider, "navi.example");
    }

    /// Another server: its own key and name, and no guest of someone else's.
    #[test]
    fn a_server_is_its_own_source() {
        let d = Subsonic::new(DOGMAZIC, "", "me", "pw", Duration::ZERO);
        assert_eq!((d.key(), d.provider.as_str(), d.base.as_str()), (SourceKey::Dogmazic, "Dogmazic", "https://play.dogmazic.net"));
        assert!(d.auth().starts_with("u=me&t="));
        let song = Song { id: "7".into(), title: "Argile".into(), suffix: Some("ogg".into()), bit_rate: Some(192), ..Song::default() };
        let o = d.track_offer(&song, None);
        assert_eq!(o.source, SourceKey::Dogmazic);
        assert_eq!(o.format.unwrap().label(), "VORBIS 192");
    }

    #[test]
    fn errors_of_the_api_are_told_apart() {
        let err = |code: u32| api_error(&ApiError { code, message: "x".into() });
        assert!(matches!(err(40), SourceError::Blocked(_)));
        assert_eq!(err(50), SourceError::Http { status: 403 });
        assert_eq!(err(70), SourceError::Http { status: 404 });
        assert!(matches!(err(0), SourceError::Parse(_)));
        let denied = r#"{"subsonic-response":{"status":"failed","version":"1.16.1","type":"navidrome","serverVersion":"0.64.2 (10114574)","openSubsonic":true,"error":{"code":50,"message":"downloads are disabled"}}}"#;
        assert_eq!(parse(denied).unwrap().unwrap_err(), ApiError { code: 50, message: "downloads are disabled".into() });
        assert!(parse("<html>").is_err());
    }

    #[test]
    fn formats_come_from_the_songs() {
        let song = |suffix: &str, ct: &str, kbps: u32, bits: u8, hz: u32| Song {
            id: "1".into(),
            suffix: Some(suffix.into()),
            content_type: Some(ct.into()),
            bit_rate: Some(kbps),
            bit_depth: Some(bits),
            sampling_rate: Some(hz),
            ..Song::default()
        };
        assert_eq!(format_of(&song("flac", "audio/flac", 953, 16, 44_100)).label(), "FLAC");
        assert_eq!(format_of(&song("flac", "audio/flac", 2900, 24, 96_000)).label(), "FLAC 24/96");
        assert_eq!(format_of(&song("mp3", "audio/mpeg", 320, 0, 44_100)).label(), "MP3 320");
        assert_eq!(codec_of(&song("m4a", "audio/mp4", 256, 0, 44_100)), Codec::Aac);
        assert_eq!(codec_of(&song("m4a", "audio/mp4", 900, 16, 44_100)), Codec::Alac);
        assert_eq!(codec_of(&song("", "audio/flac", 900, 16, 44_100)), Codec::Flac);
        assert_eq!(ext_of(&song("", "audio/mpeg", 320, 0, 44_100)), "mp3");
        // An album is what its worst song is.
        let (a, b) = (song("flac", "audio/flac", 953, 16, 44_100), song("mp3", "audio/mpeg", 320, 0, 44_100));
        assert_eq!(album_format(&[&a, &b]).unwrap().label(), "MP3 320");
        assert_eq!(album_format(&[&a]).unwrap().label(), "FLAC");
        assert_eq!(isrc_of(&serde_json::json!(["JPVI00123456"])).as_deref(), Some("JPVI00123456"));
        assert_eq!(isrc_of(&serde_json::json!([])), None);
        assert_eq!(isrc_of(&serde_json::json!("")), None);
    }

    #[test]
    fn an_error_answer_saved_as_a_file_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("01 - Tank!.flac");
        std::fs::write(&p, r#"{"subsonic-response":{"status":"failed","version":"1.16.1","error":{"code":50,"message":"downloads are disabled"}}}"#).unwrap();
        assert_eq!(answer_in(&p).map(|e| e.code), Some(50));
        std::fs::write(&p, r#"<?xml version="1.0"?><subsonic-response status="failed"><error code="70" message="nf"/></subsonic-response>"#).unwrap();
        assert_eq!(answer_in(&p).map(|e| e.code), Some(70));
        std::fs::write(&p, b"fLaC\0\0\0\x22rest").unwrap();
        assert_eq!(answer_in(&p), None);
    }

    #[test]
    fn album_names_alike() {
        assert!(album_named("Cowboy Bebop OST 1", "COWBOY BEBOP OST 1"));
        assert!(album_named("Cowboy Bebop Original Soundtrack 1", "Cowboy Bebop OST 1"));
        assert!(!album_named("Samurai Champloo Music Record: Departure", "Cowboy Bebop OST 1"));
    }
}
