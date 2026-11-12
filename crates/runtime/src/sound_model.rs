//! The neural sound model of the recommendations (docs/recommendations.md
//! §5): Discogs-EffNet on ONNX Runtime, the same runtime the lyrics
//! alignment loads, on the CPU on one quiet thread whatever the device
//! setting says ([`AUTO_DEVICE`]).
//!
//! * **On demand.** The runtime and the model (`models/discogs-effnet`,
//!   18 MB) are downloaded only once the user picks a mode of «Нейросетевой
//!   анализ звука» that needs them ([`Effnet::download`]).
//! * **0 MB idle.** The session is created on the first window of a batch
//!   with no CPU memory arena and no memory pattern (what a run takes goes
//!   back at once) and dropped [`KEEP_LOADED`] after its last use by a
//!   watch thread of its own: out of RAM and VRAM between batches, whoever
//!   loaded it and however long the analysis rests.
//! * **batch = 1.** Each window's patches (128 frames × 96 mel bands, see
//!   [`nikgob_recs::embedding::Mel`]) run one by one; the window's vector
//!   is their mean.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use nikgob_align::catalog::{Catalog, ModelId};
use nikgob_align::engine::{self, Device, Model};
use nikgob_recs::embedding::{EFFNET_DIM, EMBED_RATE, MEL_BANDS, Mel, PATCH};
use nikgob_recs::{AnalysisError, EMBEDDED_WITH, SoundModel};
use parking_lot::Mutex;

/// Where «Автоматически» runs the model: the CPU. Measured on one 15 s
/// window (`tests::window_bench`, one thread, a debug build): the CPU takes
/// 70 ms a window and its session ~37 MB private; DirectML 33 ms, but 3 s to
/// load and ~300 MB private, ~150 MB of which stay after the session goes
/// (the DirectML device). Background analysis has no use for the GPU's
/// speed, and the budget has no room for its memory.
const AUTO_DEVICE: Device = Device::Cpu;

/// The bench's GPU session (`NIKGOB_BENCH_DEVICE=gpu`), measured against.
#[cfg(test)]
static BENCH_GPU: AtomicBool = AtomicBool::new(false);

fn auto_device() -> Device {
    #[cfg(test)]
    if BENCH_GPU.load(Ordering::Relaxed) {
        return Device::Gpu;
    }
    AUTO_DEVICE
}

/// The session goes this long after its last use.
pub(crate) const KEEP_LOADED: Duration = Duration::from_secs(20);
/// A failed download or load is not tried again for this long.
const RETRY_AFTER: Duration = Duration::from_secs(30 * 60);

/// The model's state, for the settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ModelState {
    Missing,
    Downloading(f32),
    Ready,
    /// The download or the load failed (tried again later).
    Failed,
}

struct Loaded {
    model: Model,
    used: Instant,
    /// Made with «Только на процессоре» on.
    cpu_only: bool,
}

/// The session and whether its idle watch runs.
#[derive(Default)]
struct Session {
    loaded: Mutex<Option<Loaded>>,
    watched: AtomicBool,
}

impl Session {
    /// A thread of its own drops the session [`KEEP_LOADED`] after its last
    /// use, whatever the analysis threads are doing (resting, waiting for
    /// work, or another thread loaded it). It never waits for a window
    /// being computed: it looks again a little later.
    fn watch(self: &Arc<Self>) {
        if self.watched.swap(true, Ordering::AcqRel) {
            return;
        }
        let s = self.clone();
        let spawned = std::thread::Builder::new().name("nikgob-sound-model-idle".into()).spawn(move || {
            loop {
                let mut wait = Duration::from_secs(1);
                if let Some(mut guard) = s.loaded.try_lock() {
                    match guard.as_ref().map(|l| l.used.elapsed()) {
                        None => break,
                        Some(idle) if idle >= KEEP_LOADED => {
                            *guard = None;
                            log::info!("sound model: unloaded after {} s idle", KEEP_LOADED.as_secs());
                            break;
                        }
                        Some(idle) => wait = KEEP_LOADED - idle + Duration::from_millis(50),
                    }
                }
                std::thread::sleep(wait);
            }
            s.watched.store(false, Ordering::Release);
            // A session loaded just as the watch ended gets a watch again.
            if s.loaded.try_lock().is_none_or(|g| g.is_some()) {
                s.watch();
            }
        });
        if let Err(e) = spawned {
            log::warn!("sound model: no idle watch: {e}");
            self.watched.store(false, Ordering::Release);
        }
    }
}

