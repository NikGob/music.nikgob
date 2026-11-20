//! File, LyricsPlus, lrc.red, Better Lyrics, LRCLIB and Genius lyrics, and
//! the text heard in the recording when there is none. Synced file lyrics
//! win automatically; online texts compete by sync, then source order.
//!
//! A text the aligner checked against the track's own recording
//! ([`crate::align`]) and found to be another song's
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
//!
//! The user's own text ([`LyricsSource::Own`], edited by hand and kept as
//! a marked `.ttml` next to the file, see [`crate::lyrics_keep`]) is shown
//! first whatever the sources found; nothing fetched or made in the
//! background ever takes its place, and only the user removes it.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use nikgob_core::lyrics::{LyricLine, LyricsSource, LyricsSync};
use nikgob_core::model::TrackId;
use nikgob_library::{Library, TrackRow, now_ms};
use nikgob_metadata::{Metadata, MetadataError};
use nikgob_view::{self as v};

use crate::Worker;
use crate::enrich::LYRICS_RETRY_MS;
use crate::import::Internal;

mod fetch;
#[cfg(test)]
mod tests;

pub(crate) use fetch::{cached_lyrics, embed_text, fetch_track_lyrics, read_ttml, restore_pauses};
#[cfg(test)]
use fetch::{lookup_row, stored_lines};
use fetch::{ask_source, next_asked, shown_as, show_key};
#[cfg(test)]
use fetch::{cached_lyrics_at, fetch_online, genius_artists, miss_holds};

pub(crate) const LOOKUP_TIMEOUT: Duration = Duration::from_secs(20);
/// Tracks whose lyrics stay in memory besides the ones in use. Every source's
/// text down to syllables is kept per track, and the background backfill
/// goes through the whole library; the library rebuilds an entry on demand.
pub(crate) const LYRICS_KEPT: usize = 16;

/// Tracks whose lyrics are in memory and their estimated bytes, as of the
/// last change of the cache ([`memory`]).
static CACHED: AtomicUsize = AtomicUsize::new(0);
static CACHED_BYTES: AtomicUsize = AtomicUsize::new(0);

/// How many tracks' lyrics are in memory and about how many bytes they
/// take (every source's text with its words, what the aligner made), for
/// the app's memory line.
pub fn memory() -> (usize, usize) {
    (CACHED.load(Ordering::Relaxed), CACHED_BYTES.load(Ordering::Relaxed))
}

/// Notes the cache's size for [`memory`].
fn note_memory(cache: &HashMap<TrackId, TrackLyrics>) {
    CACHED.store(cache.len(), Ordering::Relaxed);
    CACHED_BYTES.store(cache.values().map(TrackLyrics::approx_bytes).sum(), Ordering::Relaxed);
}

/// About how many bytes `lines` take: the lines, their words and texts.
fn lines_bytes(lines: &[LyricLine]) -> usize {
    let text = |s: &Option<String>| s.as_ref().map_or(0, String::len);
    lines
        .iter()
        .map(|l| {
            std::mem::size_of::<LyricLine>()
                + l.text.len()
                + text(&l.translation)
                + text(&l.romanization)
                + l.words.iter().map(|w| std::mem::size_of_val(w) + w.text.len()).sum::<usize>()
        })
        .sum()
}

/// Marks `track` as the latest used in `order` (least recently used first).
fn touch(order: &mut VecDeque<TrackId>, track: TrackId) {
    order.retain(|t| *t != track);
    order.push_back(track);
}

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

