use super::home::build_home;
use super::*;

/// What the builder thread needs.
pub(super) struct BuildInput {
    pub(super) lib: Library,
    pub(super) index: SoundIndex,
    pub(super) day: String,
    pub(super) locale: Locale,
    /// The home's epoch: «Миксы для тебя» are its.
    pub(super) epoch: crate::home_v2::Epoch,
    pub(super) utc_offset_min: i32,
    pub(super) now: UnixMs,
    pub(super) refresh_daily: bool,
    pub(super) refresh_daylist: bool,
    pub(super) new_share: f32,
    /// «Миксы только из скачанного».
    pub(super) local_only: bool,
    /// New music for the daily mix (the discoveries).
    pub(super) new: Vec<StreamTrack>,
    /// When the pass that found them ended (0: none): the day's «Твой
    /// микс» and daylist of older finds are picked again.
    pub(super) new_at: UnixMs,
    /// Every discovery the pass kept, best first: «С новым» of the cards
    /// draws from them all (and from the graph's finds).
    pub(super) new_pool: Vec<StreamTrack>,
    /// Where each of them came from, by [`stream_song_key`].
    pub(super) new_sources: HashMap<String, nikgob_recs::Source>,
    /// The services' suggestions, of the services switched on.
    pub(super) suggested: Vec<suggest::Kept>,
    /// They changed: the discoveries are ranked again after this build.
    pub(super) rerank: bool,
    /// [`RecsState::refresh_mix`].
    pub(super) refresh_mix: Option<v::MixKey>,
    /// [`RecsState::row_more`].
    pub(super) row_more: usize,
    /// The blends made today.
    pub(super) blends: Vec<v::MixKey>,
    /// The session of the moment: the order of the mixes follows it.
    pub(super) session: Session,
    /// The settings of «Миксы для тебя».
    pub(super) mix_prefs: lanes::MixPrefs,
    pub(super) extra: ReadExtra,
    /// How new music was received: «С новым» takes its share by it.
    pub(super) new_log: nikgob_recs::NewLog,
}

pub(super) fn build(input: BuildInput) -> Result<Built, String> {
    let started = Instant::now();
    let (snap, tiles_input, index_len) =
        read_parts(&input.lib, &input.index, input.now, input.utc_offset_min, &input.suggested, input.local_only, &input.extra)
            .map(|(snap, tiles, len, _)| (snap, tiles, len))?;
    let home = build_home(&snap, &input, &tiles_input);
    log::info!(
        "recs: {} tracks, {} vectors, {} events in {:?}",
        snap.catalog.len(),
        snap.index.len(),
        tiles_input.events.len(),
        started.elapsed()
    );
    Ok(Built {
        snap: Arc::new(snap),
        home,
        index_len,
        rerank: input.rerank,
    })
}

/// The snapshot of the library at `now`, what the tiles read besides it,
/// and the size of the shared sound index it was cut from.
pub(super) fn read_snapshot(
    lib: &Library,
    index: &SoundIndex,
    now: UnixMs,
    utc_offset_min: i32,
    suggested: &[suggest::Kept],
    local_only: bool,
) -> Result<(Snapshot, nikgob_recs::TilesInput, usize), String> {
    read_parts(lib, index, now, utc_offset_min, suggested, local_only, &ReadExtra::default()).map(|(snap, tiles, len, _)| (snap, tiles, len))
}

/// What a snapshot reads besides the library: the new music found (the
/// analysed previews of it are kept with the snapshot), «Учитывать
/// дизлайки в рекомендациях» and the settings of the taste profiles.
#[derive(Clone)]
pub(crate) struct ReadExtra {
    pub new: Vec<StreamTrack>,
    pub dislike_neighbours: bool,
    pub background_less: bool,
    pub split_by_sound: bool,
    pub names: v::ProfileNames,
    pub locale: Locale,
    /// The graph of similar music: used at all, the personal graph, the
    /// library as the taste while the history is short.
    pub graph: bool,
    pub personal_graph: bool,
    pub library_prior: bool,
}

impl Default for ReadExtra {
    fn default() -> Self {
        Self::of(Vec::new(), &v::Prefs::default())
    }
}

impl ReadExtra {
    pub(crate) fn of(new: Vec<StreamTrack>, prefs: &v::Prefs) -> Self {
        Self {
            new,
            dislike_neighbours: prefs.dislike_in_recs,
            background_less: prefs.background_counts_less,
            split_by_sound: prefs.profiles_split,
            names: prefs.profile_names,
            locale: prefs.resolved_locale(),
            graph: prefs.graph_use,
            personal_graph: prefs.graph_personal,
            library_prior: prefs.library_as_taste,
        }
    }
}

