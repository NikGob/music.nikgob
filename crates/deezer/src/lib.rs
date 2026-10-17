//! The Deezer catalogue as a [`StreamService`], over Deezer's public API
//! (`api.deezer.com`: no key, no login).
//!
//! * **Catalogue only** ([`StreamService::catalogue_only`]): search, tracks
//!   (with their ISRC), albums, artists with their discography, playlists,
//!   ISRC lookups. There is no account ([`Account::LoggedOut`] always, and
//!   [`StreamService::login`] is [`StreamError::Unsupported`]) and no sound:
//!   [`StreamService::open_audio`] is `Unsupported`, the runtime plays a
//!   Deezer track from YouTube Music the way it plays a Spotify track
//!   without Premium. Likes stay in the library; nothing is sent to Deezer.
//! * **Requests**: at most 4 at once and about 8 a second ([`limit`]); a
//!   quota error (`error.code 4`) pauses the service for 5 s
//!   ([`StreamService::paused_until`]). The «Всё» search asks three
//!   categories ([`StreamService::search_top`]), the service's own chip also
//!   the playlists. Album tracks come without ISRCs and are not fetched one
//!   by one: a track gets its ISRC when it is looked up alone
//!   ([`StreamService::track`]), which the runtime does when it plays or
//!   likes one.
//! * **Cache**: answers are kept in memory (searches 10 minutes, tracks,
//!   albums and artists an hour, playlists 10 minutes).
//!
//! Every future runs on the service's own small tokio runtime (so the pooled
//! connections and timers belong to one runtime) and is awaited through a
//! handle that cancels it when dropped; callers may poll on any executor.

mod api;
mod cache;
mod limit;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use nikgob_core::streaming::{
    Account, AudioStream, BoxFuture, EntityKind, LoginInput, Played, SearchKind, SearchParts,
    Service, StreamAlbum, StreamArtist, StreamError, StreamPlaylist, StreamSearch, StreamService,
    StreamTrack,
};
use serde::de::DeserializeOwned;
use tokio::task::JoinHandle;

use cache::TtlCache;

/// The public API.
pub const API_BASE: &str = "https://api.deezer.com";
/// Hits per search category.
const SEARCH_LIMIT: u32 = 25;
/// Albums of an artist read for its discography.
const ARTIST_ALBUMS: u32 = 100;
/// Tracks of a playlist read at most (pages of [`PAGE`]).
const PLAYLIST_MAX: usize = 2_000;
const PAGE: u32 = 100;
const SEARCH_TTL: Duration = Duration::from_secs(10 * 60);
const ITEM_TTL: Duration = Duration::from_secs(3600);
const PLAYLIST_TTL: Duration = Duration::from_secs(10 * 60);

/// Where the service asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeezerConfig {
    /// [`API_BASE`]; a local mock server in tests.
    pub base_url: String,
}

impl Default for DeezerConfig {
    fn default() -> Self {
        Self {
            base_url: API_BASE.to_owned(),
        }
    }
}

struct Inner {
    http: reqwest::Client,
    base: String,
    gate: limit::Gate,
    searches: TtlCache<StreamSearch>,
    tracks: TtlCache<StreamTrack>,
    albums: TtlCache<StreamAlbum>,
    artists: TtlCache<StreamArtist>,
    playlists: TtlCache<StreamPlaylist>,
}

/// The runtime every future of the service runs on.
struct Runtime(Option<tokio::runtime::Runtime>);

impl Runtime {
    fn handle(&self) -> &tokio::runtime::Handle {
        self.0
            .as_ref()
            .expect("runtime lives as long as the service")
            .handle()
    }
}

impl Drop for Runtime {
    /// The last `Arc<Deezer>` may go away inside some async context, where a
    /// blocking shutdown would panic.
    fn drop(&mut self) {
        if let Some(rt) = self.0.take() {
            rt.shutdown_background();
        }
    }
}

/// The Deezer catalogue; see the crate docs.
pub struct Deezer {
    inner: Arc<Inner>,
    runtime: Runtime,
}

/// A spawned task, aborted when dropped.
struct Joined<T>(JoinHandle<Result<T, StreamError>>);

impl<T> Future for Joined<T> {
    type Output = Result<T, StreamError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0)
            .poll(cx)
            .map(|r| r.unwrap_or_else(|e| Err(StreamError::Network(format!("task failed: {e}")))))
    }
}

