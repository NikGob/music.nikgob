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

use std::ops::ControlFlow;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use nikgob_core::clip_title::same_dual_title;
use nikgob_core::model::{SourceId, TrackId};
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
pub const SAME_SONG: f64 = 0.6;
/// …when the file's length fits the track it is taken for and not the
/// other song's.
pub const SAME_SONG_AGAINST_LENGTH: f64 = 0.8;
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
    for s in &segments {
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
        if Some(k.track) == heard.track || same_title(heard.title, k.title) {
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
