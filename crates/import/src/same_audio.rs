//! A file that holds another song's audio (docs/TODO.md 32): «Пошлая Молли
//! — Сфотографирую» from Soulseek was «Тмстс» of the same album, and
//! nothing noticed.
//!
//! The rule ([`judge`]): a file sounds like another song when its
//! Chromaprint and the other file's cover the same music over most of the
//! longer of the two ([`overlap`], [`SAME_SONG`]), the lengths of the two
//! files agree, and the other song has another title (the same title is the
//! same song again: a re-encode, an upgrade). A file whose length fits the
//! track's tracklist length rather than the other file's needs more
//! ([`SAME_SONG_AGAINST_LENGTH`]). Silence and repeated frames count for
//! nothing ([`informative`]), and a print with too little music in it
//! (an intro, a skit) never matches anything ([`MIN_ITEMS`]).
//!
//! What follows from it: a file that sounds like a song of its album that
//! was in the library before is not imported as this song
//! ([`Judgement::Refuse`]), unless its length fits the tracklist's length
//! of the track it is taken for (then the library's file may be the wrong
//! one: it is asked about); one that sounds like a song that came with it,
//! or one of another album, is imported and asked about
//! ([`Judgement::Flag`]); the background check asks about every such pair
//! it finds.
//!
//! One song on two albums ([`same_recording`]): the same ISRC, recording
//! MBID or AcoustID, or near bit-exact audio under one title, on two
//! albums is one recording (linked, both tracks stay); the same audio under
//! two titles on two albums is asked about («Одна и та же песня?»); on one
//! album it is the rule above (a file holds another song).

use std::ops::ControlFlow;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use nikgob_core::clip_title::same_dual_title;
use nikgob_core::model::{ReleaseId, SourceId, TrackId};
use nikgob_library::WrongAudio;
use nikgob_metadata::text::title_key;
use rusty_chromaprint::{Configuration, Fingerprinter, match_fingerprints};

use crate::analysis::FINGERPRINT_SECS;
use crate::decode::{PcmSpec, decode_file};
use crate::error::ImportError;
use crate::quality::CheckError;
use crate::tags;

/// A print needs this many informative items (about 15 s of music) to be
/// compared at all.
pub const MIN_ITEMS: usize = 120;
/// Items equal for this many in a row are silence or a held frame.
const FLAT_RUN: usize = 3;
/// The share of the longer print two files must share to be one song.
pub const SAME_SONG: f64 = 0.8;
/// …when the file's length fits the track it is taken for and not the
/// other song's.
pub const SAME_SONG_AGAINST_LENGTH: f64 = 0.9;
/// A matching segment counts only when its items differ in this many bits
/// of 32 at most: one recording (a re-encode, the same file) stays well
/// under it, while two songs on one melody (a soundtrack's leitmotif) or two
/// versions of one song (another language over the same backing) reach the
/// matcher's own limit of 10 but not this.
const SAME_AUDIO_SCORE: f64 = 6.0;
/// A file marked as another song before is caught again from anywhere
/// above this.
pub const SAME_FILE_AGAIN: f64 = 0.85;
/// Two files of one song are this close in length at most…
const LENGTH_SLACK_MS: u32 = 3_000;
/// …or this share of the longer one.
const LENGTH_SHARE: f64 = 0.03;
/// A file's length "fits" a tracklist length within this.
const FIT_MS: u32 = 1_500;

/// Which items of a print carry music: not those in a run of
/// [`FLAT_RUN`] or more equal items (digital silence, a frozen frame).
pub fn informative(print: &[u32]) -> Vec<bool> {
    let mut out = vec![true; print.len()];
    let mut start = 0;
    while start < print.len() {
        let mut end = start + 1;
        while end < print.len() && print[end] == print[start] {
            end += 1;
        }
        if end - start >= FLAT_RUN {
            out[start..end].fill(false);
        }
        start = end;
    }
    out
}

