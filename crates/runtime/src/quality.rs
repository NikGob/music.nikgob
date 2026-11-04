//! The background spectrum check of the library's files: every local file
//! gets a verdict once (lossless-labelled files first), and again when the
//! file at its place changes. A "lossless" file found made from a lossy
//! one is ranked as what it really is, and with «Превращать «фейковый»
//! FLAC в MP3» on it becomes an MP3 (see `nikgob_import::quality`).
//!
//! One thread at the lowest priority, one file at a time with a rest
//! between files (a longer one while music plays). Files the import
//! checked on their way in, and Opus files made from a checked lossless
//! original, are not decoded again. Rows whose label changed go back to
//! the worker as [`Msg::Checked`].

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossbeam_channel::Sender;
use nikgob_core::model::{Source, SourceId, SourceOrigin, TrackId, quality_score, score};
use nikgob_core::quality::{QualityCheck, Verdict, fake_score};
use nikgob_import::quality::{CheckError, Mp3Encoder, check_file, convert_fake, file_stamp, is_fake};
use nikgob_library::{Library, QualityTodo};
use parking_lot::{Condvar, Mutex};

use crate::Worker;
use crate::import::Internal;

/// Rest between two files.
const REST: Duration = Duration::from_millis(500);
/// Rest between two files while music plays.
const REST_PLAYING: Duration = Duration::from_secs(4);
/// A shutdown waits this long for the file being checked.
const STOP_WAIT: Duration = Duration::from_secs(5);
/// Changed rows are reported at least this often during a pass.
const REPORT_EVERY: usize = 8;

/// From the check's thread to the worker.
pub(crate) enum Msg {
    /// These tracks were checked; `changed`: what their rows show changed
    /// (a fake found, a file converted), so the lists reload.
    Checked { tracks: Vec<TrackId>, changed: bool },
}

/// What the settings allow, set by the worker.
#[derive(Clone, Debug, Default)]
pub(crate) struct Settings {
    /// «Проверять «фейковый» FLAC».
    pub check: bool,
    /// «Превращать «фейковый» FLAC в MP3».
    pub convert: bool,
    /// The library's Trash, where a converted fake goes (no import service:
    /// nothing is converted).
    pub trash_dir: Option<PathBuf>,
}

#[derive(Default)]
struct Wake {
    kicked: bool,
}

struct Shared {
    lib: Library,
    root: PathBuf,
    tx: Sender<Internal>,
    stop: AtomicBool,
    playing: AtomicBool,
    settings: Mutex<Settings>,
    wake: Mutex<Wake>,
    cond: Condvar,
    /// Without work, the library is looked at again this often.
    idle: Duration,
}

/// What to do with one listed source.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Plan {
    Check,
    Skip,
}

/// Whether a listed source needs a check now; `now` is its file's stamp.
pub(crate) fn plan(t: &QualityTodo, now: Option<(u64, i64)>) -> Plan {
    match (&t.checked, &t.made_from) {
        // A lossy file ranked as lossless was made from a lossless
        // original that was judged on its way in; its own lowpass says
        // nothing about it.
        (None, _) if !t.codec.is_lossless() && t.quality_score >= score::CD_LOSSLESS => Plan::Skip,
        (None, _) => Plan::Check,
        // The check is of the original it was made from: it stays.
        (Some(_), Some(_)) => Plan::Skip,
        (Some(stamp), None) => match now {
            Some(now) if now != *stamp => Plan::Check,
            _ => Plan::Skip,
        },
    }
}

/// The score a checked source deserves, if it should change: a fake
/// lossless file scores as what it really is, an upsampled one as CD, a
/// lossless file found genuine (again) as lossless. Lossy files and audio
/// saved from a stream keep theirs.
pub(crate) fn rescore(src: &Source, c: &QualityCheck) -> Option<i32> {
    if !src.codec.is_lossless() || src.origin == SourceOrigin::Stream || c.made_from.is_some() {
        return None;
    }
    let want = match c.verdict {
        Verdict::Transcoded => fake_score(c.origin),
        Verdict::Upsampled => quality_score(&src.codec, src.bitrate, Some(44_100), Some(16), false),
        Verdict::Genuine => quality_score(&src.codec, src.bitrate, src.sample_rate, src.bit_depth, false),
        Verdict::Unknown => return None,
    };
    (want != src.quality_score).then_some(want)
}