/// [`read_snapshot`] and the history its model was built of.
pub(super) fn read_parts(
    lib: &Library,
    index: &SoundIndex,
    now: UnixMs,
    utc_offset_min: i32,
    suggested: &[suggest::Kept],
    local_only: bool,
    extra: &ReadExtra,
) -> Result<(Snapshot, nikgob_recs::TilesInput, usize, TasteInput), String> {
    let err = |what: &str, e: nikgob_library::LibraryError| format!("{what}: {e}");
    // Where the time goes, stage by stage (a long history makes it felt).
    let mut stage = Instant::now();
    let mut stages: Vec<(&str, u128)> = Vec::new();
    let mut lap = |name: &'static str, stages: &mut Vec<(&str, u128)>| {
        stages.push((name, stage.elapsed().as_millis()));
        stage = Instant::now();
    };
    let rows = lib.rec_tracks().map_err(|e| err("tracks", e))?;
    let mut events = lib.listening_events().map_err(|e| err("events", e))?;
    let mut like_times = lib.liked_track_times().map_err(|e| err("likes", e))?;
    // One song on two albums is one recording: the copy standing for it
    // takes the other copies' plays and likes, and only it is picked.
    let spares = lib.recording_spares().unwrap_or_default();
    let in_library: HashSet<TrackId> = rows.iter().filter(|r| r.in_library).map(|r| r.id).collect();
    let spares: HashMap<TrackId, TrackId> = spares.into_iter().filter(|(_, canon)| in_library.contains(canon)).collect();
    fold_recordings(&spares, &mut events, &mut like_times);
    let saved = lib.saved_tracks().map_err(|e| err("saved songs", e))?;
    let entries = lib.playlist_entry_times().map_err(|e| err("playlists", e))?;
    let feedback = lib.rec_dislikes().map_err(|e| err("feedback", e))?;
    let artists = lib.artist_names().map_err(|e| err("artists", e))?;
    let remote = lib.track_remote_ids().map_err(|e| err("remote ids", e))?;
    let releases = lib.release_names().map_err(|e| err("releases", e))?;
    let release_rows = lib
        .releases(0, 20_000, nikgob_library::Sort::desc(nikgob_library::ReleaseSortKey::Added))
        .map_err(|e| err("albums", e))?;
    let mut artist_keys: HashMap<String, ArtistId> = HashMap::new();
    let mut names: HashMap<ArtistId, String> = HashMap::new();
    for (id, name) in &artists {
        artist_keys.entry(artist_key(name)).or_insert(*id);
        names.insert(*id, name.clone());
    }
    lap("rows", &mut stages);
    let tiles_input = tiles::input(lib, &rows, &events, names.clone());
    lap("tiles", &mut stages);
    // The songs saved in a service's library that the library lacks: the
    // taste of their artists and seeds of the new music, no catalog track.
    let saved_set: HashSet<TrackId> = saved.iter().copied().collect();
    let elsewhere: Vec<&nikgob_library::RecTrack> = rows.iter().filter(|r| !r.in_library && saved_set.contains(&r.id)).collect();
    let mut elsewhere_names: HashMap<ArtistId, String> = HashMap::new();
    for a in elsewhere.iter().flat_map(|r| &r.artists) {
        if !names.contains_key(a) && !elsewhere_names.contains_key(a)
            && let Some(artist) = lib.get_artist(*a).ok().flatten()
        {
            elsewhere_names.insert(*a, artist.name);
        }
    }

    // One song on two albums: the track with a file (the oldest) stands for it.
    let mut songs: HashMap<&str, (bool, TrackId)> = HashMap::new();
    for r in rows.iter().filter(|r| r.in_library && !spares.contains_key(&r.id)) {
        let e = songs.entry(r.song.as_str()).or_insert((r.has_local, r.id));
        if r.has_local && !e.0 {
            *e = (true, r.id);
        }
    }
    // The versions of one work (an off vocal, a karaoke, a TV size of a
    // song beside it) are one song for a session or a mix: they stand for
    // the plain one (or, without it, the first of them).
    let works = work_heads(rows.iter().filter(|r| r.in_library && !spares.contains_key(&r.id)).map(|r| (r.id, r.title.as_str(), r.artists.first().copied())));
    // Both together: every copy of every version of a work has one key.
    let song_of = compose_songs(
        rows.iter().filter(|r| r.in_library && !spares.contains_key(&r.id)).map(|r| (r.id, songs.get(r.song.as_str()).map_or(r.id, |s| s.1))),
        &works,
    );
    // The worlds of Главная: one of them does not fill a mix.
    let album_worlds = crate::home_v2::worlds_of_releases(&release_rows);
    let mut worlds: HashMap<u64, (String, String)> = HashMap::new();
    let catalog = Catalog::new(rows.iter().filter(|r| r.in_library && !spares.contains_key(&r.id)).map(|r| {
        let song = song_of.get(&r.id).copied().unwrap_or(r.id);
        let world = r.release.and_then(|a| album_worlds.get(&a));
        let mut t = TrackInfo::new(r.id, r.artists.clone(), r.release)
            .with_local(r.has_local)
            .with_song(song)
            .with_duration(tiles_input.facts.get(&r.id).and_then(|f| f.duration_ms))
            .with_position(tiles_input.facts.get(&r.id).map_or(0, |f| f.position))
            .with_added(tiles_input.facts.get(&r.id).map_or(0, |f| f.added_at));
        let main = r.artists.first().copied();
        if let Some(key) = crate::home_v2::track_world_key(world.map(|w| w.0.as_str()), main) {
            t = t.with_world(&key);
            let token = nikgob_recs::world_token(&key);
            worlds.entry(token).or_insert_with(|| {
                let title = world.map_or_else(|| main.and_then(|a| names.get(&a)).cloned().unwrap_or_default(), |w| w.1.clone());
                (key, title)
            });
        }
        t
    }))
    .with_aliases(spares.iter().map(|(copy, canon)| (*copy, *canon)));

    // «Не рекомендовать»: library tracks and artists, songs and artists of
    // the services (an artist disliked by name is the library's too).
    let mut filter = DiscoveryFilter::default();
    let mut disliked_tracks = Vec::new();
    let mut disliked_artists = Vec::new();
    for f in &feedback {
        match f.target_kind.as_str() {
            "track" => disliked_tracks.extend(f.target_id.parse::<TrackId>().ok()),
            "artist" => disliked_artists.extend(f.target_id.parse::<ArtistId>().ok()),
            SONG => {
                filter.disliked_songs.insert(f.target_id.clone());
            }
            ARTIST_NAME => {
                filter.disliked_artists.insert(f.target_id.clone());
                disliked_artists.extend(artist_keys.get(&f.target_id).copied());
            }
            _ => {}
        }
    }
    for a in &disliked_artists {
        if let Some(n) = names.get(a) {
            filter.disliked_artists.insert(artist_key(n));
        }
    }
    let by_id: HashMap<TrackId, &nikgob_library::RecTrack> = rows.iter().map(|r| (r.id, r)).collect();
    for t in &disliked_tracks {
        if let Some(r) = by_id.get(t) {
            filter.disliked_songs.insert(song_key(&r.title, &r.artist));
        }
    }
    // A song the user deleted from an album is a strong «not for me» for
    // that song (not its artist): new music never brings it back.
    for u in lib.unwanted_songs().unwrap_or_default() {
        filter.disliked_songs.insert(song_key(&u.title, &u.artist));
    }
    filter.known_ids = remote.into_iter().collect();
    // A song's versions are the song (an off vocal, a TV size: its work,
    // [`nikgob_recs::work_title`]): a service's version of a library song
    // is no new music.
    filter.known_songs = rows
        .iter()
        .filter(|r| r.in_library)
        .flat_map(|r| [song_key(&r.title, &r.artist), song_key(&nikgob_recs::work_title(&r.title).0, &r.artist)])
        .collect();
    filter.known_artists = artist_keys.keys().cloned().collect();
    // A song the user keeps elsewhere is no new music, nor its artist.
    for r in &elsewhere {
        filter.known_songs.insert(song_key(&r.title, &r.artist));
        filter.known_ids.extend(lib.external_ids(nikgob_core::streaming::EntityKind::Track, *r.id.as_uuid()).unwrap_or_default());
    }
    filter.known_artists.extend(elsewhere_names.values().map(|n| artist_key(n)));
    let played: HashSet<TrackId> = events.iter().map(|e| e.track_id).collect();
    let played_songs = played
        .iter()
        .filter_map(|t| by_id.get(t))
        .map(|r| song_key(&r.title, &r.artist))
        .collect();
    let known_albums = releases.iter().map(|(t, a)| album_key(t, a)).collect();

    // The services' suggestions: a library track by the service's id or
    // the same song, a library artist by name; the rest is new music.
    let lists: Vec<ServiceSuggestions<'_>> = suggested
        .iter()
        .map(|k| ServiceSuggestions {
            at: k.at,
            tracks: &k.tracks,
            artists: &k.artists,
        })
        .collect();
    let mut by_song: HashMap<String, TrackId> = HashMap::new();
    for r in rows.iter().filter(|r| r.in_library) {
        by_song.entry(song_key(&r.title, &r.artist)).or_insert(r.id);
    }
    let matched = match_suggestions(
        &lists,
        |t| {
            lib.track_by_remote(&t.id)
                .ok()
                .flatten()
                .filter(|id| catalog.position(*id).is_some())
                .or_else(|| by_song.get(&stream_song_key(t)).copied())
        },
        |name| artist_keys.get(&artist_key(name)).copied(),
    );
    let suggested_new: Vec<(StreamTrack, f32)> =
        matched.unmatched.into_iter().filter(|(t, _)| is_new(&filter, t) && nikgob_recs::junk(t, None).is_none()).collect();

    // The analysed previews of the new music found and the model's vectors
    // of them (their tracks are not the catalog's: the snapshot's index
    // drops them below).
    let (new_vectors, new_embeddings) = {
        let shared = index.read();
        let mut vectors: HashMap<String, nikgob_recs::Vector> = HashMap::new();
        let mut embeddings: HashMap<String, nikgob_recs::Compact> = HashMap::new();
        for t in &extra.new {
            let Some(id) = lib.track_by_remote(&t.id).ok().flatten() else { continue };
            let Some(v) = shared.get(id) else { continue };
            let key = stream_song_key(t);
            if let Some(c) = shared.embedding(id) {
                embeddings.insert(key.clone(), c);
            }
            vectors.insert(key, *v);
        }
        (vectors, embeddings)
    };
    // Vectors of tracks that are gone (or left the library) stay in the
    // shared index until a restart; not in the snapshot.
    let mut index = index.read().clone();
    let index_len = index.len();
    let stale: Vec<TrackId> = index.iter().map(|(id, _)| id).filter(|id| catalog.position(*id).is_none()).collect();
    for id in stale {
        index.remove(id);
    }
    lap("catalog", &mut stages);
    let types = read_song_types(lib, &catalog, &index, &tiles_input, &names);
    lap("types", &mut stages);
    let cached: Option<Weights> = lib
        .cache_get(MODEL_KEY)
        .ok()
        .flatten()
        .and_then(|(_, json)| serde_json::from_str(&json).ok())
        .filter(|w: &Weights| w.version == FEATURE_VERSION);
    // The seeks are touches of the plays (how the user listens).
    let seeks = lib.seek_events().unwrap_or_default();
    let kept: Option<nikgob_recs::profiles::ProfileHistory> =
        lib.cache_get(PROFILES_KEY).ok().flatten().and_then(|(_, json)| serde_json::from_str(&json).ok());
    // Each play on the kind of device it was on, for the device the taste
    // is for (this one); a guest's device is left out.
    let devices = Devices::read(lib);
    let history = TasteInput {
        events: events.iter().chain(&seeks).filter_map(|e| devices.place(e, play_of(e))).collect(),
        target_device: devices.here(),
        // Each play by the clock of its own day (the summer time and back);
        // an offset asked for that is not the zone's now keeps to itself.
        utc_offsets: Some(local_offsets(events.iter().map(|e| e.ts).min().unwrap_or(now), now))
            .filter(|o| o.last().map(|x| x.1) == Some(utc_offset_min))
            .unwrap_or_default(),
        playlists: tiles_input.playlists.iter().map(|p| p.tracks.clone()).collect(),
        profiles: nikgob_recs::profiles::Settings {
            background_less: extra.background_less,
            split_by_sound: extra.split_by_sound,
            history: kept.clone(),
        },
        likes: like_times.iter().map(|l| l.0).collect(),
        saved,
        saved_elsewhere: elsewhere.iter().map(|r| (r.id, r.artists.clone())).collect(),
        like_times,
        playlist_entries: entries,
        disliked_tracks,
        disliked_artists,
        utc_offset_min,
        suggested_tracks: matched.tracks,
        suggested_artists: matched.artists,
        dislike_neighbours: extra.dislike_neighbours,
        personal_graph: extra.personal_graph,
        library_prior: extra.library_prior,
    };
    // The graph of similar music the library keeps, resolved to it now.
    let similar = if extra.graph { graph::read(lib, &rows, &artist_keys) } else { graph::Resolved::default() };
    lap("graph", &mut stages);
    let mut model = UserModel::build(
        &catalog,
        &history,
        ModelInput { index: &index, types: Some(&types), weights: cached, graph: (!similar.input.is_empty()).then_some(&similar.input) },
        now,
    );
    // The library's works by the keys new songs are known by, so the wave
    // plays one version of a song of either kind.
    model.set_song_works(rows.iter().filter(|r| catalog.position(r.id).is_some()).map(|r| (song_key(&nikgob_recs::work_title(&r.title).0, &r.artist), r.id)));
    lap("model", &mut stages);
    log::debug!(
        "recs: snapshot of {} events: {}",
        history.events.len(),
        stages.iter().map(|(n, ms)| format!("{n} {ms} ms")).collect::<Vec<_>>().join(", ")
    );
    if cached.is_none()
        && let Ok(json) = serde_json::to_string(&model.weights)
        && let Err(e) = lib.cache_put(MODEL_KEY, now, &json)
    {
        log::warn!("recs: cannot keep the model: {e}");
    }
    if kept.as_ref() != Some(&model.profiles.history)
        && let Ok(json) = serde_json::to_string(&model.profiles.history)
        && let Err(e) = lib.cache_put(PROFILES_KEY, now, &json)
    {
        log::warn!("recs: cannot keep the profiles: {e}");
    }
    log_profiles(&model, &tiles_input, &names, extra.names, extra.locale);
    let artist_affinity: HashMap<String, f32> = model
        .taste
        .all_artist_affinities()
        .filter_map(|(a, s)| Some((artist_key(names.get(&a).or_else(|| elsewhere_names.get(&a))?), (s.max(0.0) / 3.0).tanh())))
        .collect();
    // The artists beyond the library the graph links to the loved ones:
    // their new music counts as a little known.
    let artist_affinity = graph::with_votes(artist_affinity, &similar.votes, &|a| (model.taste.artist_affinity(a).max(0.0) / 3.0).tanh());
    // The songs the graph found beside the library's: new music, by how
    // loved the tracks whose radios hold them are. A track whose kind the
    // source did not say (lists kept before it was) is judged as a video
    // too, unless its channel is an official one.
    let graph_new = graph::finds(
        &similar.beyond,
        &|t, known| {
            is_new(&filter, t)
                && if known { nikgob_recs::junk(t, None).is_none() } else { nikgob_recs::junk::junk_any_kind(t, None).is_none() }
        },
        &|t| model.position(t).map(|i| (model.taste.affinity_at(i).max(0.0) / 3.0).tanh()),
        &|t| model.position(t).and_then(|i| model.main_artist_at(i)),
    );
    let genres = {
        let mut tags: HashMap<ArtistId, Vec<(String, f32)>> = HashMap::new();
        for (a, t, w) in &similar.input.tags {
            tags.entry(*a).or_default().push((t.clone(), *w));
        }
        let mut genres: HashMap<ArtistId, u8> = tags.iter().filter_map(|(a, t)| Some((*a, nikgob_recs::mix_names::leading(t)?))).collect();
        let untagged = style_families(&catalog, &index, &|a| tags.contains_key(&a), &genres);
        genres.extend(untagged);
        genres
    };
    let mut snap = Snapshot {
        genres,
        graph_new,
        background: Vec::new(),
        profiles: Arc::new(TrackProfiles::from_model(&model)),
        new_vectors,
        new_embeddings,
        artist_affinity,
        catalog,
        model,
        types,
        index,
        filter,
        artist_keys,
        known_albums,
        played_songs,
        suggested_new,
        local_only,
        worlds,
    };
    snap.background = background_fit(&snap, &tiles_input, &events);
    Ok((snap, tiles_input, index_len, history))
}