/// How much of two prints is the same music: the informative items that
/// lie in matching segments, over the informative items of the longer one
/// (0..=1). A short piece inside a long song stays low; a print with less
/// than [`MIN_ITEMS`] informative items is 0.
pub fn overlap(a: &[u32], b: &[u32]) -> f64 {
    let (ia, ib) = (informative(a), informative(b));
    let (na, nb) = (ia.iter().filter(|x| **x).count(), ib.iter().filter(|x| **x).count());
    if na < MIN_ITEMS || nb < MIN_ITEMS {
        return 0.0;
    }
    let Ok(segments) = match_fingerprints(a, b, &Configuration::preset_test2()) else {
        return 0.0;
    };
    let mut covered = 0usize;
    for s in segments.iter().filter(|s| s.score <= SAME_AUDIO_SCORE) {
        for k in 0..s.items_count {
            if ia.get(s.offset1 + k) == Some(&true) && ib.get(s.offset2 + k) == Some(&true) {
                covered += 1;
            }
        }
    }
    (covered as f64 / na.max(nb) as f64).min(1.0)
}

/// Two titles of one song (the same folded title, or a dual title and one
/// of its halves).
pub fn same_title(a: &str, b: &str) -> bool {
    let (ka, kb) = (title_key(a), title_key(b));
    (!ka.base.is_empty() && ka.base == kb.base) || same_dual_title(a, b, |t| title_key(t).base)
}

/// Two titles of versions of one song: the same title once a version in
/// brackets or after a dash is left out («spiral (TV size)» and «spiral
/// (English Short Ver.)»). Not another song, whatever the files sound like.
pub fn versions_of_one_song(a: &str, b: &str) -> bool {
    fn main(t: &str) -> String {
        let cut = t.find(|c| matches!(c, '(' | '[' | '（' | '【')).unwrap_or(t.len());
        let t = &t[..cut];
        let t = t.split(" - ").next().unwrap_or(t);
        title_key(t.trim()).base
    }
    let (ma, mb) = (main(a), main(b));
    !ma.is_empty() && ma == mb
}

/// Two files as long as one recording.
fn lengths_close(a: Option<u32>, b: Option<u32>) -> bool {
    let (Some(a), Some(b)) = (a, b) else { return false };
    let slack = LENGTH_SLACK_MS.max((f64::from(a.max(b)) * LENGTH_SHARE) as u32);
    a.abs_diff(b) <= slack
}

fn fits(a: Option<u32>, b: Option<u32>) -> bool {
    matches!((a, b), (Some(a), Some(b)) if a.abs_diff(b) <= FIT_MS)
}

/// The file being judged.
#[derive(Clone, Copy, Debug)]
pub struct Heard<'a> {
    /// The title it is taken for (its track's, when it has one).
    pub title: &'a str,
    /// The library track it would become.
    pub track: Option<TrackId>,
    pub print: &'a [u32],
    pub duration_ms: Option<u32>,
    /// The length the tracklist gives that track.
    pub expected_ms: Option<u32>,
    /// The prints of the files that track has now.
    pub own: &'a [Vec<u32>],
}

/// A file of the library it is compared with.
#[derive(Clone, Copy, Debug)]
pub struct Known<'a> {
    pub source: SourceId,
    pub track: TrackId,
    pub title: &'a str,
    pub print: &'a [u32],
    pub duration_ms: Option<u32>,
    /// It was in the library before the file being judged came.
    pub settled: bool,
    /// It is on the album the judged file is for.
    pub same_release: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Judgement {
    /// Nothing says it is another song.
    Fine,
    /// It sounds like `track`'s file that came with it, or one of another
    /// album: imported, and the pair is asked about.
    Flag { source: SourceId, track: TrackId, similarity: f64 },
    /// It sounds like `track`, a song of its album the library had: not
    /// imported as this song.
    Refuse { source: SourceId, track: TrackId, similarity: f64 },
}

