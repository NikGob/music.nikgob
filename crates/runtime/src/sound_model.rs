//! The neural sound model of the recommendations (docs/recommendations.md
//! §5): Discogs-EffNet on ONNX Runtime, the same runtime the lyrics
//! alignment loads, on the device «На чём считать анализ звука» says;
//! «Автоматически» picks it batch by batch ([`choose`]).
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
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::{Duration, Instant};

use nikgob_align::catalog::{Catalog, ModelId};
use nikgob_align::engine::{self, Device, Model};
use nikgob_recs::embedding::{EFFNET_DIM, EMBED_RATE, MEL_BANDS, Mel, PATCH};
use nikgob_recs::{AnalysisError, EMBEDDED_WITH, SoundModel};
use nikgob_view::SoundDevice;
use parking_lot::Mutex;

// «Автоматически» ([`choose`]). Measured on one 15 s window
// (`tests::window_bench`, one thread, a debug build): the CPU takes 70 ms a
// window and its session ~37 MB private; DirectML 33 ms, but 3 s to load
// and ~300 MB private, ~150 MB of which stay after the session goes (the
// DirectML device). So the CPU while it is free (the memory budget counts
// more than the speed); the GPU only to keep a busy CPU free.

/// The whole CPU this busy or more (0..1): «Автоматически» moves to a card
/// that has room…
const CPU_BUSY: f32 = 0.70;
/// …and stays there until the CPU is this calm again (no flapping).
const CPU_CALM: f32 = 0.50;
/// A card with this much free memory has room for the session (~300 MB)
/// and the program in front of it…
const VRAM_ROOM: u64 = 1 << 30;
/// …and one on it goes back to the CPU below this much (a game took it).
const VRAM_LOW: u64 = 512 << 20;
/// The card's busiest engine this busy with other programs' work (0..1;
/// a windowed game) keeps «Автоматически» off it…
const GPU_FREE: f32 = 0.25;
/// …and one on it goes back to the CPU at this much.
const GPU_BUSY: f32 = 0.50;
/// «Автоматически» looks again this often while a session is loaded.
const RECHOOSE: Duration = Duration::from_secs(60);

/// What «Автоматически» looks at: how busy the whole CPU is (0..1), the
/// card's free memory, how busy the card is with other programs (its
/// busiest engine, 0..1), and whether there is a card DirectML runs on.
/// `None`: not known (Linux knows no card: the CPU then).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Probe {
    pub cpu: Option<f32>,
    pub vram_free: Option<u64>,
    pub gpu_load: Option<f32>,
    pub gpu: bool,
}

/// Where «Автоматически» runs the next windows, from the `probe` and the
/// device of the session loaded now (`now`): the CPU while it is free; the
/// GPU while the CPU is busy and the card is idle with room; the CPU again
/// once the card is busy (a game drawing, its memory nearly full) or the
/// CPU calm. Anything unknown keeps to the CPU.
pub(crate) fn choose(probe: Probe, now: Option<Device>) -> Device {
    let (Some(cpu), Some(free), Some(load), true) = (probe.cpu, probe.vram_free, probe.gpu_load, probe.gpu) else {
        return Device::Cpu;
    };
    let on_gpu = now == Some(Device::Gpu);
    let busy = if on_gpu { cpu >= CPU_CALM } else { cpu >= CPU_BUSY };
    let room = if on_gpu { free >= VRAM_LOW } else { free >= VRAM_ROOM };
    let idle = if on_gpu { load < GPU_BUSY } else { load < GPU_FREE };
    if busy && room && idle { Device::Gpu } else { Device::Cpu }
}

/// The card's load by other programs from the «GPU Engine» counters'
/// `(instance, utilization %)`: each engine's (`luid_…_phys_…_eng_…`) sum
/// over the processes but `own` (this one's work on it), the busiest one
/// as a share.
fn engine_load<'a>(items: impl IntoIterator<Item = (&'a str, f64)>, own: u32) -> f32 {
    let own = format!("pid_{own}_");
    let mut engines: std::collections::HashMap<&str, f64> = std::collections::HashMap::new();
    for (name, value) in items {
        if name.starts_with(&own) {
            continue;
        }
        let engine = name.find("luid_").map_or(name, |i| &name[i..]);
        *engines.entry(engine).or_default() += value;
    }
    let top = engines.values().copied().fold(0.0, f64::max);
    (top / 100.0).clamp(0.0, 1.0) as f32
}

