//! Bridges the audio engine (`nikgob-audio`) to the runtime's `Playback`:
//! the engine of the app and of the headless player (`nikgobd`).
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
//! These live sources are the `live` feature (on by default, the app's);
//! the headless player builds without it, and every Spotify or SoundCloud
//! track goes through its service like any stream there.
//!
//! SoundCloud tracks it streams only with DRM go the same way through its
//! official player (`nikgob-sc-widget`, Windows): the first open of such a
//! track fails with [`nikgob_runtime::OFFICIAL_PLAYER_ROUTE`] in its error,
//! the track is remembered, and the runtime plays the same source again,
//! which is then a live source of the widget. A track the widget failed
//! keeps the old fallback (the runtime's next source, YouTube Music).
//!
//! The developer profile's tracks have no files (the app's `profile.rs`): a missing
//! local file of a known length plays there as silence of that length
//! ([`Silence`]), so position, lyrics and the end of the track run as for a
//! real song; elsewhere a missing file fails as ever.
//!
//! Switching the output device restarts the engine: the old `Player` is
//! dropped (its threads stop and the device closes), a new one opens the
//! chosen device, and the runtime replays the current track from where it
//! was. Events and the analyzer go through shared slots so the runtime and
//! the window never hold a stale engine.
//!
//! The duck ([`Playback::set_duck`]) is the engine's own gain after the
//! volume and the sleep fade; an engine that takes over on another device
//! starts at the duck in force, and a live player heard by itself is turned
//! down with it.

#[cfg(feature = "live")]
use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use nikgob_audio::{
    Analyzer, ByteSource, Event, ItemFx, ItemSource, PcmError, PcmRead, PcmSource, PlayItem, Player, PlayerOptions, Reverb,
};
use nikgob_core::streaming::{AudioStream, StreamService};
#[cfg(feature = "live")]
use nikgob_core::streaming::{Service as StreamKind, StreamError};
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
    /// The engine heard now.
    generation: Arc<AtomicU64>,
    /// The newest engine started (one past `generation` while a device
    /// switch waits for its device to open).
    newest: Arc<AtomicU64>,
}

impl Links {
    /// Where engine events go (set once the runtime exists).
    pub fn connect(&self, to: Sender<PlaybackEvent>) {
        *self.events.lock().unwrap_or_else(PoisonError::into_inner) = Some(to);
    }
}

/// The live players a Spotify or a DRM-only SoundCloud track plays
/// through, where the host runs them (the app; none in the headless
/// player).
#[derive(Clone, Default)]
pub struct LivePlayers {
    /// Spotify Soloist.
    #[cfg(feature = "live")]
    pub soloist: Option<Arc<nikgob_soloist::Soloist>>,
    /// SoundCloud's official player (Windows).
    #[cfg(feature = "live")]
    pub widget: Option<Arc<nikgob_sc_widget::ScWidget>>,
}

pub struct Engine {
    player: Player,
    /// An engine on another device, waiting for its device to open
    /// ([`Playback::adopt_device`]); the old one plays on meanwhile.
    pending: Option<(Player, u64)>,
    links: Links,
    streams: Vec<Arc<dyn StreamService>>,
    #[cfg(feature = "live")]
    soloist: Option<Arc<nikgob_soloist::Soloist>>,
    /// SoundCloud's official player (Windows).
    #[cfg(feature = "live")]
    widget: Option<Arc<nikgob_sc_widget::ScWidget>>,
    /// SoundCloud tracks found to be DRM-only this session.
    #[cfg(feature = "live")]
    drm: Arc<Mutex<HashSet<String>>>,
    /// The user's volume (the engine plays a live track at 0 while its
    /// player is heard itself).
    volume: f32,
    /// What plays now is a live track (Soloist, SoundCloud's player).
    on_live: Option<Live>,
    /// ... and its player is heard itself (no capture): the engine is at 0.
    live_direct: bool,
    /// The sleep timer's fade level (1: none). The engine fades by itself;
    /// a live player heard directly is turned down with it.
    fade: f32,
    /// The duck's gain (1: none, linear). The engine glides to it by
    /// itself; a live player heard directly is turned down with it.
    duck: f32,
}