/// The work each track of `rows` (id, title, main artist) is a version of
/// ([`nikgob_recs::work_title`]): for every version of a work that has two
/// or more tracks, the one standing for it — its plain title, else the
/// first of them.
pub(crate) fn work_heads<'a>(rows: impl Iterator<Item = (TrackId, &'a str, Option<ArtistId>)>) -> HashMap<TrackId, TrackId> {
    let mut by_work: HashMap<(Option<ArtistId>, String), Vec<(bool, TrackId)>> = HashMap::new();
    for (id, title, artist) in rows {
        let (base, marked) = nikgob_recs::work_title(title);
        let key = nikgob_recs::discover::title_key(&base);
        if !key.is_empty() {
            by_work.entry((artist, key)).or_default().push((marked, id));
        }
    }
    let mut out = HashMap::new();
    for (_, mut group) in by_work {
        if group.len() < 2 || group.iter().all(|g| !g.0) {
            continue;
        }
        group.sort();
        let head = group[0].1;
        for (marked, id) in group {
            if marked && id != head {
                out.insert(id, head);
            }
        }
    }
    out
}

/// `h` with a track's model vector folded in (FNV-1a over its scale and
/// every coordinate; a track without one counts too).
pub(crate) fn embedding_fingerprint(h: u64, c: Option<&nikgob_recs::Compact>) -> u64 {
    const PRIME: u64 = 0x100_0000_01b3;
    let Some(c) = c else {
        return (h ^ 0xff).wrapping_mul(PRIME);
    };
    let h = (h ^ 1).wrapping_mul(PRIME);
    c.to_bytes().iter().fold(h, |h, b| (h ^ u64::from(*b)).wrapping_mul(PRIME))
}

