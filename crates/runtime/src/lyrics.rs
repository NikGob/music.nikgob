//! File, LyricsPlus, lrc.red, Better Lyrics, LRCLIB and Genius lyrics, and
//! the text heard in the recording when there is none. Synced file lyrics
//! win automatically; online texts compete by sync, then source order.
//!
//! A text the aligner checked against the track's own recording
//! (docs/lyrics-align.md, [`crate::align`]) and found to be another song's
//! or another version's loses to every text that fits. One that fits is
//! shown with the word timing made from the recording when word timing is
//! on ([`Shown`]), which ranks between a source's own word timing and line
//! timing; else line texts keep their lines (moved when they are one shift
//! off, made when they are another version's) and a plain text gets the
//! made lines. Timing the user asked for is always shown.
//! A LyricsPlus text whose generated word timing was dropped (QQ Music, see
//! [`nikgob_metadata::lyricsplus`]) is a weak line sync: any other line
//! sync beats it. LyricsPlus is asked with the track's full artist credit
//! («WENARO, LXNER», which finds Apple's lines) and with the main artist
//! only when that finds nothing.

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
    /// Its line times are less trustworthy than another source's of the
    /// same sync (a QQ Music text without its generated words).
    pub(crate) weak: bool,
}

/// How a weak or a plain text is cached (a plain list of lines is a normal
/// one).
#[derive(serde::Serialize, serde::Deserialize)]
struct Stored {
    lines: Vec<LyricLine>,
    #[serde(default)]
    weak: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plain: Option<String>,
}

/// What the aligner made of one text (`lyrics_align`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Aligned {
    /// The text it was made from ([`Text::hash`]).
    pub(crate) hash: String,
    /// The recording spells it: this song's text.
    pub(crate) fits: bool,
    pub(crate) timing: nikgob_align::fit::Timing,
    /// Source line time − voice.
    pub(crate) offset: Duration,
    pub(crate) offset_early: bool,
    pub(crate) covered: f32,
    /// The text with word (or, saved from the sheet, line) timing made
    /// from the recording.
    pub(crate) lines: Vec<LyricLine>,
    pub(crate) saved: bool,
    /// Made by this aligner version ([`crate::align::MADE_BY`]); an older
    /// row is shown until the text is checked again.
    pub(crate) current: bool,
}

impl Aligned {
    pub(crate) fn from_row(row: &nikgob_library::AlignedLyrics) -> Option<(LyricsSource, Self)> {
        use nikgob_align::fit::Timing;
        let source = LyricsSource::from_key(&row.source)?;
        let lines: Vec<LyricLine> = row.body.as_deref().and_then(|b| serde_json::from_str(b).ok()).unwrap_or_default();
        let timing = match row.timing.as_str() {
            "fits" => Timing::Fits,
            "shifted" => Timing::Shifted,
            "elsewhere" => Timing::Elsewhere,
            _ => Timing::None,
        };
        Some((
            source,
            Self {
                hash: row.text_hash.clone(),
                fits: row.body.is_some(),
                timing,
                offset: Duration::from_millis(row.offset_ms.unsigned_abs()),
                offset_early: row.offset_ms < 0,
                covered: row.covered,
                lines,
                saved: row.saved,
                current: row.made_by == crate::align::MADE_BY,
            },
        ))
    }
}

/// A stable short hash of a text (FNV-1a), to tell when a source's text
/// changed since it was checked.
pub(crate) fn text_hash(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

impl Text {
    /// From LRC text and/or plain text; `None` when both are empty.
    pub(crate) fn from_lrc(synced: Option<&str>, plain: Option<&str>) -> Option<Self> {
        let lines = synced.map(nikgob_metadata::parse_lrc_lines).unwrap_or_default();
        let plain = plain
            .filter(|p| lines.is_empty() && !p.trim().is_empty())
            .map(str::to_owned);
        (!lines.is_empty() || plain.is_some()).then_some(Self { lines, plain, weak: false })
    }

    pub(crate) fn from_ttml(xml: &str) -> Option<Self> {
        let lines = nikgob_metadata::parse_ttml(xml)
            .map_err(|e| log::debug!("bad TTML: {e}"))
            .ok()?
            .lines;
        (!lines.is_empty()).then_some(Self { lines, plain: None, weak: false })
    }

    pub(crate) fn from_lines(lines: Vec<LyricLine>) -> Option<Self> {
        (!lines.is_empty()).then_some(Self { lines, plain: None, weak: false })
    }

    /// A plain text; the zero-width spaces of texts cached before the
    /// Genius parser dropped them are left out.
    pub(crate) fn from_plain(text: &str) -> Option<Self> {
        let text = text.replace(['\u{200b}', '\u{feff}'], "");
        (!text.trim().is_empty()).then(|| Self { lines: Vec::new(), plain: Some(text.trim().to_owned()), weak: false })
    }

    /// The cached body of an online text (see [`stored_lines`]).
    pub(crate) fn body(&self) -> Option<String> {
        if self.weak || (self.lines.is_empty() && self.plain.is_some()) {
            serde_json::to_string(&Stored { lines: self.lines.clone(), weak: self.weak, plain: self.plain.clone() }).ok()
        } else {
            serde_json::to_string(&self.lines).ok()
        }
    }

    /// [`text_hash`] of the text.
    pub(crate) fn hash(&self) -> String {
        text_hash(&self.serialized())
    }

    /// The lines to align: the synced ones, or a plain text's lines (with
    /// no times). The second value says whether the line times are real.
    pub(crate) fn align_lines(&self) -> (Vec<LyricLine>, bool) {
        if self.lines.is_empty() {
            (nikgob_align::fit::plain_lines(self.plain.as_deref().unwrap_or_default()), false)
        } else {
            (self.lines.clone(), true)
        }
    }

    fn serialized(&self) -> String {
        self.plain.clone().unwrap_or_else(|| nikgob_metadata::lrc::serialize_lrc(&self.lines)).trim().to_owned()
    }

    fn sync(&self) -> LyricsSync {
        LyricsSync::of(&self.lines).unwrap_or(LyricsSync::Plain)
    }
}

/// What made timing is shown (from the settings).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Shown {
    /// Word timing made from the recording for line and plain texts.
    pub(crate) words: bool,
    /// Line timing made from the recording for a plain text.
    pub(crate) lines: bool,
    /// A text heard in the recording earlier is shown (recognition is on);
    /// off, it is neither listed nor chosen, picked or not.
    pub(crate) heard: bool,
}

