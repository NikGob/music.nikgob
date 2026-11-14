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
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, mpsc};
use std::time::{Duration, Instant};

use librespot_audio::{AudioDecrypt, AudioFile, StreamLoaderController};
use librespot_core::spotify_id::SpotifyId;
use librespot_core::{Session, SpotifyUri};
use librespot_metadata::audio::{AudioFileFormat, AudioItem};
use nikgob_core::streaming::{
    AudioStream, StreamError, StreamInterrupter, format, interrupted_read,
};

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
    /// Kept so the download is stopped when the track is dropped.
    _controller: Closing,
}

/// Stops librespot's download of a file when dropped: with the opened
/// track, or with an open given up on before it was done.
struct Closing(StreamLoaderController);

impl Drop for Closing {
    fn drop(&mut self) {
        self.0.close();
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
    let controller = Closing(
        file.get_stream_loader_controller()
            .map_err(|e| failed(network(e)))?,
    );
    let size = controller.0.len() as u64;
    let key = match key {
        Ok(key) => key,
        Err(e) => {
            log::warn!("spotify: audio key refused for {id}: {e}");
            drop(controller);
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
    controller.0.set_stream_mode();
    Ok(Opened {
        body,
        gain_db,
        duration,
        kbps,
        cached,
        _controller: controller,
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
/// librespot waits for a missing range on a condvar of its own, which
/// nothing outside it can wake. So the opened track is read on a thread of
/// its own ([`Reader`]), and the engine's thread waits for that thread's
/// answer where the [`interrupter`](AudioStream::interrupter) reaches it:
/// a read the engine interrupts returns at once, while librespot's read
/// finishes on the reader thread (it gives up by itself after a few
/// seconds without data). The interrupter also ends a wait for the open,
/// whose work on the runtime is then stopped.
pub struct SpotifyStream {
    opener: Option<Opener>,
    /// The engine interrupted the stream (the open looks at this).
    interrupted: Arc<AtomicBool>,
    reader: Arc<Reader>,
    info: Option<Info>,
    failure: Option<String>,
    /// Where the next read starts, on the plain Ogg stream.
    pos: u64,
    /// Where the opened track goes when this stream is dropped (see
    /// [`Recent`]), with its id.
    keep: Option<(String, Recent)>,
}

/// What an opened track says about itself.
#[derive(Clone, Copy)]
struct Info {
    len: u64,
    gain_db: Option<f32>,
    duration: Option<Duration>,
    kbps: u32,
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
            reader: Arc::new(Reader::default()),
            info: None,
            failure: None,
            pos: 0,
            keep: None,
        }
    }

    /// Keeps the opened track in `recent` when dropped.
    pub fn keep_in(mut self, id: String, recent: Recent) -> Self {
        self.keep = Some((id, recent));
        self
    }

    /// Opens the track on first use; what it says about itself.
    fn open(&mut self) -> io::Result<Info> {
        if let Some(opener) = self.opener.take() {
            match opener(&self.interrupted) {
                Ok(o) => {
                    let info = Info {
                        len: o.body.len(),
                        gain_db: o.gain_db,
                        duration: o.duration,
                        kbps: o.kbps,
                    };
                    let keep = self.keep.take();
                    let done = move |body: Body| {
                        if let Some((id, recent)) = keep {
                            let old = recent
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .replace((id, body.0));
                            drop(old);
                        }
                    };
                    match Reader::start(&self.reader, Body(o), done) {
                        Ok(()) => self.info = Some(info),
                        Err(e) => self.failure = Some(format!("Spotify: {e}")),
                    }
                }
                Err(e) => self.failure = Some(describe(&e)),
            }
        }
        match (self.info, &self.failure) {
            (Some(info), _) => Ok(info),
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
        let info = self.open()?;
        if buf.is_empty() || self.pos >= info.len {
            return Ok(0);
        }
        let n = self.reader.read_at(self.pos, buf)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for SpotifyStream {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let info = self.open()?;
        let target = match pos {
            SeekFrom::Start(p) => i128::from(p),
            SeekFrom::End(d) => i128::from(info.len) + i128::from(d),
            SeekFrom::Current(d) => i128::from(self.pos) + i128::from(d),
        };
        self.pos = u64::try_from(target)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "seek before start"))?;
        Ok(self.pos)
    }
}

impl AudioStream for SpotifyStream {
    fn byte_len(&self) -> Option<u64> {
        self.info.map(|i| i.len)
    }

    fn format_hint(&self) -> &'static str {
        format::OGG
    }

    fn gain_db(&self) -> Option<f32> {
        self.info.and_then(|i| i.gain_db)
    }

    fn duration(&self) -> Option<Duration> {
        self.info.and_then(|i| i.duration)
    }

    fn quality(&self) -> Option<String> {
        self.info
            .map(|i| nikgob_core::streaming::quality_label("Vorbis", i.kbps, false))
    }

    fn interrupter(&self) -> Option<StreamInterrupter> {
        let flag = Arc::clone(&self.interrupted);
        let reader = Arc::clone(&self.reader);
        Some(Arc::new(move |on| {
            flag.store(on, Ordering::SeqCst);
            reader.interrupt(on);
        }))
    }
}

impl Drop for SpotifyStream {
    fn drop(&mut self) {
        // The reader thread ends once a read librespot still waits in is
        // over, and keeps the track (see `keep_in`).
        self.reader.close();
    }
}

/// An opened track as the reader thread reads it.
struct Body(Opened);

impl Read for Body {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.body.read(buf)
    }
}

impl Seek for Body {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.0.body.seek(pos)
    }
}

/// The most one read asks of the reader thread.
const READ_MAX: usize = 64 * 1024;

/// Reads of a source done on a thread of its own (see [`SpotifyStream`]):
/// one read at a time, asked by position; the caller's wait for the answer
/// gives way to an interrupt.
#[derive(Default)]
struct Reader {
    state: Mutex<ReadState>,
    changed: Condvar,
}

#[derive(Default)]
struct ReadState {
    /// The read asked for and not taken yet: `(number, position, length)`.
    ask: Option<(u64, u64, usize)>,
    /// The answer to read `number`.
    answer: Option<(u64, io::Result<Vec<u8>>)>,
    /// The thread is reading.
    busy: bool,
    asked: u64,
    interrupted: bool,
    closed: bool,
}

impl Reader {
    fn lock(&self) -> MutexGuard<'_, ReadState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn wait<'a>(&self, st: MutexGuard<'a, ReadState>) -> MutexGuard<'a, ReadState> {
        self.changed.wait(st).unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts the thread over `body` (at position 0); `done` gets it back
    /// when the thread ends.
    fn start<B: Read + Seek + Send + 'static>(
        this: &Arc<Self>,
        body: B,
        done: impl FnOnce(B) + Send + 'static,
    ) -> io::Result<()> {
        let me = Arc::clone(this);
        std::thread::Builder::new()
            .name("nikgob-spotify-read".into())
            .spawn(move || done(me.serve(body)))
            .map(drop)
    }

    /// The reader thread: answers reads until closed.
    fn serve<B: Read + Seek>(&self, mut body: B) -> B {
        let mut at = 0u64;
        loop {
            let mut st = self.lock();
            let (n, pos, len) = loop {
                if let Some(ask) = st.ask.take() {
                    break ask;
                }
                if st.closed {
                    return body;
                }
                st = self.wait(st);
            };
            st.busy = true;
            drop(st);
            let read = (|| {
                if at != pos {
                    body.seek(SeekFrom::Start(pos))?;
                    at = pos;
                }
                let mut buf = vec![0; len];
                let got = body.read(&mut buf)?;
                buf.truncate(got);
                at += got as u64;
                Ok(buf)
            })();
            if read.is_err() {
                // Where the source stands after a failure is not known.
                at = u64::MAX;
            }
            let mut st = self.lock();
            st.busy = false;
            st.answer = Some((n, read));
            self.changed.notify_all();
        }
    }

    /// Reads at `pos` into `buf`, waiting for the thread unless interrupted.
    fn read_at(&self, pos: u64, buf: &mut [u8]) -> io::Result<usize> {
        let mut st = self.lock();
        // One read at a time: one interrupted earlier may still be running.
        while st.ask.is_some() || st.busy {
            if st.interrupted {
                return Err(interrupted_read());
            }
            st = self.wait(st);
        }
        st.asked += 1;
        let n = st.asked;
        st.answer = None;
        st.ask = Some((n, pos, buf.len().min(READ_MAX)));
        self.changed.notify_all();
        loop {
            if st.answer.as_ref().is_some_and(|(m, _)| *m == n) {
                let (_, read) = st.answer.take().expect("just seen");
                let bytes = read?;
                buf[..bytes.len()].copy_from_slice(&bytes);
                return Ok(bytes.len());
            }
            if st.interrupted {
                return Err(interrupted_read());
            }
            st = self.wait(st);
        }
    }

    fn interrupt(&self, on: bool) {
        self.lock().interrupted = on;
        self.changed.notify_all();
    }

    fn close(&self) {
        self.lock().closed = true;
        self.changed.notify_all();
    }
}

/// Runs `open` on the runtime and waits for it on this (engine) thread,
/// which may be any thread: inside or outside a runtime. Gives up when
/// `stop` is set or it takes too long; the open's task is stopped then
/// (what it had opened is closed with it).
pub fn block_on_runtime<T: Send + 'static>(
    handle: &tokio::runtime::Handle,
    stop: &AtomicBool,
    fut: impl Future<Output = Result<T, StreamError>> + Send + 'static,
) -> Result<T, StreamError> {
    let (tx, rx) = mpsc::sync_channel(1);
    let task = handle.spawn(async move {
        let _ = tx.send(fut.await);
    });
    let deadline = Instant::now() + OPEN_TIMEOUT;
    let given_up = |why: &str| {
        task.abort();
        Err(StreamError::Network(why.into()))
    };
    loop {
        if stop.load(Ordering::SeqCst) {
            return given_up("interrupted");
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return given_up("Spotify did not answer");
        }
        match rx.recv_timeout(left.min(OPEN_POLL)) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return given_up("Spotify did not answer"),
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

    /// Bytes whose reads past `stall` wait until `go` is set, as librespot
    /// waits for a range that does not come.
    struct Stalling {
        bytes: Cursor<Vec<u8>>,
        stall: u64,
        go: Arc<AtomicBool>,
    }

    impl Read for Stalling {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            while self.bytes.position() >= self.stall && !self.go.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
            self.bytes.read(buf)
        }
    }

    impl Seek for Stalling {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            self.bytes.seek(pos)
        }
    }