impl Shared {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Sleeps `d` unless stopped or kicked meanwhile.
    fn pause(&self, d: Duration) {
        let mut w = self.wake.lock();
        if w.kicked || self.stopped() {
            return;
        }
        let _ = self.cond.wait_for(&mut w, d);
    }

    fn rest(&self) {
        let d = if self.playing.load(Ordering::Relaxed) { REST_PLAYING } else { REST };
        // A kick (an import) does not cut the rest short; a stop does.
        let until = std::time::Instant::now() + d;
        let mut w = self.wake.lock();
        while !self.stopped() {
            if self.cond.wait_until(&mut w, until).timed_out() {
                break;
            }
        }
    }

    fn report(&self, tracks: &mut Vec<TrackId>, changed: &mut bool) {
        if tracks.is_empty() {
            return;
        }
        let _ = self.tx.send(Internal::Quality(Msg::Checked { tracks: std::mem::take(tracks), changed: *changed }));
        *changed = false;
    }

    /// One look over the library; true when a file was checked.
    fn pass(&self, skip: &mut HashSet<SourceId>) -> bool {
        let todo = match self.lib.quality_todo() {
            Ok(t) => t,
            Err(e) => {
                log::warn!("quality: listing the files: {e}");
                return false;
            }
        };
        let (mut tracks, mut changed, mut worked) = (Vec::new(), false, false);
        for t in todo {
            if self.stopped() || !self.settings.lock().check {
                break;
            }
            if skip.contains(&t.source_id) {
                continue;
            }
            let path = file_path(&self.root, &t.locator);
            let now = file_stamp(&path);
            if plan(&t, now) == Plan::Skip {
                continue;
            }
            worked = true;
            match check_file(&path, &self.stop) {
                Ok(c) => {
                    changed |= self.store(&t, c);
                    tracks.push(t.track_id);
                }
                Err(CheckError::Stopped) => break,
                Err(CheckError::Unavailable(e)) => {
                    log::info!("quality: skipping {}: {e}", path.display());
                    skip.insert(t.source_id);
                }
                Err(CheckError::Undecodable(e)) => {
                    log::info!("quality: cannot check {}: {e}", path.display());
                    let (size, mtime) = now.unwrap_or_default();
                    if let Err(e) = self.lib.mark_quality_failed(t.source_id, size, mtime, &e) {
                        log::warn!("quality: marking {}: {e}", path.display());
                        skip.insert(t.source_id);
                    }
                }
            }
            if tracks.len() >= REPORT_EVERY {
                self.report(&mut tracks, &mut changed);
            }
            self.rest();
        }
        self.report(&mut tracks, &mut changed);
        worked
    }

    /// Stores a check and acts on it; true when what the track's rows show
    /// changed.
    fn store(&self, t: &QualityTodo, c: QualityCheck) -> bool {
        log::info!(
            "quality: {} is {:?}, cut at {:.0} Hz ({:.1} dB), origin {}",
            t.locator,
            c.verdict,
            c.cutoff_hz,
            c.steepness_db,
            c.origin
        );
        if let Err(e) = self.lib.put_quality_check(t.source_id, &c) {
            log::warn!("quality: storing {}: {e}", t.locator);
            return false;
        }
        let Some(src) = self.lib.get_source(t.source_id).ok().flatten() else { return false };
        let shown = nikgob_core::quality::label(&src.codec, src.bitrate, Some(&c.summary()))
            != nikgob_core::quality::label(&src.codec, src.bitrate, None);
        let mut changed = shown;
        if let Some(want) = rescore(&src, &c) {
            match self.lib.set_source_score(src.id, want) {
                Ok(moved) => changed |= moved,
                Err(e) => log::warn!("quality: ranking {}: {e}", t.locator),
            }
        }
        if is_fake(&src.codec, &c) {
            let (convert, trash) = {
                let s = self.settings.lock();
                (s.convert, s.trash_dir.clone())
            };
            if let (true, Some(trash)) = (convert, trash) {
                match Mp3Encoder::find() {
                    Some(enc) => {
                        // The score it was given just now goes along.
                        let src = self.lib.get_source(src.id).ok().flatten().unwrap_or(src);
                        match convert_fake(&self.lib, &trash, &enc, &src, &c) {
                            Ok(_) => changed = true,
                            Err(e) => log::warn!("quality: converting {}: {e}", t.locator),
                        }
                    }
                    None => log::info!("quality: no ffmpeg, {} stays as it is", t.locator),
                }
            }
        }
        changed
    }