/// Whether `heard` holds another song's audio than the one it is taken for
/// (see the module docs). The closest other song decides.
pub fn judge(heard: &Heard<'_>, known: &[Known<'_>]) -> Judgement {
    let mine = informative(heard.print).iter().filter(|x| **x).count();
    if mine < MIN_ITEMS {
        return Judgement::Fine;
    }
    // It sounds like the track's own file: the same song again.
    if heard.own.iter().any(|own| overlap(heard.print, own) >= SAME_SONG) {
        return Judgement::Fine;
    }
    let mut best: Option<(f64, &Known<'_>)> = None;
    for k in known {
        if Some(k.track) == heard.track || same_title(heard.title, k.title) || versions_of_one_song(heard.title, k.title) {
            continue;
        }
        if !lengths_close(heard.duration_ms, k.duration_ms) {
            continue;
        }
        // The length says it is the track's own song: the music must say
        // otherwise more firmly.
        let need = if fits(heard.duration_ms, heard.expected_ms) && !fits(heard.duration_ms, k.duration_ms) {
            SAME_SONG_AGAINST_LENGTH
        } else {
            SAME_SONG
        };
        let sim = overlap(heard.print, k.print);
        if sim >= need && best.as_ref().is_none_or(|(s, _)| sim > *s) {
            best = Some((sim, k));
        }
    }
    match best {
        None => Judgement::Fine,
        // The tracklist's length of the track fits the file: the library's
        // file may be the wrong one, so the user decides.
        Some((similarity, k)) if k.settled && k.same_release && !fits(heard.duration_ms, heard.expected_ms) => {
            Judgement::Refuse { source: k.source, track: k.track, similarity }
        }
        Some((similarity, k)) => Judgement::Flag { source: k.source, track: k.track, similarity },
    }
}

/// Two copies of one recording on two albums are this close in length at
/// most.
pub const RECORDING_SLACK_MS: u32 = 2_000;

/// A track as [`same_recording`] sees it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Take<'a> {
    pub title: &'a str,
    pub release: Option<ReleaseId>,
    pub duration_ms: Option<u32>,
    pub isrc: Option<&'a str>,
    pub recording_mbid: Option<&'a str>,
    pub acoustid: Option<&'a str>,
    /// The stored print of its file, if it has one.
    pub print: Option<&'a [u32]>,
}

/// How two tracks were found to be one recording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    /// The same ISRC, recording MBID or AcoustID.
    Ids,
    /// Near bit-exact audio under one title.
    Audio,
}

impl How {
    /// The `how` of the stored link.
    pub fn key(self) -> &'static str {
        match self {
            Self::Ids => "ids",
            Self::Audio => "audio",
        }
    }
}

/// What two tracks are to each other.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Recording {
    /// One recording on two albums: linked, both stay.
    Same(How),
    /// The same audio under two titles on two albums: «Одна и та же песня?».
    Ask { similarity: f64 },
    /// On one album, a file holds the other song's audio (the old card).
    WrongFile { similarity: f64 },
    /// Nothing ties them.
    Apart,
}