impl Default for Shown {
    fn default() -> Self {
        Self { words: true, lines: true, heard: true }
    }
}

impl Shown {
    pub(crate) fn of(p: &v::Prefs) -> Self {
        Self { words: p.lyrics_words_playing || p.lyrics_words_all, lines: p.lyrics_lines_auto, heard: p.lyrics_recognize }
    }
}

/// Where the audio to make timing from is (set by the worker).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Place {
    /// Nothing to listen to, or not allowed (a stream with the setting off).
    #[default]
    None,
    /// The track's own file.
    File,
    /// A stream whose artist and song are clear: fetched for a while.
    Stream,
    /// A stream whose song is unclear (a clip, an unofficial upload): why.
    Unclear(String),
}

impl Place {
    fn has_audio(&self) -> bool {
        matches!(self, Self::File | Self::Stream)
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
    /// What the aligner made of the texts (by source).
    pub(crate) aligned: BTreeMap<LyricsSource, Aligned>,
    /// The texts went to the aligner (and their hashes then).
    pub(crate) checking: Option<Vec<String>>,
    /// Genius may be asked (the setting).
    pub(crate) genius: bool,
    /// Which made timing is shown.
    pub(crate) show: Shown,
    /// Where the audio to make timing from is.
    pub(crate) place: Place,
}

impl TrackLyrics {
    /// What the aligner made of `source`'s current text.
    pub(crate) fn aligned(&self, source: LyricsSource) -> Option<&Aligned> {
        let text = self.texts.get(&source)?;
        self.aligned.get(&source).filter(|a| a.hash == text.hash())
    }

    /// The text is another song's or another version's (the aligner said).
    fn misfit(&self, source: LyricsSource) -> bool {
        self.aligned(source).is_some_and(|a| !a.fits)
    }

    /// The lines shown for `source`, and whether their timing was made by
    /// the player: what the user asked for; a source's own word timing
    /// (moved when it is one constant shift off); with word timing on,
    /// the made words; else a line text's own lines (moved, or the made
    /// lines of another version) and a plain text's made lines.
    pub(crate) fn shown(&self, source: LyricsSource) -> (Vec<LyricLine>, bool) {
        use nikgob_align::fit::Timing;
        let Some(text) = self.texts.get(&source) else { return (Vec::new(), false) };
        if source == LyricsSource::Recognized {
            return (text.lines.clone(), true);
        }
        let Some(a) = self.aligned(source).filter(|a| a.fits && !a.lines.is_empty()) else {
            return (text.lines.clone(), false);
        };
        if a.saved {
            return (a.lines.clone(), true);
        }
        let shifted = || shift_lines(&text.lines, a.offset, a.offset_early);
        match (text.sync(), a.timing) {
            (LyricsSync::Syllable | LyricsSync::Word, Timing::Fits | Timing::None) => (text.lines.clone(), false),
            (LyricsSync::Syllable | LyricsSync::Word, Timing::Shifted) => (shifted(), false),
            // Another version's words: the made ones, as fine as the source's.
            (LyricsSync::Syllable | LyricsSync::Word, _) => (a.lines.clone(), true),
            _ if self.show.words => (a.lines.clone(), true),
            (LyricsSync::Line, Timing::Fits | Timing::None) => (text.lines.clone(), false),
            (LyricsSync::Line, Timing::Shifted) => (shifted(), false),
            (LyricsSync::Plain, _) if !self.show.lines => (text.lines.clone(), false),
            _ => (crate::align::to_lines(&a.lines), true),
        }
    }

    /// Some source has line timing (or finer) that is not another song's.
    pub(crate) fn has_timed(&self) -> bool {
        self.texts.iter().any(|(s, t)| *s != LyricsSource::Recognized && t.sync() > LyricsSync::Plain && !self.misfit(*s))
    }

    /// How good a source's text is here, higher first: fitting, then its
    /// precision (own syllables, own words, made words, lines, a weak line
    /// sync, plain, heard from scratch), how much of the song it covers,
    /// then the source order.
    fn rank(&self, source: LyricsSource) -> Option<(bool, u8, u8, u8)> {
        let text = self.texts.get(&source)?;
        let (lines, made) = self.shown(source);
        let sync = LyricsSync::of(&lines).unwrap_or(LyricsSync::Plain);
        let tier = match (source, sync, made) {
            (LyricsSource::Recognized, ..) => 0,
            (_, LyricsSync::Syllable, false) => 7,
            (_, LyricsSync::Word, false) => 6,
            (_, LyricsSync::Syllable | LyricsSync::Word, true) => 5,
            (_, LyricsSync::Line, _) if text.weak => 3,
            (_, LyricsSync::Line, _) => 4,
            (_, LyricsSync::Plain, _) => 2,
        };
        let covered = self.aligned(source).filter(|a| a.fits).map_or(10, |a| (a.covered * 10.0).round() as u8);
        let order = match source {
            LyricsSource::Recognized => 0,
            LyricsSource::Genius => 1,
            LyricsSource::File => 2,
            LyricsSource::Lrclib => 3,
            LyricsSource::BetterLyrics => 4,
            LyricsSource::LrcRed => 5,
            LyricsSource::LyricsPlus => 6,
        };
        Some((!self.misfit(source), tier, covered, order))
    }

    /// `source` may be shown: a heard text only while recognition is on.
    fn usable(&self, source: LyricsSource) -> bool {
        source != LyricsSource::Recognized || self.show.heard
    }