impl<T> Drop for Joined<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Deezer {
    /// Builds the client and the service's runtime; no network.
    pub fn new(config: DeezerConfig) -> Result<Arc<Self>, StreamError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("deezer")
            .enable_all()
            .build()
            .map_err(|e| StreamError::Network(format!("cannot start the runtime: {e}")))?;
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| StreamError::Network(e.to_string()))?;
        Ok(Arc::new(Self {
            inner: Arc::new(Inner {
                http,
                base: config.base_url.trim_end_matches('/').to_owned(),
                gate: limit::Gate::new(),
                searches: TtlCache::new(SEARCH_TTL, 64),
                tracks: TtlCache::new(ITEM_TTL, 2_000),
                albums: TtlCache::new(ITEM_TTL, 200),
                artists: TtlCache::new(ITEM_TTL, 200),
                playlists: TtlCache::new(PLAYLIST_TTL, 50),
            }),
            runtime: Runtime(Some(runtime)),
        }))
    }

    /// Runs `f` on the service's runtime; the returned future cancels it
    /// when dropped.
    fn run<T, F>(&self, f: impl FnOnce(Arc<Inner>) -> F) -> BoxFuture<'_, Result<T, StreamError>>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, StreamError>> + Send + 'static,
    {
        let task = self.runtime.handle().spawn(f(Arc::clone(&self.inner)));
        Box::pin(Joined(task))
    }
}

/// The search key of the cache: the kind, then the query folded to lower
/// case and single spaces.
fn search_key(kind: &str, q: &str) -> String {
    format!("{kind}:{}", q.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase())
}

impl Inner {
    /// One GET of `path` (with `query`), through the gate. Deezer's error
    /// body becomes a [`StreamError`]; its quota error pauses the service.
    async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> Result<T, StreamError> {
        let started = std::time::Instant::now();
        let _turn = self.gate.turn().await?;
        let url = format!("{}{path}", self.base);
        let resp = self
            .http
            .get(&url)
            .query(query)
            .send()
            .await
            .map_err(|e| StreamError::Network(e.to_string()))?;
        let status = resp.status().as_u16();
        if status == 429 {
            let pause = self.gate.quota_hit();
            return Err(StreamError::RateLimited { retry_after: Some(pause) });
        }
        if !(200..300).contains(&status) {
            log::info!("deezer: {path} -> HTTP {status}");
            return Err(StreamError::Http { status });
        }
        let bytes = resp.bytes().await.map_err(|e| StreamError::Network(e.to_string()))?;
        let env: api::Envelope = serde_json::from_slice(&bytes).unwrap_or_default();
        if let Some(e) = env.error {
            log::info!("deezer: {path} -> error {} ({}): {}", e.code, e.kind, e.message);
            return Err(match e.code {
                api::code::QUOTA | api::code::BUSY => StreamError::RateLimited {
                    retry_after: Some(self.gate.quota_hit()),
                },
                api::code::NO_DATA => StreamError::Unavailable,
                _ if e.kind == "DataException" => StreamError::Unavailable,
                _ => StreamError::Parse(format!("{} {}", e.code, e.message)),
            });
        }
        let out = serde_json::from_slice(&bytes).map_err(|e| StreamError::Parse(format!("{path}: {e}")));
        log::debug!("deezer: {path} -> ok in {} ms", started.elapsed().as_millis());
        out
    }

    async fn search_tracks(&self, q: &str) -> Result<Vec<StreamTrack>, StreamError> {
        let l: api::List<api::Track> = self.get("/search", &[("q", q.to_owned()), ("limit", SEARCH_LIMIT.to_string())]).await?;
        Ok(l.data.iter().map(|t| api::track(t, None)).collect())
    }

    async fn search_albums(&self, q: &str) -> Result<Vec<StreamAlbum>, StreamError> {
        let l: api::List<api::Album> =
            self.get("/search/album", &[("q", q.to_owned()), ("limit", SEARCH_LIMIT.to_string())]).await?;
        Ok(l.data.iter().map(api::album).collect())
    }

    async fn search_artists(&self, q: &str) -> Result<Vec<StreamArtist>, StreamError> {
        let l: api::List<api::Artist> =
            self.get("/search/artist", &[("q", q.to_owned()), ("limit", SEARCH_LIMIT.to_string())]).await?;
        Ok(l.data.iter().map(api::artist).collect())
    }

