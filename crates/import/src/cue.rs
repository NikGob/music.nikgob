//! Cue sheets: the index card that ships with a single-file rip of a disc
//! (`Album.flac` + `Album.cue`). This module only reads them: the text in
//! whatever encoding it came, the sheet's fields, and where each track
//! starts in samples. Cutting the audio lives in [`crate::cue_import`].
//!
//! * Times are `mm:ss:ff` with 75 frames to the second (minutes may go
//!   past 99).
//! * A track starts at its `INDEX 01`; the pregap before it (`INDEX 00`)
//!   stays at the end of the track before, as a CD player plays it.
//! * A track belongs to the `FILE` its `INDEX 01` is in, so a sheet that
//!   puts a pregap in the file before still places the track right.

use std::fmt;
use std::ops::Range;
use std::path::Path;

/// CD frames to the second.
pub const FRAMES_PER_SECOND: u64 = 75;

/// What a sheet says about the disc and its files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CueSheet {
    /// The disc's `TITLE`: the album.
    pub title: Option<String>,
    /// The disc's `PERFORMER`: the album artist.
    pub performer: Option<String>,
    /// `REM DATE` as written (`1988`, `1988-04-01`).
    pub date: Option<String>,
    /// `REM GENRE`.
    pub genre: Option<String>,
    /// `REM DISCNUMBER`.
    pub disc_no: Option<u32>,
    /// `REM TOTALDISCS`.
    pub disc_total: Option<u32>,
    pub files: Vec<CueFile>,
    /// The text encoding the sheet was read in (`UTF-8`, `windows-1251`…).
    pub encoding: &'static str,
}

/// One `FILE` and the audio tracks that start in it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CueFile {
    /// The name as the sheet writes it (relative to the sheet's folder).
    pub name: String,
    pub tracks: Vec<CueTrack>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CueTrack {
    pub number: u32,
    pub title: Option<String>,
    pub performer: Option<String>,
    pub isrc: Option<String>,
    /// `INDEX 00` in frames, when the track has a pregap in the file.
    pub pregap: Option<u32>,
    /// `INDEX 01` in frames from the start of its file.
    pub start: u32,
}

impl CueSheet {
    /// Every audio track of the sheet, in order.
    pub fn tracks(&self) -> impl Iterator<Item = &CueTrack> {
        self.files.iter().flat_map(|f| f.tracks.iter())
    }

    pub fn track_count(&self) -> usize {
        self.files.iter().map(|f| f.tracks.len()).sum()
    }
}

/// Why a sheet cannot be read or does not fit its audio.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CueError {
    /// No `FILE` with an audio track in it.
    NoTracks,
    /// A `TRACK` before any `FILE`.
    TrackOutsideFile(u32),
    /// A track without `INDEX 01`.
    NoStart(u32),
    /// A time that is not `mm:ss:ff`.
    BadTime(String),
    /// Tracks of one file that do not start one after another.
    OutOfOrder { file: String, track: u32 },
    /// A track starts at or past the end of its file.
    PastEnd { file: String, track: u32, start_s: f64, length_s: f64 },
}

impl fmt::Display for CueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoTracks => f.write_str("no audio track in the sheet"),
            Self::TrackOutsideFile(n) => write!(f, "track {n} comes before any FILE"),
            Self::NoStart(n) => write!(f, "track {n} has no INDEX 01"),
            Self::BadTime(t) => write!(f, "bad time «{t}»"),
            Self::OutOfOrder { file, track } => write!(f, "track {track} of «{file}» starts before the track before it"),
            Self::PastEnd {
                file,
                track,
                start_s,
                length_s,
            } => write!(f, "track {track} starts at {start_s:.1} s, past the end of «{file}» ({length_s:.1} s)"),
        }
    }
}

impl std::error::Error for CueError {}

/// Reads a sheet from disk: [`decode_text`] then [`parse`].
pub fn read(path: &Path) -> std::io::Result<Result<CueSheet, CueError>> {
    let bytes = std::fs::read(crate::sanitize::long_path(path))?;
    Ok(parse_bytes(&bytes))
}

/// [`decode_text`] then [`parse`].
pub fn parse_bytes(bytes: &[u8]) -> Result<CueSheet, CueError> {
    let (text, encoding) = decode_text(bytes);
    let mut sheet = parse(&text)?;
    sheet.encoding = encoding;
    Ok(sheet)
}

