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
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
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
    // mode prefetches ahead of the read position. A header read cannot be
    // stopped once it runs (librespot gives it up by itself): at most
    // `MAX_HEADER_READS` run at once, a given-up open's one included.
    let permit = header_reads()
        .acquire_owned()
        .await
        .map_err(|e| failed(StreamError::Network(e.to_string())))?;
    let header = tokio::task::spawn_blocking(move || {
        let _permit = permit;
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

/// Header reads running at most (see [`open`]).
const MAX_HEADER_READS: usize = 3;

fn header_reads() -> Arc<tokio::sync::Semaphore> {
    static READS: std::sync::OnceLock<Arc<tokio::sync::Semaphore>> = std::sync::OnceLock::new();
    Arc::clone(READS.get_or_init(|| Arc::new(tokio::sync::Semaphore::new(MAX_HEADER_READS))))
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
/// nothing outside it can wake, and a file has one reading position. So the
/// opened track is read on a thread of its own ([`Reader`]), and the
/// engine's thread waits for that thread's answer where the
/// [`interrupter`](AudioStream::interrupter) reaches it: a read the engine
/// interrupts returns at once, while librespot's read finishes on the
/// reader thread (it gives up by itself after a few seconds without data).
/// What was read is kept, so a seek back into it is answered at once, also
/// while librespot still waits elsewhere. At most [`MAX_READERS`] such
/// threads live at once (stalled ones included); past that a stream reads
/// on the caller's thread, as before the reader existed. The interrupter
/// also ends a wait for the open, whose work on the runtime is then
/// stopped.
pub struct SpotifyStream {
    opener: Option<Opener>,
    /// The engine interrupted the stream (the open looks at this).
    interrupted: Arc<AtomicBool>,
    reader: Arc<Reader>,
    /// The opened track read on this thread: no reader thread was free.
    direct: Option<Body>,
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

/// Streams dropped so far: the number of the last one, whose track is the
/// one to keep in [`Recent`]. A track handed back later by a reader thread
/// (librespot still waited in it) is kept only if no stream was dropped
/// since its own.
static DROPPED: AtomicU64 = AtomicU64::new(0);

/// Keeps `opened` (of stream number `dropped`) as the recent track, unless
/// a stream dropped later came first.
fn keep_recent(keep: Option<(String, Recent)>, opened: Opened, dropped: u64) {
    if let Some((id, recent)) = keep {
        keep_latest(&recent, id, opened, dropped);
    }
}

/// [`keep_recent`] for any kind of kept value.
fn keep_latest<T>(slot: &Mutex<Option<(String, T)>>, id: String, value: T, dropped: u64) {
    let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
    if DROPPED.load(Ordering::SeqCst) == dropped {
        let old = slot.replace((id, value));
        drop(slot);
        drop(old);
    }
}

impl SpotifyStream {
    pub fn new(opener: Opener) -> Self {
        Self {
            opener: Some(opener),
            interrupted: Arc::new(AtomicBool::new(false)),
            reader: Arc::new(Reader::default()),
            direct: None,
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
                    self.info = Some(Info {
                        len: o.body.len(),
                        gain_db: o.gain_db,
                        duration: o.duration,
                        kbps: o.kbps,
                    });
                    let keep = self.keep.clone();
                    let done = move |body: Body, dropped: u64| keep_recent(keep, body.0, dropped);
                    if let Err(body) = Reader::start(&self.reader, Body(o), done) {
                        log::debug!("spotify: every reader thread is busy, reading on this thread");
                        self.direct = Some(body);
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

/// Machine-readable tag included in [`describe`] when Spotify requires Premium.
pub const TAG_PREMIUM_REQUIRED: &str = "[spotify:premium_required]";
/// Machine-readable tag included in [`describe`] when Spotify playback is not logged in.
pub const TAG_NOT_LOGGED_IN: &str = "[spotify:not_logged_in]";
/// Machine-readable tag included in [`describe`] when a Spotify track is unavailable.
pub const TAG_UNAVAILABLE: &str = "[spotify:unavailable]";

/// The error text the engine shows.
pub fn describe(e: &StreamError) -> String {
    let tag = match e {
        StreamError::PremiumRequired => Some(TAG_PREMIUM_REQUIRED),
        StreamError::NotLoggedIn => Some(TAG_NOT_LOGGED_IN),
        StreamError::Unavailable => Some(TAG_UNAVAILABLE),
        _ => None,
    };
    match tag {
        Some(tag) => format!("{tag} Spotify: {e}"),
        None => format!("Spotify: {e}"),
    }
}

impl Read for SpotifyStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let info = self.open()?;
        if buf.is_empty() || self.pos >= info.len {
            return Ok(0);
        }
        let n = match &mut self.direct {
            Some(body) => {
                body.seek(SeekFrom::Start(self.pos))?;
                body.read(buf)?
            }
            None => self.reader.read_at(self.pos, buf)?,
        };
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
        // Only a stream that opened its track counts: one dropped while it
        // opened has nothing to keep and must not displace one that has.
        let dropped = if self.info.is_some() {
            DROPPED.fetch_add(1, Ordering::SeqCst) + 1
        } else {
            0
        };
        if let Some(body) = self.direct.take() {
            keep_recent(self.keep.take(), body.0, dropped);
        }
        // The reader thread ends once a read librespot still waits in is
        // over, and keeps the track if nothing newer was kept meanwhile.
        self.reader.close(dropped);
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
/// Reader threads alive at most, the ones librespot still holds included.
pub const MAX_READERS: usize = 4;
/// Reader threads alive now.
static READERS: AtomicUsize = AtomicUsize::new(0);
/// What is read is kept in pieces of this size.
const KEPT_BLOCK: u64 = 64 * 1024;
/// At most this much of a track is kept (a long mix keeps its start).
const KEPT_MAX: u64 = 16 * 1024 * 1024;

/// Reads of a source done on a thread of its own (see [`SpotifyStream`]):
/// one read at a time, asked by position; the caller's wait for the answer
/// gives way to an interrupt, and what was read is kept and answered from.
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
    /// The stream was dropped, as stream number this.
    closed: Option<u64>,
    kept: Kept,
}

/// The bytes read so far, by position.
#[derive(Default)]
struct Kept {
    blocks: std::collections::HashMap<u64, Box<[u8]>>,
    /// Sorted, disjoint, non-adjacent ranges of kept bytes.
    have: Vec<std::ops::Range<u64>>,
}

impl Kept {
    fn put(&mut self, pos: u64, bytes: &[u8]) {
        let end = pos + bytes.len() as u64;
        if bytes.is_empty() || end > KEPT_MAX {
            return;
        }
        let mut at = pos;
        while at < end {
            let block = at / KEPT_BLOCK;
            let off = (at % KEPT_BLOCK) as usize;
            let n = ((KEPT_BLOCK as usize) - off).min((end - at) as usize);
            let from = (at - pos) as usize;
            self.blocks
                .entry(block)
                .or_insert_with(|| vec![0; KEPT_BLOCK as usize].into_boxed_slice())[off..off + n]
                .copy_from_slice(&bytes[from..from + n]);
            at += n as u64;
        }
        let (mut start, mut stop) = (pos, end);
        self.have.retain(|r| {
            if r.end < start || r.start > stop {
                return true;
            }
            start = start.min(r.start);
            stop = stop.max(r.end);
            false
        });
        let i = self.have.partition_point(|r| r.start < start);
        self.have.insert(i, start..stop);
    }

    /// Copies kept bytes from `pos` into `buf`; how many (0: not kept).
    fn get(&self, pos: u64, buf: &mut [u8]) -> usize {
        let Some(run) = self.have.iter().find(|r| r.start <= pos && pos < r.end) else {
            return 0;
        };
        let n = buf.len().min((run.end - pos) as usize);
        let mut done = 0;
        while done < n {
            let at = pos + done as u64;
            let off = (at % KEPT_BLOCK) as usize;
            let m = ((KEPT_BLOCK as usize) - off).min(n - done);
            let block = &self.blocks[&(at / KEPT_BLOCK)];
            buf[done..done + m].copy_from_slice(&block[off..off + m]);
            done += m;
        }
        n
    }
}

impl Reader {
    fn lock(&self) -> MutexGuard<'_, ReadState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn wait<'a>(&self, st: MutexGuard<'a, ReadState>) -> MutexGuard<'a, ReadState> {
        self.changed.wait(st).unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts the thread over `body` (at position 0); `done` gets it back,
    /// with the number of its stream, when the thread ends. `body` back
    /// when no reader thread is free ([`MAX_READERS`]).
    fn start<B: Read + Seek + Send + 'static>(
        this: &Arc<Self>,
        body: B,
        done: impl FnOnce(B, u64) + Send + 'static,
    ) -> Result<(), B> {
        if READERS.fetch_add(1, Ordering::SeqCst) >= MAX_READERS {
            READERS.fetch_sub(1, Ordering::SeqCst);
            return Err(body);
        }
        let me = Arc::clone(this);
        let body = Arc::new(Mutex::new(Some(body)));
        let mine = Arc::clone(&body);
        let spawned = std::thread::Builder::new()
            .name("nikgob-spotify-read".into())
            .spawn(move || {
                let body = mine.lock().unwrap_or_else(PoisonError::into_inner).take();
                if let Some(body) = body {
                    let body = me.serve(body);
                    let dropped = me.lock().closed.unwrap_or(0);
                    done(body, dropped);
                }
                READERS.fetch_sub(1, Ordering::SeqCst);
            });
        match spawned {
            Ok(_) => Ok(()),
            Err(_) => {
                READERS.fetch_sub(1, Ordering::SeqCst);
                let body = body.lock().unwrap_or_else(PoisonError::into_inner).take();
                body.map_or(Ok(()), Err)
            }
        }
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
                if st.closed.is_some() {
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
            if let Ok(bytes) = &read {
                st.kept.put(pos, bytes);
            }
            st.busy = false;
            st.answer = Some((n, read));
            self.changed.notify_all();
        }
    }

    /// Reads at `pos` into `buf`: from what was read before, else from the
    /// thread, waiting for it unless interrupted.
    fn read_at(&self, pos: u64, buf: &mut [u8]) -> io::Result<usize> {
        let mut st = self.lock();
        let kept = st.kept.get(pos, buf);
        if kept > 0 {
            return Ok(kept);
        }
        // One read at a time: one interrupted earlier may still be running.
        while st.ask.is_some() || st.busy {
            if st.interrupted {
                return Err(interrupted_read());
            }
            st = self.wait(st);
            // It may have brought the bytes asked for now.
            let kept = st.kept.get(pos, buf);
            if kept > 0 {
                return Ok(kept);
            }
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

    /// The stream (number `dropped`) is gone: the thread ends.
    fn close(&self, dropped: u64) {
        self.lock().closed = Some(dropped);
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
        let started = Reader::start(&reader, body, move |b, _| back_tx.send(b.bytes.position()).unwrap());
        assert!(started.is_ok());
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

        // A seek back into what was read is answered at once, though
        // librespot still waits where the read was abandoned.
        reader.interrupt(false);
        let began = Instant::now();
        assert_eq!(reader.read_at(20, &mut buf).unwrap(), 40);
        assert_eq!(buf[..40], data[20..60]);
        assert!(began.elapsed() < Duration::from_millis(500), "{:?}", began.elapsed());

        // The data comes: reads go on from anywhere.
        go.store(true, Ordering::SeqCst);
        assert_eq!(reader.read_at(0, &mut buf).unwrap(), 50);
        assert_eq!(buf[..], data[..50]);
        reader.close(0);
        back_rx.recv_timeout(Duration::from_secs(2)).expect("the thread ends and hands the body back");
    }

    #[test]
    fn reader_threads_are_capped_and_a_stream_past_the_cap_reads_on_its_own_thread() {
        let go = Arc::new(AtomicBool::new(false));
        let mut readers = Vec::new();
        let mut refused = 0;
        for _ in 0..=MAX_READERS {
            let reader = Arc::new(Reader::default());
            let body = Stalling { bytes: Cursor::new(vec![0; 10]), stall: 0, go: Arc::clone(&go) };
            match Reader::start(&reader, body, |_, _| {}) {
                Ok(()) => readers.push(reader),
                Err(_) => refused += 1,
            }
        }
        assert!(refused >= 1 && readers.len() <= MAX_READERS, "{}", readers.len());
        go.store(true, Ordering::SeqCst);
        for r in &readers {
            r.close(0);
        }
        let t = Instant::now();
        while READERS.load(Ordering::SeqCst) >= MAX_READERS {
            assert!(t.elapsed() < Duration::from_secs(2), "the threads end");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn a_late_track_does_not_displace_one_kept_after_it() {
        let recent: Mutex<Option<(String, u32)>> = Mutex::new(None);
        // Stream A is dropped, then stream B; B's track is kept at once, and
        // A's comes late (its reader was still held by librespot).
        let a = DROPPED.fetch_add(1, Ordering::SeqCst) + 1;
        let b = DROPPED.fetch_add(1, Ordering::SeqCst) + 1;
        keep_latest(&recent, "b".into(), 2, b);
        keep_latest(&recent, "a".into(), 1, a);
        let kept = recent.lock().unwrap().as_ref().map(|(id, _)| id.clone());
        assert_eq!(kept.as_deref(), Some("b"));
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

    #[test]
    fn describe_uses_machine_tags_without_localized_prose() {
        let cases = [
            (StreamError::PremiumRequired, TAG_PREMIUM_REQUIRED),
            (StreamError::NotLoggedIn, TAG_NOT_LOGGED_IN),
            (StreamError::Unavailable, TAG_UNAVAILABLE),
        ];
        for (err, expected_tag) in &cases {
            let msg = describe(err);
            assert!(msg.starts_with(expected_tag), "{msg}");
            assert!(msg.contains(&err.to_string()), "{msg}");
            assert!(msg.is_ascii(), "expected ASCII machine-tagged output: {msg}");

            let mut stream = SpotifyStream::new(Box::new({
                let err = err.clone();
                move |_| Err(err)
            }));
            let io_err = stream.read(&mut [0; 4]).unwrap_err().to_string();
            assert!(io_err.starts_with(expected_tag), "{io_err}");
            assert_eq!(stream.seek(SeekFrom::Start(0)).unwrap_err().to_string(), io_err);
        }

        let net = describe(&StreamError::Network("timed out".into()));
        assert_eq!(net, "Spotify: network error: timed out");
        for tag in [TAG_PREMIUM_REQUIRED, TAG_NOT_LOGGED_IN, TAG_UNAVAILABLE] {
            assert!(!net.contains(tag), "{net}");
        }
        assert_eq!(
            describe(&StreamError::RateLimited { retry_after: None }),
            "Spotify: rate limited"
        );
    }
}
