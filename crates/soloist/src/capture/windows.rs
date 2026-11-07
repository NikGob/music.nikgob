//! WASAPI process loopback: the sound one process (and the processes it
//! started) renders, captured on its own, with nothing else of the system mix
//! (Windows 10 2004 / build 19041 and later; activation fails before that).
//!
//! Capture runs on a thread of its own in the multithreaded COM apartment:
//! `ActivateAudioInterfaceAsync` on the virtual process-loopback device, an
//! event-driven shared-mode client in 48 kHz stereo `f32` (the engine
//! converts from the process' own format), and the packets go into a bounded
//! queue the reader drains. A packet flagged silent is written as zeros so
//! the stream keeps its timing.
//!
//! The session helpers find the target's audio sessions on the active output
//! devices (by process id) and set their mute and volume, which is what the
//! Windows volume mixer does.
#![allow(unsafe_code)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK, AUDIOCLIENT_ACTIVATION_PARAMS,
    AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, ActivateAudioInterfaceAsync, DEVICE_STATE_ACTIVE,
    IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
    IActivateAudioInterfaceCompletionHandler_Impl, IAudioCaptureClient, IAudioClient,
    IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume,
    MMDeviceEnumerator, PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
    VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, WAVEFORMATEX, eRender,
};
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Com::{
    BLOB, CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW, SetEvent, WaitForSingleObject,
};
use windows::Win32::System::Variant::VT_BLOB;
use windows_core::{IUnknown, Interface, implement, w};

/// The format asked of the capture; Windows converts to it.
pub const RATE: u32 = 48_000;
pub const CHANNELS: u16 = 2;
/// At most this much captured sound waits for the reader; older is dropped.
const QUEUE_SECONDS: usize = 2;
/// Activation answers within milliseconds; this only bounds a hung one.
const ACTIVATION_TIMEOUT: Duration = Duration::from_secs(5);
/// `WAVE_FORMAT_IEEE_FLOAT`.
const FORMAT_FLOAT: u16 = 3;

/// How the capture thread runs.
#[derive(Clone, Copy, Debug)]
pub struct CaptureOptions {
    /// Join MMCSS's "Pro Audio" class, as the engine's render thread joins
    /// "Audio" (cpal's `realtime`). The capture wakes every 10 ms; with every
    /// core busy (14 spinning threads and a 144 Hz frame loop on 12) the
    /// longest time between two packets was 40 ms at normal priority and
    /// 11 ms in MMCSS, and the engine's live source only has its small ring
    /// to cover such a wait.
    pub mmcss: bool,
    /// The buffer asked of the capture client. It adds no latency (packets
    /// come every engine period, 10 ms of 480 frames, however big it is).
    /// Process loopback reports a buffer of one period whatever is asked,
    /// yet a thread held up for 40 ms by a fully loaded CPU lost nothing
    /// (`examples/route_bench.rs`), so the packets queue behind it.
    pub buffer: Duration,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            mmcss: true,
            buffer: Duration::from_millis(200),
        }
    }
}

/// What the capture saw, for diagnostics.
#[derive(Clone, Copy, Debug, Default)]
pub struct CaptureStats {
    /// The client's buffer, in frames.
    pub buffer_frames: u32,
    /// The engine's default period, in 100 ns units.
    pub period_hns: i64,
    pub packets: u64,
    pub min_packet: u32,
    pub max_packet: u32,
    /// Packets WASAPI flagged as following lost sound (the thread was late
    /// beyond the buffer).
    pub discontinuities: u64,
    /// The longest time between two wakes that found sound, in µs.
    pub max_gap_us: u64,
    /// Whether the thread joined MMCSS.
    pub mmcss: bool,
}

struct Shared {
    queue: Mutex<VecDeque<f32>>,
    stop: AtomicBool,
    /// Frames captured since the start (silent packets included).
    frames: AtomicU64,
    /// Frames of them with any sample other than zero.
    sounding: AtomicU64,
    failure: Mutex<Option<String>>,
    stats: Mutex<CaptureStats>,
    /// The thread's start-up result, once it has one.
    started: (Mutex<Option<Result<(), String>>>, Condvar),
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The sound of one process tree, captured until dropped.
pub struct ProcessCapture {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl ProcessCapture {
    /// Starts capturing `pid` and its child processes; returns once the
    /// capture client runs (or could not be made).
    pub fn start(pid: u32) -> Result<Self, String> {
        Self::start_with(pid, CaptureOptions::default())
    }