/// One song key for each track of `tracks` (its id and the copy standing
/// for its song on several albums) and the versions of works `works`
/// joins ([`work_heads`]): the two relations together, so a copy of a
/// version and a copy of the plain song share it. A group stands for the
/// copy that stands for its work's head (the first head, when two works
/// meet through a song), else for its song's copy.
pub(crate) fn compose_songs(tracks: impl Iterator<Item = (TrackId, TrackId)>, works: &HashMap<TrackId, TrackId>) -> HashMap<TrackId, TrackId> {
    fn root(parent: &mut HashMap<TrackId, TrackId>, x: TrackId) -> TrackId {
        let mut r = x;
        while let Some(&p) = parent.get(&r).filter(|p| **p != r) {
            r = p;
        }
        let mut y = x;
        while y != r {
            y = parent.insert(y, r).unwrap_or(r);
        }
        r
    }
    fn join(parent: &mut HashMap<TrackId, TrackId>, a: TrackId, b: TrackId) {
        let (ra, rb) = (root(parent, a), root(parent, b));
        if ra != rb {
            parent.insert(ra.max(rb), ra.min(rb));
        }
    }
    let mut parent: HashMap<TrackId, TrackId> = HashMap::new();
    let mut copy_of: HashMap<TrackId, TrackId> = HashMap::new();
    let tracks: Vec<(TrackId, TrackId)> = tracks.collect();
    for &(id, song) in &tracks {
        parent.entry(id).or_insert(id);
        parent.entry(song).or_insert(song);
        copy_of.insert(id, song);
        join(&mut parent, id, song);
    }
    for (&version, &head) in works {
        if parent.contains_key(&version) && parent.contains_key(&head) {
            join(&mut parent, version, head);
        }
    }
    // Each group's head: the smallest work head in it.
    let mut heads: HashMap<TrackId, TrackId> = HashMap::new();
    for &head in works.values() {
        if !parent.contains_key(&head) {
            continue;
        }
        let r = root(&mut parent, head);
        let e = heads.entry(r).or_insert(head);
        *e = (*e).min(head);
    }
    tracks
        .iter()
        .map(|&(id, song)| {
            let r = root(&mut parent, id);
            let key = heads.get(&r).map_or(song, |h| copy_of.get(h).copied().unwrap_or(*h));
            (id, key)
        })
        .collect()
}

