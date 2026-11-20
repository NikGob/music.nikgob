use super::snapshot::{BuildInput, ReadExtra, build};
use super::*;

impl Worker {
    /// The home's epoch now: this run's, of this part of the day.
    pub(crate) fn home_epoch(&self) -> crate::home_v2::Epoch {
        crate::home_v2::Epoch::of(self.recs.run, recs_now(), local_day().1)
    }

    /// The recommendation parts of the home page as they stand, without
    /// their rows: the home's builder reads those, resolves the covers and
    /// makes the new-music rows ([`Self::discover_input`]).
    pub(crate) fn recs_home_parts(&self) -> HomeParts {
        let (loc, local_only, card) = {
            let p = &self.state.read().prefs;
            (p.resolved_locale(), p.mixes_downloaded_only, p.home_card_shown())
        };
        HomeParts::of_in(loc, self.recs.home.as_ref(), Vec::new(), Vec::new(), local_only, card)
    }

    /// Draws the recommendation parts of the home page again (built off
    /// the loop, [`crate::home_build`]).
    pub(crate) fn publish_recs_home(&mut self) {
        self.ask_home(false);
    }

    /// «Не предлагать этот микс»: its card goes at once (another takes its
    /// place with the build), «Отменить» brings it back.
    pub(crate) fn mix_hidden(&mut self, m: v::HiddenMix) {
        if let Some(h) = self.recs.home.as_mut() {
            h.lanes.retain(|l| l.key != m.key);
        }
        let key = m.key;
        self.patch_home(move |home| home.lanes.retain(|l| l.key != key));
        let undo = self.undo_action(Command::SetPref(v::PrefPatch::ShowMix(m.key)));
        let text = format!("{} {}", activity_in(self.locale()).not_recommend_mix, fmt_quoted_in(self.locale(), &m.title));
        self.toast_info(text, Some(undo));
    }

    /// «Миксы для тебя» scrolled to its end: [`ROW_MORE`] more cards, built
    /// now, while the last build filled the row it was asked for (else
    /// there are no more).
    pub(crate) fn recs_more_mixes(&mut self) {
        let Some(home) = self.recs.home.as_ref() else { return };
        let shown = home.lanes.iter().filter(|l| l.key != v::MixKey::OnRepeat).count();
        if shown < nikgob_recs::mixes::ROW_MIXES + self.recs.row_more {
            return;
        }
        self.recs.row_more += ROW_MORE;
        self.recs_dirty(Duration::ZERO);
        if !self.recs.building {
            self.recs_build();
        }
    }

    /// A build is due after `after` (sooner if one is due already).
    pub(crate) fn recs_dirty(&mut self, after: Duration) {
        let at = Instant::now() + after;
        let r = &mut self.recs;
        r.due = Some(r.due.map_or(at, |d| d.min(at)));
        r.urgent |= after.is_zero();
    }

    /// «Миксы только из скачанного» changed: the home is built again now
    /// (its kept picks are picked again under the new setting).
    pub(crate) fn recs_prefs_changed(&mut self) {
        self.recs_dirty(Duration::ZERO);
        if !self.recs.building {
            self.recs_build();
        }
    }

    /// The library changed (likes, playlists, imports): a build once it has
    /// been quiet for the debounce, so a run of likes or playlist edits
    /// builds once, yet at most ten debounces after the first of them.
    pub(crate) fn recs_library_changed(&mut self) {
        let d = self.streaming.schedule.recs_debounce;
        let now = Instant::now();
        let r = &mut self.recs;
        let first = *r.changed_since.get_or_insert(now);
        let at = (now + d).min(first + d * 10);
        // A build already due (or due before this run began) stays so.
        if !r.due.is_some_and(|due| due <= now || due < first) {
            r.due = Some(at);
        }
    }

    /// A track ended: a build after enough of them.
    pub(crate) fn recs_played(&mut self) {
        self.recs.plays += 1;
        if self.recs.plays >= PLAYS_PER_BUILD {
            self.recs.plays = 0;
            let d = self.streaming.schedule.recs_debounce;
            self.recs_dirty(d);
        }
    }

    /// A start, `origin` its chooser: the session goes on, and a start the
    /// user chose teaches the model of the next start.
    pub(crate) fn recs_started(&mut self, track: TrackId, origin: nikgob_core::model::EventOrigin) {
        let track = self.recs_song_of(track);
        let now = recs_now();
        let ctx = self.play_context;
        let r = &mut self.recs;
        if r.session.last_ts().is_none_or(|l| now - l >= nikgob_recs::taste::SESSION_GAP_MS) {
            r.session = Session::default();
        }
        r.skip_x = None;
        if let Some(snap) = r.snap.clone()
            && let Some(i) = snap.model.position(track)
        {
            let model = &snap.model;
            let m = model.moment(&r.session, now);
            r.skip_x = Some((track, model.skip_features(&m, i, ctx)));
            if m.prev.is_some() && nikgob_recs::model::teaches(Some(origin), ctx) {
                let w = r.weights.get_or_insert(model.weights);
                model.teach_next(&mut w.next, &m, i);
                self.recs_keep_weights();
            }
        }
        if self.recs.skip_x.is_none()
            && let Some(x) = self.new_started(track, origin)
        {
            self.recs.skip_x = Some((track, x));
        }
        self.recs.session.start(track, now, ctx, Some(origin));
    }