    async fn search_playlists(&self, q: &str) -> Result<Vec<StreamPlaylist>, StreamError> {
        let l: api::List<api::Playlist> =
            self.get("/search/playlist", &[("q", q.to_owned()), ("limit", SEARCH_LIMIT.to_string())]).await?;
        Ok(l.data.iter().map(api::playlist).collect())
    }

    /// Tracks, albums, artists (and playlists when `playlists`) at once;
    /// the tracks go to `part` first. A failed category keeps the first
    /// error; the answer fails only when every category did.
    async fn search(&self, q: &str, playlists: bool, part: SearchParts) -> Result<StreamSearch, StreamError> {
        let q = q.trim();
        if q.is_empty() {
            return Ok(StreamSearch::default());
        }
        let key = search_key(if playlists { "all" } else { "top" }, q);
        if let Some(hit) = self.searches.get(&key) {
            return Ok(hit);
        }
        self.gate.check()?;
        let started = std::time::Instant::now();
        let tracks = async {
            let r = self.search_tracks(q).await;
            if let Ok(t) = &r {
                part(StreamSearch {
                    tracks: t.clone(),
                    ..StreamSearch::default()
                });
            }
            r
        };
        let pl = async {
            if playlists {
                self.search_playlists(q).await
            } else {
                Ok(Vec::new())
            }
        };
        let (tracks, albums, artists, pl) =
            tokio::join!(tracks, self.search_albums(q), self.search_artists(q), pl);
        let mut first_err = None;
        let tracks = part_of(tracks, &mut first_err);
        let albums = part_of(albums, &mut first_err);
        let artists = part_of(artists, &mut first_err);
        let pl = part_of(pl, &mut first_err);
        if tracks.is_none() && albums.is_none() && artists.is_none() {
            return Err(first_err.unwrap_or(StreamError::Unavailable));
        }
        let out = StreamSearch {
            tracks: tracks.unwrap_or_default(),
            albums: albums.unwrap_or_default(),
            artists: artists.unwrap_or_default(),
            playlists: pl.unwrap_or_default(),
        };
        log::debug!(
            "deezer: search done in {} ms: {} tracks, {} albums, {} artists, {} playlists",
            started.elapsed().as_millis(),
            out.tracks.len(),
            out.albums.len(),
            out.artists.len(),
            out.playlists.len()
        );
        if first_err.is_none() {
            self.searches.put(&key, out.clone());
        }
        Ok(out)
    }

    async fn track(&self, id: &str) -> Result<StreamTrack, StreamError> {
        if let Some(t) = self.tracks.get(id) {
            return Ok(t);
        }
        let t: api::Track = self.get(&format!("/track/{}", path_id(id)?), &[]).await?;
        let t = api::track(&t, None);
        self.tracks.put(id, t.clone());
        Ok(t)
    }

    async fn album(&self, id: &str) -> Result<StreamAlbum, StreamError> {
        if let Some(a) = self.albums.get(id) {
            return Ok(a);
        }
        let id = path_id(id)?;
        let mut raw: api::Album = self.get(&format!("/album/{id}"), &[]).await?;
        // Long albums list their tracks in pages.
        let listed = raw.tracks.as_ref().map_or(0, |l| l.data.len());
        if raw.nb_tracks.is_some_and(|n| n as usize > listed) {
            let more: api::List<api::Track> = self
                .get(&format!("/album/{id}/tracks"), &[("limit", "500".into())])
                .await?;
            if more.data.len() > listed {
                raw.tracks = Some(more);
            }
        }
        let a = api::album(&raw);
        self.albums.put(id, a.clone());
        Ok(a)
    }

    async fn artist(&self, id: &str) -> Result<StreamArtist, StreamError> {
        if let Some(a) = self.artists.get(id) {
            return Ok(a);
        }
        let id = path_id(id)?;
        let raw: api::Artist = self.get(&format!("/artist/{id}"), &[]).await?;
        let albums: api::List<api::Album> = self
            .get(&format!("/artist/{id}/albums"), &[("limit", ARTIST_ALBUMS.to_string())])
            .await?;
        let mut a = api::artist(&raw);
        let me = nikgob_core::streaming::ArtistRef::new(Some(a.id.clone()), a.name.clone());
        a.albums = albums
            .data
            .iter()
            .map(|al| {
                let mut al = api::album(al);
                if al.artists.is_empty() {
                    al.artists = vec![me.clone()];
                }
                al
            })
            .collect();
        // Newest first, as the contract says.
        a.albums.sort_by_key(|al| std::cmp::Reverse(al.year.unwrap_or(i32::MIN)));
        self.artists.put(id, a.clone());
        Ok(a)
    }

