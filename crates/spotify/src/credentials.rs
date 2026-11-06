//! Durable Spotify grants in the platform credential store (Windows
//! Credential Manager, the Secret Service on Linux), plus the small
//! non-secret state file beside them.
//!
//! Ported (simplified) from Spotifast `src/credentials.rs`
//! (https://github.com/crmne/spotifast).
//! Copyright (c) 2026 Carmine Paolino, MIT License.
//!
//! Every native call and every state file write runs on one dedicated
//! thread, in order: a locked store cannot hold the caller, and a logout
//! queued before a later login's save cannot erase it. A generation counter
//! per slot rejects a save that was started before a logout. The state file
//! lists the slots that should exist, and is cleared first on logout, so a
//! grant whose deletion failed is never restored.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::auth::StoredToken;

const SERVICE: &str = "nikgob-music";
const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Slot {
    /// Legacy: the grant of the shared public Web API app, no longer used
    /// (docs/stage3.md §0, 2026-10-17). Still read from old state files so
    /// that the start can delete it ([`crate::Spotify`]'s restore); never
    /// saved or loaded.
    Shared,
    Personal,
    Playback,
}

impl Slot {
    pub const ALL: [Self; 3] = [Self::Shared, Self::Personal, Self::Playback];

    fn name(self) -> &'static str {
        match self {
            Self::Shared => "shared-web",
            Self::Personal => "personal-web",
            Self::Playback => "playback",
        }
    }
}

/// librespot's reusable credential (`AUTHENTICATION_STORED_SPOTIFY_CREDENTIALS`).
/// No Debug: the blob logs in.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaybackCredential {
    pub username: String,
    #[serde(with = "b64")]
    pub auth_data: Vec<u8>,
}

impl PlaybackCredential {
    pub fn to_librespot(&self) -> librespot_core::authentication::Credentials {
        librespot_core::authentication::Credentials {
            username: Some(self.username.clone()),
            auth_type: librespot_protocol::authentication::AuthenticationType::AUTHENTICATION_STORED_SPOTIFY_CREDENTIALS,
            auth_data: self.auth_data.clone(),
        }
    }
}

mod b64 {
    use super::*;
    pub fn serialize<S: serde::Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(v))
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        STANDARD.decode(text).map_err(serde::de::Error::custom)
    }
}

/// What one slot holds. No Debug: usable secrets.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub enum Secret {
    Web(StoredToken),
    Playback(PlaybackCredential),
}

impl Secret {
    fn valid_for(&self, slot: Slot) -> bool {
        match (slot, self) {
            (Slot::Personal, Self::Web(t)) => {
                !t.client_id.is_empty() && !t.refresh_token.is_empty()
            }
            (Slot::Playback, Self::Playback(c)) => {
                !c.username.is_empty() && !c.auth_data.is_empty()
            }
            _ => false,
        }
    }
}

/// Non-secret facts kept in `spotify/state.json`: which grants exist and
/// what the account looked like, so the UI shows it at once on startup.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub slots: Vec<Slot>,
    #[serde(default)]
    pub personal_client_id: Option<String>,
    #[serde(default)]
    pub profile: Option<Profile>,
    #[serde(default)]
    pub playback_username: Option<String>,
    /// The playback session's account type, when it was last seen.
    #[serde(default)]
    pub session_premium: Option<bool>,
    /// When Spotify last refused audio keys track after track to this
    /// account (unix seconds): its tracks play from YouTube Music for a
    /// while ([`crate::KEYS_RETRY`]).
    #[serde(default)]
    pub keys_refused_at: Option<u64>,
    /// When `profile` was last read from `GET /me` (unix seconds).
    #[serde(default)]
    pub profile_at: Option<u64>,
    /// What each app was found to lack, by Client ID (asked once, not on
    /// every start).
    #[serde(default)]
    pub probes: Vec<(String, crate::api::Probes)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    /// `product` of `GET /me` (`"premium"`, `"free"`); Development Mode apps
    /// no longer get it.
    #[serde(default)]
    pub product: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[cfg_attr(windows, error("хранилище учётных данных Windows недоступно"))]
    #[cfg_attr(not(windows), error("хранилище паролей системы недоступно"))]
    Unavailable,
    #[error("хранилище учётных данных не ответило")]
    Timeout,
    #[error("вход изменился, пока данные сохранялись")]
    Stale,
}

/// The protected backend: Windows Credential Manager, the Secret Service on
/// Linux, or memory in tests.
pub(crate) trait Backend: Send {
    fn read(&mut self, key: &str) -> Result<Option<Vec<u8>>, StoreError>;
    fn write(&mut self, key: &str, secret: &[u8]) -> Result<(), StoreError>;
    fn delete(&mut self, key: &str) -> Result<(), StoreError>;
}

