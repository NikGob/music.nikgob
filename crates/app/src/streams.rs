//! The streaming services the app runs.
//!
//! Normal start: the real Spotify, YouTube Music and SoundCloud services,
//! each with its own folder in the app's data dir (`spotify/`, `youtube/`,
//! `soundcloud/`) and its secrets from settings.toml (SoundCloud's token
//! from the credential store), and the Deezer catalogue (no folder, no
//! secrets).
//! All are always registered; whether one is used is the runtime's call
//! (`Prefs::spotify_enabled` / `youtube_enabled` / `deezer_enabled`), so
//! switching a service on in the settings works without a restart.
//!
//! Demo (`NIKGOB_DEMO=1`): the deterministic fakes (Deezer's is its fixed
//! catalogue without an account; YouTube Music and Deezer also have two
//! singles of the last weeks for new releases, and every fake its
//! discovery answers). `NIKGOB_DEMO_ACCOUNTS`
//! sets their account per service for screenshots, e.g.
//! `spotify=prompt,youtube=out` (`in`, `out` = not connected, `free` =
//! logged in without sound, `prompt` = a login waiting for the browser —
//! for YouTube Music the app's Google sign-in window —, `failed`, and for
//! Spotify `premium` = a login refused for want of Premium, `nosound` =
//! Premium without the playback login yet; for YouTube Music `clips` =
//! likes made as music videos, one of them with its song, and «AIYOU - Eve
//! Music Video» whose song is only found by hand: «Найти песню…»).
//! `NIKGOB_DEMO_REAL_STREAMS=1` registers the real services instead, with
//! their folders inside the demo dir, and turns the online metadata on
//! (the contact from the demo dir's `settings.toml`), so real results can be
//! looked at without touching the user's library or accounts.

use std::path::Path;
use std::sync::Arc;

use nikgob_core::settings::StreamingSettings;
use nikgob_core::streaming::{Account, LoginInput, Service, StreamService};

#[cfg(any(feature = "dev", test))]
use std::path::PathBuf;
#[cfg(any(feature = "dev", test))]
use nikgob_core::streaming::{
    AudioStream, BoxFuture, EntityKind, FakeService, LikedHead, LoginPrompt, Played, SearchKind,
    SoloistAction, SoloistPhase, SoloistStatus, StreamAlbum, StreamArtist, StreamError, StreamPlaylist,
    StreamSearch, StreamTrack,
};

pub type Services = Vec<Arc<dyn StreamService>>;

/// The real Spotify's Soloist, for the audio engine (set by [`real`]).
static SOLOIST: std::sync::OnceLock<Arc<nikgob_soloist::Soloist>> = std::sync::OnceLock::new();

/// Spotify Soloist, when the real Spotify runs.
pub fn soloist() -> Option<Arc<nikgob_soloist::Soloist>> {
    SOLOIST.get().cloned()
}

/// SoundCloud's official player for DRM-only tracks (Windows, the real
/// SoundCloud; set by [`real`]).
static SC_WIDGET: std::sync::OnceLock<Arc<nikgob_sc_widget::ScWidget>> = std::sync::OnceLock::new();

/// SoundCloud's official player, when it can run here.
pub fn sc_widget() -> Option<Arc<nikgob_sc_widget::ScWidget>> {
    SC_WIDGET.get().cloned()
}

/// The real SoundCloud (set by [`real`]): its sign-in fetches the
/// originals its uploaders let download.
static SOUNDCLOUD: std::sync::OnceLock<Arc<nikgob_soundcloud::Soundcloud>> = std::sync::OnceLock::new();

/// SoundCloud's originals as a download source, when the real SoundCloud
/// runs.
pub fn soundcloud_originals() -> Option<Arc<dyn nikgob_core::source::Source>> {
    let sc: Arc<dyn nikgob_sources::soundcloud::Originals> = SOUNDCLOUD.get()?.clone();
    Some(Arc::new(nikgob_sources::soundcloud::SoundcloudOriginals::new(sc)))
}

/// Stops Soloist and SoundCloud's player host as the app quits.
pub fn shutdown() {
    if let Some(s) = SOLOIST.get() {
        s.shutdown();
    }
    if let Some(w) = SC_WIDGET.get() {
        w.shutdown();
    }
}