    async fn playlist(&self, id: &str) -> Result<StreamPlaylist, StreamError> {
        if let Some(p) = self.playlists.get(id) {
            return Ok(p);
        }
        let id = path_id(id)?;
        let raw: api::Playlist = self.get(&format!("/playlist/{id}"), &[]).await?;
        let mut p = api::playlist(&raw);
        let total = raw.nb_tracks.unwrap_or(0) as usize;
        while p.tracks.len() < total.min(PLAYLIST_MAX) {
            let page: api::List<api::Track> = self
                .get(
                    &format!("/playlist/{id}/tracks"),
                    &[("index", p.tracks.len().to_string()), ("limit", PAGE.to_string())],
                )
                .await?;
            if page.data.is_empty() {
                break;
            }
            p.tracks.extend(page.data.iter().map(|t| api::track(t, None)));
        }
        self.playlists.put(id, p.clone());
        Ok(p)
    }

    async fn find_isrc(&self, isrc: &str) -> Result<Option<StreamTrack>, StreamError> {
        let isrc: String = isrc
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_uppercase())
            .collect();
        if isrc.len() != 12 {
            return Ok(None);
        }
        match self.get::<api::Track>(&format!("/track/isrc:{isrc}"), &[]).await {
            Ok(t) if t.id > 0 => {
                let t = api::track(&t, None);
                self.tracks.put(&t.id.id, t.clone());
                Ok(Some(t))
            }
            Ok(_) | Err(StreamError::Unavailable) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// One category of a search; a failed one keeps the first error.
fn part_of<T>(r: Result<T, StreamError>, first_err: &mut Option<StreamError>) -> Option<T> {
    r.map_err(|e| {
        first_err.get_or_insert(e);
    })
    .ok()
}

/// A Deezer id is a number; anything else is no such item.
fn path_id(id: &str) -> Result<&str, StreamError> {
    let id = id.trim();
    if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) {
        Ok(id)
    } else {
        Err(StreamError::Unavailable)
    }
}

fn unsupported<'a, T: Send + 'a>() -> BoxFuture<'a, Result<T, StreamError>> {
    Box::pin(std::future::ready(Err(StreamError::Unsupported)))
}

impl StreamService for Deezer {
    fn service(&self) -> Service {
        Service::Deezer
    }

    fn catalogue_only(&self) -> bool {
        true
    }

    /// No account: always [`Account::LoggedOut`], usable all the same.
    fn account(&self) -> Account {
        Account::LoggedOut
    }