/// Kind of a native error for the log. Provider texts may contain the
/// secret, so they are never logged.
fn log_native(operation: &str, error: &keyring_core::Error) {
    use keyring_core::Error as E;
    let kind = match error {
        E::PlatformFailure(_) => "platform failure",
        E::NoStorageAccess(_) => "no storage access",
        E::NoEntry => "no entry",
        E::BadEncoding(_) => "bad encoding",
        E::TooLong(..) => "too long",
        E::Invalid(..) => "invalid parameter",
        E::Ambiguous(_) => "ambiguous entry",
        _ => "other",
    };
    log::warn!("spotify: credential store {operation} failed: {kind}");
}

#[derive(Default)]
struct Native {
    store: Option<Arc<keyring_core::api::CredentialStore>>,
}

impl Native {
    fn entry(&mut self, key: &str) -> Result<keyring_core::Entry, StoreError> {
        if self.store.is_none() {
            #[cfg(windows)]
            let store = windows_native_keyring_store::Store::new()
                .map(|s| s as Arc<keyring_core::api::CredentialStore>);
            // No Secret Service on the session bus (no keyring daemon, or no
            // session bus at all) fails here, as an unavailable store.
            #[cfg(target_os = "linux")]
            let store = zbus_secret_service_keyring_store::Store::new()
                .map(|s| s as Arc<keyring_core::api::CredentialStore>);
            #[cfg(not(any(windows, target_os = "linux")))]
            let store: keyring_core::Result<Arc<keyring_core::api::CredentialStore>> =
                Err(keyring_core::Error::NoDefaultStore);
            match store {
                Ok(s) => self.store = Some(s),
                Err(e) => {
                    log_native("open", &e);
                    return Err(StoreError::Unavailable);
                }
            }
        }
        // Local persistence survives a restart on every machine; the
        // default (Enterprise) is refused or kept per logon session on some
        // (Spotifast #815).
        let modifiers = std::collections::HashMap::from([("persistence", "Local")]);
        // The Secret Service has no such choice; the label is what a keyring
        // manager (Seahorse, KWallet) lists the item as.
        #[cfg(target_os = "linux")]
        let label = format!("music.nikgob {key}");
        #[cfg(target_os = "linux")]
        let modifiers = {
            drop(modifiers);
            std::collections::HashMap::from([("label", label.as_str())])
        };
        #[cfg(not(any(windows, target_os = "linux")))]
        let modifiers = {
            drop(modifiers);
            std::collections::HashMap::new()
        };
        self.store
            .as_ref()
            .ok_or(StoreError::Unavailable)?
            .build(SERVICE, key, (!modifiers.is_empty()).then_some(&modifiers))
            .map_err(|e| {
                log_native("entry", &e);
                StoreError::Unavailable
            })
    }
}

impl Backend for Native {
    fn read(&mut self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        match self.entry(key)?.get_secret() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => {
                log_native("read", &e);
                Err(StoreError::Unavailable)
            }
        }
    }

    fn write(&mut self, key: &str, secret: &[u8]) -> Result<(), StoreError> {
        self.entry(key)?.set_secret(secret).map_err(|e| {
            log_native("write", &e);
            StoreError::Unavailable
        })
    }

    fn delete(&mut self, key: &str) -> Result<(), StoreError> {
        match self.entry(key)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(e) => {
                log_native("delete", &e);
                Err(StoreError::Unavailable)
            }
        }
    }
}

/// Secrets in memory, for tests.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct Memory(pub std::collections::HashMap<String, Vec<u8>>);

#[cfg(test)]
impl Backend for Memory {
    fn read(&mut self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self.0.get(key).cloned())
    }
    fn write(&mut self, key: &str, secret: &[u8]) -> Result<(), StoreError> {
        self.0.insert(key.into(), secret.to_vec());
        Ok(())
    }
    fn delete(&mut self, key: &str) -> Result<(), StoreError> {
        self.0.remove(key);
        Ok(())
    }
}

type Job = Box<dyn FnOnce(&mut dyn Backend) + Send>;

/// Handle to the store thread. Cheap to clone.
#[derive(Clone)]
pub struct Store {
    jobs: mpsc::Sender<Job>,
    generations: Arc<[AtomicU64; 3]>,
    /// Distinguishes data directories (a test profile from the real one).
    profile: String,
    state_path: PathBuf,
}

impl Store {
    /// The platform's credential store for the app data in `dir`.
    pub fn native(dir: PathBuf) -> Self {
        Self::with_backend(dir, Box::new(Native::default()))
    }

    pub(crate) fn with_backend(dir: PathBuf, mut backend: Box<dyn Backend>) -> Self {
        let profile = {
            let hash = Sha256::digest(dir.to_string_lossy().as_bytes());
            hash[..4].iter().map(|b| format!("{b:02x}")).collect()
        };
        let (jobs, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("spotify-credentials".into())
            .spawn(move || {
                for job in rx {
                    job(backend.as_mut());
                }
            })
            .expect("spawn the credential thread");
        Self {
            jobs,
            generations: Arc::new([AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)]),
            profile,
            state_path: dir.join("state.json"),
        }
    }

    fn key(&self, slot: Slot) -> String {
        format!("spotify-{}-{}", self.profile, slot.name())
    }