/// The words in brackets and after « - » of a title, folded, a credit
/// («feat. X») left out: «spiral (TV size)» says `tvsize`, «spiral» nothing.
fn version_words(t: &str) -> Vec<String> {
    let lower = t.to_lowercase();
    let mut parts: Vec<String> = Vec::new();
    let mut depth = 0usize;
    let mut cur = String::new();
    for c in lower.chars() {
        match c {
            '(' | '[' | '（' | '【' => {
                if depth == 0 {
                    cur.clear();
                } else {
                    cur.push(c);
                }
                depth += 1;
            }
            ')' | ']' | '）' | '】' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    parts.push(std::mem::take(&mut cur));
                } else {
                    cur.push(c);
                }
            }
            _ if depth > 0 => cur.push(c),
            _ => {}
        }
    }
    if let Some((_, tail)) = lower.split_once(" - ") {
        let cut = tail.find(['(', '[', '（', '【']).unwrap_or(tail.len());
        parts.push(tail[..cut].to_owned());
    }
    let credit = |p: &str| ["feat", "ft.", "ft ", "with ", "prod"].iter().any(|w| p.starts_with(w));
    let mut out: Vec<String> = parts
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty() && !credit(p))
        .map(|p| p.chars().filter(|c| c.is_alphanumeric()).collect::<String>())
        .filter(|p| !p.is_empty())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Two titles of one song where one is a version of the other («spiral
/// (TV size)» and «spiral», «Song - Remastered» and «Song»): not the same
/// recording unless the audio says so. A title and its other script
/// («夏子 - Natsuko» and «夏子») is no version.
pub fn version_pair(a: &str, b: &str) -> bool {
    let brackets = |t: &str| t.contains(['(', '[', '（', '【']);
    if same_dual_title(a, b, |t| title_key(t).base) && !brackets(a) && !brackets(b) {
        return false;
    }
    (same_title(a, b) || versions_of_one_song(a, b)) && version_words(a) != version_words(b)
}

fn shared_id(a: &Take<'_>, b: &Take<'_>) -> bool {
    let fold = |s: &str| s.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_uppercase();
    let same = |x: Option<&str>, y: Option<&str>| match (x.map(fold), y.map(fold)) {
        (Some(x), Some(y)) => !x.is_empty() && x == y,
        _ => false,
    };
    same(a.isrc, b.isrc) || same(a.recording_mbid, b.recording_mbid) || same(a.acoustid, b.acoustid)
}

fn within(a: Option<u32>, b: Option<u32>, ms: u32) -> bool {
    matches!((a, b), (Some(a), Some(b)) if a.abs_diff(b) <= ms)
}

/// Whether `a` and `b` are one recording on two albums, a pair to ask
/// about, a file that holds another song, or nothing:
///
/// * one album (both on it): the same audio under two titles is a file
///   with another song's audio (as [`judge`] finds it); else nothing.
/// * two albums (or loose tracks): the same ISRC, recording MBID or
///   AcoustID and lengths within [`RECORDING_SLACK_MS`] is one recording
///   (a version of the title, «(TV size)», needs the audio to say so too);
///   near bit-exact audio ([`overlap`] ≥ [`SAME_SONG`]) within that length
///   under one title (or a version of it) is one recording; the same audio
///   under two titles is asked about.
pub fn same_recording(a: &Take<'_>, b: &Take<'_>) -> Recording {
    let sim = match (a.print, b.print) {
        (Some(x), Some(y)) => overlap(x, y),
        _ => 0.0,
    };
    let one_title = same_title(a.title, b.title) || versions_of_one_song(a.title, b.title);
    if a.release.is_some() && a.release == b.release {
        if sim >= SAME_SONG && !one_title && lengths_close(a.duration_ms, b.duration_ms) {
            return Recording::WrongFile { similarity: sim };
        }
        return Recording::Apart;
    }
    let close = within(a.duration_ms, b.duration_ms, RECORDING_SLACK_MS);
    let same_audio = sim >= SAME_SONG;
    if shared_id(a, b) && close && (same_audio || !version_pair(a.title, b.title)) {
        return Recording::Same(How::Ids);
    }
    if same_audio {
        if one_title {
            return if close { Recording::Same(How::Audio) } else { Recording::Apart };
        }
        if lengths_close(a.duration_ms, b.duration_ms) {
            return Recording::Ask { similarity: sim };
        }
    }
    Recording::Apart
}

/// What a file is called among the things not taken for a track again: its
/// hash as it came, and where it came from.
pub fn file_keys(came_hash: Option<&str>, came_from: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(h) = came_hash.map(str::trim).filter(|h| !h.is_empty()) {
        out.push(format!("hash:{h}"));
    }
    for f in came_from.iter().map(|f| f.trim()).filter(|f| !f.is_empty()) {
        let key = format!("from:{f}");
        if !out.contains(&key) {
            out.push(key);
        }
    }
    out
}

/// The row of `marked` (what is not taken for a track again) that this
/// file is: the same hash, the same place it came from, or the same audio.
pub fn marked_before<'a>(keys: &[String], print: Option<&[u32]>, marked: &'a [WrongAudio]) -> Option<&'a WrongAudio> {
    marked.iter().find(|m| keys.contains(&m.key)).or_else(|| {
        let print = print?;
        marked
            .iter()
            .find(|m| m.print.as_deref().is_some_and(|p| overlap(print, p) >= SAME_FILE_AGAIN))
    })
}

/// The Chromaprint of a file's first [`FINGERPRINT_SECS`] (only that much
/// is decoded) and the length of the whole file. `stop` aborts.
pub fn print_file(path: &Path, stop: &AtomicBool) -> Result<(Vec<u32>, Option<u32>), CheckError> {
    print_file_paced(path, stop, &mut || {})
}