    fn run(self: &Arc<Self>, start: Duration, alive: Sender<()>) {
        crate::analysis::lower_priority();
        self.pause(start);
        match self.lib.drop_orphan_checks() {
            Ok(0) => {}
            Ok(n) => log::info!("quality: {n} checks of sources that are gone dropped"),
            Err(e) => log::warn!("quality: dropping old checks: {e}"),
        }
        // Not tried again in this run (could not be opened).
        let mut skip = HashSet::new();
        while !self.stopped() {
            self.wake.lock().kicked = false;
            let worked = self.settings.lock().check && self.pass(&mut skip);
            if !worked {
                self.pause(self.idle);
            }
        }
        drop(alive);
    }
}

/// Absolute path of a source locator (relative ones are under the library).
fn file_path(root: &Path, locator: &str) -> PathBuf {
    let p = Path::new(locator);
    if p.is_absolute() { p.to_owned() } else { root.join(p) }
}

/// The background check.
pub(crate) struct QualityJob {
    tx: Sender<Internal>,
    settings: Settings,
    shared: Option<Arc<Shared>>,
    gone: Option<crossbeam_channel::Receiver<()>>,
}

impl QualityJob {
    pub(crate) fn new(tx: Sender<Internal>) -> Self {
        Self { tx, settings: Settings::default(), shared: None, gone: None }
    }

    /// Starts checking after `start`; `idle` is how often the library is
    /// looked at again when there is nothing to do.
    pub(crate) fn start(&mut self, lib: Library, root: PathBuf, settings: Settings, start: Duration, idle: Duration) {
        if self.shared.is_some() {
            return;
        }
        self.settings = settings.clone();
        let shared = Arc::new(Shared {
            lib,
            root,
            tx: self.tx.clone(),
            stop: AtomicBool::new(false),
            playing: AtomicBool::new(false),
            settings: Mutex::new(settings),
            wake: Mutex::new(Wake::default()),
            cond: Condvar::new(),
            idle,
        });
        let (alive, gone) = crossbeam_channel::bounded::<()>(0);
        let s = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("nikgob-quality".into())
            .spawn(move || s.run(start, alive));
        match spawned {
            Ok(_) => {
                self.shared = Some(shared);
                self.gone = Some(gone);
            }
            Err(e) => log::warn!("quality: cannot start: {e}"),
        }
    }

    /// New settings; turning the check on wakes it up.
    pub(crate) fn configure(&mut self, settings: Settings) {
        let on = settings.check && !self.settings.check;
        self.settings = settings.clone();
        if let Some(s) = &self.shared {
            *s.settings.lock() = settings;
            if on {
                s.wake.lock().kicked = true;
                s.cond.notify_all();
            }
        }
    }

    pub(crate) fn set_playing(&self, playing: bool) {
        if let Some(s) = &self.shared {
            s.playing.store(playing, Ordering::Relaxed);
        }
    }

    /// New files may be waiting (an import landed).
    pub(crate) fn kick(&self) {
        if let Some(s) = &self.shared {
            s.wake.lock().kicked = true;
            s.cond.notify_all();
        }
    }

    /// Stops the thread: a decode stops at its next chunk. Waits up to
    /// [`STOP_WAIT`].
    pub(crate) fn shutdown(&mut self) {
        let Some(s) = self.shared.take() else { return };
        s.stop.store(true, Ordering::Relaxed);
        {
            let _w = s.wake.lock();
            s.cond.notify_all();
        }
        if let Some(gone) = self.gone.take()
            && let Err(crossbeam_channel::RecvTimeoutError::Timeout) = gone.recv_timeout(STOP_WAIT)
        {
            log::warn!("quality: the check is still busy at shutdown");
        }
    }
}

impl Drop for QualityJob {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Worker {
    /// What the prefs and the import service say the check may do.
    pub(crate) fn quality_settings(&self) -> Settings {
        let (check, convert) = {
            let st = self.state.read();
            (st.prefs.check_fake_lossless, st.prefs.convert_fake_lossless)
        };
        Settings { check, convert, trash_dir: self.import.as_ref().map(|s| s.config().trash_dir.clone()) }
    }

    pub(crate) fn start_quality(&mut self) {
        let root = self.res.root().to_owned();
        let settings = self.quality_settings();
        let (start, idle) = (self.streaming.schedule.analysis_start, self.streaming.schedule.analysis_idle);
        self.quality.start(self.lib.clone(), root, settings, start, idle);
    }

    /// The prefs changed: the check follows.
    pub(crate) fn quality_prefs(&mut self) {
        let settings = self.quality_settings();
        self.quality.configure(settings);
    }

