//! User settings, stored as TOML in the per-user config directory.

use std::fmt;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("settings I/O error at {}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid settings file {}: {source}", path.display())]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("cannot serialize settings: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("no per-user config directory available")]
    NoConfigDir,
}

pub type Result<T, E = SettingsError> = std::result::Result<T, E>;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub indexer: IndexerSettings,
    pub metadata: MetadataSettings,
    pub streaming: StreamingSettings,
    pub sources: SourcesSettings,
    pub soulseek: SoulseekSettings,
}

/// Where the download sources other than the indexer live
/// (docs/stage4.5.md). Public instances, no accounts: empty lists take the
/// built-in ones, a list of the user's own replaces them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SourcesSettings {
    /// Qobuz-DL instances (`https://host`, the API under `/api/`).
    pub qobuz_instances: Vec<String>,
    /// hifi-api instances for Tidal (`https://host`).
    pub tidal_instances: Vec<String>,
    /// khinsider's address (empty: downloads.khinsider.com).
    pub khinsider_url: String,
}

/// Where Soulseek runs (docs/stage4.5.md, poll answer 3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SoulseekMode {
    /// The client inside the player.
    #[default]
    Embedded,
    /// A `nikgob-slsk` service on a server or a small device.
    Remote,
}

/// The Soulseek source. The account is the user's own; nothing is filled in
/// by default, and the password and the service token never reach the
/// prefs JSON or logs.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SoulseekSettings {
    pub enabled: bool,
    pub mode: SoulseekMode,
    pub username: String,
    pub password: String,
    /// `host:port` of the Soulseek server.
    pub server: String,
    /// The port peers connect to.
    pub listen_port: u16,
    /// Share the library back (read-only), answer searches and browse.
    pub share: bool,
    /// KiB/s for all uploads together; 0 = no limit.
    pub upload_limit_kbps: u32,
    pub upload_slots: u32,
    /// The service's address in remote mode, like `http://pi.local:5031`.
    pub remote_url: String,
    pub remote_token: String,
}

impl Default for SoulseekSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: SoulseekMode::Embedded,
            username: String::new(),
            password: String::new(),
            server: "server.slsknet.org:2242".into(),
            listen_port: 2234,
            share: true,
            upload_limit_kbps: 0,
            upload_slots: 2,
            remote_url: String::new(),
            remote_token: String::new(),
        }
    }
}

impl fmt::Debug for SoulseekSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SoulseekSettings")
            .field("enabled", &self.enabled)
            .field("mode", &self.mode)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("server", &self.server)
            .field("listen_port", &self.listen_port)
            .field("share", &self.share)
            .field("upload_limit_kbps", &self.upload_limit_kbps)
            .field("upload_slots", &self.upload_slots)
            .field("remote_url", &self.remote_url)
            .field("remote_token", &"<redacted>")
            .finish()
    }
}

/// Secrets of the streaming services (docs/stage3.md §0). Tokens and
/// sessions live in each service's own cache in the data folder; this holds
/// only what the user types in. Never copied into the prefs JSON or logs.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StreamingSettings {
    /// Client ID of the user's own Spotify developer app (optional: its own
    /// Web API quota). `None` = the shared public app only.
    pub spotify_client_id: Option<String>,
    /// YouTube Music cookies (`cookies.txt` content or a `Cookie:` header),
    /// for the login when the device-code flow does not work; the app logs in
    /// with them again on start.
    pub youtube_cookies: Option<String>,
}

impl fmt::Debug for StreamingSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redact = |v: &Option<String>| v.as_ref().map(|_| "<redacted>");
        f.debug_struct("StreamingSettings")
            .field("spotify_client_id", &redact(&self.spotify_client_id))
            .field("youtube_cookies", &redact(&self.youtube_cookies))
            .finish()
    }
}

/// Online metadata lookups (MusicBrainz, AcoustID).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MetadataSettings {
    /// Contact put into the User-Agent (MusicBrainz asks for an e-mail or URL).
    pub contact: String,
    /// AcoustID application key (https://acoustid.org/new-application);
    /// without it identification uses tags and names only.
    pub acoustid_key: Option<String>,
    /// The user's own Last.fm API key (https://www.last.fm/api/account/create):
    /// the graph of similar music asks Last.fm only with it.
    pub lastfm_key: Option<String>,
    /// The user's own ListenBrainz token (https://listenbrainz.org/settings/):
    /// the graph's popularity asks ListenBrainz only with it.
    pub listenbrainz_token: Option<String>,
}