/// The text of a sheet and the encoding it was in: a byte order mark
/// (UTF-8, UTF-16) says it; valid UTF-8 is UTF-8; anything else is
/// guessed among the legacy code pages (windows-1251, Shift_JIS,
/// windows-1252…) by how natural the text reads in each.
pub fn decode_text(bytes: &[u8]) -> (String, &'static str) {
    if let Some((enc, bom)) = encoding_rs::Encoding::for_bom(bytes) {
        let (text, _) = enc.decode_without_bom_handling(&bytes[bom..]);
        return (text.into_owned(), enc.name());
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return (s.to_owned(), encoding_rs::UTF_8.name());
    }
    let mut det = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    det.feed(bytes, true);
    let enc = det.guess(None, chardetng::Utf8Detection::Deny);
    let (text, _, _) = enc.decode(bytes);
    (text.into_owned(), enc.name())
}

/// The quoted (or bare) argument after a keyword: `TITLE "A B"` and
/// `TITLE A B` are both `A B`.
fn argument(rest: &str) -> String {
    let rest = rest.trim();
    match (rest.find('"'), rest.rfind('"')) {
        (Some(a), Some(b)) if b > a => rest[a + 1..b].to_owned(),
        // An opening quote that never closes: the rest of the line.
        (Some(a), _) => rest[a + 1..].trim().to_owned(),
        _ => rest.to_owned(),
    }
}

fn text_field(rest: &str) -> Option<String> {
    let s = argument(rest);
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_owned())
}

/// `mm:ss:ff` in frames.
pub fn parse_time(s: &str) -> Result<u32, CueError> {
    let bad = || CueError::BadTime(s.to_owned());
    let mut parts = s.trim().split(':');
    let (Some(m), Some(sec), Some(fr), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(bad());
    };
    let num = |p: &str| p.trim().parse::<u32>().map_err(|_| bad());
    let (m, sec, fr) = (num(m)?, num(sec)?, num(fr)?);
    if sec >= 60 || u64::from(fr) >= FRAMES_PER_SECOND {
        return Err(bad());
    }
    m.checked_mul(60)
        .and_then(|x| x.checked_add(sec))
        .and_then(|x| x.checked_mul(FRAMES_PER_SECOND as u32))
        .and_then(|x| x.checked_add(fr))
        .ok_or_else(bad)
}

/// A track being read: its fields and the file its `INDEX 01` fell in.
#[derive(Default)]
struct Open {
    track: CueTrack,
    audio: bool,
    file: Option<usize>,
    started: bool,
}