/// A player whose sound a live track hands the engine (none without the
/// `live` feature).
#[derive(Clone)]
enum Live {
    #[cfg(feature = "live")]
    Soloist(Arc<nikgob_soloist::Soloist>),
    #[cfg(feature = "live")]
    Widget(Arc<nikgob_sc_widget::ScWidget>),
}

impl Live {
    fn routed(&self) -> bool {
        match *self {
            #[cfg(feature = "live")]
            Self::Soloist(ref s) => s.routed(),
            #[cfg(feature = "live")]
            Self::Widget(ref w) => w.routed(),
        }
    }

    fn set_volume(&self, _v: f32) {
        match *self {
            #[cfg(feature = "live")]
            Self::Soloist(ref s) => s.set_volume(_v),
            #[cfg(feature = "live")]
            Self::Widget(ref w) => w.set_volume(_v),
        }
    }

    fn pause(&self) {
        match *self {
            #[cfg(feature = "live")]
            Self::Soloist(ref s) => s.pause(),
            #[cfg(feature = "live")]
            Self::Widget(ref w) => w.pause(),
        }
    }

    fn resume(&self) {
        match *self {
            #[cfg(feature = "live")]
            Self::Soloist(ref s) => s.resume(),
            #[cfg(feature = "live")]
            Self::Widget(ref w) => w.resume(),
        }
    }
}

/// Starts an engine on `device` and forwards its events; not heard until
/// [`adopt`]ed.
fn spawn(device: Option<String>, links: &Links) -> Result<(Player, u64), String> {
    let mut options = PlayerOptions::default();
    if let Some(d) = device {
        options.output.device = nikgob_audio::Device::Named(d);
    }
    let (player, events) = Player::spawn(options).map_err(|e| e.to_string())?;
    let generation = links.newest.fetch_add(1, Ordering::AcqRel) + 1;
    forward(events, links.clone(), generation);
    Ok((player, generation))
}

/// Makes engine `generation` the one heard: its events go to the runtime,
/// the visualizer reads it.
fn adopt(player: &Player, generation: u64, links: &Links) {
    links.generation.store(generation, Ordering::Release);
    *links.analyzer.lock().unwrap_or_else(PoisonError::into_inner) = Some(player.analyzer());
}

impl Engine {
    /// Starts the engine on `device` (None = system default; on Linux also
    /// a PulseAudio sink's name); stream requests play through `streams`,
    /// Spotify and DRM-only SoundCloud tracks through `live` where it has
    /// them.
    pub fn start(
        device: Option<String>,
        streams: Vec<Arc<dyn StreamService>>,
        live: LivePlayers,
    ) -> Result<(Self, Links), String> {
        #[cfg(not(feature = "live"))]
        let LivePlayers {} = live;
        let links = Links::default();
        let (player, generation) = spawn(device, &links)?;
        adopt(&player, generation, &links);
        Ok((
            Self {
                player,
                pending: None,
                links: links.clone(),
                streams,
                #[cfg(feature = "live")]
                soloist: live.soloist,
                #[cfg(feature = "live")]
                widget: live.widget,
                #[cfg(feature = "live")]
                drm: Arc::default(),
                volume: 1.0,
                on_live: None,
                live_direct: false,
                fade: 1.0,
                duck: 1.0,
            },
            links,
        ))
    }

    /// The volume a live player heard by itself plays at: the user's, with
    /// the sleep fade and the duck (on the volume's perceptual curve, which
    /// the duck's linear gain is not).
    fn live_level(&self) -> f32 {
        self.volume * self.fade * self.duck.cbrt()
    }

    /// The Soloist a request plays through: a Spotify track while Soloist
    /// is ready.
    #[cfg(feature = "live")]
    fn soloist_for(&self, r: &PlayRequest) -> Option<&Arc<nikgob_soloist::Soloist>> {
        match &r.source {
            PlaySource::Stream { service: StreamKind::Spotify, .. } => self.soloist.as_ref().filter(|s| s.ready()),
            _ => None,
        }
    }

