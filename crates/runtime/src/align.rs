//! The lyrics aligner's thread (docs/lyrics-align.md): it checks the
//! playing track's texts against its recording as soon as their lookups
//! settle, makes the timing the user asks for under the lyrics, goes
//! through the whole library in the background when that is on (every
//! text checked, word timing made, a text heard from scratch where there
//! is none), and downloads the models on first use.
//!
//! One thread at the lowest priority with one [`Aligner`]; the user's jobs
//! go first, and a background track gives way the moment one comes. Every
//! result goes to the library (`lyrics_align`, the heard text into
//! `lyrics_text`), so nothing is made twice; the worker hears of it as
//! [`Msg`].

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use nikgob_align::catalog::{self, ModelId, ModelState};
use nikgob_align::deep::Deep;
use nikgob_align::fit::{Fit, Timing};
use nikgob_align::pipeline::{Route, fallback};
use nikgob_align::{Aligner, Catalog, Device, Error, Prepared, Stage};
use nikgob_core::lyrics::{LyricLine, LyricsSource, LyricsSync};
use nikgob_core::model::TrackId;
use nikgob_library::{AlignCounts, AlignedLyrics, Library};
use nikgob_view as v;

use crate::Worker;
use crate::import::Internal;
use crate::lyrics::{Aligned, Place, Text};

/// Who made a row: a new aligner or new rules go through the library again,
/// and the playing song's texts are checked again (timing the user kept
/// stays). `align-3`: words end where the voice ends (never past the
/// source's end of their line or into a silent tail) and last while the
/// voice holds them, and a silent window is no longer raised into noise.
/// `align-4`: words timed by a second pass that keeps each line within its
/// source times, lets voice the text lacks go between lines, times hums
/// by the voice, never lets lines overlap and adds lines sung once more.
/// `align-5`: a word is not split over a stretch without voice.
pub(crate) const MADE_BY: &str = "align-6";
/// A text the recording spells at least this well (over chance) is this
/// song's (docs/lyrics-align.md, «The check»).
pub(crate) const FITS: f32 = 0.08;
/// Or its letters are heard where they are put this much over chance (MMS
/// on English songs: right texts 0.58–0.94, other songs' texts 0.44–0.62;
/// docs/lyrics-align.md, «The check»).
pub(crate) const HEARD: f32 = 0.6;
/// The same for the English aligner: right 0.63–0.98, others 0.11–0.40.
pub(crate) const HEARD_EN: f32 = 0.5;
/// The same for the Japanese aligner, which is unsure of every sung
/// character: right texts 0.12–0.37, others 0.00–0.03.
pub(crate) const HEARD_JA: f32 = 0.07;
/// A failed track is tried again after a week.
const RETRY_MS: i64 = 7 * 86_400_000;
/// Models stay loaded this long without work (an aligner and its few
/// hundred megabytes; the separator and Whisper go after each use).
const KEEP_LOADED: Duration = Duration::from_secs(25);
/// CPU threads of the models.
const THREADS: usize = 3;

/// One text to check.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AlignText {
    pub(crate) source: LyricsSource,
    pub(crate) lines: Vec<LyricLine>,
    /// Its line starts are real times.
    pub(crate) timed: bool,
    pub(crate) hash: String,
}

/// What the thread is told.
pub(crate) enum Req {
    /// Check these texts of a track (the playing one); `recognise`: hear a
    /// text from scratch when none fits.
    Check { track: TrackId, path: PathBuf, texts: Vec<AlignText>, recognise: bool },
    /// Make timing for one text (asked under the lyrics); `temp`: the
    /// audio was fetched from a stream for this and goes after it. `seq`
    /// numbers the asks ([`AlignService::cancel_crafts`]).
    Craft { track: TrackId, path: PathBuf, text: AlignText, target: LyricsSync, temp: bool, seq: u64 },
    Download(ModelId),
    Remove(ModelId),
    Settings(Settings),
    Playing(bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Settings {
    pub(crate) gpu: bool,
    pub(crate) words_all: bool,
    pub(crate) recognise: bool,
    pub(crate) separate: bool,
    pub(crate) models_auto: bool,
    /// «Глубокое создание субтитров» (docs/lyrics-align.md, «Deep mode»).
    pub(crate) deep: bool,
}

impl Settings {
    pub(crate) fn of(p: &v::Prefs) -> Self {
        Self {
            gpu: p.lyrics_gpu,
            words_all: p.lyrics_words_all,
            recognise: p.lyrics_recognize,
            separate: p.lyrics_separate,
            models_auto: p.lyrics_models_auto,
            deep: p.lyrics_deep,
        }
    }

    /// What deep mode adds with these settings.
    pub(crate) fn deep(&self) -> Deep {
        if self.deep { Deep::CHOSEN } else { Deep::OFF }
    }
}

/// What the thread tells the worker.
pub(crate) enum Msg {
    Models(Vec<v::AlignModel>),
    Job(Option<v::AlignJob>),
    /// A track's texts were checked (rows as stored), and maybe a text heard.
    Checked { track: TrackId, rows: Vec<AlignedLyrics>, heard: Option<Vec<LyricLine>> },
    /// A track's check was dropped for a newer song's (skipped past):
    /// its texts stay unchecked, to be checked when it plays again.
    CheckDropped(TrackId),
    Crafted { track: TrackId, source: LyricsSource, result: Result<Vec<LyricLine>, String> },
    /// A stream's audio was fetched to make timing from.
    StreamAudio { track: TrackId, source: LyricsSource, target: LyricsSync, result: Result<PathBuf, String> },
    /// A text went onto the computer («Сохранить текст»), as read back.
    Kept { track: TrackId, result: Result<Text, String> },
    /// A stream was saved as a file for its text («Песню и текст»).
    SongSaved { track: TrackId, result: Result<PathBuf, String> },
    Counts(AlignCounts),
    Device(bool),
    Error(String),
}

/// The worker's handle on the thread.
#[derive(Default)]
pub(crate) struct AlignService {
    tx: Option<Sender<Req>>,
    stop: Arc<AtomicBool>,
    /// Timing asked for up to this [`Req::Craft`] `seq` was called off.
    cancel: Arc<AtomicU64>,
    /// The `seq` of the last [`Req::Craft`] sent.
    crafts: u64,
    /// How many [`Req::Check`]s were sent: a newer one calls off the
    /// check under way (only the playing song's is wanted).
    checks: Arc<AtomicU64>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub(crate) catalog: Option<Catalog>,
    /// The tracks whose check is on its way (and their texts' hashes).
    pub(crate) asked: HashMap<TrackId, Vec<String>>,
    /// Lyrics tags to write once their file is not playing.
    pub(crate) tag_later: HashMap<TrackId, (PathBuf, crate::lyrics_keep::Keep)>,
    /// Texts to keep next to a stream's file once it is imported.
    pub(crate) after_import: HashMap<TrackId, crate::lyrics_keep::Keep>,

    /// Stream audio saved for that, to delete once imported.
    pub(crate) saved_audio: HashMap<PathBuf, TrackId>,
}

impl AlignService {
    pub(crate) fn start(&mut self, lib: Library, root: PathBuf, models: PathBuf, settings: Settings, tx: Sender<Internal>, start: Duration) {
        if self.tx.is_some() {
            return;
        }
        let (req_tx, req_rx) = crossbeam_channel::unbounded();
        let stop = self.stop.clone();
        let cancel = self.cancel.clone();
        let checks = self.checks.clone();
        let catalog = Catalog::new(models);
        self.catalog = Some(catalog.clone());
        let spawned = std::thread::Builder::new().name("nikgob-align".into()).spawn(move || {
            crate::analysis::lower_priority();
            let mut t = Thread::new(lib, root, catalog, settings, tx, req_rx, stop, start);
            t.cancel = cancel;
            t.checks = checks;
            t.run();
        });
        match spawned {
            Ok(h) => {
                self.tx = Some(req_tx);
                self.thread = Some(h);
            }
            Err(e) => log::warn!("align: cannot start: {e}"),
        }
    }

    pub(crate) fn send(&self, req: Req) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(req);
        }
    }