/// Parses the text of a sheet. Lines it does not know (`CATALOG`,
/// `SONGWRITER`, `FLAGS`, `PREGAP`…) are skipped; data tracks are left
/// out; a file without audio tracks is dropped.
pub fn parse(text: &str) -> Result<CueSheet, CueError> {
    let mut sheet = CueSheet::default();
    let mut open: Option<Open> = None;
    let close = |open: Option<Open>, sheet: &mut CueSheet| -> Result<(), CueError> {
        let Some(o) = open else { return Ok(()) };
        if !o.audio {
            return Ok(());
        }
        match (o.started, o.file) {
            (true, Some(f)) => {
                sheet.files[f].tracks.push(o.track);
                Ok(())
            }
            _ => Err(CueError::NoStart(o.track.number)),
        }
    };
    for raw in text.lines() {
        let line = raw.trim().trim_start_matches('\u{feff}');
        if line.is_empty() {
            continue;
        }
        let (word, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let word = word.to_ascii_uppercase();
        let rest = rest.trim();
        match word.as_str() {
            "FILE" => {
                // `FILE "name with spaces.wav" WAVE`: the type is the last word.
                let name = if rest.contains('"') {
                    argument(rest)
                } else {
                    rest.rsplit_once(char::is_whitespace).map_or(rest, |(n, _)| n).trim().to_owned()
                };
                sheet.files.push(CueFile {
                    name,
                    tracks: Vec::new(),
                });
            }
            "TRACK" => {
                close(open.take(), &mut sheet)?;
                let mut words = rest.split_whitespace();
                let number = words.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                let audio = words.next().is_none_or(|t| t.eq_ignore_ascii_case("AUDIO"));
                if sheet.files.is_empty() {
                    return Err(CueError::TrackOutsideFile(number));
                }
                open = Some(Open {
                    track: CueTrack {
                        number,
                        ..CueTrack::default()
                    },
                    audio,
                    ..Open::default()
                });
            }
            "INDEX" => {
                let Some(o) = open.as_mut() else { continue };
                let mut words = rest.split_whitespace();
                let (Some(n), Some(t)) = (words.next(), words.next()) else { continue };
                let frames = parse_time(t)?;
                match n.parse::<u32>() {
                    Ok(0) => o.track.pregap = Some(frames),
                    Ok(1) => {
                        o.track.start = frames;
                        o.started = true;
                        o.file = Some(sheet.files.len() - 1);
                    }
                    _ => {}
                }
            }
            "TITLE" | "PERFORMER" | "ISRC" => {
                let value = text_field(rest);
                match (open.as_mut(), word.as_str()) {
                    (Some(o), "TITLE") => o.track.title = value,
                    (Some(o), "PERFORMER") => o.track.performer = value,
                    (Some(o), _) => o.track.isrc = value,
                    (None, "TITLE") => sheet.title = value,
                    (None, "PERFORMER") => sheet.performer = value,
                    (None, _) => {}
                }
            }
            "REM" => {
                let (key, value) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                let value = text_field(value);
                match key.to_ascii_uppercase().as_str() {
                    "DATE" => sheet.date = sheet.date.take().or(value),
                    "GENRE" => sheet.genre = sheet.genre.take().or(value),
                    "DISCNUMBER" => sheet.disc_no = value.as_deref().and_then(leading_number),
                    "TOTALDISCS" => sheet.disc_total = value.as_deref().and_then(leading_number),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    close(open.take(), &mut sheet)?;
    sheet.files.retain(|f| !f.tracks.is_empty());
    if sheet.files.is_empty() {
        return Err(CueError::NoTracks);
    }
    Ok(sheet)
}

/// `2` of `2`, `2/3`, `02`.
fn leading_number(s: &str) -> Option<u32> {
    let digits: String = s.trim().chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok().filter(|&n| n > 0)
}

/// The first sample of a time in frames at `rate`, to the nearest sample
/// (exact at every rate a multiple of 75: 44.1, 48, 88.2, 96, 192 kHz).
pub fn frames_to_samples(frames: u32, rate: u32) -> u64 {
    (u64::from(frames) * u64::from(rate) + FRAMES_PER_SECOND / 2) / FRAMES_PER_SECOND
}

/// Where each track of `file` lies in samples at `rate`: from its
/// `INDEX 01` to the next track's (the last to `total`, the file's
/// length in samples, or to the end when it is not known). Tracks must
/// start one after another and before the end.
pub fn spans(file: &CueFile, rate: u32, total: Option<u64>) -> Result<Vec<Range<u64>>, CueError> {
    let starts: Vec<u64> = file.tracks.iter().map(|t| frames_to_samples(t.start, rate)).collect();
    for (i, t) in file.tracks.iter().enumerate() {
        if i > 0 && starts[i] <= starts[i - 1] {
            return Err(CueError::OutOfOrder {
                file: file.name.clone(),
                track: t.number,
            });
        }
        if let Some(total) = total
            && starts[i] >= total
        {
            return Err(CueError::PastEnd {
                file: file.name.clone(),
                track: t.number,
                start_s: starts[i] as f64 / f64::from(rate.max(1)),
                length_s: total as f64 / f64::from(rate.max(1)),
            });
        }
    }
    let end = total.unwrap_or(u64::MAX);
    Ok((0..starts.len())
        .map(|i| starts[i]..starts.get(i + 1).copied().unwrap_or(end))
        .collect())
}

/// The disc a name says it is: `CD2`, `Disc 2`, `disk_2`, `Диск 2`,
/// `(CD 2)`. A word must not glue to letters before it.
pub fn disc_hint(name: &str) -> Option<u32> {
    let lower = name.to_lowercase();
    for prefix in ["cd", "disc", "disk", "диск"] {
        for (i, _) in lower.match_indices(prefix) {
            if lower[..i].chars().next_back().is_some_and(char::is_alphabetic) {
                continue;
            }
            let rest = lower[i + prefix.len()..].trim_start_matches([' ', '_', '-', '.', '#']);
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            if let Ok(n) = digits.parse::<u32>()
                && (1..100).contains(&n)
            {
                return Some(n);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUSSIAN: &str = "REM GENRE Rock\r\n\
        REM DATE 1988\r\n\
        REM DISCID 8A0B4C09\r\n\
        REM COMMENT \"ExactAudioCopy v1.6\"\r\n\
        PERFORMER \"Кино\"\r\n\
        TITLE \"Группа крови\"\r\n\
        FILE \"Кино - Группа крови.wav\" WAVE\r\n\
        \x20 TRACK 01 AUDIO\r\n\
        \x20   TITLE \"Группа крови\"\r\n\
        \x20   PERFORMER \"Кино\"\r\n\
        \x20   INDEX 01 00:00:00\r\n\
        \x20 TRACK 02 AUDIO\r\n\
        \x20   TITLE \"Закрой за мной дверь, я ухожу\"\r\n\
        \x20   PERFORMER \"Кино\"\r\n\
        \x20   INDEX 00 04:44:30\r\n\
        \x20   INDEX 01 04:46:00\r\n\
        \x20 TRACK 03 AUDIO\r\n\
        \x20   TITLE \"Война\"\r\n\
        \x20   INDEX 01 09:01:72\r\n";

    const JAPANESE: &str = "PERFORMER \"宇多田ヒカル\"\n\
        TITLE \"ファースト・ラヴ\"\n\
        REM DATE 1999\n\
        FILE \"宇多田ヒカル - ファースト・ラヴ.flac\" WAVE\n\
        TRACK 01 AUDIO\n\
        TITLE \"オートマティック\"\n\
        INDEX 01 00:00:00\n\
        TRACK 02 AUDIO\n\
        TITLE \"ムーヴィン・オン・ウィズアウト・ユー\"\n\
        INDEX 01 05:06:12\n";

    #[test]
    fn a_sheet_gives_its_disc_fields_and_tracks() {
        let s = parse(RUSSIAN).unwrap();
        assert_eq!(s.title.as_deref(), Some("Группа крови"));
        assert_eq!(s.performer.as_deref(), Some("Кино"));
        assert_eq!(s.date.as_deref(), Some("1988"));
        assert_eq!(s.genre.as_deref(), Some("Rock"));
        assert_eq!(s.files.len(), 1);
        assert_eq!(s.files[0].name, "Кино - Группа крови.wav");
        let t = &s.files[0].tracks;
        assert_eq!(t.len(), 3);
        assert_eq!((t[1].number, t[1].title.as_deref()), (2, Some("Закрой за мной дверь, я ухожу")));
        assert_eq!(t[2].performer, None, "a track without PERFORMER takes the disc's later");
        // The pregap is read but the track starts at INDEX 01.
        assert_eq!(t[1].pregap, Some((4 * 60 + 44) * 75 + 30));
        assert_eq!(t[1].start, (4 * 60 + 46) * 75);
        assert_eq!(t[2].start, (9 * 60 + 1) * 75 + 72);
    }

    #[test]
    fn each_encoding_is_read_back() {
        let check = |bytes: Vec<u8>, want: &str, title: &str| {
            let s = parse_bytes(&bytes).unwrap();
            assert_eq!(s.encoding, want);
            assert_eq!(s.files[0].tracks[0].title.as_deref(), Some(title));
        };
        // UTF-8 with and without a byte order mark.
        check(RUSSIAN.as_bytes().to_vec(), "UTF-8", "Группа крови");
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice(RUSSIAN.as_bytes());
        check(bom, "UTF-8", "Группа крови");
        // UTF-16 LE with its mark (foobar2000 writes these).
        let mut utf16 = vec![0xFF, 0xFE];
        utf16.extend(RUSSIAN.encode_utf16().flat_map(u16::to_le_bytes));
        check(utf16, "UTF-16LE", "Группа крови");
        // The legacy code pages of the trackers.
        let (cp1251, _, bad) = encoding_rs::WINDOWS_1251.encode(RUSSIAN);
        assert!(!bad);
        check(cp1251.into_owned(), "windows-1251", "Группа крови");
        let (sjis, _, bad) = encoding_rs::SHIFT_JIS.encode(JAPANESE);
        assert!(!bad);
        check(sjis.into_owned(), "Shift_JIS", "オートマティック");
        let latin = "PERFORMER \"Beyoncé\"\nTITLE \"Déjà vu\"\nFILE \"a.wav\" WAVE\nTRACK 01 AUDIO\nTITLE \"Café del Mar, été\"\nINDEX 01 00:00:00\n";
        let (cp1252, _, bad) = encoding_rs::WINDOWS_1252.encode(latin);
        assert!(!bad);
        check(cp1252.into_owned(), "windows-1252", "Café del Mar, été");
    }

    #[test]
    fn several_files_and_a_pregap_in_the_file_before() {
        // A gap-split sheet: track 2's pregap is the end of file 1, its
        // INDEX 01 the start of file 2; a data track is left out.
        let text = "TITLE \"Live\"\n\
            FILE \"CD1-1.flac\" WAVE\n\
            TRACK 01 AUDIO\n INDEX 01 00:00:00\n\
            TRACK 02 AUDIO\n INDEX 01 03:00:00\n\
            TRACK 03 AUDIO\n TITLE \"Gap\"\n INDEX 00 06:00:00\n\
            FILE \"CD1-2.flac\" WAVE\n INDEX 01 00:00:00\n\
            TRACK 04 AUDIO\n INDEX 01 04:00:00\n\
            TRACK 05 MODE1/2352\n INDEX 01 08:00:00\n";
        let s = parse(text).unwrap();
        assert_eq!(s.files.len(), 2);
        let n = |f: usize| s.files[f].tracks.iter().map(|t| t.number).collect::<Vec<_>>();
        assert_eq!(n(0), vec![1, 2]);
        assert_eq!(n(1), vec![3, 4]);
        assert_eq!(s.files[1].tracks[0].pregap, Some(6 * 60 * 75));
        assert_eq!(s.files[1].tracks[0].title.as_deref(), Some("Gap"));
        assert_eq!(s.track_count(), 4);
    }

    #[test]
    fn broken_sheets_say_why() {
        assert_eq!(parse("TITLE \"x\"\n"), Err(CueError::NoTracks));
        assert_eq!(parse("TRACK 01 AUDIO\nINDEX 01 00:00:00\n"), Err(CueError::TrackOutsideFile(1)));
        assert_eq!(
            parse("FILE \"a.wav\" WAVE\nTRACK 01 AUDIO\nINDEX 00 00:00:00\n"),
            Err(CueError::NoStart(1))
        );
        assert!(matches!(parse("FILE a.wav WAVE\nTRACK 01 AUDIO\nINDEX 01 00:61:00\n"), Err(CueError::BadTime(_))));
        // A bare file name with spaces.
        let s = parse("FILE my disc.wav WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\n").unwrap();
        assert_eq!(s.files[0].name, "my disc.wav");
    }

    #[test]
    fn times_and_samples() {
        assert_eq!(parse_time("00:00:01"), Ok(1));
        assert_eq!(parse_time("01:02:03"), Ok((60 + 2) * 75 + 3));
        assert_eq!(parse_time("120:00:00"), Ok(120 * 60 * 75), "minutes go past 99");
        assert!(parse_time("00:00:75").is_err());
        assert!(parse_time("1:2").is_err());
        // One frame is 588 samples at 44.1 kHz, 640 at 48 kHz.
        assert_eq!(frames_to_samples(1, 44_100), 588);
        assert_eq!(frames_to_samples(75, 44_100), 44_100);
        assert_eq!(frames_to_samples(1, 48_000), 640);
        assert_eq!(frames_to_samples(3, 96_000), 3840);
        assert_eq!(frames_to_samples(1, 22_050), 294);
    }

    #[test]
    fn spans_run_from_index_01_to_the_next() {
        let s = parse(RUSSIAN).unwrap();
        let f = &s.files[0];
        let total = 12 * 60 * 44_100;
        let sp = spans(f, 44_100, Some(total)).unwrap();
        assert_eq!(sp.len(), 3);
        assert_eq!(sp[0], 0..frames_to_samples(f.tracks[1].start, 44_100));
        // The pregap of track 2 (04:44:30..04:46:00) ends track 1.
        assert_eq!(sp[0].end, 286 * 44_100);
        assert_eq!(sp[1].start, sp[0].end);
        assert_eq!(sp[2], frames_to_samples(f.tracks[2].start, 44_100)..total);
        // Back to back: no sample lost or doubled.
        assert!(sp.windows(2).all(|w| w[0].end == w[1].start));
        // Without a known length the last runs to the end.
        assert_eq!(spans(f, 44_100, None).unwrap()[2].end, u64::MAX);
        // A track past the end of the file.
        let short = 5 * 60 * 44_100;
        assert!(matches!(spans(f, 44_100, Some(short)), Err(CueError::PastEnd { track: 3, .. })));
        // Out of order.
        let mut bad = f.clone();
        bad.tracks[2].start = 10;
        assert!(matches!(spans(&bad, 44_100, None), Err(CueError::OutOfOrder { track: 3, .. })));
    }

    #[test]
    fn discs_from_names() {
        assert_eq!(disc_hint("CD2"), Some(2));
        assert_eq!(disc_hint("Album (Disc 1).cue"), Some(1));
        assert_eq!(disc_hint("disk_3"), Some(3));
        assert_eq!(disc_hint("Диск 2"), Some(2));
        assert_eq!(disc_hint("cd-02"), Some(2));
        assert_eq!(disc_hint("ABCD 2"), None, "glued to a word");
        assert_eq!(disc_hint("Album 1988"), None);
        assert_eq!(disc_hint("Discography"), None);
    }
}
