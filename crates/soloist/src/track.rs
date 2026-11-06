//! One Spotify track played through Soloist, as a live [`PcmSource`] of
//! our engine.
//!
//! Soloist plays the track itself (its own output is the one heard: muting
//! its audio session silences the process-loopback capture as well, see
//! `tests/loopback_mute.rs`). The engine still gets an item, so the queue,
//! the position, the end of the track and errors flow exactly as for any
//! other source: the source hands the engine Soloist's sound as captured
//! (Windows), which feeds the visualizer and the spectrum while the engine
//! plays it at volume 0, or, without a capture, silence paced by Soloist's
//! own position.
//!
//! Soloist has no "track ended" event. The end is read from its state: the
//! item changes to another one (it went on by itself: it is paused at once,
//! our queue decides what comes next), it goes idle, or it stops at the end
//! of the track without having been paused by us.
//!
//! The engine reads on its decoder thread every few milliseconds while a
//! read is [`PcmRead::Pending`]; nothing here blocks.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nikgob_audio::{PcmError, PcmRead, PcmSource};

use crate::manager::LiveView;
use crate::protocol::{Command, Status};

pub const RATE: u32 = 48_000;
pub const CHANNELS: u16 = 2;
/// How long Soloist may take to start a track.
const START_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a seek may take to show in Soloist's position.
const SEEK_TIMEOUT: Duration = Duration::from_secs(4);
/// Captured sound held before the first frames go to the engine: enough
/// for the capture's 10 ms packets to arrive unevenly without the engine
/// running dry, little enough to keep the copy close to what is heard.
pub const PREBUFFER_MS: u64 = 40;
/// More than this much waiting means the clocks drifted (Soloist's output
/// and ours are different devices): it is cut back to the prebuffer.
pub const DRIFT_MS: u64 = 160;
/// A pause this close to the track's end is its end.
const END_SLACK_MS: u64 = 1500;

/// What a track needs of Soloist (the manager; a fake in tests).
pub trait Link: Send + Sync + 'static {
    fn snapshot(&self) -> LiveView;
    fn send(&self, c: Command) -> bool;
    /// Moves captured samples into `out`; `None` when there is no capture.
    fn take_captured(&self, out: &mut [f32]) -> Option<usize>;
    fn buffered_frames(&self) -> Option<usize>;
    fn clear_captured(&self);
    /// Drops all but the newest `frames` captured frames.
    fn keep_captured(&self, frames: usize);
    /// The gain the capture went through after Soloist (its session level).
    fn capture_gain(&self) -> f32;
    /// The engine paused Soloist (a pause near the end is not an end then).
    fn paused_by_us(&self) -> bool;
    /// A track starts (+1) or is done (-1).
    fn playing(&self, delta: i32);
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Unrequested,
    Requested { at: Instant, changes: u64 },
    Playing,
    Seeking { target_ms: u64, at: Instant },
    Ended,
}

pub struct SoloistTrack {
    link: Arc<dyn Link>,
    uri: String,
    duration: Option<Duration>,
    phase: Phase,
    counted: bool,
    prebuffered: bool,
    /// Without a capture: Soloist's position where silence counting began,
    /// and the frames of silence given since.
    base_ms: u64,
    given: u64,
    /// A seek asked for before the track started.
    pending_seek: Option<u64>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn err(text: &str) -> PcmError {
    text.to_owned().into()
}

impl SoloistTrack {
    /// `uri`: `spotify:track:…`; `duration`: the length the library knows.
    pub fn new(link: Arc<dyn Link>, uri: String, duration: Option<Duration>) -> Self {
        Self {
            link,
            uri,
            duration,
            phase: Phase::Unrequested,
            counted: false,
            prebuffered: false,
            base_ms: 0,
            given: 0,
            pending_seek: None,
        }
    }

    fn position_ms(view: &LiveView) -> u64 {
        view.anchor.map_or(0, |a| a.at(now_ms()))
    }

    fn duration_ms(&self, view: &LiveView) -> Option<u64> {
        view.duration_ms.or_else(|| self.duration.map(|d| d.as_millis() as u64))
    }

    fn ours(&self, view: &LiveView) -> bool {
        view.uri.as_deref() == Some(self.uri.as_str())
    }