    pub(crate) fn quality_msg(&mut self, m: Msg) {
        match m {
            Msg::Checked { tracks, changed } => {
                if changed {
                    self.reload_after_import();
                }
                let current = self.playing.map(|e| e.track).or_else(|| self.queue.current().map(|e| e.track));
                if current.is_some_and(|c| tracks.contains(&c)) {
                    self.publish_now_playing();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use nikgob_core::model::{Codec, SourceKind};
    use nikgob_core::quality::SoundOrigin;

    use super::*;

    fn todo(codec: Codec, score: i32, checked: Option<(u64, i64)>, made_from: Option<Codec>) -> QualityTodo {
        QualityTodo {
            source_id: SourceId::new(),
            track_id: TrackId::new(),
            locator: "a".into(),
            codec,
            bitrate: None,
            quality_score: score,
            checked,
            made_from,
        }
    }

    #[test]
    fn files_are_checked_once_and_again_when_they_change() {
        let flac = todo(Codec::Flac, score::CD_LOSSLESS, None, None);
        assert_eq!(plan(&flac, Some((1, 1))), Plan::Check);
        let mp3 = todo(Codec::Mp3, score::MP3_320, None, None);
        assert_eq!(plan(&mp3, Some((1, 1))), Plan::Check);
        // An Opus made from a lossless original before checks were kept.
        let opus = todo(Codec::Opus, score::CD_LOSSLESS, None, None);
        assert_eq!(plan(&opus, Some((1, 1))), Plan::Skip);
        let same = todo(Codec::Flac, score::CD_LOSSLESS, Some((10, 20)), None);
        assert_eq!(plan(&same, Some((10, 20))), Plan::Skip);
        assert_eq!(plan(&same, Some((11, 20))), Plan::Check);
        assert_eq!(plan(&same, Some((10, 21))), Plan::Check);
        // Not there right now: nothing to compare.
        assert_eq!(plan(&same, None), Plan::Skip);
        // A check of the original the file was made from stays.
        let made = todo(Codec::Mp3, score::FAKE_LOSSLESS, Some((10, 20)), Some(Codec::Flac));
        assert_eq!(plan(&made, Some((99, 99))), Plan::Skip);
    }

    fn check(verdict: Verdict, cutoff: f32) -> QualityCheck {
        QualityCheck {
            verdict,
            cutoff_hz: cutoff,
            steepness_db: 40.0,
            origin: SoundOrigin::estimate(verdict, cutoff),
            sample_rate: 44_100,
            spectrum_db: Vec::new(),
            made_from: None,
            file_size: 1,
            file_mtime: 1,
            checked_at: 1,
        }
    }

    #[test]
    fn a_checked_file_is_ranked_as_what_it_really_is() {
        let mut flac = Source::new(TrackId::new(), SourceKind::LocalFile, "a.flac", Codec::Flac);
        flac.sample_rate = Some(44_100);
        flac.bit_depth = Some(16);
        flac.quality_score = score::CD_LOSSLESS;
        let fake = rescore(&flac, &check(Verdict::Transcoded, 16_000.0)).unwrap();
        assert!(fake < score::MP3_V0);
        assert_eq!(rescore(&flac, &check(Verdict::Genuine, 22_050.0)), None);
        // Found genuine after it was taken for a fake (the file changed).
        flac.quality_score = fake;
        assert_eq!(rescore(&flac, &check(Verdict::Genuine, 22_050.0)), Some(score::CD_LOSSLESS));
        // Upsampled "Hi-Res" is CD.
        let mut hires = flac.clone();
        hires.sample_rate = Some(96_000);
        hires.bit_depth = Some(24);
        hires.quality_score = score::HI_RES_LOSSLESS;
        assert_eq!(rescore(&hires, &check(Verdict::Upsampled, 22_000.0)), Some(score::CD_LOSSLESS));
        // Lossy files and saved streams keep their rank.
        let mp3 = Source::new(TrackId::new(), SourceKind::LocalFile, "a.mp3", Codec::Mp3);
        assert_eq!(rescore(&mp3, &check(Verdict::Transcoded, 16_000.0)), None);
        let mut saved = flac.clone();
        saved.origin = SourceOrigin::Stream;
        assert_eq!(rescore(&saved, &check(Verdict::Transcoded, 16_000.0)), None);
        assert_eq!(rescore(&flac, &check(Verdict::Unknown, 0.0)), None);
    }
}
