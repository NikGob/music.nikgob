//! The Soulseek source (docs/stage4.5.md, poll answer 3), run from the
//! `[soulseek]` part of settings.toml: either the embedded client, on a
//! thread of its own at the lowest priority with a one-thread Tokio
//! runtime, sharing the library folder read-only; or a remote `nikgob-slsk`
//! the player only talks to. It starts when the settings turn it on, and
//! stops, or restarts with new credentials, when they change; share
//! settings apply live.
//!
//! Searching and downloading go through [`SoulseekService::source`] on its
//! runtime: [`SlskSource`] is the download source «Хочу скачать» asks like
//! the others. It runs when `[soulseek]` has an account and its switch on
//! «Источники» is on.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use nikgob_core::indexer::BoxFuture;
use nikgob_core::model::Codec;
use nikgob_core::settings::{SoulseekMode, SoulseekSettings};
use nikgob_core::source::{Fetched, Format, Offer, OfferScope, Progress, Source, SourceError, SourceKey, Want, WantScope};
use nikgob_metadata::text::title_key;
use nikgob_soulseek::{
    AlbumResult, Client, Config, DownloadState, QueryKind, RemoteClient, SearchQuery, SearchResult, ShareSettings, Soulseek,
};
use nikgob_sources::matching::contains_words;
use nikgob_sources::text::{album_core, catalogue_queries, main_artist, title_keys};

use crate::Worker;

struct Running {
    rt: tokio::runtime::Handle,
    source: Soulseek,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

pub(crate) struct SoulseekService {
    running: Option<Running>,
    applied: Option<SoulseekSettings>,
    library_dir: PathBuf,
    /// The share index cache; None = not kept.
    state_dir: Option<PathBuf>,
    /// Counts the clients started (a new one counts its uploads from 0).
    generation: u64,
}

fn share_of(s: &SoulseekSettings) -> ShareSettings {
    ShareSettings { enabled: s.share, upload_limit_kbps: s.upload_limit_kbps, upload_slots: s.upload_slots.max(1) }
}

/// Settings that need a new client (the rest apply live).
fn same_session(a: &SoulseekSettings, b: &SoulseekSettings) -> bool {
    a.enabled == b.enabled
        && a.mode == b.mode
        && a.username == b.username
        && a.password == b.password
        && a.server == b.server
        && a.listen_port == b.listen_port
        && a.remote_url == b.remote_url
        && a.remote_token == b.remote_token
}

impl SoulseekService {
    pub(crate) fn new(library_dir: PathBuf, state_dir: Option<PathBuf>) -> Self {
        Self { running: None, applied: None, library_dir, state_dir, generation: 0 }
    }

    /// Which client runs: changes whenever one is started.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// The running source, if any.
    pub(crate) fn source(&self) -> Option<Soulseek> {
        self.running.as_ref().map(|r| r.source.clone())
    }

    /// The download source over the running client, if any.
    pub(crate) fn download_source(&self) -> Option<SlskSource> {
        let rt = self.running.as_ref()?.rt.clone();
        Some(SlskSource { slsk: self.source()?, rt })
    }

    /// Runs `f` on the source's runtime.
    pub(crate) fn spawn<F>(&self, f: F) -> bool
    where
        F: Future<Output = ()> + Send + 'static,
    {
        match &self.running {
            Some(r) => {
                r.rt.spawn(f);
                true
            }
            None => false,
        }
    }

    /// Starts, restarts, changes or stops the source to match `s`.
    pub(crate) fn apply(&mut self, s: &SoulseekSettings) {
        if self.applied.as_ref() == Some(s) {
            return;
        }
        if let (Some(old), Some(r)) = (&self.applied, &self.running)
            && same_session(old, s)
        {
            if let Soulseek::Embedded(c) = &r.source {
                c.set_share(share_of(s));
            }
            self.applied = Some(s.clone());
            return;
        }
        self.shutdown();
        self.applied = Some(s.clone());
        if !s.enabled {
            return;
        }
        match self.start(s) {
            Ok(r) => {
                self.generation += 1;
                self.running = Some(r);
            }
            Err(e) => log::warn!("soulseek: not started: {e}"),
        }
    }