    fn chosen(&self) -> Option<LyricsSource> {
        if let Some(p) = self.pick.filter(|p| self.texts.contains_key(p) && self.usable(*p)) {
            return Some(p);
        }
        if self.texts.get(&LyricsSource::File).is_some_and(|t| t.sync() > LyricsSync::Plain)
            && !self.duplicate_file()
            && !self.misfit(LyricsSource::File)
        {
            return Some(LyricsSource::File);
        }
        LyricsSource::ALL.into_iter()
            .filter(|s| *s != LyricsSource::File || !self.duplicate_file())
            .filter(|s| self.usable(*s))
            .filter_map(|s| self.rank(s).map(|r| (s, r)))
            .max_by_key(|(_, r)| *r)
            .map(|(s, _)| s)
    }

    /// The texts the aligner has not checked in their current form, as it
    /// takes them.
    pub(crate) fn unchecked(&self) -> Vec<crate::align::AlignText> {
        self.texts
            .iter()
            .filter(|(s, _)| **s != LyricsSource::Recognized)
            .filter(|(s, _)| **s != LyricsSource::File || !self.duplicate_file())
            // Made by an older aligner: checked again (timing the user
            // kept stays until they make it again).
            .filter(|(s, _)| self.aligned(**s).is_none_or(|a| !a.current && !a.saved))
            .map(|(s, t)| {
                let (lines, timed) = t.align_lines();
                crate::align::AlignText { source: *s, lines, timed, hash: t.hash() }
            })
            .filter(|t| !t.lines.is_empty())
            .collect()
    }

    /// No text anywhere (every lookup settled): the recording is the only
    /// place left to hear one in.
    pub(crate) fn nothing_found(&self) -> bool {
        self.pending.is_empty() && self.texts.keys().all(|s| *s == LyricsSource::Recognized)
            && !self.texts.contains_key(&LyricsSource::Recognized)
    }

    /// Lookups are done: the texts can go to the aligner.
    pub(crate) fn settled(&self) -> bool {
        self.pending.is_empty()
    }

    /// The source the user picked (`None`: automatic).
    pub(crate) fn set_pick(&mut self, pick: Option<LyricsSource>) {
        self.pick = pick;
    }

    /// Takes the aligner's verdict on a text.
    pub(crate) fn set_aligned(&mut self, source: LyricsSource, a: Aligned) {
        self.aligned.insert(source, a);
    }

    /// Takes a text (from a source or heard in the recording).
    pub(crate) fn set_text(&mut self, source: LyricsSource, text: Option<Text>) {
        self.insert(source, text);
    }

    pub(crate) fn texts_of(&self, source: LyricsSource) -> Option<&Text> {
        self.texts.get(&source)
    }

    /// No text at all, heard or found.
    pub(crate) fn texts_empty(&self) -> bool {
        self.texts.is_empty()
    }

    /// Genius is worth asking now: no text with timing fits the recording.
    fn genius_wanted(&self) -> bool {
        self.genius
            && !self.texts.contains_key(&LyricsSource::Genius)
            && !self.missing.contains(&LyricsSource::Genius)
            && !self.has_timed()
    }

    fn duplicate_file(&self) -> bool {
        self.texts.get(&LyricsSource::File).is_some_and(|file| {
            self.texts.iter().any(|(s, online)| *s != LyricsSource::File && (
                file == online || file.serialized() == online.serialized()
            ))
        })
    }

    /// Only confirmed misses enable Better Lyrics; failures never count as
    /// misses. Genius (plain texts) comes last, and only when no text with
    /// timing that fits the recording is there. When nothing is left to
    /// ask, nothing stays pending (a source is never shown as searching
    /// without a lookup on its way).
    fn next_lookups(&mut self) -> Vec<LyricsSource> {
        let online = self.next_online();
        if !online.is_empty() {
            return online;
        }
        if self.pending.contains(&LyricsSource::Genius) && self.genius && !self.has_timed() {
            return vec![LyricsSource::Genius];
        }
        self.pending.clear();
        Vec::new()
    }

