use super::*;
use super::retry::{at_words, fetch_down_text_in, plan_retry, refusal_at, same_state, unix_now};

impl Worker {
    // ------------------------------------------------------------- scheduler

    /// Starts searches while there are free slots.
    pub(crate) fn wish_tick(&mut self) {
        if self.import.is_none() || !self.can_download() {
            return;
        }
        let limit = self.state.read().prefs.auto_download_limit.max(1) as usize;
        let busy = [WishlistStatus::Searching, WishlistStatus::Downloading]
            .iter()
            .map(|s| self.lib.list_wishlist_by_status(*s).map_or(0, |l| l.len()))
            .sum::<usize>();
        let mut free = limit.saturating_sub(busy);
        if free == 0 {
            return;
        }
        let now = Self::now();
        // Items whose file came another way are done; track items an album
        // item covers are searched with it, never apart.
        match self.lib.settle_downloaded_wishlist() {
            Ok(0) => {}
            Ok(n) => {
                log::info!("wishlist: {n} items have their file already");
                self.publish_wishlist();
            }
            Err(e) => log::warn!("wishlist: settling the downloaded items: {e}"),
        }
        let covered = self.lib.wishlist_covered().unwrap_or_default();
        let ask = (free + covered.len()) as u32;
        let mut due: Vec<WishlistItem> = self
            .lib
            .wishlist_due(WishlistStatus::New, now, ask)
            .unwrap_or_default()
            .into_iter()
            .filter(|i| !covered.contains(&i.id))
            .collect();
        if due.len() < free {
            // The weekly search of what was not found, or has only the
            // YouTube copy.
            due.extend(
                self.lib
                    .wishlist_due(WishlistStatus::NoTorrent, now, ask)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|i| i.next_try_at.is_some() && !covered.contains(&i.id)),
            );
        }
        for item in due {
            if free == 0 {
                break;
            }
            free -= 1;
            self.wish_search(item);
        }
    }

    pub(super) fn wish_search(&mut self, item: WishlistItem) {
        let mut sources = self.download_sources();
        // Searched again because sources turned it away: those are asked,
        // and the others once they had their [`ASK_AGAIN_AFTER`] (what had
        // nothing then may have it now).
        if let Some(only) = self.streaming.wish.retry_only.remove(&item.id) {
            let asked = self.streaming.wish.asked.get(&item.id);
            let stale = |k: SourceKey| asked.and_then(|a| a.get(&k)).is_some_and(|t| t.elapsed() >= ASK_AGAIN_AFTER);
            let some: Vec<Arc<dyn Source>> =
                sources.iter().filter(|s| only.contains(&s.key()) || stale(s.key())).cloned().collect();
            if !some.is_empty() {
                sources = some;
            }
        }
        if sources.is_empty() {
            return;
        }
        // A web source sitting out (a bot check, a day's limit) is not
        // asked: it counts as having said no again, without a request.
        let now = Instant::now();
        let paused: Vec<(SourceKey, SourceError)> = sources
            .iter()
            .filter_map(|s| self.streaming.wish.pauses.error(s.key(), now).map(|e| (s.key(), e)))
            .collect();
        sources.retain(|s| !paused.iter().any(|(k, _)| *k == s.key()));
        let not_asked: Vec<SourceKey> = paused.iter().map(|(k, _)| *k).collect();
        let want = self.want_of(&item);
        if want.artists.is_empty() || (want.title.is_none() && want.album.is_none()) {
            self.no_torrent(item.id, false);
            return;
        }
        let id = item.id;
        // A few searches a day at most (better copies keep their own pace).
        if !sources.is_empty() && item.origin != WishlistOrigin::Upgrade {
            let day = self.streaming.wish.tries.take(id, Self::now());
            self.save_backoff();
            if let Err(at) = day {
                let wait = Duration::from_millis((at - Self::now()).max(0) as u64);
                log::info!("wishlist: {id} searched {} times today, again in {} min", backoff::DAY_TRIES, wait.as_secs() / 60);
                self.set_item(id, |i| {
                    i.status = WishlistStatus::New;
                    i.next_try_at = Some(at);
                    i.note = Some(StoredLine::SearchAgain { at: at_words(wait) }.encode());
                });
                self.publish_wishlist();
                return;
            }
        }
        self.set_item(id, |i| {
            i.status = WishlistStatus::Searching;
            i.attempts += 1;
            i.note = Some(note_code(WishNote::Searching));
        });
        let names: Vec<&str> = sources.iter().map(|s| s.name()).collect();
        log::info!(
            "wishlist: searching {} — {} in {names:?}",
            want.artist(),
            want.title.as_deref().or(want.album.as_deref()).unwrap_or("?")
        );
        self.streaming.spawn(async move {
            let asks = sources.iter().map(|s| {
                let s = s.clone();
                let want = &want;
                async move {
                    let r = match tokio::time::timeout(SEARCH_TIMEOUT, s.search(want)).await {
                        Ok(r) => r,
                        Err(_) => Err(SourceError::Unreachable("no answer in time".into())),
                    };
                    (s.key(), r)
                }
            });
            let mut offers = Vec::new();
            let (mut answered, mut down) = (Vec::new(), paused);
            for (key, r) in futures_util::future::join_all(asks).await {
                match r {
                    Ok(o) => {
                        log::info!("wishlist: {} offers from {key:?}", o.len());
                        offers.extend(o);
                        answered.push(key);
                    }
                    Err(SourceError::NotConfigured) => {}
                    // It searched, but nothing it finds can be had: no
                    // offers, and «Источники» say so.
                    Err(SourceError::NothingToFetch) => {
                        answered.push(key);
                        down.push((key, SourceError::NothingToFetch));
                    }
                    // Logged once per state, when it comes back
                    // ([`Worker::wish_msg`]).
                    Err(e) => down.push((key, e)),
                }
            }
            // Nobody could answer: not "not found", just later.
            let result = if answered.is_empty() && !down.is_empty() {
                Err(down.iter().map(|(k, e)| format!("{}: {e}", k.display_name())).collect::<Vec<_>>().join("; "))
            } else {
                Ok(offers)
            };
            Msg::Wish(WishMsg::Searched {
                item: id,
                result,
                answered,
                down,
                paused: not_asked,
            })
        });
        self.publish_wishlist();
    }

    pub(crate) fn wish_msg(&mut self, m: WishMsg) {
        match m {
            WishMsg::Searched {
                item,
                result,
                answered,
                down,
                paused,
            } => {
                let now = Instant::now();
                let mut kept = false;
                for k in answered {
                    let w = &mut self.streaming.wish;
                    w.down.remove(&k);
                    w.asked.entry(item).or_default().insert(k, now);
                    kept |= w.pauses.answered(k);
                }
                for (k, e) in &down {
                    let w = &mut self.streaming.wish;
                    // Once per state, not at every search.
                    if !w.down.insert(*k, e.clone()).is_some_and(|was| same_state(&was, e)) {
                        log::warn!("wishlist: {k:?}: {e}");
                    }
                    if paused.contains(k) {
                        continue;
                    }
                    if let Some(until) = w.pauses.failed(*k, e, now) {
                        log::info!("wishlist: {k:?} is not asked for {} min", (until - now).as_secs() / 60);
                        kept = true;
                    }
                }
                if kept {
                    self.save_backoff();
                }
                let turned: Vec<(SourceKey, SourceError)> = down.into_iter().filter(|(_, e)| e.is_refusal()).collect();
                self.streaming.wish.turned.insert(item, turned);
                self.publish_sources();
                self.wish_searched(item, result);
            }
            WishMsg::Fetched { item, offer, result } if self.streaming.wish.probe.contains_key(&item) => match result {
                Ok(Fetched::Torrent(p)) => self.wish_inspect(item, *offer, p),
                Ok(Fetched::Files(_) | Fetched::Partial { .. }) | Err(_) => self.probe_done(item, false),
            },
            WishMsg::Fetched { item, offer, result } => match result {
                Ok(Fetched::Torrent(p)) => self.wish_inspect(item, *offer, p),
                Ok(Fetched::Files(files)) => {
                    let was_aside = self.streaming.wish.refusals.worked(offer.source);
                    if was_aside {
                        self.save_backoff();
                    }
                    if self.streaming.wish.fetch_down.remove(&offer.source).is_some() {
                        self.publish_sources();
                    }
                    let key = offer.source;
                    self.wish_files(item, *offer, files);
                    if was_aside {
                        self.wish_wake(key);
                    }
                }
                Ok(Fetched::Partial { files, missing, error }) => self.wish_partial(item, *offer, files, missing.len(), error),
                Err(e) => {
                    log::warn!("wishlist: fetching {} from {:?}: {e}", offer.title, offer.source);
                    if offer.source != SourceKey::Torrents && fetch_down_text_in(self.locale(), &e, None).is_some() {
                        self.streaming.wish.fetch_down.insert(offer.source, e.clone());
                        self.publish_sources();
                    }
                    self.streaming.wish.live.lock().remove(&item);
                    self.streaming.wish.runs.end(item);
                    self.drop_direct(item);
                    let refused = e.is_refusal() && offer.source != SourceKey::Torrents;
                    if refused {
                        self.wish_turned_away(item, &offer, &e);
                    }
                    let next = self.wish_next_candidate(item);
                    if refused {
                        self.refusal_line(item, &offer, &e, &next);
                    }
                }
            },
            WishMsg::Inspected {
                item,
                offer,
                payload,
                result,
            } => self.wish_inspected(item, *offer, payload, result),
            WishMsg::Saved { item, track, run, result } => self.wish_saved(item, track, run, result),
            WishMsg::RunStopped { item, token } => self.run_stopped(item, token),
            WishMsg::AlbumCame { item, origin, files, images } => self.album_staged(item, origin, files, images),
            WishMsg::AlbumRestored { release, done, failed, left } => self.album_restored(release, done, failed, left),
        }
    }

    fn wish_searched(&mut self, id: WishlistId, result: Result<Vec<Offer>, String>) {
        let Some(item) = self.lib.get_wishlist_item(id).ok().flatten() else { return };
        if item.status != WishlistStatus::Searching {
            return; // dismissed meanwhile
        }
        let offers = match result {
            Ok(c) => c,
            Err(e) => {
                // The sources are down: not "not found", just later.
                log::warn!("wishlist search: {e}");
                if self.streaming.wish.turned.get(&id).is_some_and(|t| !t.is_empty()) {
                    self.wish_later(id);
                    return;
                }
                self.set_item(id, |i| {
                    i.status = WishlistStatus::New;
                    i.next_try_at = Some(Self::now() + 3_600_000);
                    i.note = Some(note_code(WishNote::SourcesSilent));
                });
                self.publish_wishlist();
                return;
            }
        };
        let want = self.want_of(&item);
        let order: Vec<SourceKey> = self.state.read().prefs.source_order().iter().map(|s| s.key).collect();
        // A better copy of a file the library has: only what is at least as
        // good as the user wants.
        let copy = self.copy_quality(&item).or_else(|| self.upgrade_floor(&item));
        // …and really better than one of the item's files at least
        // ([`nikgob_core::quality::really_better`]): nothing is better than
        // our Opus of a genuine FLAC, or a genuine FLAC.
        let ranks = self.upgrade_ranks(&item);
        // A source set aside (its downloads kept failing) is not picked;
        // the item may wait for it.
        let now = Instant::now();
        let mut offers = offers;
        let mut aside: Vec<SourceKey> = Vec::new();
        offers.retain(|o| {
            let out = self.streaming.wish.refusals.aside(o.source, now).is_some();
            if out && !aside.contains(&o.source) {
                aside.push(o.source);
            }
            !out
        });
        // A place a file of which held another song for the track is not
        // taken for it again.
        let bad = self.wrong_places(&item);
        if !bad.is_empty() {
            offers.retain(|o| {
                let out = bad.contains(&offer_key(o));
                if out {
                    log::info!("wishlist: {} gave another song for {:?} before, left out", o.title, want.title);
                }
                !out
            });
        }
        for key in aside {
            let left = self.streaming.wish.refusals.aside(key, now).map(|u| u - now);
            log::info!("wishlist: {key:?} is set aside, its offers wait");
            // Told as what it kept doing (a 403 is not «просит подождать»).
            let why = self.streaming.wish.refusals.why(key).cloned().unwrap_or(SourceError::RateLimited { retry_after: left });
            self.streaming.wish.turned.entry(id).or_default().push((key, why));
        }
        let ranked: Vec<_> = rank(offers, &want, &order)
            .into_iter()
            // With a copy, only what is better than it.
            .filter(|r| copy.is_none_or(|q| r.offer.quality() > q))
            .filter(|r| upgrade::offer_counts(ranks.as_deref(), r.offer.quality()))
            // A torrent whose list had no file of it before: never again.
            .filter(|r| {
                let missed = self.missed_before(&item, &r.offer);
                if missed {
                    log::info!("wishlist: {} had no file of it before", r.offer.title);
                }
                !missed
            })
            // A better copy of an album is a folder with the album's songs
            // in it, not one that only shares a word with it.
            .filter(|r| match crate::soulseek::folder_listing(&r.offer) {
                Some(files) => {
                    let fits = self.upgrade_folder_fits(&item, &files);
                    if !fits {
                        log::info!("wishlist: {} has none of the songs of {:?}", r.offer.title, want.album);
                    }
                    fits
                }
                None => true,
            })
            .collect();
        if ranked.is_empty() {
            // Nothing found, but a source could not be asked: later, not
            // in a week.
            if self.streaming.wish.turned.get(&id).is_some_and(|t| !t.is_empty()) {
                self.wish_later(id);
            } else {
                self.no_torrent(id, true);
            }
            return;
        }
        // It replaces a file the user has: asked first, unless they let it.
        if self.upgrade_asks(&item) {
            let offers: Vec<Offer> = ranked.into_iter().map(|r| r.offer).collect();
            self.confirm_after_look(id, offers);
            return;
        }
        match pick(&ranked, copy) {
            Some(first) => {
                // Every sure one the policy takes without asking (lossless,
                // or better than the copy), best first, the picked first:
                // when one source turns the download away, the next takes over.
                let takes = |r: &nikgob_sources::select::Ranked| {
                    r.verdict.sure && (r.offer.lossless() || copy.is_some_and(|q| r.offer.quality() > q))
                };
                let mut sure: Vec<Offer> = vec![ranked[first].offer.clone()];
                sure.extend(
                    ranked
                        .iter()
                        .enumerate()
                        .filter(|(i, r)| *i != first && takes(r))
                        .map(|(_, r)| r.offer.clone()),
                );
                // The rest (doubtful, or lossy and sure) go to «Подтверди»
                // when every one taken is turned away: another source's
                // file to choose, not a wait for the one that refuses.
                let doubtful: Vec<Offer> = ranked
                    .iter()
                    .enumerate()
                    .filter(|(i, r)| *i != first && !takes(r))
                    .map(|(_, r)| r.offer.clone())
                    .collect();
                let best = sure[0].clone();
                self.streaming.wish.cands.insert(id, sure);
                self.streaming.wish.doubtful.insert(id, doubtful);
                self.wish_fetch(id, best);
            }
            None => {
                let doubtful: Vec<Offer> = ranked.into_iter().map(|r| r.offer).collect();
                self.confirm_after_look(id, doubtful);
            }
        }
    }

    /// The doubtful `offers` (best first) into «Подтверди»; for a track,
    /// the torrents among them only once their list shows its file
    /// ([`Probe`]).
    fn confirm_after_look(&mut self, id: WishlistId, offers: Vec<Offer>) {
        // A track's torrents, and those of an album a better copy is looked
        // for (its list must have the album's songs), are looked into first,
        // the likeliest by their titles and sizes first.
        let item = self.lib.get_wishlist_item(id).ok().flatten();
        let track = item
            .as_ref()
            .is_some_and(|i| i.scope == WishlistScope::Track || i.origin == WishlistOrigin::Upgrade);
        let mut looked: Vec<usize> = offers
            .iter()
            .enumerate()
            .filter(|(_, o)| track && o.source == SourceKey::Torrents)
            .map(|(i, _)| i)
            .collect();
        if let Some(item) = &item {
            let want = self.want_of(item);
            // Stable: alike ones stay in the policy's order.
            looked.sort_by(|a, b| look_first(&offers[*b], &want).total_cmp(&look_first(&offers[*a], &want)));
        }
        looked.truncate(PROBE_MAX);
        // The torrents not looked into stay out; the rest keep their order.
        let mut all: Vec<Offer> = Vec::new();
        let mut at: HashMap<usize, usize> = HashMap::new();
        for (i, o) in offers.into_iter().enumerate() {
            if track && o.source == SourceKey::Torrents && !looked.contains(&i) {
                continue;
            }
            at.insert(i, all.len());
            all.push(o);
        }
        let todo: Vec<usize> = looked.iter().filter_map(|i| at.get(i).copied()).collect();
        self.streaming.wish.probe.insert(id, Probe { all, todo, passed: HashSet::new() });
        self.probe_next(id);
    }

    /// The next torrent of the item's [`Probe`], or «Подтверди» with what
    /// passed once none is left.
    fn probe_next(&mut self, id: WishlistId) {
        let Some(p) = self.streaming.wish.probe.get_mut(&id) else { return };
        if p.todo.is_empty() {
            let Some(p) = self.streaming.wish.probe.remove(&id) else { return };
            let shown: Vec<Offer> = p
                .all
                .into_iter()
                .enumerate()
                .filter(|(i, o)| o.source != SourceKey::Torrents || p.passed.contains(i))
                .map(|(_, o)| o)
                .take(CONFIRM_OPTIONS)
                .collect();
            if shown.is_empty() {
                // None of them had the file; a source that said no may give it later.
                if self.streaming.wish.turned.get(&id).is_some_and(|t| !t.is_empty()) {
                    self.wish_later(id);
                } else {
                    self.no_torrent(id, true);
                }
            } else {
                self.ask_confirm(id, &shown);
            }
            return;
        }
        let offer = p.all[p.todo[0]].clone();
        let Some(source) = self.source_of(offer.source) else {
            if let Some(p) = self.streaming.wish.probe.get_mut(&id) {
                p.todo.remove(0);
            }
            return self.probe_next(id);
        };
        let Some(item) = self.lib.get_wishlist_item(id).ok().flatten() else { return };
        let want = self.want_of(&item);
        let dest = self.direct_dir(id);
        let note = StoredLine::CheckingTorrent { title: offer.title.clone() }.encode();
        self.set_item(id, |i| i.note = Some(note));
        self.streaming.spawn(async move {
            let quiet: nikgob_core::source::Progress = Arc::new(|_, _| {});
            let result = match tokio::time::timeout(Duration::from_secs(60), source.fetch(&offer, &want, &dest, quiet)).await {
                Ok(r) => r,
                Err(_) => Err(SourceError::Unreachable("the torrent took too long".into())),
            };
            Msg::Wish(WishMsg::Fetched { item: id, offer: Box::new(offer), result })
        });
        self.publish_wishlist();
    }

    /// The torrent the item's [`Probe`] looks into now has the track's
    /// file (`found`) or not: on to the next, or to «Подтверди» with it
    /// once one has it.
    fn probe_done(&mut self, id: WishlistId, found: bool) {
        if let Some(p) = self.streaming.wish.probe.get_mut(&id)
            && !p.todo.is_empty()
        {
            let i = p.todo.remove(0);
            if found {
                p.passed.insert(i);
                p.todo.clear();
            }
        }
        self.probe_next(id);
    }

    /// The user picks among `shown` in «Подтверди».
    fn ask_confirm(&mut self, id: WishlistId, shown: &[Offer]) {
        let json = serde_json::to_string(shown).ok();
        let upgrade = self.is_upgrade(id);
        self.set_item(id, |i| {
            i.status = WishlistStatus::Confirm;
            i.candidates_json = json;
            i.note = Some(note_code(if upgrade { WishNote::FoundBetter } else { WishNote::NotSure }));
        });
        if upgrade {
            self.upgrade_offered(id, shown);
        }
        self.publish_wishlist();
    }

    /// The source of an offer, on or not (a pick in «Подтверди» from a
    /// source switched off since still downloads).
    fn source_of(&self, key: SourceKey) -> Option<Arc<dyn Source>> {
        if key == SourceKey::Torrents {
            return self
                .indexer
                .as_ref()
                .map(|i| Arc::new(nikgob_sources::torrent::IndexerSource::new(i.clone())) as Arc<dyn Source>);
        }
        self.sources.iter().find(|s| s.key() == key).cloned()
    }

    /// The folder a direct download of `item` writes into.
    pub(super) fn direct_dir(&self, item: WishlistId) -> PathBuf {
        let root = match &self.import {
            Some(svc) => svc.config().downloads_dir.join(SOURCES_DIR),
            None => std::env::temp_dir().join("nikgob-sources"),
        };
        root.join(item.to_string())
    }

    /// The folder of `item`'s direct download goes (the import took it, or
    /// it failed).
    pub(super) fn drop_direct(&mut self, item: WishlistId) {
        if let Some(dir) = self.streaming.wish.direct.remove(&item)
            && dir.exists()
            && let Err(e) = std::fs::remove_dir_all(&dir)
        {
            log::warn!("removing {}: {e}", dir.display());
        }
    }

    pub(super) fn wish_fetch(&mut self, id: WishlistId, offer: Offer) {
        let Some(source) = self.source_of(offer.source) else {
            self.wish_next_candidate(id);
            return;
        };
        let Some(item) = self.lib.get_wishlist_item(id).ok().flatten() else { return };
        self.upgrade_fetching(&item);
        let want = self.want_of(&item);
        match nikgob_sources::matching::performer_of(&offer, &want).filter(|_| item.scope == WishlistScope::Track) {
            Some(names) => self.streaming.wish.relabel.insert(id, names),
            None => self.streaming.wish.relabel.remove(&id),
        };
        let torrent = offer.source == SourceKey::Torrents;
        let dest = self.direct_dir(id);
        let from = if offer.provider.is_empty() { offer.source.display_name().to_owned() } else { offer.provider.clone() };
        self.streaming.wish.from.insert(id, from);
        self.streaming.wish.offer_keys.insert(id, offer_key(&offer));
        if torrent {
            let note = StoredLine::Torrent { title: offer.title.clone() }.encode();
            self.set_item(id, |i| i.note = Some(note));
        } else {
            let _ = std::fs::remove_dir_all(&dest);
            if let Err(e) = std::fs::create_dir_all(&dest) {
                log::warn!("{}: {e}", dest.display());
                self.toast_err(nikgob_core::i18n::errors_in(self.locale()).could_not_create_folder, None);
                return;
            }
            self.streaming.wish.direct.insert(id, dest.clone());
            self.streaming.wish.live.lock().insert(id, (0, offer.size_bytes));
            let note = StoredLine::Offer { offer: OfferWords::of(&offer), title: offer.title.clone() }.encode();
            self.set_item(id, |i| {
                i.status = WishlistStatus::Downloading;
                i.note = Some(note);
            });
        }
        let live = self.streaming.wish.live.clone();
        let size = offer.size_bytes;
        let progress: nikgob_core::source::Progress = Arc::new(move |done, total| {
            let mut l = live.lock();
            // Only while it runs: a late call of a stopped one adds nothing.
            if let Some(at) = l.get_mut(&id) {
                *at = (done, total.or(size).filter(|t| *t > 0));
            }
        });
        let (title, source_name) = (self.item_title(id), offer.source.display_name().to_owned());
        let from = self.streaming.wish.from.get(&id).cloned().unwrap_or_default();
        let folder = dest.clone();
        let token = self.streaming.wish.runs.reserve();
        let task = self.streaming.spawn(async move {
            let limit = if torrent { Duration::from_secs(120) } else { FETCH_TIMEOUT };
            let result = match tokio::time::timeout(limit, source.fetch(&offer, &want, &dest, progress)).await {
                Ok(r) => r,
                Err(_) => Err(SourceError::Unreachable("the download took too long".into())),
            };
            Msg::Wish(WishMsg::Fetched {
                item: id,
                offer: Box::new(offer),
                result,
            })
        });
        // A torrent's fetch only brings its .torrent: the torrent's own row
        // follows once it is added.
        if !torrent {
            self.streaming.wish.runs.start(id, token, title, from, Some(source_name), Some(folder), Some(task));
        }
        self.publish_wishlist();
        self.publish_running();
    }

    /// A source other than torrents brought its files: the import takes
    /// them for the item, as a file the user handed in would be.
    fn wish_files(&mut self, id: WishlistId, offer: Offer, files: Vec<PathBuf>) {
        self.streaming.wish.live.lock().remove(&id);
        self.streaming.wish.runs.end(id);
        let Some(item) = self.lib.get_wishlist_item(id).ok().flatten() else {
            self.drop_direct(id);
            return;
        };
        if matches!(item.status, WishlistStatus::Dismissed | WishlistStatus::Done) || files.is_empty() {
            self.drop_direct(id);
            if files.is_empty() {
                self.wish_next_candidate(id);
            }
            return;
        }
        // A better copy of an album asked for by hand: matched and asked
        // about first, nothing imported yet.
        if album_better::hand_album(&item) {
            self.album_came(id, files, SourceOrigin::Inbox);
            self.publish_wishlist();
            return;
        }
        let one = files.len() == 1 && item.scope == WishlistScope::Track && self.targets_of(&item).len() == 1;
        let targets = self.item_targets(&item, one);
        // Named apart from the item's own line of «История» («Artist —
        // Title», [`Self::wish_line`]): the import leaves a line of its own.
        let title = format!("{}: {}", offer.provider, item.title.clone().unwrap_or_default());
        let note = StoredLine::Offer { offer: OfferWords::of(&offer), title: offer.title.clone() }.encode();
        if self.import_for(files, title, SourceOrigin::Inbox, targets) {
            self.set_item(id, |i| {
                i.status = WishlistStatus::Downloading;
                i.note = Some(note);
            });
        } else {
            self.drop_direct(id);
            self.wish_next_candidate(id);
        }
        self.publish_wishlist();
    }

    fn wish_inspect(&mut self, id: WishlistId, offer: Offer, payload: TorrentPayload) {
        let tx = self.streaming.internal.clone();
        self.with_client(Box::new(move |w, c| {
            let Some(svc) = &w.import else { return };
            svc.spawn(async move {
                let result = c.inspect(payload.clone()).await.map_err(|e| e.to_string());
                let _ = tx.send(crate::import::Internal::Stream(Msg::Wish(WishMsg::Inspected {
                    item: id,
                    offer: Box::new(offer),
                    payload,
                    result,
                })));
            });
        }));
    }

    fn wish_inspected(
        &mut self,
        id: WishlistId,
        offer: Offer,
        payload: TorrentPayload,
        result: Result<nikgob_torrent::TorrentMeta, String>,
    ) {
        let Some(item) = self.lib.get_wishlist_item(id).ok().flatten() else { return };
        if matches!(item.status, WishlistStatus::Dismissed | WishlistStatus::Done) {
            self.streaming.wish.probe.remove(&id);
            return;
        }
        if self.streaming.wish.probe.contains_key(&id) {
            let want = self.want_of(&item);
            let found = match &result {
                Ok(meta) => {
                    let files: Vec<(usize, String, u64)> = meta.files.iter().map(|f| (f.index, f.path.clone(), f.size)).collect();
                    let found = pick_files(&files, &want).is_some_and(|p| picked_fits(&item, &want, &files, &p))
                        && self.torrent_fits(&item, &files);
                    if !found {
                        self.note_miss(&item, &offer, Some(meta.info_hash.as_str()));
                    }
                    found
                }
                // Not listed (no peers in time): not a miss, asked again later.
                Err(_) => false,
            };
            log::info!("wishlist: {} {} of {:?}", offer.title, if found { "has a file" } else { "has no file" }, want.title);
            self.probe_done(id, found);
            return;
        }
        let meta = match result {
            Ok(m) => m,
            Err(e) => {
                log::warn!("wishlist: listing {}: {e}", offer.title);
                self.wish_next_candidate(id);
                return;
            }
        };
        let files: Vec<(usize, String, u64)> = meta.files.iter().map(|f| (f.index, f.path.clone(), f.size)).collect();
        let want = self.want_of(&item);
        let Some(picked) = pick_files(&files, &want)
            .filter(|p| picked_fits(&item, &want, &files, p))
            .filter(|_| self.torrent_fits(&item, &files))
        else {
            log::info!("wishlist: {} has no file of {:?}", offer.title, want.title.as_ref().or(want.album.as_ref()));
            self.note_miss(&item, &offer, Some(meta.info_hash.as_str()));
            self.wish_next_candidate(id);
            return;
        };
        let size: u64 = files.iter().filter(|(i, ..)| picked.contains(i)).map(|(.., s)| s).sum();
        let seeders = candidate_of(&offer).and_then(|c| c.seeders).unwrap_or(0);
        let note = StoredLine::TorrentFound {
            format: offer.format.clone(),
            size,
            seeders,
            title: offer.title.clone(),
        }
        .encode();
        self.set_item(id, |i| {
            i.status = WishlistStatus::Downloading;
            i.note = Some(note);
        });
        self.streaming.wish.cands.remove(&id);
        self.streaming.wish.doubtful.remove(&id);
        let magnet = match &payload {
            TorrentPayload::Magnet(m) => Some(m.clone()),
            TorrentPayload::TorrentFile(_) => None,
        };
        self.add_payload_for(payload, Some(picked), magnet, Some(id));
        self.publish_wishlist();
    }

    /// What an item's torrent misses are kept under: its track, or its
    /// album.
    fn miss_target(item: &WishlistItem) -> Option<String> {
        match item.scope {
            WishlistScope::Track => item.track_id.map(|t| format!("t{t}")),
            _ => item.release_id.map(|r| format!("r{r}")),
        }
    }

    /// The torrent's `cache` keys of a miss for the item: its info hash
    /// (the indexer's, the list's) and the indexer's id of it.
    fn miss_keys(item: &WishlistItem, offer: &Offer, listed: Option<&str>) -> Vec<String> {
        let Some(target) = Self::miss_target(item) else { return Vec::new() };
        let cand = candidate_of(offer);
        let mut keys: Vec<String> = cand
            .as_ref()
            .and_then(|c| c.info_hash.as_deref())
            .into_iter()
            .chain(listed)
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .map(|h| format!("{MISS_KEY}:{target}:{}", h.to_lowercase()))
            .collect();
        if !offer.id.is_empty() {
            keys.push(format!("{MISS_KEY}:{target}:id:{}:{}", offer.provider, offer.id));
        }
        keys.dedup();
        keys
    }

    /// The torrent's list came without the item's file: kept, so it is
    /// never looked into or offered for the item again.
    fn note_miss(&self, item: &WishlistItem, offer: &Offer, listed: Option<&str>) {
        for key in Self::miss_keys(item, offer, listed) {
            if let Err(e) = self.lib.cache_put(&key, Self::now(), "1") {
                log::warn!("wishlist: a torrent's miss is not kept: {e}");
            }
        }
    }

    /// The torrent's list was seen before without the item's file.
    fn missed_before(&self, item: &WishlistItem, offer: &Offer) -> bool {
        offer.source == SourceKey::Torrents
            && Self::miss_keys(item, offer, None).iter().any(|k| self.lib.cache_get(k).ok().flatten().is_some())
    }

    /// A torrent's list (path, size) for an item that looks for a better
    /// copy of an album: it has the album's songs. Anything else passes.
    fn torrent_fits(&self, item: &WishlistItem, files: &[(usize, String, u64)]) -> bool {
        let names: Vec<(String, Option<u32>)> = files.iter().map(|(_, p, _)| (p.clone(), None)).collect();
        self.upgrade_folder_fits(item, &names)
    }

    /// The offer did not work out: the next sure one, else the doubtful
    /// ones in «Подтверди», else later when a source turned the item away,
    /// else not found.
    pub(super) fn wish_next_candidate(&mut self, id: WishlistId) -> Next {
        // Turned down (or done) meanwhile: nothing more for it.
        if self
            .lib
            .get_wishlist_item(id)
            .ok()
            .flatten()
            .is_none_or(|i| matches!(i.status, WishlistStatus::Dismissed | WishlistStatus::Done))
        {
            let w = &mut self.streaming.wish;
            w.cands.remove(&id);
            w.doubtful.remove(&id);
            w.probe.remove(&id);
            return Next::NotFound;
        }
        let next = match self.streaming.wish.cands.get_mut(&id) {
            Some(list) if !list.is_empty() => {
                list.remove(0);
                list.first().cloned()
            }
            _ => None,
        };
        match next {
            Some(c) => {
                let source = c.source;
                self.wish_fetch(id, c);
                Next::Offer(source)
            }
            None => {
                self.streaming.wish.cands.remove(&id);
                let doubtful = self.streaming.wish.doubtful.remove(&id).unwrap_or_default();
                if !doubtful.is_empty() {
                    self.confirm_after_look(id, doubtful);
                    Next::Doubtful
                } else if self.streaming.wish.turned.get(&id).is_some_and(|t| !t.is_empty()) {
                    self.wish_later(id)
                } else {
                    // A better copy was found but did not come: no «nothing
                    // better» for it.
                    let tell = !self.is_upgrade(id);
                    self.no_torrent(id, tell);
                    Next::NotFound
                }
            }
        }
    }

    /// A source gave part of an album: what came is imported, the rest
    /// goes on once the import took it ([`Self::album_goes_on`]).
    fn wish_partial(&mut self, id: WishlistId, offer: Offer, files: Vec<PathBuf>, missing: usize, error: SourceError) {
        let all = files.len() + missing;
        log::warn!("wishlist: {} from {:?}: {} of {all} tracks, the rest: {error}", offer.title, offer.source, files.len());
        let why = if error.is_refusal() {
            self.wish_turned_away(id, &offer, &error);
            StoredLine::Refusal { why: refusal_at(offer.source, &error, What::Rest, unix_now()) }
        } else {
            if self.streaming.wish.refusals.worked(offer.source) {
                self.save_backoff();
            }
            StoredLine::Absent { missing, source: offer.source }
        };
        // A better copy says only what it found and what came of it.
        if !self.is_upgrade(id) {
            let what = self.item_title(id);
            let line = StoredLine::Partial { have: files.len(), all, why: Box::new(why) }.encode();
            self.wish_line(id, what, line, None);
        }
        self.streaming.wish.partial.insert(id);
        self.wish_files(id, offer, files);
    }

    /// `offer`'s source turned the item's download away with `e`: noted
    /// for the item and the source (set aside when it keeps doing so), and
    /// the item's other offers of that source go (they would be refused too).
    fn wish_turned_away(&mut self, id: WishlistId, offer: &Offer, e: &SourceError) {
        let key = offer.source;
        self.streaming.wish.turned.entry(id).or_default().push((key, e.clone()));
        if let Some(list) = self.streaming.wish.cands.get_mut(&id) {
            let mut first = true;
            list.retain(|o| std::mem::take(&mut first) || o.source != key);
        }
        if let Some(until) = self.streaming.wish.refusals.refused(key, e, Instant::now()) {
            let left = until.saturating_duration_since(Instant::now());
            log::info!("wishlist: {key:?} keeps turning downloads away, set aside for {} min", left.as_secs() / 60);
            // «Находит, но не скачивает — пропускаю до 02:07», the time
            // told while it holds ([`Self::source_status`]).
            self.streaming.wish.fetch_down.entry(key).or_insert_with(|| e.clone());
            self.publish_sources();
        }
        self.save_backoff();
    }

    /// A download of `key` worked after it had been set aside: the items
    /// that wait for it are searched again now.
    fn wish_wake(&mut self, key: SourceKey) {
        let waiting: Vec<WishlistId> =
            self.streaming.wish.retry_only.iter().filter(|(_, keys)| keys.contains(&key)).map(|(id, _)| *id).collect();
        let now = Self::now();
        for id in waiting {
            self.set_item(id, |i| {
                if i.status == WishlistStatus::New && i.next_try_at.is_some_and(|t| t > now) {
                    i.next_try_at = Some(now);
                }
            });
        }
        self.wish_tick();
    }

    /// «Загрузки → История»: `offer`'s source turned the item away, and
    /// what came of it (`next`): the next source, or the choice in
    /// «Подтверди». A wait has its line from [`Self::wish_later`].
    fn refusal_line(&mut self, id: WishlistId, offer: &Offer, e: &SourceError, next: &Next) {
        let then = match next {
            Next::Offer(source) => StoredLine::NextOffer { source: *source },
            Next::Doubtful => StoredLine::DoubtfulPick,
            Next::Later(_) | Next::NotFound => return,
        };
        if self.is_upgrade(id) {
            return;
        }
        let why = StoredLine::Refusal { why: refusal_at(offer.source, e, self.item_what(id), unix_now()) };
        let line = StoredLine::Capitalised { line: Box::new(StoredLine::Joined { a: Box::new(why), b: Box::new(then) }) }.encode();
        let title = self.item_title(id);
        self.wish_line(id, title, line, None);
    }

    /// Every offer of the item was turned away (or a source could not be
    /// asked): it is searched again ([`plan_retry`]) [`LATER_TRIES`] times,
    /// then in a week. The item's note and its line in «История» say why
    /// and when which sources are asked.
    pub(super) fn wish_later(&mut self, id: WishlistId) -> Next {
        let turned = self.streaming.wish.turned.remove(&id).unwrap_or_default();
        let Some(item) = self.lib.get_wishlist_item(id).ok().flatten() else { return Next::NotFound };
        if matches!(item.status, WishlistStatus::Dismissed | WishlistStatus::Done) {
            return Next::NotFound;
        }
        // One reason per source: what it kept doing when it is set aside.
        let now = Instant::now();
        let mut seen: Vec<(SourceKey, SourceError, Option<Duration>)> = Vec::new();
        for (k, e) in turned {
            if seen.iter().any(|(s, ..)| *s == k) {
                continue;
            }
            let aside = self.streaming.wish.refusals.aside(k, now).map(|u| u - now);
            // A web source sitting out after a bot check is back when its
            // pause ends (a check names no wait of its own).
            let paused = self.streaming.wish.pauses.until(k, now).map(|u| u - now);
            let back = [e.wait(), aside, paused].into_iter().flatten().max();
            let why = match aside {
                Some(_) => self.streaming.wish.refusals.why(k).cloned().unwrap_or(e),
                None => e,
            };
            seen.push((k, why, back));
        }
        let partly = match item.scope {
            WishlistScope::Album => match self.album_files(&item) {
                (have, all) if have > 0 && have < all => Some((have, all)),
                _ => None,
            },
            WishlistScope::Track => None,
        };
        let loc = self.locale();
        let what = match (item.scope, partly) {
            (_, Some(_)) => What::Rest,
            (WishlistScope::Album, None) => What::Album,
            (WishlistScope::Track, None) => What::Track,
        };
        let why: Vec<Refusal> = seen.iter().map(|(k, e, _)| refusal_at(*k, e, what, unix_now())).collect();
        let n = {
            let n = self.streaming.wish.later.entry(id).or_default();
            *n += 1;
            *n
        };
        if n > LATER_TRIES {
            // Tried enough for now: the weekly search, in every source.
            self.streaming.wish.later.remove(&id);
            self.streaming.wish.retry_only.remove(&id);
            self.save_backoff();
            let copy = self.copy_quality(&item).is_some();
            self.no_torrent(id, false);
            // A better copy is looked for when its schedule says, quietly.
            if item.origin == WishlistOrigin::Upgrade {
                return Next::NotFound;
            }
            let line = StoredLine::Capitalised {
                line: Box::new(StoredLine::Joined {
                    a: Box::new(StoredLine::Reasons { why }),
                    b: Box::new(StoredLine::Week),
                }),
            }
            .encode();
            if !copy {
                self.set_item(id, |i| i.note = Some(line.clone()));
                self.publish_wishlist();
            }
            let title = self.item_title(id);
            self.wish_line(id, title, String::new(), Some(line));
            return Next::NotFound;
        }
        let keys: Vec<SourceKey> = seen.iter().map(|(k, ..)| *k).collect();
        let asked = self.streaming.wish.asked.get(&id);
        let others: Vec<(SourceKey, Duration)> = self
            .download_sources()
            .iter()
            .map(|s| s.key())
            .filter(|k| !keys.contains(k))
            .filter_map(|k| asked.and_then(|a| a.get(&k)).map(|t| (k, t.elapsed())))
            .collect();
        let backs: Vec<(SourceKey, Option<Duration>)> = seen.iter().map(|(k, _, b)| (*k, *b)).collect();
        let retry = plan_retry(n, &backs, &others);
        let at = Self::now() + retry.wait.as_millis() as i64;
        self.streaming.wish.retry_only.insert(id, keys);
        self.save_backoff();
        let note = StoredLine::Later {
            have: partly,
            why,
            at: at_words(retry.wait),
            asked: retry.ask.clone(),
        }
        .encode();
        log::info!(
            "wishlist: {id} was turned away ({}), again in {} min",
            seen.iter().map(|(k, e, _)| refusal_at(*k, e, what, unix_now()).words_in(loc)).collect::<Vec<_>>().join(", "),
            retry.wait.as_secs() / 60
        );
        self.set_item(id, |i| {
            i.status = WishlistStatus::New;
            i.next_try_at = Some(at);
            i.note = Some(note.clone());
        });
        if item.origin != WishlistOrigin::Upgrade {
            let title = self.item_title(id);
            self.wish_line(id, title, note, None);
        }
        self.publish_wishlist();
        Next::Later(at)
    }

    /// «этот трек» or «этот альбом», as the item is.
    fn item_what(&self, id: WishlistId) -> What {
        match self.lib.get_wishlist_item(id).ok().flatten().map(|i| i.scope) {
            Some(WishlistScope::Album) => What::Album,
            _ => What::Track,
        }
    }

    /// The item's name for the lines: «Artist — Title».
    pub(super) fn item_title(&self, id: WishlistId) -> String {
        match self.lib.get_wishlist_item(id).ok().flatten() {
            Some(i) => match (&i.artist, &i.title) {
                (Some(a), Some(t)) => format!("{a} — {t}"),
                (_, Some(t)) => t.clone(),
                _ => status_in(self.locale()).track.to_owned(),
            },
            None => status_in(self.locale()).track.to_owned(),
        }
    }

    pub(super) fn no_torrent(&mut self, id: WishlistId, tell: bool) {
        if self.is_upgrade(id) {
            self.upgrade_none(id, tell);
            return;
        }
        let copy = self.lib.get_wishlist_item(id).ok().flatten().is_some_and(|i| self.copy_quality(&i).is_some());
        let note = if copy { WishNote::HasCopy } else { WishNote::NotFound };
        let Some(item) = self.set_item(id, |i| {
            i.status = WishlistStatus::NoTorrent;
            i.next_try_at = Some(Self::now() + RETRY_AFTER.as_millis() as i64);
            i.note = Some(note_code(note));
        }) else {
            return;
        };
        if tell && !copy && self.state.read().prefs.download_toasts {
            let loc = self.locale();
            let what = match (&item.artist, &item.title) {
                (Some(a), Some(t)) => format!("{a} — {t}"),
                (_, Some(t)) => t.clone(),
                _ => nikgob_core::i18n::status_in(loc).track.to_owned(),
            };
            let action = (item.track_id.is_some() && self.service(Service::Youtube).is_some()).then(|| {
                (
                    nikgob_core::i18n::downloads_in(loc).save_from_youtube.to_owned(),
                    Command::WishlistSaveFromYoutube(id),
                )
            });
            let text = format!("{}: {what}", nikgob_core::i18n::errors_in(loc).nothing_found_to_download);
            self.toast_err(text, action);
        }
        self.publish_wishlist();
    }
}
