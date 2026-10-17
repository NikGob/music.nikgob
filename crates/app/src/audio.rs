//! Bridges the audio engine (`nikgob-audio`) to the runtime's `Playback`.
//!
//! Stream tracks (Spotify, YouTube) are opened through the registered
//! [`StreamService`]s: the request becomes an [`ItemSource::Stream`] whose
//! opener calls `open_audio` on the engine's opener thread, and the
//! service's [`AudioStream`] is read like a file. Its loudness (when the
//! service gives one) replaces the item gain, so normalization covers
//! streams too.
//!
//! Switching the output device restarts the engine: the old `Player` is
//! dropped (its threads stop and the device closes), a new one opens the
//! chosen device, and the runtime replays the current track from where it
//! was. Events and the analyzer go through shared slots so the runtime and
//! the window never hold a stale engine.

use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use nikgob_audio::{Analyzer, ByteSource, Event, ItemSource, PlayItem, Player, PlayerOptions};
use nikgob_core::streaming::{AudioStream, StreamService};
use nikgob_runtime::{PlayRequest, PlaySource, Playback, PlaybackEvent};

/// The analyzer of the engine that plays right now.
pub type AnalyzerSlot = Arc<Mutex<Option<Analyzer>>>;

/// Shared by every engine generation.
#[derive(Clone, Default)]
pub struct Links {
    pub analyzer: AnalyzerSlot,
    events: Arc<Mutex<Option<Sender<PlaybackEvent>>>>,
    generation: Arc<AtomicU64>,
}

impl Links {
    /// Where engine events go (set once the runtime exists).
    pub fn connect(&self, to: Sender<PlaybackEvent>) {
        *self.events.lock().unwrap_or_else(PoisonError::into_inner) = Some(to);
    }
}

pub struct Engine {
    player: Player,
    links: Links,
    streams: Vec<Arc<dyn StreamService>>,
}

fn spawn(device: Option<String>, links: &Links) -> Result<Player, String> {
    let mut options = PlayerOptions::default();
    if let Some(d) = device {
        options.output.device = nikgob_audio::Device::Named(d);
    }
    let (player, events) = Player::spawn(options).map_err(|e| e.to_string())?;
    let generation = links.generation.fetch_add(1, Ordering::AcqRel) + 1;
    *links.analyzer.lock().unwrap_or_else(PoisonError::into_inner) = Some(player.analyzer());
    forward(events, links.clone(), generation);
    Ok(player)
}

impl Engine {
    /// Starts the engine on `device` (None = system default); stream
    /// requests play through `streams`.
    pub fn start(
        device: Option<String>,
        streams: Vec<Arc<dyn StreamService>>,
    ) -> Result<(Self, Links), String> {
        let links = Links::default();
        let player = spawn(device, &links)?;
        Ok((
            Self {
                player,
                links: links.clone(),
                streams,
            },
            links,
        ))
    }

    fn item(&self, r: PlayRequest) -> PlayItem {
        let source = match r.source {
            PlaySource::File(path) => ItemSource::File(path),
            PlaySource::Stream { service, id } => {
                let svc = self.streams.iter().find(|s| s.service() == service).cloned();
                let events = self.links.events.clone();
                let item = r.id;
                ItemSource::Stream(Box::new(move || {
                    let svc = svc.ok_or_else(|| format!("{service} is not available"))?;
                    log::debug!("opening {service} stream {id} (item {item})");
                    let stream = svc.open_audio(&id).map_err(|e| {
                        log::info!("{service} stream {id} did not open: {e}");
                        format!("{service}: {e}")
                    })?;
                    Ok(Box::new(StreamBytes {
                        inner: stream,
                        item,
                        what: format!("{service} stream {id}"),
                        opened: Instant::now(),
                        reported: false,
                        events,
                    }) as Box<dyn ByteSource>)
                }))
            }
        };
        PlayItem {
            id: r.id,
            source,
            gain_db: r.gain_db,
            duration_hint: r.duration,
        }
    }
}

/// A service's [`AudioStream`] as the engine's [`ByteSource`]. Its first
/// bytes are logged (time to first byte) and tell the runtime what really
/// plays ([`PlaybackEvent::Quality`]).
struct StreamBytes {
    inner: Box<dyn AudioStream>,
    item: u64,
    what: String,
    opened: Instant,
    reported: bool,
    events: Arc<Mutex<Option<Sender<PlaybackEvent>>>>,
}

