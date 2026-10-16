//! Public value types: items, events, errors.

use std::fmt;
use std::io::{Read, Seek};
use std::path::PathBuf;
use std::time::Duration;

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
        }
    }

    /// Sets the loudness correction.
    #[must_use]
    pub fn with_gain_db(mut self, gain_db: f32) -> Self {
        self.gain_db = gain_db;
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
    /// nothing is heard (what played before is cut at once) and no position
    /// is reported; `Started` / `TrackChanged` follow once it is open, an
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
    /// Playback position, at most about four times a second while playing.
    Position {
        /// The item.
        id: u64,
        /// Time-based position.
        pos: Duration,
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
