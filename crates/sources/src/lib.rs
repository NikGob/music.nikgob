//! Download sources behind [`nikgob_core::source::Source`]: torrents through the indexer ([`torrent`]),
//! khinsider ([`khinsider`]), the free FLAC services ([`qobuz`],
//! [`tidal`]), Subsonic servers ([`subsonic`]: [`squidify`], [`dogmazic`]),
//! the Internet Archive ([`archive`]), Bandcamp's free releases
//! ([`bandcamp`]), SoundCloud's originals ([`soundcloud`]); the query
//! variants ([`text`]), whether an offer is the wished thing ([`matching`]),
//! what a file is from its bytes ([`sniff`]) and the one quality policy
//! over all of them ([`select`]). Soulseek (crates/soulseek) is one more implementation of
//! the same trait.

pub mod album;
pub mod archive;
pub mod bandcamp;
pub mod config;
pub mod dogmazic;
pub mod html;
pub mod http;
pub mod instances;
pub mod khinsider;
pub mod matching;
pub mod qobuz;
pub mod select;
pub mod sniff;
pub mod soundcloud;
pub mod squidify;
pub mod subsonic;
pub mod text;
pub mod tidal;
pub mod torrent;

use std::sync::Arc;

use nikgob_core::settings::Settings;
use nikgob_core::source::Source;

/// The web sources, with handles for the ones whose list or browser can
/// change while the app runs.
pub struct WebSources {
    pub list: Vec<Arc<dyn Source>>,
    pub khinsider: Option<Arc<khinsider::Khinsider>>,
    pub qobuz: Option<Arc<qobuz::Qobuz>>,
    pub tidal: Option<Arc<tidal::Tidal>>,
}

impl WebSources {
    pub fn empty() -> Self {
        Self { list: Vec::new(), khinsider: None, qobuz: None, tidal: None }
    }
}

/// The sources that need no setup (khinsider, the free services, the
/// Subsonic server, Dogmazic). Qobuz and Tidal use `lists` (from
/// `sources.toml`, merged with the built-in addresses). SoundCloud's
/// originals come with the SoundCloud service
/// ([`soundcloud::SoundcloudOriginals`]). Whether each is used is the
/// user's switch on «Источники».
pub fn web_sources(settings: &Settings, lists: &config::Lists) -> WebSources {
    let s = &settings.sources;
    let kh = Arc::new(khinsider::Khinsider::new(&s.khinsider_url));
    let q = Arc::new(qobuz::Qobuz::from_list(&lists.qobuz, lists.qobuz_own, std::time::Duration::from_millis(1500)));
    let t = Arc::new(tidal::Tidal::from_list(&lists.tidal, lists.tidal_own, std::time::Duration::from_millis(1500)));
    let list: Vec<Arc<dyn Source>> = vec![
        kh.clone(),
        q.clone(),
        t.clone(),
        Arc::new(squidify::Squidify::from_settings(s)),
        Arc::new(archive::Archive::new(&s.archive_url)),
        Arc::new(bandcamp::Bandcamp::new()),
        Arc::new(dogmazic::Dogmazic::from_settings(s)),
    ];
    WebSources { list, khinsider: Some(kh), qobuz: Some(q), tidal: Some(t) }
}