/// Discogs-EffNet, loaded when used and dropped when idle.
pub(crate) struct Effnet {
    catalog: Catalog,
    mel: Mel,
    cpu_only: AtomicBool,
    session: Arc<Session>,
    /// The last failure, and when.
    failed: Mutex<Option<(Instant, String)>>,
    /// Bytes of a download in flight, as a share.
    downloading: Mutex<Option<f32>>,
}

impl Effnet {
    pub(crate) fn new(models: PathBuf) -> Self {
        Self {
            catalog: Catalog::new(models),
            mel: Mel::new(),
            cpu_only: AtomicBool::new(false),
            session: Arc::new(Session::default()),
            failed: Mutex::new(None),
            downloading: Mutex::new(None),
        }
    }

    /// The runtime and the model are on disk.
    pub(crate) fn ready(&self) -> bool {
        self.catalog.ready(ModelId::Runtime) && self.catalog.ready(ModelId::SoundEffnet)
    }

    pub(crate) fn state(&self) -> ModelState {
        if let Some(f) = *self.downloading.lock() {
            return ModelState::Downloading(f);
        }
        if self.failed.lock().as_ref().is_some_and(|(at, _)| at.elapsed() < RETRY_AFTER) {
            return ModelState::Failed;
        }
        if self.ready() { ModelState::Ready } else { ModelState::Missing }
    }

    /// A failure lately: not tried again yet.
    pub(crate) fn resting(&self) -> bool {
        self.state() == ModelState::Failed
    }

    fn fail(&self, what: String) {
        log::warn!("sound model: {what}");
        *self.failed.lock() = Some((Instant::now(), what));
    }

    /// «Только на процессоре»: the next window runs on a session of that
    /// kind (a loaded one of the other kind is made again there, on the
    /// analysis thread; the caller never waits for a window).
    pub(crate) fn set_cpu_only(&self, on: bool) {
        self.cpu_only.store(on, Ordering::Relaxed);
    }

    /// Downloads what is missing of the runtime and the model; `stop`
    /// stops it (what came stays for a resume), `tick` hears the progress
    /// now and then ([`Self::state`]). True when both are there.
    pub(crate) fn download(&self, stop: &AtomicBool, tick: &mut dyn FnMut()) -> bool {
        if self.ready() {
            return true;
        }
        if self.resting() {
            return false;
        }
        let total: u64 = [ModelId::Runtime, ModelId::SoundEffnet]
            .iter()
            .filter(|id| !self.catalog.ready(**id))
            .map(|id| id.spec().size())
            .sum();
        let mut before = 0u64;
        for id in [ModelId::Runtime, ModelId::SoundEffnet] {
            if self.catalog.ready(id) {
                continue;
            }
            log::info!("sound model: downloading {id:?}");
            *self.downloading.lock() = Some(before as f32 / total.max(1) as f32);
            tick();
            let mut told = Instant::now();
            let result = self.catalog.download(id, &mut |done, _| {
                *self.downloading.lock() = Some(((before + done) as f32 / total.max(1) as f32).min(1.0));
                if told.elapsed() >= Duration::from_millis(500) {
                    told = Instant::now();
                    tick();
                }
                !stop.load(Ordering::Relaxed)
            });
            *self.downloading.lock() = None;
            match result {
                Ok(()) => before += id.spec().size(),
                Err(nikgob_align::Error::Stopped) => return false,
                Err(e) => {
                    self.fail(format!("downloading {id:?}: {e}"));
                    return false;
                }
            }
        }
        *self.failed.lock() = None;
        true
    }