/// The «GPU Engine» utilization counters, open between looks (a rate
/// needs two readings).
#[cfg(windows)]
struct GpuCounter {
    query: windows_sys::Win32::System::Performance::PDH_HQUERY,
    counter: windows_sys::Win32::System::Performance::PDH_HCOUNTER,
}

// SAFETY: PDH handles may be used from any thread; the counter is used
// under its lock only.
#[cfg(windows)]
#[allow(unsafe_code)]
unsafe impl Send for GpuCounter {}

#[cfg(windows)]
#[allow(unsafe_code)]
impl GpuCounter {
    /// Opens the counters and takes their first reading.
    fn open() -> Option<Self> {
        use windows_sys::Win32::System::Performance::{PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhOpenQueryW};
        let path: Vec<u16> = "\\GPU Engine(*)\\Utilization Percentage".encode_utf16().chain(Some(0)).collect();
        let mut query = std::ptr::null_mut();
        // SAFETY: out-pointers to our locals; the path is NUL-terminated.
        unsafe {
            if PdhOpenQueryW(std::ptr::null(), 0, &mut query) != 0 {
                return None;
            }
            let mut counter = std::ptr::null_mut();
            if PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter) != 0 || PdhCollectQueryData(query) != 0 {
                PdhCloseQuery(query);
                return None;
            }
            Some(Self { query, counter })
        }
    }

    /// The load since the last reading ([`engine_load`]).
    fn read(&self) -> Option<f32> {
        use windows_sys::Win32::System::Performance::{
            PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_MORE_DATA,
            PdhCollectQueryData, PdhGetFormattedCounterArrayW,
        };
        // SAFETY: the handles are open; the buffer is as large and as
        // aligned as PDH asked, and its names live as long as it.
        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                return None;
            }
            let (mut size, mut count) = (0u32, 0u32);
            let r = PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut size, &mut count, std::ptr::null_mut());
            if r != PDH_MORE_DATA {
                return None;
            }
            let mut buf = vec![0u64; (size as usize).div_ceil(8)];
            let items = buf.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
            if PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut size, &mut count, items) != 0 || count == 0 {
                return None;
            }
            let items = std::slice::from_raw_parts(items, count as usize);
            let names: Vec<(String, f64)> = items
                .iter()
                .filter(|i| matches!(i.FmtValue.CStatus, PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA) && !i.szName.is_null())
                .map(|i| {
                    let len = (0..).take_while(|k| *i.szName.add(*k) != 0).count();
                    (String::from_utf16_lossy(std::slice::from_raw_parts(i.szName, len)), i.FmtValue.Anonymous.doubleValue)
                })
                .collect();
            Some(engine_load(names.iter().map(|(n, v)| (n.as_str(), *v)), std::process::id()))
        }
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
impl Drop for GpuCounter {
    fn drop(&mut self) {
        // SAFETY: opened by `open` and closed once.
        unsafe {
            windows_sys::Win32::System::Performance::PdhCloseQuery(self.query);
        }
    }
}

/// Elsewhere the card's load is not known.
#[cfg(not(windows))]
struct GpuCounter;

#[cfg(not(windows))]
impl GpuCounter {
    fn open() -> Option<Self> {
        None
    }

    fn read(&self) -> Option<f32> {
        None
    }
}

/// What the last look of «Автоматически» left: the CPU's times and the
/// card's counters.
#[derive(Default)]
struct Seen {
    cpu: Option<(u64, u64)>,
    gpu: Option<GpuCounter>,
}