    fn start_playing(&mut self, view: &LiveView) {
        // What was captured before is the track before ours.
        self.link.clear_captured();
        self.phase = Phase::Playing;
        self.prebuffered = false;
        self.base_ms = Self::position_ms(view);
        self.given = 0;
        if let Some(ms) = self.pending_seek.take() {
            self.seek_to(ms);
        }
    }

    fn seek_to(&mut self, ms: u64) {
        self.link.send(Command::Seek(ms));
        self.link.clear_captured();
        self.phase = Phase::Seeking { target_ms: ms, at: Instant::now() };
    }

    /// The track is over by Soloist's state.
    fn ended(&self, view: &LiveView) -> bool {
        if !self.ours(view) || view.status == Status::Idle {
            return true;
        }
        if view.status == Status::Paused && !self.link.paused_by_us() {
            let pos = Self::position_ms(view);
            return self.duration_ms(view).is_some_and(|d| pos + END_SLACK_MS >= d);
        }
        false
    }

    fn produce(&mut self, view: &LiveView, out: &mut [f32]) -> PcmRead {
        let ch = usize::from(CHANNELS);
        let want = out.len() / ch;
        match self.link.buffered_frames() {
            Some(buffered) => {
                let pre = (PREBUFFER_MS * u64::from(RATE) / 1000) as usize;
                if !self.prebuffered {
                    if buffered < pre {
                        return PcmRead::Pending;
                    }
                    self.prebuffered = true;
                }
                if buffered > (DRIFT_MS * u64::from(RATE) / 1000) as usize {
                    self.link.keep_captured(pre);
                }
                let n = self.link.take_captured(&mut out[..want * ch]).unwrap_or(0) / ch * ch;
                if n == 0 {
                    return PcmRead::Pending;
                }
                // The capture comes after the session level; the visualizer
                // should not shrink with the volume.
                let g = self.link.capture_gain();
                if g > 1e-4 && g < 0.999 {
                    let k = 1.0 / g;
                    for s in &mut out[..n] {
                        *s = (*s * k).clamp(-1.0, 1.0);
                    }
                }
                PcmRead::Frames(n / ch)
            }
            None => {
                // Silence as long as Soloist's position moved on.
                let moved = Self::position_ms(view).saturating_sub(self.base_ms);
                let due = moved * u64::from(RATE) / 1000;
                let n = due.saturating_sub(self.given).min(want as u64) as usize;
                if n == 0 {
                    return PcmRead::Pending;
                }
                out[..n * ch].fill(0.0);
                self.given += n as u64;
                PcmRead::Frames(n)
            }
        }
    }
}

impl PcmSource for SoloistTrack {
    fn sample_rate(&self) -> u32 {
        RATE
    }

    fn channels(&self) -> u16 {
        CHANNELS
    }

    fn duration(&self) -> Option<Duration> {
        self.duration
    }