    /// The session, loaded if it is not.
    fn with_session<T>(&self, f: impl FnOnce(&mut Model) -> Result<T, AnalysisError>) -> Result<T, AnalysisError> {
        let mut guard = self.session.loaded.lock();
        let cpu_only = self.cpu_only.load(Ordering::Relaxed);
        if guard.as_ref().is_some_and(|l| l.cpu_only != cpu_only) {
            *guard = None;
            log::info!("sound model: made again for the new device setting");
        }
        if guard.is_none() {
            if !self.ready() {
                return Err(AnalysisError::Model("the model is not downloaded".into()));
            }
            let loaded = engine::load_runtime(&self.catalog.path(ModelId::Runtime)).and_then(|()| {
                let device = if cpu_only { Device::Cpu } else { auto_device() };
                let file = self.catalog.file(ModelId::SoundEffnet, ModelId::SoundEffnet.model_file());
                Model::load(&file, device, 1)
            });
            match loaded {
                Ok(model) => {
                    log::info!("sound model: loaded on the {:?}", model.device);
                    *guard = Some(Loaded { model, used: Instant::now(), cpu_only });
                    self.session.watch();
                }
                Err(e) => {
                    let msg = format!("loading: {e}");
                    self.fail(msg.clone());
                    return Err(AnalysisError::Model(msg));
                }
            }
        }
        let loaded = guard.as_mut().expect("loaded above");
        let out = f(&mut loaded.model);
        loaded.used = Instant::now();
        out
    }

    /// A session is in memory (`None`: a window is being computed now).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn loaded(&self) -> Option<bool> {
        self.session.loaded.try_lock().map(|g| g.is_some())
    }

    /// Drops the session now. Only the analysis threads call it (it waits
    /// for a window being computed).
    pub(crate) fn unload(&self) {
        if self.session.loaded.lock().take().is_some() {
            log::info!("sound model: unloaded");
        }
    }
}

impl SoundModel for Effnet {
    fn name(&self) -> &str {
        EMBEDDED_WITH
    }

    fn dim(&self) -> usize {
        EFFNET_DIM
    }

    fn sample_rate(&self) -> u32 {
        EMBED_RATE
    }

