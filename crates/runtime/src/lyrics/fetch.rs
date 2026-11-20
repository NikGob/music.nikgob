use super::*;

/// A `.ttml` (Apple Music style, word-synced) next to the file, and
/// whether it is the user's own text.
fn local_ttml(file: &Path) -> Option<(Text, bool)> {
    read_ttml(&file.with_extension("ttml"))
}

/// The `.ttml` at `path`, and whether it is the user's own text
/// ([`nikgob_metadata::ttml_is_own`]).
pub(crate) fn read_ttml(path: &Path) -> Option<(Text, bool)> {
    let xml = std::fs::read_to_string(nikgob_import::sanitize::long_path(path)).ok()?;
    Some((Text::from_ttml(&xml)?, nikgob_metadata::ttml_is_own(&xml)))
}

/// The artists LyricsPlus is asked with, in order: the full credit
/// («WENARO, LXNER»), then the main artist alone when that differs.
fn lyricsplus_artists(lib: &Library, row: &TrackRow) -> Vec<String> {
    let names: Vec<String> = lib
        .get_track(row.id)
        .ok()
        .flatten()
        .map(|t| t.artist_credit.0.into_iter().map(|p| p.name).filter(|n| !n.trim().is_empty()).collect())
        .unwrap_or_default();
    let (full, main) = match names.first() {
        Some(first) => (names.join(", "), first.clone()),
        None => {
            let main = row.artist.split([',', '&']).next().unwrap_or(&row.artist);
            let main = main.split(" feat").next().unwrap_or(main).trim();
            (row.artist.clone(), main.to_owned())
        }
    };
    let mut out = vec![full];
    if !main.is_empty() && !out.contains(&main) {
        out.push(main);
    }
    out
}

/// The artists Genius is asked with, the main one first: each credited
/// artist on its own («MellSher», «и ещё 1 исполнитель»), or the shown
/// credit when the track has none.
pub(super) fn genius_artists(row: &TrackRow) -> Vec<String> {
    let names: Vec<String> = row.credit.0.iter().map(|p| p.name.trim().to_owned()).filter(|n| !n.is_empty()).collect();
    if names.is_empty() { vec![row.artist.clone()] } else { names }
}

/// LyricsPlus for `row`: the full credit first, the main artist only when
/// that finds nothing (see the module docs).
async fn find_lyricsplus(meta: &Metadata, lib: &Library, row: &TrackRow) -> Result<Option<Text>, MetadataError> {
    let duration = row.duration_ms.map(|ms| Duration::from_millis(ms.into()));
    for artist in lyricsplus_artists(lib, row) {
        if let Some(t) = meta.lyricsplus.find_text(&artist, &row.title, row.album.as_deref(), duration).await? {
            log::debug!(
                "lyricsplus: «{artist} — {}» from {}{}",
                row.title,
                t.provider.as_deref().unwrap_or("?"),
                if t.words_dropped { ", generated word timing dropped" } else { "" }
            );
            if t.lines.is_empty() {
                // A timing type with no times: the words alone.
                return Ok(t.plain.as_deref().and_then(Text::from_plain));
            }
            return Ok(Text::from_lines(t.lines).map(|x| Text { weak: t.words_dropped, ..x }));
        }
    }
    Ok(None)
}