    /// How a start ended (not by closing the app), where, and its length
    /// when known: the session and the skip model learn it.
    pub(crate) fn recs_ended(&mut self, track: TrackId, complete: bool, position_ms: u32, duration_ms: Option<u32>) {
        let track = self.recs_song_of(track);
        // A short track's outro is no early skip.
        let ended = if complete { Ended::Complete } else { nikgob_recs::session::skip_ended(position_ms, duration_ms) };
        self.new_ended(track, ended);
        let r = &mut self.recs;
        r.session.end_at(track, ended, position_ms, duration_ms, None);
        let Some((t, x)) = r.skip_x.take() else { return };
        let Some(snap) = r.snap.clone() else { return };
        // What it teaches the skip model («not now» of a loved track:
        // nothing, it was the mood).
        let lesson = r.session.last_of(track).and_then(|p| Session::skip_lesson(p, &snap.model));
        let Some(skipped) = lesson.filter(|_| t == track) else { return };
        r.weights.get_or_insert(snap.model.weights).skip.observe(&x, skipped);
        self.recs_keep_weights();
    }

    /// The bar of the playing `track` was dragged from `from_ms` to
    /// `to_ms`: a skip soon after it is a scrub-skip
    /// ([`nikgob_recs::session::Session::seek`]).
    pub(crate) fn recs_seeked(&mut self, track: TrackId, from_ms: u32, to_ms: u32) {
        let track = self.recs_song_of(track);
        self.recs.session.seek(track, Some(from_ms), to_ms);
    }

    /// The user liked `track`: its start in the session is approved, and a
    /// radio playing it takes its neighbours in.
    pub(crate) fn recs_liked(&mut self, track: TrackId) {
        let song = self.recs_song_of(track);
        self.recs.session.like(song);
        self.radio_liked(track);
    }

    /// The catalog's track for `track`: itself, or the copy standing for
    /// its recording when it is another copy of one song on two albums
    /// (the session holds one id per song: «Не повторять сыгранное»
    /// covers both).
    fn recs_song_of(&self, track: TrackId) -> TrackId {
        self.recs.snap.as_ref().and_then(|s| s.catalog.get(track)).map_or(track, |t| t.id)
    }

    /// The weights learnt online, into `recs.model`: at most every
    /// [`KEEP_WEIGHTS_EVERY`] (a start and an end write nothing more on the
    /// loop); what is learnt between goes with the next write, a tick or
    /// the app's close ([`Self::recs_flush_weights`]).
    fn recs_keep_weights(&mut self) {
        if self.recs.weights_kept.is_some_and(|t| t.elapsed() < KEEP_WEIGHTS_EVERY) {
            self.recs.weights_unkept = true;
            return;
        }
        self.recs_write_weights();
    }

    fn recs_write_weights(&mut self) {
        self.recs.weights_kept = Some(Instant::now());
        self.recs.weights_unkept = false;
        if let Some(w) = &self.recs.weights
            && let Ok(json) = serde_json::to_string(w)
            && let Err(e) = self.lib.cache_put(MODEL_KEY, now_ms(), &json)
        {
            log::warn!("recs: cannot keep the model: {e}");
        }
    }

    /// Writes the weights learnt since the last write (`now`: whenever they
    /// wait; else once [`KEEP_WEIGHTS_EVERY`] is over).
    pub(crate) fn recs_flush_weights(&mut self, now: bool) {
        if self.recs.weights_unkept && (now || self.recs.weights_kept.is_none_or(|t| t.elapsed() >= KEEP_WEIGHTS_EVERY)) {
            self.recs_write_weights();
        }
    }

    /// Every tick: a build when one is due, the discovery job.
    pub(crate) fn recs_tick(&mut self) {
        self.recs_flush_weights(false);
        let now = Instant::now();
        let every = self.streaming.schedule.recs_every;
        let (day, utc_offset_min) = local_day();
        let (_, _, daylist) = daylist_key(recs_now(), utc_offset_min);
        let r = &self.recs;
        let grown = {
            let (len, built) = (self.analysis.index().read().len(), r.built_index_len);
            len > built && (len - built >= NEW_VECTORS.max(built / 10) || len >= 2 * built)
        };
        let stale = r.built_at.is_some_and(|t| t.elapsed() >= every)
            || r.home.as_ref().is_some_and(|h| h.day != day || h.daylist_key != daylist)
            || (r.built_at.is_some() && grown);
        // Nobody looks at the recommendations while a game is in front:
        // they are built, and new music looked for, when it is over.
        if crate::quiet::busy() {
            return;
        }
        let rested = r.urgent || r.built_at.is_none_or(|t| t.elapsed() >= (r.build_took * REST).min(MAX_REST));
        if !r.building && rested && (r.due.is_some_and(|d| now >= d) || stale) {
            self.recs_build();
        }
        self.discover_tick();
        self.suggest_tick();
        self.graph_tick();
        self.niche_tick();
    }

    /// The run's first build at once, once the sound vectors are in memory
    /// (no network pass with it: those wait for the tick), so the mixes do
    /// not wait for the first tick. Only in a run that keeps its state (the
    /// app's; a test's builds keep to its ticks), and not while a game is in
    /// front.
    pub(crate) fn recs_first_build(&mut self) {
        if self.prefs_path.is_none() || self.recs.built_at.is_some() || self.recs.building || crate::quiet::busy() {
            return;
        }
        log::info!("recs: the run's first build, before the first tick");
        self.recs_build();
    }

