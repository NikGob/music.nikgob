//! One Spotify track played through Soloist, as a live [`PcmSource`] of
//! our engine.
//!
//! Soloist plays the track; the source hands the engine Soloist's sound as
//! captured from its process (Windows), and that copy is the one heard:
//! Soloist's own output is turned down to an inaudible level the capture
//! undoes (see [`crate::route::ROUTED_LEVEL`]), so the engine's volume,
//! normalization, visualizer and output device apply as to any track. The
//! queue, the position, the end of the track and errors flow as for any
//! other source. Without a capture the source gives silence paced by
//! Soloist's own position, and Soloist is heard itself.
//!
//! The level is undone only once Soloist's session is known to be at it;
//! until then, and whenever the captured sound is louder than that level
//! allows (the session is at another level: a new session, the volume
//! mixer), the copy is silence.
//!
//! Soloist has no "track ended" event. The end is read from its state: the
//! item changes to another one (it went on by itself: it is paused at once,
//! our queue decides what comes next), it goes idle, it stops (or goes back
//! to the start) after reaching the end of the track without having been
//! paused by us, or its position runs past the end.
//!
//! The engine reads on its decoder thread every few milliseconds while a
//! read is [`PcmRead::Pending`]; nothing here blocks.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nikgob_audio::{PcmError, PcmRead, PcmSource};

use crate::manager::LiveView;
use crate::protocol::{Command, Status};
pub use crate::route::{CHANNELS, Captured, DRIFT_MS, PREBUFFER_MS, RATE};
use crate::route::Feed;

/// How long Soloist may take to start a track.
const START_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a seek may take to show in Soloist's position.
const SEEK_TIMEOUT: Duration = Duration::from_secs(4);
/// With no sign that Soloist started our request, the state counts as
/// ours after this long.
const FRESH_FALLBACK: Duration = Duration::from_secs(2);
/// A position this close to the start, sent after our request, is the
/// start of what we asked for (the same track again included).
const NEAR_START_MS: u64 = 1500;
/// A pause this close to the track's end is its end.
const END_SLACK_MS: u64 = 1500;
/// A position this far past the track's end is its end, whatever the
/// status says.
const OVERRUN_MS: u64 = 3000;
/// Back this far from the furthest point after reaching the end, without
/// a seek of ours: it went round to the start, the track is over.
const REWIND_MS: u64 = 10_000;

/// What a track needs of Soloist (the manager; a fake in tests), besides
/// its capture.
pub trait Link: Captured + 'static {
    fn snapshot(&self) -> LiveView;
    fn send(&self, c: Command) -> bool;
    /// The engine paused Soloist (a pause near the end is not an end then).
    fn paused_by_us(&self) -> bool;
    /// A track starts (+1) or is done (-1).
    fn playing(&self, delta: i32);
    /// Called while the track starts: Soloist's output gets its level.
    fn prepare_output(&self) {}
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
    /// The sound for the engine.
    feed: Feed,
    /// The furthest position seen while playing.
    max_pos: u64,
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

/// The refusal that answers our `play` asked at `at`: a refusal of another
/// command, or one after the play was accepted, is not.
fn refusal(view: &LiveView, at: Instant) -> Option<&str> {
    let (message, when) = view.error.as_ref()?;
    if *when < at {
        return None;
    }
    let ours = match view.error_command {
        Some(c) => c == "play",
        None => !view.play_accepted.is_some_and(|p| p >= at && p <= *when),
    };
    ours.then_some(message.as_str())
}

