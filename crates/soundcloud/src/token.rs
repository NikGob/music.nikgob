//! The user's SoundCloud sign-in: the `oauth_token` cookie of
//! `.soundcloud.com`, sent as `Authorization: OAuth <token>`, and where it
//! is kept (the platform credential store: Windows Credential Manager, the
//! Secret Service on Linux; never settings.toml or a log).
//!
//! The token comes from the app's sign-in window (a Netscape cookies.txt)
//! or is pasted: cookies.txt, a `Cookie:` header, `OAuth <token>` or the
//! bare token all work ([`parse`]).
//!
//! Every native call runs on one thread of its own, in order, so a locked
//! store never holds the caller and a logout queued after a save always
//! wins.

use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

const SERVICE: &str = "nikgob-music";
/// The cookie that holds the token.
pub const COOKIE: &str = "oauth_token";
const TIMEOUT: Duration = Duration::from_secs(20);

/// The token in a pasted or windowed text, if there is one.
pub fn parse(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    // cookies.txt: 7 tab-separated fields, the name 6th, the value 7th.
    for line in text.lines() {
        let line = line.trim_start_matches("#HttpOnly_");
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() == 7 && f[5].trim() == COOKIE && f[0].to_ascii_lowercase().contains("soundcloud.com") {
            return clean(f[6]);
        }
    }
    // `Cookie: a=b; oauth_token=…` or a bare `a=b; …` list.
    let header = text.strip_prefix("Cookie:").or_else(|| text.strip_prefix("cookie:")).unwrap_or(text);
    if header.contains('=') {
        return header
            .split(';')
            .filter_map(|kv| kv.split_once('='))
            .find(|(k, _)| k.trim() == COOKIE)
            .and_then(|(_, v)| clean(v));
    }
    // `Authorization: OAuth …`, `OAuth …` or the token alone.
    let bare = text
        .strip_prefix("Authorization:")
        .map_or(text, str::trim)
        .trim_start_matches("OAuth ")
        .trim();
    clean(bare)
}

/// A token is one word of visible ASCII.
fn clean(v: &str) -> Option<String> {
    let v = v.trim().trim_matches('"');
    (!v.is_empty() && v.len() < 512 && v.bytes().all(|b| b.is_ascii_graphic())).then(|| v.to_owned())
}

/// A small non-cryptographic hash, so two data folders (a test profile,
/// the real one) keep separate tokens.
fn profile(dir: &Path) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in dir.to_string_lossy().bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{:08x}", h as u32)
}

/// The protected backend.
trait Backend: Send {
    fn read(&mut self, key: &str) -> Result<Option<String>, ()>;
    fn write(&mut self, key: &str, secret: &str) -> Result<(), ()>;
    fn delete(&mut self, key: &str);
}

/// What kind of native failure, for the log (provider texts may hold the
/// secret, so they are never logged).
fn log_native(op: &str, e: &keyring_core::Error) {
    use keyring_core::Error as E;
    let kind = match e {
        E::PlatformFailure(_) => "platform failure",
        E::NoStorageAccess(_) => "no storage access",
        E::NoEntry => "no entry",
        E::BadEncoding(_) => "bad encoding",
        E::TooLong(..) => "too long",
        E::Invalid(..) => "invalid parameter",
        E::Ambiguous(_) => "ambiguous entry",
        _ => "other",
    };
    log::warn!("soundcloud: credential store {op} failed: {kind}");
}

#[derive(Default)]
struct Native {
    store: Option<Arc<keyring_core::api::CredentialStore>>,
}

impl Native {
    fn entry(&mut self, key: &str) -> Result<keyring_core::Entry, ()> {
        if self.store.is_none() {
            #[cfg(windows)]
            let store = windows_native_keyring_store::Store::new().map(|s| s as Arc<keyring_core::api::CredentialStore>);
            #[cfg(target_os = "linux")]
            let store =
                zbus_secret_service_keyring_store::Store::new().map(|s| s as Arc<keyring_core::api::CredentialStore>);
            #[cfg(not(any(windows, target_os = "linux")))]
            let store: keyring_core::Result<Arc<keyring_core::api::CredentialStore>> =
                Err(keyring_core::Error::NoDefaultStore);
            match store {
                Ok(s) => self.store = Some(s),
                Err(e) => {
                    log_native("open", &e);
                    return Err(());
                }
            }
        }
        // As the Spotify grants: local persistence on Windows, a label the
        // keyring managers list on Linux.
        #[cfg(windows)]
        let modifiers = std::collections::HashMap::from([("persistence", "Local")]);
        #[cfg(target_os = "linux")]
        let label = format!("music.nikgob {key}");
        #[cfg(target_os = "linux")]
        let modifiers = std::collections::HashMap::from([("label", label.as_str())]);
        #[cfg(not(any(windows, target_os = "linux")))]
        let modifiers: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
        self.store
            .as_ref()
            .ok_or(())?
            .build(SERVICE, key, (!modifiers.is_empty()).then_some(&modifiers))
            .map_err(|e| log_native("entry", &e))
    }
}