    fn start(&self, s: &SoulseekSettings) -> Result<Running, String> {
        let remote = match s.mode {
            SoulseekMode::Remote => Some(RemoteClient::new(&s.remote_url, &s.remote_token).map_err(|e| e.to_string())?),
            SoulseekMode::Embedded => None,
        };
        let cfg = Config {
            server: s.server.clone(),
            username: s.username.clone(),
            password: s.password.clone(),
            listen_port: s.listen_port,
            share_folders: shared_folders(&self.library_dir),
            share: share_of(s),
            state_dir: self.state_dir.clone(),
            ..Config::default()
        };
        let (tx, rx) = std::sync::mpsc::channel::<Result<(tokio::runtime::Handle, Soulseek), String>>();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("nikgob-slsk".into())
            .spawn(move || {
                let _ = thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Min);
                let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = tx.send(Err(e.to_string()));
                        return;
                    }
                };
                rt.block_on(async move {
                    let source = match remote {
                        Some(r) => Soulseek::Remote(r),
                        None => match Client::start(cfg).await {
                            Ok(c) => Soulseek::Embedded(c),
                            Err(e) => {
                                let _ = tx.send(Err(e.to_string()));
                                return;
                            }
                        },
                    };
                    let _ = tx.send(Ok((tokio::runtime::Handle::current(), source.clone())));
                    let _ = stop_rx.await;
                    if let Soulseek::Embedded(c) = source {
                        c.shutdown();
                        // Lets the tasks see the stop and close their sockets.
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                });
            })
            .map_err(|e| e.to_string())?;
        let (rt, source) = match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                let _ = thread.join();
                return Err(e);
            }
            Err(_) => return Err("the Soulseek thread did not start".into()),
        };
        log::info!("soulseek: started ({})", if matches!(source, Soulseek::Remote(_)) { "remote" } else { "embedded" });
        Ok(Running { rt, source, stop: Some(stop_tx), thread: Some(thread) })
    }

    pub(crate) fn shutdown(&mut self) {
        if let Some(mut r) = self.running.take() {
            if let Some(s) = r.stop.take() {
                let _ = s.send(());
            }
            if let Some(t) = r.thread.take() {
                let _ = t.join();
            }
        }
    }
}

impl Drop for SoulseekService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Worker {
    /// Starts or stops Soulseek to match settings.toml and its switch on
    /// «Источники» (at start-up, whenever its settings are saved and when
    /// the switch moves), and puts its download source in or out.
    pub(crate) fn soulseek_apply(&mut self) {
        let mut s = self
            .settings_path
            .as_deref()
            .and_then(|p| nikgob_core::settings::Settings::load(p).ok())
            .map(|s| s.soulseek)
            .unwrap_or_default();
        let saved = s.enabled;
        s.enabled = saved && self.state.read().prefs.source_on(SourceKey::Soulseek);
        self.soulseek.apply(&s);
        self.sources.retain(|x| x.key() != SourceKey::Soulseek);
        if let Some(src) = self.soulseek.download_source() {
            self.sources.push(Arc::new(src));
        }
        let view = nikgob_view::SoulseekView {
            remote: s.mode == SoulseekMode::Remote,
            username: s.username.clone(),
            password_saved: !s.password.is_empty(),
            remote_url: s.remote_url.clone(),
            token_saved: !s.remote_token.is_empty(),
            share: s.share,
            upload_limit_kbps: s.upload_limit_kbps,
            running: self.soulseek.source().is_some(),
        };
        self.share_settings(&s, view.configured());
        self.publish(|st| st.downloads.soulseek = view);
        self.publish_sources();
        self.wish_sources_ready();
        self.wish_tick();
    }

    /// Changes `[soulseek]` with `f` and applies it; false when
    /// settings.toml could not be read or written (the user was told).
    fn soulseek_edit(&mut self, f: impl FnOnce(&mut SoulseekSettings)) -> bool {
        let Some(path) = self.settings_path.clone() else {
            self.toast("Некуда сохранить настройки", None);
            return false;
        };
        // A file that does not parse is not replaced by defaults: it holds
        // the other secrets.
        let mut settings = match nikgob_core::settings::Settings::load(&path) {
            Ok(s) => s,
            Err(e) => {
                log::error!("loading settings: {e}");
                self.toast("settings.toml не читается — поправь файл вручную", None);
                return false;
            }
        };
        f(&mut settings.soulseek);
        if let Err(e) = settings.save(&path) {
            log::error!("saving settings: {e}");
            self.toast("Не удалось сохранить настройки", None);
            return false;
        }
        self.soulseek_apply();
        true
    }