    /// [`start`](Self::start) with the given options.
    pub fn start_with(pid: u32, options: CaptureOptions) -> Result<Self, String> {
        let shared = Arc::new(Shared {
            queue: Mutex::new(VecDeque::with_capacity(RATE as usize * usize::from(CHANNELS))),
            stop: AtomicBool::new(false),
            frames: AtomicU64::new(0),
            sounding: AtomicU64::new(0),
            failure: Mutex::new(None),
            stats: Mutex::new(CaptureStats::default()),
            started: (Mutex::new(None), Condvar::new()),
        });
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("soloist-capture".into())
            .spawn(move || run(pid, options, &s))
            .map_err(|e| e.to_string())?;
        let (lock_, cv) = &shared.started;
        let mut started = lock(lock_);
        while started.is_none() {
            let (next, timeout) = cv
                .wait_timeout(started, ACTIVATION_TIMEOUT * 2)
                .unwrap_or_else(PoisonError::into_inner);
            started = next;
            if timeout.timed_out() && started.is_none() {
                shared.stop.store(true, Ordering::Release);
                return Err("process loopback did not start".into());
            }
        }
        let result = started.take().unwrap_or(Ok(()));
        drop(started);
        let capture = Self {
            shared,
            thread: Some(thread),
        };
        result.map(|()| capture)
    }

    /// Moves up to `out.len()` captured samples (interleaved stereo) into
    /// `out`; returns how many.
    pub fn take(&self, out: &mut [f32]) -> usize {
        let mut q = lock(&self.shared.queue);
        let n = out.len().min(q.len());
        for (o, s) in out.iter_mut().zip(q.drain(..n)) {
            *o = s;
        }
        n
    }

    /// Whole frames waiting to be taken.
    pub fn buffered_frames(&self) -> usize {
        lock(&self.shared.queue).len() / usize::from(CHANNELS)
    }

    /// Drops what waits (a seek or pause: what was captured before is stale).
    pub fn clear(&self) {
        lock(&self.shared.queue).clear();
    }

    /// Drops all but the newest `frames` frames.
    pub fn keep_newest(&self, frames: usize) {
        let mut q = lock(&self.shared.queue);
        let keep = frames * usize::from(CHANNELS);
        if q.len() > keep {
            let extra = q.len() - keep;
            q.drain(..extra);
        }
    }

    pub fn frames_captured(&self) -> u64 {
        self.shared.frames.load(Ordering::Acquire)
    }

    /// Frames captured that were not silence.
    pub fn frames_sounding(&self) -> u64 {
        self.shared.sounding.load(Ordering::Acquire)
    }

    pub fn stats(&self) -> CaptureStats {
        *lock(&self.shared.stats)
    }

    /// Why capture stopped, once it has.
    pub fn failure(&self) -> Option<String> {
        lock(&self.shared.failure).clone()
    }
}

impl Drop for ProcessCapture {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// An event handle closed when dropped.
struct Event(HANDLE);

impl Event {
    fn new() -> Result<Self, String> {
        unsafe { CreateEventW(None, false, false, None) }
            .map(Self)
            .map_err(|e| format!("CreateEvent: {e}"))
    }