/// The system's CPU time so far: (idle, all), in the system's units.
#[cfg(windows)]
#[allow(unsafe_code)]
fn cpu_times() -> Option<(u64, u64)> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetSystemTimes;
    let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    // SAFETY: the three structs are ours and written by the call.
    let ok = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } != 0;
    let n = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
    // Kernel time includes the idle time.
    ok.then(|| (n(idle), n(kernel) + n(user)))
}

/// The system's CPU time so far: (idle, all), from `/proc/stat`.
#[cfg(not(windows))]
fn cpu_times() -> Option<(u64, u64)> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    proc_stat_times(stat.lines().next()?)
}

/// (idle, all) of the `cpu` line of `/proc/stat` (idle and iowait idle).
#[cfg_attr(windows, allow(dead_code))]
fn proc_stat_times(line: &str) -> Option<(u64, u64)> {
    let mut it = line.split_whitespace();
    (it.next()? == "cpu").then_some(())?;
    let v: Vec<u64> = it.filter_map(|x| x.parse().ok()).collect();
    (v.len() >= 5).then(|| (v[3] + v[4], v.iter().sum()))
}

/// How busy the CPU was between two readings of [`cpu_times`].
fn load_between(a: (u64, u64), b: (u64, u64)) -> Option<f32> {
    let all = b.1.checked_sub(a.1).filter(|d| *d > 0)?;
    let idle = b.0.saturating_sub(a.0).min(all);
    Some(1.0 - idle as f32 / all as f32)
}

/// What «Автоматически» sees now: the CPU's and the card's load since the
/// last look (a first look waits a quarter of a second for a second
/// reading; the analysis thread may), the card's free memory.
fn probe(seen: &Mutex<Seen>) -> Probe {
    let mut seen = seen.lock();
    let mut wait = false;
    if seen.cpu.is_none() {
        seen.cpu = cpu_times();
        wait = true;
    }
    if seen.gpu.is_none() {
        seen.gpu = GpuCounter::open();
        wait |= seen.gpu.is_some();
    }
    if wait {
        std::thread::sleep(Duration::from_millis(250));
    }
    let now = cpu_times();
    let cpu = seen.cpu.zip(now).and_then(|(a, b)| load_between(a, b));
    seen.cpu = now;
    let gpu_load = seen.gpu.as_ref().and_then(GpuCounter::read);
    let vram_free = engine::card_free();
    Probe { cpu, vram_free, gpu_load, gpu: cfg!(windows) && vram_free.is_some() }
}

/// [`SoundDevice`] as a number (its place in [`SoundDevice::ALL`]).
fn device_index(d: SoundDevice) -> u8 {
    SoundDevice::ALL.iter().position(|x| *x == d).unwrap_or(0) as u8
}

