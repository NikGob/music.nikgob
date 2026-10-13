//! Lyrics of the playing track from every source at once: the file (`.ttml`,
//! `.lrc`, tag), LRCLIB, NetEase (word-synced YRC where it has it) and QQ
//! Music. What is known right away is
//! shown right away; the rest arrives as the lookups finish. Without a pick by
//! the user the most precise text wins (word > line > plain), the file first
//! among equals.

use std::collections::{BTreeMap, HashSet};
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

const LOOKUP_TIMEOUT: Duration = Duration::from_secs(20);

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
    failed: bool,
    pick: Option<LyricsSource>,
}

impl TrackLyrics {
    fn chosen(&self) -> Option<LyricsSource> {
        if let Some(p) = self.pick.filter(|p| self.texts.contains_key(p)) {
            return Some(p);
        }
        let rank = |s: &LyricsSource| match s {
            LyricsSource::File => 3,
            LyricsSource::Netease => 2,
            LyricsSource::Lrclib => 1,
            LyricsSource::Qq => 0,
        };
        self.texts
            .iter()
            .max_by_key(|(s, t)| (t.sync(), rank(s)))
            .map(|(s, _)| *s)
    }

    pub(crate) fn view(&self, track: TrackId) -> v::LyricsView {
        let source = self.chosen();
        let text = source.and_then(|s| self.texts.get(&s)).cloned().unwrap_or_default();
        let sources = LyricsSource::ALL
            .into_iter()
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
        // The `.lrc` written after an LRCLIB lookup is not a source of its own.
        if let (Some(f), Some(l)) = (self.texts.get(&LyricsSource::File), self.texts.get(&LyricsSource::Lrclib))
            && f == l
        {
            self.texts.remove(&LyricsSource::File);
        }
    }
}

/// A `.ttml` (Apple Music style, word-synced) next to the file.
fn local_ttml(file: &Path) -> Option<Text> {
    let path = nikgob_import::sanitize::long_path(&file.with_extension("ttml"));
    Text::from_ttml(&std::fs::read_to_string(path).ok()?)
}

/// Asks NetEase or QQ Music and caches the answer ("not found" included) as
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
            LyricsSource::Netease => meta
                .netease
                .find(&row.artist, &row.title, album, duration)
                .await?
                .map(|l| nikgob_metadata::netease_lines(&l)),
            LyricsSource::Qq => meta
                .qq
                .find(&row.artist, &row.title, album, duration)
                .await?
                .map(|l| nikgob_metadata::qq_lines(&l)),
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

