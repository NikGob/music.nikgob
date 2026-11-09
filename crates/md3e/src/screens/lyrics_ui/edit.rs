//! The lines of the lyrics editor («Изменить текст…», [`super::editor`]):
//! the text as the user edits it, each line with its start, and the text
//! with its timing that saving keeps as theirs (`Command::SaveOwnLyrics`).
//!
//! A line keeps the word timing it came with while only its time moves
//! (the words move with it); once its text changes, it is timed by its
//! start alone (the aligner may time its words again, «Сделать подсветку
//! заново»). Splitting a line between two of its words keeps each half's
//! words.

use std::time::Duration;

use nikgob_view::{LyricLine, LyricWord, LyricsSync, LyricsView, TrackId};

/// How far apart the lines after the last timed one are guessed.
const STEP: Duration = Duration::from_secs(2);

/// One line being edited.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Line {
    pub text: String,
    /// Its start; `None` until it is timed.
    pub start: Option<Duration>,
    /// The line as it came: its words (timed from its own start), its end
    /// and how it is sung (a background voice, a duet's second voice, a
    /// translation). Its words hold while the text is the same.
    from: LyricLine,
}

impl Line {
    /// A line of `text`, timed or not.
    pub(crate) fn new(text: impl Into<String>, start: Option<Duration>) -> Self {
        let text = text.into();
        Self { from: LyricLine { start: start.unwrap_or_default(), text: text.clone(), ..Default::default() }, text, start }
    }

    /// A line as a text has it; `timed`: its start is a real time.
    fn of(l: &LyricLine, timed: bool) -> Self {
        Self { text: l.text.clone(), start: timed.then_some(l.start), from: l.clone() }
    }

    /// A background voice (shown smaller).
    pub(crate) fn background(&self) -> bool {
        self.from.background
    }

    /// Its words still hold: the same text, and a start to move them by.
    pub(crate) fn worded(&self) -> bool {
        self.start.is_some() && !self.from.words.is_empty() && self.text.trim() == self.from.text.trim()
    }

    /// `t` of the line as it came, where it is now.
    fn moved(&self, t: Duration) -> Duration {
        let Some(start) = self.start else { return t };
        if start >= self.from.start { t + (start - self.from.start) } else { t.saturating_sub(self.from.start - start) }
    }

    /// Its words where they are now (none once the text changed).
    fn words(&self) -> Vec<LyricWord> {
        if !self.worded() {
            return Vec::new();
        }
        self.from.words.iter().map(|w| LyricWord { start: self.moved(w.start), end: self.moved(w.end), text: w.text.clone() }).collect()
    }

    /// As a line of the saved text, starting at `start`.
    fn to_line(&self, start: Duration) -> LyricLine {
        let worded = self.worded();
        LyricLine {
            start,
            end: if worded { self.from.end.map(|e| self.moved(e)) } else { None },
            text: self.text.trim().to_owned(),
            words: self.words(),
            ..self.from.clone()
        }
    }

    /// The same line with `text` and `words` (timed where they are now)
    /// from `start`.
    fn remade(&self, text: String, start: Option<Duration>, words: Vec<LyricWord>) -> Self {
        let end = words.last().map(|w| w.end);
        let from = LyricLine { start: start.unwrap_or_default(), end, text: text.clone(), words, ..self.from.clone() };
        Self { text, start, from }
    }
}

/// The text being edited for one track, kept for the session until it is
/// saved or given up (closing the editor by accident loses nothing).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Draft {
    pub track: TrackId,
    /// «Песня · Исполнитель», for the editor's head.
    pub title: String,
    pub lines: Vec<Line>,
    pub selected: Option<usize>,
    /// The text cursor in a line's text: (the line, the character).
    pub cursor: Option<(usize, usize)>,
    /// A line whose text takes the focus next frame.
    pub focus: Option<usize>,
    /// The selected line is brought into view next frame.
    pub scroll: bool,
    /// The lines as they were opened, to tell a change.
    opened: Vec<Line>,
}