    /// «Сохранить» under Soulseek: the account, or the service's address.
    pub(crate) fn set_soulseek(&mut self, form: nikgob_view::SoulseekForm) {
        let remote = form.remote;
        // The account (again): once it is in, a new search everywhere.
        self.streaming.wish.enabled.insert(SourceKey::Soulseek);
        let saved = self.soulseek_edit(|s| {
            s.mode = if form.remote { SoulseekMode::Remote } else { SoulseekMode::Embedded };
            s.username = form.username.trim().to_owned();
            if !form.password.is_empty() {
                s.password = form.password;
            }
            s.remote_url = form.remote_url.trim().to_owned();
            if !form.remote_token.trim().is_empty() {
                s.remote_token = form.remote_token.trim().to_owned();
            }
            s.enabled = match s.mode {
                SoulseekMode::Remote => !s.remote_url.is_empty(),
                SoulseekMode::Embedded => !s.username.is_empty() && !s.password.is_empty(),
            };
        });
        if !saved {
            return;
        }
        let on = self.state.read().prefs.source_on(SourceKey::Soulseek);
        let text = match (self.soulseek.source().is_some(), on) {
            (true, _) => "Soulseek подключён",
            (false, false) => "Soulseek сохранён — включи его в «Источниках скачивания»",
            (false, true) if remote => "Soulseek сохранён, но утилита не отвечает",
            (false, true) => "Soulseek сохранён, но не подключился",
        };
        self.toast(text, None);
    }

    /// Soulseek uploads at once (1 to 10).
    pub(crate) fn set_soulseek_slots(&mut self, n: u32) {
        let n = n.clamp(1, 10);
        self.soulseek_edit(|s| s.upload_slots = n);
    }

    /// Sharing and its limit, at once.
    pub(crate) fn set_soulseek_share(&mut self, share: bool, upload_limit_kbps: u32) {
        self.soulseek_edit(|s| {
            s.share = share;
            s.upload_limit_kbps = upload_limit_kbps;
        });
    }
}

/// How long a search listens for the peers' answers.
const SEARCH_WAIT: Duration = Duration::from_secs(8);
/// How long a search waits for the client to log in.
const LOGIN_WAIT: Duration = Duration::from_secs(30);
/// Lengths further apart (s) are another recording.
const OTHER_SECS: u32 = 10;
/// The files a folder of a sharer is taken for.
const AUDIO: &[&str] = &["FLAC", "MP3", "OGG", "OPUS", "M4A", "AAC", "WAV", "ALAC", "APE", "WV", "AIFF"];

/// «Хочу скачать» from Soulseek: the peers' files and folders as offers,
/// judged by the one policy like a torrent's name (the sharer's folders and
/// the file's name), downloaded into the item's folder.
pub(crate) struct SlskSource {
    slsk: Soulseek,
    rt: tokio::runtime::Handle,
}

#[derive(serde::Serialize, serde::Deserialize)]
enum SlskHandle {
    File(SearchResult),
    Folder { user: String, folder: String, files: Vec<SearchResult> },
}

/// The audio files of a peer's folder offer as its search listed them:
/// each file's name and length (ms). `None`: not a Soulseek folder.
pub(crate) fn folder_listing(offer: &Offer) -> Option<Vec<(String, Option<u32>)>> {
    if offer.source != SourceKey::Soulseek {
        return None;
    }
    match serde_json::from_str::<SlskHandle>(&offer.handle).ok()? {
        SlskHandle::Folder { files, .. } => Some(
            files
                .iter()
                .filter(|f| is_audio(f))
                .map(|f| (f.file_name().to_owned(), f.duration.map(|d| d * 1000)))
                .collect(),
        ),
        SlskHandle::File(_) => None,
    }
}

fn source_error(e: nikgob_soulseek::Error) -> SourceError {
    use nikgob_soulseek::Error as E;
    match e {
        E::Login(s) | E::Denied(s) => SourceError::Blocked(s),
        other => SourceError::Unreachable(other.to_string()),
    }
}

