use std::fmt;

use serde::{Deserialize, Serialize};

/// Settings the app fills in from its configuration. Nothing here is hardcoded.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MetadataConfig {
    /// Contact (e-mail or URL) put into the `User-Agent`; MusicBrainz asks for it
    /// and throttles anonymous clients.
    pub contact: String,
    /// AcoustID application key. Without it fingerprint lookups are unavailable.
    pub acoustid_client_key: Option<String>,
    /// The user's Last.fm API key. Without it Last.fm is not asked.
    pub lastfm_key: Option<String>,
}

impl MetadataConfig {
    pub fn new(contact: impl Into<String>) -> Self {
        Self {
            contact: contact.into(),
            acoustid_client_key: None,
            lastfm_key: None,
        }
    }

    #[must_use]
    pub fn with_lastfm_key(mut self, key: impl Into<String>) -> Self {
        self.lastfm_key = Some(key.into()).filter(|k: &String| !k.trim().is_empty());
        self
    }

    #[must_use]
    pub fn with_acoustid_key(mut self, key: impl Into<String>) -> Self {
        self.acoustid_client_key = Some(key.into());
        self
    }

    /// `music.nikgob/<version> ( <contact> )`, or just the product when no contact is set.
    pub fn user_agent(&self) -> String {
        let product = concat!("music.nikgob/", env!("CARGO_PKG_VERSION"));
        let contact: String = self
            .contact
            .chars()
            .filter(|c| !c.is_control())
            .collect::<String>()
            .trim()
            .to_owned();
        if contact.is_empty() {
            product.to_owned()
        } else {
            format!("{product} ( {contact} )")
        }
    }

    pub(crate) fn has_contact(&self) -> bool {
        !self.contact.trim().is_empty()
    }

    pub(crate) fn acoustid_key(&self) -> Option<&str> {
        self.acoustid_client_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
    }
}

// The key is a secret: never print it.
impl fmt::Debug for MetadataConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MetadataConfig")
            .field("contact", &self.contact)
            .field(
                "acoustid_client_key",
                &self.acoustid_client_key.as_ref().map(|_| "<redacted>"),
            )
            .field("lastfm_key", &self.lastfm_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}
