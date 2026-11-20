use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use nikgob_core::streaming::{SearchKind, StreamError, StreamService};
use super::slots::{ordinary_seeds, queue_order};

/// One new-music call; `None` when its second request did not go because
/// the listener's YouTube Music began (`listening`, YouTube Music only).
async fn fetch(
    svc: Arc<dyn StreamService>,
    call: &Call,
    listening: &AtomicBool,
) -> Option<(Result<Vec<StreamTrack>, StreamError>, Option<(ArtistId, RemoteId)>)> {
    Some(match call {
        Call::Radio(id) => (svc.radio(id).await, None),
        Call::Related(_, id) => (svc.related(id).await, None),
        Call::ArtistRadio { artist, id, name, .. } => {
            let (id, matched) = match id {
                Some(id) => (id.clone(), None),
                None => {
                    let key = artist_key(name);
                    let hits = match svc.search_kind(name, SearchKind::Artists).await {
                        Ok(s) => s.artists,
                        Err(e) => return Some((Err(e), None)),
                    };
                    match hits.into_iter().find(|a| artist_key(&a.name) == key) {
                        Some(a) => (a.id.id.clone(), Some((*artist, a.id))),
                        None => return Some((Ok(Vec::new()), None)),
                    }
                }
            };
            if svc.service() == Service::Youtube && listening.load(Ordering::Relaxed) {
                return None;
            }
            (svc.artist_radio(&id).await, matched)
        }
    })
}

fn cache_key(service: Service, what: &str, id: &str) -> String {
    format!("radio_new.{}.{what}.{id}", service.as_str())
}