/// The family of genres of each artist of the catalog that `tagged` says
/// has no tags, by the styles of their songs' sketches, when they lead:
/// what each style is of is learnt from the songs of the artists whose
/// tags gave them a family (`families`,
/// [`nikgob_recs::styles::StyleFamilies`]).
pub(crate) fn style_families(
    catalog: &Catalog,
    index: &VectorIndex<TrackId>,
    tagged: &dyn Fn(ArtistId) -> bool,
    families: &HashMap<ArtistId, u8>,
) -> HashMap<ArtistId, u8> {
    let mut by_artist: HashMap<ArtistId, Vec<nikgob_recs::Styles>> = HashMap::new();
    let mut taught: Vec<(nikgob_recs::Styles, u8)> = Vec::new();
    for t in catalog.tracks() {
        let Some(a) = t.artists.first().copied() else { continue };
        let Some(s) = index.sketch(t.id).and_then(|s| s.styles()) else { continue };
        if let Some(f) = families.get(&a) {
            taught.push((s, *f));
        } else if !tagged(a) {
            by_artist.entry(a).or_default().push(s);
        }
    }
    if by_artist.is_empty() || taught.is_empty() {
        return HashMap::new();
    }
    let learnt = nikgob_recs::styles::StyleFamilies::learn(taught.iter().map(|(s, f)| (s, *f)));
    by_artist.into_iter().filter_map(|(a, list)| Some((a, learnt.leading(&list)?))).collect()
}

