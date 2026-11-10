//! Public value types: items, events, errors.

use std::fmt;
use std::io::{Read, Seek};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// What the engine plays. The `id` is opaque to the engine and comes back in
/// every [`Event`] that concerns the item.
pub struct PlayItem {
    /// Caller's identifier (e.g. a track id).
    pub id: u64,
    /// Where the audio comes from.
    pub source: ItemSource,
    /// Loudness correction in dB (ReplayGain / R128 track gain computed by the
    /// caller). Applied only while normalization is enabled
    /// ([`Player::set_normalization`](crate::Player::set_normalization)).
    pub gain_db: f32,
    /// Length known by the caller. Used when the source cannot tell its own
    /// length (live PCM, MP3 without a Xing header); the source's own value
    /// wins otherwise.
    pub duration_hint: Option<Duration>,
    /// Speed, tone and reverb of this item ([`ItemFx::default`]: none).
    pub fx: ItemFx,
}

/// How one item is changed on its way out: played faster or slower (the
/// pitch follows, as on a turntable), tilted in tone, with a reverb.
///
/// Positions and lengths the engine reports stay on the item's own
/// timeline: at speed 1.25 the position of a playing item advances 1.25 s
/// every second, and a four-minute item still reports four minutes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ItemFx {
    /// Playback speed, 0.5 to 2.0 (1.0: as recorded), rounded to 0.01.
    pub speed: f32,
    /// Shelf below about 100 Hz, dB.
    pub low_shelf_db: f32,
    /// Shelf above about 7 kHz, dB.
    pub high_shelf_db: f32,
    /// Level before the tone and the reverb, dB: the headroom they need.
    pub preamp_db: f32,
    /// The reverb, if any.
    pub reverb: Option<Reverb>,
    /// Keep the voice's timbre: the pitch still follows the speed, but the
    /// spectral envelope (the formants) is put back where it was recorded,
    /// so a sped-up voice does not turn thin and a slowed one not hollow.
    /// Only has an effect at a speed other than 1.
    pub keep_formants: bool,
}

impl Default for ItemFx {
    fn default() -> Self {
        Self {
            speed: 1.0,
            low_shelf_db: 0.0,
            high_shelf_db: 0.0,
            preamp_db: 0.0,
            reverb: None,
            keep_formants: false,
        }
    }
}

/// A reverb added to an item (see [`ItemFx`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reverb {
    /// Level of the reverberation against the dry sound (which stays at
    /// 1): 1.0 is as loud as the music itself on a sustained sound.
    pub wet: f32,
    /// Time the tail takes to fall by 60 dB, seconds (the low and middle
    /// frequencies; the highs go sooner, see `damping`).
    pub decay_s: f32,
    /// 0 (bright tail) to 1 (dark): how soon the highs of the tail die.
    pub damping: f32,
    /// Gap between the sound and its reverberation, milliseconds.
    pub pre_delay_ms: f32,
}

/// A speed as an exact ratio, so lengths and positions never drift.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Speed {
    pub num: u32,
    pub den: u32,
}

impl Default for Speed {
    fn default() -> Self {
        Self::ONE
    }
}

impl Speed {
    pub(crate) const ONE: Self = Self { num: 1, den: 1 };


    /// `speed` rounded to a hundredth, within 0.5 to 2.
    pub(crate) fn new(speed: f32) -> Self {
        let speed = if speed.is_finite() { speed } else { 1.0 };
        let num = (speed.clamp(0.5, 2.0) * 100.0).round() as u32;
        let g = gcd(u64::from(num), 100) as u32;
        Self {
            num: num / g,
            den: 100 / g,
        }
    }

    pub(crate) const fn is_one(self) -> bool {
        self.num == self.den
    }

    /// `ns` of wall time as time on the item's timeline.
    pub(crate) fn item_ns(self, ns: u64) -> u64 {
        if self.is_one() {
            return ns;
        }
        (u128::from(ns) * u128::from(self.num) / u128::from(self.den)) as u64
    }

    pub(crate) const fn pack(self) -> u64 {
        ((self.num as u64) << 32) | self.den as u64
    }

    pub(crate) const fn unpack(bits: u64) -> Self {
        let (num, den) = ((bits >> 32) as u32, bits as u32);
        if num == 0 || den == 0 {
            Self::ONE
        } else {
            Self { num, den }
        }
    }
}

pub(crate) fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

impl PlayItem {
    /// A file with no gain correction and no duration hint.
    #[must_use]
    pub fn file(id: u64, path: impl Into<PathBuf>) -> Self {
        Self {
            id,
            source: ItemSource::File(path.into()),
            gain_db: 0.0,
            duration_hint: None,
            fx: ItemFx::default(),
        }
    }

