//! Download sources behind [`nikgob_core::source::Source`]
//! (docs/stage4.5.md): torrents through the indexer ([`torrent`]),
//! khinsider ([`khinsider`]), the free FLAC services ([`qobuz`],
//! [`tidal`]); the query variants ([`text`]), whether an offer is the
//! wished thing ([`matching`]) and the one quality policy over all of them
//! ([`select`]). Soulseek (crates/soulseek) is one more implementation of
//! the same trait.

pub mod html;
pub mod http;
pub mod instances;
pub mod khinsider;
pub mod matching;
pub mod qobuz;
pub mod select;
pub mod text;
pub mod tidal;
pub mod torrent;

use std::sync::Arc;

use nikgob_core::settings::Settings;
use nikgob_core::source::Source;

/// The sources that need no setup (khinsider and the free services), at
/// the addresses settings.toml gives or the built-in ones. Whether each
/// is used is the user's switch on «Источники».
pub fn web_sources(settings: &Settings) -> Vec<Arc<dyn Source>> {
    let s = &settings.sources;
    vec![
        Arc::new(khinsider::Khinsider::new(&s.khinsider_url)),
        Arc::new(qobuz::Qobuz::new(&s.qobuz_instances)),
        Arc::new(tidal::Tidal::new(&s.tidal_instances)),
    ]
}