    /// SoundCloud's player for a request: a SoundCloud track known to be
    /// DRM-only that the player has not failed.
    #[cfg(feature = "live")]
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
    #[cfg(feature = "live")]
    fn live_for(&self, r: &PlayRequest) -> Option<Live> {
        if let Some(s) = self.soloist_for(r) {
            return Some(Live::Soloist(s.clone()));
        }
        self.widget_for(r).map(|w| Live::Widget(w.clone()))
    }

    /// No live players here.
    #[cfg(not(feature = "live"))]
    fn live_for(&self, _: &PlayRequest) -> Option<Live> {
        None
    }

    #[cfg(feature = "live")]
    fn quality(&self, id: u64, label: &str) {
        let to = self.links.events.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(to) = to {
            let _ = to.send(PlaybackEvent::Quality { id, label: label.into() });
        }
    }

    /// A live track (another player's sound as it plays) cannot be sped
    /// up: it plays as recorded, and the runtime is told so.
    #[cfg(feature = "live")]
    fn plays_live(&self, r: &PlayRequest) {
        if r.speed == nikgob_view::SpeedVariant::Normal {
            return;
        }
        let to = self.links.events.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(to) = to {
            let _ = to.send(PlaybackEvent::Speed { id: r.id, speed: 1.0 });
        }
    }

    /// The item of a request a live player plays (SoundCloud's player,
    /// Soloist), if it is one.
    #[cfg(feature = "live")]
    fn live_item(&self, r: &PlayRequest) -> Option<PlayItem> {
        if let (Some(w), PlaySource::Stream { id, .. }) = (self.widget_for(r), &r.source) {
            log::debug!("soundcloud {id} plays through SoundCloud's player (item {})", r.id);
            self.quality(r.id, "Плеер SoundCloud");
            self.plays_live(r);
            return Some(PlayItem {
                id: r.id,
                source: ItemSource::Pcm(Box::new(w.track(id, r.duration))),
                gain_db: 0.0,
                duration_hint: r.duration,
                fx: ItemFx::default(),
            });
        }
        if let (Some(s), PlaySource::Stream { id, .. }) = (self.soloist_for(r), &r.source) {
            log::debug!("spotify {id} plays through Soloist (item {})", r.id);
            let to = self.links.events.lock().unwrap_or_else(PoisonError::into_inner).clone();
            if let Some(to) = to {
                let _ = to.send(PlaybackEvent::Quality { id: r.id, label: "Soloist".into() });
            }
            self.plays_live(r);
            // Another device took Spotify's one stream (or paused this one):
            // the player pauses, keeping the position.
            let events = self.links.events.clone();
            let item = r.id;
            let track = s.track(id, r.duration).on_taken(move || {
                let to = events.lock().unwrap_or_else(PoisonError::into_inner).clone();
                if let Some(to) = to {
                    let _ = to.send(PlaybackEvent::PausedElsewhere { id: item });
                }
            });
            return Some(PlayItem {
                id: r.id,
                source: ItemSource::Pcm(Box::new(track)),
                gain_db: 0.0,
                duration_hint: r.duration,
                fx: ItemFx::default(),
            });
        }
        None
    }