/// The demo Spotify's Soloist (`NIKGOB_DEMO_SOLOIST`: `absent`, `offer`,
/// `downloading`, `key`, `pairing`, `ready`, `off`, `failed`), for
/// screenshots of its setup.
#[cfg(any(feature = "dev", test))]
fn demo_soloist(base: FakeService) -> FakeService {
    let Ok(mode) = std::env::var("NIKGOB_DEMO_SOLOIST") else {
        return base;
    };
    let phase = match mode.as_str() {
        "absent" | "offer" => SoloistPhase::Absent,
        "downloading" => SoloistPhase::Downloading { done: 4_200_000, total: Some(9_780_408) },
        "key" => SoloistPhase::NeedsKey,
        "pairing" => SoloistPhase::Pairing,
        "ready" => SoloistPhase::Ready,
        "off" => SoloistPhase::Off,
        "failed" => SoloistPhase::Failed("Soloist остановился".into()),
        _ => return base,
    };
    base.with_soloist(phase, mode == "offer")
}

/// The real services, their state under `data_dir`.
pub fn real(data_dir: &Path, secrets: &StreamingSettings) -> Services {
    let mut out: Services = Vec::new();
    let mut sp = nikgob_spotify::SpotifyConfig::new(data_dir.join("spotify"));
    sp.personal_client_id = secrets.spotify_client_id.clone();
    // Recently played audio: YouTube's files as they came, Spotify's
    // encrypted by librespot (never decrypted on disk). Bounded by
    // `Prefs::stream_cache_mb`, set by the runtime at start.
    let cache = data_dir.join("cache").join("streams");
    let spotify = nikgob_spotify::Spotify::new(sp)
        .with_audio_cache(cache.join("spotify"))
        .with_soloist(data_dir.join("spotify").join("soloist"));
    if let Some(s) = spotify.soloist_handle() {
        let _ = SOLOIST.set(s);
    }
    out.push(spotify);
    match nikgob_youtube::Youtube::new(nikgob_youtube::YoutubeConfig::new(data_dir.join("youtube"))) {
        Ok(yt) => {
            yt.warm_up();
            out.push(yt.with_audio_cache(cache.join("youtube")));
        }
        Err(e) => log::error!("YouTube Music did not start: {e}"),
    }
    match nikgob_deezer::Deezer::new(nikgob_deezer::DeezerConfig::default()) {
        Ok(dz) => out.push(dz),
        Err(e) => log::error!("Deezer did not start: {e}"),
    }
    let mut sc = nikgob_soundcloud::SoundcloudConfig::new(data_dir.join("soundcloud"));
    // The demo's real services keep their sign-in out of the user's store.
    sc.memory_store = real_in_demo();
    match nikgob_soundcloud::Soundcloud::new(sc) {
        Ok(sc) => {
            let _ = SOUNDCLOUD.set(sc.clone());
            out.push(sc);
        }
        Err(e) => log::error!("SoundCloud did not start: {e}"),
    }
    // Its DRM-only tracks play through its own player (WebView2 has the
    // Widevine CDM; WebKitGTK has none, so not on Linux). Its profile is its
    // own, never the sign-in window's: it plays signed out.
    if cfg!(windows)
        && let Ok(exe) = std::env::current_exe()
    {
        let _ = SC_WIDGET.set(nikgob_sc_widget::ScWidget::new(nikgob_sc_widget::Config {
            exe,
            profile: data_dir.join("soundcloud").join("widget"),
        }));
    }
    out
}

pub fn real_in_demo() -> bool {
    #[cfg(feature = "dev")]
    {
        std::env::var_os("NIKGOB_DEMO_REAL_STREAMS").is_some()
    }
    #[cfg(not(feature = "dev"))]
    {
        false
    }
}

/// What the demo runs: the fakes (accounts per `NIKGOB_DEMO_ACCOUNTS`) or,
/// with `NIKGOB_DEMO_REAL_STREAMS`, the real services in `demo_dir`.
#[cfg(any(feature = "dev", test))]
pub fn demo(demo_dir: &Path, secrets: &StreamingSettings) -> Services {
    if real_in_demo() {
        return real(demo_dir, secrets);
    }
    let accounts = std::env::var("NIKGOB_DEMO_ACCOUNTS").unwrap_or_default();
    let mode = |name: &str| {
        accounts
            .split(',')
            .filter_map(|kv| kv.split_once('='))
            .find(|(k, _)| k.trim().eq_ignore_ascii_case(name))
            .map(|(_, v)| v.trim().to_ascii_lowercase())
            .unwrap_or_else(|| "in".to_owned())
    };
    [Service::Spotify, Service::Youtube, Service::Deezer, Service::Soundcloud]
        .into_iter()
        .map(|s| fake(s, &mode(s.as_str())))
        .collect()
}

