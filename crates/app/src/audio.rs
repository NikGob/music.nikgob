//! Bridges the audio engine (`nikgob-audio`) to the runtime's `Playback`.
//!
//! Stream tracks (Spotify, YouTube) are opened through the registered
//! [`StreamService`]s: the request becomes an [`ItemSource::Stream`] whose
//! opener calls `open_audio` on the engine's opener thread, and the
//! service's [`AudioStream`] is read like a file. Its loudness (when the
//! service gives one) replaces the item gain, so normalization covers
//! streams too.
//!
//! Spotify tracks play through Spotify Soloist while it is set up and
//! paired (`nikgob-soloist`): the item is a live PCM source driving Soloist
//! and handing the engine Soloist's captured sound, which is the copy heard
//! (Soloist's own output is turned down to an inaudible level), so volume,
//! normalization, visualizer and output device work as for any track.
//! Without a capture Soloist is heard itself: the engine plays the item at
//! volume 0 and the user's volume goes to Soloist. Nothing is preloaded into
//! or out of such a track (one player cannot overlap two tracks): a clean
//! cut.
//!
//! SoundCloud tracks it streams only with DRM go the same way through its
//! official player (`nikgob-sc-widget`, Windows): the first open of such a
//! track fails with [`nikgob_runtime::OFFICIAL_PLAYER_ROUTE`] in its error,
//! the track is remembered, and the runtime plays the same source again,
//! which is then a live source of the widget. A track the widget failed
//! keeps the old fallback (the runtime's next source, YouTube Music).
//!
//! The developer profile's tracks have no files (`profile.rs`): a missing
//! local file of a known length plays there as silence of that length
//! ([`Silence`]), so position, lyrics and the end of the track run as for a
//! real song; elsewhere a missing file fails as ever.
//!
//! Switching the output device restarts the engine: the old `Player` is
//! dropped (its threads stop and the device closes), a new one opens the
//! chosen device, and the runtime replays the current track from where it
//! was. Events and the analyzer go through shared slots so the runtime and
//! the window never hold a stale engine.

use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use nikgob_audio::{
    Analyzer, ByteSource, Event, ItemFx, ItemSource, PcmError, PcmRead, PcmSource, PlayItem, Player, PlayerOptions, Reverb,
};
use nikgob_core::streaming::{AudioStream, StreamError, StreamService};
use nikgob_core::streaming::Service as StreamKind;
use nikgob_runtime::{PlayRequest, PlaySource, Playback, PlaybackEvent};

/// The analyzer of the engine that plays right now.
pub type AnalyzerSlot = Arc<Mutex<Option<Analyzer>>>;

/// Missing files of a known length play as silence (the developer profile).
static SILENT_MISSING: AtomicBool = AtomicBool::new(false);

/// From now on a local file that is not there plays as silence of the
/// track's length (the developer profile, whose tracks have no files).
pub fn play_missing_files_silent() {
    SILENT_MISSING.store(true, Ordering::Relaxed);
}

/// Silence of a set length: a track of the developer profile without a file.
struct Silence {
    frames: u64,
    at: u64,
}

impl Silence {
    const RATE: u32 = 44_100;

    fn new(length: Duration) -> Self {
        Self { frames: (length.as_secs_f64() * f64::from(Self::RATE)) as u64, at: 0 }
    }
}

impl PcmSource for Silence {
    fn sample_rate(&self) -> u32 {
        Self::RATE
    }

    fn channels(&self) -> u16 {
        2
    }

    fn read(&mut self, out: &mut [f32]) -> Result<PcmRead, PcmError> {
        let left = self.frames.saturating_sub(self.at);
        if left == 0 {
            return Ok(PcmRead::End);
        }
        let n = (out.len() / 2).min(usize::try_from(left).unwrap_or(usize::MAX));
        out[..n * 2].fill(0.0);
        self.at += n as u64;
        Ok(PcmRead::Frames(n))
    }

    fn duration(&self) -> Option<Duration> {
        Some(Duration::from_secs_f64(self.frames as f64 / f64::from(Self::RATE)))
    }

