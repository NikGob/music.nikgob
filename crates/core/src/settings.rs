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
    pub api: ApiSettings,
    pub mesh: MeshSettings,
}

/// The owner's devices (`[mesh]`): on unless switched off; this device's
/// name (empty: the computer's own name).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MeshSettings {
    pub enabled: bool,
    pub name: String,
}

impl Default for MeshSettings {
    fn default() -> Self {
        Self { enabled: true, name: String::new() }
    }
}

/// The default port of the local REST API.
pub const API_PORT: u16 = 47210;

/// Who may reach the local REST API.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiAccess {
    /// This computer only (`127.0.0.1`).
    #[default]
    Local,
    /// The home network too (`0.0.0.0`, or [`ApiSettings::address`]).
    Network,
}

/// What a token may do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiPreset {
    /// The player, the queue, the volume, the duck, the output, reading and
    /// searching.
    #[default]
    Control,
    /// Everything but the tokens and the devices.
    Full,
    /// Reading and searching only.
    Read,
}

impl ApiPreset {
    pub const ALL: [Self; 3] = [Self::Control, Self::Full, Self::Read];

    pub const fn key(self) -> &'static str {
        match self {
            Self::Control => "control",
            Self::Full => "full",
            Self::Read => "read",
        }
    }
}

/// Who plays with a token: the listening history keeps it, so music an
/// assistant or a guest turned on does not bend the owner's
/// recommendations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiActor {
    #[default]
    Owner,
    Assistant,
    Agent,
    Guest,
}

impl ApiActor {
    pub const ALL: [Self; 4] = [Self::Owner, Self::Assistant, Self::Agent, Self::Guest];

    pub const fn key(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Assistant => "assistant",
            Self::Agent => "agent",
            Self::Guest => "guest",
        }
    }
}

/// One token of the API. The secret itself is never stored: only its
/// BLAKE3 hash ([`crate::api_token::hash`]).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiTokenRecord {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub preset: ApiPreset,
    #[serde(default)]
    pub actor: ApiActor,
    /// Hex BLAKE3 of the secret.
    pub hash: String,
    #[serde(default)]
    pub created_ms: i64,
    #[serde(default)]
    pub last_used_ms: Option<i64>,
    /// Revoked at (unix ms): refused from then on, removed once its undo
    /// is gone.
    #[serde(default)]
    pub revoked_ms: Option<i64>,
}

impl fmt::Debug for ApiTokenRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApiTokenRecord")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("preset", &self.preset)
            .field("actor", &self.actor)
            .field("hash", &"<redacted>")
            .field("created_ms", &self.created_ms)
            .field("last_used_ms", &self.last_used_ms)
            .field("revoked_ms", &self.revoked_ms)
            .finish()
    }
}

/// The local REST API (`[api]`): off until switched on, on this computer
/// only unless the user opens it to the home network.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiSettings {
    pub enabled: bool,
    pub access: ApiAccess,
    /// Listen on exactly this address (`192.168.0.157`), whatever `access`
    /// says; empty: by `access`.
    pub address: String,
    pub port: u16,
    pub tokens: Vec<ApiTokenRecord>,
}

impl Default for ApiSettings {
    fn default() -> Self {
        Self { enabled: false, access: ApiAccess::Local, address: String::new(), port: API_PORT, tokens: Vec::new() }
    }
}

impl ApiSettings {
    /// The address the server listens on: the explicit one, else by
    /// [`Self::access`].
    pub fn bind_ip(&self) -> String {
        let explicit = self.address.trim();
        if !explicit.is_empty() {
            return explicit.to_owned();
        }
        match self.access {
            ApiAccess::Local => "127.0.0.1".to_owned(),
            ApiAccess::Network => "0.0.0.0".to_owned(),
        }
    }
}

/// Where the download sources other than the indexer live. Public instances, no accounts. Qobuz and Tidal
/// instance lists live in `sources.toml` beside this file (written once
/// with the built-in addresses). A non-empty list here is copied into that
/// file only when it is first created.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SourcesSettings {
    /// Qobuz-DL instances (`https://host`, the API under `/api/`).
    /// Copied into `sources.toml` once, when that file is first created.
    pub qobuz_instances: Vec<String>,
    /// hifi-api instances for Tidal (`https://host`).
    /// Copied into `sources.toml` once, when that file is first created.
    pub tidal_instances: Vec<String>,
    /// khinsider's address (empty: downloads.khinsider.com).
    pub khinsider_url: String,
    /// A Subsonic server's address (empty: squidify.org), like
    /// `https://music.example` or the user's own Navidrome.
    pub subsonic_url: String,
    /// Its login (empty: the guest login squidify.org publishes).
    pub subsonic_user: String,
    /// Never reaches logs.
    pub subsonic_password: String,
    /// The Internet Archive's address (empty: archive.org).
    pub archive_url: String,
    /// A play.dogmazic.net login (free to register). Empty: its public
    /// pages, which anyone may search and download from.
    pub dogmazic_user: String,
    /// Never reaches logs.
    pub dogmazic_password: String,
}

impl fmt::Debug for SourcesSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SourcesSettings")
            .field("qobuz_instances", &self.qobuz_instances)
            .field("tidal_instances", &self.tidal_instances)
            .field("khinsider_url", &self.khinsider_url)
            .field("subsonic_url", &self.subsonic_url)
            .field("subsonic_user", &self.subsonic_user)
            .field("subsonic_password", &"<redacted>")
            .field("archive_url", &self.archive_url)
            .field("dogmazic_user", &self.dogmazic_user)
            .field("dogmazic_password", &"<redacted>")
            .finish()
    }
}

