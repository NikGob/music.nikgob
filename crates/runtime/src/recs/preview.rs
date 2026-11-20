use super::snapshot::{BuildInput, build, listening_line, played_in_background, profile_name, profile_order, read_parts};
use super::*;

/// The home of [`preview_home`] and what it was built from.
pub struct HomePreview {
    /// As the player would show it, but «Новое для тебя» and «Новые
    /// релизы» (their rows need the services; see the counts below).
    pub home: v::HomeView,
    /// Tracks the engine may pick (library tracks and liked streams).
    pub catalog: usize,
    /// Sound vectors of the analysis version in use.
    pub vectors: usize,
    pub events: usize,
    pub likes: usize,
    /// Something was listened, liked or put in a playlist.
    pub history: bool,
    /// The daylist's song count.
    pub daylist_tracks: usize,
    /// Each tile's song count.
    pub tile_tracks: Vec<(v::MixId, usize)>,
    /// Discoveries and new releases the last discovery pass left.
    pub discover_cached: usize,
    pub new_releases_cached: usize,
    /// Library tracks the services' kept suggestions give affinity now,
    /// and suggested songs new to the library.
    pub suggested_library: usize,
    pub suggested_new: usize,
    pub took: Duration,
}

/// The home exactly as the runtime builds it, without a player: the
/// recommendation snapshot of `lib` now and the home over it (checks on a
/// copy of a real library). Like a build, it keeps
/// the day's picks in the library's cache.
pub fn preview_home(lib: &Library, library_dir: &std::path::Path) -> Result<HomePreview, String> {
    preview_home_with(lib, library_dir, &v::Prefs::default())
}

/// [`preview_home`] under `prefs` («Новое в радио», «Миксы только из
/// скачанного»).
pub fn preview_home_with(lib: &Library, library_dir: &std::path::Path, prefs: &v::Prefs) -> Result<HomePreview, String> {
    let started = Instant::now();
    let index = SoundIndex::default();
    crate::analysis::load_offline(lib, &index, prefs);
    let vectors = index.read().len();
    let (day, utc_offset_min) = local_day();
    let (discoveries, new_releases_cached) = discover::cached_found(lib);
    let epoch = crate::home_v2::Epoch::of(crate::home_v2::Epoch::run(), recs_now(), utc_offset_min);
    let input = BuildInput {
        lib: lib.clone(),
        index: index.clone(),
        day,
        locale: prefs.resolved_locale(),
        epoch: epoch.clone(),
        utc_offset_min,
        now: recs_now(),
        refresh_daily: false,
        refresh_daylist: false,
        new_share: new_share(prefs, v::RadioKind::Mix),
        local_only: prefs.mixes_downloaded_only,
        new: discoveries.clone(),
        new_at: 0,
        new_pool: discoveries.clone(),
        new_sources: HashMap::new(),
        suggested: suggest::cached(lib),
        rerank: false,
        refresh_mix: None,
        row_more: 0,
        blends: Vec::new(),
        session: Session::default(),
        mix_prefs: lanes::MixPrefs::of(prefs),
        extra: ReadExtra::of(discoveries.clone(), prefs),
        new_log: novelty::log_of(lib),
    };
    let built = build(input)?;
    let h = &built.home;
    let mut res = crate::Resolver::new(lib.clone(), library_dir.to_owned());
    let mut visit = crate::home_v2::Visit::now(None, prefs);
    visit.sound = Some(index.clone());
    visit.profiles = Some(built.snap.profiles.clone());
    visit.epoch = epoch;

    let mut home = res.home_in(&crate::home::build_with(lib, &visit), prefs.resolved_locale());
    HomeParts::of_in(prefs.resolved_locale(), Some(h), Vec::new(), Vec::new(), prefs.mixes_downloaded_only, prefs.home_card_shown()).apply(&mut home, &mut res);
    Ok(HomePreview {
        catalog: built.snap.catalog.len(),
        vectors,
        events: lib.listening_events().map_or(0, |e| e.len()),
        likes: lib.liked_track_ids().map_or(0, |l| l.len()),
        history: h.history,
        daylist_tracks: h.daylist.as_ref().map_or(0, |d| d.picks.len()),
        tile_tracks: h.tiles.iter().map(|t| (t.mix_id(), t.picks.len())).collect(),
        discover_cached: discoveries.len(),
        new_releases_cached,
        suggested_library: (0..built.snap.catalog.len()).filter(|&i| built.snap.model.taste.suggested_at(i) > 0.0).count(),
        suggested_new: built.snap.suggested_new.len(),
        took: started.elapsed(),
        home,
    })
}

