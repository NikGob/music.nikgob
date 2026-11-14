//! Spotify audio as plain Ogg Vorbis bytes for our own engine.
//!
//! Spotify files are Ogg Vorbis encrypted with AES-128-CTR, with a custom
//! first Ogg page of 0xa7 bytes holding the normalisation data. librespot
//! downloads the file in ranges (prefetching ahead of the read position)
//! and decrypts it; [`OggBody`] hides the header, so the engine sees an
//! ordinary Ogg Vorbis file and decodes it itself (volume, normalisation,
//! visualizer and crossfade are the engine's). The header layout follows
//! librespot-playback's `player.rs` (MIT).

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use librespot_audio::{AudioDecrypt, AudioFile, StreamLoaderController};
use librespot_core::spotify_id::SpotifyId;
use librespot_core::{Session, SpotifyUri};
use librespot_metadata::audio::{AudioFileFormat, AudioItem};
use nikgob_core::streaming::{AudioStream, StreamError, StreamInterrupter, format};

/// End of Spotify's own first Ogg page, which decoders must not see.
pub const SPOTIFY_OGG_HEADER_END: u64 = 0xa7;
/// Where the normalisation data sits inside that page: track gain (dB),
/// track peak, album gain, album peak, four little-endian `f32`s.
pub const NORMALISATION_OFFSET: u64 = 144;
/// Ogg Vorbis only: the engine decodes Vorbis, and MP3/AAC files of
/// Spotify are not what librespot streams for music.
const FORMATS: [(AudioFileFormat, usize, u32); 3] = [
    (AudioFileFormat::OGG_VORBIS_320, 40 * 1024, 320),
    (AudioFileFormat::OGG_VORBIS_160, 20 * 1024, 160),
    (AudioFileFormat::OGG_VORBIS_96, 12 * 1024, 96),
];
/// The whole open (metadata, key, first bytes) must not take longer.
const OPEN_TIMEOUT: Duration = Duration::from_secs(45);
/// How often a thread waiting for the open looks whether it was
/// interrupted.
const OPEN_POLL: Duration = Duration::from_millis(20);

/// A decrypted Spotify file seen from after its header: offsets are those
/// of the plain Ogg stream.
pub struct OggBody<R> {
    inner: R,
    len: u64,
    pos: u64,
}

impl<R: Read + Seek> OggBody<R> {
    /// Reads the track gain from the header and positions at the first Ogg
    /// page of the music. `file_size` is the size of the whole file.
    pub fn new(mut inner: R, file_size: u64) -> io::Result<(Self, Option<f32>)> {
        if file_size <= SPOTIFY_OGG_HEADER_END {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Spotify file shorter than its header",
            ));
        }
        inner.seek(SeekFrom::Start(NORMALISATION_OFFSET))?;
        let mut data = [0u8; 16];
        inner.read_exact(&mut data)?;
        let gain = f32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        let gain = (gain.is_finite() && gain.abs() < 60.0).then_some(gain);
        inner.seek(SeekFrom::Start(SPOTIFY_OGG_HEADER_END))?;
        Ok((
            Self {
                inner,
                len: file_size - SPOTIFY_OGG_HEADER_END,
                pos: 0,
            },
            gain,
        ))
    }

    pub fn len(&self) -> u64 {
        self.len
    }
}

impl<R: Read> Read for OggBody<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.len.saturating_sub(self.pos);
        if left == 0 || buf.is_empty() {
            return Ok(0);
        }
        let want = buf.len().min(usize::try_from(left).unwrap_or(usize::MAX));
        let n = self.inner.read(&mut buf[..want])?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl<R: Seek> Seek for OggBody<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(p) => i128::from(p),
            SeekFrom::End(d) => i128::from(self.len) + i128::from(d),
            SeekFrom::Current(d) => i128::from(self.pos) + i128::from(d),
        };
        let target = u64::try_from(target)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "seek before start"))?;
        // Always an absolute seek: librespot's AudioFile computes
        // `SeekFrom::End` one byte off.
        self.inner
            .seek(SeekFrom::Start(target + SPOTIFY_OGG_HEADER_END))?;
        self.pos = target;
        Ok(target)
    }
}