    #[test]
    fn a_read_waiting_in_librespot_gives_way_to_an_interrupt() {
        let data: Vec<u8> = (0..=255).collect();
        let go = Arc::new(AtomicBool::new(false));
        let reader = Arc::new(Reader::default());
        let (back_tx, back_rx) = mpsc::channel();
        let body = Stalling { bytes: Cursor::new(data.clone()), stall: 100, go: Arc::clone(&go) };
        Reader::start(&reader, body, move |b| back_tx.send(b.bytes.position()).unwrap()).unwrap();
        let mut buf = [0u8; 50];
        assert_eq!(reader.read_at(10, &mut buf).unwrap(), 50);
        assert_eq!(buf[..], data[10..60]);

        // Past the stall: the caller waits until it is interrupted, then
        // returns at once while librespot's read goes on.
        let r = Arc::clone(&reader);
        let t = std::thread::spawn(move || {
            let mut buf = [0u8; 20];
            let began = Instant::now();
            let got = r.read_at(150, &mut buf);
            (got.is_err(), began.elapsed())
        });
        std::thread::sleep(Duration::from_millis(100));
        assert!(!t.is_finished(), "the read waits");
        reader.interrupt(true);
        let (failed, took) = t.join().unwrap();
        assert!(failed);
        assert!(took < Duration::from_secs(2), "{took:?}");
        // While interrupted, a read behind the one still running fails too.
        assert!(reader.read_at(0, &mut buf).is_err());

        // Lifted, and the data comes: reads go on from anywhere.
        reader.interrupt(false);
        go.store(true, Ordering::SeqCst);
        assert_eq!(reader.read_at(0, &mut buf).unwrap(), 50);
        assert_eq!(buf[..], data[..50]);
        reader.close();
        back_rx.recv_timeout(Duration::from_secs(2)).expect("the thread ends and hands the body back");
    }

    #[test]
    fn an_open_that_does_not_answer_gives_up_when_interrupted() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        /// Set when the open's work is dropped (its task stopped).
        struct Gone(Arc<AtomicBool>);
        impl Drop for Gone {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let gone = Arc::new(AtomicBool::new(false));
        let g = Gone(Arc::clone(&gone));
        let mut s = SpotifyStream::new(Box::new({
            let handle = rt.handle().clone();
            move |stop| {
                block_on_runtime(&handle, stop, async move {
                    let _g = g;
                    std::future::pending::<Result<Opened, StreamError>>().await
                })
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
        let t = Instant::now();
        while !gone.load(Ordering::SeqCst) {
            assert!(t.elapsed() < Duration::from_secs(2), "the open's task is stopped");
            std::thread::sleep(Duration::from_millis(1));
        }
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