/// The prequential replay of a library's history: every
/// start predicted by the model learnt from the starts before it, against
/// the affinity of phase 1 and the hand weights. The build keeps its
/// caches in `lib`: run it on a copy.
pub fn preview_replay(lib: &Library) -> Result<nikgob_recs::model::ReplayReport, String> {
    let preview = ModelPreview::of(lib)?;
    let snap = &preview.snap;
    Ok(nikgob_recs::model::replay(&snap.catalog, &preview.history, ModelInput { index: &snap.index, types: Some(&snap.types), weights: None, graph: None }))
}

/// [`preview_replay`] with the recommendations' settings of `prefs`: the
/// graph of similar music the library keeps, the personal graph.
pub fn preview_replay_with(lib: &Library, prefs: &v::Prefs) -> Result<nikgob_recs::model::ReplayReport, String> {
    let preview = ModelPreview::of_with(lib, prefs)?;
    let snap = &preview.snap;
    let graph = (!preview.graph.is_empty()).then_some(&preview.graph);
    Ok(nikgob_recs::model::replay(&snap.catalog, &preview.history, ModelInput { index: &snap.index, types: Some(&snap.types), weights: None, graph }))
}

/// A snapshot of a library to try the radio on, as the runtime would
/// (its caches are kept in the library: a copy).
pub struct ModelPreview {
    snap: Snapshot,
    history: TasteInput,
    /// The graph of similar music it was built with.
    graph: nikgob_recs::GraphInput,
    /// The last pass's discoveries and where each came from.
    new: Vec<StreamTrack>,
    new_sources: HashMap<String, nikgob_recs::Source>,
}

impl ModelPreview {
    pub fn of(lib: &Library) -> Result<Self, String> {
        Self::of_with(lib, &v::Prefs::default())
    }

    /// The snapshot with the recommendations' settings of `prefs`.
    pub fn of_with(lib: &Library, prefs: &v::Prefs) -> Result<Self, String> {
        let index = SoundIndex::default();
        crate::analysis::load_offline(lib, &index, prefs);
        let (_, utc_offset_min) = local_day();
        // The last pass's finds: their analysed previews come with the
        // snapshot.
        let (new, new_sources) = discover::cached_pool(lib);
        let extra = ReadExtra::of(new.clone(), prefs);
        let (snap, _, _, history) = read_parts(lib, &index, recs_now(), utc_offset_min, &[], false, &extra)?;
        let graph = if extra.graph {
            let rows = lib.rec_tracks().map_err(|e| e.to_string())?;
            graph::read(lib, &rows, &snap.artist_keys).input
        } else {
            nikgob_recs::GraphInput::default()
        };
        Ok(Self { snap, history, graph, new, new_sources })
    }

    /// What the snapshot was built of, for the offline evaluation
    /// ([`nikgob_recs::eval`]): the catalog, its sound and song types, the
    /// history and the graph, and the new songs of the services the cards
    /// of «Миксы для тебя» could take (the discoveries and the graph's
    /// finds, weighed as a build weighs them).
    pub fn dataset(&self, name: &str) -> nikgob_recs::eval::Dataset {
        let graph = (!self.graph.is_empty()).then_some(&self.graph);
        let mut d = nikgob_recs::eval::Dataset::of(name, &self.snap.catalog, &self.snap.index, &self.snap.types, &self.history, graph);
        let snap = &self.snap;
        d.new = lanes::candidates(snap, &self.new, &self.new_sources, &nikgob_recs::NewLog::default(), 1)
            .into_iter()
            .map(|c| nikgob_recs::eval::DNew {
                vector: snap.new_vectors.get(&stream_song_key(&c.track)).map(|v| v.to_vec()),
                near: c.near.iter().map(|&i| snap.model.id(i)).collect(),
                weight: c.weight,
                artist: c.artist_tracks.first().and_then(|&i| snap.model.main_artist_at(i)),
                track: c.track,
            })
            .collect();
        d
    }