/// Where Soulseek runs.
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

/// Secrets of the streaming services. Tokens and
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
    /// The user's own MusicLink API key (https://musiclink.one/dashboard,
    /// free plan): «Поделиться» then gets a MusicLink page for every album,
    /// also one MusicLink does not know yet. Optional: songs and known
    /// albums need no key.
    pub musiclink_key: Option<String>,
}

impl Default for MetadataSettings {
    fn default() -> Self {
        Self {
            contact: "music.nikgob personal player".into(),
            acoustid_key: None,
            lastfm_key: None,
            listenbrainz_token: None,
            musiclink_key: None,
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
            .field("musiclink_key", &self.musiclink_key.as_ref().map(|_| "<redacted>"))
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
        let tmp = temp_path(path);
        let written = (|| {
            // A name of this write alone: two writers never share it.
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&tmp, path)
        })();
        if written.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        written.map_err(io)
    }
}

/// The temporary file a save of `path` writes first: by this process and
/// this save, so no other save (here or in another process) writes it.
fn temp_path(path: &Path) -> PathBuf {
    static SAVES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SAVES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.{n}.tmp", std::process::id()));
    PathBuf::from(tmp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_save_writes_its_own_temporary_file() {
        let p = Path::new("settings.toml");
        let (a, b) = (super::temp_path(p), super::temp_path(p));
        assert_ne!(a, b);
        assert!(a.to_string_lossy().starts_with("settings.toml."));
        assert!(a.to_string_lossy().ends_with(".tmp"));
    }

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
                subsonic_user: "navi-user".into(),
                subsonic_password: "navi-pass".into(),
                dogmazic_user: "dz-user".into(),
                dogmazic_password: "dz-pass".into(),
                ..SourcesSettings::default()
            },
            soulseek: SoulseekSettings {
                enabled: true,
                username: "slsk-user".into(),
                password: "slsk-pass".into(),
                remote_token: "slsk-token".into(),
                ..SoulseekSettings::default()
            },
            api: ApiSettings {
                enabled: true,
                access: ApiAccess::Network,
                address: "192.168.0.157".into(),
                port: 47211,
                tokens: vec![ApiTokenRecord {
                    id: "t1".into(),
                    name: "Ассистент на Pi".into(),
                    preset: ApiPreset::Control,
                    actor: ApiActor::Assistant,
                    hash: "api-hash-secret".into(),
                    created_ms: 1_763_300_000_000,
                    last_used_ms: Some(1_763_300_000_420),
                    revoked_ms: None,
                }],
            },
            mesh: MeshSettings { enabled: false, name: "Кабинет".into() },
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
        assert!(!dbg.contains("navi-pass"));
        assert!(!dbg.contains("dz-pass"));
        assert!(!dbg.contains("api-hash-secret"));
        assert!(dbg.contains("navi-user"));
        assert!(dbg.contains("localhost:9696"));
    }

    /// `[api]` reads back as written, a file without it is the defaults
    /// (off, this computer only, port 47210), and the words in the file are
    /// the ones the API speaks.
    #[test]
    fn mesh_section_round_trips() {
        let path = temp_path("mesh");
        let s = sample();
        s.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[mesh]"), "{text}");
        assert_eq!(Settings::load(&path).unwrap().mesh, s.mesh);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();

        let none: Settings = toml::from_str("[soulseek]\n").unwrap();
        assert!(none.mesh.enabled, "on unless switched off");
        assert!(none.mesh.name.is_empty());
        let named: Settings = toml::from_str("[mesh]\nname = \"Pi\"\n").unwrap();
        assert!(named.mesh.enabled);
        assert_eq!(named.mesh.name, "Pi");
    }

    #[test]
    fn api_section_round_trips() {
        let path = temp_path("api");
        let s = sample();
        s.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[api]"), "{text}");
        assert!(text.contains("access = \"network\""), "{text}");
        assert!(text.contains("preset = \"control\""), "{text}");
        assert!(text.contains("actor = \"assistant\""), "{text}");
        assert!(!text.contains("revoked_ms"), "None is left out: {text}");
        assert_eq!(Settings::load(&path).unwrap().api, s.api);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();

        let none: Settings = toml::from_str("[soulseek]\n").unwrap();
        assert_eq!(none.api, ApiSettings::default());
        assert!(!none.api.enabled);
        assert_eq!(none.api.port, API_PORT);
        assert_eq!(none.api.bind_ip(), "127.0.0.1");
        let open = ApiSettings { access: ApiAccess::Network, ..ApiSettings::default() };
        assert_eq!(open.bind_ip(), "0.0.0.0");
        let pinned = ApiSettings { address: " 192.168.0.157 ".into(), ..ApiSettings::default() };
        assert_eq!(pinned.bind_ip(), "192.168.0.157");

        let partial: Settings = toml::from_str(
            "[api]\nenabled = true\n[[api.tokens]]\nid = \"a\"\nname = \"x\"\nhash = \"h\"\n",
        )
        .unwrap();
        assert_eq!(partial.api.port, API_PORT);
        assert_eq!(partial.api.tokens[0].preset, ApiPreset::Control);
        assert_eq!(partial.api.tokens[0].actor, ApiActor::Owner);
    }
}