impl Backend for Native {
    fn read(&mut self, key: &str) -> Result<Option<String>, ()> {
        match self.entry(key)?.get_secret() {
            Ok(s) => Ok(String::from_utf8(s).ok()),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => {
                log_native("read", &e);
                Err(())
            }
        }
    }

    fn write(&mut self, key: &str, secret: &str) -> Result<(), ()> {
        self.entry(key)?.set_secret(secret.as_bytes()).map_err(|e| log_native("write", &e))
    }

    fn delete(&mut self, key: &str) {
        if let Ok(entry) = self.entry(key) {
            match entry.delete_credential() {
                Ok(()) | Err(keyring_core::Error::NoEntry) => {}
                Err(e) => log_native("delete", &e),
            }
        }
    }
}

/// Secrets in memory (tests, and a store that cannot open).
#[derive(Default)]
struct Memory(std::collections::HashMap<String, String>);

impl Backend for Memory {
    fn read(&mut self, key: &str) -> Result<Option<String>, ()> {
        Ok(self.0.get(key).cloned())
    }
    fn write(&mut self, key: &str, secret: &str) -> Result<(), ()> {
        self.0.insert(key.into(), secret.into());
        Ok(())
    }
    fn delete(&mut self, key: &str) {
        self.0.remove(key);
    }
}

type Job = Box<dyn FnOnce(&mut dyn Backend) + Send>;

/// Handle to the store thread. Cheap to clone.
#[derive(Clone)]
pub struct Store {
    jobs: Arc<Mutex<mpsc::Sender<Job>>>,
    key: String,
}

impl Store {
    /// The platform's store, for the app data in `dir`.
    pub fn native(dir: &Path) -> Self {
        Self::with_backend(dir, Box::new(Native::default()))
    }

    /// Secrets kept in memory only.
    pub fn memory(dir: &Path) -> Self {
        Self::with_backend(dir, Box::new(Memory::default()))
    }

    fn with_backend(dir: &Path, mut backend: Box<dyn Backend>) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        let spawned = std::thread::Builder::new().name("soundcloud-credentials".into()).spawn(move || {
            for job in rx {
                job(backend.as_mut());
            }
        });
        if let Err(e) = spawned {
            log::error!("soundcloud: credential thread: {e}");
        }
        Self {
            jobs: Arc::new(Mutex::new(tx)),
            key: format!("soundcloud-{}-oauth", profile(dir)),
        }
    }

    fn send(&self, job: Job) -> bool {
        self.jobs.lock().unwrap_or_else(PoisonError::into_inner).send(job).is_ok()
    }

    /// The saved token, if any.
    pub async fn load(&self) -> Option<String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let key = self.key.clone();
        if !self.send(Box::new(move |b| {
            let _ = tx.send(b.read(&key).ok().flatten());
        })) {
            return None;
        }
        tokio::time::timeout(TIMEOUT, rx).await.ok()?.ok()?.and_then(|t| clean(&t))
    }

    /// Saves the token (does not wait).
    pub fn save(&self, token: &str) {
        let (key, token) = (self.key.clone(), token.to_owned());
        self.send(Box::new(move |b| {
            if b.write(&key, &token).is_err() {
                log::warn!("soundcloud: the sign-in is not kept for the next start");
            }
        }));
    }

    /// Forgets the token (does not wait).
    pub fn clear(&self) {
        let key = self.key.clone();
        self.send(Box::new(move |b| b.delete(&key)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_way_of_pasting_gives_the_token() {
        let t = "2-312345-987654321-AbCdEfGhIjKl";
        let jar = format!(
            "# Netscape HTTP Cookie File\n.soundcloud.com\tTRUE\t/\tTRUE\t1900000000\tsc_anonymous_id\tx\n#HttpOnly_.soundcloud.com\tTRUE\t/\tTRUE\t1900000000\toauth_token\t{t}\n"
        );
        assert_eq!(parse(&jar).as_deref(), Some(t));
        assert_eq!(parse(&format!("Cookie: a=1; oauth_token={t}; b=2")).as_deref(), Some(t));
        assert_eq!(parse(&format!("a=1; oauth_token={t}")).as_deref(), Some(t));
        assert_eq!(parse(&format!("OAuth {t}")).as_deref(), Some(t));
        assert_eq!(parse(&format!("Authorization: OAuth {t}")).as_deref(), Some(t));
        assert_eq!(parse(&format!("  {t}\n")).as_deref(), Some(t));
        assert_eq!(parse(""), None);
        assert_eq!(parse("a=1; b=2"), None);
        assert_eq!(parse("two words"), None);
    }

    #[test]
    fn profiles_differ_by_folder() {
        assert_ne!(profile(Path::new("/a")), profile(Path::new("/b")));
        assert_eq!(profile(Path::new("/a")).len(), 8);
    }

    #[tokio::test]
    async fn memory_store_round_trip() {
        let s = Store::memory(Path::new("/x"));
        assert_eq!(s.load().await, None);
        s.save("2-1-2-abc");
        assert_eq!(s.load().await.as_deref(), Some("2-1-2-abc"));
        s.clear();
        assert_eq!(s.load().await, None);
    }
}