    /// The edges of tracks and of artists of the graph of similar music in
    /// the model.
    pub fn graph_size(&self) -> (usize, usize) {
        self.snap.model.graph.len()
    }

    /// The model the snapshot scores by.
    pub fn model(&self) -> &UserModel {
        &self.snap.model
    }

    /// The catalog and the sound index of the snapshot.
    pub fn catalog(&self) -> (&Catalog, &VectorIndex<TrackId>) {
        (&self.snap.catalog, &self.snap.index)
    }

    /// The seeds of a pass of «Новое» now: the tracks, then the artists,
    /// a line each, and the artists whose new releases are looked for.
    pub fn new_music_seeds(&self, lib: &Library) -> (Vec<String>, Vec<String>) {
        let recs = self.snap.recs();
        let seeds = recs.discovery_seeds();
        let track = |t: TrackId| lib.get_track(t).ok().flatten().map_or_else(|| t.to_string(), |t| format!("«{}» {}", t.title, t.artist_credit.display()));
        let artist = |a: ArtistId| lib.get_artist(a).ok().flatten().map_or_else(|| a.to_string(), |a| a.name);
        let elsewhere: HashSet<TrackId> = self.snap.model.taste.saved_elsewhere().iter().map(|s| s.0).collect();
        let mut lines: Vec<String> = seeds
            .tracks
            .iter()
            .map(|(t, a)| format!("{} {a:.2}{}", track(*t), if elsewhere.contains(t) { " (saved elsewhere)" } else { "" }))
            .collect();
        lines.extend(seeds.artists.iter().map(|(a, s)| format!("artist {} {s:.2}", artist(*a))));
        (lines, recs.release_artists().into_iter().map(artist).collect())
    }

    /// The playlists the user plays in the background, by name.
    pub fn background_playlists(&self, lib: &Library) -> Vec<String> {
        let names: HashMap<ArtistId, String> = lib.artist_names().unwrap_or_default().into_iter().collect();
        let rows = lib.rec_tracks().unwrap_or_default();
        let events = lib.listening_events().unwrap_or_default();
        let tiles = tiles::input(lib, &rows, &events, names);
        let (kept, _) = played_in_background(&self.snap, &tiles, &events);
        kept.into_iter().filter_map(|k| tiles.playlists.get(k)).map(|p| p.name.clone()).collect()
    }

    /// The worlds by title (their tokens).
    pub fn world_named(&self, title: &str) -> Option<u64> {
        self.snap.worlds.iter().find(|(_, (_, t))| t.eq_ignore_ascii_case(title)).map(|(w, _)| *w)
    }

    /// The mixes of a world's page now, with their tracks and why each is
    /// offered.
    pub fn world_mixes(&self, lib: &Library, world: u64) -> Vec<(nikgob_recs::WorldMixKind, Vec<TrackId>, Option<String>)> {
        let (part, _, _) = daylist_key(recs_now(), local_day().1);
        let recs = self.snap.recs();
        let name = self.world_title(world).unwrap_or_default();
        let loc = nikgob_core::i18n::locale();
        let title = |i: usize| lib.get_track(self.snap.model.id(i)).ok().flatten().map(|t| t.title);
        recs.world_mixes(world, part)
            .into_iter()
            .map(|m| {
                let why = recs.why_world_mix(&m, &name, &title, loc);
                (m.kind, m.tracks.iter().map(|&i| self.snap.model.id(i)).collect(), why)
            })
            .collect()
    }