impl Draft {
    /// The shown text of `l` to edit: its finest timing (the source's own
    /// or the player's), else its plain text one line a line.
    pub(crate) fn of(l: &LyricsView, title: String) -> Option<Self> {
        let track = l.track?;
        let lines: Vec<Line> = match base(l) {
            Some(lines) => lines.iter().map(|x| Line::of(x, true)).collect(),
            None => l.plain.as_deref().unwrap_or_default().replace("\r\n", "\n").trim().lines().map(|t| Line::new(t.trim(), None)).collect(),
        };
        Some(Self { track, title, opened: lines.clone(), lines, selected: None, cursor: None, focus: None, scroll: false })
    }

    /// Something was changed since it was opened.
    pub(crate) fn changed(&self) -> bool {
        self.lines != self.opened
    }

    /// Some line has a start: saved with timing.
    pub(crate) fn timed(&self) -> bool {
        self.lines.iter().any(|l| l.start.is_some())
    }

    pub(crate) fn select(&mut self, i: usize) {
        if i < self.lines.len() {
            self.selected = Some(i);
        }
    }

    /// Takes line `i` away; the line after it is selected (or the last).
    pub(crate) fn delete(&mut self, i: usize) {
        if i >= self.lines.len() {
            return;
        }
        self.lines.remove(i);
        self.cursor = None;
        self.selected = if self.lines.is_empty() { None } else { Some(i.min(self.lines.len() - 1)) };
    }

    /// An empty untimed line under `i` (at the end without one), selected
    /// and taking the focus.
    pub(crate) fn add_below(&mut self, i: Option<usize>) {
        let at = i.map_or(self.lines.len(), |i| (i + 1).min(self.lines.len()));
        self.lines.insert(at, Line::new("", None));
        self.selected = Some(at);
        self.focus = Some(at);
        self.cursor = None;
    }

    /// Line `i` cut in two at character `at` (spaces around the cut go).
    /// The first half keeps the start; between two of its words each half
    /// keeps its words and the second starts with its first word, else the
    /// second half waits to be timed. The second half is selected.
    pub(crate) fn split(&mut self, i: usize, at: usize) {
        let Some(line) = self.lines.get(i).cloned() else { return };
        let byte = line.text.char_indices().nth(at).map_or(line.text.len(), |(b, _)| b);
        let (a, b) = line.text.split_at(byte);
        let (a, b) = (a.trim_end().to_owned(), b.trim_start().to_owned());
        if a.is_empty() || b.is_empty() {
            return;
        }
        let (first, second) = match word_cut(&line, byte) {
            Some(k) => {
                let words = line.words();
                let (wa, wb) = words.split_at(k);
                let mut wa = wa.to_vec();
                if let Some(last) = wa.last_mut() {
                    last.text = last.text.trim_end().to_owned();
                }
                let second_start = wb.first().map(|w| w.start);
                (line.remade(a, line.start, wa), line.remade(b, second_start, wb.to_vec()))
            }
            None => (line.remade(a, line.start, Vec::new()), line.remade(b, None, Vec::new())),
        };
        self.lines[i] = first;
        self.lines.insert(i + 1, second);
        self.selected = Some(i + 1);
        self.cursor = None;
    }

    /// Line `i` and the next as one line from `i`'s start (the next one's
    /// when `i` has none): their words go on when both kept theirs.
    pub(crate) fn merge(&mut self, i: usize) {
        if i + 1 >= self.lines.len() {
            return;
        }
        let (a, b) = (self.lines[i].clone(), self.lines[i + 1].clone());
        let text = match (a.text.trim(), b.text.trim()) {
            ("", t) | (t, "") => t.to_owned(),
            (x, y) => format!("{x} {y}"),
        };
        let words = if a.worded() && b.worded() {
            let mut words = a.words();
            if let Some(last) = words.last_mut()
                && !last.text.ends_with(char::is_whitespace)
            {
                last.text.push(' ');
            }
            words.extend(b.words());
            words
        } else {
            Vec::new()
        };
        self.lines[i] = a.remade(text, a.start.or(b.start), words);
        self.lines.remove(i + 1);
        self.selected = Some(i);
        self.cursor = None;
    }

    /// Line `i` starts at `t` (its words move with it).
    pub(crate) fn set_start(&mut self, i: usize, t: Duration) {
        if let Some(l) = self.lines.get_mut(i) {
            l.start = Some(t);
        }
    }