/// Asks an online source and caches the answer ("not found" included) as
/// parsed lines, keyed by the track's id (a retag keeps it). Network
/// failures are returned and nothing is stored; a miss leaves a stored
/// text alone.
pub(super) async fn fetch_online(
    meta: Metadata,
    lib: Library,
    row: TrackRow,
    source: LyricsSource,
) -> Result<Option<Text>, MetadataError> {
    let duration = row.duration_ms.map(|ms| Duration::from_millis(ms.into()));
    let lookup = async {
        Ok::<_, MetadataError>(match source {
            LyricsSource::LyricsPlus => find_lyricsplus(&meta, &lib, &row).await?,
            LyricsSource::LrcRed => {
                let isrc = lib.get_track(row.id).ok().flatten().and_then(|t| t.isrc);
                meta.lrcred
                    .find(&meta.deezer, isrc.as_deref(), &row.artist, &row.title, duration)
                    .await?
                    .and_then(Text::from_lines)
            }
            LyricsSource::BetterLyrics => meta.betterlyrics.find(&row.artist, &row.title).await?.and_then(Text::from_lines),
            LyricsSource::Genius => {
                let artists = genius_artists(&row);
                let artists: Vec<&str> = artists.iter().map(String::as_str).collect();
                meta.genius.find(&artists, &row.title, row.album.as_deref()).await?.and_then(|t| Text::from_plain(&t))
            }
            LyricsSource::Own | LyricsSource::File | LyricsSource::Lrclib | LyricsSource::Recognized => None,
        })
    };
    let text = tokio::time::timeout(LOOKUP_TIMEOUT, lookup)
        .await
        .unwrap_or_else(|_| Err(MetadataError::Network("timeout".into())))?;
    // A miss never takes the place of a text found earlier (a search
    // asked again): the text is kept.
    let kept = text.is_none() && matches!(lib.lyrics_text(row.id, source.key()), Ok(Some((Some(_), _))));
    let body = text.as_ref().and_then(Text::body);
    if !kept && let Err(e) = lib.put_lyrics_text(row.id, source.key(), body.as_deref()) {
        log::warn!("caching lyrics ({}): {e}", source.key());
    }
    Ok(text)
}

/// What the lyrics of `row` are looked up by: a music video (a clip that
/// stays one) by its song's title, without what clips add («Starboy ft.
/// Daft Punk (Official Video)» → «Starboy ft. Daft Punk»), and with no
/// length (the video's is not the song's: an exact match by it finds a
/// text timed for another cut). Its timing is unverified until the check
/// ([`TrackLyrics::clip`]).
pub(crate) fn lookup_row(mut row: TrackRow) -> TrackRow {
    if row.is_clip() {
        let artist = row.credit.primary().map_or_else(|| row.artist.clone(), |p| p.name.clone());
        row.title = nikgob_core::clip_title::display_title(&row.title, &artist);
        row.duration_ms = None;
    }
    row
}

/// One source's lookup of `row` (LRCLIB through its own cache), noted in
/// the sources' pauses ([`nikgob_metadata::pause`]): refusals in a row make
/// the source sit out, logged once when the pause starts.
pub(super) async fn ask_source(meta: Metadata, lib: Library, row: TrackRow, source: LyricsSource) -> Result<Option<Text>, MetadataError> {
    let row = lookup_row(row);
    let pauses = meta.lyrics_pauses.clone();
    let got = if source == LyricsSource::Lrclib {
        crate::enrich::fetch_and_store(&meta.lrclib, &lib, &row).await
            .map(|st| Text::from_lrc(st.synced.as_deref(), st.plain.as_deref()))
    } else {
        fetch_online(meta, lib.clone(), row, source).await
    };
    let verdict = nikgob_metadata::Verdict::of(&got);
    let key = pause_key(source);
    if let Some(pause) = pauses.note(source.key(), verdict, std::time::Instant::now()) {
        let now = now_ms();
        let until = now + i64::try_from(pause.as_millis()).unwrap_or(i64::MAX / 2);
        if let Err(e) = lib.cache_put(&key, now, &until.to_string()) {
            log::debug!("lyrics ({}): keeping the pause: {e}", source.key());
        }
        if let Err(e) = &got {
            log::warn!("lyrics ({}): {e}; not asked for {} min", source.key(), pause.as_secs() / 60);
        }
    } else if verdict == nikgob_metadata::Verdict::Answered && lib.cache_get(&key).ok().flatten().is_some() {
        let _ = lib.cache_drop(&key);
    }
    got
}

/// Where a source's pause is kept, so it outlasts the app.
fn pause_key(source: LyricsSource) -> String {
    format!("lyrics_pause.{}", source.key())
}

/// The pauses noted before the app last quit, still running, go on
/// ([`ask_source`] keeps them).
pub(crate) fn restore_pauses(meta: &Metadata, lib: &Library) {
    let now = now_ms();
    for source in LyricsSource::ALL {
        let Ok(Some((_, until))) = lib.cache_get(&pause_key(source)) else { continue };
        let Some(left) = until.parse::<i64>().ok().and_then(|u| u64::try_from(u - now).ok()).filter(|l| *l > 0) else {
            continue;
        };
        meta.lyrics_pauses.hold(source.key(), std::time::Instant::now() + Duration::from_millis(left));
    }
}