    /// Waits up to `ms`; true when it was set.
    fn wait(&self, ms: u32) -> bool {
        unsafe { WaitForSingleObject(self.0, ms) == WAIT_OBJECT_0 }
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

// SAFETY of sharing: a Win32 event handle may be waited on, signalled and
// closed from any thread.
unsafe impl Send for Event {}
unsafe impl Sync for Event {}

/// Gets the activation's result and signals `done`. It holds the event
/// itself: it may be called after the activation timed out and `activate`
/// returned, and the handle must still be the event then (closed only when
/// both are done with it).
#[implement(IActivateAudioInterfaceCompletionHandler)]
struct Activated {
    done: Arc<Event>,
}

impl IActivateAudioInterfaceCompletionHandler_Impl for Activated_Impl {
    fn ActivateCompleted(
        &self,
        _operation: windows_core::Ref<'_, IActivateAudioInterfaceAsyncOperation>,
    ) -> windows_core::Result<()> {
        unsafe { SetEvent(self.done.0) }
    }
}

/// COM for this thread, released when dropped.
struct Com;

impl Com {
    fn init() -> Result<Self, String> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map(|()| Self)
            .map_err(|e| format!("CoInitializeEx: {e}"))
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

fn float_format() -> WAVEFORMATEX {
    let block = CHANNELS * 4;
    WAVEFORMATEX {
        wFormatTag: FORMAT_FLOAT,
        nChannels: CHANNELS,
        nSamplesPerSec: RATE,
        nAvgBytesPerSec: RATE * u32::from(block),
        nBlockAlign: block,
        wBitsPerSample: 32,
        cbSize: 0,
    }
}

/// The capture client of `pid`'s process tree, initialised and given `event`.
fn activate(pid: u32, buffer: Duration, event: &Event) -> Result<(IAudioClient, IAudioCaptureClient), String> {
    let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: pid,
                ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
            },
        },
    };
    // ManuallyDrop: windows-rs drops a PROPVARIANT with PropVariantClear,
    // which would free the blob, and it points at `params` on the stack.
    let prop = std::mem::ManuallyDrop::new(PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_BLOB,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: PROPVARIANT_0_0_0 {
                    blob: BLOB {
                        cbSize: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                        pBlobData: (&raw mut params).cast(),
                    },
                },
            }),
        },
    });
    let done = Arc::new(Event::new()?);
    let handler: IActivateAudioInterfaceCompletionHandler = Activated { done: done.clone() }.into();
    let operation = unsafe {
        ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&raw const *prop),
            &handler,
        )
    }
    .map_err(|e| format!("ActivateAudioInterfaceAsync: {e}"))?;
    if !done.wait(ACTIVATION_TIMEOUT.as_millis() as u32) {
        return Err("process loopback activation timed out".into());
    }
    let mut hr = windows_core::HRESULT(0);
    let mut unknown: Option<IUnknown> = None;
    unsafe { operation.GetActivateResult(&raw mut hr, &raw mut unknown) }
        .map_err(|e| format!("GetActivateResult: {e}"))?;
    hr.ok().map_err(|e| format!("process loopback activation: {e}"))?;
    let client: IAudioClient = unknown
        .ok_or("process loopback gave no client")?
        .cast()
        .map_err(|e| format!("IAudioClient: {e}"))?;
    let format = float_format();
    unsafe {
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
            (buffer.as_nanos() / 100) as i64,
            0,
            &raw const format,
            None,
        )
    }
    .map_err(|e| format!("IAudioClient::Initialize: {e}"))?;
    let packets: IAudioCaptureClient = unsafe { client.GetService() }.map_err(|e| format!("IAudioCaptureClient: {e}"))?;
    unsafe { client.SetEventHandle(event.0) }.map_err(|e| format!("SetEventHandle: {e}"))?;
    unsafe { client.Start() }.map_err(|e| format!("IAudioClient::Start: {e}"))?;
    Ok((client, packets))
}

/// The calling thread in an MMCSS class until dropped.
struct Mmcss(HANDLE);

impl Mmcss {
    fn join() -> Option<Self> {
        let mut index = 0u32;
        match unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &raw mut index) } {
            Ok(h) => Some(Self(h)),
            Err(e) => {
                log::debug!("soloist capture: MMCSS: {e}");
                None
            }
        }
    }
}

impl Drop for Mmcss {
    fn drop(&mut self) {
        let _ = unsafe { AvRevertMmThreadCharacteristics(self.0) };
    }
}

