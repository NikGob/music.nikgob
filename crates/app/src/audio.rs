//! Bridges the audio engine (`nikgob-audio`) to the runtime's `Playback`.
//!
//! Switching the output device restarts the engine: the old `Player` is
//! dropped (its threads stop and the device closes), a new one opens the
//! chosen device, and the runtime replays the current track from where it
//! was. Events and the analyzer go through shared slots so the runtime and
//! the window never hold a stale engine.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use nikgob_audio::{Analyzer, Event, ItemSource, PlayItem, Player, PlayerOptions};
use nikgob_runtime::{PlayRequest, Playback, PlaybackEvent};

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
    /// Starts the engine on `device` (None = system default).
    pub fn start(device: Option<String>) -> Result<(Self, Links), String> {
        let links = Links::default();
        let player = spawn(device, &links)?;
        Ok((
            Self {
                player,
                links: links.clone(),
            },
            links,
        ))
    }
}

fn item(r: PlayRequest) -> PlayItem {
    PlayItem {
        id: r.id,
        source: ItemSource::File(r.path),
        gain_db: r.gain_db,
        duration_hint: r.duration,
    }
}

impl Playback for Engine {
    fn play(&mut self, req: PlayRequest) {
        self.player.play(item(req));
    }

    fn set_next(&mut self, next: Option<PlayRequest>) {
        match next {
            Some(r) => self.player.set_next(item(r)),
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