    fn seek(&mut self, position: Duration) -> bool {
        self.at = ((position.as_secs_f64() * f64::from(Self::RATE)) as u64).min(self.frames);
        true
    }
}

/// What the engine does for a version of a track («Скорость»): its speed,
/// tone and reverb as the view names them, and whether the voices keep
/// their timbre.
fn item_fx(speed: nikgob_view::SpeedVariant, keep_timbre: bool) -> ItemFx {
    let s = speed.sound();
    ItemFx {
        speed: s.speed,
        low_shelf_db: s.low_shelf_db,
        high_shelf_db: s.high_shelf_db,
        preamp_db: s.preamp_db,
        reverb: s.echo.map(|e| Reverb {
            wet: e.wet,
            decay_s: e.decay_s,
            damping: e.damping,
            pre_delay_ms: e.pre_delay_ms,
        }),
        keep_formants: keep_timbre,
    }
}

/// The silence a request plays instead of its missing file, when that is on.
fn silence_for(r: &PlayRequest) -> Option<Silence> {
    let PlaySource::File(path) = &r.source else { return None };
    let length = r.duration.filter(|d| !d.is_zero())?;
    (SILENT_MISSING.load(Ordering::Relaxed) && !path.exists()).then(|| Silence::new(length))
}

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
    soloist: Option<Arc<nikgob_soloist::Soloist>>,
    /// SoundCloud's official player (Windows).
    widget: Option<Arc<nikgob_sc_widget::ScWidget>>,
    /// SoundCloud tracks found to be DRM-only this session.
    drm: Arc<Mutex<HashSet<String>>>,
    /// The user's volume (the engine plays a live track at 0 while its
    /// player is heard itself).
    volume: f32,
    /// What plays now is a live track (Soloist, SoundCloud's player).
    on_live: Option<Live>,
    /// ... and its player is heard itself (no capture): the engine is at 0.
    live_direct: bool,
}

/// A player whose sound a live track hands the engine.
#[derive(Clone)]
enum Live {
    Soloist(Arc<nikgob_soloist::Soloist>),
    Widget(Arc<nikgob_sc_widget::ScWidget>),
}

impl Live {
    fn routed(&self) -> bool {
        match self {
            Self::Soloist(s) => s.routed(),
            Self::Widget(w) => w.routed(),
        }
    }

    fn set_volume(&self, v: f32) {
        match self {
            Self::Soloist(s) => s.set_volume(v),
            Self::Widget(w) => w.set_volume(v),
        }
    }

    fn pause(&self) {
        match self {
            Self::Soloist(s) => s.pause(),
            Self::Widget(w) => w.pause(),
        }
    }