fn run(pid: u32, options: CaptureOptions, shared: &Shared) {
    let started = |r: Result<(), String>| {
        let (m, cv) = &shared.started;
        *lock(m) = Some(r);
        cv.notify_all();
    };
    let _com = match Com::init() {
        Ok(c) => c,
        Err(e) => return started(Err(e)),
    };
    let event = match Event::new() {
        Ok(e) => e,
        Err(e) => return started(Err(e)),
    };
    let (client, packets) = match activate(pid, options.buffer, &event) {
        Ok(p) => p,
        Err(e) => return started(Err(e)),
    };
    let _mmcss = options.mmcss.then(Mmcss::join).flatten();
    {
        let mut st = lock(&shared.stats);
        st.buffer_frames = unsafe { client.GetBufferSize() }.unwrap_or(0);
        let mut period = 0i64;
        if unsafe { client.GetDevicePeriod(Some(&raw mut period), None) }.is_ok() {
            st.period_hns = period;
        }
        st.min_packet = u32::MAX;
        st.mmcss = _mmcss.is_some();
    }
    started(Ok(()));
    let mut last_sound: Option<std::time::Instant> = None;
    let cap = RATE as usize * usize::from(CHANNELS) * QUEUE_SECONDS;
    let mut scratch: Vec<f32> = Vec::with_capacity(RATE as usize / 10 * usize::from(CHANNELS));
    let failed = |e: String| {
        log::warn!("soloist capture stopped: {e}");
        *lock(&shared.failure) = Some(e);
    };
    while !shared.stop.load(Ordering::Acquire) {
        event.wait(100);
        loop {
            let next = match unsafe { packets.GetNextPacketSize() } {
                Ok(n) => n,
                Err(e) => {
                    failed(format!("GetNextPacketSize: {e}"));
                    let _ = unsafe { client.Stop() };
                    return;
                }
            };
            if next == 0 {
                break;
            }
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            if let Err(e) = unsafe { packets.GetBuffer(&raw mut data, &raw mut frames, &raw mut flags, None, None) } {
                failed(format!("GetBuffer: {e}"));
                let _ = unsafe { client.Stop() };
                return;
            }
            let samples = frames as usize * usize::from(CHANNELS);
            scratch.clear();
            if flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 || data.is_null() || frames > RATE {
                scratch.resize(samples.min(RATE as usize * 2), 0.0);
            } else {
                // SAFETY: WASAPI hands `frames` frames of the format asked for
                // (stereo f32) at `data` until ReleaseBuffer.
                let packet = unsafe { std::slice::from_raw_parts(data.cast::<f32>(), samples) };
                scratch.extend_from_slice(packet);
            }
            let _ = unsafe { packets.ReleaseBuffer(frames) };
            {
                let now = std::time::Instant::now();
                let mut st = lock(&shared.stats);
                st.packets += 1;
                st.min_packet = st.min_packet.min(frames);
                st.max_packet = st.max_packet.max(frames);
                if flags & (AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32) != 0 {
                    st.discontinuities += 1;
                }
                if let Some(last) = last_sound {
                    st.max_gap_us = st.max_gap_us.max(now.duration_since(last).as_micros() as u64);
                }
                last_sound = Some(now);
            }
            shared.frames.fetch_add(u64::from(frames), Ordering::AcqRel);
            if scratch.iter().any(|s| *s != 0.0) {
                shared.sounding.fetch_add(u64::from(frames), Ordering::AcqRel);
            }
            let mut q = lock(&shared.queue);
            q.extend(scratch.iter().copied());
            if q.len() > cap {
                let extra = q.len() - cap;
                q.drain(..extra);
            }
        }
    }
    let _ = unsafe { client.Stop() };
}

/// Sets mute and volume on every audio session of the processes in `pids`
/// on the active output devices; returns how many sessions it changed.
pub fn set_sessions(pids: &[u32], mute: Option<bool>, volume: Option<f32>) -> Result<usize, String> {
    set_sessions_read_back(pids, mute, volume).map(|levels| levels.len())
}