    fn next_online(&mut self) -> Vec<LyricsSource> {
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
        let (lines, made) = source.map(|s| self.shown(s)).unwrap_or_default();
        let sources = LyricsSource::ALL
            .into_iter()
            .filter(|s| *s != LyricsSource::File || self.pick == Some(LyricsSource::File) || !self.duplicate_file())
            .filter(|s| self.usable(*s))
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
                let fits = self.aligned(s).map(|a| a.fits);
                let made = self.texts.contains_key(&s) && self.shown(s).1;
                Some(v::LyricsOption { source: s, state, fits, made })
            })
            .collect();
        let texts = self
            .texts
            .iter()
            .filter(|(s, _)| **s != LyricsSource::File || self.pick == Some(LyricsSource::File) || !self.duplicate_file())
            .filter(|(s, _)| self.usable(**s))
            .map(|(s, t)| {
                let a = self.aligned(*s);
                let (made_lines, made) = self.shown(*s);
                v::LyricsText {
                    source: Some(*s),
                    lines: t.lines.clone(),
                    plain: t.plain.clone(),
                    sync: Some(t.sync()),
                    fits: if *s == LyricsSource::Recognized { Some(true) } else { a.map(|a| a.fits) },
                    made: made.then_some(made_lines),
                    saved: a.is_some_and(|a| a.saved),
                }
            })
            .collect();
        let home = match source {
            Some(LyricsSource::Recognized) => v::LyricsHome::Library,
            Some(_) if made => v::LyricsHome::Library,
            Some(LyricsSource::File) => v::LyricsHome::Computer,
            _ => v::LyricsHome::Online,
        };
        let craft = match (source, &self.place) {
            (None | Some(LyricsSource::Recognized), _) | (_, Place::None) => v::LyricsCraft::Off,
            (_, Place::Unclear(why)) => v::LyricsCraft::No(why.clone()),
            _ => v::LyricsCraft::Ready,
        };
        // The player's own timing of a found text can go to the computer
        // (a heard text is its guess: never).
        let savable = made && source.is_some_and(|s| s != LyricsSource::Recognized) && self.place.has_audio();
        v::LyricsView {
            track: Some(track),
            lines,
            plain: text.plain,
            loading: source.is_none() && !self.pending.is_empty(),
            source,
            picked: self.pick.is_some() && self.pick == source,
            sources,
            made,
            checking: self.checking.is_some(),
            texts,
            home,
            craft,
            savable,
        }
    }

    /// The lines `source` shows, with the player's timing (to keep them).
    pub(crate) fn shown_lines(&self, source: LyricsSource) -> Option<Vec<LyricLine>> {
        let (lines, made) = self.shown(source);
        (made && !lines.is_empty()).then_some(lines)
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

/// `lines` moved by `by` (earlier when `early`), words too.
fn shift_lines(lines: &[LyricLine], by: Duration, early: bool) -> Vec<LyricLine> {
    let mv = |d: Duration| if early { d + by } else { d.saturating_sub(by) };
    lines
        .iter()
        .map(|l| LyricLine {
            start: mv(l.start),
            end: l.end.map(mv),
            words: l.words.iter().map(|w| nikgob_core::lyrics::LyricWord { start: mv(w.start), end: mv(w.end), text: w.text.clone() }).collect(),
            ..l.clone()
        })
        .collect()
}

/// A `.ttml` (Apple Music style, word-synced) next to the file.
fn local_ttml(file: &Path) -> Option<Text> {
    read_ttml(&file.with_extension("ttml"))
}

fn read_ttml(path: &Path) -> Option<Text> {
    Text::from_ttml(&std::fs::read_to_string(nikgob_import::sanitize::long_path(path)).ok()?)
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
            return Ok(Text::from_lines(t.lines).map(|x| Text { weak: t.words_dropped, ..x }));
        }
    }
    Ok(None)
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
            LyricsSource::Genius => meta.genius.find(&row.artist, &row.title, row.album.as_deref()).await?.and_then(|t| Text::from_plain(&t)),
            LyricsSource::File | LyricsSource::Lrclib | LyricsSource::Recognized => None,
        })
    };
    let text = tokio::time::timeout(LOOKUP_TIMEOUT, lookup)
        .await
        .unwrap_or_else(|_| Err(MetadataError::Network("timeout".into())))?;
    let body = text.as_ref().and_then(Text::body);
    let _ = lib.put_lyrics_text(row.id, source.key(), body.as_deref());
    Ok(text)
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
    let mut tl = TrackLyrics {
        pick: lib
            .lyrics_pick(track)
            .ok()
            .flatten()
            .and_then(|k| LyricsSource::from_key(&k)),
        aligned: lib.lyrics_aligned(track).unwrap_or_default().iter().filter_map(Aligned::from_row).collect(),
        genius,
        ..Default::default()
    };
    let fresh_miss = |at: i64| now_ms() - at < LYRICS_RETRY_MS;

    if let Some(t) = path.and_then(local_ttml) {
        tl.insert(LyricsSource::File, Some(t));
    } else if let Some(t) = kept.filter(|_| path.is_none()).and_then(read_ttml) {
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
            Some((None, at)) if fresh_miss(at) => tl.insert(source, None),
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

/// Background backfill uses the same cache, fallback gates and ranking as playback.
pub(crate) async fn fetch_track_lyrics(meta: Metadata, lib: Library, row: TrackRow, path: Option<PathBuf>, genius: bool, kept: Option<PathBuf>) -> TrackLyrics {
    let tags = path.as_deref().and_then(|p| nikgob_import::tags::read_tags(p).ok()).map(|(t, _)| t);
    let mut tl = cached_lyrics(&lib, row.id, path.as_deref(), tags.as_ref(), genius, kept.as_deref());
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
        let show = Shown::of(&self.state.read().prefs);
        let place = self.lyrics_place(track);
        let Some(svc) = self.import.as_mut() else { return };
        svc.lyrics_cache.insert(track, TrackLyrics { show, place, ..tl });
        svc.lyrics_order.retain(|t| *t != track);
        svc.lyrics_order.push_back(track);
        let (cache, later) = (&svc.lyrics_cache, &svc.embed_later);
        let spare = |t: TrackId| {
            keep.contains(&Some(t)) || later.contains(&t) || cache.get(&t).is_some_and(TrackLyrics::busy)
        };
        for t in evict(&mut svc.lyrics_order, LYRICS_KEPT, spare) {
            svc.lyrics_cache.remove(&t);
            // Rebuilt from the library next time; what it lacks is asked again.
            svc.lyrics_asking.retain(|(asked, _)| *asked != t);
        }
    }

    pub(crate) fn lyrics_backfilled(&mut self, track: TrackId, mut tl: TrackLyrics) {
        tl.pick = self.lib.lyrics_pick(track).ok().flatten().and_then(|k| LyricsSource::from_key(&k));
        tl.show = Shown::of(&self.state.read().prefs);
        if tl.texts_of(LyricsSource::File).is_none()
            && let Some(kept) = self.kept_text(track).as_deref().and_then(read_ttml)
        {
            tl.insert(LyricsSource::File, Some(kept));
        }
        let Some(svc) = self.import.as_mut() else { return };
        // Playback lookups keep ownership of their in-flight results.
        if svc.lyrics_cache.get(&track).is_some_and(|t| !t.pending.is_empty() || t.embedding) { return }
        let view = tl.view(track);
        self.cache_lyrics(track, tl);
        self.publish(|s| {
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
            self.publish(|s| s.lyrics = view);
            self.embed_lyrics(track);
            self.check_if_playing(track);
            return;
        }
        let genius = self.state.read().prefs.lyrics_genius;
        let kept = if path.is_none() { self.kept_text(track) } else { None };
        let mut tl = cached_lyrics(&self.lib, track, path.as_deref(), tags, genius, kept.as_deref());
        tl.show = Shown::of(&self.state.read().prefs);
        tl.place = self.lyrics_place(track);
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
        self.publish(|s| s.lyrics = view);
        self.embed_lyrics(track);
        self.check_if_playing(track);
    }

    /// After a check: when no text with timing fits, Genius is asked too.
    pub(crate) fn start_lyrics_lookups_again(&mut self, track: TrackId) {
        let Some(tl) = self.import.as_mut().and_then(|s| s.lyrics_cache.get_mut(&track)) else { return };
        if tl.genius_wanted() && tl.pending.insert(LyricsSource::Genius) {
            self.start_lyrics_lookups(track);
        }
    }

    /// The playing track's texts go to the aligner once they settle.
    fn check_if_playing(&mut self, track: TrackId) {
        if self.queue.current().map(|e| e.track) == Some(track) {
            self.check_lyrics(track);
        }
    }

    fn start_lyrics_lookups(&mut self, track: TrackId) {
        let Some(row) = self.lib.get_track_row(track).ok().flatten() else { return };
        let Some(svc) = self.import.as_mut() else { return };
        let Some(meta) = svc.metadata() else { return };
        let Some(tl) = svc.lyrics_cache.get_mut(&track) else { return };
        let sources = tl.next_lookups();
        for source in sources {
            // Already on its way: its answer asks for the next ones.
            if !svc.lyrics_asking.insert((track, source)) {
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
        // A text heard in the recording is the player's guess: never into
        // the file.
        let Some(source) = tl.chosen().filter(|s| !matches!(s, LyricsSource::File | LyricsSource::Recognized)) else { return };
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
            self.publish(|s| {
                if s.lyrics.track == Some(track) { s.lyrics = view; }
            });
        }
        if saved { self.embed_lyrics(track); }
    }

    pub(crate) fn lyrics_fetched(&mut self, track: TrackId, source: LyricsSource, got: Result<Option<Text>, String>) {
        let Some(svc) = &mut self.import else { return };
        svc.lyrics_asking.remove(&(track, source));
        let Some(tl) = svc.lyrics_cache.get_mut(&track) else { return };
        match got {
            Ok(text) => tl.insert(source, text),
            Err(e) => {
                log::debug!("lyrics lookup ({}): {e}", source.key());
                tl.pending.remove(&source);
                tl.failed = true;
            }
        }
        self.start_lyrics_lookups(track);
        let view = self.import.as_ref().unwrap().lyrics_cache[&track].view(track);
        self.embed_lyrics(track);
        self.publish(|s| {
            if s.lyrics.track == Some(track) {
                s.lyrics = view;
            }
        });
        self.check_if_playing(track);
    }

    /// The lyrics settings changed: what is shown follows, and the playing
    /// song is checked for what is newly wanted.
    pub(crate) fn lyrics_prefs_changed(&mut self) {
        let show = Shown::of(&self.state.read().prefs);
        let shown = self.state.read().lyrics.track;
        let tracks: Vec<TrackId> = self.import.as_ref().map(|s| s.lyrics_cache.keys().copied().collect()).unwrap_or_default();
        let places: Vec<(TrackId, Place)> = tracks.into_iter().map(|t| (t, self.lyrics_place(t))).collect();
        let Some(svc) = self.import.as_mut() else { return };
        for tl in svc.lyrics_cache.values_mut() {
            tl.show = show;
        }
        for (t, place) in places {
            if let Some(tl) = svc.lyrics_cache.get_mut(&t) {
                tl.place = place;
            }
        }
        if let Some(track) = shown
            && let Some(view) = svc.lyrics_cache.get(&track).map(|tl| tl.view(track))
        {
            self.publish(|s| {
                if s.lyrics.track == Some(track) {
                    s.lyrics = view;
                }
            });
        }
        if let Some(track) = self.queue.current().map(|e| e.track) {
            self.check_lyrics(track);
        }
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
        self.publish(|s| {
            if s.lyrics.track == Some(track) {
                s.lyrics = view;
            }
        });
    }
}

pub(crate) fn embed_text(path: &Path, text: &str) -> nikgob_import::Result<()> {
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
        Text { lines: vec![l], plain: None, weak: false }
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
                    state: v::LyricsAvail::Missing,
                    fits: None,
                    made: false,
                },
                v::LyricsOption {
                    source: LyricsSource::Lrclib,
                    state: v::LyricsAvail::Ready(LyricsSync::Line),
                    fits: None,
                    made: false,
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

    #[test]
    fn a_weak_line_sync_loses_to_any_other_line_sync_but_not_to_plain_text() {
        let lrc = |t: &str| Text::from_lrc(Some(t), None);
        let mut weak = lrc("[00:01.00]a").unwrap();
        weak.weak = true;
        let mut tl = TrackLyrics::default();
        tl.insert(LyricsSource::LyricsPlus, Some(weak.clone()));
        tl.insert(LyricsSource::Lrclib, Text::from_lrc(None, Some("a")));
        assert_eq!(tl.chosen(), Some(LyricsSource::LyricsPlus), "lines beat plain text");
        for other in [LyricsSource::Lrclib, LyricsSource::BetterLyrics, LyricsSource::LrcRed] {
            let mut tl = tl.clone();
            tl.insert(other, lrc("[00:01.50]a"));
            assert_eq!(tl.chosen(), Some(other), "an accurate line sync beats the weak one");
            // Real word sync of LyricsPlus wins again.
            tl.insert(LyricsSource::LyricsPlus, Some(worded()));
            assert_eq!(tl.chosen(), Some(LyricsSource::LyricsPlus));
        }
        // Cached and read back as weak; a normal text stays a plain list.
        let back = stored_lines(&weak.body().unwrap()).unwrap();
        assert!(back.weak);
        assert_eq!(back.lines, weak.lines);
        let normal = worded();
        assert!(normal.body().unwrap().starts_with('['));
        assert!(!stored_lines(&normal.body().unwrap()).unwrap().weak);
    }

    fn made(lines: &[(u64, &str)]) -> Vec<LyricLine> {
        lines
            .iter()
            .map(|(ms, t)| {
                let mut l = line(*ms, t);
                l.words.push(nikgob_core::lyrics::LyricWord {
                    start: Duration::from_millis(*ms),
                    end: Duration::from_millis(ms + 400),
                    text: (*t).to_owned(),
                });
                l.end = Some(Duration::from_millis(ms + 400));
                l
            })
            .collect()
    }

    fn aligned(text: &Text, fits: bool, timing: nikgob_align::fit::Timing, offset_ms: i64, lines: Vec<LyricLine>) -> Aligned {
        Aligned {
            hash: text.hash(),
            fits,
            timing,
            offset: Duration::from_millis(offset_ms.unsigned_abs()),
            offset_early: offset_ms < 0,
            covered: 0.9,
            lines,
            saved: false,
            current: true,
        }
    }

    #[test]
    fn a_line_text_of_another_take_is_shown_with_the_timing_of_this_recording() {
        use nikgob_align::fit::Timing;
        // «космоцветок»: LRCLIB's lines are 2.14 s early for this recording.
        let mut tl = TrackLyrics::default();
        let lrc = Text::from_lrc(Some("[00:14.60]Слово за слово\n[00:18.32]Дальше"), None).unwrap();
        tl.insert(LyricsSource::Lrclib, Some(lrc.clone()));
        let v0 = tl.view(TrackId::new());
        assert!(!v0.made);
        assert_eq!(v0.lines[0].start, Duration::from_millis(14_600));
        let retimed = made(&[(16_740, "Слово за слово"), (20_460, "Дальше")]);
        tl.set_aligned(LyricsSource::Lrclib, aligned(&lrc, true, Timing::Shifted, -2140, retimed.clone()));
        let v = tl.view(TrackId::new());
        assert_eq!(v.source, Some(LyricsSource::Lrclib));
        assert!(v.made);
        assert_eq!(v.lines, retimed);
        assert_eq!(v.sources[0].fits, Some(true));
        assert!(v.sources[0].made);
        let t = v.texts.iter().find(|t| t.source == Some(LyricsSource::Lrclib)).unwrap();
        assert_eq!(t.lines, lrc.lines, "the sheet shows the source's own text too");
        assert_eq!(t.made.as_ref(), Some(&retimed));
        // A changed text is not covered by the old check.
        tl.insert(LyricsSource::Lrclib, Text::from_lrc(Some("[00:14.60]Другое"), None));
        assert!(!tl.view(TrackId::new()).made);
        assert_eq!(tl.unchecked().len(), 1);
    }

    #[test]
    fn a_text_that_does_not_fit_loses_to_one_that_does() {
        use nikgob_align::fit::Timing;
        let mut tl = TrackLyrics::default();
        let wrong = Text::from_lrc(Some("[00:01.00]чужие слова"), None).unwrap();
        let genius = Text::from_plain("свои слова").unwrap();
        tl.insert(LyricsSource::Lrclib, Some(wrong.clone()));
        tl.insert(LyricsSource::Genius, Some(genius.clone()));
        assert_eq!(tl.chosen(), Some(LyricsSource::Lrclib), "unchecked: lines beat plain");
        tl.set_aligned(LyricsSource::Lrclib, aligned(&wrong, false, Timing::Elsewhere, 0, Vec::new()));
        assert_eq!(tl.chosen(), Some(LyricsSource::Genius), "plain text that fits beats lines that do not");
        tl.set_aligned(LyricsSource::Genius, aligned(&genius, true, Timing::None, 0, made(&[(3000, "свои слова")])));
        let v = tl.view(TrackId::new());
        assert_eq!(v.source, Some(LyricsSource::Genius));
        assert!(v.made);
        assert_eq!(LyricsSync::of(&v.lines), Some(LyricsSync::Word));
        assert_eq!(v.sources.iter().find(|o| o.source == LyricsSource::Lrclib).unwrap().fits, Some(false));
        // A text heard in the recording loses to any text that fits, but
        // not to one that does not.
        let mut heard = TrackLyrics::default();
        heard.insert(LyricsSource::Recognized, Some(Text::from_lines(made(&[(500, "услышано")])).unwrap()));
        heard.insert(LyricsSource::Lrclib, Some(wrong.clone()));
        assert_eq!(heard.chosen(), Some(LyricsSource::Lrclib));
        heard.set_aligned(LyricsSource::Lrclib, aligned(&wrong, false, Timing::Elsewhere, 0, Vec::new()));
        assert_eq!(heard.chosen(), Some(LyricsSource::Recognized));
        assert!(heard.view(TrackId::new()).made);
    }

    /// Word timing off (the default): a line text keeps its lines (moved
    /// when one shift off, made from the recording when another version's);
    /// a plain text gets made lines, or none with that off too; timing the
    /// user asked for shows whatever the settings.
    #[test]
    fn without_word_timing_lines_stay_lines_and_a_plain_text_gets_made_lines() {
        use nikgob_align::fit::Timing;
        let off = Shown { words: false, lines: true, heard: true };
        let lrc = Text::from_lrc(Some("[00:14.60]Слово за слово\n[00:18.32]Дальше"), None).unwrap();
        let words = made(&[(16_740, "Слово за слово"), (20_460, "Дальше")]);
        for (timing, first, made_flag) in [(Timing::Fits, 14_600, false), (Timing::Shifted, 16_740, false), (Timing::Elsewhere, 16_740, true)] {
            let mut tl = TrackLyrics { show: off, ..Default::default() };
            tl.insert(LyricsSource::Lrclib, Some(lrc.clone()));
            tl.set_aligned(LyricsSource::Lrclib, aligned(&lrc, true, timing, -2140, words.clone()));
            let v = tl.view(TrackId::new());
            assert_eq!(LyricsSync::of(&v.lines), Some(LyricsSync::Line), "{timing:?}");
            assert_eq!(v.lines[0].start, Duration::from_millis(first), "{timing:?}");
            assert_eq!(v.made, made_flag, "{timing:?}");
        }
        let genius = Text::from_plain("Слово за слово\nДальше").unwrap();
        let mut tl = TrackLyrics { show: off, ..Default::default() };
        tl.insert(LyricsSource::Genius, Some(genius.clone()));
        tl.set_aligned(LyricsSource::Genius, aligned(&genius, true, Timing::None, 0, words.clone()));
        let v = tl.view(TrackId::new());
        assert!(v.made);
        assert_eq!(LyricsSync::of(&v.lines), Some(LyricsSync::Line));
        assert_eq!(v.lines[1].start, Duration::from_millis(20_460));
        tl.show.lines = false;
        let v = tl.view(TrackId::new());
        assert!(v.lines.is_empty() && v.plain.is_some() && !v.made);
        // Asked for by the user: shown as made, whatever the settings.
        tl.set_aligned(LyricsSource::Genius, Aligned { saved: true, ..aligned(&genius, true, Timing::None, 0, words.clone()) });
        let v = tl.view(TrackId::new());
        assert!(v.made);
        assert_eq!(v.lines, words);
        // With word timing on, the made words.
        let mut tl = TrackLyrics { show: Shown { words: true, lines: false, heard: true }, ..Default::default() };
        tl.insert(LyricsSource::Genius, Some(genius.clone()));
        tl.set_aligned(LyricsSource::Genius, aligned(&genius, true, Timing::None, 0, words.clone()));
        assert_eq!(tl.view(TrackId::new()).lines, words);
    }

    /// With recognition off, a text heard earlier is never shown: not
    /// chosen, not listed, not even when it was picked.
    #[test]
    fn a_heard_text_is_hidden_while_recognition_is_off() {
        let heard = Text::from_lines(made(&[(500, "услышано")])).unwrap();
        let mut tl = TrackLyrics { show: Shown { heard: false, ..Shown::default() }, ..Default::default() };
        tl.insert(LyricsSource::Recognized, Some(heard));
        tl.pick = Some(LyricsSource::Recognized);
        let v = tl.view(TrackId::new());
        assert_eq!(v.source, None);
        assert!(v.lines.is_empty() && v.sources.is_empty() && v.texts.is_empty());
        // A found text still shows.
        tl.insert(LyricsSource::Genius, Text::from_plain("свои слова"));
        assert_eq!(tl.view(TrackId::new()).source, Some(LyricsSource::Genius));
        // Recognition on again: the heard text is back.
        tl.show.heard = true;
        assert_eq!(tl.view(TrackId::new()).source, Some(LyricsSource::Recognized));
    }

    /// Genius is asked once: a text it gave is kept in the library and read
    /// back from there, never asked for again.
    #[test]
    fn a_stored_genius_text_is_not_asked_for_again() {
        use nikgob_core::model::{ArtistCredit, Track};
        let lib = Library::open_in_memory().unwrap();
        let track = lib.upsert_track(&Track::new("Замок", ArtistCredit::single("nyan.mp3", None))).unwrap().id;
        let fresh = cached_lyrics(&lib, track, None, None, true, None);
        assert!(fresh.pending.contains(&LyricsSource::Genius));
        let body = Text::from_plain("Я невменяемый (Ай)").unwrap().body();
        lib.put_lyrics_text(track, LyricsSource::Genius.key(), body.as_deref()).unwrap();
        let tl = cached_lyrics(&lib, track, None, None, true, None);
        assert!(!tl.pending.contains(&LyricsSource::Genius));
        assert_eq!(tl.texts_of(LyricsSource::Genius).and_then(|t| t.plain.as_deref()), Some("Я невменяемый (Ай)"));
    }

    #[test]
    fn own_word_timing_beats_made_timing_and_moves_when_shifted() {
        use nikgob_align::fit::Timing;
        let mut tl = TrackLyrics::default();
        let words = worded();
        tl.insert(LyricsSource::LyricsPlus, Some(words.clone()));
        let lines = Text::from_lrc(Some("[00:01.00]a"), None).unwrap();
        tl.insert(LyricsSource::Lrclib, Some(lines.clone()));
        tl.set_aligned(LyricsSource::Lrclib, aligned(&lines, true, Timing::Fits, 0, made(&[(1000, "a")])));
        assert_eq!(tl.chosen(), Some(LyricsSource::LyricsPlus), "a source's own words beat made ones");
        // One steady shift: the source's own words, moved (here 300 ms late).
        tl.set_aligned(LyricsSource::LyricsPlus, aligned(&words, true, Timing::Shifted, 300, made(&[(700, "a")])));
        let v = tl.view(TrackId::new());
        assert!(!v.made);
        assert_eq!(v.lines[0].words[0].start, Duration::from_millis(700));
        // Another version's timing: the made words instead.
        tl.set_aligned(LyricsSource::LyricsPlus, aligned(&words, true, Timing::Elsewhere, 0, made(&[(5000, "a")])));
        let v = tl.view(TrackId::new());
        assert!(v.made);
        assert_eq!(v.lines[0].start, Duration::from_millis(5000));
    }

    #[test]
    fn genius_is_asked_only_without_timed_text_that_fits() {
        use nikgob_align::fit::Timing;
        let mut tl = TrackLyrics { genius: true, ..Default::default() };
        tl.pending.extend([LyricsSource::Lrclib, LyricsSource::Genius]);
        assert_eq!(tl.next_lookups(), [LyricsSource::Lrclib]);
        let lrc = Text::from_lrc(Some("[00:01.00]a"), None).unwrap();
        tl.insert(LyricsSource::Lrclib, Some(lrc.clone()));
        assert!(tl.next_lookups().is_empty(), "a timed text is there");
        assert!(!tl.pending.contains(&LyricsSource::Genius));
        // The check says it is another song's: Genius is wanted now.
        tl.set_aligned(LyricsSource::Lrclib, aligned(&lrc, false, Timing::Elsewhere, 0, Vec::new()));
        assert!(tl.genius_wanted());
        tl.pending.insert(LyricsSource::Genius);
        assert_eq!(tl.next_lookups(), [LyricsSource::Genius]);
        // Off in the settings: never.
        let mut off = TrackLyrics::default();
        off.pending.insert(LyricsSource::Genius);
        assert!(off.next_lookups().is_empty());
        // A plain text is cached and read back as plain.
        let g = Text::from_plain("раз\nдва").unwrap();
        let back = stored_lines(&g.body().unwrap()).unwrap();
        assert_eq!(back.plain.as_deref(), Some("раз\nдва"));
        assert!(back.lines.is_empty());
        let (lines, timed) = back.align_lines();
        assert!(!timed);
        assert_eq!(lines.len(), 2);
    }

    /// «Summer 15/16»: LRCLIB's lines and lrc.red's text are in the
    /// library, LyricsPlus, Better Lyrics and Genius are not. Once
    /// LyricsPlus answers, nothing is left to ask: Genius must not stay
    /// «ищу…» with no lookup on its way.
    #[test]
    fn nothing_stays_pending_once_nothing_is_left_to_ask() {
        let lib = Library::open_in_memory().unwrap();
        let track = lib
            .upsert_track(&nikgob_core::model::Track::new("Summer 15/16", nikgob_core::model::ArtistCredit::single("LAZZY2WICE", None)))
            .unwrap()
            .id;
        lib.put_lyrics(track, Some("[00:01.00]a\n[00:02.00]b"), Some("a\nb"), false).unwrap();
        let lrcred = Text::from_lrc(Some("[00:01.00]a\n[00:02.00]b"), None).unwrap().body();
        lib.put_lyrics_text(track, LyricsSource::LrcRed.key(), lrcred.as_deref()).unwrap();
        let mut tl = cached_lyrics(&lib, track, None, None, true, None);
        assert!(tl.pending.contains(&LyricsSource::Genius));
        assert_eq!(tl.next_lookups(), [LyricsSource::LyricsPlus]);
        for got in [None, Some(worded())] {
            let mut tl = tl.clone();
            tl.insert(LyricsSource::LyricsPlus, got);
            assert!(tl.next_lookups().is_empty());
            assert!(tl.pending.is_empty(), "{:?}", tl.pending);
            let v = tl.view(track);
            assert!(!v.sources.iter().any(|o| o.state == v::LyricsAvail::Loading), "{:?}", v.sources);
        }
        // LyricsPlus failed (no network): still nothing left pending.
        tl.pending.remove(&LyricsSource::LyricsPlus);
        tl.failed = true;
        assert!(tl.next_lookups().is_empty());
        assert!(tl.pending.is_empty());
        // Without a timed text Genius is what is asked.
        let mut plain = TrackLyrics { genius: true, ..Default::default() };
        plain.insert(LyricsSource::LrcRed, Text::from_lrc(None, Some("a")));
        plain.pending.extend([LyricsSource::BetterLyrics, LyricsSource::Genius]);
        assert_eq!(plain.next_lookups(), [LyricsSource::Genius]);
        assert!(plain.pending.contains(&LyricsSource::Genius));
    }

    #[tokio::test]
    async fn lyricsplus_is_asked_with_the_full_credit_before_the_main_artist() {
        use nikgob_core::model::{ArtistCredit, Track};
        use nikgob_metadata::{LyricsPlus, MetadataConfig};
        use wiremock::matchers::{path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let mut meta = Metadata::new(&MetadataConfig::new("tests@example.invalid")).unwrap();
        meta.lyricsplus = LyricsPlus::with_base_url(nikgob_metadata::Http::new(&MetadataConfig::new("tests@example.invalid")).unwrap(), &server.uri()).unwrap();
        let lib = Library::open_in_memory().unwrap();
        let mut credit = ArtistCredit::single("WENARO", None);
        credit.push(", ", "LXNER", None);
        let t = lib.upsert_track(&Track::new("Лёд", credit)).unwrap();
        let row = lib.get_track_row(t.id).unwrap().unwrap();
        let apple = r#"{"type":"Line","metadata":{"source":"Apple"},"lyrics":[{"time":1000,"duration":2000,"text":"Ты мой лёд"}]}"#;
        let qq = r#"{"type":"Word","metadata":{"source":"QQ Music"},"lyrics":[{"time":0,"duration":500,"text":"Composed by：Еремин Василий Сергеевич"},{"time":1000,"duration":2000,"text":"Ты мой лёд","syllabus":[{"time":1000,"duration":500,"text":"Ты "},{"time":1500,"duration":500,"text":"мой "},{"time":2000,"duration":1000,"text":"лёд"}]}]}"#;
        let found = |body: &'static str| ResponseTemplate::new(200).set_body_raw(body, "application/json");

        // The full credit is found: one request, Apple's lines.
        Mock::given(path("/v2/lyrics/get")).and(query_param("artist", "WENARO, LXNER")).respond_with(found(apple)).mount(&server).await;
        Mock::given(path("/v2/lyrics/get")).and(query_param("artist", "WENARO")).respond_with(found(qq)).mount(&server).await;
        let text = fetch_online(meta.clone(), lib.clone(), row.clone(), LyricsSource::LyricsPlus).await.unwrap().unwrap();
        assert!(!text.weak);
        assert_eq!(text.lines[0].text, "Ты мой лёд");
        let asked: Vec<String> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter_map(|r| r.url.query_pairs().find(|(k, _)| k == "artist").map(|(_, v)| v.into_owned()))
            .collect();
        assert_eq!(asked, ["WENARO, LXNER"]);

        // Not found under the full credit: the main artist next, whose QQ
        // text is a weak line sync without credits.
        server.reset().await;
        Mock::given(path("/v2/lyrics/get")).and(query_param("artist", "WENARO, LXNER")).respond_with(ResponseTemplate::new(404)).mount(&server).await;
        Mock::given(path("/v2/lyrics/get")).and(query_param("artist", "WENARO")).respond_with(found(qq)).mount(&server).await;
        let text = fetch_online(meta.clone(), lib.clone(), row.clone(), LyricsSource::LyricsPlus).await.unwrap().unwrap();
        assert!(text.weak);
        assert_eq!(text.sync(), LyricsSync::Line);
        assert_eq!(text.lines.len(), 1, "{:?}", text.lines);
        let asked: Vec<String> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter_map(|r| r.url.query_pairs().find(|(k, _)| k == "artist").map(|(_, v)| v.into_owned()))
            .collect();
        assert_eq!(asked, ["WENARO, LXNER", "WENARO"]);
        // Cached weak.
        assert!(cached_lyrics(&lib, t.id, None, None, false, None).texts[&LyricsSource::LyricsPlus].weak);
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
        assert!(cached_lyrics(&lib, t.id, None, None, false, None).missing.contains(&LyricsSource::LyricsPlus));
        server.reset().await;
        Mock::given(path("/v2/lyrics/get")).respond_with(ResponseTemplate::new(502)).mount(&server).await;
        assert!(fetch_online(meta, lib.clone(), row, LyricsSource::LyricsPlus).await.is_err());
        assert_eq!(lib.lyrics_text(t.id, "lyricsplus").unwrap().unwrap(), miss);
    }
}