    /// The vectors the sync brought are in memory (a device that does not
    /// analyse): the run's first build, or one soon when anything of the
    /// sound `changed` (a vector new or replaced, a sketch, a vector of
    /// the model).
    pub(crate) fn recs_sound_loaded(&mut self, changed: bool) {
        // A build under way read the index before these came.
        if self.recs.built_at.is_none() && !self.recs.building {
            self.recs_first_build();
        } else if changed {
            self.recs_dirty(SYNCED_SOUND_REST);
        }
    }

    fn recs_build(&mut self) {
        // The last pass's finds, before the run's first build takes them.
        self.discover_load();
        let (day, utc_offset_min) = local_day();
        let prefs = self.state.read().prefs.clone();
        self.recs.blends.retain(|b| b.0 == day);
        let blends = self.recs.blends.iter().map(|b| b.1).collect();
        let input = BuildInput {
            lib: self.lib.clone(),
            index: self.analysis.index().clone(),
            day,
            locale: prefs.resolved_locale(),
            epoch: self.home_epoch(),
            utc_offset_min,
            now: recs_now(),
            refresh_daily: std::mem::take(&mut self.recs.refresh_daily),
            refresh_daylist: std::mem::take(&mut self.recs.refresh_daylist),
            new_share: new_share(&prefs, v::RadioKind::Mix),
            local_only: prefs.mixes_downloaded_only,
            new: self.discoveries(),
            new_at: self.discoveries_at(),
            new_pool: self.discoveries_all(),
            new_sources: self.discovery_sources(),
            suggested: self.suggestions_kept(),
            rerank: std::mem::take(&mut self.recs.discover_rerank),
            refresh_mix: self.recs.refresh_mix.take(),
            row_more: self.recs.row_more,
            blends,
            session: self.recs.session.clone(),
            mix_prefs: lanes::MixPrefs::of(&prefs),
            extra: ReadExtra::of(self.discoveries_all(), &prefs),
            new_log: {
                self.new_log_load();
                self.recs.newlog.log.clone()
            },
        };

        let tx = self.recs.tx.clone();
        let spawned = std::thread::Builder::new().name("nikgob-recs".into()).spawn(move || {
            lower_priority();
            let built = build(input);
            let _ = tx.send(Internal::Recs(Msg::Built(Box::new(built))));
            // What the build left free goes back with the thread's heap.
            crate::release_thread_memory();
        });
        match spawned {
            Ok(_) => {
                self.recs.building = true;
                self.recs.build_began = Some(Instant::now());
                self.recs.urgent = false;
                self.recs.due = None;
                self.recs.changed_since = None;
            }
            Err(e) => log::warn!("recs: cannot build: {e}"),
        }
    }

    pub(crate) fn recs_msg(&mut self, m: Msg) {
        match m {
            Msg::Built(b) => self.recs_built(*b),
            Msg::Radio(m) => self.radio_msg(m),
            Msg::Plus(m) => self.plus_msg(m),
            Msg::Discover(m) => self.discover_msg(m),
            Msg::Suggest(m) => self.suggest_msg(m),
            Msg::Graph(m) => self.graph_msg(m),
            Msg::Niche(m) => self.niche_msg(m),
        }
    }

    fn recs_built(&mut self, b: Result<Built, String>) {
        self.recs.building = false;
        self.recs.built_at = Some(Instant::now());
        self.recs.build_took = self.recs.build_began.take().map_or(Duration::ZERO, |t| t.elapsed());
        let rerank = match b {
            Ok(mut b) => {
                let rerank = b.rerank;
                self.recs.built_index_len = b.index_len;
                let built = b.snap.model.weights;
                if self.recs.weights.is_none_or(|w| w.version != built.version || w.next.seen + w.skip.seen <= built.next.seen + built.skip.seen) {
                    self.recs.weights = None;
                }
                let loc = self.state.read().prefs.resolved_locale();
                b.home.relocalize(loc);
                // The run's first, or a new epoch's: the home's shortcuts
                // and «Слушать снова» come with its mixes (the first drawn
                // again with the taste profiles they were drawn without).
                let fresh = self.recs.home.as_ref().is_none_or(|h| h.epoch != b.home.epoch);
                // The build before is let go of off the loop (a large
                // catalog takes a while to free).
                let old = (self.recs.snap.replace(b.snap), self.recs.home.replace(b.home));
                self.drop_off_loop(old);
                self.turn.lap("swap");
                // The whole page with its mixes, or the mixes on the page.
                if fresh {
                    self.load_home();
                } else {
                    self.publish_recs_home();
                }
                self.turn.lap("home asked");
                rerank
            }
            Err(e) => {
                log::warn!("recs: {e}");
                return;
            }
        };
        self.world_mixes_again();
        self.turn.lap("world");
        if let Some(id) = self.recs.pending_play.take() {
            self.play_mix(id);
            self.turn.lap("pending mix");
        }
        self.radio_snapshot_ready();
        self.plus_ready();
        self.turn.lap("radio");
        if rerank {
            self.discover_rerank();
            self.turn.lap("rerank");
        }
        self.discover_tick();
        self.turn.lap("discover");
    }