    pub(crate) fn running(&self) -> bool {
        self.tx.is_some()
    }

    /// Asks for the playing song's check; one asked before it is called off.
    pub(crate) fn check(&self, req: Req) {
        self.checks.fetch_add(1, Ordering::Relaxed);
        self.send(req);
    }

    /// The `seq` for the next [`Req::Craft`].
    pub(crate) fn next_craft(&mut self) -> u64 {
        self.crafts += 1;
        self.crafts
    }

    /// Calls off every timing asked for so far: one being made stops at
    /// its next step (a download too), queued ones are dropped, and none
    /// of them reports back.
    pub(crate) fn cancel_crafts(&self) {
        self.cancel.store(self.crafts, Ordering::Relaxed);
    }

    pub(crate) fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.tx = None;
        if let Some(h) = self.thread.take() {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !h.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            if h.is_finished() {
                let _ = h.join();
            } else {
                log::warn!("align: still busy at shutdown");
            }
        }
    }
}

impl Drop for AlignService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The models a part list shows, with their labels.
pub(crate) fn label(id: ModelId) -> &'static str {
    match id {
        ModelId::Runtime => "Основа для моделей (ONNX Runtime)",
        ModelId::Separator => "Отделение голоса от музыки",
        ModelId::AlignerRu => "Слова по голосу: русский",
        ModelId::AlignerJa => "Слова по голосу: японский",
        ModelId::AlignerEn => "Слова по голосу: английский",
        ModelId::AlignerMms => "Слова по голосу: другие языки",
        ModelId::Whisper => "Расслышать текст",
        ModelId::SeparatorDeep => "Глубокое создание: отделение голоса",
        ModelId::AlignerRuDeep => "Глубокое создание: русский",
        ModelId::AlignerSung => "Глубокое создание: английский и другие языки",
    }
}

pub(crate) fn key(id: ModelId) -> &'static str {
    match id {
        ModelId::Runtime => "runtime",
        ModelId::Separator => "separator",
        ModelId::AlignerRu => "ru",
        ModelId::AlignerJa => "ja",
        ModelId::AlignerEn => "en",
        ModelId::AlignerMms => "mms",
        ModelId::Whisper => "whisper",
        ModelId::SeparatorDeep => "separator-deep",
        ModelId::AlignerRuDeep => "ru-deep",
        ModelId::AlignerSung => "sung",
    }
}

pub(crate) fn id_of(key_: &str) -> Option<ModelId> {
    catalog::ALL.iter().map(|s| s.id).find(|id| key(*id) == key_)
}

/// The parts as the settings list them.
pub(crate) fn models_view(catalog: &Catalog, downloading: &HashMap<ModelId, f32>) -> Vec<v::AlignModel> {
    catalog::ALL
        .iter()
        .map(|s| v::AlignModel {
            key: key(s.id).to_owned(),
            label: label(s.id).to_owned(),
            size: s.size(),
            state: match (downloading.get(&s.id), catalog.state(s.id)) {
                (Some(f), _) => v::ModelState::Downloading(*f),
                (None, ModelState::Ready) => v::ModelState::Ready,
                _ => v::ModelState::Missing,
            },
        })
        .collect()
}

/// Whether a checked text is this recording's: the recording spells it
/// better than chance ([`FITS`]), or its letters are heard where the path
/// puts them well over chance ([`HEARD`]), or its own line times sit on
/// the voice (one steady offset), which a stranger's text never does
/// (docs/lyrics-align.md, «The check»).
pub(crate) fn fits(f: &Fit) -> bool {
    let heard = match f.aligner {
        Some(ModelId::AlignerJa) => HEARD_JA,
        Some(ModelId::AlignerEn) => HEARD_EN,
        _ => HEARD,
    };
    f.spelled >= FITS || f.heard >= heard || matches!(f.timing, Timing::Fits | Timing::Shifted)
}

/// A checked text as the library keeps it.
pub(crate) fn row_of(text: &AlignText, fit: Option<&Fit>) -> AlignedLyrics {
    let fits = fit.is_some_and(fits);
    AlignedLyrics {
        source: text.source.key().to_owned(),
        text_hash: text.hash.clone(),
        fit: fit.map_or(0.0, |f| f.spelled),
        timing: match fit.map(|f| f.timing) {
            Some(Timing::Fits) => "fits",
            Some(Timing::Shifted) => "shifted",
            Some(Timing::Elsewhere) => "elsewhere",
            _ => "none",
        }
        .to_owned(),
        offset_ms: fit.map_or(0, |f| (f.offset * 1000.0).round() as i64),
        covered: fit.map_or(1.0, |f| f.covered),
        body: fit.filter(|_| fits).and_then(|f| serde_json::to_string(&f.lines).ok()),
        made_by: MADE_BY.to_owned(),
        made_at: 0,
        saved: false,
    }
}

/// Line timing from word timing: the lines keep their made start and end.
pub(crate) fn to_lines(lines: &[LyricLine]) -> Vec<LyricLine> {
    lines.iter().map(|l| LyricLine { words: Vec::new(), ..l.clone() }).collect()
}