/// Soloist (re)started something after our request at `at`: a position
/// near the start came after it, or a position came after the play was
/// accepted. This is how the same track played again is told from the
/// play before.
fn restarted(view: &LiveView, at: Instant) -> bool {
    let Some(anchor_at) = view.anchor_at.filter(|t| *t >= at) else {
        return false;
    };
    let near_start = view.anchor.is_some_and(|a| a.position_ms < NEAR_START_MS);
    let after_accept = view.play_accepted.is_some_and(|p| p >= at && anchor_at >= p);
    near_start || after_accept
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
            feed: Feed::new("soloist"),
            max_pos: 0,
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

    /// Counting starts again from Soloist's position `ms` (a start, a seek,
    /// a switch between the capture and silence).
    fn restart_count(&mut self, ms: u64) {
        self.feed.restart(ms);
    }

    fn start_playing(&mut self, view: &LiveView) {
        // What was captured before is the track before ours.
        self.link.clear_captured();
        self.phase = Phase::Playing;
        let pos = Self::position_ms(view);
        self.restart_count(pos);
        self.max_pos = pos;
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
    fn ended(&mut self, view: &LiveView) -> bool {
        if !self.ours(view) || view.status == Status::Idle {
            return true;
        }
        let pos = Self::position_ms(view);
        if view.status == Status::Playing {
            self.max_pos = self.max_pos.max(pos);
        }
        let Some(d) = self.duration_ms(view) else { return false };
        let reached = self.max_pos.max(pos) + END_SLACK_MS >= d;
        match view.status {
            // It stops at the end, where it is or back at the start.
            Status::Paused => reached && !self.link.paused_by_us(),
            // Past the end (no word from it since), or round to the start.
            Status::Playing => pos >= d + OVERRUN_MS || (reached && pos + REWIND_MS < self.max_pos),
            Status::Buffering | Status::Idle => false,
        }
    }

    fn produce(&mut self, view: &LiveView, out: &mut [f32]) -> PcmRead {
        self.feed.produce(Self::position_ms(view), &*self.link, out)
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
                self.link.prepare_output();
                if !view.ready {
                    return Err(err("Spotify Soloist отключился"));
                }
                if let Some(m) = refusal(&view, at) {
                    return Err(format!("Spotify Soloist: {m}").into());
                }
                // The state may still show the track before ours (or ours
                // from before): it counts once an item came after the
                // request, or a restart of ours shows.
                let fresh = view.track_changes > changes || restarted(&view, at) || at.elapsed() > FRESH_FALLBACK;
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
                    let from = if there { pos } else { target_ms };
                    self.restart_count(from);
                    self.max_pos = from;
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
        self.link.prepare_output();
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
        wrong: Mutex<u32>,
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
                v.anchor_at = Some(Instant::now());
            });
        }
        /// Soloist's position is `ms` (moving while it plays), as of now.
        fn at(&self, ms: u64) {
            self.set(|v| {
                let speed = if v.status == Status::Playing { 1.0 } else { 0.0 };
                v.anchor = Some(Anchor { position_ms: ms, timestamp_ms: now_ms(), speed });
                v.anchor_at = Some(Instant::now());
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
        fn paused_by_us(&self) -> bool {
            *self.paused_by_us.lock().unwrap()
        }
        fn playing(&self, delta: i32) {
            *self.playing.lock().unwrap() += delta;
        }
    }

    impl Captured for Fake {
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
        fn session_wrong(&self) {
            *self.wrong.lock().unwrap() += 1;
        }
    }

    const URI: &str = "spotify:track:ours";

    fn track(f: &Arc<Fake>) -> SoloistTrack {
        SoloistTrack::new(f.clone(), URI.into(), Some(Duration::from_secs(200)))
    }

    /// A track of `f`, requested and started at 0, its capture flowing.
    fn started(f: &Arc<Fake>) -> SoloistTrack {
        let mut t = track(f);
        let mut buf = vec![0f32; 960];
        t.read(&mut buf).unwrap();
        f.plays(URI, 0);
        t.read(&mut buf).unwrap();
        f.sent();
        t
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
    fn the_same_track_again_starts_on_its_new_position() {
        let f = Fake::new(true);
        f.plays(URI, 90_000);
        let mut t = track(&f);
        let mut buf = vec![0f32; 960];
        t.read(&mut buf).unwrap();
        f.capture(4000, 0.1);
        assert_eq!(t.read(&mut buf).unwrap(), PcmRead::Pending, "still the play before, at 1:30");
        f.at(90_050);
        assert_eq!(t.read(&mut buf).unwrap(), PcmRead::Pending, "a position of the play before");
        f.at(0);
        assert_eq!(t.read(&mut buf).unwrap(), PcmRead::Pending, "started again: the old capture is dropped");
        assert_eq!(f.buffered_frames(), Some(0));
        f.capture(4000, 0.1);
        assert!(matches!(t.read(&mut buf).unwrap(), PcmRead::Frames(_)));

        // A position after the play was accepted counts too.
        let f = Fake::new(true);
        f.plays(URI, 90_000);
        let mut t = track(&f);
        t.read(&mut buf).unwrap();
        f.set(|v| v.play_accepted = Some(Instant::now()));
        assert_eq!(t.read(&mut buf).unwrap(), PcmRead::Pending);
        f.at(3_000);
        t.read(&mut buf).unwrap();
        f.capture(4000, 0.1);
        assert!(matches!(t.read(&mut buf).unwrap(), PcmRead::Frames(_)));
    }

    #[test]
    fn another_item_or_idle_ends_the_track_and_autoplay_is_paused() {
        let f = Fake::new(true);
        let mut t = started(&f);
        let mut buf = vec![0f32; 960];
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
        let mut t = started(&f);
        f.set(|v| v.status = Status::Idle);
        assert_eq!(t.read(&mut buf).unwrap(), PcmRead::End);
    }

    #[test]
    fn a_pause_at_the_end_is_the_end_unless_we_paused() {
        let f = Fake::new(false);
        let mut t = started(&f);
        let mut buf = vec![0f32; 960];
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
    fn the_end_shows_after_a_rewind_or_a_position_past_it() {
        let mut buf = vec![0f32; 960];
        // It reached the end, then paused back at the start.
        let f = Fake::new(false);
        let mut t = started(&f);
        f.at(199_200);
        assert_ne!(t.read(&mut buf).unwrap(), PcmRead::End);
        f.set(|v| v.status = Status::Paused);
        f.at(0);
        assert_eq!(t.read(&mut buf).unwrap(), PcmRead::End);

        // ... or went round to the start, still playing.
        let f = Fake::new(false);
        let mut t = started(&f);
        f.at(199_200);
        t.read(&mut buf).unwrap();
        f.at(200);
        assert_eq!(t.read(&mut buf).unwrap(), PcmRead::End);

        // A pause in the middle and back to the start is not the end.
        let f = Fake::new(false);
        let mut t = started(&f);
        f.at(60_000);
        t.read(&mut buf).unwrap();
        f.set(|v| v.status = Status::Paused);
        f.at(0);
        assert_ne!(t.read(&mut buf).unwrap(), PcmRead::End);

        // No word from it while its position runs past the end.
        let f = Fake::new(false);
        let mut t = started(&f);
        f.at(203_500);
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
    fn a_capture_lost_mid_track_counts_silence_from_there() {
        let f = Fake::new(true);
        let mut t = started(&f);
        let mut buf = vec![1f32; 48_000 * 2];
        f.capture(4000, 0.1);
        assert!(matches!(t.read(&mut buf).unwrap(), PcmRead::Frames(_)));
        // A minute on, the capture is gone.
        *f.captured.lock().unwrap() = None;
        f.at(60_000);
        let r = t.read(&mut buf).unwrap();
        assert!(matches!(r, PcmRead::Pending | PcmRead::Frames(0..=480)), "not a minute of silence at once: {r:?}");
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
    fn only_a_refusal_of_the_play_fails_the_track() {
        let mut buf = vec![0f32; 960];
        // Of another command (activate goes out first).
        let f = Fake::new(true);
        let mut t = track(&f);
        t.read(&mut buf).unwrap();
        f.set(|v| {
            v.error = Some(("no active device".into(), Instant::now()));
            v.error_command = Some("activate");
        });
        assert_eq!(t.read(&mut buf).unwrap(), PcmRead::Pending);

        // After the play was accepted, of something unknown.
        let f = Fake::new(true);
        let mut t = track(&f);
        t.read(&mut buf).unwrap();
        f.set(|v| v.play_accepted = Some(Instant::now()));
        f.set(|v| v.error = Some(("rate limited".into(), Instant::now())));
        assert_eq!(t.read(&mut buf).unwrap(), PcmRead::Pending);

        // Of the play itself.
        let f = Fake::new(true);
        let mut t = track(&f);
        t.read(&mut buf).unwrap();
        f.set(|v| {
            v.error = Some(("track unavailable".into(), Instant::now()));
            v.error_command = Some("play");
        });
        assert!(t.read(&mut buf).unwrap_err().to_string().contains("track unavailable"));
    }

    #[test]
    fn the_visualizer_copy_keeps_its_level_at_a_lower_volume() {
        let f = Fake::new(true);
        *f.gain.lock().unwrap() = 0.125;
        let mut t = started(&f);
        let mut buf = vec![0f32; 960];
        f.capture(4000, 0.05);
        assert!(matches!(t.read(&mut buf).unwrap(), PcmRead::Frames(_)));
        assert!((buf[0] - 0.4).abs() < 1e-5);
    }

    #[test]
    fn the_routed_level_is_undone_only_when_the_sound_fits_it() {
        let mut buf = vec![1f32; 960];
        // The session level is not known yet: silence.
        let f = Fake::new(true);
        *f.gain.lock().unwrap() = 0.0;
        let mut t = started(&f);
        f.capture(4000, 0.3);
        assert!(matches!(t.read(&mut buf).unwrap(), PcmRead::Frames(_)));
        assert!(buf.iter().all(|s| *s == 0.0));

        // At 1e-6: scaled back.
        let f = Fake::new(true);
        *f.gain.lock().unwrap() = 1e-6;
        let mut t = started(&f);
        f.capture(4000, 2e-7);
        assert!(matches!(t.read(&mut buf).unwrap(), PcmRead::Frames(_)));
        assert!((buf[0] - 0.2).abs() < 1e-4, "{}", buf[0]);
        assert_eq!(*f.wrong.lock().unwrap(), 0);

        // Louder than 1e-6 allows (the session is at another level): silence,
        // and the level is set again.
        f.clear_captured();
        f.capture(4000, 0.01);
        buf.fill(1.0);
        assert!(matches!(t.read(&mut buf).unwrap(), PcmRead::Frames(_)));
        assert!(buf.iter().all(|s| *s == 0.0));
        assert!(*f.wrong.lock().unwrap() > 0);
    }
}