impl Default for MetadataSettings {
    fn default() -> Self {
        Self {
            contact: "music.nikgob personal player".into(),
            acoustid_key: None,
            lastfm_key: None,
            listenbrainz_token: None,
        }
    }
}

impl fmt::Debug for MetadataSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MetadataSettings")
            .field("contact", &self.contact)
            .field("acoustid_key", &self.acoustid_key.as_ref().map(|_| "<redacted>"))
            .field("lastfm_key", &self.lastfm_key.as_ref().map(|_| "<redacted>"))
            .field("listenbrainz_token", &self.listenbrainz_token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IndexerSettings {
    pub prowlarr: Option<ProwlarrSettings>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProwlarrSettings {
    /// Base URL, e.g. `https://prowlarr.example.com`.
    pub url: String,
    pub api_key: String,
}

impl fmt::Debug for ProwlarrSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProwlarrSettings")
            .field("url", &self.url)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl Settings {
    /// `<config dir>/settings.toml`.
    pub fn default_path() -> Result<PathBuf> {
        let dirs =
            ProjectDirs::from("dev", "nikgob", "music.nikgob").ok_or(SettingsError::NoConfigDir)?;
        Ok(dirs.config_dir().join("settings.toml"))
    }

    /// A missing file yields defaults.
    pub fn load(path: &Path) -> Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(SettingsError::Io {
                    path: path.into(),
                    source,
                });
            }
        };
        toml::from_str(&text).map_err(|source| SettingsError::Parse {
            path: path.into(),
            source,
        })
    }

    /// Writes a temp file next to `path`, then renames it over the target.
    pub fn save(&self, path: &Path) -> Result<()> {
        let io = |source| SettingsError::Io {
            path: path.into(),
            source,
        };
        let text = toml::to_string_pretty(self)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        let mut file = std::fs::File::create(&tmp).map_err(io)?;
        file.write_all(text.as_bytes()).map_err(io)?;
        file.sync_all().map_err(io)?;
        drop(file);
        std::fs::rename(&tmp, path).map_err(io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Settings {
        Settings {
            indexer: IndexerSettings {
                prowlarr: Some(ProwlarrSettings {
                    url: "http://localhost:9696".into(),
                    api_key: "s3cret-key".into(),
                }),
            },
            metadata: MetadataSettings::default(),
            streaming: StreamingSettings {
                spotify_client_id: Some("my-client-id".into()),
                youtube_cookies: Some("SID=cookie-secret".into()),
            },
            sources: SourcesSettings {
                qobuz_instances: vec!["https://qobuz.example".into()],
                ..SourcesSettings::default()
            },
            soulseek: SoulseekSettings {
                enabled: true,
                username: "slsk-user".into(),
                password: "slsk-pass".into(),
                remote_token: "slsk-token".into(),
                ..SoulseekSettings::default()
            },
        }
    }

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nikgob-core-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("settings.toml")
    }

    #[test]
    fn round_trip() {
        let path = temp_path("round-trip");
        sample().save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), sample());
        assert!(!path.with_extension("toml.tmp").exists());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn missing_file_is_default() {
        let path = temp_path("missing");
        assert_eq!(Settings::load(&path).unwrap(), Settings::default());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn soulseek_is_off_and_empty_by_default() {
        let s: Settings = toml::from_str("[soulseek]
username = \"me\"
").unwrap();
        assert_eq!(s.soulseek.username, "me");
        assert!(!s.soulseek.enabled);
        assert!(s.soulseek.password.is_empty());
        assert_eq!(Settings::default().soulseek.username, "");
    }

    #[test]
    fn partial_file_fills_defaults() {
        let s: Settings = toml::from_str("[indexer]\n").unwrap();
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn debug_redacts_api_key() {
        let dbg = format!("{:?}", sample());
        assert!(!dbg.contains("s3cret-key"));
        assert!(!dbg.contains("cookie-secret"));
        assert!(!dbg.contains("my-client-id"));
        assert!(!dbg.contains("slsk-pass"));
        assert!(!dbg.contains("slsk-token"));
        assert!(dbg.contains("localhost:9696"));
    }
}