impl Worker {
    /// Shows the track's lyrics, starting lookups for the sources not known yet.
    pub(crate) fn load_lyrics(
        &mut self,
        track: TrackId,
        path: Option<PathBuf>,
        tags: Option<&nikgob_import::tags::TagSet>,
    ) {
        if let Some(tl) = self.import.as_ref().and_then(|s| s.lyrics_cache.get(&track))
            && !tl.failed
        {
            let view = tl.view(track);
            self.publish(|s, _| s.lyrics = view);
            return;
        }
        let mut tl = TrackLyrics {
            pick: self
                .lib
                .lyrics_pick(track)
                .ok()
                .flatten()
                .and_then(|k| LyricsSource::from_key(&k)),
            ..Default::default()
        };
        let fresh_miss = |at: i64| now_ms() - at < LYRICS_RETRY_MS;

        if let Some(t) = path.as_deref().and_then(local_ttml) {
            tl.insert(LyricsSource::File, Some(t));
        } else if let Some((synced, plain)) = path
            .as_deref()
            .and_then(|p| crate::enrich::local_lyrics(p, tags))
        {
            tl.insert(LyricsSource::File, Text::from_lrc(synced.as_deref(), plain.as_deref()));
        }
        let mut need = Vec::new();
        match self.lib.lyrics(track).ok().flatten() {
            Some(st) if !st.is_miss() => tl.insert(
                LyricsSource::Lrclib,
                Text::from_lrc(st.synced.as_deref(), st.plain.as_deref()),
            ),
            Some(st) if fresh_miss(st.fetched_at) => tl.insert(LyricsSource::Lrclib, None),
            _ => need.push(LyricsSource::Lrclib),
        }
        for source in [LyricsSource::Netease, LyricsSource::Qq] {
            match self.lib.lyrics_text(track, source.key()).ok().flatten() {
                Some((Some(body), _)) => tl.insert(source, stored_lines(&body)),
                Some((None, at)) if fresh_miss(at) => tl.insert(source, None),
                _ => need.push(source),
            }
        }
        // The file's `.lrc` duplicates LRCLIB until the LRCLIB answer is known.
        if tl.texts.contains_key(&LyricsSource::File) && need.contains(&LyricsSource::Lrclib) {
            need.retain(|s| *s != LyricsSource::Lrclib);
        }

        let row = self.lib.get_track_row(track).ok().flatten();
        let lib = self.lib.clone();
        let lrc_path = path.map(|p| p.with_extension("lrc"));
        if let (Some(svc), Some(row)) = (self.import.as_mut(), row) {
            if let Some(meta) = svc.metadata() {
                for source in need {
                    if !svc.lyrics_tried.insert((track, source)) {
                        continue;
                    }
                    tl.pending.insert(source);
                    let tx = svc.internal();
                    let (lib, row) = (lib.clone(), row.clone());
                    match source {
                        LyricsSource::Lrclib => {
                            let lrclib = meta.lrclib.clone();
                            let lrc_path = lrc_path.clone();
                            svc.spawn(async move {
                                let got = crate::enrich::fetch_and_store(&lrclib, &lib, &row).await.map(|st| {
                                    if let (Some(text), Some(p)) = (&st.synced, &lrc_path) {
                                        let _ = std::fs::write(nikgob_import::sanitize::long_path(p), text);
                                    }
                                    Text::from_lrc(st.synced.as_deref(), st.plain.as_deref())
                                });
                                let _ = tx.send(Internal::LyricsFetched(track, source, got.map_err(|e| e.to_string())));
                            });
                        }
                        LyricsSource::Netease | LyricsSource::Qq => {
                            let meta = meta.clone();
                            svc.spawn(async move {
                                let got = fetch_online(meta, lib, row, source).await;
                                let _ = tx.send(Internal::LyricsFetched(track, source, got.map_err(|e| e.to_string())));
                            });
                        }
                        LyricsSource::File => {}
                    }
                }
            }
            svc.lyrics_cache.insert(track, tl.clone());
        }
        let view = tl.view(track);
        self.publish(|s, _| s.lyrics = view);
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
        let view = tl.view(track);
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
        self.publish(|s, _| {
            if s.lyrics.track == Some(track) {
                s.lyrics = view;
            }
        });
    }
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
    fn best_text_wins_unless_picked() {
        let id = TrackId::new();
        let mut tl = TrackLyrics::default();
        tl.pending.insert(LyricsSource::Netease);
        tl.insert(LyricsSource::Lrclib, Text::from_lrc(Some("[00:01.00]a"), None));
        let v = tl.view(id);
        assert_eq!(v.source, Some(LyricsSource::Lrclib));
        assert!(!v.loading);
        assert_eq!(v.sources.len(), 2);

        tl.insert(LyricsSource::Netease, Some(worded()));
        assert_eq!(tl.view(id).source, Some(LyricsSource::Netease), "word sync beats line sync");

        tl.pick = Some(LyricsSource::Lrclib);
        let v = tl.view(id);
        assert_eq!(v.source, Some(LyricsSource::Lrclib));
        assert!(v.picked);

        // A pick whose text is gone falls back to the automatic choice.
        tl.pick = Some(LyricsSource::File);
        assert_eq!(tl.view(id).source, Some(LyricsSource::Netease));
    }

    #[test]
    fn written_lrc_is_not_a_second_source_and_misses_are_listed() {
        let id = TrackId::new();
        let mut tl = TrackLyrics::default();
        tl.insert(LyricsSource::File, Text::from_lrc(Some("[00:01.00]a"), None));
        tl.insert(LyricsSource::Lrclib, Text::from_lrc(Some("[00:01.00]a"), None));
        tl.insert(LyricsSource::Netease, None);
        let v = tl.view(id);
        assert_eq!(v.source, Some(LyricsSource::Lrclib));
        assert_eq!(
            v.sources,
            vec![
                v::LyricsOption {
                    source: LyricsSource::Netease,
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
}
