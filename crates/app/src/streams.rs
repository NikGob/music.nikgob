//! The streaming services the app runs (docs/stage3.md §1 "app").
//!
//! Normal start: the real Spotify and YouTube Music services, each with its
//! own folder in the app's data dir (`spotify/`, `youtube/`) and its secrets
//! from settings.toml, and the Deezer catalogue (no folder, no secrets).
//! All are always registered; whether one is used is the runtime's call
//! (`Prefs::spotify_enabled` / `youtube_enabled` / `deezer_enabled`), so
//! switching a service on in the settings works without a restart.
//!
//! Demo (`NIKGOB_DEMO=1`): the deterministic fakes (Deezer's is its fixed
//! catalogue without an account). `NIKGOB_DEMO_ACCOUNTS`
//! sets their account per service for screenshots, e.g.
//! `spotify=prompt,youtube=out` (`in`, `out` = not connected, `free` =
//! logged in without sound, `prompt` = a login waiting for the browser —
//! for YouTube Music the app's Google sign-in window —, `failed`, and for
//! Spotify `premium` = a login refused for want of Premium, `nosound` =
//! Premium without the playback login yet; for YouTube Music `clips` =
//! likes made as music videos, one of them with its song, and «AIYOU - Eve
//! Music Video» whose song is only found by hand: «Найти песню…»).
//! `NIKGOB_DEMO_REAL_STREAMS=1` registers the real services instead, with
//! their folders inside the demo dir, so real results can be looked at
//! without touching the user's library or accounts.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use nikgob_core::settings::StreamingSettings;
use nikgob_core::streaming::{
    Account, AudioStream, BoxFuture, EntityKind, FakeService, LikedHead, LoginInput, LoginPrompt,
    Played, SearchKind, Service, StreamAlbum, StreamArtist, StreamError, StreamPlaylist, StreamSearch, StreamService,
    StreamTrack,
};

pub type Services = Vec<Arc<dyn StreamService>>;

/// The real services, their state under `data_dir`.
pub fn real(data_dir: &Path, secrets: &StreamingSettings) -> Services {
    let mut out: Services = Vec::new();
    let mut sp = nikgob_spotify::SpotifyConfig::new(data_dir.join("spotify"));
    sp.personal_client_id = secrets.spotify_client_id.clone();
    // Recently played audio: YouTube's files as they came, Spotify's
    // encrypted by librespot (never decrypted on disk). Bounded by
    // `Prefs::stream_cache_mb`, set by the runtime at start.
    let cache = data_dir.join("cache").join("streams");
    out.push(nikgob_spotify::Spotify::new(sp).with_audio_cache(cache.join("spotify")));
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
    out
}

/// What the demo runs: the fakes (accounts per `NIKGOB_DEMO_ACCOUNTS`) or,
/// with `NIKGOB_DEMO_REAL_STREAMS`, the real services in `demo_dir`.
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
    [Service::Spotify, Service::Youtube, Service::Deezer]
        .into_iter()
        .map(|s| fake(s, &mode(s.as_str())))
        .collect()
}

pub fn real_in_demo() -> bool {
    std::env::var_os("NIKGOB_DEMO_REAL_STREAMS").is_some()
}

fn fake(service: Service, mode: &str) -> Arc<dyn StreamService> {
    // The songs «Найти песню…» finds for the demo's AIYOU clip.
    let base = match service {
        Service::Youtube => FakeService::new(service)
            .with_song("aiyou", "AIYOU", "Eve", "AIYOU", 163)
            .with_song("aiyou-inst", "AIYOU (Instrumental)", "Eve", "AIYOU", 163)
            .with_song("kaikai", "Kaikai Kitan", "Eve", "Kaikai Kitan / Ao No Waltz", 221),
        Service::Deezer => FakeService::new(service)
            .with_song("aiyou", "AIYOU", "Eve", "AIYOU", 163)
            .with_song("aiyou-tv", "AIYOU (TV Size)", "Eve", "AIYOU", 89),
        Service::Spotify => FakeService::new(service),
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
struct Shown {
    inner: FakeService,
    account: Account,
}

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