    fn resume(&self) {
        match self {
            Self::Soloist(s) => s.resume(),
            Self::Widget(w) => w.resume(),
        }
    }
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
        soloist: Option<Arc<nikgob_soloist::Soloist>>,
        widget: Option<Arc<nikgob_sc_widget::ScWidget>>,
    ) -> Result<(Self, Links), String> {
        let links = Links::default();
        let player = spawn(device, &links)?;
        Ok((
            Self {
                player,
                links: links.clone(),
                streams,
                soloist,
                widget,
                drm: Arc::default(),
                volume: 1.0,
                on_live: None,
                live_direct: false,
            },
            links,
        ))
    }

    /// The Soloist a request plays through: a Spotify track while Soloist
    /// is ready.
    fn soloist_for(&self, r: &PlayRequest) -> Option<&Arc<nikgob_soloist::Soloist>> {
        match &r.source {
            PlaySource::Stream { service: StreamKind::Spotify, .. } => self.soloist.as_ref().filter(|s| s.ready()),
            _ => None,
        }
    }

    /// SoundCloud's player for a request: a SoundCloud track known to be
    /// DRM-only that the player has not failed.
    fn widget_for(&self, r: &PlayRequest) -> Option<&Arc<nikgob_sc_widget::ScWidget>> {
        match &r.source {
            PlaySource::Stream { service: StreamKind::Soundcloud, id } => self
                .widget
                .as_ref()
                .filter(|w| w.plays(id) && self.drm.lock().unwrap_or_else(PoisonError::into_inner).contains(id)),
            _ => None,
        }
    }

    /// The live player a request plays through, if any.
    fn live_for(&self, r: &PlayRequest) -> Option<Live> {
        if let Some(s) = self.soloist_for(r) {
            return Some(Live::Soloist(s.clone()));
        }
        self.widget_for(r).map(|w| Live::Widget(w.clone()))
    }

    fn quality(&self, id: u64, label: &str) {
        let to = self.links.events.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(to) = to {
            let _ = to.send(PlaybackEvent::Quality { id, label: label.into() });
        }
    }

    /// A live track (another player's sound as it plays) cannot be sped
    /// up: it plays as recorded, and the runtime is told so.
    fn plays_live(&self, r: &PlayRequest) {
        if r.speed == nikgob_view::SpeedVariant::Normal {
            return;
        }
        let to = self.links.events.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(to) = to {
            let _ = to.send(PlaybackEvent::Speed { id: r.id, speed: 1.0 });
        }
    }

    fn item(&self, r: PlayRequest) -> PlayItem {
        if let (Some(w), PlaySource::Stream { id, .. }) = (self.widget_for(&r), &r.source) {
            log::debug!("soundcloud {id} plays through SoundCloud's player (item {})", r.id);
            self.quality(r.id, "Плеер SoundCloud");
            self.plays_live(&r);
            return PlayItem {
                id: r.id,
                source: ItemSource::Pcm(Box::new(w.track(id, r.duration))),
                gain_db: 0.0,
                duration_hint: r.duration,
                fx: ItemFx::default(),
            };
        }
        if let (Some(s), PlaySource::Stream { id, .. }) = (self.soloist_for(&r), &r.source) {
            log::debug!("spotify {id} plays through Soloist (item {})", r.id);
            let to = self.links.events.lock().unwrap_or_else(PoisonError::into_inner).clone();
            if let Some(to) = to {
                let _ = to.send(PlaybackEvent::Quality { id: r.id, label: "Soloist".into() });
            }
            self.plays_live(&r);
            return PlayItem {
                id: r.id,
                source: ItemSource::Pcm(Box::new(s.track(id, r.duration))),
                gain_db: 0.0,
                duration_hint: r.duration,
                fx: ItemFx::default(),
            };
        }
        if let Some(silence) = silence_for(&r) {
            log::debug!("item {}: no file, silence of {:?}", r.id, r.duration);
            return PlayItem {
                id: r.id,
                source: ItemSource::Pcm(Box::new(silence)),
                gain_db: 0.0,
                duration_hint: r.duration,
                fx: item_fx(r.speed, r.keep_timbre),
            };
        }
        let fx = item_fx(r.speed, r.keep_timbre);
        let source = match r.source {
            PlaySource::File(path) => ItemSource::File(path),
            PlaySource::Stream { service, id } => {
                let svc = self.streams.iter().find(|s| s.service() == service).cloned();
                let events = self.links.events.clone();
                let item = r.id;
                // A DRM-only SoundCloud track can go to SoundCloud's player.
                let widget = self.widget.clone().filter(|_| service == StreamKind::Soundcloud);
                let drm = self.drm.clone();
                ItemSource::Stream(Box::new(move || {
                    let svc = svc.ok_or_else(|| format!("{service} is not available"))?;
                    log::debug!("opening {service} stream {id} (item {item})");
                    let stream = svc.open_audio(&id).map_err(|e| {
                        log::info!("{service} stream {id} did not open: {e}");
                        if matches!(e, StreamError::Drm)
                            && let Some(w) = widget.as_ref().filter(|w| w.plays(&id))
                        {
                            drm.lock().unwrap_or_else(PoisonError::into_inner).insert(id.clone());
                            // Its play comes back at once: the host starts now.
                            w.warm();
                            return format!("{service}: {e} {}", nikgob_runtime::OFFICIAL_PLAYER_ROUTE);
                        }
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
            fx,
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
        let live = self.live_for(&req);
        if let Some(Live::Widget(w)) = &live {
            // Its host runs and its capture had a moment, so `routed`
            // answers for this track.
            w.prepare();
        }
        let direct = live.as_ref().is_some_and(|l| !l.routed());
        if let Some(l) = &live {
            l.set_volume(self.volume);
        }
        if direct {
            // The player is heard itself; the engine's copy only moves the
            // visualizer.
            self.player.set_volume(0.0);
        } else if self.live_direct {
            self.player.set_volume(self.volume);
        }
        self.on_live = live;
        self.live_direct = direct;
        let item = self.item(req);
        self.player.play(item);
    }

    fn set_next(&mut self, next: Option<PlayRequest>) {
        match next {
            // Into or out of a live track: a clean cut after `Ended`.
            Some(r) if self.on_live.is_some() || self.live_for(&r).is_some() => self.player.clear_next(),
            Some(r) => {
                let item = self.item(r);
                self.player.set_next(item);
            }
            None => self.player.clear_next(),
        }
    }

    fn pause(&mut self) {
        self.player.pause();
        if let Some(l) = &self.on_live {
            l.pause();
        }
    }

    fn resume(&mut self) {
        self.player.resume();
        if let Some(l) = &self.on_live {
            l.resume();
        }
    }

    fn stop(&mut self) {
        self.player.stop();
        self.on_live = None;
        if self.live_direct {
            self.live_direct = false;
            self.player.set_volume(self.volume);
        }
    }

    fn seek(&mut self, position: Duration) {
        self.player.seek(position);
    }

    fn set_volume(&mut self, volume: f32) {
        self.volume = volume;
        if let Some(s) = &self.soloist {
            s.set_volume(volume);
        }
        if let Some(w) = &self.widget {
            w.set_volume(volume);
        }
        if !self.live_direct {
            self.player.set_volume(volume);
        }
    }

    fn position(&self) -> Duration {
        self.player.position().unwrap_or_default()
    }

    fn configure(&mut self, crossfade: Option<Duration>, normalization: bool) {
        self.player.set_crossfade(crossfade);
        self.player.set_normalization(normalization);
    }

    fn set_equalizer(&mut self, on: bool, gains: [f32; nikgob_view::EQ_BANDS]) {
        self.player.set_eq(on, gains);
    }

    fn set_speed(&mut self, id: u64, speed: nikgob_view::SpeedVariant, keep_timbre: bool) {
        if self.on_live.is_none() {
            self.player.set_fx(id, item_fx(speed, keep_timbre));
            return;
        }
        // Not for a live track: it plays as its player plays it.
        let to = self.links.events.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(to) = to {
            let _ = to.send(PlaybackEvent::Speed { id, speed: 1.0 });
        }

    }


    fn set_device(&mut self, device: Option<String>) -> bool {
        self.player.stop();
        match spawn(device, &self.links) {
            Ok(p) => {
                self.player = p;
                self.player.set_volume(self.volume);
                self.on_live = None;
                self.live_direct = false;
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
                // The length the engine found goes along (a stream track
                // whose service did not say has none of its own).
                let length = match &e {
                    Event::Started { id, duration: Some(d) } => Some((*id, *d)),
                    Event::TrackChanged { to, duration: Some(d), .. } => Some((*to, *d)),
                    _ => None,
                };
                let mapped = match e {
                    Event::Started { id, .. } => Some(PlaybackEvent::Started { id }),
                    Event::TrackChanged { to, .. } => Some(PlaybackEvent::TrackChanged { to }),
                    Event::Position { id, pos, at } => Some(PlaybackEvent::PositionAt { id, position: pos, at }),
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
                    && (to.send(m).is_err()
                        || length.is_some_and(|(id, duration)| to.send(PlaybackEvent::Duration { id, duration }).is_err()))
                {
                    break;
                }
            }
        })
        .expect("spawn audio event thread");
}