struct Thread {
    lib: Library,
    root: PathBuf,
    aligner: Aligner,
    settings: Settings,
    tx: Sender<Internal>,
    rx: Receiver<Req>,
    stop: Arc<AtomicBool>,
    /// [`AlignService::cancel`].
    cancel: Arc<AtomicU64>,
    /// The `seq` of the timing being made now.
    crafting: Option<u64>,
    /// [`AlignService::checks`].
    checks: Arc<AtomicU64>,
    /// [`Self::checks`] when the check under way started.
    checking: Option<u64>,
    asked: VecDeque<Req>,
    /// The background pass waits until then (playback and the first screens
    /// go first).
    pass_at: Instant,
    /// Tracks the pass could not do in this run.
    skip: std::collections::HashSet<TrackId>,
    downloading: HashMap<ModelId, f32>,
    last_work: Instant,
    playing: bool,
    /// The last download failed: the pass waits for the user.
    offline: bool,
}

impl Thread {
    #[allow(clippy::too_many_arguments)]
    fn new(lib: Library, root: PathBuf, catalog: Catalog, settings: Settings, tx: Sender<Internal>, rx: Receiver<Req>, stop: Arc<AtomicBool>, start: Duration) -> Self {
        let device = if settings.gpu { Device::Gpu } else { Device::Cpu };
        let mut aligner = Aligner::new(catalog, device, THREADS);
        aligner.set_deep(settings.deep());
        Self {
            lib,
            root,
            aligner,
            settings,
            tx,
            rx,
            stop,
            cancel: Arc::new(AtomicU64::new(0)),
            crafting: None,
            checks: Arc::new(AtomicU64::new(0)),
            checking: None,
            asked: VecDeque::new(),
            pass_at: Instant::now() + start,
            skip: Default::default(),
            downloading: HashMap::new(),
            last_work: Instant::now(),
            playing: false,
            offline: false,
        }
    }

    fn send(&self, m: Msg) {
        let _ = self.tx.send(Internal::Align(m));
    }