fn codec_of(format: &str) -> Codec {
    match format {
        "FLAC" => Codec::Flac,
        "MP3" => Codec::Mp3,
        "OGG" => Codec::Vorbis,
        "OPUS" => Codec::Opus,
        "M4A" | "AAC" => Codec::Aac,
        "WAV" | "AIFF" => Codec::Wav,
        "ALAC" => Codec::Alac,
        other => Codec::Other(other.to_lowercase()),
    }
}

fn format_of(r: &SearchResult) -> Format {
    Format {
        bitrate_kbps: r.bitrate.filter(|_| !r.lossless),
        sample_rate: r.sample_rate,
        bit_depth: r.bit_depth.and_then(|b| u8::try_from(b).ok()),
        ..Format::new(codec_of(&r.format))
    }
}

/// The last `n` parts of a sharer's path ("Artist / Album / 03 - Title").
fn tail(path: &str, n: usize) -> String {
    let parts: Vec<&str> = path.split('\\').filter(|p| !p.is_empty()).collect();
    parts[parts.len().saturating_sub(n)..].join(" / ")
}

fn stem(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(s, _)| s)
}

fn is_audio(r: &SearchResult) -> bool {
    AUDIO.contains(&r.format.as_str())
}

/// The peers' files of the wished track: the title in the file's name, and
/// the length within [`OTHER_SECS`] when both say it.
pub(crate) fn track_offers(results: &[SearchResult], want: &Want) -> Vec<Offer> {
    let titles: Vec<String> = want.title.as_deref().map(title_keys).unwrap_or_default();
    results
        .iter()
        .filter(|r| is_audio(r))
        .filter(|r| {
            let name = title_key(stem(r.file_name())).base;
            titles.iter().any(|t| contains_words(&name, t))
        })
        .filter(|r| match (r.duration, want.duration_ms) {
            (Some(a), Some(b)) => a.abs_diff(b / 1000) <= OTHER_SECS,
            _ => true,
        })
        .map(|r| Offer {
            source: SourceKey::Soulseek,
            provider: r.user.clone(),
            id: format!("{}\\{}", r.user, r.path),
            title: tail(stem(&r.path), 3),
            artist: None,
            track_title: None,
            album: None,
            duration_ms: r.duration.map(|d| d * 1000),
            isrc: None,
            scope: OfferScope::Track,
            format: Some(format_of(r)),
            size_bytes: Some(r.size),
            // A peer with a free slot sends at once, one with a queue later.
            seeders: Some(u32::from(r.free_slot)),
            info_url: None,
            handle: serde_json::to_string(&SlskHandle::File(r.clone())).unwrap_or_default(),
        })
        .collect()
}

/// The peers' folders of the wished album, two audio files or more each.
pub(crate) fn album_offers(albums: &[AlbumResult]) -> Vec<Offer> {
    albums
        .iter()
        .filter_map(|a| {
            let files: Vec<SearchResult> = a.files.iter().filter(|f| is_audio(f)).cloned().collect();
            if files.len() < 2 {
                return None;
            }
            // Lossless when most of its files are.
            let lossless = files.iter().filter(|f| f.lossless).count() * 2 > files.len();
            let first = files.iter().find(|f| f.lossless == lossless).unwrap_or(&files[0]);
            Some(Offer {
                source: SourceKey::Soulseek,
                provider: a.user.clone(),
                id: format!("{}\\{}", a.user, a.folder),
                title: tail(&a.folder, 2),
                artist: None,
                track_title: None,
                album: None,
                duration_ms: None,
                isrc: None,
                scope: OfferScope::Album,
                format: Some(format_of(first)),
                size_bytes: Some(files.iter().map(|f| f.size).sum()),
                seeders: Some(u32::from(a.free_slot)),
                info_url: None,
                handle: serde_json::to_string(&SlskHandle::Folder { user: a.user.clone(), folder: a.folder.clone(), files })
                    .unwrap_or_default(),
            })
        })
        .collect()
}