/// Line times one shift this far off belong to another take of the song
/// (a source's own habit is a few tenths at most).
const OTHER_TAKE: Duration = Duration::from_secs(1);

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
        // A text that does not fit keeps its timing in the library (synced,
        // never made again) but is shown as before: as not fitting.
        let lines: Vec<LyricLine> =
            row.body.as_deref().filter(|_| row.fits).and_then(|b| serde_json::from_str(b).ok()).unwrap_or_default();
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
                fits: row.fits && row.body.is_some(),
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
        // A text kept without timing («Сохранить текст» of a plain text).
        if let Some(plain) = nikgob_metadata::parse_ttml_plain(xml) {
            return Self::from_plain(&plain);
        }
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

    /// The text less its credits lines ([`nikgob_core::lyrics::drop_credits`]);
    /// `None` when nothing is left.
    pub(crate) fn without_credits(mut self, names: &[String]) -> Option<Self> {
        nikgob_core::lyrics::drop_credits(&mut self.lines, names);
        if let Some(p) = &self.plain {
            self.plain = Some(nikgob_core::lyrics::drop_credit_text(p, names));
        }
        (!self.lines.is_empty() || self.plain.as_deref().is_some_and(|p| !p.trim().is_empty())).then_some(self)
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
    /// A song without a file whose text can be kept in its artist's
    /// folder of the library (set by the worker).
    pub(crate) folder: bool,
    /// How the user chose to see the shown text (`None`: as it comes).
    pub(crate) display: Option<LyricsSync>,
    /// The track's artists, to tell a line of a fetched text that only
    /// credits them from a sung one ([`nikgob_core::lyrics::drop_credits`]).
    pub(crate) names: Vec<String>,
    /// What plays is a music video: a found text's timing is the song's
    /// record's, not the video's (an intro moves it), so its lines show
    /// without highlighting until the check heard them in the video (and
    /// moved them when they are one steady shift off).
    pub(crate) clip: bool,
}

impl TrackLyrics {
    /// About how many bytes the entry takes (see [`memory`]).
    fn approx_bytes(&self) -> usize {
        let texts: usize = self.texts.values().map(|t| lines_bytes(&t.lines) + t.plain.as_ref().map_or(0, String::len)).sum();
        let aligned: usize = self.aligned.values().map(|a| lines_bytes(&a.lines) + a.hash.len()).sum();
        let checking: usize = self.checking.iter().flatten().map(String::len).sum();
        std::mem::size_of::<Self>()
            + texts
            + aligned
            + checking
            + self.last_embed.as_ref().map_or(0, String::len)
            + self.names.iter().map(String::len).sum::<usize>()
    }

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