/// An opened Spotify track.
pub struct Opened {
    pub body: OggBody<AudioDecrypt<AudioFile>>,
    pub gain_db: Option<f32>,
    pub duration: Option<Duration>,
    /// Bitrate of the file (320, 160 or 96 kbps).
    pub kbps: u32,
    /// It came from librespot's encrypted file cache (no download).
    pub cached: bool,
    /// Kept so the download is stopped when the stream is dropped.
    controller: StreamLoaderController,
}

impl Drop for Opened {
    fn drop(&mut self) {
        self.controller.close();
    }
}

/// Why opening failed, with whether a new session might help.
pub struct OpenError {
    pub error: StreamError,
    pub retry_with_new_session: bool,
    /// Spotify refused the file's key to a Premium account.
    pub key_refused: bool,
}

impl OpenError {
    fn new(error: StreamError, session: &Session) -> Self {
        Self {
            error,
            retry_with_new_session: session.is_invalid(),
            key_refused: false,
        }
    }
}

/// Loads a track over `session`: metadata (following alternatives for a
/// track unavailable in the account's region), the best Ogg Vorbis file,
/// its key, the header. Blocking parts run off the async workers.
pub async fn open(session: &Session, id: &str, premium: Option<bool>) -> Result<Opened, OpenError> {
    let failed = |e: StreamError| OpenError::new(e, session);
    let track_id = SpotifyId::from_base62(id).map_err(|_| failed(StreamError::Unavailable))?;
    let uri = SpotifyUri::Track { id: track_id };
    let item = AudioItem::get_file(session, uri)
        .await
        .map_err(|e| failed(network(e)))?;
    let item = playable(session, item)
        .await
        .ok_or_else(|| failed(StreamError::Unavailable))?;
    let (file_id, bytes_per_second, kbps) = FORMATS
        .iter()
        .find_map(|(f, bps, kbps)| item.files.get(f).map(|id| (*id, *bps, *kbps)))
        .ok_or_else(|| failed(StreamError::Unavailable))?;
    let duration =
        Some(Duration::from_millis(u64::from(item.duration_ms))).filter(|d| !d.is_zero());
    let loaded_id = SpotifyId::try_from(&item.track_id).unwrap_or(track_id);

    // The file and its key are asked for at once (one round trip less
    // before the first sound).
    let (file, key) = tokio::join!(
        AudioFile::open(session, file_id, bytes_per_second),
        session.audio_key().request(loaded_id, file_id)
    );
    let file = file.map_err(|e| failed(network(e)))?;
    let cached = file.is_cached();
    let controller = file
        .get_stream_loader_controller()
        .map_err(|e| failed(network(e)))?;
    let size = controller.len() as u64;
    let key = match key {
        Ok(key) => key,
        Err(e) => {
            log::warn!("spotify: audio key refused for {id}: {e}");
            controller.close();
            // Spotify refuses keys to free accounts, and lately to some
            // Premium ones too (librespot #1649).
            return Err(if premium == Some(true) {
                OpenError { key_refused: true, ..failed(StreamError::Unavailable) }
            } else {
                failed(StreamError::PremiumRequired)
            });
        }
    };
    // The first range is fetched in random-access mode, then streaming
    // mode prefetches ahead of the read position.
    let header = tokio::task::spawn_blocking(move || {
        let decrypted = AudioDecrypt::new(Some(key), file);
        OggBody::new(decrypted, size).map(|(body, gain)| (body, gain, controller))
    })
    .await
    .map_err(|e| failed(StreamError::Network(e.to_string())))?;
    let (body, gain_db, controller) =
        header.map_err(|e| failed(StreamError::Network(e.to_string())))?;
    controller.set_stream_mode();
    Ok(Opened {
        body,
        gain_db,
        duration,
        kbps,
        cached,
        controller,
    })
}

