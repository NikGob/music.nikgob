//! A token made without the window: the headless player's command line
//! (`nikgobd token …`) writes it into settings.toml and prints the secret
//! once. Only while no player owns those settings: it takes the player's
//! own lock file for the time of the write and refuses when the player
//! holds it (a running player keeps settings.toml as it knows it).

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
    /// A player owns these settings now (it holds the lock file).
    PlayerRunning,
    /// The lock file could not be opened.
    Lock(std::io::Error),
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Settings(e) => write!(f, "{e}"),
            Self::NoRandom => f.write_str("the system gives no randomness"),
            Self::NoName => f.write_str("a token needs a name"),
            Self::PlayerRunning => f.write_str("the player is running: stop it first"),
            Self::Lock(e) => write!(f, "the lock file: {e}"),
        }
    }
}

impl std::error::Error for TokenError {}

/// Adds a token named `name` to `[api]` of the settings at `path` and
/// returns its secret (kept nowhere: show it once). `lock` is the lock file
/// of the player that owns these settings (nikgobd's `nikgobd.lock` in its
/// data dir): held for the write, [`TokenError::PlayerRunning`] while the
/// player holds it.
pub fn create_token(path: &Path, lock: &Path, name: &str, preset: ApiPreset, actor: ApiActor) -> Result<String, TokenError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(TokenError::NoName);
    }
    if let Some(dir) = lock.parent() {
        std::fs::create_dir_all(dir).map_err(TokenError::Lock)?;
    }
    // Not truncated: the player writes its pid there.
    let lock_file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(lock).map_err(TokenError::Lock)?;
    match lock_file.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => return Err(TokenError::PlayerRunning),
        Err(std::fs::TryLockError::Error(e)) => return Err(TokenError::Lock(e)),
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
    // The lock goes with the file.
    drop(lock_file);
    Ok(secret)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_made_here_is_kept_hashed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let lock = dir.path().join("nikgobd.lock");
        let secret = create_token(&path, &lock, " Nux ", ApiPreset::Control, ApiActor::Assistant).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains(&secret), "the secret is not written");
        let s = Settings::load(&path).unwrap();
        assert_eq!(s.api.tokens.len(), 1);
        assert_eq!(s.api.tokens[0].name, "Nux");
        assert!(api_token::matches(&secret, &s.api.tokens[0].hash));
        assert!(matches!(create_token(&path, &lock, "  ", ApiPreset::Read, ApiActor::Owner), Err(TokenError::NoName)));
    }

    /// While the player holds its lock, no token is written.
    #[test]
    fn not_while_the_player_runs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let lock = dir.path().join("nikgobd.lock");
        let player = std::fs::File::create(&lock).unwrap();
        player.lock().unwrap();
        assert!(matches!(create_token(&path, &lock, "Nux", ApiPreset::Control, ApiActor::Assistant), Err(TokenError::PlayerRunning)));
        assert!(!path.exists());
        drop(player);
        assert!(create_token(&path, &lock, "Nux", ApiPreset::Control, ApiActor::Assistant).is_ok());
    }
}