impl SlskSource {
    /// Runs `f` on the client's own runtime and waits for it here.
    async fn on<T: Send + 'static>(&self, f: impl Future<Output = T> + Send + 'static) -> Result<T, SourceError> {
        self.rt.spawn(f).await.map_err(|e| SourceError::Unreachable(e.to_string()))
    }

    async fn search_text(&self, text: String, kind: QueryKind) -> Result<Vec<SearchResult>, SourceError> {
        let slsk = self.slsk.clone();
        self.on(async move {
            // Just started (the switch, the account saved): a search sent
            // before the login would come back empty, as "not found".
            if let Soulseek::Embedded(c) = &slsk {
                c.wait_logged_in(LOGIN_WAIT).await?;
            }
            slsk.search(&SearchQuery { text, kind }, SEARCH_WAIT).await
        })
            .await?
            .map_err(source_error)
    }

    /// One file into `dest`, its bytes told to `progress` (`before`: what
    /// the files before it brought, of `total`).
    async fn get(&self, file: SearchResult, dest: PathBuf, progress: Progress, before: u64, total: u64) -> Result<PathBuf, SourceError> {
        let slsk = self.slsk.clone();
        self.on(async move {
            let d = slsk.download(&file, &dest).await?;
            let mut rx = d.subscribe();
            loop {
                let state = rx.borrow_and_update().clone();
                match state {
                    DownloadState::Done { path } => return Ok(path),
                    DownloadState::Failed { reason } => return Err(nikgob_soulseek::Error::Denied(reason)),
                    DownloadState::Transferring { received, .. } => progress(before + received, Some(total)),
                    DownloadState::Queued { .. } | DownloadState::Connecting => {}
                }
                if rx.changed().await.is_err() {
                    return Err(nikgob_soulseek::Error::Cancelled);
                }
            }
        })
        .await?
        .map_err(source_error)
    }
}

impl Source for SlskSource {
    fn key(&self) -> SourceKey {
        SourceKey::Soulseek
    }

    fn name(&self) -> &str {
        "Soulseek"
    }

    fn search<'a>(&'a self, want: &'a Want) -> BoxFuture<'a, Result<Vec<Offer>, SourceError>> {
        Box::pin(async move {
            match want.scope {
                WantScope::Track => {
                    let mut offers: Vec<Offer> = Vec::new();
                    for q in catalogue_queries(want).into_iter().take(2) {
                        let found = self.search_text(q, QueryKind::Track).await?;
                        for o in track_offers(&found, want) {
                            if !offers.iter().any(|x| x.id == o.id) {
                                offers.push(o);
                            }
                        }
                        if offers.iter().any(Offer::lossless) {
                            break;
                        }
                    }
                    Ok(offers)
                }
                WantScope::Album => {
                    let Some(album) = want.album.as_deref().or(want.title.as_deref()) else { return Ok(Vec::new()) };
                    let text = format!("{} {}", main_artist(want.artist()), album_core(album)).trim().to_owned();
                    let found = self.search_text(text, QueryKind::Album).await?;
                    Ok(album_offers(&nikgob_soulseek::group_albums(&found)))
                }
            }
        })
    }

    fn fetch<'a>(&'a self, offer: &'a Offer, _: &'a Want, dest: &'a Path, progress: Progress) -> BoxFuture<'a, Result<Fetched, SourceError>> {
        Box::pin(async move {
            let handle: SlskHandle = serde_json::from_str(&offer.handle).map_err(|e| SourceError::Parse(e.to_string()))?;
            let files = match handle {
                SlskHandle::File(f) => vec![f],
                SlskHandle::Folder { user, folder, files } => {
                    // The whole folder when the sharer lists it: a search
                    // brings only the files whose path has every word.
                    let slsk = self.slsk.clone();
                    let listed = self.on(async move { slsk.folder(&user, &folder).await }).await;
                    let all: Vec<SearchResult> = match listed {
                        Ok(Ok(all)) => all.into_iter().filter(is_audio).collect(),
                        _ => Vec::new(),
                    };
                    if all.len() >= files.len() { all } else { files }
                }
            };
            if files.is_empty() {
                return Err(SourceError::NothingToFetch);
            }
            let total: u64 = files.iter().map(|f| f.size).sum();
            let mut before = 0;
            let mut out = Vec::new();
            for f in files {
                let size = f.size;
                out.push(self.get(f, dest.to_path_buf(), progress.clone(), before, total).await?);
                before += size;
            }
            Ok(Fetched::Files(out))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_soulseek::codec::{read_frame, split_u32, write_all};
    use nikgob_soulseek::msg::{FromServer, ToServer};

    /// A server that logs everyone in and keeps what they send.
    async fn server() -> (std::net::SocketAddr, std::sync::Arc<parking_lot::Mutex<Vec<ToServer>>>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let got = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        let seen = got.clone();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                let seen = seen.clone();
                tokio::spawn(async move {
                    let _ = read_frame(&mut s, 1 << 16).await;
                    let ok = FromServer::LoginOk { greet: String::new(), ip: "127.0.0.1".parse().unwrap(), supporter: false };
                    write_all(&mut s, &ok.encode()).await.unwrap();
                    while let Ok(f) = read_frame(&mut s, 1 << 20).await {
                        let (code, body) = split_u32(&f).unwrap();
                        if let Ok(m) = ToServer::decode(code, body) {
                            seen.lock().push(m);
                        }
                    }
                });
            }
        });
        (addr, got)
    }