/// [`set_sessions`], returning the level each session it changed was read
/// back at afterwards (what Windows really applies, not what was asked).
pub fn set_sessions_read_back(pids: &[u32], mute: Option<bool>, volume: Option<f32>) -> Result<Vec<f32>, String> {
    let _com = Com::init().ok();
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.map_err(|e| format!("MMDeviceEnumerator: {e}"))?;
    let devices = unsafe { enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) }.map_err(|e| e.to_string())?;
    let mut levels = Vec::new();
    for d in 0..unsafe { devices.GetCount() }.unwrap_or(0) {
        let Ok(device) = (unsafe { devices.Item(d) }) else { continue };
        let Ok(manager) = (unsafe { device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) }) else {
            continue;
        };
        let Ok(sessions) = (unsafe { manager.GetSessionEnumerator() }) else { continue };
        for i in 0..unsafe { sessions.GetCount() }.unwrap_or(0) {
            let Ok(control) = (unsafe { sessions.GetSession(i) }) else { continue };
            let Ok(control2) = control.cast::<IAudioSessionControl2>() else { continue };
            let Ok(pid) = (unsafe { control2.GetProcessId() }) else { continue };
            if !pids.contains(&pid) {
                continue;
            }
            let Ok(simple) = control.cast::<ISimpleAudioVolume>() else { continue };
            if let Some(m) = mute {
                unsafe { simple.SetMute(m, std::ptr::null()) }.map_err(|e| format!("SetMute: {e}"))?;
            }
            if let Some(v) = volume {
                unsafe { simple.SetMasterVolume(v.clamp(0.0, 1.0), std::ptr::null()) }
                    .map_err(|e| format!("SetMasterVolume: {e}"))?;
            }
            let level = unsafe { simple.GetMasterVolume() }.map_err(|e| format!("GetMasterVolume: {e}"))?;
            levels.push(level);
        }
    }
    Ok(levels)
}

/// `pid` and every process it started, and the ones those started, as
/// running now (a WebView2's audio service is a grandchild of its host).
/// Just `pid` when the processes cannot be listed.
///
/// A parent's id may have been reused since its child started, which would
/// pull an unrelated program into the tree (and its sessions down to the
/// routed level). With `pid` in our job object only the job's processes
/// count (everything `pid` started is in it); otherwise a child counts only
/// if it started after its parent.
pub fn process_tree(pid: u32) -> Vec<u32> {
    let mut parents: Vec<(u32, u32)> = Vec::new();
    // SAFETY: a process snapshot handle, walked with an entry of the right
    // size and closed afterwards.
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return vec![pid] };
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        if Process32FirstW(snap, &raw mut entry).is_ok() {
            loop {
                parents.push((entry.th32ProcessID, entry.th32ParentProcessID));
                if Process32NextW(snap, &raw mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    match crate::job::members().filter(|m| m.contains(&pid)) {
        Some(members) => {
            parents.retain(|(child, _)| members.contains(child));
            tree_of(pid, &parents, |_, _| true)
        }
        None => tree_of(pid, &parents, |child, parent| {
            matches!((created(child), created(parent)), (Some(c), Some(p)) if c > p)
        }),
    }
}

/// When process `pid` started (FILETIME ticks).
fn created(pid: u32) -> Option<u64> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: a process handle opened for querying, closed afterwards.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let (mut created, mut exited, mut kernel, mut user) =
            (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let ok = GetProcessTimes(process, &raw mut created, &raw mut exited, &raw mut kernel, &raw mut user).is_ok();
        let _ = CloseHandle(process);
        ok.then(|| (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
    }
}

/// The processes under `root` in a (pid, parent) list, `root` first;
/// `related(child, parent)` must also hold for a child to count.
fn tree_of(root: u32, parents: &[(u32, u32)], related: impl Fn(u32, u32) -> bool) -> Vec<u32> {
    let mut out = vec![root];
    let mut i = 0;
    while i < out.len() {
        let p = out[i];
        for &(child, parent) in parents {
            // Pid 0 is its own parent; a reused pid may claim a parent that
            // is already listed: each once.
            if parent == p && child != p && !out.contains(&child) && related(child, parent) {
                out.push(child);
            }
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_tree_has_children_and_grandchildren_only() {
        let list = [(1, 0), (10, 1), (11, 10), (12, 10), (20, 2), (13, 11), (0, 0)];
        assert_eq!(super::tree_of(10, &list, |_, _| true), vec![10, 11, 12, 13]);
        assert_eq!(super::tree_of(99, &list, |_, _| true), vec![99]);
        // 12 is an older program whose parent's id 10 was reused since.
        assert_eq!(super::tree_of(10, &list, |c, _| c != 12), vec![10, 11, 13]);
    }
}
