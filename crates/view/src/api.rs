//! The local REST API as the settings show it («Межсеть → API и MCP»):
//! its switch, who may reach it, its port, the tokens, and whether the
//! server really listens.

pub use nikgob_core::settings::{API_PORT, ApiAccess, ApiActor, ApiPreset};

/// `[api]` of settings.toml as the runtime keeps it, and what the server
/// reported.
#[derive(Clone, Debug, PartialEq)]
pub struct ApiView {
    pub enabled: bool,
    pub access: ApiAccess,
    /// The address set in settings.toml (`192.168.0.157`); empty: by
    /// [`Self::access`].
    pub address: String,
    pub port: u16,
    pub tokens: Vec<ApiTokenView>,
    /// The server listens on this `ip:port` (it said so).
    pub listening: Option<String>,
    /// Why it does not listen while on (a port taken, an address the
    /// computer does not have), in words.
    pub error: Option<String>,
    /// The settings can be saved (there is a settings.toml).
    pub configurable: bool,
    /// The last [`crate::Command::ApiRun`] the runtime got through: the
    /// API waits for its own to answer with the state its commands made.
    pub synced: u64,
    /// What the last few [`crate::Command::ApiRun`]s could not do, by their
    /// `ack` (a service track no longer known, a file outside the library):
    /// the API answers an error instead of «done».
    pub skipped: Vec<(u64, Vec<String>)>,
    /// A token the dialog sent that was not kept (its id, why): the dialog
    /// shows the error instead of the secret.
    pub token_failed: Option<(String, String)>,
}

impl Default for ApiView {
    fn default() -> Self {
        Self {
            enabled: false,
            access: ApiAccess::Local,
            address: String::new(),
            port: API_PORT,
            tokens: Vec::new(),
            listening: None,
            error: None,
            configurable: false,
            synced: 0,
            skipped: Vec::new(),
            token_failed: None,
        }
    }
}

impl ApiView {
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

    /// Only this computer can reach it.
    pub fn local_only(&self) -> bool {
        let ip = self.bind_ip();
        ip == "127.0.0.1" || ip == "::1" || ip.eq_ignore_ascii_case("localhost")
    }

    /// The tokens that work (not revoked), oldest first.
    pub fn live_tokens(&self) -> impl Iterator<Item = &ApiTokenView> {
        self.tokens.iter().filter(|t| !t.revoked)
    }
}

/// One token as the settings list it. The hash is there for the API
/// server; the secret is nowhere.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiTokenView {
    pub id: String,
    pub name: String,
    pub preset: ApiPreset,
    pub actor: ApiActor,
    pub created_ms: i64,
    pub last_used_ms: Option<i64>,
    /// Revoked, its «Отменить» still up: every request with it is refused.
    pub revoked: bool,
    /// Hex BLAKE3 of the secret.
    pub hash: String,
}

impl std::fmt::Debug for ApiTokenView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiTokenView")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("preset", &self.preset)
            .field("actor", &self.actor)
            .field("created_ms", &self.created_ms)
            .field("last_used_ms", &self.last_used_ms)
            .field("revoked", &self.revoked)
            .finish_non_exhaustive()
    }
}

/// A token the settings made: the secret stays with the dialog that shows
/// it once, the runtime keeps this.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiNewToken {
    pub id: String,
    pub name: String,
    pub preset: ApiPreset,
    pub actor: ApiActor,
    /// Hex BLAKE3 of the secret ([`nikgob_core::api_token::hash`]).
    pub hash: String,
}

impl std::fmt::Debug for ApiNewToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiNewToken")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("preset", &self.preset)
            .field("actor", &self.actor)
            .finish_non_exhaustive()
    }
}

impl ApiNewToken {
    /// A new token named `name`: its secret (shown once) and what the
    /// runtime keeps. `None` when the system gives no randomness.
    pub fn make(name: &str, preset: ApiPreset, actor: ApiActor) -> Option<(String, Self)> {
        let secret = nikgob_core::api_token::generate()?;
        let id = nikgob_core::api_token::new_id()?;
        let hash = nikgob_core::api_token::hash(&secret);
        Some((secret, Self { id, name: name.trim().to_owned(), preset, actor, hash }))
    }
}

/// «Управление», «Полный», «Только чтение».
pub fn preset_label(p: ApiPreset) -> &'static str {
    match p {
        ApiPreset::Control => "Управление",
        ApiPreset::Full => "Полный",
        ApiPreset::Read => "Только чтение",
    }
}

/// «Я», «Ассистент», «Агент», «Гость».
pub fn actor_label(a: ApiActor) -> &'static str {
    match a {
        ApiActor::Owner => "Я",
        ApiActor::Assistant => "Ассистент",
        ApiActor::Agent => "Агент",
        ApiActor::Guest => "Гость",
    }
}

