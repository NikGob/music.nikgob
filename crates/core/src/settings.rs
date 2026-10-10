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
}

impl Default for MetadataSettings {
    fn default() -> Self {
        Self {
            contact: "music.nikgob personal player".into(),
            acoustid_key: None,
        }
    }
}

impl fmt::Debug for MetadataSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MetadataSettings")
            .field("contact", &self.contact)
            .field("acoustid_key", &self.acoustid_key.as_ref().map(|_| "<redacted>"))
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
    fn partial_file_fills_defaults() {
        let s: Settings = toml::from_str("[indexer]\n").unwrap();
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn debug_redacts_api_key() {
        let dbg = format!("{:?}", sample());
        assert!(!dbg.contains("s3cret-key"));
        assert!(dbg.contains("localhost:9696"));
    }
}