    /// Line `i`'s start moved by `ms` (never before the song's start); an
    /// untimed line stays so.
    pub(crate) fn nudge(&mut self, i: usize, ms: i64) {
        if let Some(l) = self.lines.get_mut(i)
            && let Some(s) = l.start
        {
            let t = (s.as_millis() as i64 + ms).max(0);
            l.start = Some(Duration::from_millis(t as u64));
        }
    }

    /// «Отметить»: the selected line starts at `t` (where the song is now)
    /// and the next line is selected (none after the last). False with no
    /// line selected.
    pub(crate) fn mark(&mut self, t: Duration) -> bool {
        let Some(i) = self.selected.filter(|i| *i < self.lines.len()) else { return false };
        self.set_start(i, t);
        self.selected = (i + 1 < self.lines.len()).then_some(i + 1);
        self.scroll = true;
        true
    }

    /// The selection moved by `by` lines (the first or last line at most).
    pub(crate) fn step(&mut self, by: isize) {
        if self.lines.is_empty() {
            return;
        }
        let last = self.lines.len() as isize - 1;
        let at = match self.selected {
            Some(i) => (i as isize + by).clamp(0, last),
            None if by < 0 => last,
            None => 0,
        };
        self.selected = Some(at as usize);
        self.scroll = true;
    }

    /// The line playing at `t`: the last timed one started by then.
    pub(crate) fn playing(&self, t: Duration) -> Option<usize> {
        self.lines.iter().enumerate().filter(|(_, l)| l.start.is_some_and(|s| s <= t)).map(|(i, _)| i).last()
    }

    /// Line `i` starts before a line above it.
    pub(crate) fn early(&self, i: usize) -> bool {
        let Some(s) = self.lines.get(i).and_then(|l| l.start) else { return false };
        self.lines[..i].iter().filter_map(|l| l.start).any(|before| before > s)
    }

    /// The text as it is saved: every line with a start (an untimed one
    /// between timed ones evenly between them, before the first from the
    /// song's start, after the last [`STEP`] apart), and its words where
    /// they still hold. With no line timed, the starts are all zero (it is
    /// saved as a plain text).
    pub(crate) fn to_lines(&self) -> Vec<LyricLine> {
        let known: Vec<(usize, Duration)> = self.lines.iter().enumerate().filter_map(|(i, l)| l.start.map(|s| (i, s))).collect();
        self.lines
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let start = l.start.unwrap_or_else(|| guess(&known, i));
                l.to_line(start)
            })
            .collect()
    }
}

/// The start of untimed line `i` from the timed ones around it.
fn guess(known: &[(usize, Duration)], i: usize) -> Duration {
    let before = known.iter().rev().find(|(k, _)| *k < i);
    let after = known.iter().find(|(k, _)| *k > i);
    match (before, after) {
        (Some(&(pi, ps)), Some(&(ni, ns))) if ns > ps => ps + (ns - ps).mul_f64((i - pi) as f64 / (ni - pi) as f64),
        (Some(&(_, ps)), Some(_)) => ps,
        (Some(&(pi, ps)), None) => ps + STEP * (i - pi) as u32,
        (None, Some(&(ni, ns))) => ns.mul_f64(i as f64 / ni as f64),
        (None, None) => Duration::ZERO,
    }
}

/// How many of `line`'s words come before a cut at byte `at` of its text
/// that falls between two of them (on the space or either side of it);
/// `None` inside a word, or when its words are not its text.
fn word_cut(line: &Line, at: usize) -> Option<usize> {
    if !line.worded() {
        return None;
    }
    let joined: String = line.from.words.iter().map(|w| w.text.as_str()).collect();
    if joined != line.text {
        return None;
    }
    let mut end = 0;
    for (k, w) in line.from.words.iter().enumerate().take(line.from.words.len() - 1) {
        let word_end = end + w.text.trim_end().len();
        end += w.text.len();
        if (word_end..=end).contains(&at) {
            return Some(k + 1);
        }
    }
    None
}