impl StreamBytes {
    fn first_bytes(&mut self) {
        self.reported = true;
        let quality = self.inner.quality();
        log::debug!(
            "{}: first bytes after {} ms ({}, {:?}, {} bytes)",
            self.what,
            self.opened.elapsed().as_millis(),
            self.inner.format_hint(),
            quality,
            self.inner.byte_len().unwrap_or(0)
        );
        let to = self.events.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let (Some(label), Some(to)) = (quality, to) {
            let _ = to.send(PlaybackEvent::Quality { id: self.item, label });
        }
    }
}

impl Read for StreamBytes {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf).inspect_err(|e| {
            if !self.reported {
                log::info!("{}: failed after {} ms: {e}", self.what, self.opened.elapsed().as_millis());
            }
        })?;
        if n > 0 && !self.reported {
            self.first_bytes();
        }
        Ok(n)
    }
}

impl Seek for StreamBytes {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

impl ByteSource for StreamBytes {
    fn byte_len(&self) -> Option<u64> {
        self.inner.byte_len()
    }

    fn format_hint(&self) -> &str {
        self.inner.format_hint()
    }

    fn gain_db(&self) -> Option<f32> {
        self.inner.gain_db()
    }

    fn duration(&self) -> Option<Duration> {
        self.inner.duration()
    }
}

impl Playback for Engine {
    fn play(&mut self, req: PlayRequest) {
        let item = self.item(req);
        self.player.play(item);
    }

    fn set_next(&mut self, next: Option<PlayRequest>) {
        match next {
            Some(r) => {
                let item = self.item(r);
                self.player.set_next(item);
            }
            None => self.player.clear_next(),
        }
    }

    fn pause(&mut self) {
        self.player.pause();
    }

    fn resume(&mut self) {
        self.player.resume();
    }

    fn stop(&mut self) {
        self.player.stop();
    }

    fn seek(&mut self, position: Duration) {
        self.player.seek(position);
    }

    fn set_volume(&mut self, volume: f32) {
        self.player.set_volume(volume);
    }

    fn position(&self) -> Duration {
        self.player.position().unwrap_or_default()
    }

    fn configure(&mut self, crossfade: Option<Duration>, normalization: bool) {
        self.player.set_crossfade(crossfade);
        self.player.set_normalization(normalization);
    }

    fn set_device(&mut self, device: Option<String>) -> bool {
        self.player.stop();
        match spawn(device, &self.links) {
            Ok(p) => {
                self.player = p;
                true
            }
            Err(e) => {
                log::error!("cannot switch the audio device: {e}");
                false
            }
        }
    }
}

/// Forwards engine events to the runtime while this engine is the current one.
fn forward(events: Receiver<Event>, links: Links, generation: u64) {
    std::thread::Builder::new()
        .name("nikgob-audio-events".into())
        .spawn(move || {
            for e in events {
                if links.generation.load(Ordering::Acquire) != generation {
                    break;
                }
                let mapped = match e {
                    Event::Started { id, .. } => Some(PlaybackEvent::Started { id }),
                    Event::TrackChanged { to, .. } => Some(PlaybackEvent::TrackChanged { to }),
                    Event::Position { id, pos } => Some(PlaybackEvent::Position { id, position: pos }),
                    Event::Paused => Some(PlaybackEvent::Paused),
                    Event::Resumed => Some(PlaybackEvent::Resumed),
                    Event::Ended { id } => Some(PlaybackEvent::Ended { id }),
                    Event::Error { id: Some(id), message } => Some(PlaybackEvent::Error { id, message }),
                    Event::Error { id: None, message } => {
                        log::error!("audio device: {message}");
                        None
                    }
                    Event::DeviceChanged { device, sample_rate, channels } => {
                        log::info!("audio output: {device} {sample_rate} Hz × {channels}");
                        None
                    }
                    Event::QueueEmpty | Event::Stopped => None,
                };
                let Some(m) = mapped else { continue };
                let to = links.events.lock().unwrap_or_else(PoisonError::into_inner).clone();
                if let Some(to) = to
                    && to.send(m).is_err()
                {
                    break;
                }
            }
        })
        .expect("spawn audio event thread");
}