    /// Synchronously relocalizes cached home recommendations and active wave
    /// scenes after a UI language change (`< 0.2 ms`, no SQLite queries).
    pub(crate) fn relocalize_recs(&mut self, loc: Locale) {
        if let Some(h) = self.recs.home.as_mut() {
            h.relocalize(loc);
        }
        if let Some(snap) = self.snapshot() {
            let needs_scenes = self.recs.radio.scenes.as_ref().is_none_or(|(w, cached_loc, _)| {
                *cached_loc != loc || !std::sync::Weak::ptr_eq(w, &Arc::downgrade(&snap))
            });
            if needs_scenes {
                let profiles = self.recs.home.as_ref().map(|h| h.blend_profiles_in(loc)).unwrap_or_default();
                let list = wave::scenes(&snap, &profiles, loc);
                self.recs.radio.scenes = Some((Arc::downgrade(&snap), loc, list));
            }
        }
        let mut active_changed = self.relocalize_active_radio(loc);
        if !active_changed
            && self.recs.radio.run_id().is_none()
            && self.play_context == EventContext::HomeSection
            && let Some(ru_name) = self.play_context_id.as_deref().and_then(|id| id.strip_prefix("mix:"))
            && let Some(h) = self.recs.home.as_ref()
        {
            if let Some(l) = h.lanes.iter().find(|l| l.ru_title() == ru_name) {
                let mt = play_lane_mix_title(l.key, l.ru_title(), &l.title, l.side_flavor, Some(&l.labels));
                self.queue.set_origin(v::QueueOrigin::Mix(mt));
                active_changed = true;
            } else if let Some(t) = h.tiles.iter().find(|t| t.ru_title() == ru_name) {
                self.queue.set_origin(v::QueueOrigin::Mix(t.mix_title()));
                active_changed = true;
            }
        }
        if active_changed || self.wave_seed().is_some() || !self.recs.why.is_empty() {
            self.publish_player();
        }
    }

    /// Test hook: the online metadata the graph's pass asks.
    pub(crate) fn set_graph_metadata(&mut self, m: nikgob_metadata::Metadata) {
        self.recs.graph.metadata = Some(m);
        self.graph_now();
    }

    pub(crate) fn snapshot(&self) -> Option<Arc<Snapshot>> {
        self.recs.snap.clone()
    }

    /// Recommendation commands; anything else is handed back.
    pub(crate) fn recs_command(&mut self, c: Command) -> Option<Command> {
        match c {
            Command::PlayMix { id } => self.play_mix(id),
            Command::StartRadio { seed } => self.start_radio_cmd(seed),
            Command::RecFeedback { target, dislike } => self.rec_feedback(target, dislike),
            Command::Dislike { track, dislike, sure, relike } => self.dislike(track, dislike, sure, relike),
            Command::RefreshMix { id } => self.refresh_mix(id),
            Command::PlayBlend { profiles } => self.play_blend(&profiles),
            Command::PlayWave { preset } => self.play_wave(preset),
            Command::SetWave(filter) => self.set_wave(filter),
            Command::OtherVibe => self.other_vibe(),
            Command::DownloadMix { id } => self.download_mix(id),
            Command::PlayMixNext { id } => self.queue_mix(id, true),
            Command::EnqueueMix { id } => self.queue_mix(id, false),
            other => return Some(other),
        }
        None
    }