impl Worker {
    /// New music the radio's queue lacks (it came after the queue was
    /// picked, or the short extensions rounded it away): what its share of
    /// all the radio queued so far wants more joins between the queued
    /// tracks at once, evenly.
    pub(super) fn radio_splice_new(&mut self) {
        if !self.radio_alive() {
            return;
        }
        let prefs = self.state.read().prefs.clone();
        let Some(snap) = self.snapshot() else { return };
        let Some(run) = self.recs.radio.run.as_mut() else { return };
        let share = super::super::new_share(&prefs, run.kind());
        // New music waits for YouTube Music's pause too.
        if self.pause_left(Service::Youtube).is_some() {
            return;
        }
        let Some(run) = self.recs.radio.run.as_mut() else { return };
        if is_wave(&run.state.seed) || run.extending || run.queue_gen.is_none() || !streams_allowed(new_seed(&run.state.seed), &prefs, run.kind()) || share <= 0.0 {
            return;
        }
        let up: Vec<u64> = self.queue.upcoming().iter().filter(|e| run.added.contains(&e.id)).map(|e| e.id).collect();
        let have = self.queue.upcoming().iter().filter(|e| run.added.contains(&e.id) && run.streams.contains_key(&e.track)).count();
        // Of all queued so far (the next pick counted), its share is new.
        let owed = (((run.queued.0 + 1) as f32) * share).floor() as usize;
        let more = owed.saturating_sub(run.queued.1).min(up.len().saturating_sub(have));
        if up.is_empty() || more == 0 {
            return;
        }
        run.state.set_explore(Some(prefs.radio_explore.fraction()));
        let picks = run.state.take_new(&snap.recs().local_only(prefs.mixes_downloaded_only), more, up.len(), have);
        let lately = self.played_lately();
        let mut ids: Vec<(TrackId, RemoteId)> = Vec::new();
        for p in picks {
            let Item::Stream(r) = p.item else { continue };
            let pooled = self.recs.radio.run.as_ref().and_then(|run| run.pool.get(&r).cloned());
            let Some(t) = pooled else { continue };
            self.streaming.learn(std::slice::from_ref(&t));
            if let Some(tid) = self.upsert_streams(std::slice::from_ref(&t)).first().copied()
                && !lately.contains(&tid)
                && !ids.iter().any(|(x, _)| *x == tid)
            {
                ids.push((tid, r));
            }
        }
        // The buffer holds three: new music takes the place of the
        // radio's farthest picks of the library (never the next one, it
        // is preloaded, never the user's), else waits for a free place.
        let room = EXTEND_BY.saturating_sub(self.queue.upcoming().len());
        let mut gone: Vec<Entry> = Vec::new();
        if let Some(run) = self.recs.radio.run.as_ref() {
            let movable: Vec<Entry> =
                self.queue.upcoming().iter().skip(1).filter(|e| run.added.contains(&e.id) && !run.streams.contains_key(&e.track)).copied().collect();
            gone = movable.into_iter().rev().take(ids.len().saturating_sub(room)).collect();
        }
        ids.truncate(room + gone.len());
        if ids.is_empty() {
            return;
        }
        if let Some(run) = self.recs.radio.run.as_mut() {
            let items: Vec<Item> = gone.iter().map(|e| Item::Track(e.track)).collect();
            run.state.forget(&items);
            for e in &gone {
                run.added.remove(&e.id);
                run.queued.0 = run.queued.0.saturating_sub(1);
            }
        }
        self.sleep_replacing(&gone.iter().map(|e| e.id).collect::<Vec<_>>());
        for e in &gone {
            self.queue.remove(e.id);
            self.recs.why.remove(&e.id);
        }
        let up: Vec<u64> = self.queue.upcoming().iter().filter(|e| self.recs.radio.run.as_ref().is_some_and(|r| r.added.contains(&e.id))).map(|e| e.id).collect();
        if up.is_empty() {
            // Nothing of the radio's ahead to go between: after the queue.
            for (tid, r) in &ids {
                let added = self.queue.enqueue(&[*tid]);
                if let Some(run) = self.recs.radio.run.as_mut() {
                    run.added.extend(added.iter().map(|e| e.id));
                    run.streams.insert(*tid, r.clone());
                    run.queued.1 += 1;
                    run.queued.0 += 1;
                }
                self.want_preview(*tid, (r.service == Service::Youtube).then(|| r.id.clone()));
            }
            self.preload_next();
            self.publish_player();
            return;
        }
        let every = (up.len() / (ids.len() + 1)).max(1);
        for (k, (tid, r)) in ids.iter().enumerate() {
            self.queue.enqueue(&[*tid]);
            let Some(entry) = self.queue.upcoming().last().map(|e| e.id) else { continue };
            if let Some(&anchor) = up.get(((k + 1) * every).min(up.len()) - 1) {
                self.queue.move_next_to(entry, anchor, true);
            }
            if let Some(run) = self.recs.radio.run.as_mut() {
                run.added.insert(entry);
                run.streams.insert(*tid, r.clone());
                run.queued.1 += 1;
                run.queued.0 += 1;
                if let Some(why) = run.via.get(r) {
                    self.recs.why.insert(entry, why.clone());
                }
            }
            self.want_preview(*tid, (r.service == Service::Youtube).then(|| r.id.clone()));
        }
        if let Some(run) = self.recs.radio.run.as_ref() {
            log::info!("radio «{}»: {} new music joined the queue ({} of {} new so far)", run.title.title_in(v::Locale::Ru), ids.len(), run.queued.1, run.queued.0);
        }
        self.preload_next();
        self.publish_player();
    }

    /// The new-music pool runs low: more is asked around the last tracks
    /// the radio played to the end (their service's radio or related list).
    pub(super) fn radio_refill(&mut self) {
        let prefs = self.state.read().prefs.clone();
        let Some(run) = self.recs.radio.run.as_ref() else { return };
        if is_wave(&run.state.seed) || super::super::new_share(&prefs, run.kind()) <= 0.0 || prefs.mixes_downloaded_only {
            return;
        }
        if run.state.new_left() >= REFILL_AT {
            return;
        }
        let mut calls: Vec<(Call, String)> = Vec::new();
        let mut asked: Vec<String> = Vec::new();
        for t in run.heard.iter().rev() {
            if calls.len() == REFILL_FROM {
                break;
            }
            let call = match run.streams.get(t) {
                Some(r) if r.service == Service::Youtube => Some(Call::Radio(r.id.clone())),
                Some(r) if r.service == Service::Deezer => Some(Call::Related(Service::Deezer, r.id.clone())),
                Some(_) => None,
                None => self
                    .lib
                    .stream_refs(*t)
                    .unwrap_or_default()
                    .into_iter()
                    .find(|r| r.id.service == Service::Youtube)
                    .map(|r| Call::Radio(r.id.id)),
            };
            let Some(call) = call else { continue };
            let key = match &call {
                Call::Radio(v) | Call::Related(_, v) => v.clone(),
                Call::ArtistRadio { name, .. } => name.clone(),
            };
            if run.asked.contains(&key) {
                continue;
            }
            asked.push(key);
            let title = self.lib.get_track(*t).ok().flatten().map(|t| t.title).unwrap_or_default();
            calls.push((call, title));
        }
        if calls.is_empty() {
            return;
        }
        let id = run.id;
        log::info!("radio «{}»: {} new left, asking around {} of its tracks", run.title.title_in(v::Locale::Ru), run.state.new_left(), calls.len());
        if let Some(run) = self.recs.radio.run.as_mut() {
            run.asked.extend(asked);
        }
        self.radio_ask(id, calls);
    }