#[cfg(any(feature = "dev", test))]
fn fake(service: Service, mode: &str) -> Arc<dyn StreamService> {
    // The songs «Найти песню…» finds for the demo's AIYOU clip.
    let base = match service {
        Service::Youtube => FakeService::new(service)
            .with_song("aiyou", "AIYOU", "Eve", "AIYOU", 163)
            .with_song("aiyou-inst", "AIYOU (Instrumental)", "Eve", "AIYOU", 163)
            .with_song("kaikai", "Kaikai Kitan", "Eve", "Kaikai Kitan / Ao No Waltz", 221)
            .with_new_release("utada", "kaze", "風の歌", 23)
            .with_new_release("bjork", "aurora", "Aurora", 9),
        Service::Deezer => FakeService::new(service)
            .with_song("aiyou", "AIYOU", "Eve", "AIYOU", 163)
            .with_song("aiyou-tv", "AIYOU (TV Size)", "Eve", "AIYOU", 89)
            .with_new_release("utada", "kaze", "風の歌", 23)
            .with_new_release("bjork", "aurora", "Aurora", 9)
            // An artist of the demo library: «Новые релизы» has its single.
            .with_new_release("daft", "voyager2", "Voyager 2", 12),
        Service::Spotify => demo_soloist(FakeService::new(service)),
        Service::Soundcloud => FakeService::new(service),
    };
    // Long enough to watch a track play (lyrics, progress) in screenshots.
    base.set_audio_secs(180.0);
    match mode {
        "out" => Arc::new(base.logged_out()),
        // YouTube likes made as clips: one whose song YouTube Music has
        // (it imports as the song), two without (they stay clips).
        "clips" if service == Service::Youtube => {
            let base = base
                .with_video("ghost", "Ghost Avenue", "Eve", 232, None)
                .with_video("natsuko", "夏子 - Natsuko", "Funky Monkey Bλby's", 301, None)
                .with_video("aiyou", "AIYOU - Eve Music Video", "Eve", 165, None);
            for id in base.liked_ids() {
                base.set_liked_remotely(&id, false);
            }
            for id in ["yt-video-natsuko", "yt-discovery-1", "yt-video-ghost", "yt-video-kino", "yt-video-aiyou"] {
                base.set_liked_remotely(id, true);
            }
            Arc::new(base)
        }
        "free" => Arc::new(base.without_audio()),
        "nosound" => Arc::new(base.premium_without_audio()),
        "premium" => Arc::new(Shown {
            inner: base.logged_out(),
            account: Account::Failed(nikgob_core::streaming::SPOTIFY_PREMIUM_REQUIRED.into()),
        }),
        // Logged in with sound, but every track fails once it is read
        // (Spotify refusing the key of a Free account mid-open).
        "refused" => {
            for t in base.tracks() {
                base.set_unavailable(&t.id.id, true);
            }
            Arc::new(base)
        }
        "prompt" | "failed" => Arc::new(Shown {
            inner: base.logged_out(),
            account: if mode == "prompt" && service == Service::Youtube {
                Account::LoggingIn(Some(LoginPrompt::Window(nikgob_runtime::WINDOW_HINT.into())))
            } else if mode == "failed" && service == Service::Youtube {
                Account::Failed(nikgob_runtime::WebLoginError::Refused.text())
            } else if mode == "prompt" {
                Account::LoggingIn(Some(LoginPrompt::OpenUrl(format!(
                    "https://accounts.spotify.com/authorize?client_id=demo-client-id&response_type=code&redirect_uri={}&code_challenge_method=S256",
                    nikgob_spotify::WEB_REDIRECT_URI,
                ))))
            } else {
                Account::Failed("Порт 8989 занят другой программой".into())
            },
        }),
        _ => Arc::new(base),
    }
}

/// A fake whose account always reads `account` (a login frozen in the
/// middle, a failure), for screenshots of those states.
#[cfg(any(feature = "dev", test))]
struct Shown {
    inner: FakeService,
    account: Account,
}

