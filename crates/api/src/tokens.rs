//! A token made without the window: the headless player's command line
//! (`nikgobd token …`) writes it into settings.toml while the player is
//! not running and prints the secret once. A running player reads its
//! tokens only at start.

use std::path::Path;

use nikgob_core::api_token;
use nikgob_core::settings::{ApiActor, ApiPreset, ApiTokenRecord, Settings, SettingsError};

#[derive(Debug)]
pub enum TokenError {
    Settings(SettingsError),
    /// The system gave no randomness.
    NoRandom,
    /// A token needs a name.
    NoName,
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Settings(e) => write!(f, "{e}"),
            Self::NoRandom => f.write_str("the system gives no randomness"),
            Self::NoName => f.write_str("a token needs a name"),
        }
    }
}

impl std::error::Error for TokenError {}

/// Adds a token named `name` to `[api]` of the settings at `path` and
/// returns its secret (kept nowhere: show it once).
pub fn create_token(path: &Path, name: &str, preset: ApiPreset, actor: ApiActor) -> Result<String, TokenError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(TokenError::NoName);
    }
    let mut settings = Settings::load(path).map_err(TokenError::Settings)?;
    let secret = api_token::generate().ok_or(TokenError::NoRandom)?;
    let id = api_token::new_id().ok_or(TokenError::NoRandom)?;
    settings.api.tokens.push(ApiTokenRecord {
        id,
        name: name.to_owned(),
        preset,
        actor,
        hash: api_token::hash(&secret),
        created_ms: nikgob_library::now_ms(),
        last_used_ms: None,
        revoked_ms: None,
    });
    settings.save(path).map_err(TokenError::Settings)?;
    Ok(secret)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_made_here_is_kept_hashed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let secret = create_token(&path, " Nux ", ApiPreset::Control, ApiActor::Assistant).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains(&secret), "the secret is not written");
        let s = Settings::load(&path).unwrap();
        assert_eq!(s.api.tokens.len(), 1);
        assert_eq!(s.api.tokens[0].name, "Nux");
        assert!(api_token::matches(&secret, &s.api.tokens[0].hash));
        assert!(matches!(create_token(&path, "  ", ApiPreset::Read, ApiActor::Owner), Err(TokenError::NoName)));
    }
}