    fn embed(&self, mono: &[f32]) -> Result<Vec<f32>, AnalysisError> {
        let frames = self.mel.frames(mono);
        let n = frames.len() / MEL_BANDS;
        if n == 0 {
            return Err(AnalysisError::TooShort);
        }
        self.with_session(|model| {
            let mut sum = vec![0f64; EFFNET_DIM];
            let mut count = 0usize;
            for start in Mel::patch_starts(n) {
                let patch = Mel::patch(&frames, start);
                let outputs = model.run_f32_outputs(vec![1, PATCH, MEL_BANDS], patch).map_err(|e| AnalysisError::Model(e.to_string()))?;
                // The embedding is the 1280-wide head (the other is the 400 styles).
                let Some((_, v)) = outputs.into_iter().find(|(shape, _)| shape.last() == Some(&(EFFNET_DIM as i64))) else {
                    return Err(AnalysisError::Model("no 1280-wide output".into()));
                };
                if v.len() != EFFNET_DIM || v.iter().any(|x| !x.is_finite()) {
                    return Err(AnalysisError::Model("a broken embedding".into()));
                }
                for (s, x) in sum.iter_mut().zip(&v) {
                    *s += f64::from(*x);
                }
                count += 1;
            }
            Ok(sum.into_iter().map(|s| (s / count.max(1) as f64) as f32).collect())
        })
        .inspect_err(|e| {
            // A model that cannot run rests: bliss goes on alone.
            if let AnalysisError::Model(m) = e
                && !self.resting()
            {
                self.fail(m.clone());
                self.unload();
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Without the files nothing loads, nothing is downloaded by asking for
    /// the state, and a window gets a clear error instead of a vector.
    #[test]
    fn without_the_model_nothing_runs() {
        let dir = tempfile::tempdir().unwrap();
        let m = Effnet::new(dir.path().to_owned());
        assert!(!m.ready());
        assert_eq!(m.state(), ModelState::Missing);
        assert!(matches!(m.embed(&vec![0.1; 16_000 * 3]), Err(AnalysisError::Model(_))));
        assert!(matches!(m.embed(&[]), Err(AnalysisError::TooShort)));
        assert_eq!(m.loaded(), Some(false));
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none(), "nothing written");
    }

    /// The process's private bytes (Windows) or mimalloc's estimate of its
    /// commit (elsewhere), in MB.
    #[allow(unsafe_code)]
    fn private_mb() -> f64 {
        let (mut rss, mut commit) = (0usize, 0usize);
        // SAFETY: mimalloc writes the counters it is given and ignores the
        // null ones.
        unsafe {
            libmimalloc_sys::mi_process_info(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &raw mut rss,
                std::ptr::null_mut(),
                &raw mut commit,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
        }
        commit as f64 / 1_048_576.0
    }

    /// The model on one window of 15 s, as the analysis runs it: the load
    /// with the first window, then the time of a window and what the
    /// session holds. `NIKGOB_MODELS` = a models folder with the runtime
    /// and the model (never downloaded here), `NIKGOB_BENCH_DEVICE` = `gpu`
    /// for a DirectML session (the CPU's, as «Автоматически» makes, by
    /// default):
    ///
    /// `NIKGOB_MODELS=… cargo test -p nikgob-runtime --lib sound_model::tests::window_bench -- --ignored --nocapture`
    #[test]
    #[ignore = "a bench: needs the model's files"]
    fn window_bench() {
        let Some(dir) = std::env::var_os("NIKGOB_MODELS") else {
            eprintln!("NIKGOB_MODELS is not set: nothing to measure");
            return;
        };
        let m = Effnet::new(PathBuf::from(dir));
        if !m.ready() {
            eprintln!("the model is not in that folder: nothing to measure");
            return;
        }
        let gpu = std::env::var("NIKGOB_BENCH_DEVICE").is_ok_and(|d| d == "gpu");
        BENCH_GPU.store(gpu, Ordering::Relaxed);
        // Something like music: a few partials and a beat, with noise.
        let rate = EMBED_RATE as f32;
        let mut seed = 0x2545_f491_u32;
        let mono: Vec<f32> = (0..EMBED_RATE as usize * 15)
            .map(|i| {
                let t = i as f32 / rate;
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let noise = (seed as f32 / u32::MAX as f32 - 0.5) * 0.05;
                let beat = if (t * 2.0).fract() < 0.05 { 0.4 } else { 0.0 };
                0.2 * (std::f32::consts::TAU * 220.0 * t).sin() + 0.1 * (std::f32::consts::TAU * 330.0 * t).sin() + beat + noise
            })
            .collect();
        let before = private_mb();
        let t = Instant::now();
        let first = m.embed(&mono).expect("a vector");
        let load = t.elapsed();
        let loaded = private_mb();
        assert_eq!(first.len(), EFFNET_DIM);
        let runs = 5;
        let t = Instant::now();
        for _ in 0..runs {
            m.embed(&mono).expect("a vector");
        }
        let window = t.elapsed() / runs;
        let device = m.session.loaded.lock().as_ref().map(|l| l.model.device);
        let running = private_mb();
        m.unload();
        let after = private_mb();
        eprintln!(
            "sound model on {device:?}: load + first window {load:?}, a window {window:?}; private {before:.0} MB → {loaded:.0} loaded → {running:.0} after {runs} windows → {after:.0} unloaded"
        );
    }

    /// While a window is being computed (the session held), the worker's
    /// calls return at once: the device setting and the look at the state
    /// never wait for it.
    #[test]
    fn the_worker_never_waits_for_a_window() {
        let dir = tempfile::tempdir().unwrap();
        let m = Effnet::new(dir.path().to_owned());
        let held = m.session.loaded.lock();
        let t = Instant::now();
        m.set_cpu_only(true);
        assert_eq!(m.loaded(), None, "busy, not waited for");
        let _ = m.state();
        assert!(t.elapsed() < Duration::from_millis(500));
        drop(held);
        assert_eq!(m.loaded(), Some(false));
    }
}
