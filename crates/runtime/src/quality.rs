//! The background spectrum check of the library's lossless files: each
//! gets a verdict once, and again when the file at its place changes (a
//! lossy file's own lowpass is its format's: it is never judged). A
//! "lossless" file found made from a lossy one is ranked as what it really
//! is and labelled so. It never changes by itself: a plain fake is offered
//! in «Подтверди» as an MP3, one card per album with a row per song; the
//! user ticks songs and «Превратить 3 в MP3» / «Оставить 3 FLAC» act on
//! those only. Only with «Без вопроса» on is a plain one converted at
//! once. A converted file's FLAC waits in the Trash, and «Вернуть FLAC»
//! brings every one back (see `nikgob_import::quality`).
//!
//! One thread at the lowest priority, paced by [`crate::quiet`] (a rest
//! five times its work, none while a game runs), one file at a time with a rest
//! between files (a longer one while music plays). Files the import
//! checked on their way in, and Opus files made from a checked lossless
//! original, are not decoded again. Rows whose label changed go back to
//! the worker as [`Msg::Checked`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossbeam_channel::Sender;
use nikgob_core::i18n::activity::fmt_quoted_in;
use nikgob_core::i18n::{Locale, activity_in, plural_forms};
use nikgob_core::model::{Source, SourceId, SourceOrigin, TrackId, quality_score, score};
use nikgob_core::quality::{QualityCheck, Verdict, clear_fake, fake_score};
use nikgob_import::quality::{CheckError, Mp3Encoder, check_file_paced, convert_fake, file_stamp, is_fake, restore_conversion};
use nikgob_library::{FakeOffer, Library, QualityTodo};
use nikgob_view as v;
use parking_lot::{Condvar, Mutex};

use crate::Worker;
use crate::import::Internal;
use crate::quiet::Pacer;

/// Rest between two files.
const REST: Duration = Duration::from_millis(500);
/// Rest between two files while music plays.
const REST_PLAYING: Duration = Duration::from_secs(4);
/// A shutdown waits this long for the file being checked.
const STOP_WAIT: Duration = Duration::from_secs(5);
/// Changed rows are reported at least this often during a pass.
const REPORT_EVERY: usize = 8;

/// Bit of the «Подтверди» ids of the offers to turn fakes into MP3s.
pub(crate) const QUALITY_ID: u64 = 1 << 45;

/// From the check's thread (and the conversions the user asked for) to the
/// worker.
pub(crate) enum Msg {
    /// These tracks were checked; `changed`: what their rows show changed
    /// (a fake found, a file converted), so the lists reload.
    Checked { tracks: Vec<TrackId>, changed: bool },
    /// The fakes the user said to turn into MP3s are done: the `(mp3,
    /// original)` sources of each one made, and how many failed.
    Converted { done: Vec<(SourceId, SourceId)>, failed: usize },
    /// «Вернуть FLAC»: how many came back, and how many could not.
    Restored { done: usize, failed: usize },
    /// The fake file of a track the user turned into an MP3 by hand: the
    /// track's title, the codec the original had, and what came of it.
    OneConverted { track: TrackId, title: String, codec: String, result: Result<(SourceId, SourceId), String> },
}