    /// `Command::PlayMix`.
    pub(crate) fn play_mix(&mut self, id: v::MixId) {
        if self.recs.snap.is_none() {
            // The first build is on its way.
            self.recs.pending_play = Some(id);
            self.recs_dirty(Duration::ZERO);
            return;
        }
        match id {
            v::MixId::Daily => self.play_daily(),
            v::MixId::Mood(m) => {
                let seed = nikgob_recs::RadioSeed::Mood(from_view_mood(m));
                self.start_radio(seed, v::RadioTitle::Mix(v::MixTitle::Mood(m.kind())), EventContext::HomeSection, None);
            }
            v::MixId::Style(i) => {
                let style = self
                    .recs
                    .home
                    .as_ref()
                    .and_then(|h| h.styles.get(usize::from(i)))
                    .map(|s| (s.mix.seed(), s.artist_names.clone(), s.mix.artists.len()));
                match style {
                    Some((seed, artists, total_artists)) => {
                        let mt = v::MixTitle::Localized(PerLocale::init(|loc| style_title(&artists, total_artists, loc)));
                        self.start_radio(seed, v::RadioTitle::Mix(mt), EventContext::HomeSection, None);
                        self.set_run_mix_naming(radio::ActiveMixNaming::Style { artists, total_artists });
                    }
                    None => self.toast_err(nikgob_core::i18n::errors_in(self.locale()).mix_gone_home, None),
                }
            }
            v::MixId::Artist(a) => {
                let name = self.lib.get_artist(a).ok().flatten().map_or_else(String::new, |a| a.name);
                let seed = nikgob_recs::RadioSeed::Artist(a);
                self.start_radio(seed, v::RadioTitle::Seed(name), EventContext::HomeSection, None);
            }
            v::MixId::Discover => {
                if self.state.read().prefs.mixes_downloaded_only {
                    self.toast_err(nikgob_core::i18n::errors_in(self.locale()).downloaded_only_no_new, None);
                    return;
                }
                if self.discoveries().is_empty() {
                    self.toast_err(nikgob_core::i18n::errors_in(self.locale()).discover_still_loading, None);
                    return;
                }
                let seed = nikgob_recs::RadioSeed::Discover;
                self.start_radio(seed, v::RadioTitle::Discover, EventContext::HomeSection, None);
            }
            v::MixId::Daylist => {
                let daylist = self.recs.home.as_ref().and_then(|h| h.daylist.as_ref());
                let Some(d) = daylist else {
                    self.toast_nothing_to_play();
                    return;
                };
                let (state, picks, title, part, day) = (d.state.clone(), d.picks.clone(), d.title.clone(), d.part, d.day);
                let centre = d.centre.or_else(|| match state.seed {
                    nikgob_recs::RadioSeed::Daylist { centre, .. } => Some(centre),
                    _ => None,
                });
                let mt = match centre {
                    Some(c) => v::MixTitle::Localized(PerLocale::init(|loc| {
                        daylist_title_in(loc, Axes::from_array(c), part, day)
                    })),
                    None => v::MixTitle::Custom(title),
                };
                let streams = d.streams.values().cloned().collect();
                self.start_radio_picked(state, picks, streams, v::RadioTitle::Mix(mt), EventContext::HomeSection);
                if let Some(centre) = centre {
                    self.set_run_mix_naming(radio::ActiveMixNaming::Daylist { part, centre, day });
                }
            }
            v::MixId::Lane(v::MixKey::World(kind)) => {
                let world_title = self.state.read().world.as_ref().map(|w| w.world.title.clone());
                let mix = self.recs.world.as_ref().and_then(|(_, m)| m.iter().find(|m| m.0 == kind));
                match mix.map(|m| (m.1.clone(), m.3.iter().map(|&id| Pick { item: Item::Track(id), reason: Reason::Random }).collect::<Vec<_>>())) {
                    Some((ru_title, picks)) => {
                        let mt = world_title
                            .map(|world| v::MixTitle::World { world: Some(world), mix: kind.kind() })
                            .unwrap_or_else(|| v::MixTitle::World { world: None, mix: kind.kind() });
                        self.play_picks(&picks, &HashMap::new(), &ru_title, mt);
                    }
                    None => self.toast_err(nikgob_core::i18n::errors_in(self.locale()).mix_gone_world, None),
                }
            }
            v::MixId::Lane(key) => {
                let with_new = {
                    let p = &self.state.read().prefs;
                    p.mixes_with_new && !p.mixes_downloaded_only
                };
                let lane = self.recs.home.as_ref().and_then(|h| h.lanes.iter().find(|l| l.key == key));
                match lane.map(|l| {
                    let mt = play_lane_mix_title(key, l.ru_title(), &l.title, l.side_flavor, Some(&l.labels));
                    (l.picks(with_new), l.new.clone(), l.ru_title().to_owned(), mt)
                }) {
                    Some((picks, new, ru_title, mt)) => {
                        let streams = new.into_iter().map(|t| (t.id.clone(), t)).collect();
                        self.play_picks(&picks, &streams, &ru_title, mt);
                    }
                    None => self.toast_err(nikgob_core::i18n::errors_in(self.locale()).mix_gone_home, None),
                }
            }
            v::MixId::Tile(v::TileKind::Lottery) => self.play_lottery(),
            v::MixId::Tile(kind) => match self.home_tile(kind).map(|t| (t.picks.clone(), t.ru_title(), t.mix_title())) {
                Some((picks, ru_title, mt)) => {
                    self.play_picks(&picks, &HashMap::new(), &ru_title, mt);
                }
                None => self.toast_err(nikgob_core::i18n::errors_in(self.locale()).tile_gone_home, None),
            },
        }
    }

    /// Keeps why each of `entries` was picked (`whys` beside them).
    pub(crate) fn explain_entries(&mut self, entries: &[u64], whys: Vec<Option<nikgob_core::i18n::PerLocale<String>>>) {
        for (e, why) in entries.iter().zip(whys) {
            if let Some(why) = why {
                self.recs.why.insert(*e, why);
            }
        }
    }