/// Where the runtime answers an API request that waits for work of its
/// own (a lookup on the services, a read of files): called once, from
/// whatever thread the work ends on, with JSON or the error in words.
#[derive(Clone)]
pub struct ApiReply(std::sync::Arc<dyn Fn(Result<serde_json::Value, String>) + Send + Sync>);

impl ApiReply {
    pub fn new(f: impl Fn(Result<serde_json::Value, String>) + Send + Sync + 'static) -> Self {
        Self(std::sync::Arc::new(f))
    }

    pub fn send(&self, answer: Result<serde_json::Value, String>) {
        (self.0)(answer);
    }
}

impl std::fmt::Debug for ApiReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApiReply")
    }
}

impl PartialEq for ApiReply {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

/// Tags an API request changes on a song: a field left `None` keeps what
/// the file has.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TagPatch {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub year: Option<String>,
    pub track_no: Option<String>,
    pub disc_no: Option<String>,
    pub genre: Option<String>,
}

impl TagPatch {
    /// Nothing to change.
    pub fn is_empty(&self) -> bool {
        self.fields().iter().all(|(_, v)| v.is_none())
    }

    /// Each field by its name, in the order the API lists them.
    pub fn fields(&self) -> [(&'static str, Option<&str>); 8] {
        [
            ("title", self.title.as_deref()),
            ("artist", self.artist.as_deref()),
            ("album", self.album.as_deref()),
            ("album_artist", self.album_artist.as_deref()),
            ("year", self.year.as_deref()),
            ("track_no", self.track_no.as_deref()),
            ("disc_no", self.disc_no.as_deref()),
            ("genre", self.genre.as_deref()),
        ]
    }

    /// `now` with these changes: what the file is to have.
    pub fn apply(&self, now: &crate::TagEdit) -> crate::TagEdit {
        let pick = |new: &Option<String>, old: &String| new.as_ref().map_or_else(|| old.clone(), |v| v.trim().to_owned());
        crate::TagEdit {
            title: pick(&self.title, &now.title),
            artist: pick(&self.artist, &now.artist),
            album: pick(&self.album, &now.album),
            album_artist: pick(&self.album_artist, &now.album_artist),
            year: pick(&self.year, &now.year),
            track_no: pick(&self.track_no, &now.track_no),
            disc_no: pick(&self.disc_no, &now.disc_no),
            genre: pick(&self.genre, &now.genre),
            recording_mbid: now.recording_mbid.clone(),
            release_mbid: now.release_mbid.clone(),
        }
    }
}

/// The fields of a [`crate::TagEdit`] by the names [`TagPatch::fields`]
/// uses.
pub fn tag_fields(e: &crate::TagEdit) -> [(&'static str, &str); 8] {
    [
        ("title", e.title.as_str()),
        ("artist", e.artist.as_str()),
        ("album", e.album.as_str()),
        ("album_artist", e.album_artist.as_str()),
        ("year", e.year.as_str()),
        ("track_no", e.track_no.as_str()),
        ("disc_no", e.disc_no.as_str()),
        ("genre", e.genre.as_str()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_token_keeps_only_the_hash() {
        let (secret, t) = ApiNewToken::make("  Claude ", ApiPreset::Full, ApiActor::Agent).expect("randomness");
        assert_eq!(t.name, "Claude");
        assert!(nikgob_core::api_token::matches(&secret, &t.hash));
        let dbg = format!("{t:?}");
        assert!(!dbg.contains(&t.hash) && !dbg.contains(&secret), "{dbg}");
    }

    #[test]
    fn where_it_listens() {
        let mut v = ApiView::default();
        assert_eq!(v.port, 47210);
        assert!(v.local_only());
        v.access = ApiAccess::Network;
        assert_eq!(v.bind_ip(), "0.0.0.0");
        assert!(!v.local_only());
        v.address = "192.168.0.157".into();
        assert_eq!(v.bind_ip(), "192.168.0.157");
        v.address = "127.0.0.1".into();
        assert!(v.local_only());
    }

    #[test]
    fn a_tag_patch_keeps_what_it_does_not_name() {
        let now = crate::TagEdit { title: "Old".into(), artist: "A".into(), genre: "Rock".into(), recording_mbid: "m".into(), ..Default::default() };
        let p = TagPatch { title: Some(" New ".into()), ..Default::default() };
        assert!(!p.is_empty() && TagPatch::default().is_empty());
        let after = p.apply(&now);
        assert_eq!((after.title.as_str(), after.artist.as_str(), after.genre.as_str(), after.recording_mbid.as_str()), ("New", "A", "Rock", "m"));
        assert_eq!(tag_fields(&after)[0], ("title", "New"));
    }

    #[test]
    fn a_reply_is_called_on_each_send() {
        let n = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let m = n.clone();
        let r = ApiReply::new(move |a| {
            assert!(a.is_ok());
            m.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        let same = r.clone();
        same.send(Ok(serde_json::json!({})));
        assert_eq!(n.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(r, same);
        assert_ne!(r, ApiReply::new(|_| {}));
    }
}