/// The finest timed lines of the shown text: what is seen, or the
/// source's or the player's own when they are finer (seen by lines).
fn base(l: &LyricsView) -> Option<Vec<LyricLine>> {
    let text = l.texts.iter().find(|t| l.source.is_some() && t.source == l.source);
    let finer = text.and_then(|t| t.made.clone().filter(|m| !m.is_empty()).or_else(|| (!t.lines.is_empty()).then(|| t.lines.clone())));
    match finer {
        Some(f) if LyricsSync::of(&f) > LyricsSync::of(&l.lines) => Some(f),
        _ if !l.lines.is_empty() => Some(l.lines.clone()),
        f => f,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    /// «Знаешь, не беда» at 1 s with its three words.
    fn worded() -> LyricLine {
        let w = |a: u64, b: u64, t: &str| LyricWord { start: ms(a), end: ms(b), text: t.into() };
        let mut l = LyricLine::new(ms(1_000), "Знаешь, не беда");
        l.words = vec![w(1_000, 1_400, "Знаешь, "), w(1_400, 1_600, "не "), w(1_600, 2_400, "беда")];
        l.end = Some(ms(2_400));
        l
    }

    fn draft(lines: Vec<Line>) -> Draft {
        Draft { track: TrackId::new(), title: String::new(), opened: lines.clone(), lines, selected: None, cursor: None, focus: None, scroll: false }
    }

    fn texts(d: &Draft) -> Vec<&str> {
        d.lines.iter().map(|l| l.text.as_str()).collect()
    }

    #[test]
    fn moving_a_line_moves_its_words_and_changing_its_text_drops_them() {
        let mut d = draft(vec![Line::of(&worded(), true), Line::new("Вторая", Some(ms(3_000)))]);
        assert!(!d.changed());
        d.nudge(0, 100);
        d.nudge(0, 100);
        let saved = d.to_lines();
        assert_eq!(saved[0].start, ms(1_200));
        assert_eq!(saved[0].words.iter().map(|w| w.start).collect::<Vec<_>>(), [ms(1_200), ms(1_600), ms(1_800)]);
        assert_eq!(saved[0].end, Some(ms(2_600)));
        assert!(d.changed());
        // Earlier than it came, and never before the song's start.
        d.set_start(0, ms(500));
        assert_eq!(d.to_lines()[0].words[0].start, ms(500));
        d.nudge(0, -900);
        assert_eq!(d.lines[0].start, Some(ms(0)));
        // A new text: timed by its start alone.
        d.lines[0].text = "Знаешь, не беда, нет".into();
        let saved = d.to_lines();
        assert!(saved[0].words.is_empty() && saved[0].end.is_none());
        assert_eq!(saved[0].text, "Знаешь, не беда, нет");
        // The same text with a space more is the same text.
        d.lines[0].text = "Знаешь, не беда ".into();
        assert!(d.lines[0].worded());
        assert_eq!(d.to_lines()[0].text, "Знаешь, не беда");
    }

    #[test]
    fn deleting_and_adding_lines_keeps_the_selection_in_place() {
        let mut d = draft(vec![Line::new("a", None), Line::new("b", None), Line::new("c", None)]);
        d.delete(1);
        assert_eq!(texts(&d), ["a", "c"]);
        assert_eq!(d.selected, Some(1));
        d.delete(1);
        assert_eq!(d.selected, Some(0));
        d.add_below(Some(0));
        assert_eq!(texts(&d), ["a", ""]);
        assert_eq!((d.selected, d.focus), (Some(1), Some(1)));
        d.add_below(None);
        assert_eq!(d.lines.len(), 3);
        d.delete(9);
        assert_eq!(d.lines.len(), 3);
        d.delete(0);
        d.delete(0);
        d.delete(0);
        assert!(d.lines.is_empty() && d.selected.is_none());
    }

    /// Cut between two words, each half keeps its words and the second
    /// starts with its first word; cut inside a word, the second half
    /// waits to be timed.
    #[test]
    fn splitting_between_words_keeps_them_and_inside_a_word_drops_them() {
        let mut d = draft(vec![Line::of(&worded(), true)]);
        d.nudge(0, 100);
        // Right before «не» (after the space).
        d.split(0, 8);
        assert_eq!(texts(&d), ["Знаешь,", "не беда"]);
        assert_eq!(d.selected, Some(1));
        let saved = d.to_lines();
        assert_eq!(saved[0].words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), ["Знаешь,"]);
        assert_eq!(saved[1].start, ms(1_500), "its first word's start, moved with the line");
        assert_eq!(saved[1].words.iter().map(|w| w.start).collect::<Vec<_>>(), [ms(1_500), ms(1_700)]);
        // Merged back: one line with every word again.
        d.merge(0);
        assert_eq!(texts(&d), ["Знаешь, не беда"]);
        let saved = d.to_lines();
        assert_eq!(saved[0].start, ms(1_100));
        assert_eq!(saved[0].words.len(), 3);
        assert_eq!(saved[0].words[0].text, "Знаешь, ");
        // Inside «беда»: the words go, the second half has no start.
        d.split(0, 13);
        assert_eq!(texts(&d), ["Знаешь, не бе", "да"]);
        assert!(d.lines.iter().all(|l| !l.worded()));
        assert_eq!(d.lines[1].start, None);
        // At either end nothing is cut.
        d.split(1, 0);
        d.split(1, 2);
        assert_eq!(d.lines.len(), 2);
    }

    #[test]
    fn merging_lines_without_words_or_with_an_empty_one() {
        let mut d = draft(vec![Line::new("Раз", None), Line::new("два", Some(ms(4_000))), Line::new("", None)]);
        d.merge(0);
        assert_eq!(texts(&d), ["Раз два", ""]);
        assert_eq!(d.lines[0].start, Some(ms(4_000)), "the next one's start when it had none");
        d.merge(0);
        assert_eq!(texts(&d), ["Раз два"]);
        d.merge(0);
        assert_eq!(d.lines.len(), 1);
    }

    /// «Отметить» times the selected line where the song is and moves on
    /// to the next, off after the last; without a selection it does
    /// nothing.
    #[test]
    fn marking_times_the_selected_line_and_moves_on() {
        let mut d = draft(vec![Line::new("a", None), Line::new("b", None), Line::new("c", None)]);
        assert!(!d.mark(ms(500)));
        d.select(0);
        assert!(d.mark(ms(1_000)));
        assert!(d.mark(ms(2_500)));
        assert_eq!(d.selected, Some(2));
        assert!(d.mark(ms(4_000)));
        assert_eq!(d.selected, None);
        assert!(!d.mark(ms(5_000)));
        assert_eq!(d.lines.iter().map(|l| l.start).collect::<Vec<_>>(), [Some(ms(1_000)), Some(ms(2_500)), Some(ms(4_000))]);
        assert_eq!(d.playing(ms(3_000)), Some(1));
        assert_eq!(d.playing(ms(100)), None);
        // Marked out of order: said so on that line.
        d.select(0);
        d.mark(ms(3_000));
        assert!(d.early(1) && !d.early(2) && !d.early(0));
        d.step(-1);
        assert_eq!(d.selected, Some(0));
        d.step(5);
        assert_eq!(d.selected, Some(2));
    }

    #[test]
    fn untimed_lines_are_saved_between_the_timed_ones() {
        let d = draft(vec![
            Line::new("a", None),
            Line::new("b", Some(ms(2_000))),
            Line::new("c", None),
            Line::new("d", Some(ms(4_000))),
            Line::new("e", None),
        ]);
        assert!(d.timed());
        let starts: Vec<Duration> = d.to_lines().iter().map(|l| l.start).collect();
        assert_eq!(starts, [ms(0), ms(2_000), ms(3_000), ms(4_000), ms(6_000)]);
        let plain = draft(vec![Line::new("a", None)]);
        assert!(!plain.timed());
    }

    #[test]
    fn a_text_opens_with_its_finest_timing_or_as_plain_lines() {
        let track = TrackId::new();
        let mut l = LyricsView { track: Some(track), source: Some(nikgob_view::LyricsSource::Genius), plain: Some("Раз\n\nДва".into()), ..Default::default() };
        let d = Draft::of(&l, "Песня · Артист".into()).unwrap();
        assert_eq!(texts(&d), ["Раз", "", "Два"]);
        assert!(!d.timed());
        // Seen by lines, while the player's words are there: the words.
        let mut by_lines = worded();
        by_lines.words.clear();
        l.lines = vec![by_lines];
        l.texts = vec![nikgob_view::LyricsText { source: l.source, made: Some(vec![worded()]), ..Default::default() }];
        let d = Draft::of(&l, String::new()).unwrap();
        assert!(d.lines[0].worded());
        assert_eq!(d.lines[0].start, Some(ms(1_000)));
        assert!(Draft::of(&LyricsView::default(), String::new()).is_none(), "no track");
    }
}
