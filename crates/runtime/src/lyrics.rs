//! File, LyricsPlus, lrc.red, Better Lyrics and LRCLIB lyrics. Synced file
//! lyrics win automatically; online texts compete by sync, then source order.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Duration;

use nikgob_core::lyrics::{LyricLine, LyricsSource, LyricsSync};
use nikgob_core::model::TrackId;
use nikgob_library::{Library, TrackRow, now_ms};
use nikgob_metadata::{Metadata, MetadataError};
use nikgob_view::{self as v};

use crate::Worker;
use crate::enrich::LYRICS_RETRY_MS;
use crate::import::Internal;

pub(crate) const LOOKUP_TIMEOUT: Duration = Duration::from_secs(20);
/// Tracks whose lyrics stay in memory besides the ones in use. Every source's
/// text down to syllables is kept per track, and the background backfill
/// goes through the whole library; the library rebuilds an entry on demand.
pub(crate) const LYRICS_KEPT: usize = 16;

/// One source's text.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Text {
    pub(crate) lines: Vec<LyricLine>,
    pub(crate) plain: Option<String>,
}

impl Text {
    /// From LRC text and/or plain text; `None` when both are empty.
    pub(crate) fn from_lrc(synced: Option<&str>, plain: Option<&str>) -> Option<Self> {
        let lines = synced.map(nikgob_metadata::parse_lrc_lines).unwrap_or_default();
        let plain = plain
            .filter(|p| lines.is_empty() && !p.trim().is_empty())
            .map(str::to_owned);
        (!lines.is_empty() || plain.is_some()).then_some(Self { lines, plain })
    }

    pub(crate) fn from_ttml(xml: &str) -> Option<Self> {
        let lines = nikgob_metadata::parse_ttml(xml)
            .map_err(|e| log::debug!("bad TTML: {e}"))
            .ok()?
            .lines;
        (!lines.is_empty()).then_some(Self { lines, plain: None })
    }

    fn from_lines(lines: Vec<LyricLine>) -> Option<Self> {
        (!lines.is_empty()).then_some(Self { lines, plain: None })
    }

    fn serialized(&self) -> String {
        self.plain.clone().unwrap_or_else(|| nikgob_metadata::lrc::serialize_lrc(&self.lines)).trim().to_owned()
    }

    fn sync(&self) -> LyricsSync {
        LyricsSync::of(&self.lines).unwrap_or(LyricsSync::Plain)
    }
}

/// Everything known about one track's lyrics.
#[derive(Clone, Debug, Default)]
pub(crate) struct TrackLyrics {
    texts: BTreeMap<LyricsSource, Text>,
    pending: HashSet<LyricsSource>,
    missing: HashSet<LyricsSource>,
    /// A lookup failed for lack of network: rebuilt (and retried) next time.
    pub(crate) failed: bool,
    pick: Option<LyricsSource>,
    embedding: bool,
    last_embed: Option<String>,
}

impl TrackLyrics {
    fn chosen(&self) -> Option<LyricsSource> {
        if let Some(p) = self.pick.filter(|p| self.texts.contains_key(p)) {
            return Some(p);
        }
        if self.texts.get(&LyricsSource::File).is_some_and(|t| t.sync() > LyricsSync::Plain)
            && !self.duplicate_file()
        {
            return Some(LyricsSource::File);
        }
        LyricsSource::ALL.into_iter()
            .filter(|s| *s != LyricsSource::File || !self.duplicate_file())
            .filter_map(|s| self.texts.get(&s).map(|t| (s, t.sync())))
            .max_by_key(|(source, sync)| (*sync, match source {
                LyricsSource::File => 0,
                LyricsSource::Lrclib => 1,
                LyricsSource::BetterLyrics => 2,
                LyricsSource::LrcRed => 3,
                LyricsSource::LyricsPlus => 4,
            }))
            .map(|(s, _)| s)
    }

    fn duplicate_file(&self) -> bool {
        self.texts.get(&LyricsSource::File).is_some_and(|file| {
            self.texts.iter().any(|(s, online)| *s != LyricsSource::File && (
                file == online || file.serialized() == online.serialized()
            ))
        })
    }

