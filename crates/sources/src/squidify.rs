//! squidify.org as a [`Source`]: a public Navidrome of anime, game and
//! film soundtracks, in FLAC, through the Subsonic client
//! ([`crate::subsonic`]); or any other Subsonic server the user names in
//! settings.toml (`[sources] subsonic_url`, `subsonic_user`,
//! `subsonic_password`), their own Navidrome too. An empty login is the
//! guest squidify.org publishes: it may stream but not download
//! (`downloadRole` false, «downloads are disabled», checked 2026-11-09), so
//! files come through `stream?format=raw`, as they are.

use std::path::Path;
use std::time::Duration;

use nikgob_core::indexer::BoxFuture;
use nikgob_core::settings::SourcesSettings;
use nikgob_core::source::{Fetched, Offer, Progress, Source, SourceError, SourceKey, Want};

use crate::subsonic::{Server, Subsonic};
pub use crate::subsonic::{ApiError, token};

/// squidify.org, as its guide gives it.
pub const DEFAULT_URL: &str = "https://www.squidify.org";
/// The guest login squidify.org publishes (name and password alike).
pub const GUEST: &str = "Guest";

const SERVER: Server = Server { key: SourceKey::Squidify, default_url: DEFAULT_URL, guest: Some(GUEST) };

pub struct Squidify(Subsonic);

impl Squidify {
    /// From `[sources]` of settings.toml: empty address = squidify.org,
    /// empty login = its guest.
    pub fn from_settings(s: &SourcesSettings) -> Self {
        Self::new(&s.subsonic_url, &s.subsonic_user, &s.subsonic_password)
    }

    pub fn new(url: &str, user: &str, password: &str) -> Self {
        Self::with_pace(url, user, password, Duration::from_millis(1500))
    }

    /// With another pace (tests).
    pub fn with_pace(url: &str, user: &str, password: &str, every: Duration) -> Self {
        Self(Subsonic::new(SERVER, url, user, password, every))
    }
}

impl Source for Squidify {
    fn key(&self) -> SourceKey {
        self.0.key()
    }

    fn name(&self) -> &str {
        self.0.name()
    }

    fn search<'a>(&'a self, want: &'a Want) -> BoxFuture<'a, Result<Vec<Offer>, SourceError>> {
        self.0.search(want)
    }

    fn fetch<'a>(&'a self, offer: &'a Offer, want: &'a Want, dest: &'a Path, progress: Progress) -> BoxFuture<'a, Result<Fetched, SourceError>> {
        self.0.fetch(offer, want, dest, progress)
    }
}