/// Sitting out ([`ask_source`]): taken out of `tl`'s lookups as a failure
/// (not a miss: asked again when the song is shown after the pause).
fn sitting_out(meta: &Metadata, source: LyricsSource) -> bool {
    meta.lyrics_pauses.paused_for(source.key(), std::time::Instant::now()).is_some()
}

/// `tl`'s next lookups without the sources sitting out; those are taken
/// out of its lookups as failures, and the ones after them come up.
pub(super) fn next_asked(tl: &mut TrackLyrics, meta: &Metadata) -> Vec<LyricsSource> {
    loop {
        let (held, ask): (Vec<LyricsSource>, Vec<LyricsSource>) = tl.next_lookups().into_iter().partition(|s| sitting_out(meta, *s));
        if held.is_empty() {
            return ask;
        }
        for s in &held {
            tl.pending.remove(s);
        }
        tl.failed = true;
        if !ask.is_empty() {
            return ask;
        }
    }
}

/// A text cached by [`fetch_online`]: a list of lines, a weak text or a
/// plain one.
pub(crate) fn stored_lines(body: &str) -> Option<Text> {
    if let Ok(lines) = serde_json::from_str::<Vec<LyricLine>>(body) {
        return Text::from_lines(lines);
    }
    let s: Stored = serde_json::from_str(body).ok()?;
    if s.lines.is_empty() {
        return s.plain.as_deref().and_then(Text::from_plain);
    }
    Text::from_lines(s.lines).map(|t| Text { weak: s.weak, ..t })
}

/// Reads only files and caches; expired misses become pending lookups
/// (Genius only when `genius`). `kept`: the text kept in its artist's
/// folder for a song without a file ([`nikgob_import::layout::kept_text_path`]).
pub(crate) fn cached_lyrics(
    lib: &Library,
    track: TrackId,
    path: Option<&Path>,
    tags: Option<&nikgob_import::tags::TagSet>,
    genius: bool,
    kept: Option<&Path>,
) -> TrackLyrics {
    cached_lyrics_at(lib, track, path, tags, genius, kept, now_ms())
}

/// [`cached_lyrics`] at `now` (unix ms): a found text is never asked for
/// again, a miss younger than [`LYRICS_RETRY_MS`] holds ([`miss_holds`]),
/// anything else is a pending lookup.
pub(super) fn cached_lyrics_at(
    lib: &Library,
    track: TrackId,
    path: Option<&Path>,
    tags: Option<&nikgob_import::tags::TagSet>,
    genius: bool,
    kept: Option<&Path>,
    now: i64,
) -> TrackLyrics {
    // One song on two albums: the lyrics and the word timing found for the
    // other copy are this one's when the two are as long (never fetched
    // twice).
    match lib.share_recording_lyrics(track) {
        Ok(true) => log::info!("lyrics of {track}: from the same recording on another album"),
        Ok(false) => {}
        Err(e) => log::warn!("lyrics of {track}: the same recording's: {e}"),
    }
    let mut tl = TrackLyrics {
        pick: lib
            .lyrics_pick(track)
            .ok()
            .flatten()
            .and_then(|k| LyricsSource::from_key(&k)),
        aligned: lib.lyrics_aligned(track).unwrap_or_default().iter().filter_map(Aligned::from_row).collect(),
        genius,
        display: shown_as(lib, track),
        names: lib.get_track(track).ok().flatten().map(|t| t.artist_credit.0.into_iter().map(|p| p.name).collect()).unwrap_or_default(),
        ..Default::default()
    };
    let fresh_miss = |at: i64| now - at < LYRICS_RETRY_MS;

    // The `.ttml` next to the file (or kept for a song without one): the
    // user's own text, or the file's; the user's leaves the file's text to
    // its tag.
    let ttml = match path {
        Some(p) => local_ttml(p),
        None => kept.and_then(read_ttml),
    };
    let file = match ttml {
        Some((t, true)) => {
            tl.insert(LyricsSource::Own, Some(t));
            None
        }
        Some((t, false)) => Some(t),
        None => None,
    };
    if let Some(t) = file {
        tl.insert(LyricsSource::File, Some(t));
    } else if let Some((synced, plain)) = path
        .and_then(|p| crate::enrich::local_lyrics(p, tags))
    {
        tl.insert(LyricsSource::File, Text::from_lrc(synced.as_deref(), plain.as_deref()));
    }
    let mut need = Vec::new();
    match lib.lyrics(track).ok().flatten() {
        Some(st) if !st.is_miss() => tl.insert(
            LyricsSource::Lrclib,
            Text::from_lrc(st.synced.as_deref(), st.plain.as_deref()),
        ),
        Some(st) if fresh_miss(st.fetched_at) => tl.insert(LyricsSource::Lrclib, None),
        _ => need.push(LyricsSource::Lrclib),
    }
    for source in [LyricsSource::LyricsPlus, LyricsSource::LrcRed, LyricsSource::BetterLyrics, LyricsSource::Genius] {
        match lib.lyrics_text(track, source.key()).ok().flatten() {
            Some((Some(body), _)) => tl.insert(source, stored_lines(&body)),
            Some((None, at)) if miss_holds(source, at, now) => tl.insert(source, None),
            _ if source == LyricsSource::Genius && !genius => {}
            _ => need.push(source),
        }
    }
    // Heard in the recording earlier (never looked up online).
    if let Some((Some(body), _)) = lib.lyrics_text(track, LyricsSource::Recognized.key()).ok().flatten() {
        tl.insert(LyricsSource::Recognized, stored_lines(&body));
    }
    tl.pending.extend(need);
    tl
}