    fn models(&self) {
        self.send(Msg::Models(models_view(self.aligner.catalog(), &self.downloading)));
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// The timing being made was called off.
    fn cancelled(&self) -> bool {
        self.crafting.is_some_and(|seq| seq <= self.cancel.load(Ordering::Relaxed))
    }

    /// The check under way is for a song skipped past.
    fn superseded(&self) -> bool {
        self.checking.is_some_and(|n| self.checks.load(Ordering::Relaxed) > n)
    }

    /// Takes what came in; settings apply at once, jobs queue.
    fn take(&mut self, req: Req) {
        match req {
            Req::Settings(s) => {
                if s.gpu != self.settings.gpu {
                    self.aligner.set_device(if s.gpu { Device::Gpu } else { Device::Cpu });
                }
                if s.models_auto && !self.settings.models_auto {
                    self.offline = false;
                }
                self.aligner.set_deep(s.deep());
                self.settings = s;
            }
            Req::Playing(p) => self.playing = p,
            Req::Remove(id) => {
                if self.aligner.loaded() {
                    self.aligner.unload();
                }
                if let Err(e) = self.aligner.catalog().remove(id) {
                    self.send(Msg::Error(e.to_string()));
                }
                self.models();
            }
            other => {
                if let Req::Download(_) = other {
                    self.offline = false;
                }
                if matches!(other, Req::Check { .. }) {
                    // Only the playing song's check is wanted: skipping
                    // through songs must not queue a separation for each.
                    let mut dropped = Vec::new();
                    self.asked.retain(|r| match r {
                        Req::Check { track, .. } => {
                            dropped.push(*track);
                            false
                        }
                        _ => true,
                    });
                    for track in dropped {
                        self.send(Msg::CheckDropped(track));
                    }
                }
                self.asked.push_back(other);
            }
        }
    }

    fn run(&mut self) {
        self.models();
        if let Ok(c) = self.lib.alignment_counts(MADE_BY) {
            self.send(Msg::Counts(c));
        }
        loop {
            if self.stopped() {
                return;
            }
            while let Ok(r) = self.rx.try_recv() {
                self.take(r);
            }
            if let Some(job) = self.asked.pop_front() {
                let asked = match &job {
                    Req::Check { track, .. } => Some((*track, None)),
                    Req::Craft { track, text, .. } => Some((*track, Some(text.source))),
                    _ => None,
                };
                // A job that breaks off (a bug on one song) says so and
                // leaves the thread serving the others.
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.job(job))).is_err() {
                    log::error!("align: a job broke off");
                    self.crafting = None;
                    self.checking = None;
                    match asked {
                        Some((track, None)) => self.send(Msg::Checked { track, rows: Vec::new(), heard: None }),
                        Some((track, Some(source))) => {
                            self.send(Msg::Crafted { track, source, result: Err("Не получилось сделать подсветку".to_owned()) });
                        }
                        None => {}
                    }
                    self.send(Msg::Job(None));
                }
                self.last_work = Instant::now();
                continue;
            }
            if Instant::now() >= self.pass_at && self.pass_wanted() {
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.pass_step())) {
                    Ok(true) => {
                        self.last_work = Instant::now();
                        continue;
                    }
                    Ok(false) => {}
                    Err(_) => {
                        log::error!("align: the library pass broke off");
                        self.send(Msg::Job(None));
                        self.pass_at = Instant::now() + Duration::from_secs(30 * 60);
                    }
                }
            }
            if self.aligner.loaded() && self.last_work.elapsed() > KEEP_LOADED {
                self.aligner.unload();
            }
            let mut wait = if Instant::now() < self.pass_at { (self.pass_at - Instant::now()).min(Duration::from_secs(30)) } else { Duration::from_secs(30) };
            if self.aligner.loaded() {
                // Wake up in time to free the models.
                wait = wait.min(KEEP_LOADED.saturating_sub(self.last_work.elapsed()) + Duration::from_millis(200));
            }
            match self.rx.recv_timeout(wait) {
                Ok(r) => self.take(r),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    /// The pass over the library runs only when word timing for every
    /// song is on (it takes hours); the check and recognition otherwise
    /// serve the playing song.
    fn pass_wanted(&self) -> bool {
        self.settings.words_all && !self.offline
    }

    fn job(&mut self, job: Req) {
        match job {
            Req::Check { track, path, texts, recognise } => {
                self.checking = Some(self.checks.load(Ordering::Relaxed));
                let result = self.check(track, &path, &texts, recognise, true);
                let superseded = self.superseded();
                self.checking = None;
                if let Err(e) = result {
                    if superseded {
                        log::info!("align: checking {track} called off for the next song");
                        self.send(Msg::CheckDropped(track));
                    } else {
                        log::info!("align: checking {track}: {e}");
                        self.send(Msg::Checked { track, rows: Vec::new(), heard: None });
                    }
                }
            }
            Req::Craft { track, path, text, target, temp, seq } => {
                self.crafting = Some(seq);
                // Called off while queued: never started.
                let result = if self.cancelled() { Err(Error::Stopped) } else { self.craft(track, &path, &text, target) };
                let cancelled = self.cancelled();
                self.crafting = None;
                if temp && let Err(e) = std::fs::remove_file(&path) {
                    log::warn!("align: removing {}: {e}", path.display());
                }
                if cancelled {
                    log::info!("align: timing of {track} called off");
                    self.send(Msg::Job(None));
                    return;
                }
                let result = result.map_err(|e| match e {
                    Error::Missing(_) | Error::Download(_) => "Не удалось скачать модели".to_owned(),
                    Error::Unavailable(_) => "Файл недоступен".to_owned(),
                    e => e.to_string(),
                });
                self.send(Msg::Crafted { track, source: text.source, result });
            }
            Req::Download(id) => {
                let _ = self.ensure(&[id], true, None);
            }
            _ => {}
        }
        self.send(Msg::Job(None));
    }

    /// Makes sure the parts are there, downloading what lacks (when the
    /// settings let it, or `asked`); the progress counts the bytes of all
    /// that are downloaded, for `track` when a song waits for them. An
    /// aligner with a stand-in that is there is not downloaded unless the
    /// settings let it.
    fn ensure(&mut self, ids: &[ModelId], asked: bool, track: Option<TrackId>) -> Result<(), Error> {
        let lacking: Vec<ModelId> = ids
            .iter()
            .copied()
            .filter(|id| !self.aligner.catalog().ready(*id))
            .filter(|id| asked || self.settings.models_auto || !fallback(*id).is_some_and(|f| self.aligner.catalog().ready(f)))
            .collect();
        let total: u64 = lacking.iter().map(|id| id.spec().size()).sum();
        let mut before = 0u64;
        for &id in ids {
            if self.aligner.catalog().ready(id) {
                continue;
            }
            if !asked && !self.settings.models_auto {
                // Deep mode's parts only add to the normal ones: without
                // them the timing is made as usual.
                if id.deep() || fallback(id).is_some_and(|f| self.aligner.catalog().ready(f)) {
                    continue;
                }
                return Err(Error::Missing(label(id)));
            }
            self.downloading.insert(id, 0.0);
            self.models();
            let tx = self.tx.clone();
            let stop = self.stop.clone();
            let (cancel, crafting) = (self.cancel.clone(), self.crafting);
            let mut last = Instant::now();
            let catalog = self.aligner.catalog().clone();
            let downloading = self.downloading.clone();
            let result = catalog.download(id, &mut |done, size| {
                let f = done as f32 / size.max(1) as f32;
                if last.elapsed() > Duration::from_millis(250) {
                    last = Instant::now();
                    let mut d = downloading.clone();
                    d.insert(id, f);
                    let _ = tx.send(Internal::Align(Msg::Models(models_view(&catalog, &d))));
                    let all = before + done.min(size);
                    let job = v::AlignJob {
                        track,
                        stage: v::AlignStage::Download,
                        progress: all as f32 / total.max(1) as f32,
                        asked,
                        bytes: Some((all, total.max(all))),
                    };
                    let _ = tx.send(Internal::Align(Msg::Job(Some(job))));
                }
                !stop.load(Ordering::Relaxed) && crafting.is_none_or(|seq| seq > cancel.load(Ordering::Relaxed))
            });
            before += id.spec().size();
            self.downloading.remove(&id);
            self.models();
            if let Err(e) = result {
                // Called off or shutting down: not a failure.
                if matches!(e, Error::Stopped) {
                    return Err(e);
                }
                log::warn!("align: downloading {id:?}: {e}");
                self.send(Msg::Error(format!("{}: {e}", label(id))));
                // A deep mode part that would not come: the timing is made
                // as usual (it is tried again next time).
                if id.deep() {
                    continue;
                }
                self.offline = true;
                return Err(e);
            }
        }
        Ok(())
    }

    /// The progress callback of a job: reports the stage, stops on
    /// shutdown and, for the background, when the user asks for something.
    fn progress<'a>(&'a self, track: TrackId, asked: bool) -> impl FnMut(Stage) -> bool + 'a {
        let mut last = Instant::now() - Duration::from_secs(1);
        let mut last_stage = None;
        move |s: Stage| {
            let (stage, f) = match s {
                Stage::Decode => (v::AlignStage::Decode, 0.0),
                Stage::Separate(f) => (v::AlignStage::Separate, f),
                Stage::Listen(f) => (v::AlignStage::Listen, f),
                Stage::Align => (v::AlignStage::Align, 1.0),
                Stage::Recognise(f) => (v::AlignStage::Recognise, f),
            };
            if last_stage != Some(stage) || last.elapsed() > Duration::from_millis(120) {
                last = Instant::now();
                last_stage = Some(stage);
                let _ = self.tx.send(Internal::Align(Msg::Job(Some(v::AlignJob { track: Some(track), stage, progress: f, asked, bytes: None }))));
            }
            !self.stopped() && !self.cancelled() && !self.superseded() && (asked || self.rx.is_empty())
        }
    }

    /// The parts a check of `texts` needs: each text's own aligner. The
    /// aligner of a few foreign lines (an English ad-lib in a Russian song)
    /// is not fetched for them: they take it when a text in that language
    /// brought it, else their text's own.
    fn needs(&self, texts: &[AlignText]) -> Vec<ModelId> {
        let mut ids = vec![ModelId::Runtime];
        if self.settings.separate {
            ids.push(ModelId::Separator);
        }
        for route in texts.iter().filter_map(|t| Route::of(&t.lines)) {
            if !ids.contains(&route.main) {
                ids.push(route.main);
            }
        }
        // Deep mode's parts after the normal ones (downloaded on its first
        // use, never needed: see [`Self::ensure`]).
        let deep = self.settings.deep();
        for route in texts.iter().filter_map(|t| Route::of(&t.lines)) {
            let main = self.aligner.usable(route.main);
            for id in deep.parts(main, self.settings.separate) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        ids
    }

    /// [`Self::ensure`] of what [`Self::needs`] says.
    fn ensure_for(&mut self, texts: &[AlignText], asked: bool, track: Option<TrackId>) -> Result<(), Error> {
        let ids = self.needs(texts);
        self.ensure(&ids, asked, track)
    }

    /// Decodes and separates `path` for texts heard by aligner `main`
    /// (deep mode separates some languages' voice its own way).
    fn prepare(&mut self, track: TrackId, path: &Path, main: Option<ModelId>, asked: bool) -> Result<Prepared, Error> {
        let separate = self.settings.separate;
        // The callback borrows `self`; the aligner is taken out meanwhile.
        let mut aligner = std::mem::replace(&mut self.aligner, Aligner::new(Catalog::new(""), Device::Cpu, 1));
        let r = {
            let mut progress = self.progress(track, asked);
            aligner.prepare_for(path, separate, main, &mut progress)
        };
        self.aligner = aligner;
        let on_gpu = self.aligner.device() == Device::Gpu;
        self.send(Msg::Device(on_gpu));
        r
    }

    fn fit(&mut self, track: TrackId, prep: &mut Prepared, text: &AlignText, asked: bool) -> Result<Option<Fit>, Error> {
        let mut aligner = std::mem::replace(&mut self.aligner, Aligner::new(Catalog::new(""), Device::Cpu, 1));
        let r = {
            let mut progress = self.progress(track, asked);
            aligner.fit(prep, &text.lines, text.timed, &mut progress)
        };
        // The made words in syllables, each timed by the voice.
        let r = r.map(|f| {
            f.map(|mut f| {
                aligner.syllables(prep, &text.lines, text.timed, &mut f);
                f
            })
        });
        self.aligner = aligner;
        r
    }

    /// Checks `texts` against the recording, stores the verdicts and tells
    /// the worker; hears a text from scratch when asked and none fits.
    fn check(&mut self, track: TrackId, path: &Path, texts: &[AlignText], recognise: bool, asked: bool) -> Result<(), Error> {
        let wants_heard = recognise && self.settings.recognise;
        if texts.is_empty() && !wants_heard {
            return Ok(());
        }
        if wants_heard && texts.is_empty() {
            self.ensure(&[ModelId::Whisper], false, asked.then_some(track))?;
        }
        self.ensure_for(texts, false, asked.then_some(track))?;
        let main = texts.iter().find_map(|t| Route::of(&t.lines)).map(|r| self.aligner.usable(r.main));
        let mut prep = self.prepare(track, path, main, asked)?;
        let mut rows = Vec::new();
        for t in texts {
            if Aligner::needs(&t.lines).is_none() {
                continue;
            }
            let fit = self.fit(track, &mut prep, t, asked)?;
            let row = row_of(t, fit.as_ref());
            if let Err(e) = self.lib.put_lyrics_aligned(track, &row) {
                log::warn!("align: storing {track}: {e}");
            }
            rows.push(row);
        }
        let mut heard = None;
        // Heard from scratch only where there is no text at all.
        if wants_heard && texts.is_empty() {
            self.ensure(&[ModelId::Whisper, ModelId::AlignerRu, ModelId::AlignerJa, ModelId::AlignerMms], false, asked.then_some(track))?;
            let mut aligner = std::mem::replace(&mut self.aligner, Aligner::new(Catalog::new(""), Device::Cpu, 1));
            let r = {
                let mut progress = self.progress(track, asked);
                aligner.recognise(&mut prep, &[], &mut progress)
            };
            self.aligner = aligner;
            let lines = r?;
            let body = Text::from_lines(lines.clone()).and_then(|t| t.body());
            if let Err(e) = self.lib.put_lyrics_text(track, LyricsSource::Recognized.key(), body.as_deref()) {
                log::warn!("align: storing the heard text of {track}: {e}");
            }
            heard = Some(lines);
        }
        self.send(Msg::Checked { track, rows, heard });
        Ok(())
    }

    fn craft(&mut self, track: TrackId, path: &Path, text: &AlignText, target: LyricsSync) -> Result<Vec<LyricLine>, Error> {
        self.ensure_for(std::slice::from_ref(text), true, Some(track))?;
        let main = Route::of(&text.lines).map(|r| self.aligner.usable(r.main));
        let mut prep = self.prepare(track, path, main, true)?;
        let fit = self.fit(track, &mut prep, text, true)?.ok_or_else(|| Error::Model("Этот текст не выровнять".into()))?;
        Ok(if target == LyricsSync::Line { to_lines(&fit.lines) } else { fit.lines })
    }

    /// One track of the background pass; false when there is none to do.
    fn pass_step(&mut self) -> bool {
        let tracks = match self.lib.tracks_needing_alignment(MADE_BY, nikgob_library::now_ms() - RETRY_MS, 16) {
            Ok(t) => t,
            Err(e) => {
                log::warn!("align: reading the queue: {e}");
                return false;
            }
        };
        let Some(track) = tracks.into_iter().find(|t| !self.skip.contains(t)) else { return false };
        let Some(src) = self.lib.active_source(track).ok().flatten().filter(|s| s.kind.is_local()) else {
            let _ = self.lib.mark_alignment(track, "done", Some("no file"), MADE_BY);
            return true;
        };
        let p = Path::new(&src.locator);
        let path = if p.is_absolute() { p.to_owned() } else { self.root.join(p) };
        let tags = nikgob_import::tags::read_tags(&path).ok().map(|(t, _)| t);
        let tl = crate::lyrics::cached_lyrics(&self.lib, track, Some(&path), tags.as_ref(), false, None);
        let texts: Vec<AlignText> = tl.unchecked();
        let recognise = self.settings.recognise && tl.texts_empty();
        if texts.is_empty() && !recognise {
            let state = if tl.texts_empty() { "no_text" } else { "done" };
            let _ = self.lib.mark_alignment(track, state, None, MADE_BY);
            return true;
        }
        match self.check(track, &path, &texts, recognise, false) {
            Ok(()) => {
                let _ = self.lib.mark_alignment(track, "done", None, MADE_BY);
            }
            Err(Error::Stopped) => {}
            Err(e @ Error::Unavailable(_)) => {
                log::info!("align: skipping {}: {e}", path.display());
                self.skip.insert(track);
            }
            Err(e @ (Error::Missing(_) | Error::Download(_))) => {
                log::info!("align: the pass waits: {e}");
                self.offline = true;
                return false;
            }
            Err(e) => {
                log::info!("align: {}: {e}", path.display());
                let _ = self.lib.mark_alignment(track, "failed", Some(&e.to_string()), MADE_BY);
            }
        }
        self.send(Msg::Job(None));
        if let Ok(c) = self.lib.alignment_counts(MADE_BY) {
            self.send(Msg::Counts(c));
        }
        true
    }
}