    async fn until(mut f: impl FnMut() -> bool) -> bool {
        for _ in 0..250 {
            if f() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn starts_applies_share_live_and_stops() {
        let (addr, got) = server().await;
        let lib = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(lib.path().join("Music").join("A")).unwrap();
        std::fs::write(lib.path().join("Music").join("A").join("x.flac"), b"x").unwrap();
        // The trash is not shared.
        std::fs::create_dir_all(lib.path().join("Trash")).unwrap();
        std::fs::write(lib.path().join("Trash").join("y.flac"), b"y").unwrap();
        let state = tempfile::tempdir().unwrap();
        let mut svc = SoulseekService::new(lib.path().to_path_buf(), Some(state.path().to_path_buf()));
        let mut s = SoulseekSettings { server: addr.to_string(), listen_port: 0, ..SoulseekSettings::default() };
        // Off by default.
        svc.apply(&s);
        assert!(svc.source().is_none());
        // On without an account: refused, nothing runs.
        s.enabled = true;
        svc.apply(&s);
        assert!(svc.source().is_none());
        s.username = "me".into();
        s.password = "pw".into();
        svc.apply(&s);
        let Some(Soulseek::Embedded(c)) = svc.source() else { panic!("not running") };
        assert!(until(|| c.status().shared_files == 1).await);
        assert!(until(|| got.lock().contains(&ToServer::SharedFoldersFiles { dirs: 1, files: 1 })).await);
        // The limit changes without a new session.
        s.upload_limit_kbps = 50;
        svc.apply(&s);
        assert_eq!(c.share_settings().upload_limit_kbps, 50);
        s.share = false;
        svc.apply(&s);
        assert_eq!(c.status().shared_files, 0);
        s.enabled = false;
        svc.apply(&s);
        assert!(svc.source().is_none());
        // The library folder was only read: still its Music and Trash.
        assert_eq!(std::fs::read_dir(lib.path()).unwrap().count(), 2);
    }

    fn file(user: &str, path: &str, format: &str, secs: u32, free: bool) -> SearchResult {
        SearchResult {
            user: user.into(),
            path: path.into(),
            size: 30 << 20,
            format: format.into(),
            lossless: format == "FLAC",
            bitrate: (format == "MP3").then_some(320),
            duration: Some(secs),
            sample_rate: Some(44_100),
            bit_depth: (format == "FLAC").then_some(16),
            free_slot: free,
            ..SearchResult::default()
        }
    }

    fn eve() -> Want {
        Want {
            artists: vec!["Eve".into()],
            title: Some("Underdog".into()),
            album: Some("Underdog".into()),
            duration_ms: Some(194_000),
            ..Want::default()
        }
    }

    #[test]
    fn a_peers_file_of_the_track_is_taken_by_its_folders_and_name() {
        let found = vec![
            file("a", "@@music\\Eve\\Underdog (2023) [FLAC]\\01 - Underdog.flac", "FLAC", 195, true),
            // Another song of the same folder, the cover sheet, a cut.
            file("a", "@@music\\Eve\\Underdog (2023) [FLAC]\\02 - Fanfare.flac", "FLAC", 200, true),
            file("a", "@@music\\Eve\\Underdog (2023) [FLAC]\\Underdog.jpg", "JPG", 0, true),
            file("b", "music\\Eve - Underdog (short).mp3", "MP3", 60, true),
            file("c", "share\\Eve\\Eve - Underdog.mp3", "MP3", 194, false),
        ];
        let offers = track_offers(&found, &eve());
        assert_eq!(offers.len(), 2, "{offers:#?}");
        let ranked = nikgob_sources::select::rank(offers, &eve(), &[]);
        let first = nikgob_sources::select::pick(&ranked, None).map(|i| &ranked[i].offer).unwrap();
        assert_eq!(first.provider, "a");
        assert!(first.lossless());
        assert_eq!(first.title, "Eve / Underdog (2023) [FLAC] / 01 - Underdog");
        // Another artist's song of that name is not it.
        let other = vec![file("d", "music\\Kanye\\Underdog.flac", "FLAC", 194, true)];
        assert!(nikgob_sources::select::rank(track_offers(&other, &eve()), &eve(), &[]).is_empty());
    }

    #[test]
    fn a_peers_folder_is_an_album() {
        let w = Want { title: None, scope: WantScope::Album, album: Some("Kaizin".into()), ..eve() };
        let found: Vec<SearchResult> = (1..=3)
            .map(|n| file("a", &format!("music\\Eve\\Kaizin [FLAC]\\0{n} - x.flac"), "FLAC", 200, true))
            .chain(std::iter::once(file("b", "music\\Eve\\Kaizin\\01 - x.mp3", "MP3", 200, true)))
            .collect();
        let offers = album_offers(&nikgob_soulseek::group_albums(&found));
        assert_eq!(offers.len(), 1, "a folder of one file is no album");
        assert_eq!(offers[0].scope, OfferScope::Album);
        assert_eq!(offers[0].size_bytes, Some(3 * (30 << 20)));
        let ranked = nikgob_sources::select::rank(offers, &w, &[]);
        assert_eq!(nikgob_sources::select::pick(&ranked, None), Some(0));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn the_running_client_is_a_download_source() {
        let (addr, _) = server().await;
        let lib = tempfile::tempdir().unwrap();
        let mut svc = SoulseekService::new(lib.path().to_path_buf(), None);
        assert!(svc.download_source().is_none(), "off: none");
        let s = SoulseekSettings {
            enabled: true,
            username: "me".into(),
            password: "pw".into(),
            server: addr.to_string(),
            listen_port: 0,
            share: false,
            ..SoulseekSettings::default()
        };
        svc.apply(&s);
        let src = svc.download_source().expect("running");
        assert_eq!(src.key(), SourceKey::Soulseek);
        // Asked from another runtime than its own; nobody shares it.
        let mut answer = src.search(&eve()).await;
        for _ in 0..100 {
            if !matches!(answer, Err(SourceError::Unreachable(_))) {
                break;
            }
            // Still logging in.
            tokio::time::sleep(Duration::from_millis(50)).await;
            answer = src.search(&eve()).await;
        }
        assert_eq!(answer.unwrap(), Vec::new());
        svc.shutdown();
    }
}

/// What the library shares: its songs (`Music`) and finished downloads
/// (`Downloads`), not the trash, the inbox or the player's own artist
/// photos beside them.
fn shared_folders(library: &std::path::Path) -> Vec<PathBuf> {
    if library.as_os_str().is_empty() {
        return Vec::new();
    }
    ["Music", "Downloads"].into_iter().map(|d| library.join(d)).filter(|d| d.is_dir()).collect()
}

#[cfg(test)]
mod share_tests {
    use super::shared_folders;

    #[test]
    fn only_the_songs_and_downloads_are_shared() {
        let dir = tempfile::tempdir().unwrap();
        for d in ["Music", "Downloads", "Trash", "Inbox", "Artists"] {
            std::fs::create_dir_all(dir.path().join(d)).unwrap();
        }
        assert_eq!(shared_folders(dir.path()), vec![dir.path().join("Music"), dir.path().join("Downloads")]);
        assert!(shared_folders(std::path::Path::new("")).is_empty());
    }
}
