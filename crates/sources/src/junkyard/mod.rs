//! «Барахолка» (`SourceKey::Junkyard`): the grey ripper sites of the
//! Lucida kind, one source over several backends asked in turn. A backend
//! behind a captcha or a Cloudflare check is skipped for a while, never
//! passed.

use std::path::Path;

use nikgob_core::indexer::BoxFuture;
use nikgob_core::source::{Fetched, Offer, Progress, Source, SourceError, SourceKey, Want};

pub struct Junkyard {}

impl Junkyard {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for Junkyard {
    fn default() -> Self {
        Self::new()
    }
}

impl Source for Junkyard {
    fn key(&self) -> SourceKey {
        SourceKey::Junkyard
    }

    fn name(&self) -> &str {
        SourceKey::Junkyard.display_name()
    }

    fn search<'a>(&'a self, _want: &'a Want) -> BoxFuture<'a, Result<Vec<Offer>, SourceError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn fetch<'a>(&'a self, _offer: &'a Offer, _want: &'a Want, _dest: &'a Path, _progress: Progress) -> BoxFuture<'a, Result<Fetched, SourceError>> {
        Box::pin(async { Err(SourceError::NothingToFetch) })
    }
}