#[cfg(any(feature = "dev", test))]
impl StreamService for Shown {
    fn service(&self) -> Service {
        self.inner.service()
    }
    fn catalogue_only(&self) -> bool {
        self.inner.catalogue_only()
    }
    fn search_top(&self, query: &str, part: nikgob_core::streaming::SearchParts) -> BoxFuture<'_, Result<StreamSearch, StreamError>> {
        self.inner.search_top(query, part)
    }
    fn account(&self) -> Account {
        self.account.clone()
    }
    fn login(&self, input: LoginInput) -> BoxFuture<'_, Result<(), StreamError>> {
        self.inner.login(input)
    }
    fn logout(&self) {
        self.inner.logout();
    }
    fn search(&self, query: &str) -> BoxFuture<'_, Result<StreamSearch, StreamError>> {
        self.inner.search(query)
    }
    fn track(&self, id: &str) -> BoxFuture<'_, Result<StreamTrack, StreamError>> {
        self.inner.track(id)
    }
    fn album(&self, id: &str) -> BoxFuture<'_, Result<StreamAlbum, StreamError>> {
        self.inner.album(id)
    }
    fn artist(&self, id: &str) -> BoxFuture<'_, Result<StreamArtist, StreamError>> {
        self.inner.artist(id)
    }
    fn playlist(&self, id: &str) -> BoxFuture<'_, Result<StreamPlaylist, StreamError>> {
        self.inner.playlist(id)
    }
    fn find_isrc(&self, isrc: &str) -> BoxFuture<'_, Result<Option<StreamTrack>, StreamError>> {
        self.inner.find_isrc(isrc)
    }
    fn liked(&self) -> BoxFuture<'_, Result<Vec<StreamTrack>, StreamError>> {
        self.inner.liked()
    }
    fn set_liked(&self, id: &str, liked: bool) -> BoxFuture<'_, Result<(), StreamError>> {
        self.inner.set_liked(id, liked)
    }
    fn my_playlists(&self) -> BoxFuture<'_, Result<Vec<StreamPlaylist>, StreamError>> {
        self.inner.my_playlists()
    }
    fn create_playlist(&self, name: &str) -> BoxFuture<'_, Result<String, StreamError>> {
        self.inner.create_playlist(name)
    }
    fn playlist_edit(
        &self,
        id: &str,
        add: &[String],
        remove: &[String],
    ) -> BoxFuture<'_, Result<Option<String>, StreamError>> {
        self.inner.playlist_edit(id, add, remove)
    }
    fn recent(&self) -> BoxFuture<'_, Result<Vec<Played>, StreamError>> {
        self.inner.recent()
    }
    fn open_audio(&self, id: &str) -> Result<Box<dyn AudioStream>, StreamError> {
        self.inner.open_audio(id)
    }
    fn save_audio(&self, id: &str, dir: &Path) -> BoxFuture<'_, Result<PathBuf, StreamError>> {
        self.inner.save_audio(id, dir)
    }
    fn web_url(&self, kind: EntityKind, id: &str) -> String {
        self.inner.web_url(kind, id)
    }
    fn soloist(&self) -> Option<SoloistStatus> {
        self.inner.soloist()
    }
    fn soloist_act(&self, action: SoloistAction) {
        self.inner.soloist_act(action);
    }
    fn search_kind(&self, query: &str, kind: SearchKind) -> BoxFuture<'_, Result<StreamSearch, StreamError>> {
        self.inner.search_kind(query, kind)
    }
    fn tracks(&self, ids: &[String]) -> BoxFuture<'_, Result<Vec<Option<StreamTrack>>, StreamError>> {
        StreamService::tracks(&self.inner, ids)
    }
    fn liked_head(&self) -> BoxFuture<'_, Result<LikedHead, StreamError>> {
        self.inner.liked_head()
    }
    fn set_liked_many(&self, ids: &[String], liked: bool) -> BoxFuture<'_, Result<(), StreamError>> {
        self.inner.set_liked_many(ids, liked)
    }
    fn playlist_version(&self, id: &str) -> BoxFuture<'_, Result<Option<String>, StreamError>> {
        self.inner.playlist_version(id)
    }
    fn cached_audio(&self, id: &str) -> Option<Box<dyn AudioStream>> {
        self.inner.cached_audio(id)
    }
    fn related(&self, track_id: &str) -> BoxFuture<'_, Result<Vec<StreamTrack>, StreamError>> {
        self.inner.related(track_id)
    }
    fn radio(&self, track_id: &str) -> BoxFuture<'_, Result<Vec<StreamTrack>, StreamError>> {
        self.inner.radio(track_id)
    }
    fn similar_artists(&self, artist_id: &str) -> BoxFuture<'_, Result<Vec<StreamArtist>, StreamError>> {
        self.inner.similar_artists(artist_id)
    }
    fn artist_top(&self, artist_id: &str) -> BoxFuture<'_, Result<Vec<StreamTrack>, StreamError>> {
        self.inner.artist_top(artist_id)
    }
    fn artist_radio(&self, artist_id: &str) -> BoxFuture<'_, Result<Vec<StreamTrack>, StreamError>> {
        self.inner.artist_radio(artist_id)
    }
}

/// YouTube Music keeps its session in its own cache; when that is gone but
/// settings.toml still has the cookies, log in with them again.
pub fn relogin(services: &Services, secrets: &StreamingSettings) -> Option<nikgob_view::Command> {
    let cookies = secrets.youtube_cookies.clone()?;
    let yt = services.iter().find(|s| s.service() == Service::Youtube)?;
    matches!(yt.account(), Account::LoggedOut).then(|| nikgob_view::Command::StreamLogin {
        service: Service::Youtube,
        input: LoginInput::Cookies(cookies),
    })
}