/// The item itself when it has files, else its first available alternative.
async fn playable(session: &Session, item: AudioItem) -> Option<AudioItem> {
    if item.availability.is_ok() && !item.files.is_empty() {
        return Some(item);
    }
    for alternative in item.alternatives.map(|a| a.0).unwrap_or_default() {
        if let Ok(alt) = AudioItem::get_file(session, alternative).await
            && alt.availability.is_ok()
            && !alt.files.is_empty()
        {
            return Some(alt);
        }
    }
    None
}

fn network(e: librespot_core::Error) -> StreamError {
    use librespot_core::error::ErrorKind;
    match e.kind {
        ErrorKind::NotFound | ErrorKind::Unavailable | ErrorKind::PermissionDenied => {
            StreamError::Unavailable
        }
        _ => StreamError::Network(e.to_string()),
    }
}

/// Opens a track in the background; given to [`SpotifyStream`]. It gives
/// up once the flag is set (see [`block_on_runtime`]).
pub type Opener = Box<dyn FnOnce(&AtomicBool) -> Result<Opened, StreamError> + Send>;

/// The [`AudioStream`] handed to the engine: nothing happens until the
/// first read or seek, which opens the track (blocking the engine's thread
/// until the header is there). Reads then block only while librespot
/// fetches the wanted range; it prefetches ahead while streaming.
///
/// Its [`interrupter`](AudioStream::interrupter) ends a wait for the open
/// (the engine dropping a track that is still opening, which is where a
/// Spotify stream waits longest). A read waiting inside librespot cannot be
/// woken: librespot gives it up itself after a few seconds without data.
pub struct SpotifyStream {
    opener: Option<Opener>,
    /// The engine interrupted the stream while it opened.
    interrupted: Arc<AtomicBool>,
    opened: Option<Opened>,
    failure: Option<String>,
    /// Where the opened track goes when this stream is dropped (see
    /// [`Recent`]), with its id.
    keep: Option<(String, Recent)>,
}

/// The last track that played, still open (its download running), so
/// «Назад» or a repeat starts at once. One track: the one before is closed.
pub type Recent = std::sync::Arc<std::sync::Mutex<Option<(String, Opened)>>>;

/// Takes the kept track `id`, rewound, if it is the one kept.
pub fn take_recent(recent: &Recent, id: &str) -> Option<Opened> {
    let mut slot = recent.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if slot.as_ref().is_some_and(|(kept, _)| kept == id) {
        let (_, mut o) = slot.take()?;
        o.body.seek(SeekFrom::Start(0)).ok()?;
        return Some(o);
    }
    None
}

impl SpotifyStream {
    pub fn new(opener: Opener) -> Self {
        Self {
            opener: Some(opener),
            interrupted: Arc::new(AtomicBool::new(false)),
            opened: None,
            failure: None,
            keep: None,
        }
    }

    /// Keeps the opened track in `recent` when dropped.
    pub fn keep_in(mut self, id: String, recent: Recent) -> Self {
        self.keep = Some((id, recent));
        self
    }

    fn body(&mut self) -> io::Result<&mut OggBody<AudioDecrypt<AudioFile>>> {
        if let Some(opener) = self.opener.take() {
            match opener(&self.interrupted) {
                Ok(o) => self.opened = Some(o),
                Err(e) => self.failure = Some(describe(&e)),
            }
        }
        match (&mut self.opened, &self.failure) {
            (Some(o), _) => Ok(&mut o.body),
            (None, Some(f)) => Err(io::Error::other(f.clone())),
            (None, None) => Err(io::Error::other("Spotify stream closed")),
        }
    }
}

/// The error text the engine shows.
pub fn describe(e: &StreamError) -> String {
    match e {
        StreamError::PremiumRequired => "Spotify: для звука нужен Premium".into(),
        StreamError::NotLoggedIn => "Spotify: включи воспроизведение в настройках".into(),
        StreamError::Unavailable => "Spotify: трек недоступен".into(),
        other => format!("Spotify: {other}"),
    }
}

impl Read for SpotifyStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.body()?.read(buf)
    }
}

impl Seek for SpotifyStream {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.body()?.seek(pos)
    }
}