/// [`print_file`] with `pace` called between the decoded chunks (it may
/// block: a background pass rests there).
pub fn print_file_paced(path: &Path, stop: &AtomicBool, pace: &mut dyn FnMut()) -> Result<(Vec<u32>, Option<u32>), CheckError> {
    if !crate::sanitize::long_path(path).exists() {
        return Err(CheckError::Unavailable(format!("{}: not found", path.display())));
    }
    let container = tags::probe_info(path).ok().map(|i| i.duration).filter(|d| !d.is_zero());
    struct State {
        printer: Fingerprinter,
        ok: bool,
        started: Option<PcmSpec>,
        left: u64,
        frames: u64,
        buf: Vec<i16>,
    }
    let mut st = State {
        printer: Fingerprinter::new(&Configuration::preset_test2()),
        ok: false,
        started: None,
        left: 0,
        frames: 0,
        buf: Vec::new(),
    };
    let mut sink = |chunk: &[f32], spec: PcmSpec| -> ControlFlow<()> {
        pace();
        if stop.load(Ordering::Relaxed) {
            return ControlFlow::Break(());
        }
        if st.started.is_none() {
            st.started = Some(spec);
            st.ok = st.printer.start(spec.sample_rate, spec.channels as u32).is_ok();
            st.left = u64::from(spec.sample_rate) * FINGERPRINT_SECS;
        }
        if !st.ok {
            return ControlFlow::Break(());
        }
        let ch = spec.channels.max(1);
        let frames = (chunk.len() / ch) as u64;
        let take = frames.min(st.left) as usize;
        st.buf.clear();
        st.buf
            .extend(chunk[..take * ch].iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16));
        st.printer.consume(&st.buf);
        st.left -= take as u64;
        st.frames += frames;
        if st.left == 0 { ControlFlow::Break(()) } else { ControlFlow::Continue(()) }
    };
    let decoded = decode_file(path, &mut sink);
    drop(sink);
    if stop.load(Ordering::Relaxed) {
        return Err(CheckError::Stopped);
    }
    let spec = match decoded {
        Ok(spec) => spec,
        Err(e @ ImportError::Io { .. }) => return Err(CheckError::Unavailable(e.to_string())),
        Err(e) => return Err(CheckError::Undecodable(e.to_string())),
    };
    if !st.ok {
        return Err(CheckError::Undecodable("no audio".into()));
    }
    st.printer.finish();
    let print = st.printer.fingerprint().to_vec();
    if print.is_empty() {
        return Err(CheckError::Undecodable("no audio".into()));
    }
    // Decoded to the end (shorter than the printed part): its own length.
    let whole = (st.left > 0 && spec.sample_rate > 0)
        .then(|| std::time::Duration::from_secs_f64(st.frames as f64 / f64::from(spec.sample_rate)));
    let duration = container.or(whole).map(|d| u32::try_from(d.as_millis()).unwrap_or(u32::MAX));
    Ok((print, duration))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spectrum::testsig::Rng;

    const RATE: u32 = 44_100;

    /// A few decaying notes with harmonics: a "song" of its own per seed.
    fn notes(secs: f64, seed: u64) -> Vec<f32> {
        let mut rng = Rng(seed);
        let n = (f64::from(RATE) * secs) as usize;
        let mut out = vec![0f32; n];
        let note = (RATE as usize) / 4;
        let mut start = 0;
        while start < n {
            let base = 110.0 * 2f64.powf(f64::from((rng.next_f32().abs() * 24.0) as i32) / 12.0);
            for i in 0..note.min(n - start) {
                let t = i as f64 / f64::from(RATE);
                let env = (-3.0 * t).exp() * (1.0 - (-200.0 * t).exp());
                let mut s = 0.0;
                for h in 1..=5 {
                    s += (2.0 * std::f64::consts::PI * base * f64::from(h) * t).sin() / f64::from(h);
                }
                out[start + i] += (s * env * 0.3) as f32;
            }
            start += note;
        }
        out
    }

    #[test]
    fn versions_of_one_song_are_not_another_song() {
        assert!(versions_of_one_song("spiral (English Short Ver.)", "spiral (TV size)"));
        assert!(versions_of_one_song("Song - Remastered", "Song"));
        assert!(!versions_of_one_song("Friends.", "Lost At A Sleepover"));
        assert!(!versions_of_one_song("Сфотографирую", "Тмстс"));
    }

    fn silence(secs: f64) -> Vec<f32> {
        vec![0.0; (f64::from(RATE) * secs) as usize]
    }

    fn print_of(samples: &[f32], rate: u32) -> Vec<u32> {
        let mut p = Fingerprinter::new(&Configuration::preset_test2());
        p.start(rate, 1).unwrap();
        let i16s: Vec<i16> = samples.iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect();
        p.consume(&i16s);
        p.finish();
        p.fingerprint().to_vec()
    }

    fn ms(samples: &[f32], rate: u32) -> Option<u32> {
        Some((samples.len() as f64 * 1000.0 / f64::from(rate)) as u32)
    }

    fn known<'a>(title: &'a str, print: &'a [u32], duration_ms: Option<u32>) -> Known<'a> {
        Known {
            source: SourceId::new(),
            track: TrackId::new(),
            title,
            print,
            duration_ms,
            settled: true,
            same_release: true,
        }
    }

    fn heard<'a>(title: &'a str, print: &'a [u32], duration_ms: Option<u32>) -> Heard<'a> {
        Heard { title, track: Some(TrackId::new()), print, duration_ms, expected_ms: None, own: &[] }
    }

    /// «Сфотографирую» that is «Тмстс» (the same audio, a bit quieter) is
    /// refused as the album's other song; one that came with it, or one of
    /// another album, is only asked about.
    #[test]
    fn the_same_audio_under_another_title_is_refused() {
        let song = notes(30.0, 11);
        let theirs = print_of(&song, RATE);
        let quieter: Vec<f32> = song.iter().map(|s| s * 0.8).collect();
        let mine = print_of(&quieter, RATE);
        let others_song = notes(30.0, 12);
        let other = print_of(&others_song, RATE);
        let len = ms(&song, RATE);
        assert!(overlap(&mine, &theirs) >= SAME_SONG, "{}", overlap(&mine, &theirs));
        assert!(overlap(&mine, &other) < 0.3, "{}", overlap(&mine, &other));

        let album = [known("Молли", &other, len), known("Тмстс", &theirs, len)];
        let tmsts = album[1].track;
        match judge(&heard("Сфотографирую", &mine, len), &album) {
            Judgement::Refuse { track, similarity, .. } => {
                assert_eq!(track, tmsts);
                assert!(similarity >= SAME_SONG);
            }
            j => panic!("not refused: {j:?}"),
        }
        // The other song came with this file (one download): asked about.
        let fresh = [Known { settled: false, ..album[1] }];
        assert!(matches!(judge(&heard("Сфотографирую", &mine, len), &fresh), Judgement::Flag { .. }));
        // The track's tracklist length fits the file: the album's file may
        // be the wrong one, so it is asked about.
        let fitting = Heard { expected_ms: len.map(|l| l + 900), ..heard("Сфотографирую", &mine, len) };
        assert!(matches!(judge(&fitting, &album), Judgement::Flag { .. }));
        // A length that is off (163 s for a file of 160.5 s) does not save it.
        let off = Heard { expected_ms: len.map(|l| l + 2_500), ..heard("Сфотографирую", &mine, len) };
        assert!(matches!(judge(&off, &album), Judgement::Refuse { .. }));
        // On another album of the artist: asked about too.
        let elsewhere = [Known { same_release: false, ..album[1] }];
        assert!(matches!(judge(&heard("Сфотографирую", &mine, len), &elsewhere), Judgement::Flag { .. }));
        // A file of another length is another recording.
        let longer = [known("Тмстс", &theirs, len.map(|l| l + 40_000))];
        assert_eq!(judge(&heard("Сфотографирую", &mine, len), &longer), Judgement::Fine);
        // Nothing near it sounds alike.
        assert_eq!(judge(&heard("Сфотографирую", &mine, len), &album[..1]), Judgement::Fine);
    }

    /// The same song again (another encode, another rate, a remaster's
    /// title) is an upgrade, never refused; nor is a file that sounds like
    /// the track's own file whatever else shares its audio.
    #[test]
    fn the_same_song_re_encoded_is_taken() {
        let song = notes(30.0, 21);
        let theirs = print_of(&song, RATE);
        let resampled: Vec<f32> = crate::resample::resample_all(&song, 1, RATE, 48_000).iter().map(|s| s * 0.7).collect();
        let mine = print_of(&resampled, 48_000);
        let len = ms(&song, RATE);
        let album = [known("Тмстс", &theirs, len)];
        assert_eq!(judge(&heard("Тмстс", &mine, len), &album), Judgement::Fine);
        assert_eq!(judge(&heard("Тмстс (Remastered 2024)", &mine, len), &album), Judgement::Fine);
        // Its own track's file sounds the same: the same song, even if a
        // wrong file elsewhere on the album holds it too.
        let own = vec![theirs.clone()];
        let h = Heard { own: &own, ..heard("Сфотографирую", &mine, len) };
        assert_eq!(judge(&h, &album), Judgement::Fine);
        // The track itself among the album's files is not "another song".
        let same_track = [Known { track: h.track.unwrap(), ..album[0] }];
        let h2 = Heard { track: Some(same_track[0].track), ..heard("Сфотографирую", &mine, len) };
        assert_eq!(judge(&h2, &same_track), Judgement::Fine);
        assert!(same_title("夏子 - Natsuko", "夏子"));
        assert!(!same_title("Тмстс", "Сфотографирую"));
    }

    /// A short intro, a shared intro, and silence match nothing.
    #[test]
    fn intros_and_silence_do_not_match_everything() {
        // An 8 s intro that is the start of a song.
        let song = notes(30.0, 31);
        let intro = &song[..(RATE as usize) * 8];
        let p_song = print_of(&song, RATE);
        let p_intro = print_of(intro, RATE);
        assert_eq!(overlap(&p_intro, &p_song), 0.0, "too little music to compare");
        assert_eq!(judge(&heard("Intro", &p_intro, ms(intro, RATE)), &[known("Song", &p_song, ms(intro, RATE))]), Judgement::Fine);

        // Two songs that open with the same 10 s and go apart.
        let opening = notes(10.0, 41);
        let a: Vec<f32> = opening.iter().copied().chain(notes(25.0, 42)).collect();
        let b: Vec<f32> = opening.iter().copied().chain(notes(25.0, 43)).collect();
        let (pa, pb) = (print_of(&a, RATE), print_of(&b, RATE));
        assert!(overlap(&pa, &pb) < SAME_SONG, "{}", overlap(&pa, &pb));
        assert_eq!(judge(&heard("A", &pa, ms(&a, RATE)), &[known("B", &pb, ms(&b, RATE))]), Judgement::Fine);

        // Long silence with a little music of each.
        let a: Vec<f32> = silence(25.0).into_iter().chain(notes(5.0, 51)).collect();
        let b: Vec<f32> = silence(25.0).into_iter().chain(notes(5.0, 52)).collect();
        let (pa, pb) = (print_of(&a, RATE), print_of(&b, RATE));
        assert_eq!(overlap(&pa, &pb), 0.0);
        assert_eq!(judge(&heard("A", &pa, ms(&a, RATE)), &[known("B", &pb, ms(&b, RATE))]), Judgement::Fine);
        // Silence alone.
        let quiet = print_of(&silence(30.0), RATE);
        assert!(informative(&quiet).iter().filter(|x| **x).count() < MIN_ITEMS);
        assert_eq!(overlap(&quiet, &quiet), 0.0);
    }

    fn take<'a>(title: &'a str, release: Option<ReleaseId>, ms: u32, print: Option<&'a [u32]>) -> Take<'a> {
        Take { title, release, duration_ms: Some(ms), print, ..Take::default() }
    }

    /// One song on two albums: the same ISRC is linked; the same audio
    /// under one title is linked; under two titles it is asked about; on
    /// one album the old rule (a file holds another song) stays.
    #[test]
    fn one_recording_on_two_albums_is_linked_or_asked_about() {
        let (ost, special) = (Some(ReleaseId::new()), Some(ReleaseId::new()));
        let song = notes(30.0, 71);
        let p = print_of(&song, RATE);
        let quieter: Vec<f32> = song.iter().map(|s| s * 0.8).collect();
        let q = print_of(&quieter, RATE);
        let other = print_of(&notes(30.0, 72), RATE);

        // The same ISRC (written another way) on two albums, as long.
        let a = Take { isrc: Some("JP-A12-3456789"), ..take("Oración", ost, 276_900, None) };
        let b = Take { isrc: Some("jpa123456789"), ..take("Oración", special, 277_600, None) };
        assert_eq!(same_recording(&a, &b), Recording::Same(How::Ids));
        // …but 20 s longer: another cut.
        let longer = Take { duration_ms: Some(297_000), ..b };
        assert_eq!(same_recording(&a, &longer), Recording::Apart);
        // The same ISRC on one album is a tagging slip, nothing more.
        assert_eq!(same_recording(&a, &Take { release: ost, ..b }), Recording::Apart);

        // Near bit-exact audio under one title (another script too).
        let a = take("Oración", ost, 276_900, Some(&p));
        let b = take("Oración", special, 277_600, Some(&q));
        assert_eq!(same_recording(&a, &b), Recording::Same(How::Audio));
        assert_eq!(same_recording(&take("夏子 - Natsuko", ost, 200_000, Some(&p)), &take("夏子", special, 200_500, Some(&q))), Recording::Same(How::Audio));
        // Other music under one title: nothing.
        assert_eq!(same_recording(&a, &take("Oración", special, 277_600, Some(&other))), Recording::Apart);

        // The same audio under two titles on two albums: asked about.
        let jp = take("オラシオン", special, 277_600, Some(&q));
        match same_recording(&a, &jp) {
            Recording::Ask { similarity } => assert!(similarity >= SAME_SONG),
            r => panic!("not asked: {r:?}"),
        }
        // Two loose tracks are on no one album: asked about too.
        assert!(matches!(same_recording(&take("Oración", None, 276_900, Some(&p)), &Take { release: None, ..jp }), Recording::Ask { .. }));
        // On one album: a file holds the other song (the old card).
        assert!(matches!(same_recording(&a, &Take { release: ost, ..jp }), Recording::WrongFile { .. }));
        assert_eq!(same_recording(&a, &Take { release: ost, ..b }), Recording::Apart, "one album's song twice is no other song");
    }

    /// A version («TV size») is linked only when the audio is near
    /// identical and the lengths agree within 2 s, whatever the ids say.
    #[test]
    fn a_version_is_linked_only_when_it_sounds_and_lasts_the_same() {
        let (ost, single) = (Some(ReleaseId::new()), Some(ReleaseId::new()));
        let song = notes(30.0, 81);
        let p = print_of(&song, RATE);
        let q = print_of(&song.iter().map(|s| s * 0.9).collect::<Vec<f32>>(), RATE);
        let other = print_of(&notes(30.0, 82), RATE);
        assert!(version_pair("spiral (TV size)", "spiral"));
        assert!(version_pair("Song - Remastered", "Song"));
        assert!(!version_pair("Song (feat. X)", "Song"));
        assert!(!version_pair("夏子 - Natsuko", "夏子"));
        assert!(!version_pair("Тмстс", "Сфотографирую"));

        // A TV size is shorter: never one recording.
        let full = take("spiral", ost, 240_000, Some(&p));
        let tv = take("spiral (TV size)", single, 89_000, Some(&q));
        assert_eq!(same_recording(&full, &tv), Recording::Apart);
        let tv_ids = Take { isrc: Some("JPX000000001"), ..tv };
        assert_eq!(same_recording(&Take { isrc: Some("JPX000000001"), ..full }, &tv_ids), Recording::Apart);
        // As long and near identical: one recording.
        let same_len = take("spiral (TV size)", single, 241_500, Some(&q));
        assert_eq!(same_recording(&full, &same_len), Recording::Same(How::Audio));
        // As long with the same ISRC but no audio to compare, or other
        // audio: not linked.
        let a = Take { isrc: Some("JPX000000001"), ..take("spiral", ost, 240_000, None) };
        let b = Take { isrc: Some("JPX000000001"), ..take("spiral (TV size)", single, 241_000, None) };
        assert_eq!(same_recording(&a, &b), Recording::Apart);
        let b_other = Take { print: Some(&other), ..b };
        assert_eq!(same_recording(&Take { print: Some(&p), ..a }, &b_other), Recording::Apart);
        // The same ISRC and as long, the audio near identical: linked.
        let b_same = Take { print: Some(&q), ..b };
        assert_eq!(same_recording(&Take { print: Some(&p), ..a }, &b_same), Recording::Same(How::Ids));
    }

    #[test]
    fn a_file_marked_before_is_known_by_hash_place_or_sound() {
        let song = notes(30.0, 61);
        let p = print_of(&song, RATE);
        let track = TrackId::new();
        let row = |key: &str, print: Option<Vec<u32>>| WrongAudio { track_id: track, key: key.into(), sounds_like: None, print, at: 1 };
        let marked = vec![row("hash:abc", Some(p.clone())), row("from:soulseek:user\\09.opus", None)];
        let keys = file_keys(Some("abc"), &[]);
        assert_eq!(keys, vec!["hash:abc".to_owned()]);
        assert!(marked_before(&keys, None, &marked).is_some());
        let keys = file_keys(Some("other"), &["soulseek:user\\09.opus".to_owned()]);
        assert_eq!(marked_before(&keys, None, &marked).map(|m| m.key.as_str()), Some("from:soulseek:user\\09.opus"));
        // Another hash from another place, but the same audio.
        let keys = file_keys(Some("zzz"), &["torrent:ffff".to_owned()]);
        assert!(marked_before(&keys, Some(&p), &marked).is_some());
        let other = print_of(&notes(30.0, 62), RATE);
        assert!(marked_before(&keys, Some(&other), &marked).is_none());
        assert!(file_keys(None, &[" ".to_owned()]).is_empty());
    }
}
