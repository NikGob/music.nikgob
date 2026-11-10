//! ONNX Runtime, loaded from the files the app downloads (never linked):
//! one environment per process, sessions on the GPU through DirectML (any
//! DirectX 12 card) or on the CPU. Linux has the CPU only.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use ort::session::Session;
pub use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;

use crate::Error;

/// Where a model runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Device {
    /// The GPU through DirectML, the CPU when that fails.
    #[default]
    Gpu,
    Cpu,
}

/// The loaded runtime (an error stays: a process loads it once).
static RUNTIME: OnceLock<Result<ort::environment::Environment, String>> = OnceLock::new();
/// Which runtime folder the process loaded.
static LOADED_FROM: Mutex<Option<PathBuf>> = Mutex::new(None);
/// The runtime's library in its folder (see `catalog::RUNTIME`).
const LIBRARY: &str = if cfg!(target_os = "linux") { "libonnxruntime.so.1.24.4" } else { "onnxruntime.dll" };
/// DirectML runs the GPU sessions, and it is Windows' own.
const GPU: bool = cfg!(windows);

/// Loads ONNX Runtime from `dir` (`onnxruntime.dll` and `DirectML.dll`;
/// on Linux `libonnxruntime.so.1.24.4`). DirectML is loaded first by its
/// full path: Windows has an older one of its own that would be found
/// otherwise.
pub fn load_runtime(dir: &Path) -> Result<(), Error> {
    let got = RUNTIME.get_or_init(|| {
        let dml = dir.join("DirectML.dll");
        if GPU
            && dml.exists()
            && let Err(e) = ort::util::preload_dylib(&dml)
        {
            log::warn!("align: loading DirectML: {e}");
        }
        let env = ort::init_from(dir.join(LIBRARY))
            .map_err(|e| e.to_string())?
            .with_name("nikgob")
            .with_telemetry(false)
            .build()
            .map_err(|e| e.to_string())?;
        *LOADED_FROM.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir.to_owned());
        log::info!("align: ONNX Runtime loaded from {}", dir.display());
        Ok(env)
    });
    got.as_ref().map(|_| ()).map_err(|e| Error::Runtime(e.clone()))
}

/// The runtime is loaded (from any folder).
pub fn runtime_loaded() -> bool {
    RUNTIME.get().is_some_and(Result::is_ok)
}

/// How much of the card's own memory is free: its size less what every
/// program holds there now (Windows' counter of the adapter), and no more
/// than Windows budgets this process; `None` where that is not known. The
/// first adapter, which is DirectML's.
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn card_free() -> Option<u64> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO, IDXGIAdapter3, IDXGIFactory1};
    use windows::Win32::System::Performance::{PDH_FMT_COUNTERVALUE, PDH_FMT_LARGE, PDH_HCOUNTER, PDH_HQUERY, PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterValue, PdhOpenQueryW};
    use windows::core::{HSTRING, Interface, PCWSTR};
    // SAFETY: plain DXGI and PDH calls on objects this function owns; each
    // struct is written by the call that takes its pointer, and the query
    // is closed before returning.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let adapter: IDXGIAdapter3 = factory.EnumAdapters1(0).ok()?.cast().ok()?;
        let desc = adapter.GetDesc1().ok()?;
        let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
        adapter.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info).ok()?;
        let budget = info.Budget.saturating_sub(info.CurrentUsage);

        let luid = desc.AdapterLuid;
        let path = HSTRING::from(format!(r"\GPU Adapter Memory(luid_0x{:08x}_0x{:08x}_phys_0)\Dedicated Usage", luid.HighPart, luid.LowPart));
        let mut query = PDH_HQUERY::default();
        if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != 0 {
            return None;
        }
        let mut counter = PDH_HCOUNTER::default();
        let mut value = PDH_FMT_COUNTERVALUE::default();
        let read = PdhAddEnglishCounterW(query, &path, 0, &mut counter) == 0
            && PdhCollectQueryData(query) == 0
            && PdhGetFormattedCounterValue(counter, PDH_FMT_LARGE, None, &mut value) == 0;
        PdhCloseQuery(query);
        if !read {
            return None;
        }
        let used = u64::try_from(value.Anonymous.largeValue).ok()?;
        Some((desc.DedicatedVideoMemory as u64).saturating_sub(used).min(budget))
    }
}