    /// A PCM source with no gain correction.
    #[must_use]
    pub fn pcm(id: u64, source: Box<dyn PcmSource>, duration_hint: Option<Duration>) -> Self {
        Self {
            id,
            source: ItemSource::Pcm(source),
            gain_db: 0.0,
            duration_hint,
            fx: ItemFx::default(),
        }
    }

    /// A stream opened lazily by `open` (see [`ItemSource::Stream`]), with no
    /// gain correction and no duration hint.
    #[must_use]
    pub fn stream(id: u64, open: StreamOpener) -> Self {
        Self {
            id,
            source: ItemSource::Stream(open),
            gain_db: 0.0,
            duration_hint: None,
            fx: ItemFx::default(),
        }
    }

    /// Sets the loudness correction.
    #[must_use]
    pub fn with_gain_db(mut self, gain_db: f32) -> Self {
        self.gain_db = gain_db;
        self
    }

    /// Sets the speed, tone and reverb.
    #[must_use]
    pub fn with_fx(mut self, fx: ItemFx) -> Self {
        self.fx = fx;
        self
    }

    /// Sets the duration hint.
    #[must_use]
    pub fn with_duration_hint(mut self, hint: Duration) -> Self {
        self.duration_hint = Some(hint);
        self
    }
}

impl fmt::Debug for PlayItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlayItem")
            .field("id", &self.id)
            .field("source", &self.source)
            .field("gain_db", &self.gain_db)
            .field("duration_hint", &self.duration_hint)
            .field("fx", &self.fx)
            .finish()
    }
}

/// The audio of a [`PlayItem`].
pub enum ItemSource {
    /// A local file (FLAC, MP3, AAC/M4A, Vorbis, WAV, ALAC, Opus in Ogg).
    File(PathBuf),
    /// Raw samples pulled from the caller.
    Pcm(Box<dyn PcmSource>),
    /// Encoded bytes of a stream (Spotify Ogg Vorbis, YouTube WebM Opus or
    /// M4A AAC), decoded like a file. The opener runs on a short-lived opener
    /// thread of the engine, never on the caller's thread, so it (and the
    /// first reads of the probe) may do network I/O; the engine keeps
    /// answering commands meanwhile. Later reads run on the decoder thread.
    ///
    /// While a [`Player::play`](crate::Player::play) of a stream is opening,
    /// nothing is heard (what played before is cut at once) and the
    /// position reads the item at its start; a seek meanwhile is applied
    /// once it is open. `Started` follows once it is open, an
    /// [`Event::Error`] for the item if it cannot be opened. A stream given to
    /// [`Player::set_next`](crate::Player::set_next) is opened in the
    /// background; if the current item ends first, playback waits for it.
    Stream(StreamOpener),
}

impl ItemSource {
    /// Whether opening may be slow (network) and happens off the decoder thread.
    pub(crate) const fn is_stream(&self) -> bool {
        matches!(self, Self::Stream(_))
    }
}

/// Opens the bytes of an [`ItemSource::Stream`]; the error text ends up in
/// [`Event::Error`].
pub type StreamOpener = Box<dyn FnOnce() -> Result<Box<dyn ByteSource>, String> + Send>;

/// Encoded audio bytes the engine reads like a file. Implemented by the
/// application over a streaming service's download.
///
/// `read` / `seek` run on engine threads (the opener thread for the probe,
/// then the decoder thread); they may block on the network, but a read that
/// blocks for long starves the output, so implementations should download
/// ahead in the background.
pub trait ByteSource: Read + Seek + Send {
    /// Total size in bytes, if known.
    fn byte_len(&self) -> Option<u64>;
    /// The container as a file extension (`"ogg"`, `"webm"`, `"m4a"`,
    /// `"wav"`); `""` lets the probe guess.
    fn format_hint(&self) -> &str;
    /// Loudness correction in dB; when `Some`, it replaces
    /// [`PlayItem::gain_db`]. Asked once the stream has been probed.
    fn gain_db(&self) -> Option<f32> {
        None
    }
    /// Length of the audio, used when the container does not tell it (it
    /// replaces [`PlayItem::duration_hint`] when `Some`). Asked once the
    /// stream has been probed.
    fn duration(&self) -> Option<Duration> {
        None
    }
}

impl fmt::Debug for ItemSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File(path) => f.debug_tuple("File").field(path).finish(),
            Self::Pcm(_) => f.write_str("Pcm(..)"),
            Self::Stream(_) => f.write_str("Stream(..)"),
        }
    }
}