    /// The mixes of the page of `world`: «Лучшее», «Ещё не
    /// слышал», «В дорогу», then its calm and its lively side, those it has;
    /// none before the first build. Kept for [`v::MixKey::World`] to play.
    pub(crate) fn world_mix_cards(&mut self, world: &v::World) -> Vec<v::LaneCard> {
        let Some(snap) = self.recs.snap.clone() else {
            self.recs.world = None;
            return Vec::new();
        };
        let (part, _, _) = daylist_key(recs_now(), local_day().1);
        let token = nikgob_recs::world_token(&world.key);
        let (local_only, loc) = {
            let p = &self.state.read().prefs;
            (p.mixes_downloaded_only, p.resolved_locale())
        };
        let started = Instant::now();
        let recs = snap.recs();
        let lib = &self.lib;
        let title = |i: usize| lib.get_track(snap.model.id(i)).ok().flatten().map(|t| t.title);
        let mut mixes: Vec<(v::WorldMix, Vec<TrackId>, Option<String>)> = recs
            .world_mixes(token, part)
            .into_iter()
            .map(|m| {
                let kind = match m.kind {
                    nikgob_recs::WorldMixKind::Best => v::WorldMix::Best,
                    nikgob_recs::WorldMixKind::Unheard => v::WorldMix::Unheard,
                    nikgob_recs::WorldMixKind::Road => v::WorldMix::Road,
                    nikgob_recs::WorldMixKind::Calm => v::WorldMix::Calm,
                    nikgob_recs::WorldMixKind::Moving => v::WorldMix::Moving,
                };
                let why = recs.why_world_mix(&m, &world.title, &title, loc);
                (kind, m.tracks.iter().map(|&i| snap.model.id(i)).collect(), why)
            })
            .collect();
        mixes.sort_by_key(|m| m.0);
        log::debug!("world mixes of «{}» in {:?}", world.title, started.elapsed());
        let mut kept = Vec::new();
        let mut cards = Vec::new();
        for (kind, ids, why) in mixes {
            let rows: Vec<nikgob_library::TrackRow> =
                ids.iter().filter_map(|id| self.lib.get_track_row(*id).ok().flatten()).filter(|t| t.has_local || !local_only).collect();
            if rows.is_empty() {
                continue;
            }
            // Each credited artist once, the most frequent first (as a mix of
            // Главная says them).
            let mut count: Vec<(String, usize)> = Vec::new();
            let mut releases: Vec<ReleaseId> = Vec::new();
            for t in &rows {
                for part in t.credit.0.iter().filter(|p| !p.name.is_empty()) {
                    match count.iter_mut().find(|c| c.0 == part.name) {
                        Some(c) => c.1 += 1,
                        None => count.push((part.name.clone(), 1)),
                    }
                }
                if let Some(r) = t.release_id
                    && !releases.contains(&r)
                {
                    releases.push(r);
                }
            }
            count.sort_by(|a, b| b.1.cmp(&a.1));
            let artists: Vec<String> = count.into_iter().map(|c| c.0).collect();
            let covers = releases.into_iter().filter_map(|r| self.res.release_cover(r)).take(COVERS).collect();
            let name = kind.title_in(loc);
            let mt = v::MixTitle::World { world: Some(world.title.clone()), mix: kind.kind() };
            let ru_queue = mt.title_in(Locale::Ru).into_owned();
            let queue = mt.title_in(loc).into_owned();
            kept.push((kind, ru_queue, queue.clone(), rows.iter().map(|t| t.id).collect()));
            cards.push(v::LaneCard {
                key: v::MixKey::World(kind),
                title: name.to_owned(),
                mix_title: Some(mt),
                short_titles: Vec::new(),
                artists,
                new_artists: Vec::new(),
                covers,
                new_cover: None,
                tracks: rows.iter().map(|t| self.res.track_in(t, loc)).collect(),
                new_tracks: Vec::new(),
                blend_base: None,
                queue: Some(queue),
                why,
            });
        }
        self.recs.world = Some((world.key.clone(), kept));
        cards
    }

    /// The page of a world open gets its mixes again (a build is done).
    fn world_mixes_again(&mut self) {
        let world = self.state.read().world.as_ref().map(|w| w.world.clone());
        let Some(world) = world else { return };
        let cards = self.world_mix_cards(&world);
        self.publish(|s| {
            if let Some(w) = s.world.as_mut().filter(|w| w.world.key == world.key) {
                w.mixes = cards;
            }
        });
    }

    /// «Наугад»: a fresh draw at every click, so what plays (the first song
    /// too) cannot be seen before.
    fn play_lottery(&mut self) {
        let Some(snap) = self.recs.snap.clone() else { return };
        let picks = snap.recs().lottery(fastrand::u64(..));
        if picks.is_empty() {
            self.toast_nothing_to_play();
            return;
        }
        self.play_picks(&picks, &HashMap::new(), TileStatic::Lottery.title_in(Locale::Ru), v::MixTitle::Tile(TileStatic::Lottery));
    }

    /// A tile of today's board.
    fn home_tile(&self, kind: v::TileKind) -> Option<&tiles::TileHome> {
        self.recs.home.as_ref()?.tiles.iter().find(|t| t.mix_id() == v::MixId::Tile(kind))
    }

    /// `Command::DownloadMix`: every song of a tile (or of the daylist,
    /// which stands among the tiles) without a file goes to «Хочу скачать».
    /// Its songs of the services too (the daylist's new ones), made tracks
    /// of their own as playing them does.
    fn download_mix(&mut self, id: v::MixId) {
        let Some((picks, streams, _)) = self.day_mix(id) else {
            self.toast_err(nikgob_core::i18n::errors_in(self.locale()).tile_gone_home, None);
            return;
        };
        let mut ids = Vec::with_capacity(picks.len());
        for p in pick_tracks(&picks, &streams) {
            match p {
                PickTrack::Library(id) => ids.push(id),
                PickTrack::Stream(t) => {
                    self.streaming.learn(std::slice::from_ref(t));
                    ids.extend(self.upsert_streams(std::slice::from_ref(t)));
                }
            }
        }
        if ids.is_empty() {
            self.toast_err(nikgob_core::i18n::errors_in(self.locale()).nothing_to_download, None);
            return;
        }
        self.download_tracks(&ids);
    }

    /// The picks of a tile of the day or of the daylist, with the details
    /// of the daylist's songs of the services, and its title; `None` once it
    /// is gone from Главная.
    fn day_mix(&self, id: v::MixId) -> Option<(Vec<Pick>, HashMap<RemoteId, StreamTrack>, String)> {
        match id {
            v::MixId::Tile(kind) => self.home_tile(kind).map(|t| (t.picks.clone(), HashMap::new(), t.title.clone())),
            v::MixId::Daylist => {
                let d = self.recs.home.as_ref()?.daylist.as_ref()?;
                Some((d.picks.clone(), d.streams.clone(), d.title.clone()))
            }
            _ => None,
        }
    }