impl Worker {
    /// Starts the aligner's thread (with the import, which knows the data
    /// folder).
    pub(crate) fn start_align(&mut self) {
        let Some(models) = self.models_dir.clone() else { return };
        let settings = Settings::of(&self.state.read().prefs);
        let root = self.res.root().to_owned();
        let start = self.streaming.schedule.analysis_start;
        let tx = self.internal_tx.clone();
        let temp = self.align_audio_dir();
        if temp.exists()
            && let Err(e) = std::fs::remove_dir_all(&temp)
        {
            log::warn!("align: removing {}: {e}", temp.display());
        }
        self.align.start(self.lib.clone(), root, models, settings, tx, start);
    }

    pub(crate) fn align_settings(&mut self) {
        let s = Settings::of(&self.state.read().prefs);
        self.align.send(Req::Settings(s));
    }

    pub(crate) fn align_playing(&mut self, playing: bool) {
        if self.align_was_playing != playing {
            self.align_was_playing = playing;
            self.align.send(Req::Playing(playing));
        }
    }

    /// Sends the track's unchecked texts to the aligner once its lookups
    /// settled (a file only): every text with the check or word timing on,
    /// else only a plain text when no source has line timing (to time its
    /// lines); with no text anywhere, asks for one heard from scratch.
    pub(crate) fn check_lyrics(&mut self, track: TrackId) {
        let prefs = self.state.read().prefs.clone();
        let every = prefs.lyrics_check || prefs.lyrics_words_playing || prefs.lyrics_words_all;
        if !self.align.running() || !(every || prefs.lyrics_lines_auto || prefs.lyrics_recognize) {
            return;
        }
        let Some(src) = self.lib.active_source(track).ok().flatten().filter(|s| s.kind.is_local()) else { return };
        let path = self.res.path(&src.locator);
        let Some(tl) = self.import.as_mut().and_then(|s| s.lyrics_cache.get_mut(&track)) else { return };
        if !tl.settled() {
            return;
        }
        let mut texts = tl.unchecked();
        if !every {
            let wanted = prefs.lyrics_lines_auto && !tl.has_timed();
            texts.retain(|t| wanted && !t.timed);
        }
        let recognise = prefs.lyrics_recognize && tl.nothing_found();
        let hashes: Vec<String> = texts.iter().map(|t| t.hash.clone()).collect();
        if (texts.is_empty() && !recognise) || self.align.asked.get(&track) == Some(&hashes) {
            return;
        }
        tl.checking = Some(hashes.clone());
        self.align.asked.insert(track, hashes);
        self.align.check(Req::Check { track, path, texts, recognise });
    }