    /// The queue ran out: a radio picking more plays on when they come.
    /// YouTube Music calls of radio `id` held back for the listener: asked
    /// again when it looks again (at most [`DEFERRED_MAX`] wait).
    pub(super) fn radio_defer(&mut self, id: u64, calls: Vec<(Call, String)>) {
        for (call, seed) in calls {
            if self.recs.radio.deferred.len() < DEFERRED_MAX {
                self.recs.radio.deferred.push((id, call, seed));
            }
        }
        if !std::mem::replace(&mut self.recs.radio.deferred_wake, true) {
            let after = self.streaming.schedule.listener_recheck + crate::streams::wake_jitter();
            self.streaming.spawn_internal(async move {
                tokio::time::sleep(after).await;
                Internal::Recs(Msg::Radio(RadioMsg::AskDeferred))
            });
        }
    }

    /// The pause that left radio `id` with nothing to pick is over: it
    /// picks again, and a radio that ran out meanwhile plays on.
    pub(super) fn radio_pause_over(&mut self, id: u64) {
        if self.recs.radio.paused_dry.is_none_or(|(r, _)| r != id) {
            return;
        }
        self.recs.radio.paused_dry = None;
        if !self.radio_alive() || self.recs.radio.run_id() != Some(id) {
            return;
        }
        let Some(run) = self.recs.radio.run.as_ref() else { return };
        if run.extending {
            return;
        }
        log::info!("radio «{}»: YouTube Music's pause is over, it picks again", run.title.title_in(v::Locale::Ru));
        if run.resume {
            self.radio_extend();
        } else {
            self.radio_top_up();
        }
    }

    /// New-music candidates for the radio (filtered: not in the library,
    /// not disliked, never junk, the snapshot there or not).
    pub(super) fn radio_add_new(&mut self, tracks: Vec<StreamTrack>) {
        let snap = self.snapshot();
        let fresh: Vec<StreamTrack> = tracks
            .into_iter()
            .filter(|t| match snap.as_ref() {
                Some(s) => s.is_new(t),
                None => t.playable && nikgob_recs::junk(t, None).is_none(),
            })
            .collect();
        if fresh.is_empty() {
            return;
        }
        self.streaming.learn(&fresh);
        let Some(run) = self.recs.radio.run.as_mut() else { return };
        let new: Vec<NewTrack> = fresh
            .iter()
            .map(|t| snap.as_ref().map_or_else(|| NewTrack::from_stream(t, None), |s| s.new_track(t)))
            .collect();
        // An ordinary radio's finds are of its seed: the log of new music
        // learns from them how far its finds are to be trusted.
        let seeds: Vec<TrackId> = ordinary_seeds(&run.state.seed).into_iter().filter(|t| snap.as_ref().is_some_and(|s| s.catalog.position(*t).is_some())).collect();
        for t in fresh {
            if !seeds.is_empty() && run.seeds.len() < SEEDS_KEEP {
                run.seeds.entry(t.id.clone()).or_insert_with(|| seeds.clone());
            }
            run.pool.insert(t.id.clone(), t);
        }
        if run.extending {
            run.pending_new.extend(new.iter().cloned());
        }
        run.state.add_new(new);
        self.radio_prune();
    }