    /// Why a radio's pick `p` after `before` ([`nikgob_recs::explain`]).
    pub fn why_pick(&self, lib: &Library, p: &Pick, before: Option<TrackId>) -> Option<String> {
        let title = |t: TrackId| lib.get_track(t).ok().flatten().map(|t| t.title);
        self.snap.recs().why_pick(p, before, &title, nikgob_core::i18n::locale())
    }

    /// The track suits the background ([`background_fit`]).
    pub fn suits_background(&self, id: TrackId) -> bool {
        self.snap.catalog.position(id).is_some_and(|i| self.snap.background.get(i).copied().unwrap_or(false))
    }

    /// The name of the taste profile of a track.
    pub fn profile_name(&self, lib: &Library, id: TrackId) -> Option<String> {
        let names: HashMap<ArtistId, String> = lib.artist_names().unwrap_or_default().into_iter().collect();
        let j = self.snap.catalog.position(id).and_then(|i| self.snap.model.profile_at(i))?;
        Some(profile_name(&self.snap.model, &names, v::ProfileNames::Kind, nikgob_core::i18n::locale(), j).0)
    }

    /// The next `n` of a radio.
    pub fn extend(&self, radio: &mut Engine, n: usize) -> Vec<Pick> {
        radio.extend(&self.snap.recs(), n, &RadioParams { new_share: 0.0, seed: 1 })
    }

    /// The next `n` of a radio with `new_share` of new music.
    pub fn extend_new(&self, radio: &mut Engine, n: usize, new_share: f32) -> Vec<Pick> {
        radio.extend(&self.snap.recs(), n, &RadioParams { new_share, seed: 1 })
    }

    /// The new music a service's answer kept in the library's cache under
    /// `key` (`radio_new.youtube.radio.<id>`) holds, new indeed.
    pub fn cached_new(&self, lib: &Library, key: &str) -> Vec<StreamTrack> {
        let tracks: Vec<StreamTrack> = cache_read(lib, key).unwrap_or_default();
        tracks.into_iter().filter(|t| self.snap.is_new(t)).collect()
    }

    /// The world of a catalog track.
    pub fn world(&self, id: TrackId) -> Option<u64> {
        self.snap.catalog.get(id).and_then(|t| t.world)
    }

    /// The world's title.
    pub fn world_title(&self, world: u64) -> Option<String> {
        self.snap.worlds.get(&world).map(|w| w.1.clone())
    }


    /// The taste profiles, the largest share first (as the log has them).
    pub fn profiles(&self, lib: &Library) -> Vec<String> {
        let names: HashMap<ArtistId, String> = lib.artist_names().unwrap_or_default().into_iter().collect();
        let rows = lib.rec_tracks().unwrap_or_default();
        let events = lib.listening_events().unwrap_or_default();
        let tiles = tiles::input(lib, &rows, &events, names.clone());
        profile_lines(&self.snap.model, &tiles, &names, v::ProfileNames::Kind, nikgob_core::i18n::locale())
    }

    /// How the user listens ([`Listening`](nikgob_recs::listening::Listening)), in a line.
    pub fn listening(&self) -> String {
        listening_line(&self.snap.model)
    }

    /// The home's cards of each profile out of `slots`, the largest share
    /// first: its name, its cards and, for a background profile, the sizes
    /// of its «Фон» cards (the basic background rule).
    pub fn home_profiles(&self, lib: &Library, slots: usize) -> Vec<String> {
        let names: HashMap<ArtistId, String> = lib.artist_names().unwrap_or_default().into_iter().collect();
        let model = &self.snap.model;
        let fit = &self.snap.background;
        let home = model.home_profiles(slots, nikgob_recs::profiles::BG_CARDS, |i| fit[i]);
        let loc = nikgob_core::i18n::locale();
        profile_order(model)
            .into_iter()
            .map(|j| {
                let (name, _) = profile_name(model, &names, v::ProfileNames::Kind, loc, j);
                let bg: Vec<usize> = home.background[j].iter().map(Vec::len).collect();
                format!("{name}: {} cards{}", home.cards[j], if bg.is_empty() { String::new() } else { format!(", «Фон» of {bg:?} tracks") })
            })
            .collect()
    }