/// Whether each catalog track suits the background
/// ([`nikgob_recs::background::Basic`]): by its type and names, and
/// whether it is in a playlist the user plays in their background sessions
/// ([`nikgob_recs::background::background_playlists`]: the plays from it,
/// by the history's `context_id`, else every playlist of the track played
/// from a playlist before the history had one).
pub(crate) fn background_fit(snap: &Snapshot, tiles: &nikgob_recs::TilesInput, events: &[nikgob_core::model::Event]) -> Vec<bool> {
    use nikgob_recs::background::{BackgroundScore, BackgroundTrack, Basic};
    let (kept, holding) = played_in_background(snap, tiles, events);
    if !kept.is_empty() {
        let names: Vec<&str> = kept.iter().filter_map(|&k| tiles.playlists.get(k)).map(|p| p.name.as_str()).collect();
        log::info!("recs: played in the background: {}", names.join(", "));
    }
    snap.catalog
        .tracks()
        .iter()
        .map(|t| {
            let (Some(song), Some(f)) = (snap.types.get(t.id), tiles.facts.get(&t.id)) else { return false };
            let background_playlist = holding.get(&t.id).is_some_and(|l| l.iter().any(|k| kept.contains(k)));
            Basic.suits(&BackgroundTrack { song, title: &f.title, album: f.album.as_deref(), artist: &f.artist, background_playlist })
        })
        .collect()
}

/// The playlists of `tiles` the user plays in the background (their
/// places), and the playlists of each track.
pub(super) fn played_in_background(
    snap: &Snapshot,
    tiles: &nikgob_recs::TilesInput,
    events: &[nikgob_core::model::Event],
) -> (HashSet<usize>, HashMap<TrackId, Vec<usize>>) {
    use nikgob_core::model::EventType;
    let by_key: HashMap<&str, usize> = tiles.playlists.iter().enumerate().map(|(k, p)| (p.key.as_str(), k)).collect();
    let mut holding: HashMap<TrackId, Vec<usize>> = HashMap::new();
    for (k, p) in tiles.playlists.iter().enumerate() {
        for t in &p.tracks {
            holding.entry(*t).or_default().push(k);
        }
    }
    let mut plays: Vec<(usize, UnixMs)> = Vec::new();
    for e in events.iter().filter(|e| e.event_type == EventType::Start) {
        match e.context_id.as_deref().and_then(|c| c.strip_prefix("playlist:")) {
            Some(id) => plays.extend(by_key.get(id).map(|&k| (k, e.ts))),
            None if e.context == EventContext::Playlist && e.context_id.is_none() => {
                plays.extend(holding.get(&e.track_id).into_iter().flatten().map(|&k| (k, e.ts)));
            }
            None => {}
        }
    }
    (nikgob_recs::background::background_playlists(&plays, &snap.model.listening), holding)
}

/// One song on two albums ([`Library::recording_spares`]: each other copy
/// with the copy standing for it): the plays of a copy are the standing
/// copy's, and its like is that one's (once, the newest time).
pub(crate) fn fold_recordings(
    spares: &HashMap<TrackId, TrackId>,
    events: &mut [nikgob_core::model::Event],
    likes: &mut Vec<(TrackId, UnixMs)>,
) {
    if spares.is_empty() {
        return;
    }
    for e in events.iter_mut() {
        if let Some(c) = spares.get(&e.track_id) {
            e.track_id = *c;
        }
    }
    for l in likes.iter_mut() {
        if let Some(c) = spares.get(&l.0) {
            l.0 = *c;
        }
    }
    // Newest first: the first of each track stays.
    likes.sort_by_key(|l| std::cmp::Reverse(l.1));
    let mut seen = HashSet::new();
    likes.retain(|l| seen.insert(l.0));
}

/// An event as the recommendations read it: a play imported before
/// imports had their origin (the import's device) is an import.
pub(crate) fn play_of(e: &nikgob_core::model::Event) -> nikgob_recs::Play {
    let mut p = nikgob_recs::Play::from(e);
    if p.origin.is_none() && e.device.as_str() == crate::streams::IMPORT_DEVICE {
        p.origin = Some(nikgob_core::model::EventOrigin::Import);
    }
    p
}

/// The kind of device this process plays on ([`v::MeshKind::ALL`]'s
/// place; a computer until it is told): one fact of the process, read by
/// the builds on their threads, which have the library only.
static OWN_KIND: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// This process plays on a `kind` of device (the Pi's daemon is a Pi).
/// Whether it changed.
pub(crate) fn set_own_kind(kind: v::MeshKind) -> bool {
    let k = v::MeshKind::ALL.iter().position(|x| *x == kind).unwrap_or(0) as u8;
    OWN_KIND.swap(k, std::sync::atomic::Ordering::Relaxed) != k
}

/// See [`set_own_kind`].
pub(crate) fn own_kind() -> v::MeshKind {
    v::MeshKind::ALL.get(usize::from(OWN_KIND.load(std::sync::atomic::Ordering::Relaxed))).copied().unwrap_or_default()
}

/// What kind of device each play was on, by the id it is stamped with:
/// this device's own ids its own kind, a paired device's as its row says;
/// a guest's device is no one's history. Any other id (one from before a
/// device took its node's, an import) a personal device.
pub(crate) struct Devices {
    own: HashSet<String>,
    here: nikgob_recs::PlayDevice,
    nodes: HashMap<String, Option<nikgob_recs::PlayDevice>>,
}