    /// The radio's bookkeeping stays bounded however long it plays: the
    /// candidates as the engine keeps them (its pool is capped) and the
    /// stream picks the queue still holds, where each was found, the songs
    /// asked around lately.
    pub(super) fn radio_prune(&mut self) {
        let in_queue: HashSet<TrackId> = queue_order(&self.queue).iter().map(|e| e.track).collect();
        let Some(run) = self.recs.radio.run.as_mut() else { return };
        // Picks on their way may hold stream candidates not queued yet.
        if run.extending {
            return;
        }
        let heard: HashSet<TrackId> = run.heard.iter().copied().collect();
        run.streams.retain(|t, _| in_queue.contains(t) || heard.contains(t));
        let playing: HashSet<RemoteId> = run.streams.values().cloned().collect();
        let state = &run.state;
        run.pool.retain(|id, _| playing.contains(id) || state.has_new(id));
        let pool = &run.pool;
        run.via.retain(|id, _| pool.contains_key(id));
        if run.asked.len() > ASKED_KEEP {
            run.asked.clear();
        }
    }

    /// The mixes (and «Новое для тебя») take the discoveries as new music.
    pub(super) fn radio_add_discoveries(&mut self) {
        let Some(run) = &self.recs.radio.run else { return };
        if !matches!(
            run.state.seed,
            RadioSeed::Mood(_) | RadioSeed::Style { .. } | RadioSeed::Discover | RadioSeed::Daylist { .. }
        ) {
            return;
        }
        let found = self.discoveries();
        self.radio_add_new(found);
    }

    /// Asks the services for new music around the radio's seed (see the
    /// module docs); what the cache has is used at once.
    pub(super) fn radio_fetch_new(&mut self) {
        let Some(run) = &self.recs.radio.run else { return };
        if is_wave(&run.state.seed) || super::super::new_share(&self.state.read().prefs, run.kind()) <= 0.0 {
            return;
        }
        let id = run.id;
        let seed_name = run.title.seed_name().unwrap_or("").to_owned();
        let mut calls: Vec<(Call, String)> = Vec::new();
        let artist_call = |w: &Worker, a: ArtistId, prefer: &[Service]| -> Option<(Call, String)> {
            let name = w.lib.get_artist(a).ok().flatten()?.name;
            let known = w.lib.external_ids(EntityKind::Artist, *a.as_uuid()).unwrap_or_default();
            prefer.iter().find(|s| w.service(**s).is_some()).map(|&service| {
                let call = Call::ArtistRadio {
                    service,
                    artist: a,
                    id: known.iter().find(|r| r.service == service).map(|r| r.id.clone()),
                    name: name.clone(),
                };
                (call, name.clone())
            })
        };
        match run.state.seed.clone() {
            RadioSeed::Track(t) => {
                let refs = self.lib.stream_refs(t).unwrap_or_default();
                if let Some(yt) = refs.iter().find(|r| r.id.service == Service::Youtube) {
                    let title = self.lib.get_track(t).ok().flatten().map(|t| t.title).unwrap_or_default();
                    calls.push((Call::Radio(yt.id.id.clone()), title));
                }
                let main = self
                    .lib
                    .get_track(t)
                    .ok()
                    .flatten()
                    .and_then(|t| t.artist_credit.artist_ids().next());
                if let Some(a) = main {
                    calls.extend(artist_call(self, a, &[Service::Deezer]));
                }
            }
            RadioSeed::Stream { id: r, .. } => match r.service {
                Service::Youtube => calls.push((Call::Radio(r.id), seed_name)),
                Service::Deezer => calls.push((Call::Related(Service::Deezer, r.id), seed_name)),
                Service::Soundcloud => calls.push((Call::Related(Service::Soundcloud, r.id), seed_name)),
                Service::Spotify => {}
            },
            RadioSeed::Artist(a) => calls.extend(artist_call(self, a, &[Service::Deezer, Service::Youtube])),
            // A list: the YouTube radio of its first song there, and its
            // commonest artist's on Deezer.
            RadioSeed::Tracks(tracks) => {
                let yt = tracks.iter().take(8).find_map(|t| {
                    let refs = self.lib.stream_refs(*t).unwrap_or_default();
                    let r = refs.into_iter().find(|r| r.id.service == Service::Youtube)?;
                    Some((r.id.id, self.lib.get_track(*t).ok().flatten().map(|t| t.title).unwrap_or_default()))
                });
                calls.extend(yt.map(|(id, title)| (Call::Radio(id), title)));
                // In the list's order: the first of the commonest.
                let mut count: Vec<(ArtistId, usize)> = Vec::new();
                for t in &tracks {
                    if let Some(a) = self.lib.get_track(*t).ok().flatten().and_then(|t| t.artist_credit.artist_ids().next()) {
                        match count.iter_mut().find(|(b, _)| *b == a) {
                            Some((_, n)) => *n += 1,
                            None => count.push((a, 1)),
                        }
                    }
                }
                let most = count.iter().map(|(_, n)| *n).max().unwrap_or(0);
                if let Some(&(a, _)) = count.iter().find(|(_, n)| *n == most) {
                    calls.extend(artist_call(self, a, &[Service::Deezer]));
                }
            }
            RadioSeed::Mood(_) | RadioSeed::Style { .. } | RadioSeed::Discover | RadioSeed::Daylist { .. } | RadioSeed::Wave(_) => {}
        }
        for (c, _) in &calls {
            if let Call::Radio(v) | Call::Related(_, v) = c
                && let Some(run) = self.recs.radio.run.as_mut()
            {
                run.asked.insert(v.clone());
            }
        }
        self.radio_ask(id, calls);
    }