    /// `source`'s timing is a found one of a clip's text that the check has
    /// not heard in the video yet (see [`Self::clip`]).
    pub(crate) fn unverified(&self, source: LyricsSource) -> bool {
        self.clip
            && !matches!(source, LyricsSource::Own | LyricsSource::File | LyricsSource::Recognized)
            && self.texts.get(&source).is_some_and(|t| t.sync() > LyricsSync::Plain)
            && !self.aligned(source).is_some_and(|a| a.fits)
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
            LyricsSource::Own => 7,
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
        // The user's own text comes before anything found.
        if self.has_own() {
            return Some(LyricsSource::Own);
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
            // The user's own text is timed only when they ask.
            .filter(|(s, _)| !matches!(**s, LyricsSource::Recognized | LyricsSource::Own))
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

    /// The source the user picked, if any.
    pub(crate) fn picked(&self) -> Option<LyricsSource> {
        self.pick
    }

    /// `source`'s text is gone (the user's own text taken away): neither
    /// found nor missed, and no longer picked.
    pub(crate) fn forget(&mut self, source: LyricsSource) {
        self.texts.remove(&source);
        self.missing.remove(&source);
        self.pending.remove(&source);
        if self.pick == Some(source) {
            self.pick = None;
        }
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

    /// Genius is worth asking now ([`Self::genius_worth`]) and has not
    /// answered yet.
    fn genius_wanted(&self) -> bool {
        self.genius
            && !self.texts.contains_key(&LyricsSource::Genius)
            && !self.missing.contains(&LyricsSource::Genius)
            && self.genius_worth()
    }

    /// Genius' plain text is worth a look: no text with timing fits the
    /// recording as this take of the song ([`Self::other_take`]), or the
    /// text shown fits only moved by a shift ([`Self::chosen_moved`]).
    fn genius_worth(&self) -> bool {
        !self.has_timed_take() || self.chosen_moved()
    }

    /// The text shown is a source's whose line times the aligner moved by
    /// one shift to fit (LRCLIB's lines 2.14 s early, «космоцветок»): its
    /// times were another take's, so Genius is asked once too, and its text
    /// is there to pick in the source menu.
    fn chosen_moved(&self) -> bool {
        use nikgob_align::fit::Timing;
        self.chosen()
            .filter(|s| !matches!(s, LyricsSource::Recognized | LyricsSource::Genius))
            .and_then(|s| self.aligned(s))
            .is_some_and(|a| a.fits && a.timing == Timing::Shifted)
    }

    /// A text with timing fits the recording and its times are this take's.
    fn has_timed_take(&self) -> bool {
        self.texts.iter().any(|(s, t)| *s != LyricsSource::Recognized && t.sync() > LyricsSync::Plain && !self.misfit(*s) && !self.other_take(*s))
    }

    /// The recording spells the text, but its own line times are another
    /// take's: another version, or one shift of [`OTHER_TAKE`] or more (a
    /// re-recording that opens with a spoken line: «космоцветок», 2.14 s).
    /// Its words may be that take's too, so a plain text is worth a look.
    fn other_take(&self, source: LyricsSource) -> bool {
        use nikgob_align::fit::Timing;
        self.aligned(source).is_some_and(|a| a.fits && (a.timing == Timing::Elsewhere || (a.timing == Timing::Shifted && a.offset >= OTHER_TAKE)))
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
    /// timing that fits the recording is there or the shown one was moved
    /// to fit ([`Self::genius_worth`]). When nothing is left to
    /// ask, nothing stays pending (a source is never shown as searching
    /// without a lookup on its way).
    fn next_lookups(&mut self) -> Vec<LyricsSource> {
        let online = self.next_online();
        if !online.is_empty() {
            return online;
        }
        if self.pending.contains(&LyricsSource::Genius) && self.genius && self.genius_worth() {
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
        let (_, made) = source.map(|s| self.shown(s)).unwrap_or_default();
        let (mut lines, made_shown) = source.map(|s| self.displayed(s)).unwrap_or_default();
        // A clip's found timing not checked yet: its lines without timing.
        if source.is_some_and(|s| self.unverified(s)) && !made_shown {
            lines.clear();
        }
        // Seen without timing: the text's lines as plain text.
        let plain = text.plain.clone().or_else(|| {
            (lines.is_empty() && !text.lines.is_empty()).then(|| text.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n"))
        });
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
            Some(LyricsSource::File | LyricsSource::Own) => v::LyricsHome::Computer,
            // A found text the file already holds as it is (saved there
            // by «Сохранять найденный текст в файл трека»).
            Some(s) if self.file_holds(s) => v::LyricsHome::Computer,
            _ => v::LyricsHome::Online,
        };
        let craft = match (source, &self.place) {
            (None | Some(LyricsSource::Recognized), _) | (_, Place::None) => v::LyricsCraft::Off,
            (_, Place::Unclear(why)) => v::LyricsCraft::No(why.clone()),
            _ => v::LyricsCraft::Ready,
        };
        // Any shown text not on the computer yet can go there: next to
        // the file, or into the artist's folder for a song without one.
        // Never another text over the user's own (it goes back to the
        // found ones first, «Вернуть найденный»).
        let savable = home != v::LyricsHome::Computer
            && source.is_some_and(|s| self.to_keep(s).is_some())
            && (source == Some(LyricsSource::Own) || !self.has_own())
            && (self.place == Place::File || self.folder);
        v::LyricsView {
            track: Some(track),
            lines,
            plain,
            loading: source.is_none() && !self.pending.is_empty(),
            source,
            picked: self.pick.is_some() && self.pick == source,
            sources,
            made: made_shown,
            checking: self.checking.is_some(),
            texts,
            home,
            craft,
            savable,
            shows: source.map(|s| self.shows(s)).unwrap_or_default(),
            stream: self.place != Place::File,
        }
    }

    /// Timing the player made of `source`'s text that fits the recording.
    fn made_lines(&self, source: LyricsSource) -> Option<&Vec<LyricLine>> {
        self.aligned(source).filter(|a| a.fits && !a.lines.is_empty()).map(|a| &a.lines)
    }

    /// How `source`'s text can be seen, finest first: by words and by
    /// lines as far as the shown timing or the player's made one goes,
    /// without timing always.
    pub(crate) fn shows(&self, source: LyricsSource) -> Vec<LyricsSync> {
        if !self.texts.contains_key(&source) {
            return Vec::new();
        }
        let (lines, _) = self.shown(source);
        let made = if source == LyricsSource::Recognized { None } else { self.made_lines(source) };
        let finest = LyricsSync::of(&lines).max(made.and_then(|m| LyricsSync::of(m))).unwrap_or(LyricsSync::Plain);
        [LyricsSync::Word, LyricsSync::Line, LyricsSync::Plain].into_iter().filter(|s| *s <= finest).collect()
    }

    /// The lines `source` is seen with, and whether their timing is the
    /// player's: [`Self::shown`], made coarser or finer as the user chose
    /// ([`Self::display`]) as far as the text has that timing.
    pub(crate) fn displayed(&self, source: LyricsSource) -> (Vec<LyricLine>, bool) {
        let (lines, made) = self.shown(source);
        let Some(want) = self.display else { return (lines, made) };
        let have = LyricsSync::of(&lines).unwrap_or(LyricsSync::Plain);
        let other = if source == LyricsSource::Recognized { None } else { self.made_lines(source) };
        match want {
            LyricsSync::Plain => (Vec::new(), false),
            LyricsSync::Line if have >= LyricsSync::Line => (crate::align::to_lines(&lines), made),
            LyricsSync::Line => match other {
                Some(m) => (crate::align::to_lines(m), true),
                None => (lines, made),
            },
            _ if have >= LyricsSync::Word => (lines, made),
            _ => match other.filter(|m| LyricsSync::of(m) >= Some(LyricsSync::Word)) {
                Some(m) => (m.clone(), true),
                None => (lines, made),
            },
        }
    }

    /// What «Сохранить текст» keeps of `source`'s text: its lines with
    /// the finest timing shown or chosen to be seen (the source's own or
    /// the player's), else the plain text.
    pub(crate) fn to_keep(&self, source: LyricsSource) -> Option<crate::lyrics_keep::Keep> {
        let (shown, _) = self.shown(source);
        let (seen, _) = self.displayed(source);
        let lines = if LyricsSync::of(&seen) > LyricsSync::of(&shown) { seen } else { shown };
        if !lines.is_empty() {
            return Some(crate::lyrics_keep::Keep::Lines(lines));
        }
        let text = self.texts.get(&source)?;
        let plain = text.plain.clone().unwrap_or_else(|| text.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n"));
        (!plain.trim().is_empty()).then(|| crate::lyrics_keep::Keep::Plain(plain))
    }

    /// The user's own text is there ([`LyricsSource::Own`]).
    pub(crate) fn has_own(&self) -> bool {
        self.texts.contains_key(&LyricsSource::Own)
    }

    /// The file's own text is `source`'s as it is.
    fn file_holds(&self, source: LyricsSource) -> bool {
        match (self.texts.get(&LyricsSource::File), self.texts.get(&source)) {
            (Some(file), Some(t)) => source != LyricsSource::File && (file == t || file.serialized() == t.serialized()),
            _ => false,
        }
    }

    /// A lookup or a file write is under way: its result needs this entry.
    fn busy(&self) -> bool {
        !self.pending.is_empty() || self.embedding
    }

    /// Takes a lookup's answer. A miss leaves a text found earlier in
    /// place: a search asked again keeps what it had.
    fn found(&mut self, source: LyricsSource, text: Option<Text>) {
        if text.is_none() && self.texts.contains_key(&source) {
            self.pending.remove(&source);
        } else {
            self.insert(source, text);
        }
    }

    /// «Искать текст заново»: every online source is asked again whatever
    /// the cache says, in the usual order and with the usual gates (Genius
    /// when it is on and worth asking). The texts found earlier stay shown
    /// until a source answers with a new one ([`Self::found`]).
    pub(crate) fn search_again(&mut self) {
        let online = [LyricsSource::LyricsPlus, LyricsSource::LrcRed, LyricsSource::BetterLyrics, LyricsSource::Lrclib];
        for source in online.into_iter().chain(self.genius.then_some(LyricsSource::Genius)) {
            self.missing.remove(&source);
            self.pending.insert(source);
        }
        self.failed = false;
    }

    fn insert(&mut self, source: LyricsSource, text: Option<Text>) {
        self.pending.remove(&source);
        // A songwriters line before the first sung one is not lyrics (the
        // user's own text stays as they wrote it).
        let text = match text {
            Some(t) if !matches!(source, LyricsSource::Recognized | LyricsSource::Own) => t.without_credits(&self.names),
            t => t,
        };
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
    /// Stores `track`'s lyrics, then forgets the least recently used ones
    /// past [`LYRICS_KEPT`]: not the shown, playing or next track, and not
    /// one with a lookup or a write still to come back. A forgotten one is
    /// read from the library again when it is wanted ([`cached_lyrics`]).
    fn cache_lyrics(&mut self, track: TrackId, tl: TrackLyrics) {
        let keep = [
            Some(track),
            self.state.read().lyrics.track,
            self.queue.current().map(|e| e.track),
            self.queue.peek_auto_next().map(|e| e.track),
        ];
        let show = Shown::of(&self.state.read().prefs);
        let place = self.lyrics_place(track);
        let folder = self.lyrics_folder(track);
        let clip = self.clip_audio(track).is_some();
        let Some(svc) = self.import.as_mut() else { return };
        svc.lyrics_cache.insert(track, TrackLyrics { show, place, folder, clip, ..tl });
        touch(&mut svc.lyrics_order, track);
        let (cache, later) = (&svc.lyrics_cache, &svc.embed_later);
        let spare = |t: TrackId| {
            keep.contains(&Some(t)) || later.contains(&t) || cache.get(&t).is_some_and(TrackLyrics::busy)
        };
        for t in evict(&mut svc.lyrics_order, LYRICS_KEPT, spare) {
            svc.lyrics_cache.remove(&t);
            // Rebuilt from the library next time; what it lacks is asked again.
            svc.lyrics_asking.retain(|(asked, _)| *asked != t);
        }
        note_memory(&svc.lyrics_cache);
    }

    pub(crate) fn lyrics_backfilled(&mut self, track: TrackId, mut tl: TrackLyrics) {
        tl.pick = self.lib.lyrics_pick(track).ok().flatten().and_then(|k| LyricsSource::from_key(&k));
        tl.display = shown_as(&self.lib, track);
        tl.show = Shown::of(&self.state.read().prefs);
        let local = self.lib.active_source(track).ok().flatten().is_some_and(|s| s.kind.is_local());
        if !local
            && tl.texts_of(LyricsSource::File).is_none()
            && !tl.has_own()
            && let Some((kept, own)) = self.kept_text(track).as_deref().and_then(read_ttml)
        {
            tl.insert(if own { LyricsSource::Own } else { LyricsSource::File }, Some(kept));
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
            // Used again: the last to be forgotten.
            if let Some(svc) = self.import.as_mut() {
                touch(&mut svc.lyrics_order, track);
            }
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
        tl.clip = self.clip_audio(track).is_some();
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

    /// `Command::SearchLyricsAgain`: the track's lyrics are looked up online
    /// again, whatever the cache holds ([`TrackLyrics::search_again`]).
    pub(crate) fn search_lyrics_again(&mut self, track: TrackId) {
        let Some(svc) = self.import.as_mut() else { return };
        if svc.metadata().is_none() {
            return;
        }
        let Some(tl) = svc.lyrics_cache.get_mut(&track) else { return };
        tl.search_again();
        self.start_lyrics_lookups(track);
        let Some(view) = self.import.as_ref().and_then(|s| s.lyrics_cache.get(&track)).map(|tl| tl.view(track)) else { return };
        self.publish(|s| {
            if s.lyrics.track == Some(track) {
                s.lyrics = view;
            }
        });
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
        let sources = next_asked(tl, &meta);
        for source in sources {
            // Already on its way: its answer asks for the next ones.
            if !svc.lyrics_asking.insert((track, source)) {
                continue;
            }
            let (meta, lib, row, tx) = (meta.clone(), self.lib.clone(), row.clone(), svc.internal());
            svc.spawn(async move {
                let got = ask_source(meta, lib, row, source).await;
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
        // The user's own text stays in its `.ttml` (never into the tag).
        let Some(source) = tl.chosen().filter(|s| !matches!(s, LyricsSource::Own | LyricsSource::File | LyricsSource::Recognized)) else { return };
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
            Ok(text) => tl.found(source, text),
            Err(e) => {
                // Not a miss: asked again the next time the song is shown.
                log::info!("lyrics lookup ({}): {e}", source.key());
                tl.pending.remove(&source);
                tl.failed = true;
            }
        }
        self.start_lyrics_lookups(track);
        let view = self.import.as_ref().unwrap().lyrics_cache[&track].view(track);
        note_memory(&self.import.as_ref().unwrap().lyrics_cache);
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
        let genius = self.state.read().prefs.lyrics_genius;
        let shown = self.state.read().lyrics.track;
        let tracks: Vec<TrackId> = self.import.as_ref().map(|s| s.lyrics_cache.keys().copied().collect()).unwrap_or_default();
        let places: Vec<(TrackId, Place)> = tracks.into_iter().map(|t| (t, self.lyrics_place(t))).collect();
        let Some(svc) = self.import.as_mut() else { return };
        for tl in svc.lyrics_cache.values_mut() {
            tl.show = show;
            tl.genius = genius;
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
        // Genius just turned on: the shown song asks it when it is wanted.
        if let Some(track) = shown {
            self.start_lyrics_lookups_again(track);
        }
        if let Some(track) = self.queue.current().map(|e| e.track) {
            self.check_lyrics(track);
        }
    }

    /// `Command::ShowLyricsAs`: the track's lyrics are seen by words, by
    /// lines or without timing from now on (remembered per track; a text
    /// without that timing is seen as fine as it goes).
    pub(crate) fn show_lyrics_as(&mut self, track: TrackId, sync: LyricsSync) {
        self.set_lyrics_display(track, Some(sync));
    }

    /// Remembers how `track`'s lyrics are seen (`None`: as they come) and
    /// shows them so.
    pub(crate) fn set_lyrics_display(&mut self, track: TrackId, sync: Option<LyricsSync>) {
        let sync = sync.map(|s| if s == LyricsSync::Syllable { LyricsSync::Word } else { s });
        if let Err(e) = self.lib.set_lyrics_show(track, sync.map(show_key)) {
            log::warn!("saving how lyrics are seen: {e}");
        }
        let Some(tl) = self.import.as_mut().and_then(|s| s.lyrics_cache.get_mut(&track)) else { return };
        tl.display = sync;
        let view = tl.view(track);
        self.publish(|s| {
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
        self.publish(|s| {
            if s.lyrics.track == Some(track) {
                s.lyrics = view;
            }
        });
    }
}