impl Devices {
    pub(crate) fn read(lib: &Library) -> Self {
        let here = nikgob_recs::PlayDevice::of_kind(own_kind().key());
        let own = lib.own_device_ids().unwrap_or_else(|e| {
            log::warn!("recs: this device's ids: {e}");
            HashSet::new()
        });
        let nodes = lib
            .nodes()
            .unwrap_or_default()
            .into_iter()
            .map(|n| {
                let kind = (!n.scope.eq_ignore_ascii_case(v::MeshScope::Guest.key())).then(|| nikgob_recs::PlayDevice::of_kind(&n.kind));
                (n.id, kind)
            })
            .collect();
        Self { own, here, nodes }
    }

    /// The device the taste is for: this one.
    pub(crate) fn here(&self) -> nikgob_recs::PlayDevice {
        self.here
    }

    /// The kind of device `device` is; none for a guest's.
    pub(crate) fn of(&self, device: &str) -> Option<nikgob_recs::PlayDevice> {
        if self.own.contains(device) {
            return Some(self.here);
        }
        self.nodes.get(device).copied().unwrap_or(Some(nikgob_recs::PlayDevice::Personal))
    }

    /// `p` (read of `e`) with the device it was played on; none when it
    /// was a guest's device.
    pub(crate) fn place(&self, e: &nikgob_core::model::Event, mut p: nikgob_recs::Play) -> Option<nikgob_recs::Play> {
        p.device = self.of(e.device.as_str())?;
        Some(p)
    }
}

/// The local offsets (minutes east of UTC) of this device's time zone from
/// `from` to `to`: the one at `from`, then each change by the zone's rules,
/// oldest first ([`nikgob_recs::TasteInput::utc_offsets`]).
pub(crate) fn local_offsets(from: UnixMs, to: UnixMs) -> Vec<(UnixMs, i32)> {
    let Ok(start) = jiff::Timestamp::from_millisecond(from) else { return Vec::new() };
    let tz = jiff::tz::TimeZone::system();
    let mut out = vec![(from, tz.to_offset(start).seconds() / 60)];
    for t in tz.following(start) {
        let at = t.timestamp().as_millisecond();
        // Two changes a year: a long history has a few dozen.
        if at > to || out.len() >= 4_096 {
            break;
        }
        out.push((at, t.offset().seconds() / 60));
    }
    out
}

/// Whether each of `plays` counts whole on a list of what was played (a
/// tile, «Слушать снова»): what a room heard on and on from afar, or the
/// assistant put on in passing, weighs less than half a play there
/// ([`nikgob_recs::context::event_weights`]).
pub(crate) fn whole(plays: &[nikgob_recs::Play]) -> Vec<bool> {
    nikgob_recs::context::event_weights(plays, nikgob_recs::PlayDevice::Personal)
        .into_iter()
        .map(|w| w >= nikgob_recs::context::FULL_PLAY)
        .collect()
}

/// The taste profiles of a build in the log: a line
/// of them all, then each, the largest share first — its name and artists,
/// its share of the recent plays, how much of it is background, three of
/// its best loved songs; and how the user listens.
fn log_profiles(model: &UserModel, tiles: &nikgob_recs::TilesInput, names: &HashMap<ArtistId, String>, style: v::ProfileNames, loc: Locale) {
    log::info!("{}", listening_line(model));
    let lines = profile_lines(model, tiles, names, style, loc);
    let short: Vec<String> = profile_order(model).into_iter().map(|j| format!("{} {:.2}", profile_name(model, names, style, loc, j).0, model.profile_share.get(j).copied().unwrap_or(0.0))).collect();
    let born = model.profiles.list.iter().filter(|p| !p.visible).count();
    log::info!("profiles: {} ({}), not shown yet {born}", model.profiles.len(), short.join(", "));
    for line in lines {
        log::info!("recs: profile {line}");
    }
}

/// How the user listens, in a line: the sessions, the background ones and
/// the threshold that told them.
pub(super) fn listening_line(model: &UserModel) -> String {
    let l = &model.listening;
    format!(
        "listening: {} sessions, {} background{}",
        l.sessions.len(),
        l.background_sessions(),
        match (l.has_background, l.threshold) {
            (false, _) => " (no background listening)".to_owned(),
            (true, Some(t)) => format!(" (threshold {t:.2}, separation {:.2})", l.eta),
            (true, None) => format!(" (the fixed band, separation {:.2})", l.eta),
        }
    )
}

/// The profiles, the largest share first.
pub(super) fn profile_order(model: &UserModel) -> Vec<usize> {
    let mut order: Vec<usize> = (0..model.profiles.len()).collect();
    order.sort_by(|a, b| model.profile_share[*b].total_cmp(&model.profile_share[*a]).then(a.cmp(b)));
    order
}

/// The name and the subtitle of profile `j` in `loc`.
pub(super) fn profile_name(model: &UserModel, names: &HashMap<ArtistId, String>, style: v::ProfileNames, loc: Locale, j: usize) -> (String, String) {
    let style = match style {
        v::ProfileNames::Kind => nikgob_recs::profiles::NameStyle::Kind,
        v::ProfileNames::Artists => nikgob_recs::profiles::NameStyle::Artists,
    };
    model.profiles.name_in(j, style, loc, &|a| names.get(&a).cloned())
}