    fn login(&self, _input: LoginInput) -> BoxFuture<'_, Result<(), StreamError>> {
        unsupported()
    }

    fn logout(&self) {}

    fn search(&self, query: &str) -> BoxFuture<'_, Result<StreamSearch, StreamError>> {
        self.search_in_parts(query, Arc::new(|_| {}))
    }

    /// Tracks, albums, artists and playlists: four requests at once.
    fn search_in_parts(&self, query: &str, part: SearchParts) -> BoxFuture<'_, Result<StreamSearch, StreamError>> {
        let q = query.to_owned();
        self.run(move |inner| async move { inner.search(&q, true, part).await })
    }

    /// Tracks, albums and artists: three requests at once (playlists are
    /// for the chip).
    fn search_top(&self, query: &str, part: SearchParts) -> BoxFuture<'_, Result<StreamSearch, StreamError>> {
        let q = query.to_owned();
        self.run(move |inner| async move { inner.search(&q, false, part).await })
    }

    fn search_kind(&self, query: &str, kind: SearchKind) -> BoxFuture<'_, Result<StreamSearch, StreamError>> {
        let q = query.trim().to_owned();
        self.run(move |inner| async move {
            let mut out = StreamSearch::default();
            if q.is_empty() {
                return Ok(out);
            }
            let key = search_key(&format!("{kind:?}"), &q);
            if let Some(hit) = inner.searches.get(&key) {
                return Ok(hit);
            }
            match kind {
                SearchKind::Songs => out.tracks = inner.search_tracks(&q).await?,
                // Deezer has no music videos.
                SearchKind::Videos => return Ok(out),
                SearchKind::Albums => out.albums = inner.search_albums(&q).await?,
                SearchKind::Artists => out.artists = inner.search_artists(&q).await?,
            }
            inner.searches.put(&key, out.clone());
            Ok(out)
        })
    }

    fn track(&self, id: &str) -> BoxFuture<'_, Result<StreamTrack, StreamError>> {
        let id = id.to_owned();
        self.run(move |inner| async move { inner.track(&id).await })
    }

    /// One request per track not in the cache (Deezer has no batch call),
    /// through the gate.
    fn tracks(&self, ids: &[String]) -> BoxFuture<'_, Result<Vec<Option<StreamTrack>>, StreamError>> {
        let ids = ids.to_vec();
        self.run(move |inner| async move {
            let mut out = Vec::with_capacity(ids.len());
            for id in &ids {
                match inner.track(id).await {
                    Ok(t) => out.push(Some(t)),
                    Err(StreamError::Unavailable) => out.push(None),
                    Err(e) => return Err(e),
                }
            }
            Ok(out)
        })
    }

    fn album(&self, id: &str) -> BoxFuture<'_, Result<StreamAlbum, StreamError>> {
        let id = id.to_owned();
        self.run(move |inner| async move { inner.album(&id).await })
    }

    fn artist(&self, id: &str) -> BoxFuture<'_, Result<StreamArtist, StreamError>> {
        let id = id.to_owned();
        self.run(move |inner| async move { inner.artist(&id).await })
    }

    fn playlist(&self, id: &str) -> BoxFuture<'_, Result<StreamPlaylist, StreamError>> {
        let id = id.to_owned();
        self.run(move |inner| async move { inner.playlist(&id).await })
    }

    fn forget(&self, kind: EntityKind, id: &str) {
        match kind {
            EntityKind::Track => self.inner.tracks.remove(id),
            EntityKind::Album => self.inner.albums.remove(id),
            EntityKind::Artist => self.inner.artists.remove(id),
            EntityKind::Playlist => self.inner.playlists.remove(id),
        }
    }

    fn paused_until(&self) -> Option<std::time::SystemTime> {
        self.inner.gate.paused_until()
    }

    fn find_isrc(&self, isrc: &str) -> BoxFuture<'_, Result<Option<StreamTrack>, StreamError>> {
        let isrc = isrc.to_owned();
        self.run(move |inner| async move { inner.find_isrc(&isrc).await })
    }

    fn liked(&self) -> BoxFuture<'_, Result<Vec<StreamTrack>, StreamError>> {
        unsupported()
    }

    fn set_liked(&self, _id: &str, _liked: bool) -> BoxFuture<'_, Result<(), StreamError>> {
        unsupported()
    }

    fn my_playlists(&self) -> BoxFuture<'_, Result<Vec<StreamPlaylist>, StreamError>> {
        unsupported()
    }

    fn create_playlist(&self, _name: &str) -> BoxFuture<'_, Result<String, StreamError>> {
        unsupported()
    }

    fn playlist_edit(&self, _id: &str, _add: &[String], _remove: &[String]) -> BoxFuture<'_, Result<Option<String>, StreamError>> {
        unsupported()
    }

    fn recent(&self) -> BoxFuture<'_, Result<Vec<Played>, StreamError>> {
        unsupported()
    }

    /// No sound here: the runtime plays the track from YouTube Music.
    fn open_audio(&self, _id: &str) -> Result<Box<dyn AudioStream>, StreamError> {
        Err(StreamError::Unsupported)
    }

    fn save_audio(&self, _id: &str, _dir: &Path) -> BoxFuture<'_, Result<PathBuf, StreamError>> {
        unsupported()
    }

    fn web_url(&self, kind: EntityKind, id: &str) -> String {
        let kind = match kind {
            EntityKind::Track => "track",
            EntityKind::Album => "album",
            EntityKind::Artist => "artist",
            EntityKind::Playlist => "playlist",
        };
        format!("https://www.deezer.com/{kind}/{id}")
    }
}
