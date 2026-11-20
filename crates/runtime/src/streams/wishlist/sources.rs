use super::*;
use super::retry::{down_text_in, fetch_down_text_in};

impl Worker {
    pub(crate) fn want_of(&self, item: &WishlistItem) -> Want {
        let track = item.track_id.and_then(|t| self.lib.get_track(t).ok().flatten());
        let release = item
            .release_id
            .or(track.as_ref().and_then(|t| t.release_id))
            .and_then(|r| self.lib.get_release(r).ok().flatten());
        // The credited names, the album's artist first (a soundtrack's
        // songs are credited to their singers).
        let mut artists: Vec<String> = Vec::new();
        let credits = release
            .iter()
            .flat_map(|r| r.artist_credit.0.iter())
            .chain(track.iter().flat_map(|t| t.artist_credit.0.iter()));
        let mut main = None;
        for part in credits {
            let name = part.name.trim();
            if name.is_empty() || artists.iter().any(|a| a == name) {
                continue;
            }
            main = main.or(part.artist_id);
            artists.push(name.to_owned());
        }
        if artists.is_empty()
            && let Some(a) = item.artist.as_deref().filter(|a| !a.trim().is_empty())
        {
            artists.push(a.trim().to_owned());
        }
        // Other spellings of the main artist: its own name and its aliases.
        let mut aliases: Vec<String> = Vec::new();
        if let Some(id) = main {
            if let Some(a) = self.lib.get_artist(id).ok().flatten()
                && Some(&a.name) != artists.first()
            {
                aliases.push(a.name);
            }
            for (who, name) in self.lib.artist_aliases().unwrap_or_default() {
                if who == id && !aliases.contains(&name) && Some(&name) != artists.first() {
                    aliases.push(name);
                }
            }
        }
        Want {
            artists,
            aliases,
            album: release.as_ref().map(|r| r.title.clone()),
            title: match item.scope {
                WishlistScope::Track => track.as_ref().map(|t| t.title.clone()).or_else(|| item.title.clone()),
                WishlistScope::Album => None,
            },
            scope: match item.scope {
                WishlistScope::Track => WantScope::Track,
                WishlistScope::Album => WantScope::Album,
            },
            track_no: track.as_ref().map_or(0, |t| t.track_no),
            disc_no: track.as_ref().map_or(1, |t| t.disc_no),
            duration_ms: track.as_ref().and_then(|t| t.duration_ms),
            isrc: track.as_ref().and_then(|t| t.isrc.clone()).or_else(|| item.isrc.clone()),
            album_tracks: release.as_ref().and_then(|r| r.total_tracks).unwrap_or(0),
            // What of the album came already (a part of it from a source
            // that then turned us away), and the songs the user deleted
            // from it: a fetch leaves them out.
            got: match (item.scope, &release) {
                // A better copy of an album: what is good enough already.
                (WishlistScope::Album, Some(r)) if item.origin == WishlistOrigin::Upgrade => {
                    let bar = self.state.read().prefs.upgrade_below.score();
                    self.lib
                        .tracks_of_release(r.id)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|t| {
                            self.lib
                                .active_source(t.id)
                                .ok()
                                .flatten()
                                .is_some_and(|s| s.kind.is_local() && s.quality_score >= bar)
                        })
                        .map(|t| t.title)
                        .collect()
                }
                (WishlistScope::Album, Some(r)) => self
                    .lib
                    .tracks_of_release(r.id)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|t| t.has_local)
                    .map(|t| t.title)
                    .chain(self.lib.unwanted_of_release(r.id).unwrap_or_default().into_iter().map(|u| u.title))
                    .collect(),
                _ => Vec::new(),
            },
        }
    }

    /// The download sources that are on, in the user's order: the indexer's
    /// torrents (once Prowlarr is set up) and the sources the app has.
    pub(crate) fn download_sources(&self) -> Vec<Arc<dyn Source>> {
        let order = self.state.read().prefs.source_order();
        let mut all: Vec<Arc<dyn Source>> = self.sources.clone();
        if let Some(i) = &self.indexer {
            all.push(Arc::new(nikgob_sources::torrent::IndexerSource::new(i.clone())));
        }
        order
            .iter()
            .filter(|s| s.on)
            .filter_map(|s| all.iter().find(|x| x.key() == s.key).cloned())
            .collect()
    }

    /// A source the user turned on or set up can search now: everything not
    /// found is searched for again, there too.
    pub(crate) fn wish_sources_ready(&mut self) {
        if self.streaming.wish.enabled.is_empty() {
            return;
        }
        let keys: Vec<SourceKey> = self.download_sources().iter().map(|s| s.key()).collect();
        let before = self.streaming.wish.enabled.len();
        // Turned on again by the user: it is asked at once, whatever it
        // said before.
        let w = &mut self.streaming.wish;
        let mut forgot = false;
        for k in w.enabled.iter().filter(|k| keys.contains(k)) {
            forgot |= w.pauses.answered(*k);
            w.down.remove(k);
        }
        if forgot {
            self.save_backoff();
            self.publish_sources();
        }
        self.streaming.wish.enabled.retain(|k| !keys.contains(k));
        if self.streaming.wish.enabled.len() < before {
            self.wish_retry_all();
        }
    }

    /// Something can search for the items by itself.
    pub(crate) fn can_download(&self) -> bool {
        !self.download_sources().is_empty()
    }

    /// The quality of the copy `item`'s track has (saved from YouTube),
    /// if that is all it has.
    pub(super) fn copy_quality(&self, item: &WishlistItem) -> Option<i32> {
        let src = self.lib.active_source(item.track_id?).ok().flatten()?;
        (src.kind.is_local() && src.quality_score <= score::SAVED_STREAM).then_some(src.quality_score)
    }

    /// «Источники»: every source in the user's order, what the app has,
    /// and how the last search of each went.
    pub(crate) fn publish_sources(&mut self) {
        let order = self.state.read().prefs.source_order();
        let have: Vec<SourceKey> = self.sources.iter().map(|s| s.key()).collect();
        let indexer = self.indexer.is_some();
        let now = Instant::now();
        let mut soonest: Option<Instant> = None;
        let rows: Vec<v::SourceView> = order
            .iter()
            .map(|s| {
                let (status, until) = self.source_status(s.key, now);
                if let Some(u) = until {
                    soonest = Some(soonest.map_or(u, |x| x.min(u)));
                }
                v::SourceView {
                    key: s.key,
                    name: s.key.display_name().to_owned(),
                    available: if s.key == SourceKey::Torrents { indexer } else { have.contains(&s.key) },
                    status,
                }
            })
            .collect();
        self.streaming.wish.status_until = soonest;
        let can = self.can_download();
        self.publish(|st| {
            st.downloads.sources = rows;
            st.downloads.wishlist.can_search = can;
        });
    }

    /// How `key` is, in words for «Источники», and until when they hold
    /// (a time told in them). Sitting out after its searches turned us
    /// away says the most, then its last search failing (it is not there
    /// at all), then its last download failing.
    fn source_status(&self, key: SourceKey, now: Instant) -> (Option<String>, Option<Instant>) {
        let w = &self.streaming.wish;
        let loc = self.locale();
        if let (Some(until), Some(e)) = (w.pauses.until(key, now), w.pauses.error(key, now)) {
            return (Some(down_text_in(loc, &e, Some(until - now))), Some(until));
        }
        if let Some(e) = w.down.get(&key) {
            return (Some(down_text_in(loc, e, None)), None);
        }
        let Some(e) = w.fetch_down.get(&key) else { return (None, None) };
        let aside = w.refusals.aside(key, now);
        (fetch_down_text_in(loc, e, aside.map(|u| u - now)), aside)
    }

    /// A time told in «Источники» passed: they are told again (without it).
    pub(super) fn sources_expire(&mut self) {
        if self.streaming.wish.status_until.is_some_and(|u| Instant::now() >= u) {
            self.publish_sources();
        }
    }

    /// What turned the downloads away, kept for the next run ([`backoff`]).
    pub(super) fn save_backoff(&self) {
        let w = &self.streaming.wish;
        let saved = backoff::Saved::of(&w.pauses, &w.refusals, &w.later, &w.retry_only, &w.tries, Instant::now(), Self::now());
        match serde_json::to_string(&saved) {
            Ok(json) => {
                if let Err(e) = self.lib.cache_put(backoff::KEY, Self::now(), &json) {
                    log::warn!("wishlist: the pauses are not kept: {e}");
                }
            }
            Err(e) => log::warn!("wishlist: the pauses are not kept: {e}"),
        }
    }

    /// What turned the downloads away in the last run: its pauses go on.
    /// The retries of an item that is done or turned down meanwhile go.
    pub(super) fn load_backoff(&mut self) {
        let Some((_, json)) = self.lib.cache_get(backoff::KEY).ok().flatten() else { return };
        let saved: backoff::Saved = match serde_json::from_str(&json) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("wishlist: the kept pauses do not read: {e}");
                return;
            }
        };
        let now = Instant::now();
        let r = saved.restore(now, Self::now());
        let open = |id: &WishlistId| {
            self.lib
                .get_wishlist_item(*id)
                .ok()
                .flatten()
                .is_some_and(|i| !matches!(i.status, WishlistStatus::Dismissed | WishlistStatus::Done))
        };
        let later: HashMap<WishlistId, u32> = r.later.into_iter().filter(|(id, _)| open(id)).collect();
        let retry_only: HashMap<WishlistId, Vec<SourceKey>> = r.retry_only.into_iter().filter(|(id, _)| open(id)).collect();
        for key in SourceKey::ALL {
            if let Some(until) = r.pauses.until(key, now) {
                log::info!("wishlist: {key:?} still sits out for {} min", (until - now).as_secs() / 60);
            }
        }
        let w = &mut self.streaming.wish;
        w.pauses = r.pauses;
        w.refusals = r.refusals;
        w.later = later;
        w.retry_only = retry_only;
        w.tries = r.tries;
    }
}