    /// Only confirmed misses enable Better Lyrics; failures never count as misses.
    fn next_lookups(&mut self) -> Vec<LyricsSource> {
        let primary = [LyricsSource::LyricsPlus, LyricsSource::LrcRed];
        if primary.iter().any(|s| self.pending.contains(s)) {
            return primary.into_iter().filter(|s| self.pending.contains(s)).collect();
        }
        if !primary.iter().all(|s| self.missing.contains(s)) {
            self.pending.remove(&LyricsSource::BetterLyrics);
        }
        if self.pending.contains(&LyricsSource::BetterLyrics) {
            vec![LyricsSource::BetterLyrics]
        } else if self.pending.contains(&LyricsSource::Lrclib) {
            vec![LyricsSource::Lrclib]
        } else {
            Vec::new()
        }
    }

    pub(crate) fn view(&self, track: TrackId) -> v::LyricsView {
        let source = self.chosen();
        let text = source.and_then(|s| self.texts.get(&s)).cloned().unwrap_or_default();
        let sources = LyricsSource::ALL
            .into_iter()
            .filter(|s| *s != LyricsSource::File || self.pick == Some(LyricsSource::File) || !self.duplicate_file())
            .filter_map(|s| {
                let state = if let Some(t) = self.texts.get(&s) {
                    v::LyricsAvail::Ready(t.sync())
                } else if self.pending.contains(&s) {
                    v::LyricsAvail::Loading
                } else if self.missing.contains(&s) {
                    v::LyricsAvail::Missing
                } else {
                    return None;
                };
                Some(v::LyricsOption { source: s, state })
            })
            .collect();
        v::LyricsView {
            track: Some(track),
            lines: text.lines,
            plain: text.plain,
            loading: source.is_none() && !self.pending.is_empty(),
            source,
            picked: self.pick.is_some() && self.pick == source,
            sources,
        }
    }

    /// A lookup or a file write is under way: its result needs this entry.
    fn busy(&self) -> bool {
        !self.pending.is_empty() || self.embedding
    }

    fn insert(&mut self, source: LyricsSource, text: Option<Text>) {
        self.pending.remove(&source);
        match text {
            Some(t) => {
                self.missing.remove(&source);
                self.texts.insert(source, t);
            }
            None => {
                self.missing.insert(source);
            }
        }

    }
}

/// A `.ttml` (Apple Music style, word-synced) next to the file.
fn local_ttml(file: &Path) -> Option<Text> {
    let path = nikgob_import::sanitize::long_path(&file.with_extension("ttml"));
    Text::from_ttml(&std::fs::read_to_string(path).ok()?)
}

/// Asks an online source and caches the answer ("not found" included) as
/// parsed lines. Network failures are returned and nothing is stored.
async fn fetch_online(
    meta: Metadata,
    lib: Library,
    row: TrackRow,
    source: LyricsSource,
) -> Result<Option<Text>, MetadataError> {
    let duration = row.duration_ms.map(|ms| Duration::from_millis(ms.into()));
    let album = row.album.as_deref();
    let lookup = async {
        Ok::<_, MetadataError>(match source {
            LyricsSource::LyricsPlus => meta.lyricsplus.find(&row.artist, &row.title, album, duration).await?,
            LyricsSource::LrcRed => {
                let isrc = lib.get_track(row.id).ok().flatten().and_then(|t| t.isrc);
                meta.lrcred.find(&meta.deezer, isrc.as_deref(), &row.artist, &row.title, duration).await?
            }
            LyricsSource::BetterLyrics => meta.betterlyrics.find(&row.artist, &row.title).await?,
            LyricsSource::File | LyricsSource::Lrclib => None,
        })
    };
    let lines = tokio::time::timeout(LOOKUP_TIMEOUT, lookup)
        .await
        .unwrap_or_else(|_| Err(MetadataError::Network("timeout".into())))?
        .filter(|l| !l.is_empty());
    let body = lines.as_ref().and_then(|l| serde_json::to_string(l).ok());
    let _ = lib.put_lyrics_text(row.id, source.key(), body.as_deref());
    Ok(lines.and_then(Text::from_lines))
}

/// Lines cached by [`fetch_online`].
fn stored_lines(body: &str) -> Option<Text> {
    Text::from_lines(serde_json::from_str(body).ok()?)
}