    fn read(&mut self, out: &mut [f32]) -> Result<PcmRead, PcmError> {
        let view = self.link.snapshot();
        match self.phase {
            Phase::Ended => return Ok(PcmRead::End),
            Phase::Unrequested => {
                if !view.ready {
                    return Err(err("Spotify Soloist не подключён"));
                }
                if !view.active {
                    self.link.send(Command::Activate);
                }
                if !self.link.send(Command::Play(Some(self.uri.clone()))) {
                    return Err(err("Spotify Soloist не подключён"));
                }
                self.link.clear_captured();
                self.link.playing(1);
                self.counted = true;
                self.phase = Phase::Requested { at: Instant::now(), changes: view.track_changes };
                return Ok(PcmRead::Pending);
            }
            Phase::Requested { at, changes } => {
                if !view.ready {
                    return Err(err("Spotify Soloist отключился"));
                }
                if let Some((m, when)) = &view.error
                    && *when >= at
                {
                    return Err(format!("Spotify Soloist: {m}").into());
                }
                // The state may still show the track before ours (or ours
                // from before): it counts once an item came after the request.
                let fresh = view.track_changes > changes || at.elapsed() > Duration::from_secs(2);
                if fresh && self.ours(&view) && view.status == Status::Playing {
                    self.start_playing(&view);
                } else if at.elapsed() > START_TIMEOUT {
                    return Err(err("Spotify Soloist не начал трек"));
                } else {
                    return Ok(PcmRead::Pending);
                }
            }
            Phase::Seeking { target_ms, at } => {
                if !view.ready {
                    return Err(err("Spotify Soloist отключился"));
                }
                let pos = Self::position_ms(&view);
                let there = self.ours(&view) && view.status == Status::Playing && pos.abs_diff(target_ms) < 2500;
                if there || at.elapsed() > SEEK_TIMEOUT {
                    self.link.clear_captured();
                    self.phase = Phase::Playing;
                    self.prebuffered = false;
                    self.base_ms = if there { pos } else { target_ms };
                    self.given = 0;
                } else {
                    return Ok(PcmRead::Pending);
                }
            }
            Phase::Playing => {}
        }
        if self.phase != Phase::Playing {
            return Ok(PcmRead::Pending);
        }
        if !view.ready {
            return Err(err("Spotify Soloist отключился"));
        }
        if self.ended(&view) {
            if !self.ours(&view) && view.status == Status::Playing {
                // It went on by itself (autoplay, its queue): ours decides.
                self.link.send(Command::Pause);
            }
            self.phase = Phase::Ended;
            return Ok(PcmRead::End);
        }
        Ok(self.produce(&view, out))
    }