    pub(crate) fn align_msg(&mut self, m: Msg) {
        match m {
            Msg::Models(models) => self.publish(|s| s.align.models = models),
            Msg::Job(job) => self.publish(|s| s.align.job = job),
            Msg::Counts(c) => self.publish(|s| {
                s.align.done = c.done;
                s.align.total = c.total;
            }),
            Msg::Device(gpu) => {
                if self.state.read().align.on_gpu != Some(gpu) {
                    self.publish(|s| s.align.on_gpu = Some(gpu));
                }
            }
            Msg::Error(e) => self.publish(|s| s.align.error = Some(e)),
            Msg::Checked { track, rows, heard } => self.lyrics_checked(track, rows, heard),
            Msg::CheckDropped(track) => {
                self.align.asked.remove(&track);
                if let Some(tl) = self.import.as_mut().and_then(|s| s.lyrics_cache.get_mut(&track)) {
                    tl.checking = None;
                }
            }
            Msg::Crafted { track, source, result } => self.crafted(track, source, result),
            Msg::StreamAudio { track, source, target, result } => self.stream_audio(track, source, target, result),
            Msg::Kept { track, result } => self.lyrics_kept(track, result),
            Msg::SongSaved { track, result } => self.song_saved(track, result),
        }
    }

    fn lyrics_checked(&mut self, track: TrackId, rows: Vec<AlignedLyrics>, heard: Option<Vec<LyricLine>>) {
        self.align.asked.remove(&track);
        let Some(tl) = self.import.as_mut().and_then(|s| s.lyrics_cache.get_mut(&track)) else { return };
        tl.checking = None;
        for row in &rows {
            if let Some((source, a)) = Aligned::from_row(row) {
                tl.set_aligned(source, a);
            }
        }
        if let Some(lines) = heard {
            tl.set_text(LyricsSource::Recognized, Text::from_lines(lines));
        }
        let view = tl.view(track);
        self.publish(|s| {
            if s.lyrics.track == Some(track) {
                s.lyrics = view;
            }
        });
        // No text fits: Genius may have this recording's.
        self.start_lyrics_lookups_again(track);
    }

    /// Makes `target` timing of `source`'s text from the track's file, or
    /// from its stream's audio fetched for a while.
    pub(crate) fn craft_lyrics(&mut self, track: TrackId, source: LyricsSource, target: LyricsSync) {
        if self.import.as_ref().and_then(|s| s.lyrics_cache.get(&track)).and_then(|tl| tl.texts_of(source)).is_none() {
            return;
        }
        match self.lyrics_place(track) {
            Place::File => {
                let Some(src) = self.lib.active_source(track).ok().flatten().filter(|s| s.kind.is_local()) else { return };
                let path = self.res.path(&src.locator);
                self.publish(|s| s.align.craft = Some(v::Craft { track, source, target, lines: None, failed: None }));
                self.send_craft(track, source, target, path, false);
            }
            Place::Stream => {
                if !self.align.running() {
                    return;
                }
                let Some(src) = self.lib.active_source(track).ok().flatten() else { return };
                let Some(yt) = self.service(nikgob_core::streaming::Service::Youtube) else {
                    let failed = Some("YouTube Music выключен".to_owned());
                    self.publish(|s| s.align.craft = Some(v::Craft { track, source, target, lines: None, failed }));
                    return;
                };
                let dir = self.align_audio_dir();
                let locator = src.locator.clone();
                self.publish(|s| {
                    s.align.craft = Some(v::Craft { track, source, target, lines: None, failed: None });
                    s.align.job = Some(v::AlignJob { track: Some(track), stage: v::AlignStage::Decode, progress: 0.0, asked: true, bytes: None });
                });
                self.streaming.spawn_internal(async move {
                    let result = yt.save_audio(&locator, &dir).await.map_err(|e| e.to_string());
                    Internal::Align(Msg::StreamAudio { track, source, target, result })
                });
            }
            Place::None | Place::Unclear(_) => {}
        }
    }

    /// Where a stream's audio waits while timing is made from it: next to
    /// the copies saved from YouTube, emptied when the aligner starts (a
    /// run closed meanwhile leaves nothing behind for long).
    pub(crate) fn align_audio_dir(&self) -> PathBuf {
        match &self.import {
            Some(svc) => svc.config().downloads_dir.join(".align"),
            None => std::env::temp_dir().join("nikgob-align-audio"),
        }
    }