/// The bench's GPU session (`NIKGOB_BENCH_DEVICE=gpu`), measured against.
#[cfg(test)]
static BENCH_GPU: AtomicBool = AtomicBool::new(false);

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
    /// The setting it was made for, the device it was asked on, and when
    /// that was chosen.
    setting: SoundDevice,
    asked: Device,
    chosen: Instant,
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
    /// «На чём считать анализ звука» ([`SoundDevice`] as a number).
    device: AtomicU8,
    /// What the last look of «Автоматически» left.
    seen: Mutex<Seen>,
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
            device: AtomicU8::new(0),
            seen: Mutex::new(Seen::default()),
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

    /// «На чём считать анализ звука»: the next window runs on a session of
    /// that device (a loaded one of another is made again there, on the
    /// analysis thread; the caller never waits for a window).
    pub(crate) fn set_device(&self, device: SoundDevice) {
        self.device.store(device_index(device), Ordering::Relaxed);
    }

    /// The setting as it is now.
    fn setting(&self) -> SoundDevice {
        SoundDevice::ALL.get(usize::from(self.device.load(Ordering::Relaxed))).copied().unwrap_or_default()
    }

    /// The device the setting asks for: for «Автоматически» what
    /// [`choose`] says, looked at again every [`RECHOOSE`] while a session
    /// is loaded (`now`: its device and when it was chosen).
    fn device_for(&self, setting: SoundDevice, now: Option<(Device, Instant)>) -> Device {
        #[cfg(test)]
        if BENCH_GPU.load(Ordering::Relaxed) {
            return Device::Gpu;
        }
        match setting {
            SoundDevice::Cpu => Device::Cpu,
            SoundDevice::Gpu => Device::Gpu,
            SoundDevice::Auto => match now {
                Some((d, at)) if at.elapsed() < RECHOOSE => d,
                _ => {
                    let p = probe(&self.seen);
                    let d = choose(p, now.map(|(d, _)| d));
                    if now.is_none_or(|(was, _)| was != d) {
                        log::info!("sound model: «Автоматически» picks the {d:?} (CPU {:?}, card {:?}, card free {:?} MB)", p.cpu.map(|c| (c * 100.0).round()), p.gpu_load.map(|c| (c * 100.0).round()), p.vram_free.map(|b| b >> 20));
                    }
                    d
                }
            },
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
        let mut guard = self.session.loaded.lock();
        let setting = self.setting();
        // Another setting, or «Автоматически» chose again: on another
        // device the session is made again.
        let now = guard.as_ref().filter(|l| l.setting == setting).map(|l| (l.asked, l.chosen));
        let device = self.device_for(setting, now);
        if let Some(l) = guard.as_mut() {
            if l.setting != setting || l.asked != device {
                *guard = None;
                log::info!("sound model: made again on the {device:?}");
            } else if now.is_some_and(|(_, at)| at.elapsed() >= RECHOOSE) {
                l.chosen = Instant::now();
            }
        }
        if guard.is_none() {
            if !self.ready() {
                return Err(AnalysisError::Model("the model is not downloaded".into()));
            }
            let loaded = engine::load_runtime(&self.catalog.path(ModelId::Runtime)).and_then(|()| {
                let file = self.catalog.file(ModelId::SoundEffnet, ModelId::SoundEffnet.model_file());
                Model::load(&file, device, 1)
            });
            match loaded {
                Ok(model) => {
                    log::info!("sound model: loaded on the {:?}", model.device);
                    *guard = Some(Loaded { model, used: Instant::now(), setting, asked: device, chosen: Instant::now() });
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
    /// for a DirectML session (the CPU's by default):
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
        if !gpu {
            m.set_device(SoundDevice::Cpu);
        }
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
        m.set_device(SoundDevice::Cpu);
        assert_eq!(m.loaded(), None, "busy, not waited for");
        let _ = m.state();
        assert!(t.elapsed() < Duration::from_millis(500));
        drop(held);
        assert_eq!(m.loaded(), Some(false));
    }

    /// «Автоматически»: the CPU while it is free, the GPU while the CPU is
    /// busy and the card has room, the CPU again once the card is busy or
    /// the CPU calm; between the thresholds it stays where it is; anything
    /// unknown (Linux's card) is the CPU.
    #[test]
    fn automatic_takes_the_cpu_unless_it_is_busy_and_the_card_free() {
        const GB: u64 = 1 << 30;
        let p = |cpu: f32, free: u64| Probe { cpu: Some(cpu), vram_free: Some(free), gpu_load: Some(0.05), gpu: true };
        let drawing = |cpu: f32, load: f32| Probe { gpu_load: Some(load), ..p(cpu, 6 * GB) };
        // Both free: the CPU (less memory).
        assert_eq!(choose(p(0.2, 6 * GB), None), Device::Cpu);
        // A busy CPU and a card with room: the GPU.
        assert_eq!(choose(p(0.9, 6 * GB), None), Device::Gpu);
        assert_eq!(choose(p(0.9, 6 * GB), Some(Device::Cpu)), Device::Gpu);
        // The card busy too (a game, its memory nearly full): the CPU.
        assert_eq!(choose(p(0.9, GB / 4), None), Device::Cpu);
        assert_eq!(choose(p(0.9, GB / 4), Some(Device::Gpu)), Device::Cpu);
        // Between the thresholds each stays where it is.
        assert_eq!(choose(p(0.6, 6 * GB), Some(Device::Gpu)), Device::Gpu);
        assert_eq!(choose(p(0.6, 6 * GB), Some(Device::Cpu)), Device::Cpu);
        assert_eq!(choose(p(0.9, 3 * GB / 4), Some(Device::Gpu)), Device::Gpu);
        assert_eq!(choose(p(0.9, 3 * GB / 4), Some(Device::Cpu)), Device::Cpu);
        // The CPU calm again: back to it.
        assert_eq!(choose(p(0.3, 6 * GB), Some(Device::Gpu)), Device::Cpu);
        // A windowed game draws on the card (its memory free): the CPU,
        // and back to it from the card.
        assert_eq!(choose(drawing(0.9, 0.8), None), Device::Cpu);
        assert_eq!(choose(drawing(0.9, 0.8), Some(Device::Gpu)), Device::Cpu);
        assert_eq!(choose(drawing(0.9, 0.35), Some(Device::Cpu)), Device::Cpu);
        assert_eq!(choose(drawing(0.9, 0.35), Some(Device::Gpu)), Device::Gpu);
        assert_eq!(choose(drawing(0.9, 0.1), None), Device::Gpu);
        // Not known: the CPU.
        assert_eq!(choose(Probe { gpu_load: None, ..p(0.95, 6 * GB) }, None), Device::Cpu);
        assert_eq!(choose(Probe { gpu_load: None, ..p(0.95, 6 * GB) }, Some(Device::Gpu)), Device::Cpu);
        assert_eq!(choose(Probe { vram_free: None, gpu: false, ..p(0.95, 6 * GB) }, None), Device::Cpu);
        assert_eq!(choose(Probe { gpu: false, ..p(0.95, 6 * GB) }, None), Device::Cpu);
        assert_eq!(choose(Probe { cpu: None, ..p(0.95, 6 * GB) }, None), Device::Cpu);
        assert_eq!(choose(Probe::default(), Some(Device::Gpu)), Device::Cpu);
    }

    /// The card's load is its busiest engine's, summed over the other
    /// processes (this one's own work left out).
    #[test]
    fn the_card_load_is_its_busiest_engine_by_others() {
        let me = std::process::id();
        let own = format!("pid_{me}_luid_0x0_0x1_phys_0_eng_0_engtype_3D");
        let items = [
            ("pid_10_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 30.0),
            ("pid_11_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 25.0),
            ("pid_10_luid_0x0_0x1_phys_0_eng_3_engtype_VideoDecode", 20.0),
            (own.as_str(), 40.0),
        ];
        assert!((engine_load(items, me) - 0.55).abs() < 1e-6);
        assert_eq!(engine_load([(own.as_str(), 90.0)], me), 0.0);
        assert_eq!(engine_load([("pid_1_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 250.0)], me), 1.0);
        assert_eq!(engine_load([], me), 0.0);
        // The counters read on this system (none known elsewhere).
        if let Some(g) = GpuCounter::open() {
            std::thread::sleep(Duration::from_millis(100));
            assert!(g.read().is_none_or(|l| (0.0..=1.0).contains(&l)));
        }
    }

    /// The CPU's load between two readings, and `/proc/stat`'s line read.
    #[test]
    fn cpu_load_is_read_between_two_readings() {
        assert_eq!(load_between((100, 1_000), (175, 1_100)), Some(0.25));
        assert_eq!(load_between((100, 1_000), (100, 1_000)), None, "no time passed");
        assert_eq!(load_between((100, 1_000), (300, 1_100)), Some(0.0), "idle never more than all");
        assert_eq!(proc_stat_times("cpu  10 0 20 300 5 0 1 0 0 0"), Some((305, 336)));
        assert_eq!(proc_stat_times("cpu0 10 0 20 300 5"), None);
        assert!(cpu_times().is_some_and(|(idle, all)| idle <= all));
        // The setting's numbers round-trip.
        for d in SoundDevice::ALL {
            let m = Effnet::new(PathBuf::new());
            m.set_device(d);
            assert_eq!(m.setting(), d);
        }
    }
}