    fn item(&self, r: PlayRequest) -> PlayItem {
        #[cfg(feature = "live")]
        if let Some(item) = self.live_item(&r) {
            return item;
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
                #[cfg(feature = "live")]
                let to_widget = {
                    let widget = self.widget.clone().filter(|_| service == StreamKind::Soundcloud);
                    let drm = self.drm.clone();
                    move |e: &StreamError, id: &str| -> bool {
                        if matches!(e, StreamError::Drm)
                            && let Some(w) = widget.as_ref().filter(|w| w.plays(id))
                        {
                            drm.lock().unwrap_or_else(PoisonError::into_inner).insert(id.to_owned());
                            // Its play comes back at once: the host starts now.
                            w.warm();
                            return true;
                        }
                        false
                    }
                };
                ItemSource::Stream(Box::new(move || {
                    let svc = svc.ok_or_else(|| format!("{service} is not available"))?;
                    log::debug!("opening {service} stream {id} (item {item})");
                    let wanted = nikgob_audio::open_wanted();
                    let stream = svc.open_audio_while(&id, &wanted).map_err(|e| {
                        log::info!("{service} stream {id} did not open: {e}");
                        #[cfg(feature = "live")]
                        if to_widget(&e, &id) {
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

    fn interrupter(&self) -> Option<nikgob_audio::Interrupter> {
        self.inner.interrupter()
    }
}

impl Playback for Engine {
    fn play(&mut self, req: PlayRequest) {
        let live = self.live_for(&req);
        #[cfg(feature = "live")]
        if let Some(Live::Widget(w)) = &live {
            // Its host runs and its capture had a moment, so `routed`
            // answers for this track.
            w.prepare();
        }
        let direct = live.as_ref().is_some_and(|l| !l.routed());
        if let Some(l) = &live {
            l.set_volume(if direct { self.live_level() } else { self.volume });
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
        #[cfg(feature = "live")]
        {
            if let Some(s) = &self.soloist {
                s.set_volume(volume);
            }
            if let Some(w) = &self.widget {
                w.set_volume(volume);
            }
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

    fn set_gain(&mut self, id: u64, gain_db: f32) {
        self.player.set_gain(id, gain_db);
    }

    fn set_sleep_fade(&mut self, level: f32, over: Duration) {
        self.fade = if level.is_finite() { level.clamp(0.0, 1.0) } else { 1.0 };
        self.player.set_fade(self.fade, over);
        // A live player heard by itself steps down with the engine's fade
        // (its volume on the same perceptual curve).
        if self.live_direct
            && let Some(l) = &self.on_live
        {
            l.set_volume(self.live_level());
        }
    }

    fn set_duck(&mut self, gain: f32, fade: Duration) {
        self.duck = if gain.is_finite() { gain.clamp(0.0, 1.0) } else { 1.0 };
        self.player.set_duck(self.duck, fade);
        // A live player heard by itself steps down with it (at once: its
        // volume is another program's).
        if self.live_direct
            && let Some(l) = &self.on_live
        {
            l.set_volume(self.live_level());
        }
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


    /// Starts an engine on `device` and keeps it waiting: the old one plays
    /// on until the new device opens ([`Playback::adopt_device`]) and stays
    /// if it does not ([`Playback::drop_device`]).
    fn set_device(&mut self, device: Option<String>) -> bool {
        match spawn(device, &self.links) {
            Ok(p) => {
                if let Some((old, _)) = self.pending.replace(p) {
                    old.stop();
                }
            }
            Err(e) => log::error!("cannot switch the audio device: {e}"),
        }
        false
    }

    fn adopt_device(&mut self, generation: u64) -> bool {
        if self.pending.as_ref().is_none_or(|(_, g)| *g != generation) {
            return false;
        }
        let Some((player, generation)) = self.pending.take() else { return false };
        self.player.stop();
        self.player = player;
        adopt(&self.player, generation, &self.links);
        self.player.set_volume(self.volume);
        self.player.set_fade(self.fade, Duration::ZERO);
        self.player.set_duck(self.duck, Duration::ZERO);
        self.on_live = None;
        self.live_direct = false;
        true
    }

    fn drop_device(&mut self, generation: u64) {
        if self.pending.as_ref().is_some_and(|(_, g)| *g == generation)
            && let Some((player, _)) = self.pending.take()
        {
            player.stop();
        }
    }
}

/// What one engine event becomes for the runtime.
#[derive(Debug, PartialEq)]
enum Routed {
    /// This event (and the length the engine found, if any).
    Send(PlaybackEvent, Option<(u64, Duration)>),
    Skip,
    /// The engine was replaced: its events are not wanted any more.
    Stop,
}

/// Routes event `e` of engine `generation` while engine `current` is heard:
/// the heard engine's events go on; an engine waiting for its device (a
/// newer one) says only whether the device opened; an older one stops.
fn route(e: Event, generation: u64, current: u64) -> Routed {
    let heard = generation == current;
    if generation < current {
        return Routed::Stop;
    }
    // The length the engine found goes along (a stream track whose service
    // did not say has none of its own).
    let length = match &e {
        Event::Started { id, duration: Some(d) } if heard => Some((*id, *d)),
        Event::TrackChanged { to, duration: Some(d), .. } if heard => Some((*to, *d)),
        _ => None,
    };
    let mapped = match e {
        Event::Error { id: None, message } => {
            log::error!("audio device: {message}");
            Some(PlaybackEvent::DeviceError { generation, message })
        }
        Event::DeviceOpened => (!heard).then_some(PlaybackEvent::DeviceReady { generation }),
        _ if !heard => None,
        Event::Started { id, .. } => Some(PlaybackEvent::Started { id }),
        Event::TrackChanged { to, .. } => Some(PlaybackEvent::TrackChanged { to }),
        Event::Position { id, pos, at } => Some(PlaybackEvent::PositionAt { id, position: pos, at }),
        Event::Paused => Some(PlaybackEvent::Paused),
        Event::Resumed => Some(PlaybackEvent::Resumed),
        Event::Ended { id } => Some(PlaybackEvent::Ended { id }),
        Event::Error { id: Some(id), message } => Some(PlaybackEvent::Error { id, message }),
        Event::DeviceChanged { device, sample_rate, channels } => {
            log::info!("audio output: {device} {sample_rate} Hz × {channels}");
            None
        }
        Event::QueueEmpty | Event::Stopped => None,
    };
    mapped.map_or(Routed::Skip, |m| Routed::Send(m, length))
}

/// How long a device event waits for the runtime to be connected (the
/// engine starts before it, and its device opens at once).
const CONNECT_WAIT: Duration = Duration::from_secs(10);

/// Forwards engine `generation`'s events to the runtime (see [`route`]).
fn forward(events: Receiver<Event>, links: Links, generation: u64) {
    std::thread::Builder::new()
        .name("nikgob-audio-events".into())
        .spawn(move || {
            for e in events {
                let current = links.generation.load(Ordering::Acquire);
                let (m, length) = match route(e, generation, current) {
                    Routed::Send(m, length) => (m, length),
                    Routed::Skip => continue,
                    Routed::Stop => break,
                };
                // A device event before the runtime is connected waits for it,
                // not lost.
                let device = matches!(m, PlaybackEvent::DeviceError { .. } | PlaybackEvent::DeviceReady { .. });
                let since = std::time::Instant::now();
                let to = loop {
                    let to = links.events.lock().unwrap_or_else(PoisonError::into_inner).clone();
                    if to.is_some() || !device || since.elapsed() > CONNECT_WAIT {
                        break to;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_engine_waiting_for_its_device_only_says_how_it_opened() {
        // Engine 2 waits while engine 1 is heard.
        assert_eq!(route(Event::DeviceOpened, 2, 1), Routed::Send(PlaybackEvent::DeviceReady { generation: 2 }, None));
        assert_eq!(
            route(Event::Error { id: None, message: "busy".into() }, 2, 1),
            Routed::Send(PlaybackEvent::DeviceError { generation: 2, message: "busy".into() }, None)
        );
        assert_eq!(route(Event::Ended { id: 5 }, 2, 1), Routed::Skip);
        // The heard engine: everything but its own opening.
        assert_eq!(route(Event::Ended { id: 5 }, 1, 1), Routed::Send(PlaybackEvent::Ended { id: 5 }, None));
        assert_eq!(route(Event::DeviceOpened, 1, 1), Routed::Skip);
        let started = Event::Started { id: 3, duration: Some(Duration::from_secs(9)) };
        assert_eq!(
            route(started, 1, 1),
            Routed::Send(PlaybackEvent::Started { id: 3 }, Some((3, Duration::from_secs(9))))
        );
        // A replaced engine stops forwarding.
        assert_eq!(route(Event::Ended { id: 5 }, 1, 2), Routed::Stop);
    }
}