#[cfg(not(windows))]
pub fn card_free() -> Option<u64> {
    None
}

/// One model's session.
pub struct Model {
    session: Session,
    /// Where it actually runs.
    pub device: Device,
}

impl Model {
    /// Opens `path` on `device` (GPU falls back to the CPU) with at most
    /// `threads` CPU threads. The runtime must be loaded.
    pub fn load(path: &Path, device: Device, threads: usize) -> Result<Self, Error> {
        Self::load_with(path, device, threads, GraphOptimizationLevel::Level3)
    }

    /// [`Self::load`] with another graph optimisation: the merged Whisper
    /// decoder's quantised weights trip ONNX Runtime's extended
    /// optimisations, so it takes the basic ones.
    pub fn load_with(path: &Path, device: Device, threads: usize, level: GraphOptimizationLevel) -> Result<Self, Error> {
        let env = match RUNTIME.get() {
            Some(Ok(env)) => env,
            _ => return Err(Error::Runtime("ONNX Runtime is not loaded".into())),
        };
        let ort = |e: ort::Error| Error::Model(format!("{}: {e}", path.display()));
        // No CPU memory arena and no memory pattern: what a run takes goes
        // back to the system when the run (or the session) ends, instead of
        // the largest run's worth staying with the session.
        let cpu = || ort::ep::CPU::default().with_arena_allocator(false).build();
        let base = || -> Result<ort::session::builder::SessionBuilder, Error> {
            Session::builder(env)
                .map_err(ort)?
                .with_optimization_level(level)
                .map_err(|e| ort(e.into()))?
                .with_intra_threads(threads.max(1))
                .map_err(|e| ort(e.into()))?
                .with_inter_threads(1)
                .map_err(|e| ort(e.into()))?
                .with_memory_pattern(false)
                .map_err(|e| ort(e.into()))
        };
        if device == Device::Gpu && GPU {
            let gpu = base()?
                // DirectML wants the memory pattern (above) and this off.
                .with_parallel_execution(false)
                .map_err(|e| ort(e.into()))?
                .with_execution_providers([ort::ep::DirectML::default().build().error_on_failure(), cpu()])
                .map_err(|e| ort(e.into()))
                .and_then(|mut b| b.commit_from_file(path).map_err(ort));
            match gpu {
                Ok(session) => return Ok(Self { session, device: Device::Gpu }),
                Err(e) => log::warn!("align: no GPU for {}: {e}", path.display()),
            }
        }
        let session = base()?
            .with_execution_providers([cpu()])
            .map_err(|e| ort(e.into()))?
            .commit_from_file(path)
            .map_err(ort)?;
        Ok(Self { session, device: Device::Cpu })
    }

    pub fn session(&mut self) -> &mut Session {
        &mut self.session
    }

    /// One float input, the first output back.
    pub fn run_f32(&mut self, shape: Vec<usize>, data: Vec<f32>) -> Result<Vec<f32>, Error> {
        let input = Tensor::from_array((shape, data)).map_err(|e| Error::Model(e.to_string()))?;
        let out = self.session.run(ort::inputs![input]).map_err(|e| Error::Model(e.to_string()))?;
        let (_, data) = out[0].try_extract_tensor::<f32>().map_err(|e| Error::Model(e.to_string()))?;
        Ok(data.to_vec())
    }

    /// [`Self::run_f32`] with the output's shape.
    pub fn run_f32_shaped(&mut self, shape: Vec<usize>, data: Vec<f32>) -> Result<(Vec<i64>, Vec<f32>), Error> {
        let input = Tensor::from_array((shape, data)).map_err(|e| Error::Model(e.to_string()))?;
        let out = self.session.run(ort::inputs![input]).map_err(|e| Error::Model(e.to_string()))?;
        let (shape, data) = out[0].try_extract_tensor::<f32>().map_err(|e| Error::Model(e.to_string()))?;
        Ok((shape.to_vec(), data.to_vec()))
    }
}