    pub fn song_type(&self, id: TrackId) -> Option<nikgob_recs::SongType> {

        self.snap.types.get(id).copied()
    }

    /// The replay before and after `n` tracks with plays are disliked
    /// (drawn by `seed`): the same starts scored both times, the disliked
    /// tracks' own left out of the scoring.
    pub fn replay_dislikes(&self, n: usize, seed: u64) -> (nikgob_recs::model::ReplayReport, nikgob_recs::model::ReplayReport, Vec<TrackId>) {
        let m = &self.snap.model;
        let mut played: Vec<TrackId> = (0..m.len()).filter(|&i| m.tracks[i].starts > 0).map(|i| m.id(i)).collect();
        let mut rng = nikgob_recs::rng::Rng::new(seed);
        let mut dislikes = Vec::new();
        while dislikes.len() < n && !played.is_empty() {
            let k = (rng.next_u64() % played.len() as u64) as usize;
            dislikes.push(played.swap_remove(k));
        }
        let unscored: HashSet<TrackId> = dislikes.iter().copied().collect();
        let input = || ModelInput { index: &self.snap.index, types: Some(&self.snap.types), weights: None, graph: None };
        let before = nikgob_recs::model::replay_with(&self.snap.catalog, &self.history, input(), &[], &unscored);
        let after = nikgob_recs::model::replay_with(&self.snap.catalog, &self.history, input(), &dislikes, &unscored);
        (before, after, dislikes)
    }

    /// What a dislike of `track` does: how much lower each track stands
    /// (its nearest sound neighbours), and its artist's other tracks with
    /// theirs (0 unless one is a neighbour).
    pub fn dislike_effect(&self, track: TrackId) -> (Vec<(TrackId, f32)>, Vec<(TrackId, f32)>) {
        let m = &self.snap.model;
        let Some(d) = m.position(track) else { return (Vec::new(), Vec::new()) };
        let emb: Vec<Option<nikgob_recs::Vector>> = (0..m.len()).map(|i| m.embedded_at(i).copied()).collect();
        let near = nikgob_recs::dislike::near_dislikes(&emb, &[d], m.scale());
        let mut lower: Vec<(TrackId, f32)> = near.iter().enumerate().filter(|(_, w)| **w > 0.0).map(|(i, w)| (m.id(i), *w)).collect();
        lower.sort_by(|a, b| b.1.total_cmp(&a.1));
        let artist = m.main_artist_at(d);
        let same: Vec<(TrackId, f32)> =
            (0..m.len()).filter(|&i| i != d && artist.is_some() && m.main_artist_at(i) == artist).map(|i| (m.id(i), near[i])).collect();
        (lower, same)
    }

    /// The finds of the last discovery pass and the new music the services'
    /// answers under `keys` keep, that the junk filter leaves out, with why.
    pub fn junk_found(&self, lib: &Library, keys: &[&str]) -> Vec<(StreamTrack, nikgob_recs::Junk)> {
        let mut pool = discover::cached_finds(lib);
        for k in keys {
            pool.extend(cache_read::<Vec<StreamTrack>>(lib, k).unwrap_or_default());
        }
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for t in &pool {
            if !seen.insert(t.id.clone()) {
                continue;
            }
            let canonical = nikgob_recs::junk::canonical_in(t, &pool);
            if let Some(j) = nikgob_recs::junk(t, canonical.as_ref()) {
                out.push((t.clone(), j));
            }
        }
        out
    }

    /// How far apart two tracks sound, in the model's median distances.

    pub fn distance(&self, a: TrackId, b: TrackId) -> Option<f32> {
        let m = &self.snap.model;
        let (a, b) = (m.embedded_at(m.position(a)?)?, m.embedded_at(m.position(b)?)?);
        Some(nikgob_recs::index::euclid(a, b) / m.scale())
    }
}