/// A miss stored at `at` still holds at `now`: younger than
/// [`LYRICS_RETRY_MS`], and for Genius found by its current matching (an
/// older miss is asked again).
pub(super) fn miss_holds(source: LyricsSource, at: i64, now: i64) -> bool {
    now - at < LYRICS_RETRY_MS && (source != LyricsSource::Genius || at >= nikgob_metadata::genius::MATCHING_SINCE)
}

/// How the user chose to see `track`'s lyrics ([`TrackLyrics::display`]).
pub(super) fn shown_as(lib: &Library, track: TrackId) -> Option<LyricsSync> {
    match lib.lyrics_show(track).ok().flatten()?.as_str() {
        "word" => Some(LyricsSync::Word),
        "line" => Some(LyricsSync::Line),
        "plain" => Some(LyricsSync::Plain),
        _ => None,
    }
}

/// The key [`shown_as`] reads.
pub(super) fn show_key(sync: LyricsSync) -> &'static str {
    match sync {
        LyricsSync::Plain => "plain",
        LyricsSync::Line => "line",
        LyricsSync::Word | LyricsSync::Syllable => "word",
    }
}

/// Background backfill uses the same cache, fallback gates and ranking as playback.
pub(crate) async fn fetch_track_lyrics(meta: Metadata, lib: Library, row: TrackRow, path: Option<PathBuf>, genius: bool, kept: Option<PathBuf>) -> TrackLyrics {
    let tags = path.as_deref().and_then(|p| nikgob_import::tags::read_tags(p).ok()).map(|(t, _)| t);
    let mut tl = cached_lyrics(&lib, row.id, path.as_deref(), tags.as_ref(), genius, kept.as_deref());
    loop {
        let sources = next_asked(&mut tl, &meta);
        if sources.is_empty() { break }
        for source in sources {
            let result = ask_source(meta.clone(), lib.clone(), row.clone(), source).await;
            match result {
                Ok(text) => tl.found(source, text),
                Err(e) => {
                    // Logged as a playback lookup is: not a miss, asked again.
                    log::info!("lyrics backfill ({}): {e}", source.key());
                    tl.pending.remove(&source);
                    tl.failed = true;
                }
            }
        }
    }
    tl
}

pub(crate) fn embed_text(path: &Path, text: &str) -> nikgob_import::Result<()> {
    let (tags, _) = nikgob_import::tags::read_tags(path)?;
    if tags.lyrics.as_deref() != Some(text) {
        nikgob_import::tags::write_lyrics(path, text)?;
    }
    Ok(())
}