    fn send_craft(&mut self, track: TrackId, source: LyricsSource, target: LyricsSync, path: PathBuf, temp: bool) {
        let text = self.import.as_ref().and_then(|s| s.lyrics_cache.get(&track)).and_then(|tl| tl.texts_of(source));
        let Some(text) = text.filter(|_| self.align.running()) else {
            if temp {
                let _ = std::fs::remove_file(&path);
            }
            return;
        };
        let (lines, timed) = text.align_lines();
        let text = AlignText { source, lines, timed, hash: text.hash() };
        let seq = self.align.next_craft();
        self.align.send(Req::Craft { track, path, text, target, temp, seq });
    }

    /// A stream's audio is here: the timing is made from it (and it goes
    /// after); a failure says why on the control.
    fn stream_audio(&mut self, track: TrackId, source: LyricsSource, target: LyricsSync, result: Result<PathBuf, String>) {
        let asked = self.state.read().align.craft.as_ref().is_some_and(|c| c.track == track && c.source == source);
        match result {
            Ok(path) if asked => self.send_craft(track, source, target, path, true),
            Ok(path) => {
                let _ = std::fs::remove_file(&path);
            }
            Err(e) => {
                log::warn!("align: fetching the stream of {track}: {e}");
                self.publish(|s| {
                    s.align.job = None;
                    if let Some(c) = s.align.craft.as_mut().filter(|c| c.track == track) {
                        c.failed = Some("Не удалось получить звук песни".into());
                    }
                });
            }
        }
    }

    /// Made timing becomes the track's text at once (picking another
    /// source undoes it); a failure stays for the control to say.
    fn crafted(&mut self, track: TrackId, source: LyricsSource, result: Result<Vec<LyricLine>, String>) {
        let asked = self.state.read().align.craft.as_ref().is_some_and(|c| c.track == track && c.source == source);
        if !asked {
            return;
        }
        let lines = match result {
            Ok(lines) => lines,
            Err(e) => {
                self.publish(|s| {
                    if let Some(c) = s.align.craft.as_mut() {
                        c.failed = Some(e);
                    }
                });
                return;
            }
        };
        self.publish(|s| s.align.craft = None);
        let Some(tl) = self.import.as_mut().and_then(|s| s.lyrics_cache.get_mut(&track)) else { return };
        let Some(text) = tl.texts_of(source) else { return };
        let row = AlignedLyrics {
            source: source.key().to_owned(),
            text_hash: text.hash(),
            fit: 1.0,
            timing: "none".into(),
            offset_ms: 0,
            covered: 1.0,
            body: serde_json::to_string(&lines).ok(),
            made_by: MADE_BY.into(),
            made_at: 0,
            saved: true,
        };
        if let Err(e) = self.lib.put_lyrics_aligned(track, &row) {
            log::warn!("saving made lyrics: {e}");
            return;
        }
        if let Some((source, a)) = Aligned::from_row(&row) {
            tl.set_aligned(source, a);
        }
        // Asked for: seen as made, whatever was chosen to be seen before.
        if tl.display.take().is_some()
            && let Err(e) = self.lib.set_lyrics_show(track, None)
        {
            log::warn!("forgetting how lyrics are seen: {e}");
        }
        self.pick_lyrics_source(track, Some(source));
    }

    /// `Command::CancelCraft`: the timing being made stops (the aligner's
    /// job, or the stream's audio is dropped once it comes); the text stays
    /// as it was.
    pub(crate) fn cancel_craft(&mut self) {
        if !self.state.read().align.craft.as_ref().is_some_and(|c| c.failed.is_none()) {
            return;
        }
        self.align.cancel_crafts();
        self.publish(|s| {
            s.align.craft = None;
            if s.align.job.as_ref().is_some_and(|j| j.asked) {
                s.align.job = None;
            }
        });
    }

    /// `Command::ClearCraft`: a failure was seen.
    pub(crate) fn clear_craft(&mut self) {
        if self.state.read().align.craft.as_ref().is_some_and(|c| c.failed.is_some()) {
            self.publish(|s| s.align.craft = None);
        }
    }

    pub(crate) fn align_model(&mut self, key_: &str, download: bool) {
        let Some(id) = id_of(key_) else { return };
        self.publish(|s| s.align.error = None);
        self.align.send(if download { Req::Download(id) } else { Req::Remove(id) });
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn fit(spelled: f32, timing: Timing) -> Fit {
        Fit {
            score: 0.9,
            heard: spelled,
            spelled,
            covered: 0.8,
            timing,
            offset: -2.14,
            spread: 0.06,
            lines: vec![LyricLine::new(Duration::from_secs(16), "Слово за слово")],
            ends: vec![None],
            aligner: Some(ModelId::AlignerRu),
        }
    }

    /// Called-off timing stops the job at its next step and a queued one
    /// never starts; neither reports back (the text stays as it was), and
    /// timing asked for after that runs.
    #[test]
    fn called_off_timing_stops_and_says_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = crossbeam_channel::unbounded();
        let (_req_tx, req_rx) = crossbeam_channel::unbounded();
        let lib = Library::open_in_memory().unwrap();
        let settings = Settings { gpu: false, words_all: false, recognise: false, separate: true, models_auto: false, deep: false };
        let mut t = Thread::new(lib, dir.path().into(), Catalog::new(dir.path()), settings, tx, req_rx, Arc::new(AtomicBool::new(false)), Duration::ZERO);
        let mut service = AlignService::default();
        t.cancel = service.cancel.clone();
        let first = service.next_craft();
        // While it is made: the progress says go on until it is called off.
        t.crafting = Some(first);
        let track = TrackId::new();
        assert!(t.progress(track, true)(Stage::Decode));
        service.cancel_crafts();
        assert!(!t.progress(track, true)(Stage::Separate(0.5)));
        t.crafting = None;
        // Queued and called off: dropped; its temp audio goes too.
        let audio = dir.path().join("stream.opus");
        std::fs::write(&audio, b"x").unwrap();
        let text = AlignText { source: LyricsSource::Genius, lines: vec![LyricLine::new(Duration::ZERO, "a")], timed: false, hash: "h".into() };
        t.job(Req::Craft { track, path: audio.clone(), text, target: LyricsSync::Word, temp: true, seq: first });
        assert!(!audio.exists());
        let said: Vec<Internal> = rx.try_iter().collect();
        assert!(said.iter().all(|m| !matches!(m, Internal::Align(Msg::Crafted { .. }))));
        assert!(said.iter().any(|m| matches!(m, Internal::Align(Msg::Job(None)))));
        // Asked again: not called off.
        t.crafting = Some(service.next_craft());
        assert!(t.progress(track, true)(Stage::Decode));
    }