    /// `Command::PlayMixNext` / `Command::EnqueueMix` of a tile of the day
    /// or the daylist: its songs as playing it would take them (not the
    /// radio the daylist goes on with), after the current track (`next`)
    /// or at the end of the queue.
    fn queue_mix(&mut self, id: v::MixId, next: bool) {
        let Some((picks, streams, _)) = self.day_mix(id) else {
            self.toast_err(nikgob_core::i18n::errors_in(self.locale()).tile_gone_home, None);
            return;
        };
        let prefs = self.state.read().prefs.clone();
        let snap = self.snapshot();
        let mut ids = Vec::with_capacity(picks.len());
        for p in pick_tracks(&picks, &streams) {
            match p {
                PickTrack::Library(id) => {
                    if radio::radio_allows(id, false, false, &prefs, snap.as_deref(), v::RadioKind::Mix) {
                        ids.push(id);
                    }
                }
                PickTrack::Stream(_) if prefs.mixes_downloaded_only => {}
                PickTrack::Stream(t) => {
                    self.streaming.learn(std::slice::from_ref(t));
                    ids.extend(self.upsert_streams(std::slice::from_ref(t)));
                }
            }
        }
        if ids.is_empty() {
            self.toast_nothing_to_play();
            return;
        }
        self.command(if next { Command::PlayNext(ids) } else { Command::Enqueue(ids) });
    }

    /// «Твой микс» as it is today: its tracks, not endless.
    fn play_daily(&mut self) {
        let Some(h) = &self.recs.home else { return };
        let picks = h.daily.clone();
        let streams = h.daily_streams.clone();
        self.play_picks(&picks, &streams, home_recs_in(Locale::Ru).your_mix, v::MixTitle::YourMix);
    }

    /// A mix of the home as it is (not endless), from its first track.
    /// `context_title` must be locale-invariant (`play_context_id = "mix:{context_title}"`).
    fn play_picks(&mut self, picks: &[Pick], streams: &HashMap<RemoteId, StreamTrack>, context_title: &str, mix_title: v::MixTitle) {
        // «Только скачанное» holds even for a mix picked before it was on.
        let prefs = self.state.read().prefs.clone();
        let snap = self.snapshot();
        let mut ids = Vec::with_capacity(picks.len());
        // New music says where it was found.
        let mut whys = Vec::with_capacity(picks.len());
        for p in picks {
            match &p.item {
                Item::Track(id) => {
                    if radio::radio_allows(*id, false, false, &prefs, snap.as_deref(), v::RadioKind::Mix) {
                        ids.push(*id);
                        whys.push(None);
                    }
                }
                Item::Stream(_) if prefs.mixes_downloaded_only => {}
                Item::Stream(r) => {
                    if let Some(t) = streams.get(r) {
                        self.streaming.learn(std::slice::from_ref(t));
                        for id in self.upsert_streams(std::slice::from_ref(t)) {
                            ids.push(id);
                            whys.push(self.discovery_why(t));
                        }
                    }
                }
            }
        }
        if ids.is_empty() {
            self.toast_nothing_to_play();
            return;
        }
        self.user_switch(nikgob_core::model::EventOrigin::Mix);
        // Shuffled or in its order, as the setting of mixes says.
        self.new_list(Some(v::ListKind::Mix));
        self.queue.set_from(&ids, None, v::QueueOrigin::Mix(mix_title));
        // The entries in the mix's own order, as `whys` are.
        let entries: Vec<u64> = self.queue.context_order().iter().map(|e| e.id).collect();
        self.recs.why.clear();
        self.explain_entries(&entries, whys);
        self.play_context = EventContext::HomeSection;
        self.play_context_id = Some(format!("mix:{context_title}"));
        if let Some(e) = self.queue.current() {
            self.start(e);
        }
    }

    /// `Command::PlayBlend`: «Смешать с…». The blend is drawn now from the
    /// snapshot, plays, and stands first in «Миксы для тебя» until the
    /// day's end (the next builds draw it again the same).
    fn play_blend(&mut self, profiles: &[u32]) {
        let Some(snap) = self.recs.snap.clone() else {
            self.toast_err(nikgob_core::i18n::errors_in(self.locale()).recs_still_building, None);
            return;
        };
        let (prefs, loc) = {
            let p = &self.state.read().prefs;
            (lanes::MixPrefs::of(p), p.resolved_locale())
        };
        let lib = self.lib.clone();
        let names = |a: nikgob_core::model::ArtistId| lib.get_artist(a).ok().flatten().map(|a| a.name);
        let epoch = self.home_epoch();
        let Some(lane) = lanes::blend_now(&snap, &prefs, profiles, &names, &self.recs.session, epoch.seed, loc) else {
            self.toast_err(nikgob_core::i18n::errors_in(self.locale()).blend_impossible, None);
            return;
        };
        let (day, _) = local_day();
        lanes::remember_blend(&self.lib, &prefs, &day, &epoch.key, recs_now(), lane.key);
        self.recs.blends.retain(|b| b.0 == day && b.1 != lane.key);
        self.recs.blends.push((day, lane.key));
        let (picks, ru_title, mt) = (
            lane.picks(false),
            lane.ru_title().to_owned(),
            play_lane_mix_title(lane.key, lane.ru_title(), &lane.title, lane.side_flavor, Some(&lane.labels)),
        );
        if let Some(h) = self.recs.home.as_mut() {
            h.lanes.retain(|l| l.key != lane.key);
            let at = lanes::blend_place(&h.lanes);
            h.lanes.insert(at, lane);
            // As the next build has it: the latest blends in the places of
            // the moments that fit now least.
            let blend = |l: &lanes::LaneHome| matches!(l.key, v::MixKey::Blend(_));
            while h.lanes.iter().filter(|l| blend(l)).count() > nikgob_recs::mixes::BLENDS_SHOWN {
                if let Some(k) = h.lanes.iter().rposition(|l| blend(l)) {
                    h.lanes.remove(k);
                }
            }
            while h.lanes.iter().filter(|l| lanes::is_moment(l.key)).count() > nikgob_recs::mixes::MOMENTS {
                if let Some(k) = h.lanes.iter().rposition(|l| lanes::is_moment(l.key) && !blend(l)) {
                    h.lanes.remove(k);
                } else {
                    break;
                }
            }
        }
        self.publish_recs_home();
        self.play_picks(&picks, &HashMap::new(), &ru_title, mt);
    }