    /// Asks `calls` for new music for radio `id`: the cache first, the rest
    /// from services that can be asked now, in the background.
    /// Each call goes with the name of what it asks around (why its finds
    /// are offered).
    pub(super) fn radio_ask(&mut self, id: u64, calls: Vec<(Call, String)>) {
        // The cache first; the rest from services that can be asked now.
        let now = nikgob_library::now_ms();
        let mut cached = Vec::new();
        let mut via: Vec<(RemoteId, nikgob_core::i18n::PerLocale<String>)> = Vec::new();
        let mut asks: Vec<(Arc<dyn StreamService>, Call, String, nikgob_core::i18n::PerLocale<String>, String)> = Vec::new();
        for (call, seed) in calls {
            let (service, key) = match &call {
                Call::Radio(v) => (Service::Youtube, cache_key(Service::Youtube, "radio", v)),
                Call::Related(s, t) => (*s, cache_key(*s, "related", t)),
                Call::ArtistRadio { service, artist, .. } => (*service, cache_key(*service, "artist", &artist.to_string())),
            };
            let hit: Option<Vec<StreamTrack>> = self
                .lib
                .cache_get(&key)
                .ok()
                .flatten()
                .filter(|(at, _)| now - at < NEW_TTL_MS)
                .and_then(|(_, json)| serde_json::from_str(&json).ok());
            let why = nikgob_core::i18n::PerLocale::init(|loc| nikgob_recs::explain::new_found(&seed, service.display_name(), loc));
            if let Some(tracks) = hit {
                via.extend(tracks.iter().map(|t| (t.id.clone(), why.clone())));
                cached.extend(tracks);
                continue;
            }
            if self.pause_left(service).is_some() {
                continue;
            }
            // The listener's YouTube Music first: held back, asked later.
            if self.background_wait(service).is_some() {
                self.radio_defer(id, vec![(call, seed)]);
                continue;
            }
            if let Some(svc) = self.service(service) {
                asks.push((svc, call, key, why, seed));
            }
        }
        if !cached.is_empty() {
            if let Some(run) = self.recs.radio.run.as_mut().filter(|r| r.id == id) {
                run.via.extend(via);
            }
            self.radio_add_new(cached);
            self.radio_splice_new();
        }
        if asks.is_empty() {
            return;
        }
        let listening = self.streaming.held.listening.clone();
        self.streaming.spawn_internal(async move {
            let mut tracks = Vec::new();
            let mut artists = Vec::new();
            let mut answers = Vec::new();
            let mut via = Vec::new();
            let mut held = Vec::new();
            for (svc, call, key, why, seed) in asks {
                // The listener's YouTube Music began meanwhile: later.
                if svc.service() == Service::Youtube && listening.load(Ordering::Relaxed) {
                    held.push((call, seed));
                    continue;
                }
                let Some((result, matched)) = fetch(svc, &call, &listening).await else {
                    held.push((call, seed));
                    continue;
                };
                artists.extend(matched);
                match result {
                    Ok(found) => {
                        answers.push((key, found.clone()));
                        via.extend(found.iter().map(|t| (t.id.clone(), why.clone())));
                        tracks.extend(found);
                    }
                    Err(e) => log::info!("radio: new music from {key}: {e}"),
                }
            }
            Internal::Recs(Msg::Radio(RadioMsg::NewFound {
                id,
                tracks,
                held,
                artists,
                answers,
                via,
            }))
        });
    }
}