/// Reads only files and caches; expired misses become pending lookups.
fn cached_lyrics(lib: &Library, track: TrackId, path: Option<&Path>, tags: Option<&nikgob_import::tags::TagSet>) -> TrackLyrics {
    let mut tl = TrackLyrics {
        pick: lib
            .lyrics_pick(track)
            .ok()
            .flatten()
            .and_then(|k| LyricsSource::from_key(&k)),
        ..Default::default()
    };
    let fresh_miss = |at: i64| now_ms() - at < LYRICS_RETRY_MS;

    if let Some(t) = path.and_then(local_ttml) {
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
    for source in [LyricsSource::LyricsPlus, LyricsSource::LrcRed, LyricsSource::BetterLyrics] {
        match lib.lyrics_text(track, source.key()).ok().flatten() {
            Some((Some(body), _)) => tl.insert(source, stored_lines(&body)),
            Some((None, at)) if fresh_miss(at) => tl.insert(source, None),
            _ => need.push(source),
        }
    }
    tl.pending.extend(need);
    tl
}

/// Background backfill uses the same cache, fallback gates and ranking as playback.
pub(crate) async fn fetch_track_lyrics(meta: Metadata, lib: Library, row: TrackRow, path: Option<PathBuf>) -> TrackLyrics {
    let tags = path.as_deref().and_then(|p| nikgob_import::tags::read_tags(p).ok()).map(|(t, _)| t);
    let mut tl = cached_lyrics(&lib, row.id, path.as_deref(), tags.as_ref());
    loop {
        let sources = tl.next_lookups();
        if sources.is_empty() { break }
        for source in sources {
            let result = if source == LyricsSource::Lrclib {
                crate::enrich::fetch_and_store(&meta.lrclib, &lib, &row).await
                    .map(|st| Text::from_lrc(st.synced.as_deref(), st.plain.as_deref()))
            } else {
                fetch_online(meta.clone(), lib.clone(), row.clone(), source).await
            };
            match result {
                Ok(text) => tl.insert(source, text),
                Err(e) => {
                    log::debug!("lyrics backfill ({}): {e}", source.key());
                    tl.pending.remove(&source);
                    tl.failed = true;
                }
            }
        }
    }
    tl
}

/// Takes the oldest entries out of `order` (oldest first) until at most
/// `cap` are left, skipping the ones `spare` keeps; returns those taken.
fn evict(order: &mut VecDeque<TrackId>, cap: usize, spare: impl Fn(TrackId) -> bool) -> Vec<TrackId> {
    let mut gone = Vec::new();
    let mut i = 0;
    while order.len() > cap && i < order.len() {
        if spare(order[i]) {
            i += 1;
        } else {
            gone.extend(order.remove(i));
        }
    }
    gone
}

impl Worker {
    /// Stores `track`'s lyrics, then forgets the oldest ones past
    /// [`LYRICS_KEPT`]: not the shown, playing or next track, and not one
    /// with a lookup or a write still to come back.
    fn cache_lyrics(&mut self, track: TrackId, tl: TrackLyrics) {
        let keep = [
            Some(track),
            self.state.read().lyrics.track,
            self.queue.current().map(|e| e.track),
            self.queue.peek_auto_next().map(|e| e.track),
        ];
        let Some(svc) = self.import.as_mut() else { return };
        svc.lyrics_cache.insert(track, tl);
        svc.lyrics_order.retain(|t| *t != track);
        svc.lyrics_order.push_back(track);
        let (cache, later) = (&svc.lyrics_cache, &svc.embed_later);
        let spare = |t: TrackId| {
            keep.contains(&Some(t)) || later.contains(&t) || cache.get(&t).is_some_and(TrackLyrics::busy)
        };
        for t in evict(&mut svc.lyrics_order, LYRICS_KEPT, spare) {
            svc.lyrics_cache.remove(&t);
            // Rebuilt from the library next time; what it lacks is asked again.
            svc.lyrics_tried.retain(|(tried, _)| *tried != t);
        }
    }

    pub(crate) fn lyrics_backfilled(&mut self, track: TrackId, mut tl: TrackLyrics) {
        tl.pick = self.lib.lyrics_pick(track).ok().flatten().and_then(|k| LyricsSource::from_key(&k));
        let Some(svc) = self.import.as_mut() else { return };
        // Playback lookups keep ownership of their in-flight results.
        if svc.lyrics_cache.get(&track).is_some_and(|t| !t.pending.is_empty() || t.embedding) { return }
        let view = tl.view(track);
        self.cache_lyrics(track, tl);
        self.publish(|s, _| {
            if s.lyrics.track == Some(track) { s.lyrics = view; }
        });
        self.embed_lyrics(track);
    }

    /// Shows the track's lyrics, starting lookups for the sources not known yet.
    pub(crate) fn load_lyrics(
        &mut self,
        track: TrackId,
        path: Option<PathBuf>,
        tags: Option<&nikgob_import::tags::TagSet>,
    ) {
        if let Some(tl) = self.import.as_ref().and_then(|s| s.lyrics_cache.get(&track))
            && (!tl.failed || !tl.pending.is_empty())
        {
            let view = tl.view(track);
            self.publish(|s, _| s.lyrics = view);
            self.embed_lyrics(track);
            return;
        }
        let mut tl = cached_lyrics(&self.lib, track, path.as_deref(), tags);
        if let Some(svc) = self.import.as_mut() {
            if svc.metadata().is_none() {
                tl.pending.clear();
            }
        }
        if self.import.is_some() {
            self.cache_lyrics(track, tl.clone());
        }
        self.start_lyrics_lookups(track);
        let view = self.import.as_ref().and_then(|s| s.lyrics_cache.get(&track)).unwrap_or(&tl).view(track);
        self.publish(|s, _| s.lyrics = view);
        self.embed_lyrics(track);
    }

    fn start_lyrics_lookups(&mut self, track: TrackId) {
        let Some(row) = self.lib.get_track_row(track).ok().flatten() else { return };
        let Some(svc) = self.import.as_mut() else { return };
        let Some(meta) = svc.metadata() else { return };
        let Some(tl) = svc.lyrics_cache.get_mut(&track) else { return };
        let sources = tl.next_lookups();
        for source in sources {
            if !svc.lyrics_tried.insert((track, source)) {
                continue;
            }
            let (meta, lib, row, tx) = (meta.clone(), self.lib.clone(), row.clone(), svc.internal());
            svc.spawn(async move {
                let got = if source == LyricsSource::Lrclib {
                    crate::enrich::fetch_and_store(&meta.lrclib, &lib, &row).await
                        .map(|st| Text::from_lrc(st.synced.as_deref(), st.plain.as_deref()))
                } else {
                    fetch_online(meta, lib, row, source).await
                };
                let _ = tx.send(Internal::LyricsFetched(track, source, got.map_err(|e| e.to_string())));
            });
        }
    }

    pub(crate) fn embed_lyrics(&mut self, track: TrackId) {
        if !self.state.read().prefs.save_found_lyrics { return }
        let Some(src) = self.lib.active_source(track).ok().flatten().filter(|s| s.kind.is_local()) else { return };
        let path = self.res.path(&src.locator);
        let Some(svc) = self.import.as_mut() else { return };
        let Some(tl) = svc.lyrics_cache.get_mut(&track) else { return };
        // A manual online pick may be saved immediately; automatic selection waits.
        if tl.embedding { return }
        let Some(source) = tl.chosen().filter(|s| *s != LyricsSource::File) else { return };
        if tl.pick != Some(source) && !tl.pending.is_empty() { return }
        let text = tl.texts[&source].serialized();
        if tl.last_embed.as_deref() == Some(&text) { return }
        let in_use = |e: Option<crate::queue::Entry>| e.is_some_and(|e| e.track == track);
        if in_use(self.queue.current()) || in_use(self.queue.peek_auto_next()) {
            svc.embed_later.insert(track);
            return;
        }
        tl.last_embed = Some(text.clone());
        tl.embedding = true;
        let tx = svc.internal();
        svc.spawn_blocking(move || {
            let result = embed_text(&path, &text).map(|()| text).map_err(|e| e.to_string());
            if let Err(e) = &result { log::warn!("embedding lyrics: {e}"); }
            let _ = tx.send(Internal::LyricsEmbedded(track, result));
        });
    }

    pub(crate) fn lyrics_embedded(&mut self, track: TrackId, result: Result<String, String>) {
        let saved = result.is_ok();
        if let Some(tl) = self.import.as_mut().and_then(|s| s.lyrics_cache.get_mut(&track)) {
            tl.embedding = false;
            if let Ok(text) = result {
                tl.insert(LyricsSource::File, Text::from_lrc(Some(&text), Some(&text)));
            } else {
                tl.last_embed = None;
            }
            let view = tl.view(track);
            self.publish(|s, _| {
                if s.lyrics.track == Some(track) { s.lyrics = view; }
            });
        }
        if saved { self.embed_lyrics(track); }
    }

    pub(crate) fn lyrics_fetched(&mut self, track: TrackId, source: LyricsSource, got: Result<Option<Text>, String>) {
        let Some(svc) = &mut self.import else { return };
        let Some(tl) = svc.lyrics_cache.get_mut(&track) else { return };
        match got {
            Ok(text) => tl.insert(source, text),
            Err(e) => {
                log::debug!("lyrics lookup ({}): {e}", source.key());
                tl.pending.remove(&source);
                tl.failed = true;
                svc.lyrics_tried.remove(&(track, source));
            }
        }
        self.start_lyrics_lookups(track);
        let view = self.import.as_ref().unwrap().lyrics_cache[&track].view(track);
        self.embed_lyrics(track);
        self.publish(|s, _| {
            if s.lyrics.track == Some(track) {
                s.lyrics = view;
            }
        });
    }

    pub(crate) fn pick_lyrics_source(&mut self, track: TrackId, source: Option<LyricsSource>) {
        if let Err(e) = self.lib.set_lyrics_pick(track, source.map(LyricsSource::key)) {
            log::warn!("saving lyrics pick: {e}");
        }
        let Some(tl) = self.import.as_mut().and_then(|s| s.lyrics_cache.get_mut(&track)) else {
            return;
        };
        tl.pick = source;
        let view = tl.view(track);
        self.embed_lyrics(track);
        self.publish(|s, _| {
            if s.lyrics.track == Some(track) {
                s.lyrics = view;
            }
        });
    }
}

fn embed_text(path: &Path, text: &str) -> nikgob_import::Result<()> {
    let (tags, _) = nikgob_import::tags::read_tags(path)?;
    if tags.lyrics.as_deref() != Some(text) {
        nikgob_import::tags::write_lyrics(path, text)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn line(ms: u64, text: &str) -> LyricLine {
        LyricLine::new(Duration::from_millis(ms), text)
    }

    fn worded() -> Text {
        let mut l = line(1000, "a");
        l.words.push(nikgob_core::lyrics::LyricWord {
            start: Duration::from_millis(1000),
            end: Duration::from_millis(1500),
            text: "a".into(),
        });
        Text { lines: vec![l], plain: None }
    }

    #[test]
    fn eviction_drops_the_oldest_idle_lyrics_down_to_the_cap() {
        let ids: Vec<TrackId> = (0..=6).map(|_| TrackId::new()).collect();
        let id = |n: usize| ids[n];
        let mut order: VecDeque<TrackId> = (1..=6).map(id).collect();
        // 1 and 3 are busy (in use, a lookup on its way): they stay.
        let gone = evict(&mut order, 3, |t| t == id(1) || t == id(3));
        assert_eq!(gone, vec![id(2), id(4), id(5)]);
        assert_eq!(order, VecDeque::from(vec![id(1), id(3), id(6)]));
        // Everything busy: over the cap rather than losing a result.
        let gone = evict(&mut order, 1, |_| true);
        assert!(gone.is_empty());
        assert_eq!(order.len(), 3);
        assert!(evict(&mut order, 5, |_| false).is_empty());
    }

    #[test]
    fn best_text_wins_unless_picked() {
        let id = TrackId::new();
        let mut tl = TrackLyrics::default();
        tl.pending.insert(LyricsSource::LyricsPlus);
        tl.insert(LyricsSource::Lrclib, Text::from_lrc(Some("[00:01.00]a"), None));
        let v = tl.view(id);
        assert_eq!(v.source, Some(LyricsSource::Lrclib));
        assert!(!v.loading);
        assert_eq!(v.sources.len(), 2);

        tl.insert(LyricsSource::LyricsPlus, Some(worded()));
        assert_eq!(tl.view(id).source, Some(LyricsSource::LyricsPlus), "word sync beats line sync");

        tl.pick = Some(LyricsSource::Lrclib);
        let v = tl.view(id);
        assert_eq!(v.source, Some(LyricsSource::Lrclib));
        assert!(v.picked);

        // A pick whose text is gone falls back to the automatic choice.
        tl.pick = Some(LyricsSource::File);
        assert_eq!(tl.view(id).source, Some(LyricsSource::LyricsPlus));
    }

    #[test]
    fn syllable_sync_beats_word_sync_even_at_a_lower_source_priority() {
        let mut tl = TrackLyrics::default();
        tl.insert(LyricsSource::LyricsPlus, Some(worded()));
        let mut text = worded();
        text.lines[0].text = "ab".into();
        let mut piece = text.lines[0].words[0].clone();
        piece.text = "b".into();
        text.lines[0].words.push(piece);
        tl.insert(LyricsSource::Lrclib, Some(text));
        let view = tl.view(TrackId::new());
        assert_eq!(view.source, Some(LyricsSource::Lrclib));
        assert!(view.sources.iter().any(|o| o.state == v::LyricsAvail::Ready(LyricsSync::Syllable)));
    }

    #[test]
    fn written_lrc_is_not_a_second_source_and_misses_are_listed() {
        let id = TrackId::new();
        let mut tl = TrackLyrics::default();
        tl.insert(LyricsSource::File, Text::from_lrc(Some("[00:01.00]a"), None));
        tl.insert(LyricsSource::Lrclib, Text::from_lrc(Some("[00:01.00]a"), None));
        tl.insert(LyricsSource::LyricsPlus, None);
        let v = tl.view(id);
        assert_eq!(v.source, Some(LyricsSource::Lrclib));
        assert_eq!(
            v.sources,
            vec![
                v::LyricsOption {
                    source: LyricsSource::LyricsPlus,
                    state: v::LyricsAvail::Missing
                },
                v::LyricsOption {
                    source: LyricsSource::Lrclib,
                    state: v::LyricsAvail::Ready(LyricsSync::Line)
                },
            ]
        );
    }

    #[test]
    fn plain_text_and_loading() {
        let id = TrackId::new();
        let mut tl = TrackLyrics::default();
        tl.pending.insert(LyricsSource::Lrclib);
        assert!(tl.view(id).loading);
        tl.insert(LyricsSource::Lrclib, Text::from_lrc(None, Some("a\nb")));
        let v = tl.view(id);
        assert_eq!(v.plain.as_deref(), Some("a\nb"));
        assert!(v.lines.is_empty());
        assert!(Text::from_lrc(None, Some("  ")).is_none());
    }

    #[test]
    fn synced_file_wins_plain_file_loses_and_online_ties_follow_priority() {
        let mut tl = TrackLyrics::default();
        tl.insert(LyricsSource::File, Text::from_lrc(None, Some("local")));
        for source in [LyricsSource::Lrclib, LyricsSource::BetterLyrics, LyricsSource::LrcRed, LyricsSource::LyricsPlus] {
            tl.insert(source, Text::from_lrc(Some("[00:02.00]online"), None));
            assert_eq!(tl.chosen(), Some(source));
        }
        tl.insert(LyricsSource::Lrclib, Some(worded()));
        assert_eq!(tl.chosen(), Some(LyricsSource::Lrclib), "word beats line regardless of source");
        tl.insert(LyricsSource::File, Text::from_lrc(Some("[00:03.00]local"), None));
        assert_eq!(tl.chosen(), Some(LyricsSource::File), "synced file beats word-synced online");
        tl.pick = Some(LyricsSource::LyricsPlus);
        assert_eq!(tl.chosen(), tl.pick);
        assert_eq!(LyricsSource::from_key("netease"), None);
        assert_eq!(LyricsSource::from_key("qq"), None);
    }

    #[test]
    fn better_lyrics_requires_two_confirmed_misses_and_lrclib_is_last() {
        let mut tl = TrackLyrics::default();
        tl.pending.extend(LyricsSource::ALL.into_iter().filter(|s| *s != LyricsSource::File));
        assert_eq!(tl.next_lookups(), [LyricsSource::LyricsPlus, LyricsSource::LrcRed]);
        tl.insert(LyricsSource::LyricsPlus, None);
        assert_eq!(tl.next_lookups(), [LyricsSource::LrcRed]);
        tl.insert(LyricsSource::LrcRed, None);
        assert_eq!(tl.next_lookups(), [LyricsSource::BetterLyrics]);
        tl.insert(LyricsSource::BetterLyrics, None);
        assert_eq!(tl.next_lookups(), [LyricsSource::Lrclib]);

        for got in [Some(worded()), None] {
            let mut tl = TrackLyrics::default();
            tl.pending.extend([LyricsSource::BetterLyrics, LyricsSource::Lrclib]);
            tl.insert(LyricsSource::LyricsPlus, got);
            // lrc.red failed, so it is neither pending nor a confirmed miss.
            tl.failed = true;
            assert_eq!(tl.next_lookups(), [LyricsSource::Lrclib]);
            assert!(!tl.pending.contains(&LyricsSource::BetterLyrics));
        }
    }

    #[test]
    fn embedded_text_deduplicates_every_online_source_but_manual_file_still_works() {
        for source in LyricsSource::ALL.into_iter().filter(|s| *s != LyricsSource::File) {
            let mut tl = TrackLyrics::default();
            let text = worded();
            tl.insert(LyricsSource::File, Text::from_lrc(Some(&text.serialized()), None));
            tl.insert(source, Some(text));
            assert_eq!(tl.chosen(), Some(source));
            assert!(!tl.view(TrackId::new()).sources.iter().any(|s| s.source == LyricsSource::File));
            tl.pick = Some(LyricsSource::File);
            assert_eq!(tl.chosen(), Some(LyricsSource::File));
        }
    }

    #[test]
    fn background_groups_deduplicate_after_lrc_sorts_lines() {
        let mut online = worded();
        let mut background = line(5000, "oh");
        background.background = true;
        online.lines.extend([background, line(3000, "next")]);
        let mut tl = TrackLyrics::default();
        tl.insert(LyricsSource::File, Text::from_lrc(Some(&online.serialized()), None));
        tl.insert(LyricsSource::LyricsPlus, Some(online));
        assert!(tl.duplicate_file());
        assert_eq!(tl.chosen(), Some(LyricsSource::LyricsPlus));
    }

    #[test]
    fn identical_tag_is_not_rewritten() {
        use nikgob_import::{EncodeOptions, Encoder, RopusEncoder};
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("in.wav");
        let path = dir.path().join("song.opus");
        let mut writer = hound::WavWriter::create(&wav, hound::WavSpec {
            channels: 1, sample_rate: 48_000, bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        }).unwrap();
        for _ in 0..48_000 { writer.write_sample(0i16).unwrap(); }
        writer.finalize().unwrap();
        RopusEncoder.encode_to_opus(&wav, &path, &EncodeOptions { bitrate_kbps: 160 }).unwrap();
        let text = worded().serialized();
        embed_text(&path, &text).unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        embed_text(&path, &text).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), modified);
    }

    #[tokio::test]
    async fn online_cache_keeps_misses_but_never_records_server_errors() {
        use nikgob_core::model::{ArtistCredit, Track};
        use nikgob_metadata::{LyricsPlus, MetadataConfig};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        use wiremock::matchers::path;
        let server = MockServer::start().await;
        let mut meta = Metadata::new(&MetadataConfig::new("tests@example.invalid")).unwrap();
        meta.lyricsplus = LyricsPlus::with_base_url(nikgob_metadata::Http::new(&MetadataConfig::new("tests@example.invalid")).unwrap(), &server.uri()).unwrap();
        let lib = Library::open_in_memory().unwrap();
        let t = lib.upsert_track(&Track::new("Song", ArtistCredit::single("Artist", None))).unwrap();
        let row = lib.get_track_row(t.id).unwrap().unwrap();
        Mock::given(path("/v2/lyrics/get")).respond_with(ResponseTemplate::new(404)).mount(&server).await;
        assert!(fetch_online(meta.clone(), lib.clone(), row.clone(), LyricsSource::LyricsPlus).await.unwrap().is_none());
        let miss = lib.lyrics_text(t.id, "lyricsplus").unwrap().unwrap();
        assert!(miss.0.is_none());
        assert!(cached_lyrics(&lib, t.id, None, None).missing.contains(&LyricsSource::LyricsPlus));
        server.reset().await;
        Mock::given(path("/v2/lyrics/get")).respond_with(ResponseTemplate::new(502)).mount(&server).await;
        assert!(fetch_online(meta, lib.clone(), row, LyricsSource::LyricsPlus).await.is_err());
        assert_eq!(lib.lyrics_text(t.id, "lyricsplus").unwrap().unwrap(), miss);
    }
}