/// Result of one [`PcmSource::read`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PcmRead {
    /// This many whole frames were written to the start of the buffer.
    Frames(usize),
    /// Nothing is available right now (a live source whose producer is
    /// behind). The engine retries in a couple of milliseconds; the listener
    /// hears silence if the ring runs dry meanwhile.
    Pending,
    /// The stream is over.
    End,
}

/// Error type of a [`PcmSource`].
pub type PcmError = Box<dyn std::error::Error + Send + Sync>;

/// Pull-based interleaved `f32` audio. Implemented by the caller for
/// sources the engine cannot open itself (a librespot `Sink` bridge, a
/// YouTube stream decoder, ...).
///
/// `read` runs on the engine's decoder thread, never on the audio callback,
/// so it may do I/O, but it should not block for long (return
/// [`PcmRead::Pending`] instead): commands such as seek or stop wait for it.
pub trait PcmSource: Send + 'static {
    /// Frames per second of the produced audio.
    fn sample_rate(&self) -> u32;

    /// Interleaved channel count.
    fn channels(&self) -> u16;

    /// Writes up to `out.len() / channels()` whole frames into `out`.
    fn read(&mut self, out: &mut [f32]) -> Result<PcmRead, PcmError>;

    /// Total length, when known.
    fn duration(&self) -> Option<Duration> {
        None
    }

    /// Repositions the source. Returns `false` when it cannot seek (the
    /// default); the engine then reports an [`Event::Error`] and keeps playing.
    fn seek(&mut self, _position: Duration) -> bool {
        false
    }
}

/// What happened. Delivered through the receiver returned by
/// [`Player::spawn`](crate::Player::spawn); the wake callback fires on every one.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Playback of `id` began (nothing was playing before it).
    Started {
        /// The item.
        id: u64,
        /// Its length, if known.
        duration: Option<Duration>,
    },
    /// The audible item changed at an exact sample boundary: gapless
    /// transition, crossfade start, or `play()` over a playing item.
    TrackChanged {
        /// The item that was playing.
        from: u64,
        /// The item that plays now.
        to: u64,
        /// Length of `to`, if known.
        duration: Option<Duration>,
    },
    /// Playback position, about four times a second while an item is loaded
    /// and not paused (also while it stalls: then it stands still).
    Position {
        /// The item.
        id: u64,
        /// Time-based position.
        pos: Duration,
        /// When it was read: the event may wait in a queue before it is
        /// taken, and the position moved on meanwhile.
        at: Instant,
    },
    /// Playback was paused.
    Paused,
    /// Playback resumed.
    Resumed,
    /// The item reached its end and nothing followed it directly (followed
    /// by [`Event::QueueEmpty`]).
    Ended {
        /// The item.
        id: u64,
    },
    /// Nothing is playing and no next item is prepared.
    QueueEmpty,
    /// [`Player::stop`](crate::Player::stop) took effect.
    Stopped,
    /// Something failed. A decode error of the playing item skips to the
    /// prepared next item (if any); a failure to open an item leaves current
    /// playback alone. `id` is `None` for device errors.
    Error {
        /// The item concerned.
        id: Option<u64>,
        /// Human-readable description.
        message: String,
    },
    /// The output moved to another device or stream configuration. What is
    /// playing carries on from the same position.
    DeviceChanged {
        /// New device name.
        device: String,
        /// Its sample rate.
        sample_rate: u32,
        /// Its channel count.
        channels: u16,
    },
}

/// Why the engine could not start.
#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    /// The control thread could not be created.
    #[error("cannot start the audio control thread: {0}")]
    Thread(#[from] std::io::Error),
}

/// Errors from opening or decoding a file or PCM source.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    /// File system trouble.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// Symphonia could not parse or decode the stream.
    #[error("{0}")]
    Format(String),
    /// No audio track, or a codec/channel layout this engine cannot decode.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// A seek was refused.
    #[error("seek failed: {0}")]
    Seek(String),
    /// A [`PcmSource`] failed.
    #[error("pcm source: {0}")]
    Pcm(String),
    /// An [`ItemSource::Stream`] could not be opened.
    #[error("stream: {0}")]
    Stream(String),
}

impl From<symphonia::core::errors::Error> for DecodeError {
    fn from(error: symphonia::core::errors::Error) -> Self {
        use symphonia::core::errors::Error as E;
        match error {
            E::IoError(e) => Self::Io(e),
            E::Unsupported(m) | E::LimitError(m) => Self::Unsupported(m.to_owned()),
            e @ E::SeekError(_) => Self::Seek(e.to_string()),
            other => Self::Format(other.to_string()),
        }
    }
}