/// What the settings allow, set by the worker.
#[derive(Clone, Debug, Default)]
pub(crate) struct Settings {
    /// «Проверять «фейковый» FLAC».
    pub check: bool,
    /// «Предлагать MP3 вместо «фейкового» FLAC».
    pub convert: bool,
    /// …«без вопроса»: a plain fake becomes an MP3 at once.
    pub auto: bool,
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
    // Only lossless sound is judged.
    if !t.codec.is_lossless() {
        return Plan::Skip;
    }
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
    let want = match nikgob_core::quality::plausible(c.verdict, c.cutoff_hz) {
        Verdict::Transcoded => fake_score(c.origin, c.cutoff_hz),
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
    fn pass(&self, skip: &mut HashSet<SourceId>, pacer: &mut Pacer<'_>) -> bool {
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
            if !pacer.begin() {
                break;
            }
            match check_file_paced(&path, &self.stop, &mut || {
                pacer.step();
            }) {
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
            pacer.rest();
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
        let hi_res = src.bit_depth.is_some_and(|b| b > 16) || src.sample_rate.is_some_and(|r| r > 48_000);
        let plain = clear_fake(c.verdict, c.cutoff_hz, c.steepness_db, hi_res);
        if is_fake(&src.codec, &c) && plain {
            // Offered in «Подтверди» unless the user lets it happen by itself.
            changed = true;
            let (convert, trash) = {
                let s = self.settings.lock();
                (s.convert && s.auto, s.trash_dir.clone())
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
        crate::quiet::lower_priority();
        self.pause(start);
        match self.lib.drop_orphan_checks() {
            Ok(0) => {}
            Ok(n) => log::info!("quality: {n} checks of sources that are gone dropped"),
            Err(e) => log::warn!("quality: dropping old checks: {e}"),
        }
        // Checks of lossy files, and fakes «cut» where no encoder cuts: put
        // right (rows only, no file is touched).
        match self.lib.tidy_quality_checks() {
            Ok(tracks) if tracks.is_empty() => {}
            Ok(tracks) => {
                log::info!("quality: {} tracks' checks put right", tracks.len());
                let _ = self.tx.send(Internal::Quality(Msg::Checked { tracks, changed: true }));
            }
            Err(e) => log::warn!("quality: putting the checks right: {e}"),
        }
        // A heavy job: never beside the sound analysis or the lyrics pass.
        let mut pacer = Pacer::heavy(&self.stop, "quality check");
        // Not tried again in this run (could not be opened).
        let mut skip = HashSet::new();
        while !self.stopped() {
            self.wake.lock().kicked = false;
            let worked = self.settings.lock().check && self.pass(&mut skip, &mut pacer);
            if worked {
                // A pass's decoding: what it left goes back.
                crate::release_thread_memory();
            } else {
                self.pause(self.idle);
            }
        }
        drop(alive);
    }
}

/// The stored check is of the file at `path` as it is now: the same size
/// and time ([`plan`] checks a file again once they differ).
pub(crate) fn check_fresh(path: &Path, c: &QualityCheck) -> bool {
    file_stamp(path) == Some((c.file_size, c.file_mtime))
}

/// What a click on a conversion says while another one runs.
fn busy_text(loc: Locale) -> &'static str {
    activity_in(loc).quality_busy
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
    /// Stable ids of the «Подтверди» cards of the fakes offered as MP3s,
    /// by album key.
    card_ids: HashMap<String, u64>,
    next_card: u64,
    /// A conversion or a restore the user asked for is running.
    busy: bool,
}

/// The fakes offered as MP3s, one card per album: `(album key, title,
/// offers)`, in the order the library lists them.
pub(crate) fn fake_cards(offers: Vec<FakeOffer>, loc: Locale) -> Vec<(String, String, Vec<FakeOffer>)> {
    let mut out: Vec<(String, String, Vec<FakeOffer>)> = Vec::new();
    for o in offers {
        let key = o.release.map_or_else(|| "-".to_owned(), |r| r.to_string());
        match out.iter_mut().find(|(k, ..)| *k == key) {
            Some((.., list)) => list.push(o),
            None => {
                let title = if o.album.is_empty() { activity_in(loc).separate_tracks.to_owned() } else { o.album.clone() };
                out.push((key, title, vec![o]));
            }
        }
    }
    out
}

/// A song's row on its card: its title, and what the file likely is (only
/// a clear cliff below 19.8 kHz is offered, but the bitrate is an estimate
/// from the cut, never a fact).
pub(crate) fn fake_option(o: &FakeOffer, loc: Locale) -> v::ConfirmOption {
    let s = activity_in(loc);
    let from = o.origin.lossy_kbps().map_or_else(|| s.from_lossy.to_owned(), |k| format!("{} ~{k}", s.from_mp3));
    v::ConfirmOption {
        label: o.title.clone(),
        detail: format!("{}, {from} · {} {}", s.probably, s.cutoff, nikgob_core::quality::khz_in(o.cutoff_hz, loc)),
        recommended: false,
    }
}

/// The «Подтверди» card of an album's fakes (`id`, the album's `title`):
/// a row per song to tick, nothing ticked; the buttons act on the ticked
/// songs only.
pub(crate) fn fake_card(id: u64, title: String, offers: &[FakeOffer], loc: Locale) -> v::Confirm {
    let s = activity_in(loc);
    let n = offers.len();
    v::Confirm {
        yes_no: false,
        id,
        title,
        origin: format!("{} · {n} {}", s.fake_flac, plural_forms(loc, n as u64, &s.files)),
        options: offers.iter().map(|o| fake_option(o, loc)).collect(),
        skip_label: s.keep_flac.into(),
        question: s.fake_question.into(),
        accept_label: s.convert_to_mp3.into(),
        pick: Some(v::ConfirmPick {
            sources: offers.iter().map(|o| o.source_id).collect(),
            accept: (s.convert.into(), s.to_mp3.into()),
            skip: (s.keep.into(), s.flac.into()),
        }),
    }
}

/// Takes back exactly the conversions `pairs` (`(mp3, original)` sources,
/// as each was made): `(done, failed)`. One taken back already (its
/// original out of the Trash) counts as neither. Runs off the player loop.
pub(crate) fn take_back(lib: &Library, trash: &Path, pairs: &[(SourceId, SourceId)]) -> (usize, usize) {
    let (mut done, mut failed) = (0, 0);
    for &(mp3, original) in pairs {
        let c = match lib.conversion(mp3, original) {
            Ok(Some(c)) => c,
            Ok(None) => {
                log::info!("quality: {original} is not in the Trash any more, nothing to take back");
                continue;
            }
            Err(e) => {
                log::warn!("quality: the conversion of {original}: {e}");
                failed += 1;
                continue;
            }
        };
        match restore_conversion(lib, trash, &c) {
            Ok(()) => done += 1,
            Err(e) => {
                log::warn!("quality: {} stays in the Trash: {e}", c.trash_path);
                failed += 1;
            }
        }
    }
    (done, failed)
}

/// The ticked sources a card's answer may act on: those still offered
/// (not kept, converted or checked again meanwhile), each once, in the
/// order ticked. Nothing else, whatever album they are on.
pub(crate) fn still_offered(picked: &[SourceId], offers: &[FakeOffer]) -> Vec<SourceId> {
    let mut out: Vec<SourceId> = Vec::new();
    for s in picked {
        if offers.iter().any(|o| o.source_id == *s) && !out.contains(s) {
            out.push(*s);
        }
    }
    out
}

impl QualityJob {
    pub(crate) fn new(tx: Sender<Internal>) -> Self {
        Self {
            tx,
            settings: Settings::default(),
            shared: None,
            gone: None,
            card_ids: HashMap::new(),
            next_card: 0,
            busy: false,
        }
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
        let (check, convert, auto) = {
            let st = self.state.read();
            (st.prefs.check_fake_lossless, st.prefs.convert_fake_lossless, st.prefs.fake_flac_auto)
        };
        Settings { check, convert, auto, trash_dir: self.import.as_ref().map(|s| s.config().trash_dir.clone()) }
    }

    /// «Подтверди»: the plain fakes as MP3 offers, one card per album with
    /// a row per song (none while offers are off or a conversion runs).
    pub(crate) fn publish_fake_offers(&mut self) {
        let on = {
            let p = &self.state.read().prefs;
            p.check_fake_lossless && p.convert_fake_lossless
        };
        let offers = if on && !self.quality.busy && self.import.is_some() {
            self.lib.fake_offers().unwrap_or_default()
        } else {
            Vec::new()
        };
        let loc = self.locale();
        let q = &mut self.quality;
        let mut confirms = Vec::new();
        for (key, title, list) in fake_cards(offers, loc) {
            let id = match q.card_ids.get(&key) {
                Some(id) => *id,
                None => {
                    q.next_card += 1;
                    let id = QUALITY_ID | q.next_card;
                    q.card_ids.insert(key, id);
                    id
                }
            };
            confirms.push(fake_card(id, title, &list, loc));
        }
        self.publish(|s| {
            s.downloads.confirm.retain(|c| c.id & QUALITY_ID == 0);
            s.downloads.confirm.extend(confirms);
        });
    }

    /// The answer on a fakes card for the songs the user ticked, and only
    /// those still offered ([`still_offered`]): their MP3s now
    /// (`convert`), or they stay FLAC and are not offered again. The rest
    /// of the card, and of the album, is left as it is.
    pub(crate) fn quality_picked(&mut self, picked: Vec<SourceId>, convert: bool) {
        let offers = self.lib.fake_offers().unwrap_or_default();
        let sources = still_offered(&picked, &offers);
        if sources.len() < picked.len() {
            log::info!("quality: {} of the ticked files are no longer offered", picked.len() - sources.len());
        }
        if sources.is_empty() {
            self.publish_fake_offers();
            return;
        }
        if !convert {
            for s in &sources {
                let file = offers.iter().find(|o| o.source_id == *s).map_or("", |o| o.locator.as_str());
                match self.lib.set_quality_keep(*s, true) {
                    Ok(()) => log::info!("quality: {file} kept as it is"),
                    Err(e) => log::warn!("quality: keeping {file}: {e}"),
                }
            }
            self.publish_fake_offers();
            return;
        }
        if self.quality.busy {
            log::info!("quality: a conversion runs, the ticked files wait");
            return;
        }
        let Some(svc) = &self.import else { return };
        let Some(enc) = Mp3Encoder::find() else {
            self.toast_err(activity_in(self.locale()).need_ffmpeg, None);
            return;
        };
        let (lib, trash, tx) = (self.lib.clone(), svc.config().trash_dir.clone(), svc.internal());
        let root = self.res.root().to_owned();
        log::info!("quality: turning {} ticked fake files into MP3s", sources.len());
        svc.spawn_blocking(move || {
            crate::quiet::lower_priority();
            let (mut done, mut failed) = (Vec::new(), 0);
            for id in sources {
                let src = lib.get_source(id).ok().flatten();
                let check = lib.quality_check(id).ok().flatten();
                match (src, check) {
                    // The file changed since its check: the background check
                    // looks at it again first.
                    (Some(src), Some(c)) if src.trashed_at.is_none() && !check_fresh(&file_path(&root, &src.locator), &c) => {
                        log::warn!("quality: {} changed since its check, left as it is", src.locator);
                        failed += 1;
                    }
                    (Some(src), Some(c)) if src.trashed_at.is_none() => match convert_fake(&lib, &trash, &enc, &src, &c) {
                        Ok(mp3) => done.push((mp3.id, src.id)),
                        Err(e) => {
                            log::warn!("quality: converting {}: {e}", src.locator);
                            failed += 1;
                        }
                    },
                    _ => {}
                }
            }
            let _ = tx.send(Internal::Quality(Msg::Converted { done, failed }));
        });
        self.quality.busy = true;
        self.publish_fake_offers();
    }

    /// «Превратить в MP3» of `track` (its menu, «Источник и качество»):
    /// its lossless file that surely holds an MP3 becomes that MP3 with its
    /// tags, cover and lyrics, the original goes to the library's Trash,
    /// and «Отменить» on the toast brings it back. One file, on this click
    /// only; a doubt is never converted.
    pub(crate) fn fake_to_mp3(&mut self, track: TrackId) {
        if self.quality.busy {
            log::info!("quality: a conversion runs, {track} waits for the next click");
            self.toast_info(busy_text(self.locale()), None);
            return;
        }
        let src = self
            .lib
            .active_source(track)
            .ok()
            .flatten()
            .filter(|s| s.kind.is_local() && s.available && s.trashed_at.is_none());
        let check = src.as_ref().and_then(|s| self.lib.quality_check(s.id).ok().flatten());
        let (Some(src), Some(check)) = (src, check) else {
            log::warn!("quality: {track} has no checked file to turn into an MP3");
            return;
        };
        if !nikgob_core::quality::mp3_inside(&src.codec, &check.summary()) {
            log::warn!("quality: {} is not surely made from an MP3, left as it is", src.locator);
            return;
        }
        if !check_fresh(&file_path(self.res.root(), &src.locator), &check) {
            log::warn!("quality: {} changed since its check, left as it is", src.locator);
            self.quality.kick();
            self.toast_info(activity_in(self.locale()).file_changed, None);
            return;
        }
        let title = self.lib.get_track(track).ok().flatten().map(|t| t.title).unwrap_or_default();
        let codec = nikgob_core::quality::codec_name(&src.codec);
        let Some(svc) = &self.import else {
            self.toast_err(activity_in(self.locale()).import_unavailable, None);
            return;
        };
        let Some(enc) = Mp3Encoder::find() else {
            self.toast_err(activity_in(self.locale()).need_ffmpeg, None);
            return;
        };
        let (lib, trash, tx) = (self.lib.clone(), svc.config().trash_dir.clone(), svc.internal());
        log::info!("quality: the user turns {} into an MP3", src.locator);
        svc.spawn_blocking(move || {
            crate::quiet::lower_priority();
            let result = convert_fake(&lib, &trash, &enc, &src, &check).map(|mp3| (mp3.id, src.id)).map_err(|e| e.to_string());
            let _ = tx.send(Internal::Quality(Msg::OneConverted { track, title, codec, result }));
        });
        self.quality.busy = true;
        self.publish_fake_offers();
    }

    /// «Отменить» on the toast of [`Self::fake_to_mp3`] or of a card's
    /// «Превратить N в MP3»: exactly the conversions that click made
    /// ([`take_back`], looked up and done on another thread): each original
    /// comes back from the Trash to its place and plays again, its MP3 goes
    /// to the Trash, and the original is not offered as an MP3 again.
    pub(crate) fn undo_fake_to_mp3(&mut self, pairs: Vec<(SourceId, SourceId)>) {
        if self.quality.busy {
            log::info!("quality: a conversion runs, the undo of {} files waits for the next click", pairs.len());
            self.toast_info(busy_text(self.locale()), None);
            return;
        }
        let Some(svc) = &self.import else { return };
        let (lib, trash, tx) = (self.lib.clone(), svc.config().trash_dir.clone(), svc.internal());
        svc.spawn_blocking(move || {
            let (done, failed) = take_back(&lib, &trash, &pairs);
            let _ = tx.send(Internal::Quality(Msg::Restored { done, failed }));
        });
        self.quality.busy = true;
        self.publish_fake_offers();
    }

    /// «Вернуть FLAC из корзины»: every conversion is undone (one that was
    /// undone already is left as it is).
    pub(crate) fn restore_converted_flac(&mut self) {
        let Some(svc) = &self.import else { return };
        if self.quality.busy {
            return;
        }
        let (lib, trash, tx) = (self.lib.clone(), svc.config().trash_dir.clone(), svc.internal());
        svc.spawn_blocking(move || {
            let (mut done, mut failed) = (0, 0);
            for c in lib.conversions().unwrap_or_default() {
                match restore_conversion(&lib, &trash, &c) {
                    Ok(()) => done += 1,
                    Err(e) => {
                        log::warn!("quality: {} stays in the Trash: {e}", c.trash_path);
                        failed += 1;
                    }
                }
            }
            let _ = tx.send(Internal::Quality(Msg::Restored { done, failed }));
        });
        self.quality.busy = true;
        self.publish_fake_offers();
    }

    pub(crate) fn start_quality(&mut self) {
        let root = self.res.root().to_owned();
        let settings = self.quality_settings();
        let (start, idle) = (self.streaming.schedule.analysis_start, self.streaming.schedule.analysis_idle);
        self.quality.start(self.lib.clone(), root, settings, start, idle);
        self.publish_fake_offers();
    }

    /// The prefs changed: the check (and its offers) follow.
    pub(crate) fn quality_prefs(&mut self) {
        let settings = self.quality_settings();
        self.quality.configure(settings);
        self.publish_fake_offers();
    }

    pub(crate) fn quality_msg(&mut self, m: Msg) {
        let loc = self.locale();
        let s = activity_in(loc);
        match m {
            Msg::Restored { done, failed } => {
                self.quality.busy = false;
                match (done, failed) {
                    (0, 0) => self.toast_info(s.nothing_to_restore, None),
                    (d, 0) => self.toast_ok(format!("{}: {d}", s.restored_flac), None),
                    (d, f) => self.toast_err(format!("{}: {d}, {}: {f}", s.restored_flac, s.failed), None),
                }
                self.reload_after_import();
                self.publish_fake_offers();
            }
            Msg::Converted { done, failed } => {
                self.quality.busy = false;
                let d = done.len();
                // What this answer made, taken back in one step.
                let undo = (!done.is_empty()).then(|| self.undo_action(v::Command::UndoFakeToMp3(done)));
                if failed == 0 {
                    self.toast_ok(format!("{}: {d} · {}", s.made_mp3, s.flac_in_trash), undo);
                } else {
                    self.toast_err(format!("{}: {d}, {}: {f}", s.made_mp3, s.failed, f = failed), undo);
                }
                self.reload_after_import();
                self.publish_fake_offers();
            }
            Msg::OneConverted { track, title, codec, result } => {
                self.quality.busy = false;
                match result {
                    Ok(pair) => {
                        log::info!("quality: {track} plays its MP3 now, the {codec} is in the Trash");
                        let undo = self.undo_action(v::Command::UndoFakeToMp3(vec![pair]));
                        let text = format!(
                            "{}: {} · {codec} {}",
                            fmt_quoted_in(loc, &title),
                            s.made_mp3_lc,
                            s.in_trash
                        );
                        self.toast_ok(text, Some(undo));
                    }
                    Err(e) => {
                        log::warn!("quality: turning {track} into an MP3: {e}");
                        self.toast_err(format!("{} {}", s.could_not_make_mp3, fmt_quoted_in(loc, &title)), None);
                    }
                }
                self.reload_after_import();
                self.publish_fake_offers();
            }
            Msg::Checked { tracks, changed } => {
                if changed {
                    self.reload_after_import();
                    self.publish_fake_offers();
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

    /// A conversion goes by a check only while the file at its place is
    /// the one checked: the same size and time.
    #[test]
    fn a_check_is_of_the_file_only_while_its_size_and_time_stay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.flac");
        std::fs::write(&path, b"flac").unwrap();
        let (size, mtime) = file_stamp(&path).unwrap();
        let mut c = check(Verdict::Transcoded, 16_000.0);
        (c.file_size, c.file_mtime) = (size, mtime);
        assert!(check_fresh(&path, &c));
        std::fs::write(&path, b"another flac").unwrap();
        assert!(!check_fresh(&path, &c), "another file at its place");
        assert!(!check_fresh(&dir.path().join("gone.flac"), &c));
    }

    #[test]
    fn files_are_checked_once_and_again_when_they_change() {
        let flac = todo(Codec::Flac, score::CD_LOSSLESS, None, None);
        assert_eq!(plan(&flac, Some((1, 1))), Plan::Check);
        let mp3 = todo(Codec::Mp3, score::MP3_320, None, None);
        assert_eq!(plan(&mp3, Some((1, 1))), Plan::Skip, "a lossy file's own lowpass is its format's");
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
        // A «cut» at 3 kHz is the music's own: the file is lossless again.
        flac.quality_score = 500;
        assert_eq!(rescore(&flac, &check(Verdict::Transcoded, 2_820.0)), None, "no verdict, no change");
    }

    fn offer(title: &str, release: Option<nikgob_core::model::ReleaseId>) -> FakeOffer {
        FakeOffer {
            source_id: SourceId::new(),
            track_id: TrackId::new(),
            title: title.into(),
            release,
            album: if release.is_some() { "8 способов как бросить...".into() } else { String::new() },
            locator: format!("{title}.flac"),
            cutoff_hz: 16_100.0,
            steepness_db: 44.2,
            origin: SoundOrigin::Lossy { kbps: 128 },
        }
    }

    /// One card per album, a row per song with what it really is, nothing
    /// ticked; the buttons count the ticked songs.
    #[test]
    fn fakes_are_offered_album_by_album_and_song_by_song() {
        let r = nikgob_core::model::ReleaseId::new();
        let cards = fake_cards(vec![offer("Молли", Some(r)), offer("Lin Ansty", Some(r)), offer("Супермаркет", None)], Locale::Ru);
        assert_eq!(cards.len(), 2);
        assert_eq!((cards[0].1.as_str(), cards[0].2.len()), ("8 способов как бросить...", 2));
        assert_eq!(cards[1].1, "Отдельные треки");
        let card = fake_card(QUALITY_ID | 1, cards[0].1.clone(), &cards[0].2, Locale::Ru);
        assert_eq!(card.origin, "Ненастоящий FLAC · 2 файла");
        assert_eq!(card.options.len(), 2, "a row per song");
        assert_eq!((card.options[0].label.as_str(), card.options[0].detail.as_str()), ("Молли", "Вероятно, из MP3 ~128 · срез 16,1 кГц"));
        assert!(card.options.iter().all(|o| !o.recommended), "never accepted with the recommended ones");
        let pick = card.pick.as_ref().expect("ticked song by song");
        assert_eq!(pick.sources, cards[0].2.iter().map(|o| o.source_id).collect::<Vec<_>>());
        assert_eq!(v::ConfirmPick::words(&pick.accept, 3), "Превратить 3 в MP3");
        assert_eq!(v::ConfirmPick::words(&pick.accept, 0), "Превратить в MP3");
        assert_eq!(v::ConfirmPick::words(&pick.skip, 1), "Оставить 1 FLAC");
        let one = fake_card(QUALITY_ID | 2, cards[1].1.clone(), &cards[1].2, Locale::Ru);
        assert_eq!(one.origin, "Ненастоящий FLAC · 1 файл");
        let many: Vec<FakeOffer> = (0..21).map(|i| offer(&i.to_string(), Some(r))).collect();
        assert_eq!(fake_card(QUALITY_ID | 3, String::new(), &many, Locale::Ru).origin, "Ненастоящий FLAC · 21 файл");
        assert_eq!(fake_card(QUALITY_ID | 3, String::new(), &many[..11], Locale::Ru).origin, "Ненастоящий FLAC · 11 файлов");
        assert_eq!(fake_card(QUALITY_ID | 3, String::new(), &many[..7], Locale::Ru).origin, "Ненастоящий FLAC · 7 файлов");
        assert_eq!(fake_card(QUALITY_ID | 3, String::new(), &many[..7], Locale::En).origin, "Fake FLAC · 7 files");
    }

    /// The OMORI card: one song ticked of seven turns only that one; a
    /// source no longer offered (kept, converted, from another card) or
    /// ticked twice is never acted on, and nothing expands to the album.
    #[test]
    fn only_the_ticked_songs_still_offered_are_acted_on() {
        let r = nikgob_core::model::ReleaseId::new();
        let album: Vec<FakeOffer> = (1..=7).map(|i| offer(&format!("OMORI {i}"), Some(r))).collect();
        let ticked = vec![album[2].source_id];
        assert_eq!(still_offered(&ticked, &album), ticked);
        let gone = SourceId::new();
        let picked = vec![album[0].source_id, gone, album[0].source_id, album[5].source_id];
        assert_eq!(still_offered(&picked, &album), vec![album[0].source_id, album[5].source_id]);
        assert!(still_offered(&[], &album).is_empty());
        assert!(still_offered(&[gone], &album).is_empty());
    }

    /// «Отменить» takes back exactly the pair its conversion made: the
    /// converted FLAC comes back and its MP3 goes, even when another FLAC
    /// of the same track went to the Trash later; the other one and its
    /// file stay where they are. Done twice, the second time does nothing.
    #[test]
    fn the_undo_takes_back_exactly_the_pair_it_made() {
        use nikgob_core::model::{ArtistCredit, Codec, Source, SourceKind, Track};
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::open_in_memory().unwrap();
        let track = lib.upsert_track(&Track::new("Молли", ArtistCredit::single("Пошлая Молли", None))).unwrap();
        let (music, trash) = (dir.path().join("Music"), dir.path().join("Trash"));
        std::fs::create_dir_all(&music).unwrap();
        std::fs::create_dir_all(&trash).unwrap();
        // `name` trashed: its row and its file in the Trash.
        let trashed = |name: &str| {
            let at = music.join(name);
            let src = lib.add_source(&Source::new(track.id, SourceKind::LocalFile, at.to_string_lossy(), Codec::Flac)).unwrap();
            let in_trash = trash.join(name);
            std::fs::write(&in_trash, name).unwrap();
            lib.trash_source(src.id, Some(&in_trash.to_string_lossy())).unwrap();
            (src.id, at, in_trash)
        };
        let (original, original_at, _) = trashed("07 - Молли.flac");
        let mp3_at = music.join("07 - Молли.mp3");
        std::fs::write(&mp3_at, b"mp3").unwrap();
        let mp3 = lib.add_source(&Source::new(track.id, SourceKind::LocalFile, mp3_at.to_string_lossy(), Codec::Mp3)).unwrap();
        let (other, other_at, other_in_trash) = trashed("Молли (live).flac");

        assert_eq!(take_back(&lib, &trash, &[(mp3.id, original)]), (1, 0));
        assert!(original_at.exists(), "the converted FLAC is back");
        assert!(lib.get_source(original).unwrap().unwrap().trashed_at.is_none());
        assert!(lib.get_source(mp3.id).unwrap().unwrap().trashed_at.is_some(), "its MP3 is in the Trash");
        assert!(!other_at.exists() && other_in_trash.exists(), "the other FLAC stays in the Trash");
        assert!(lib.get_source(other).unwrap().unwrap().trashed_at.is_some());
        assert_eq!(take_back(&lib, &trash, &[(mp3.id, original)]), (0, 0), "taken back already");
    }
}
