//! The neural sound model of the recommendations (docs/recommendations.md
//! §5): Discogs-EffNet on ONNX Runtime, the same runtime the lyrics
//! alignment loads (DirectML on the GPU with the CPU as fallback, or the
//! CPU alone on one quiet thread).
//!
//! * **On demand.** The runtime and the model (`models/discogs-effnet`,
//!   18 MB) are downloaded only once the user picks a mode of «Нейросетевой
//!   анализ звука» that needs them ([`Effnet::download`]).
//! * **0 MB idle.** The session is created on the first window of a batch
//!   with no CPU memory arena and no memory pattern (what a run takes goes
//!   back at once) and dropped [`KEEP_LOADED`] after its last use
//!   ([`Effnet::unload_idle`]): out of RAM and VRAM between batches.
//! * **batch = 1.** Each window's patches (128 frames × 96 mel bands, see
//!   [`nikgob_recs::embedding::Mel`]) run one by one; the window's vector
//!   is their mean.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use nikgob_align::catalog::{Catalog, ModelId};
use nikgob_align::engine::{self, Device, Model};
use nikgob_recs::embedding::{EFFNET_DIM, EMBED_RATE, MEL_BANDS, Mel, PATCH};
use nikgob_recs::{AnalysisError, EMBEDDED_WITH, SoundModel};
use parking_lot::Mutex;

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
}

/// Discogs-EffNet, loaded when used and dropped when idle.
pub(crate) struct Effnet {
    catalog: Catalog,
    mel: Mel,
    cpu_only: AtomicBool,
    session: Mutex<Option<Loaded>>,
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
            session: Mutex::new(None),
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

    /// «Только на процессоре»: the next session runs on the CPU.
    pub(crate) fn set_cpu_only(&self, on: bool) {
        if self.cpu_only.swap(on, Ordering::Relaxed) != on {
            self.unload();
        }
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
        let mut guard = self.session.lock();
        if guard.is_none() {
            if !self.ready() {
                return Err(AnalysisError::Model("the model is not downloaded".into()));
            }
            let loaded = engine::load_runtime(&self.catalog.path(ModelId::Runtime)).and_then(|()| {
                let device = if self.cpu_only.load(Ordering::Relaxed) { Device::Cpu } else { Device::Gpu };
                let file = self.catalog.file(ModelId::SoundEffnet, ModelId::SoundEffnet.model_file());
                Model::load(&file, device, 1)
            });
            match loaded {
                Ok(model) => {
                    log::info!("sound model: loaded on the {:?}", model.device);
                    *guard = Some(Loaded { model, used: Instant::now() });
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

    /// How long until the idle session goes (`None`: none is loaded).
    pub(crate) fn unload_in(&self) -> Option<Duration> {
        self.session.lock().as_ref().map(|l| KEEP_LOADED.saturating_sub(l.used.elapsed()))
    }

    /// Drops the session once it has been idle [`KEEP_LOADED`].
    pub(crate) fn unload_idle(&self) {
        let mut guard = self.session.lock();
        if guard.as_ref().is_some_and(|l| l.used.elapsed() >= KEEP_LOADED) {
            *guard = None;
            log::info!("sound model: unloaded after {} s idle", KEEP_LOADED.as_secs());
        }
    }

    pub(crate) fn unload(&self) {
        if self.session.lock().take().is_some() {
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
        assert_eq!(m.unload_in(), None);
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none(), "nothing written");
    }
}