    fn seek(&mut self, position: Duration) -> bool {
        let ms = position.as_millis() as u64;
        match self.phase {
            Phase::Ended => false,
            Phase::Unrequested | Phase::Requested { .. } => {
                self.pending_seek = Some(ms);
                true
            }
            Phase::Playing | Phase::Seeking { .. } => {
                self.seek_to(ms);
                true
            }
        }
    }
}

impl Drop for SoloistTrack {
    fn drop(&mut self) {
        if matches!(self.phase, Phase::Requested { .. } | Phase::Playing | Phase::Seeking { .. }) {
            let view = self.link.snapshot();
            if self.ours(&view) && view.status != Status::Paused {
                self.link.send(Command::Pause);
            }
        }
        if self.counted {
            self.link.playing(-1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Anchor;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Fake {
        view: Mutex<LiveView>,
        sent: Mutex<Vec<Command>>,
        captured: Mutex<Option<Vec<f32>>>,
        gain: Mutex<f32>,
        paused_by_us: Mutex<bool>,
        playing: Mutex<i32>,
    }

    impl Fake {
        fn new(capture: bool) -> Arc<Self> {
            let f = Self::default();
            *f.gain.lock().unwrap() = 1.0;
            if capture {
                *f.captured.lock().unwrap() = Some(Vec::new());
            }
            f.view.lock().unwrap().ready = true;
            Arc::new(f)
        }
        fn set(&self, edit: impl FnOnce(&mut LiveView)) {
            edit(&mut self.view.lock().unwrap());
        }
        fn sent(&self) -> Vec<Command> {
            std::mem::take(&mut self.sent.lock().unwrap())
        }
        fn capture(&self, frames: usize, value: f32) {
            self.captured.lock().unwrap().as_mut().unwrap().extend(std::iter::repeat_n(value, frames * 2));
        }
        /// Soloist plays `uri` from `ms`, moving.
        fn plays(&self, uri: &str, ms: u64) {
            self.set(|v| {
                if v.uri.as_deref() != Some(uri) {
                    v.track_changes += 1;
                }
                v.uri = Some(uri.into());
                v.status = Status::Playing;
                v.duration_ms = Some(200_000);
                v.anchor = Some(Anchor { position_ms: ms, timestamp_ms: now_ms(), speed: 1.0 });
            });
        }
    }

    impl Link for Fake {
        fn snapshot(&self) -> LiveView {
            self.view.lock().unwrap().clone()
        }
        fn send(&self, c: Command) -> bool {
            self.sent.lock().unwrap().push(c);
            true
        }
        fn take_captured(&self, out: &mut [f32]) -> Option<usize> {
            let mut c = self.captured.lock().unwrap();
            let c = c.as_mut()?;
            let n = out.len().min(c.len());
            out[..n].copy_from_slice(&c[..n]);
            c.drain(..n);
            Some(n)
        }
        fn buffered_frames(&self) -> Option<usize> {
            self.captured.lock().unwrap().as_ref().map(|c| c.len() / 2)
        }
        fn clear_captured(&self) {
            if let Some(c) = self.captured.lock().unwrap().as_mut() {
                c.clear();
            }
        }
        fn keep_captured(&self, frames: usize) {
            if let Some(c) = self.captured.lock().unwrap().as_mut()
                && c.len() > frames * 2
            {
                let extra = c.len() - frames * 2;
                c.drain(..extra);
            }
        }
        fn capture_gain(&self) -> f32 {
            *self.gain.lock().unwrap()
        }
        fn paused_by_us(&self) -> bool {
            *self.paused_by_us.lock().unwrap()
        }
        fn playing(&self, delta: i32) {
            *self.playing.lock().unwrap() += delta;
        }
    }

    const URI: &str = "spotify:track:ours";

    fn track(f: &Arc<Fake>) -> SoloistTrack {
        SoloistTrack::new(f.clone(), URI.into(), Some(Duration::from_secs(200)))
    }

    #[test]
    fn the_first_read_plays_the_track_and_waits_for_it() {
        let f = Fake::new(true);
        f.plays("spotify:track:before", 10_000);
        let mut t = track(&f);
        let mut buf = vec![0f32; 960];
        assert_eq!(t.read(&mut buf).unwrap(), PcmRead::Pending);
        assert_eq!(f.sent(), [Command::Activate, Command::Play(Some(URI.into()))]);
        assert_eq!(*f.playing.lock().unwrap(), 1);
        // The track before still shows: nothing yet.
        f.capture(2400, 0.1);
        assert_eq!(t.read(&mut buf).unwrap(), PcmRead::Pending);
        f.plays(URI, 0);
        assert_eq!(t.read(&mut buf).unwrap(), PcmRead::Pending, "started: the old capture is dropped");
        f.capture(1000, 0.2);
        // Below the prebuffer (40 ms = 1920 frames), then it flows.
        assert_eq!(t.read(&mut buf).unwrap(), PcmRead::Pending);
        f.capture(1000, 0.2);
        assert_eq!(t.read(&mut buf).unwrap(), PcmRead::Frames(480));
        assert!((buf[0] - 0.2).abs() < 1e-6);
        drop(t);
        assert_eq!(f.sent(), [Command::Pause], "a track dropped while playing pauses Soloist");
        assert_eq!(*f.playing.lock().unwrap(), 0);
    }

    #[test]
    fn another_item_or_idle_ends_the_track_and_autoplay_is_paused() {
        let f = Fake::new(true);
        let mut t = track(&f);
        let mut buf = vec![0f32; 960];
        t.read(&mut buf).unwrap();
        f.plays(URI, 0);
        t.read(&mut buf).unwrap();
        f.capture(4000, 0.1);
        assert!(matches!(t.read(&mut buf).unwrap(), PcmRead::Frames(_)));
        f.sent();
        f.plays("spotify:track:autoplay", 0);
        assert_eq!(t.read(&mut buf).unwrap(), PcmRead::End);
        assert_eq!(f.sent(), [Command::Pause]);
        assert_eq!(t.read(&mut buf).unwrap(), PcmRead::End);
        drop(t);
        assert!(f.sent().is_empty(), "an ended track does not pause again");

        let f = Fake::new(false);
        let mut t = track(&f);
        t.read(&mut buf).unwrap();
        f.plays(URI, 0);
        t.read(&mut buf).unwrap();
        f.set(|v| v.status = Status::Idle);
        assert_eq!(t.read(&mut buf).unwrap(), PcmRead::End);
    }

    #[test]
    fn a_pause_at_the_end_is_the_end_unless_we_paused() {
        let f = Fake::new(false);
        let mut t = track(&f);
        let mut buf = vec![0f32; 960];
        t.read(&mut buf).unwrap();
        f.plays(URI, 0);
        t.read(&mut buf).unwrap();
        f.set(|v| {
            v.status = Status::Paused;
            v.anchor = Some(Anchor { position_ms: 199_500, timestamp_ms: now_ms(), speed: 0.0 });
        });
        *f.paused_by_us.lock().unwrap() = true;
        assert_ne!(t.read(&mut buf).unwrap(), PcmRead::End, "our own pause");
        *f.paused_by_us.lock().unwrap() = false;
        assert_eq!(t.read(&mut buf).unwrap(), PcmRead::End);
    }

    #[test]
    fn without_a_capture_silence_follows_soloists_position() {
        let f = Fake::new(false);
        let mut t = track(&f);
        let mut buf = vec![1f32; 48_000 * 2];
        t.read(&mut buf).unwrap();
        f.plays(URI, 0);
        // Just started: nothing has moved yet.
        let first = t.read(&mut buf).unwrap();
        assert!(matches!(first, PcmRead::Pending | PcmRead::Frames(0..=480)), "{first:?}");
        // 250 ms later (moved back in the anchor instead of sleeping).
        f.set(|v| v.anchor = Some(Anchor { position_ms: 0, timestamp_ms: now_ms() - 250, speed: 1.0 }));
        let PcmRead::Frames(n) = t.read(&mut buf).unwrap() else { panic!() };
        let total = n + if let PcmRead::Frames(m) = first { m } else { 0 };
        assert!((11_500..=12_600).contains(&total), "{total}");
        assert!(buf[..n * 2].iter().all(|s| *s == 0.0));
    }

    #[test]
    fn seeks_go_to_soloist_and_wait_for_its_position() {
        let f = Fake::new(true);
        let mut t = track(&f);
        let mut buf = vec![0f32; 960];
        assert!(t.seek(Duration::from_secs(30)), "before the start: kept");
        t.read(&mut buf).unwrap();
        f.sent();
        f.plays(URI, 0);
        assert_eq!(t.read(&mut buf).unwrap(), PcmRead::Pending);
        assert_eq!(f.sent(), [Command::Seek(30_000)]);
        f.capture(4000, 0.3);
        assert_eq!(t.read(&mut buf).unwrap(), PcmRead::Pending, "still at 0");
        f.plays(URI, 30_000);
        assert_eq!(t.read(&mut buf).unwrap(), PcmRead::Pending, "there: what came before is dropped");
        f.capture(4000, 0.3);
        assert!(matches!(t.read(&mut buf).unwrap(), PcmRead::Frames(_)));
        assert!(t.seek(Duration::from_secs(60)));
        assert_eq!(f.sent(), [Command::Seek(60_000)]);
        assert_eq!(f.buffered_frames(), Some(0), "the captured sound before the seek is dropped");
    }

    #[test]
    fn refusals_disconnects_and_drift() {
        let f = Fake::new(true);
        let mut t = track(&f);
        let mut buf = vec![0f32; 960];
        t.read(&mut buf).unwrap();
        f.set(|v| v.error = Some(("command requires authentication".into(), Instant::now())));
        let e = t.read(&mut buf).unwrap_err();
        assert!(e.to_string().contains("command requires authentication"));

        let f = Fake::new(true);
        let mut t = track(&f);
        t.read(&mut buf).unwrap();
        f.plays(URI, 0);
        t.read(&mut buf).unwrap();
        f.capture(48_000, 0.1);
        assert!(matches!(t.read(&mut buf).unwrap(), PcmRead::Frames(_)));
        let left = f.buffered_frames().unwrap();
        assert!(left <= 1920, "a second of backlog is cut to the prebuffer: {left}");
        f.set(|v| v.ready = false);
        assert!(t.read(&mut buf).is_err());

        let f = Fake::new(true);
        f.set(|v| v.ready = false);
        assert!(track(&f).read(&mut buf).is_err(), "not connected");
    }

    #[test]
    fn the_visualizer_copy_keeps_its_level_at_a_lower_volume() {
        let f = Fake::new(true);
        *f.gain.lock().unwrap() = 0.125;
        let mut t = track(&f);
        let mut buf = vec![0f32; 960];
        t.read(&mut buf).unwrap();
        f.plays(URI, 0);
        t.read(&mut buf).unwrap();
        f.capture(4000, 0.05);
        assert!(matches!(t.read(&mut buf).unwrap(), PcmRead::Frames(_)));
        assert!((buf[0] - 0.4).abs() < 1e-5);
    }
}