    /// `Command::RefreshMix`.
    fn refresh_mix(&mut self, id: v::MixId) {
        match id {
            v::MixId::Daily => {
                self.recs.refresh_daily = true;
                self.recs_dirty(Duration::ZERO);
                if !self.recs.building {
                    self.recs_build();
                }
            }
            v::MixId::Discover => self.discover_now(),
            v::MixId::Daylist => {
                self.recs.refresh_daylist = true;
                self.recs_dirty(Duration::ZERO);
                if !self.recs.building {
                    self.recs_build();
                }
            }
            // «На повторе» is what played: nothing to draw again.
            v::MixId::Lane(v::MixKey::OnRepeat) => {}
            v::MixId::Lane(key) => {
                self.recs.refresh_mix = Some(key);
                self.recs_dirty(Duration::ZERO);
                if !self.recs.building {
                    self.recs_build();
                }
            }
            // Moods, styles and artists are endless: nothing to pick again.
            v::MixId::Mood(_) | v::MixId::Style(_) | v::MixId::Artist(_) => {}
            // The tiles are what the history says today.
            v::MixId::Tile(_) => {}
        }
    }

    /// `Command::RecFeedback`: «Не рекомендовать» (and its undo).
    fn rec_feedback(&mut self, target: v::RecTarget, dislike: bool) {
        let mut marks: Vec<(&'static str, String)> = Vec::new();
        let name = match &target {
            v::RecTarget::Track(t) => {
                marks.push(("track", t.to_string()));
                self.lib.get_track(*t).ok().flatten().map(|t| t.title)
            }
            v::RecTarget::Artist(a) => {
                marks.push(("artist", a.to_string()));
                self.lib.get_artist(*a).ok().flatten().map(|a| a.name)
            }
            v::RecTarget::StreamTrack(r) => {
                let t = self.stream_track(r);
                if let Some(t) = &t {
                    marks.push((SONG, stream_song_key(t)));
                }
                // The same song in the library is out too.
                if let Some(id) = self.lib.track_by_remote(r).ok().flatten() {
                    marks.push(("track", id.to_string()));
                }
                t.map(|t| t.title)
            }
            v::RecTarget::StreamArtist(name) => {
                let key = artist_key(name);
                if !key.is_empty() {
                    marks.push((ARTIST_NAME, key));
                }
                Some(name.clone())
            }
        };
        if marks.is_empty() {
            log::warn!("rec feedback for an unknown {target:?}");
            return;
        }
        let loc = self.locale();
        let s = activity_in(loc);
        for (kind, id) in &marks {
            if self.log_err(s.feedback_err, self.lib.set_rec_feedback(kind, id, dislike)).is_none() {
                return;
            }
        }
        let artist = matches!(target, v::RecTarget::Artist(_) | v::RecTarget::StreamArtist(_));
        let name = name.unwrap_or_default();
        if dislike {
            let text = if artist {
                format!("{} {}", s.not_recommend_artist, fmt_quoted_in(loc, &name))
            } else {
                format!("{} {}", s.not_recommend_name, fmt_quoted_in(loc, &name))
            };
            let undo = self.undo_action(Command::RecFeedback { target, dislike: false });
            self.toast_info(text, Some(undo));
        } else {
            let undo = self.undo_action(Command::RecFeedback { target, dislike: true });
            let text = format!("{} {}", fmt_quoted_in(loc, &name), s.back_in_recs);
            self.toast_info(text, Some(undo));
        }
        // Discoveries follow at once; the mixes after the next build.
        self.dislikes_load(true);
        self.publish_recs_home();
        self.recs_dirty(Duration::ZERO);
    }

    /// A stream track the runtime knows by its id (a search, a page, a
    /// discovery).
    pub(crate) fn stream_track(&self, r: &RemoteId) -> Option<StreamTrack> {
        self.streaming
            .known
            .get(r)
            .cloned()
            .or_else(|| self.discoveries().into_iter().find(|t| t.id == *r))
            .or_else(|| self.recs.home.as_ref().and_then(|h| h.daily_streams.get(r).cloned()))
            .or_else(|| {
                let d = self.recs.home.as_ref()?.daylist.as_ref()?;
                d.streams.get(r).cloned()
            })
    }
}

/// A pick of a home mix as a track to take: a library one, or a song of
/// the services by its details.
#[derive(Debug, PartialEq)]
pub(super) enum PickTrack<'a> {
    Library(TrackId),
    Stream(&'a StreamTrack),
}

/// `picks` in their order as tracks: a song of the services by its
/// details in `streams` (left out without them).
pub(super) fn pick_tracks<'a>(picks: &[Pick], streams: &'a HashMap<RemoteId, StreamTrack>) -> Vec<PickTrack<'a>> {
    picks
        .iter()
        .filter_map(|p| match &p.item {
            Item::Track(id) => Some(PickTrack::Library(*id)),
            Item::Stream(r) => streams.get(r).map(PickTrack::Stream),
        })
        .collect()
}