impl AudioStream for SpotifyStream {
    fn byte_len(&self) -> Option<u64> {
        self.opened.as_ref().map(|o| o.body.len())
    }

    fn format_hint(&self) -> &'static str {
        format::OGG
    }

    fn gain_db(&self) -> Option<f32> {
        self.opened.as_ref().and_then(|o| o.gain_db)
    }

    fn duration(&self) -> Option<Duration> {
        self.opened.as_ref().and_then(|o| o.duration)
    }

    fn quality(&self) -> Option<String> {
        self.opened
            .as_ref()
            .map(|o| nikgob_core::streaming::quality_label("Vorbis", o.kbps, false))
    }

    fn interrupter(&self) -> Option<StreamInterrupter> {
        let flag = Arc::clone(&self.interrupted);
        Some(Arc::new(move |on| flag.store(on, Ordering::SeqCst)))
    }
}

impl Drop for SpotifyStream {
    fn drop(&mut self) {
        if let (Some(o), Some((id, recent))) = (self.opened.take(), self.keep.take()) {
            let old = recent
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .replace((id, o));
            drop(old);
        }
    }
}

/// Runs `open` on the runtime and waits for it on this (engine) thread,
/// which may be any thread: inside or outside a runtime. Gives up when
/// `stop` is set (what the open brings then is dropped once it comes).
pub fn block_on_runtime<T: Send + 'static>(
    handle: &tokio::runtime::Handle,
    stop: &AtomicBool,
    fut: impl Future<Output = Result<T, StreamError>> + Send + 'static,
) -> Result<T, StreamError> {
    let (tx, rx) = mpsc::sync_channel(1);
    handle.spawn(async move {
        let _ = tx.send(fut.await);
    });
    let deadline = Instant::now() + OPEN_TIMEOUT;
    loop {
        if stop.load(Ordering::SeqCst) {
            return Err(StreamError::Network("interrupted".into()));
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(StreamError::Network("Spotify did not answer".into()));
        }
        match rx.recv_timeout(left.min(OPEN_POLL)) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(StreamError::Network("Spotify did not answer".into()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use librespot_core::audio_key::AudioKey;
    use std::io::Cursor;

    const KEY: [u8; 16] = *b"0123456789abcdef";

    fn fixture() -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../audio/tests/fixtures/tone.ogg");
        std::fs::read(path).unwrap()
    }

    /// `ogg` as Spotify stores it: a 0xa7-byte first page with the
    /// normalisation data, then the music, all encrypted with AES-128-CTR.
    fn spotify_file(ogg: &[u8], track_gain: f32) -> Vec<u8> {
        let mut plain = vec![0u8; SPOTIFY_OGG_HEADER_END as usize];
        plain[..4].copy_from_slice(b"OggS");
        let norm = NORMALISATION_OFFSET as usize;
        for (i, v) in [track_gain, 0.9, -1.5, 0.95].iter().enumerate() {
            plain[norm + i * 4..norm + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        plain.extend_from_slice(ogg);
        // CTR mode: encrypting is decrypting.
        let mut encrypted = Vec::new();
        AudioDecrypt::new(Some(AudioKey(KEY)), Cursor::new(plain))
            .read_to_end(&mut encrypted)
            .unwrap();
        encrypted
    }

    type Body = OggBody<AudioDecrypt<Cursor<Vec<u8>>>>;

    fn open_body(file: Vec<u8>) -> (Body, Option<f32>) {
        let size = file.len() as u64;
        OggBody::new(
            AudioDecrypt::new(Some(AudioKey(KEY)), Cursor::new(file)),
            size,
        )
        .unwrap()
    }

    #[test]
    fn an_open_that_does_not_answer_gives_up_when_interrupted() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let mut s = SpotifyStream::new(Box::new({
            let handle = rt.handle().clone();
            move |stop| {
                block_on_runtime(&handle, stop, std::future::pending::<Result<Opened, StreamError>>())
            }
        }));
        let interrupt = s.interrupter().unwrap();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            interrupt(true);
        });
        let began = Instant::now();
        assert!(s.read(&mut [0; 4]).is_err());
        assert!(began.elapsed() < Duration::from_secs(2), "{:?}", began.elapsed());
        t.join().unwrap();
    }

    #[test]
    fn the_header_is_stripped_and_the_gain_read() {
        let ogg = fixture();
        let file = spotify_file(&ogg, -6.25);
        assert_ne!(
            &file[SPOTIFY_OGG_HEADER_END as usize..][..4],
            b"OggS",
            "encrypted"
        );
        let (mut body, gain) = open_body(file);
        assert_eq!(gain, Some(-6.25));
        assert_eq!(body.len(), ogg.len() as u64);
        let mut out = Vec::new();
        body.read_to_end(&mut out).unwrap();
        assert!(out == ogg, "the plain Ogg stream comes back");
    }

    #[test]
    fn seeks_are_mapped_past_the_header() {
        let ogg = fixture();
        let (mut body, _) = open_body(spotify_file(&ogg, 0.0));
        let mut buf = [0u8; 64];
        assert_eq!(body.seek(SeekFrom::Start(1000)).unwrap(), 1000);
        body.read_exact(&mut buf).unwrap();
        assert_eq!(&buf[..], &ogg[1000..1064]);
        assert_eq!(body.seek(SeekFrom::Current(-64)).unwrap(), 1000);
        body.read_exact(&mut buf).unwrap();
        assert_eq!(&buf[..], &ogg[1000..1064]);
        let end = body.seek(SeekFrom::End(-10)).unwrap();
        assert_eq!(end, ogg.len() as u64 - 10);
        let mut tail = Vec::new();
        body.read_to_end(&mut tail).unwrap();
        assert_eq!(tail, &ogg[ogg.len() - 10..]);
        assert_eq!(body.read(&mut buf).unwrap(), 0, "no bytes past the end");
        assert!(
            body.seek(SeekFrom::Current(-(ogg.len() as i64) - 1))
                .is_err()
        );
        assert_eq!(body.seek(SeekFrom::Start(0)).unwrap(), 0);
        body.read_exact(&mut buf[..4]).unwrap();
        assert_eq!(&buf[..4], b"OggS");
    }

    #[test]
    fn nonsense_gain_is_ignored_and_a_short_file_refused() {
        let (_, gain) = open_body(spotify_file(&fixture(), f32::NAN));
        assert_eq!(gain, None);
        let short = vec![0u8; 100];
        assert!(OggBody::new(Cursor::new(short), 100).is_err());
    }

    /// The engine decodes the stripped stream like any Ogg Vorbis file.
    #[test]
    fn nikgob_audio_decodes_the_stripped_stream() {
        use nikgob_audio::{ByteSource, OfflineOptions, PlayItem, Player};

        struct Source(Body, Option<f32>);
        impl Read for Source {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                self.0.read(buf)
            }
        }
        impl Seek for Source {
            fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
                self.0.seek(pos)
            }
        }
        impl ByteSource for Source {
            fn byte_len(&self) -> Option<u64> {
                Some(self.0.len())
            }
            fn format_hint(&self) -> &str {
                format::OGG
            }
            fn gain_db(&self) -> Option<f32> {
                self.1
            }
        }

        let (body, gain) = open_body(spotify_file(&fixture(), 0.0));
        let (player, _events, mut device) = Player::offline(OfflineOptions {
            sample_rate: 48_000,
            channels: 2,
            block_frames: 256,
        })
        .unwrap();
        let source = Source(body, gain);
        player.play(PlayItem::stream(
            1,
            Box::new(move || Ok(Box::new(source) as Box<dyn ByteSource>)),
        ));
        let out = device.drain(48_000 * 10);
        let frames = out.len() / 2;
        // tone.ogg is one second of a 440 Hz tone at 0.3.
        assert!((frames as i64 - 48_000).abs() <= 2, "{frames} frames");
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        let rms = (left[2000..46_000].iter().map(|s| s * s).sum::<f32>() / 44_000.0).sqrt();
        assert!((rms - 0.3 / 2f32.sqrt()).abs() < 0.03, "rms {rms}");
    }
}