    /// Deep mode's parts come after the normal ones, and missing ones
    /// that may not be downloaded leave the timing to the normal ones.
    #[test]
    fn deep_mode_parts_are_wanted_but_never_needed() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let (_req_tx, req_rx) = crossbeam_channel::unbounded();
        let lib = Library::open_in_memory().unwrap();
        let catalog = Catalog::new(dir.path());
        for id in [ModelId::Runtime, ModelId::Separator, ModelId::AlignerRu] {
            std::fs::create_dir_all(catalog.path(id)).unwrap();
            std::fs::write(catalog.path(id).join(".ready"), b"").unwrap();
        }
        let settings = Settings { gpu: false, words_all: false, recognise: false, separate: true, models_auto: false, deep: true };
        let mut t = Thread::new(lib, dir.path().into(), catalog, settings, tx, req_rx, Arc::new(AtomicBool::new(false)), Duration::ZERO);
        let text = AlignText { source: LyricsSource::Genius, lines: vec![LyricLine::new(Duration::ZERO, "Я иду домой")], timed: false, hash: "h".into() };
        let ids = t.needs(std::slice::from_ref(&text));
        assert_eq!(&ids[..3], [ModelId::Runtime, ModelId::Separator, ModelId::AlignerRu]);
        assert!(ids[3..].iter().all(|id| id.deep()) && ids.contains(&ModelId::SeparatorDeep) && ids.contains(&ModelId::AlignerRuDeep), "{ids:?}");
        assert!(t.ensure_for(std::slice::from_ref(&text), false, None).is_ok());
        assert!(!t.offline);
        t.settings.deep = false;
        assert_eq!(t.needs(std::slice::from_ref(&text)).len(), 3);
    }

    #[test]
    fn skipping_songs_keeps_only_the_playing_song_s_check() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = crossbeam_channel::unbounded();
        let (_req_tx, req_rx) = crossbeam_channel::unbounded();
        let lib = Library::open_in_memory().unwrap();
        let settings = Settings { gpu: false, words_all: false, recognise: false, separate: true, models_auto: false };
        let mut t = Thread::new(lib, dir.path().into(), Catalog::new(dir.path()), settings, tx, req_rx, Arc::new(AtomicBool::new(false)), Duration::ZERO);
        let service = AlignService::default();
        t.checks = service.checks.clone();
        let tracks: Vec<TrackId> = (0..3).map(|_| TrackId::new()).collect();
        let check = |track: TrackId| Req::Check { track, path: PathBuf::new(), texts: Vec::new(), recognise: false };
        // A check under way goes on until a newer song's is asked.
        service.checks.fetch_add(1, Ordering::Relaxed);
        t.checking = Some(service.checks.load(Ordering::Relaxed));
        assert!(!t.superseded());
        for &track in &tracks {
            service.checks.fetch_add(1, Ordering::Relaxed);
            t.take(check(track));
        }
        assert!(t.superseded(), "the check under way is called off");
        assert_eq!(t.asked.len(), 1, "only the last song's check is queued");
        assert!(matches!(t.asked[0], Req::Check { track, .. } if track == tracks[2]));
        let dropped = rx.try_iter().filter(|m| matches!(m, Internal::Align(Msg::CheckDropped(_)))).count();
        assert_eq!(dropped, 2);
    }

    #[test]
    fn a_fitting_text_keeps_its_made_lines_and_a_misfit_none() {
        let text = AlignText { source: LyricsSource::Lrclib, lines: Vec::new(), timed: true, hash: "h".into() };
        let row = row_of(&text, Some(&fit(0.55, Timing::Shifted)));
        assert_eq!((row.timing.as_str(), row.offset_ms, row.source.as_str()), ("shifted", -2140, "lrclib"));
        assert!(row.body.is_some());
        let (source, a) = Aligned::from_row(&row).unwrap();
        assert_eq!(source, LyricsSource::Lrclib);
        assert!(a.fits && a.offset_early && a.offset == Duration::from_millis(2140));
        let misfit = row_of(&text, Some(&Fit { heard: 0.3, ..fit(0.01, Timing::Elsewhere) }));
        // Line times that sit on the voice are this recording's text.
        assert!(row_of(&text, Some(&Fit { heard: 0.0, ..fit(0.0, Timing::Shifted) })).body.is_some());
        assert!(misfit.body.is_none());
        assert!(!Aligned::from_row(&misfit).unwrap().1.fits);
        // No aligner spells it: not a verdict on the text.
        assert_eq!(row_of(&text, None).timing, "none");
    }

    #[test]
    fn a_japanese_plain_text_is_judged_on_the_japanese_aligner_s_levels() {
        // Measured: Eve's «Ghost Avenue» with its own text without line
        // times, and with another song's text.
        let text = AlignText { source: LyricsSource::Genius, lines: Vec::new(), timed: false, hash: "h".into() };
        let ja = |heard: f32, spelled: f32| Fit { heard, aligner: Some(ModelId::AlignerJa), ..fit(spelled, Timing::None) };
        let right = row_of(&text, Some(&ja(0.12, 0.02)));
        assert!(right.body.is_some(), "a right Japanese text is not «Другая запись»");
        assert_eq!(right.timing, "none");
        assert!(row_of(&text, Some(&ja(0.03, 0.01))).body.is_none());
        // The English aligner hears other songs' texts less than MMS does.
        let en = |heard: f32| Fit { heard, aligner: Some(ModelId::AlignerEn), ..fit(0.02, Timing::None) };
        assert!(row_of(&text, Some(&en(0.63))).body.is_some());
        assert!(row_of(&text, Some(&en(0.40))).body.is_none());
        // Another aligner keeps its own bar.
        let ru = Fit { heard: 0.23, ..fit(0.04, Timing::None) };
        assert!(row_of(&text, Some(&ru)).body.is_none());
    }

    #[test]
    fn model_keys_round_trip_and_line_timing_drops_words() {
        for s in catalog::ALL {
            assert_eq!(id_of(key(s.id)), Some(s.id));
        }
        let mut l = LyricLine::new(Duration::from_secs(1), "a b");
        l.words.push(nikgob_core::lyrics::LyricWord { start: Duration::from_secs(1), end: Duration::from_secs(2), text: "a ".into() });
        l.end = Some(Duration::from_secs(3));
        let lines = to_lines(&[l]);
        assert!(lines[0].words.is_empty());
        assert_eq!(lines[0].end, Some(Duration::from_secs(3)));
    }

    #[test]
    fn the_parts_list_shows_downloads() {
        let dir = tempfile::tempdir().unwrap();
        let c = Catalog::new(dir.path());
        let mut d = HashMap::new();
        d.insert(ModelId::Whisper, 0.5);
        let view = models_view(&c, &d);
        assert_eq!(view.len(), catalog::ALL.len());
        assert!(view.iter().any(|m| m.key == "whisper" && m.state == v::ModelState::Downloading(0.5)));
        assert!(view.iter().all(|m| m.size > 0 && !m.label.is_empty()));
    }
}
