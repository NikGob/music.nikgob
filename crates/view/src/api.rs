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
}