/// [`log_profiles`]' lines of each profile.
pub(crate) fn profile_lines(model: &UserModel, tiles: &nikgob_recs::TilesInput, names: &HashMap<ArtistId, String>, style: v::ProfileNames, loc: Locale) -> Vec<String> {
    let now = model.now();
    let mut out = Vec::new();
    for j in profile_order(model) {
        let p = &model.profiles.list[j];
        let mut members: Vec<usize> = (0..model.len()).filter(|&i| model.profile_at(i) == Some(j) && model.tracks[i].aff_long(now) > 0.0).collect();
        members.sort_by(|a, b| model.tracks[*b].aff_long(now).total_cmp(&model.tracks[*a].aff_long(now)).then(a.cmp(b)));
        let songs: Vec<String> = members
            .iter()
            .take(3)
            .filter_map(|&i| tiles.facts.get(&model.id(i)).map(|f| format!("«{}» {}", f.title, f.artist)))
            .collect();
        let (name, sub) = profile_name(model, names, style, loc, j);
        out.push(format!(
            "{name} ({sub}): {:?}, key {}{}, {:.0} % of the plays, background {:.2}, {} members; {}",
            p.lane,
            p.key,
            if p.visible { "" } else { ", not shown yet" },
            100.0 * model.profile_share.get(j).copied().unwrap_or(0.0),
            model.profile_bg.get(j).copied().unwrap_or(0.0),
            p.members,
            songs.join(", ")
        ));
    }
    out
}

/// What `recs.song_types` keeps: the types and what they were told from.
#[derive(Serialize, Deserialize)]
struct SongTypesCache {
    rules: u32,
    /// [`fingerprint`] of the inputs.
    of: u64,
    types: SongTypes,
}

/// A stable hash of everything the song types are told from.
fn fingerprint(inputs: &[SongInput]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h = (h ^ u64::from(*b)).wrapping_mul(0x100_0000_01b3);
        }
        h = (h ^ 0xff).wrapping_mul(0x100_0000_01b3);
    };
    for t in inputs {
        eat(t.id.as_uuid().as_bytes());
        eat(t.main_artist.map(|a| *a.as_uuid().as_bytes()).unwrap_or_default().as_slice());
        eat(t.artist_name.as_bytes());
        eat(t.title.as_bytes());
        eat(t.credit.as_bytes());
        eat(t.album.as_deref().unwrap_or_default().as_bytes());
        eat(&[u8::from(t.ost), u8::from(t.synced_lyrics)]);
        match &t.lyrics {
            Some((flag, text)) => eat(format!("{flag}{text}").as_bytes()),
            None => eat(b"-"),
        }
        if let Some(z) = &t.z {
            for x in z {
                eat(&x.to_le_bytes());
            }
        }
        match &t.sketch {
            Some(s) => eat(&s.to_bytes()),
            None => eat(b"-"),
        }
    }
    h
}

/// The types of the catalog's tracks: kept in `recs.song_types` while
/// their lyrics, sound, names and the rules are the same.
fn read_song_types(
    lib: &Library,
    catalog: &Catalog,
    index: &VectorIndex<TrackId>,
    tiles: &nikgob_recs::TilesInput,
    names: &HashMap<ArtistId, String>,
) -> SongTypes {
    let samples = lib.lyrics_samples().unwrap_or_else(|e| {
        log::warn!("recs: lyrics for the song types: {e}");
        Vec::new()
    });
    let lyrics: HashMap<TrackId, (bool, String)> = samples.into_iter().map(|s| (s.id, (s.instrumental, s.text))).collect();
    let norm = index.normalizer();
    let mut inputs: Vec<SongInput> = catalog
        .tracks()
        .iter()
        .map(|t| {
            let f = tiles.facts.get(&t.id);
            let main = t.artists.first().copied();
            let album = f.and_then(|f| f.album.clone());
            SongInput {
                id: t.id,
                main_artist: main,
                artist_name: main.and_then(|a| names.get(&a)).cloned().unwrap_or_default(),
                title: f.map(|f| f.title.clone()).unwrap_or_default(),
                credit: f.map(|f| f.artist.clone()).unwrap_or_default(),
                ost: album.as_deref().and_then(nikgob_recs::tiles::franchise).is_some(),
                album,
                lyrics: lyrics.get(&t.id).cloned(),
                synced_lyrics: f.is_some_and(|f| matches!(f.lyrics, nikgob_recs::LyricsKind::Synced | nikgob_recs::LyricsKind::Words)),
                z: index.get(t.id).map(|v| norm.z(v)),
                sketch: index.sketch(t.id).copied(),
            }
        })
        .collect();
    inputs.sort_by_key(|t| t.id);
    // The model's vectors tell the voice too (their probe is trained on the
    // library's labels and their mean): which tracks have one, and each
    // vector whole, change the types.
    let of = inputs.iter().fold(fingerprint(&inputs), |h, t| embedding_fingerprint(h, index.embedding(t.id).as_ref()));
    let cached: Option<SongTypesCache> = lib
        .cache_get(SONG_TYPES_KEY)
        .ok()
        .flatten()
        .and_then(|(_, json)| serde_json::from_str(&json).ok())
        .filter(|c: &SongTypesCache| c.rules == SONGTYPE_RULES && c.of == of);
    if let Some(c) = cached {
        return c.types;
    }
    let types = nikgob_recs::song_types_with(&inputs, &|id| index.embedding(id));
    let cache = SongTypesCache { rules: SONGTYPE_RULES, of, types };
    if let Ok(json) = serde_json::to_string(&cache)
        && let Err(e) = lib.cache_put(SONG_TYPES_KEY, now_ms(), &json)
    {
        log::warn!("recs: cannot keep the song types: {e}");
    }
    cache.types
}