    /// The current generation of a slot; a save carries it.
    pub fn generation(&self, slot: Slot) -> u64 {
        self.generations[slot as usize].load(Ordering::SeqCst)
    }

    async fn run<T: Send + 'static>(
        &self,
        job: impl FnOnce(&mut dyn Backend) -> T + Send + 'static,
    ) -> Result<T, StoreError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.jobs
            .send(Box::new(move |b| {
                let _ = tx.send(job(b));
            }))
            .map_err(|_| StoreError::Unavailable)?;
        match tokio::time::timeout(TIMEOUT, rx).await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(_)) => Err(StoreError::Unavailable),
            Err(_) => Err(StoreError::Timeout),
        }
    }

    /// The secret of a slot; `None` when missing or not valid for the slot.
    pub async fn load(&self, slot: Slot) -> Result<Option<Secret>, StoreError> {
        let key = self.key(slot);
        let bytes = self.run(move |b| b.read(&key)).await??;
        Ok(bytes
            .and_then(|b| serde_json::from_slice::<Secret>(&b).ok())
            .filter(|s| s.valid_for(slot)))
    }

    /// Saves a secret unless the slot was cleared since `generation`.
    pub async fn save(
        &self,
        slot: Slot,
        generation: u64,
        secret: Secret,
    ) -> Result<(), StoreError> {
        if !secret.valid_for(slot) {
            return Err(StoreError::Unavailable);
        }
        let key = self.key(slot);
        let bytes = serde_json::to_vec(&secret).map_err(|_| StoreError::Unavailable)?;
        let generations = self.generations.clone();
        self.run(move |b| {
            if generations[slot as usize].load(Ordering::SeqCst) != generation {
                return Err(StoreError::Stale);
            }
            b.write(&key, &bytes)
        })
        .await?
    }

    /// Forgets a slot: later saves of the old generation are dropped. Does
    /// not wait for the deletion.
    pub fn clear(&self, slot: Slot) {
        self.generations[slot as usize].fetch_add(1, Ordering::SeqCst);
        let key = self.key(slot);
        let _ = self.jobs.send(Box::new(move |b| {
            let _ = b.delete(&key);
        }));
    }

    /// What a slot holds as stored, unchecked (tests of the migration).
    #[cfg(test)]
    pub(crate) async fn raw(&self, slot: Slot) -> Option<Vec<u8>> {
        let key = self.key(slot);
        self.run(move |b| b.read(&key)).await.ok()?.ok()?
    }

    /// Writes a slot as stored, unchecked (an install from before).
    #[cfg(test)]
    pub(crate) async fn put_raw(&self, slot: Slot, bytes: Vec<u8>) {
        let key = self.key(slot);
        self.run(move |b| b.write(&key, &bytes)).await.unwrap().unwrap();
    }

    pub async fn load_state(&self) -> State {
        let path = self.state_path.clone();
        self.run(move |_| {
            std::fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default()
    }

    /// Queues a write of the state file (no waiting).
    pub fn save_state(&self, state: &State) {
        let path = self.state_path.clone();
        let Ok(bytes) = serde_json::to_vec_pretty(state) else {
            return;
        };
        let _ = self.jobs.send(Box::new(move |_| {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, &bytes).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn web(client: &str) -> Secret {
        Secret::Web(StoredToken {
            client_id: client.into(),
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: 1,
            scope: String::new(),
        })
    }

    #[tokio::test]
    async fn secrets_round_trip_and_logout_wins_over_a_late_save() {
        let dir = std::env::temp_dir().join(format!("nikgob-spotify-cred-{}", std::process::id()));
        let store = Store::with_backend(dir.clone(), Box::new(Memory::default()));
        let g = store.generation(Slot::Personal);
        store
            .save(Slot::Personal, g, web("test-client-id"))
            .await
            .unwrap();
        assert!(store.load(Slot::Personal).await.unwrap().is_some());
        // The legacy shared slot takes nothing any more.
        let gs = store.generation(Slot::Shared);
        assert!(store.save(Slot::Shared, gs, web("test-client-id")).await.is_err());
        assert!(store.save(Slot::Playback, gs, web("test-client-id")).await.is_err());

        let playback = Secret::Playback(PlaybackCredential {
            username: "user".into(),
            auth_data: vec![1, 2, 3],
        });
        let gp = store.generation(Slot::Playback);
        store
            .save(Slot::Playback, gp, playback.clone())
            .await
            .unwrap();
        assert!(store.load(Slot::Playback).await.unwrap() == Some(playback.clone()));

        store.clear(Slot::Playback);
        assert!(store.load(Slot::Playback).await.unwrap().is_none());
        assert_eq!(
            store.save(Slot::Playback, gp, playback).await,
            Err(StoreError::Stale)
        );

        let state = State {
            slots: vec![Slot::Personal],
            profile: Some(Profile {
                id: "u".into(),
                name: "U".into(),
                product: None,
            }),
            ..State::default()
        };
        store.save_state(&state);
        assert_eq!(store.load_state().await, state);
        let _ = std::fs::remove_dir_all(dir);
    }
}
