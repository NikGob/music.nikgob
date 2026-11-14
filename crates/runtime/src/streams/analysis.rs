//! Sound analysis of stream-only tracks: the liked and
//! playlist tracks without a file get their bliss vector from their song on
//! YouTube Music, stored as [`AnalysisSource::Youtube`] (a file's analysis
//! always wins, and a file that arrives later is analysed again by the file
//! job).
//!
//! - **While playing.** A track that played from YouTube audio is analysed
//!   when it ends from the service's disk cache ([`StreamService::cached_audio`]):
//!   not a single request. A Spotify or Deezer track that played through its
//!   YouTube Music song counts too (its source is that song); Spotify's own
//!   sound is never analysed.
//! - **In the background.** The queue of [`Library::streams_needing_analysis`],
//!   newest first, one track at a time: a track without a YouTube video has
//!   its song looked up the way playback does it ([`super::resolve`], with
//!   its 5-day miss cache), then the whole audio is read into memory through
//!   [`StreamService::open_audio`] (a few MB), decoded, analysed and dropped.
//!   Tracks are [`StreamSchedule::analysis_gap`] apart, at most
//!   [`StreamSchedule::analysis_daily`] downloads a day (the count is kept in
//!   the library's cache, so a restart does not start over), only while
//!   YouTube Music is not paused, the periodic network work is not idle,
//!   nothing waits for its sound (Buffering) and YouTube Music neither plays
//!   nor comes next. The last is the listener's budget: YouTube's bot check
//!   («Sign in to confirm you’re not a bot») answers its player, which
//!   playback and these downloads both ask, and it is the service's, not a
//!   client's (every client of the player is refused alike, see
//!   `nikgob_youtube::player`), so a pause of its own for the background
//!   would not keep playback safe. The songs that play are analysed from
//!   the cache when they end anyway. A rate limit is not a failure:
//!   the job waits for the pause to end. A real failure
//!   (no song, a video that does not play or decode) is marked and tried
//!   again after [`StreamSchedule::miss_ttl`].
//! - **Previews of new music.** The candidates asked for by
//!   [`Worker::want_preview`] (the songs «Моя волна» is about to play)
//!   come first, before the library's queue; when the library's queue is
//!   empty, the discoveries found for «Новое для тебя» get a preview
//!   analysed: the first
//!   [`PREVIEW_BYTES`] of the song (about the first minute), its 30 s from
//!   the 15th second ([`nikgob_recs::preview_windows`]), at most
//!   [`PREVIEW_DAILY`] a day apart from the library's downloads, so new
//!   music is placed by its sound. With «Библиотека и
//!   превью новых песен» the neural sound model hears the preview too, and
//!   [`nikgob_recs::VectorIndex::niche_fit`] can tell whether it fits a
//!   niche before it joins a mix.
//! - **The neural sound model** ([`crate::sound_model`]): with «Нейросетевой
//!   анализ звука» on, the library's stream tracks get its vector from the
//!   same windows as bliss's.
//!
//! Both run on one thread of their own at the lowest priority, paced like the
//! file job ([`crate::quiet`]: nothing new starts while a game runs); the
//! worker only hands it jobs and hears back [`Done`]. With
//! «Анализ звука стримов» off nothing starts, and a job in flight stops at
//! its next chunk.
//!
//! [`StreamSchedule::analysis_gap`]: super::StreamSchedule::analysis_gap
//! [`StreamSchedule::analysis_daily`]: super::StreamSchedule::analysis_daily
//! [`StreamSchedule::miss_ttl`]: super::StreamSchedule::miss_ttl

use std::collections::HashSet;
use std::io::{ErrorKind, Read};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use nikgob_core::model::{AnalysisSource, SourceId, SourceKind, TrackId};
use nikgob_core::streaming::{AudioStream, Service, StreamError, StreamService};
use nikgob_library::{Library, now_ms};
use nikgob_recs::{ANALYZED_WITH, EMBEDDED_WITH, SKETCHED_WITH, SoundModel};

use nikgob_view as v;

use super::Msg;
use super::resolve::Lookup;
use crate::Worker;
use crate::analysis::{FileError, SoundIndex, Want, analyze_bytes, store_embedding, store_sketch, store_vector};
use crate::sound_model::Effnet;
use crate::quiet::{self, Pacer, lower_priority};
use crate::import::Internal;

/// Most bytes of one stream read for its analysis (8 minutes of YouTube's
/// Opus are about 10 MB; whatever is longer is cut, as the analysis is).
const MAX_BYTES: usize = 48 << 20;
/// Bytes read at a time (the thread looks at the switch in between).
const CHUNK: usize = 256 << 10;
/// Candidates read from the library at a time.
const BATCH: u32 = 16;
/// Library cache key of the downloads for the analysis today: `day:count`
/// (UTC days since the epoch).
const DAY_KEY: &str = "stream_analysis.day";
const DAY_MS: i64 = 86_400_000;
/// Bytes of a new song's preview (about the first minute of YouTube's
/// Opus, held in memory only).
const PREVIEW_BYTES: usize = 1 << 20;
/// Previews of new music analysed a day.
const PREVIEW_DAILY: u32 = 60;
/// Candidates asked for by [`Worker::want_preview`] waiting at most.
const WANTED_MAX: usize = 64;
/// Library cache key of the previews analysed today: `day:count`.
const PREVIEW_DAY_KEY: &str = "stream_analysis.preview_day";
/// Free previews (Deezer's 30 seconds of the same song, no login) looked
/// for a day at most, and their count's key.
const FREE_DAILY: u32 = 300;
const FREE_DAY_KEY: &str = "stream_analysis.free_day";
/// Songs looked for on Deezer remembered in a run at most (then all are
/// forgotten and may be looked for again).
const FREE_TRIED_MAX: usize = 4096;
/// The next step after a song Deezer has no preview of.
const FREE_MISS_GAP: Duration = Duration::from_secs(3);

/// Where the audio of a job comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    /// The track just played: only the service's disk cache.
    Played,
    /// The background queue: the cache, else a download.
    Background,
    /// A new song's preview: the cache, else the head of a download.
    Preview,
}

/// How a job ended.
#[derive(Debug)]
pub(crate) enum Outcome {
    /// Analysed; `false` when a file's analysis was there already.
    Stored(bool),
    /// The cache does not have all of it (a played track that was skipped).
    NotCached,
    /// The switch went off (or the app is closing).
    Stopped,
    /// The service asks to wait: not a failure.
    RateLimited(Option<Duration>),
    /// Maybe later (no connection, the library busy).
    Transient(String),
    /// This song cannot be analysed: marked, tried again after days.
    Failed(String),
}

/// A job's result, for [`Worker::stream_analysed`].
pub(crate) struct Done {
    track: TrackId,
    origin: Origin,
    outcome: Outcome,
}

struct Job {
    track: TrackId,
    video: String,
    origin: Origin,
    /// YouTube Music, for a job of its audio.
    youtube: Option<Arc<dyn StreamService>>,
    /// The audio already at hand (a free preview) and its container.
    bytes: Option<(Vec<u8>, String)>,
    /// YouTube Music plays or comes next (see `held::Held::listening`).
    listening: Arc<AtomicBool>,
    /// The neural sound model, when it hears this job's audio.
    model: Option<Arc<Effnet>>,
}

/// What the background job is waiting for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// The track's song on YouTube Music ([`Msg::Resolved`]).
    Resolving,
    /// A free preview of its song on Deezer ([`Msg::FreePreview`]).
    Free,
    /// Its audio and analysis ([`Msg::Analysed`]).
    Analysing,
}

/// The worker's side of the stream analysis.
#[derive(Default)]
pub(crate) struct StreamAnalysis {
    jobs: Option<Sender<Job>>,
    /// The switch is off or the app is closing: the thread drops its job.
    off: Arc<AtomicBool>,
    /// The background track in work, and what it waits for.
    busy: Option<(TrackId, Step)>,
    /// Played tracks handed to the thread.
    played: HashSet<TrackId>,
    /// Not taken again in this run (the song cannot be looked up now).
    skip: HashSet<TrackId>,
    /// No background track before this (the gap, a rate-limit pause).
    next_at: Option<Instant>,
    /// The queue was empty: looked at again then, or when the library
    /// changes.
    idle_until: Option<Instant>,
    /// Today (UTC days) and the downloads made for the analysis.
    day: Option<(i64, u32)>,
    /// Today and the previews of new music analysed.
    preview_day: Option<(i64, u32)>,
    /// Candidates whose preview was asked for ([`Worker::want_preview`]),
    /// first come first served, before the discoveries: the track and its
    /// YouTube Music video (none: of another service, heard by a free
    /// preview only).
    wanted: std::collections::VecDeque<(TrackId, Option<String>)>,
    /// New songs whose preview was tried in this run.
    previewed: HashSet<TrackId>,
    /// Today and the free previews looked for, and the songs looked for
    /// in this run ([`FREE_TRIED_MAX`] at most).
    free_day: Option<(i64, u32)>,
    free_tried: HashSet<TrackId>,
    /// No song was left to look for a free preview of: not looked at
    /// again before this (or a song asked for by a wave).
    free_idle_until: Option<Instant>,
    /// The progress is counted again on the next tick.
    recount: bool,
    /// Why the last step did not start a track (logged once per change).
    held: Option<&'static str>,
}

impl StreamAnalysis {
    /// Stops the job in flight; nothing more is analysed.
    pub(crate) fn shutdown(&mut self) {
        self.off.store(true, Ordering::Relaxed);
        self.jobs = None;
    }
}

/// The analysis thread: one job at a time, until the worker goes, paced
/// by [`crate::quiet`] (none of its jobs is one the user waits for).
fn run(jobs: Receiver<Job>, lib: Library, index: SoundIndex, tx: Sender<Internal>, off: Arc<AtomicBool>) {
    lower_priority();
    let mut pacer = Pacer::heavy(&off, "stream analysis");
    for job in &jobs {
        // While a game runs the job waits; switched off, it stops at once.
        pacer.begin();
        let started = Instant::now();
        let mut job = job;
        let outcome = work(&mut job, &lib, &index, &off, &mut pacer);
        log::debug!(
            "stream analysis: {} ({:?}) {outcome:?} in {:?}",
            job.video,
            job.origin,
            started.elapsed()
        );
        let done = Done {
            track: job.track,
            origin: job.origin,
            outcome,
        };
        if tx.send(Internal::Stream(Msg::Analysed(done))).is_err() {
            break;
        }
        // No stream waits: what the decoding and the analysis left goes back.
        if jobs.is_empty() {
            crate::release_thread_memory();
        }
        pacer.rest();
    }
}

fn work(job: &mut Job, lib: &Library, index: &SoundIndex, off: &AtomicBool, pacer: &mut Pacer<'_>) -> Outcome {
    if off.load(Ordering::Relaxed) {
        return Outcome::Stopped;
    }
    let (bytes, ext) = match job.bytes.take() {
        Some(b) => b,
        None => match youtube_bytes(job, off) {
            Ok(b) => b,
            Err(o) => return o,
        },
    };
    let ext = ext.as_str();
    let model = job.model.as_deref().filter(|m| m.ready() && !m.resting()).map(|m| m as &dyn SoundModel);
    let want = Want { bliss: true, model };
    let preview = job.origin == Origin::Preview;
    match analyze_bytes(bytes, ext, &format!("youtube:{}", job.video), preview, want, pacer) {
        Ok(out) => {
            let Some(Ok(vector)) = out.bliss else { return Outcome::Failed("no vector".into()) };
            match store_vector(lib, index, job.track, vector, AnalysisSource::Youtube) {
                Ok(stored) => {
                    if let Some(e) = &out.embedding
                        && let Err(err) = store_embedding(lib, index, job.track, e, AnalysisSource::Youtube)
                    {
                        log::warn!("stream analysis: storing the sound model's vector of {}: {err}", job.track);
                    }
                    let heard = out.embedding.as_ref().filter(|r| !matches!(r, Err(nikgob_recs::AnalysisError::Model(_))));
                    if stored && let Err(err) = store_sketch(lib, index, job.track, out.sketch, heard, AnalysisSource::Youtube) {
                        log::warn!("stream analysis: storing the sketch of {}: {err}", job.track);
                    }
                    Outcome::Stored(stored)
                }
                Err(e) => Outcome::Transient(format!("storing: {e}")),
            }
        }
        Err(FileError::Stopped) => Outcome::Stopped,
        Err(FileError::Unavailable(e)) => Outcome::Transient(e),
        Err(FileError::Undecodable(e)) => Outcome::Failed(e),
    }
}

/// The audio of a YouTube Music job and its container: the disk cache,
/// else a download (none for a played track), never while the listener's
/// YouTube Music plays.
fn youtube_bytes(job: &Job, off: &AtomicBool) -> Result<(Vec<u8>, String), Outcome> {
    let Some(youtube) = job.youtube.as_ref() else { return Err(Outcome::Failed("no audio".into())) };
    let audio = match (youtube.cached_audio(&job.video), job.origin) {
        (Some(a), _) => a,
        (None, Origin::Played) => return Err(Outcome::NotCached),
        // The listener's YouTube Music came meanwhile: later.
        (None, Origin::Background | Origin::Preview) if job.listening.load(Ordering::Relaxed) => {
            return Err(Outcome::Transient("YouTube Music plays".into()));
        }
        (None, Origin::Background | Origin::Preview) => match youtube.open_audio(&job.video) {
            Ok(a) => a,
            Err(e) => return Err(stream_outcome(e)),
        },
    };
    // Opening asked nothing yet (the first read does): one more look.
    if job.origin != Origin::Played && job.listening.load(Ordering::Relaxed) {
        return Err(Outcome::Transient("YouTube Music plays".into()));
    }
    let ext = audio.format_hint().to_owned();
    let max = if job.origin == Origin::Preview { PREVIEW_BYTES } else { MAX_BYTES };
    Ok((read_all(audio, max, off)?, ext))
}

/// Drops from `wanted` what needs no preview any more: a song heard already
/// (`done`), and one looked for by a free preview with no YouTube Music
/// preview to fall back on (a song of another service, or `youtube`: none
/// can follow today).
fn prune_wanted(wanted: &mut std::collections::VecDeque<(TrackId, Option<String>)>, done: impl Fn(TrackId) -> bool, free_tried: &HashSet<TrackId>, youtube: bool) {
    wanted.retain(|(t, v)| !done(*t) && !(free_tried.contains(t) && (v.is_none() || !youtube)));
}

/// What may be asked to be heard soon ([`Worker::ask_room`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AskRoom {
    /// Songs at most.
    pub room: usize,
    /// YouTube Music's songs may be heard by its previews today.
    pub youtube: bool,
    /// Free previews can be looked for (a song of any service).
    pub free: bool,
}

/// The container of a preview by its URL's extension (Deezer's are MP3).
fn ext_of(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or("");
    match path.rsplit_once('.') {
        Some((_, e)) if (2..=4).contains(&e.len()) && e.chars().all(|c| c.is_ascii_alphanumeric()) => e.to_ascii_lowercase(),
        _ => "mp3".into(),
    }
}

/// The whole stream (up to `max` bytes) in memory.
fn read_all(mut audio: Box<dyn AudioStream>, max: usize, off: &AtomicBool) -> Result<Vec<u8>, Outcome> {
    let mut bytes = Vec::with_capacity(audio.byte_len().map_or(4 << 20, |n| n as usize).min(max));
    let mut buf = vec![0u8; CHUNK];
    while bytes.len() < max {
        if off.load(Ordering::Relaxed) {
            return Err(Outcome::Stopped);
        }
        match audio.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => bytes.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            // A service error comes through the reader as its source.
            Err(e) => {
                return Err(match e.get_ref().and_then(|x| x.downcast_ref::<StreamError>()) {
                    Some(se) => stream_outcome(se.clone()),
                    None => Outcome::Transient(e.to_string()),
                });
            }
        }
    }
    Ok(bytes)
}

fn stream_outcome(e: StreamError) -> Outcome {
    match e {
        StreamError::RateLimited { retry_after } => Outcome::RateLimited(retry_after),
        StreamError::Network(_) | StreamError::Http { .. } | StreamError::NotLoggedIn | StreamError::NotConfigured => {
            Outcome::Transient(e.to_string())
        }
        StreamError::Unavailable
        | StreamError::AgeRestricted
        | StreamError::RegionBlocked
        | StreamError::Drm
        | StreamError::Unsupported
        | StreamError::PremiumRequired
        | StreamError::Parse(_) => Outcome::Failed(e.to_string()),
    }
}

impl Worker {
    fn stream_analysis_on(&self) -> bool {
        self.state.read().prefs.stream_analysis
    }

    /// Hands a job to the analysis thread (started on the first one); the
    /// neural model goes along where its mode hears this kind of job.
    fn analysis_job(&mut self, mut job: Job) -> bool {
        let neural = self.analysis.neural();
        let hears = if job.origin == Origin::Preview { neural.previews } else { neural.library };
        job.model = if hears { self.analysis.effnet() } else { None };
        let a = &mut self.streaming.analysis;
        if a.jobs.is_none() {
            let (tx, rx) = crossbeam_channel::unbounded();
            let (lib, index, internal, off) =
                (self.lib.clone(), self.analysis.index().clone(), self.streaming.internal.clone(), a.off.clone());
            let spawned = std::thread::Builder::new()
                .name("nikgob-stream-analysis".into())
                .spawn(move || run(rx, lib, index, internal, off));
            match spawned {
                Ok(_) => a.jobs = Some(tx),
                Err(e) => {
                    log::warn!("stream analysis: cannot start: {e}");
                    return false;
                }
            }
        }
        a.jobs.as_ref().is_some_and(|tx| tx.send(job).is_ok())
    }

    /// The track has an analysis of the current version (a file's or a
    /// stream's), and the neural model's vector when the model hears the
    /// library.
    fn analysed(&self, track: TrackId) -> bool {
        let bliss = self.lib.get_analysis(track).ok().flatten().is_some_and(|a| a.analyzed_with == ANALYZED_WITH);
        let model = self.analysis.effnet().is_some_and(|m| m.ready());
        bliss && (!(self.analysis.neural().library && model) || self.lib.has_embedding(track, EMBEDDED_WITH).unwrap_or(true))
    }

    /// The track has a sketch of the current version (a file's or a
    /// stream's), or a file's analysis (the file job sketches it).
    fn sketched(&self, track: TrackId) -> bool {
        match self.lib.sketch_of(track).ok().flatten() {
            Some(s) => s.sketched_with == SKETCHED_WITH,
            None => self.lib.get_analysis(track).ok().flatten().is_some_and(|a| a.source == AnalysisSource::File),
        }
    }

    /// Asks for the preview of a new song (`track`, its YouTube Music
    /// `video` if it is one: a song of another service is heard by a free
    /// preview only) to be analysed soon, before the discoveries' own: the niche
    /// expansion's candidates get their sound vector (and, with «Библиотека
    /// и превью новых песен», the neural model's) so
    /// [`nikgob_recs::VectorIndex::niche_fit`] can judge them. Previews keep
    /// to the day's limit; a track analysed already or asked for twice is
    /// left out, and the queue holds [`WANTED_MAX`] at most (the oldest go).
    /// «Моя волна» asks for the new songs it is about to play
    /// ([`Worker::wave_asked`]).
    pub(crate) fn want_preview(&mut self, track: TrackId, video: Option<String>) {
        let a = &mut self.streaming.analysis;
        if a.previewed.contains(&track) || a.wanted.iter().any(|(t, _)| *t == track) || self.analysis.index().read().contains(track) {
            return;
        }
        let a = &mut self.streaming.analysis;
        a.wanted.push_back((track, video));
        a.free_idle_until = None;
        while a.wanted.len() > WANTED_MAX {
            a.wanted.pop_front();
        }
        a.idle_until = None;
    }

    /// How many previews of new music may still be asked today: none while
    /// the stream analysis is off or YouTube Music has no audio, else what
    /// the day's limit leaves beside the previews waiting, within the
    /// queue.
    pub(crate) fn preview_room(&mut self) -> usize {
        if !self.stream_analysis_on() || self.service(Service::Youtube).is_none() || !self.has_audio(Service::Youtube) {
            return 0;
        }
        let waiting = self.streaming.analysis.wanted.len();
        let today = PREVIEW_DAILY.saturating_sub(self.previews_today()) as usize;
        today.saturating_sub(waiting).min(WANTED_MAX.saturating_sub(waiting))
    }

    /// How many songs may still be asked to be heard soon, and whether by
    /// a YouTube Music preview and by a free one: a free preview has a day
    /// of its own ([`FREE_DAILY`]), so a song of any service is asked
    /// while it lasts, whatever YouTube Music's day left.
    pub(crate) fn ask_room(&mut self) -> AskRoom {
        if !self.stream_analysis_on() {
            return AskRoom::default();
        }
        self.wanted_prune();
        let youtube = self.preview_room() > 0;
        let free = self.free_open();
        let room = if youtube || free { WANTED_MAX.saturating_sub(self.streaming.analysis.wanted.len()) } else { 0 };
        AskRoom { room, youtube, free }
    }

    /// A YouTube Music preview could start now: every gate the background
    /// job meets lets it (its service has audio and does not ask to wait,
    /// today has room for one, the listener's YouTube Music does not play
    /// or come next, playback does not buffer or look songs up, no game is
    /// in front, the app is not idle).
    fn youtube_preview_ready(&mut self) -> bool {
        self.service(Service::Youtube).is_some()
            && self.has_audio(Service::Youtube)
            && self.pause_left(Service::Youtube).is_none()
            && self.previews_today() < PREVIEW_DAILY
            && !self.youtube_listened()
            && !self.streams_idle()
            && self.status != v::Status::Buffering
            && self.streaming.resolving.is_empty()
            && !quiet::busy()
    }

    /// YouTube Music's preview can still follow a free one that missed
    /// today (its service is there and today has room), whatever holds it
    /// back this moment.
    fn youtube_fallback_left(&mut self) -> bool {
        self.service(Service::Youtube).is_some() && self.previews_today() < PREVIEW_DAILY
    }

    /// The songs asked for that need no preview any more leave the queue
    /// ([`prune_wanted`]).
    fn wanted_prune(&mut self) {
        let fallback = self.youtube_fallback_left();
        let index = self.analysis.index().clone();
        let a = &mut self.streaming.analysis;
        let previewed = &a.previewed;
        prune_wanted(&mut a.wanted, |t| previewed.contains(&t) || index.read().contains(t), &a.free_tried, fallback);
    }

    /// Free previews can be looked for (today's are not used up, the
    /// clients are there, and the app is not idle: it looks for them then
    /// as it does every other background request).
    fn free_open(&mut self) -> bool {
        self.free_today() < FREE_DAILY && self.graph_metadata().is_some() && !self.streams_idle()
    }

    /// The playing entry of `track` ended (or was skipped) while it played
    /// from `source`: a YouTube video whose whole audio is in the disk
    /// cache is analysed from there (no request).
    pub(crate) fn analyse_played(&mut self, track: TrackId, source: SourceId) {
        if !self.stream_analysis_on() || self.streaming.analysis.played.contains(&track) {
            return;
        }
        let Some(src) = self.lib.get_source(source).ok().flatten() else { return };
        if src.kind != SourceKind::Youtube || (self.analysed(track) && self.sketched(track)) {
            return;
        }
        let Some(youtube) = self.service(Service::Youtube) else { return };
        let job = Job {
            track,
            video: src.locator,
            origin: Origin::Played,
            youtube: Some(youtube),
            bytes: None,
            listening: self.streaming.held.listening.clone(),
            model: None,
        };
        if self.analysis_job(job) {
            self.streaming.analysis.played.insert(track);
        }
    }

    /// The library changed (likes, playlists, imports): the progress is
    /// counted again and an empty queue is looked at again.
    pub(crate) fn stream_analysis_changed(&mut self) {
        let a = &mut self.streaming.analysis;
        a.recount = true;
        a.idle_until = None;
    }

    /// «Анализ звука стримов» switched: off stops the job in flight at
    /// once, on starts the queue.
    pub(crate) fn stream_analysis_pref(&mut self, on: bool) {
        let a = &mut self.streaming.analysis;
        a.off.store(!on, Ordering::Relaxed);
        a.idle_until = None;
        if !on {
            a.busy = None;
            return;
        }
        self.stream_analysis_step();
    }

    /// Every tick: the progress if something changed, and the next
    /// background track when it is time.
    pub(crate) fn stream_analysis_tick(&mut self) {
        if std::mem::take(&mut self.streaming.analysis.recount) {
            self.publish_analysis_counts();
        }
        // (Kept for the analysis thread too.)
        self.youtube_listened();
        self.stream_analysis_step();
    }

    /// The analysis progress as the library counts it now.
    fn publish_analysis_counts(&mut self) {
        let c = match self.lib.analysis_counts(ANALYZED_WITH) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("stream analysis: counting: {e}");
                return;
            }
        };
        let p = v::AnalysisProgress {
            files_done: c.files_done,
            files_total: c.files_total,
            streams_done: c.streams_done,
            streams_total: c.streams_total,
        };
        if self.state.read().analysis != p {
            self.publish(|s| s.analysis = p);
        }
    }

    /// Downloads made today for the analysis.
    fn analysis_downloads(&mut self) -> u32 {
        let today = now_ms() / DAY_MS;
        if let Some((day, n)) = self.streaming.analysis.day
            && day == today
        {
            return n;
        }
        let n = self.day_count(DAY_KEY);
        self.streaming.analysis.day = Some((today, n));
        n
    }

    fn count_download(&mut self) {
        let n = self.analysis_downloads() + 1;
        self.streaming.analysis.day = Some((now_ms() / DAY_MS, n));
        self.day_count_put(DAY_KEY, n);
    }

    /// Previews of new music analysed today.
    fn previews_today(&mut self) -> u32 {
        let today = now_ms() / DAY_MS;
        if let Some((day, n)) = self.streaming.analysis.preview_day
            && day == today
        {
            return n;
        }
        let n = self.day_count(PREVIEW_DAY_KEY);
        self.streaming.analysis.preview_day = Some((today, n));
        n
    }

    fn count_preview(&mut self) {
        let n = self.previews_today() + 1;
        self.streaming.analysis.preview_day = Some((now_ms() / DAY_MS, n));
        self.day_count_put(PREVIEW_DAY_KEY, n);
    }

    /// Today's count kept under `key`.
    fn day_count(&self, key: &str) -> u32 {
        let today = now_ms() / DAY_MS;
        self.lib
            .cache_get(key)
            .ok()
            .flatten()
            .and_then(|(_, v)| {
                let (day, n) = v.split_once(':')?;
                (day.parse::<i64>().ok()? == today).then(|| n.parse::<u32>().ok())?
            })
            .unwrap_or(0)
    }

    fn day_count_put(&self, key: &str, n: u32) {
        let today = now_ms() / DAY_MS;
        if let Err(e) = self.lib.cache_put(key, now_ms(), &format!("{today}:{n}")) {
            log::warn!("stream analysis: keeping the day's count: {e}");
        }
    }

    /// The next background track after `gap` ± 25 %.
    fn analysis_after(&mut self, gap: Duration) {
        self.analysis_wait(gap.mul_f64(0.75 + 0.5 * fastrand::f64()));
    }

    /// No background track for `wait`; the job is woken then.
    fn analysis_wait(&mut self, wait: Duration) {
        self.streaming.analysis.next_at = Some(Instant::now() + wait);
        self.streaming.spawn(async move {
            tokio::time::sleep(wait).await;
            Msg::AnalysisWake
        });
    }

    /// Starts the next background track if everything allows it (see the
    /// module docs).
    pub(crate) fn stream_analysis_step(&mut self) {
        let now = Instant::now();
        let a = &self.streaming.analysis;
        if a.busy.is_some()
            || a.next_at.is_some_and(|t| now < t)
            || a.idle_until.is_some_and(|t| now < t)
            || !self.stream_analysis_on()
        {
            return;
        }
        // A free preview of a new song (Deezer's): no request of YouTube
        // Music's player, so it waits for neither its pause nor its
        // listener; playback first still.
        if self.free_preview_step() {
            self.streaming.analysis.held = None;
            return;
        }

        if self.service(Service::Youtube).is_none() || !self.has_audio(Service::Youtube) {
            self.analysis_held("YouTube Music has no audio now");
            return;
        }
        if let Some(left) = self.pause_left(Service::Youtube) {
            self.analysis_held("YouTube Music is paused");
            self.analysis_wait(left + super::schedule::wake_jitter());
            return;
        }
        // The listener's YouTube Music goes first: no request of its player
        // in the background while it plays or comes next.
        if self.youtube_listened() {
            self.analysis_held("YouTube Music plays");
            return;
        }
        // Playback and its lookups go first.
        if self.streams_idle() {
            self.analysis_held("the app is idle");
            return;
        }
        if self.status == v::Status::Buffering {
            self.analysis_held("playback is buffering");
            return;
        }
        if !self.streaming.resolving.is_empty() {
            self.analysis_held("playback lookups are running");
            return;
        }
        if quiet::busy() {
            self.analysis_held("a game or a fullscreen app is in front");
            return;
        }
        // The previews «Моя волна» asked for are of songs it is about to
        // play: before the library's backlog (still within their own day's
        // limit), so they are heard before the wave picks again.
        if !self.streaming.analysis.wanted.is_empty() && self.preview_step() {
            self.streaming.analysis.held = None;
            return;
        }
        let daily = self.streaming.schedule.analysis_daily;
        if self.analysis_downloads() >= daily {
            self.analysis_held("today's downloads are used up");
            self.preview_step();
            return;
        }
        self.streaming.analysis.held = None;
        let retry_before = now_ms() - self.streaming.schedule.miss_ttl.as_millis() as i64;
        // The tracks set aside stay at the head of the queue: read past them,
        // or they fill every batch and the rest is never reached.
        let mut limit = BATCH;
        loop {
            // The analyses first, then the sketches of streams analysed
            // before sketches were (the same downloads, gates and limits).
            let queue = self.lib.streams_needing_analysis(ANALYZED_WITH, retry_before, limit).and_then(|mut t| {
                if t.len() < limit as usize {
                    t.extend(self.lib.streams_needing_sketch(ANALYZED_WITH, SKETCHED_WITH, retry_before, limit - t.len() as u32)?);
                }
                Ok(t)
            });
            let todo = match queue {
                Ok(t) => t,
                Err(e) => {
                    log::warn!("stream analysis: reading the queue: {e}");
                    return;
                }
            };
            let read = todo.len();
            for t in todo {
                let a = &self.streaming.analysis;
                if a.skip.contains(&t.track_id) || a.played.contains(&t.track_id) {
                    continue;
                }
                match t.youtube {
                    Some(video) => {
                        self.analysis_fetch(t.track_id, video);
                        return;
                    }
                    None if self.can_resolve(t.track_id) => {
                        log::info!("stream analysis: looking up the song of {}", t.track_id);
                        self.streaming.analysis.busy = Some((t.track_id, Step::Resolving));
                        self.begin_resolve(t.track_id);
                        return;
                    }
                    // Looked up lately without a hit (its miss is marked when
                    // the lookup answers), or YouTube cannot be asked.
                    None => {
                        self.streaming.analysis.skip.insert(t.track_id);
                    }
                }
            }
            if read < limit as usize {
                break;
            }
            // Every track read was set aside: the next read goes past them
            // (the limit grows each time, so this ends with the queue).
            let a = &self.streaming.analysis;
            limit = BATCH + (a.skip.len() + a.played.len()) as u32;
        }
        if self.preview_step() {
            return;
        }
        let idle = self.streaming.schedule.analysis_idle;
        let a = &mut self.streaming.analysis;
        log::debug!(
            "stream analysis: nothing to take now ({} set aside in this run), again in {} s",
            a.skip.len(),
            idle.as_secs()
        );
        // Looked at afresh after the pause: a lookup miss expires, YouTube
        // may answer again.
        a.skip.clear();
        a.idle_until = Some(now + idle);
    }

    /// The background queue waits for `why`; logged when the reason changes.
    fn analysis_held(&mut self, why: &'static str) {
        let a = &mut self.streaming.analysis;
        if a.held != Some(why) {
            log::debug!("stream analysis: waiting: {why}");
            a.held = Some(why);
        }
    }

    /// The library's queue is done for now: the next new song's preview,
    /// if today has room for one.
    fn preview_step(&mut self) -> bool {
        if self.previews_today() >= PREVIEW_DAILY {
            return false;
        }
        let tried = self.streaming.analysis.previewed.clone();
        // A song a free preview may still hear waits for it: none of
        // YouTube Music's day is spent on it before it was looked for.
        let free_open = self.free_open();
        let index = self.analysis.index().clone();
        let a = &mut self.streaming.analysis;
        let (wanted, free_tried) = (&mut a.wanted, &a.free_tried);
        wanted.retain(|(t, v)| !tried.contains(t) && !index.read().contains(*t) && (v.is_some() || free_open && !free_tried.contains(t)));
        let at = wanted.iter().position(|(t, v)| v.is_some() && (!free_open || free_tried.contains(t)));
        let wanted = at.and_then(|k| wanted.remove(k)).and_then(|(t, v)| Some((t, v?)));
        let only = free_open.then(|| self.streaming.analysis.free_tried.clone());
        let Some((track, video)) = wanted.or_else(|| self.discovery_preview_next(&tried, true, only.as_ref()).map(|(t, s)| (t, s.id.id))) else { return false };
        let Some(youtube) = self.service(Service::Youtube) else { return false };
        log::info!("stream analysis: a preview of new music {track} from YouTube {video}");
        self.streaming.analysis.previewed.insert(track);
        let job = Job {
            track,
            video,
            origin: Origin::Preview,
            youtube: Some(youtube),
            bytes: None,
            listening: self.streaming.held.listening.clone(),
            model: None,
        };
        if !self.analysis_job(job) {
            return false;
        }
        self.count_preview();
        self.streaming.analysis.busy = Some((track, Step::Analysing));
        true
    }

    /// Free previews looked for today.
    fn free_today(&mut self) -> u32 {
        let today = now_ms() / DAY_MS;
        if let Some((day, n)) = self.streaming.analysis.free_day
            && day == today
        {
            return n;
        }
        let n = self.day_count(FREE_DAY_KEY);
        self.streaming.analysis.free_day = Some((today, n));
        n
    }

    fn count_free(&mut self) {
        let n = self.free_today() + 1;
        self.streaming.analysis.free_day = Some((now_ms() / DAY_MS, n));
        self.day_count_put(FREE_DAY_KEY, n);
    }

    /// The next new song to hear by a free preview, if one is worth it
    /// and everything allows it: Deezer's 30 seconds of the same song (by
    /// its Deezer id, else by its artist and title) spend nothing of
    /// YouTube Music's day of previews and never ask its player. The songs
    /// «Моя волна» is about to play first, then the discoveries by what a
    /// preview of them is worth ([`nikgob_recs::previews`]); each song is
    /// looked for once a run. Playback goes first: not while the app is
    /// idle, buffers, looks a song up or a game is in front.
    fn free_preview_step(&mut self) -> bool {
        if self.streams_idle() || self.status == v::Status::Buffering || !self.streaming.resolving.is_empty() || quiet::busy() {
            return false;
        }
        if self.free_today() >= FREE_DAILY || self.streaming.analysis.free_idle_until.is_some_and(|t| Instant::now() < t) {
            return false;
        }
        let Some(meta) = self.graph_metadata() else { return false };
        if self.streaming.analysis.free_tried.len() >= FREE_TRIED_MAX {
            self.streaming.analysis.free_tried.clear();
        }
        let a = &self.streaming.analysis;
        let mut skip: HashSet<TrackId> = a.free_tried.clone();
        skip.extend(a.previewed.iter().copied());
        let index = self.analysis.index().clone();
        let wanted = a.wanted.iter().map(|(t, _)| *t).find(|t| !skip.contains(t) && !index.read().contains(*t));
        // A song asked for that a free preview missed comes before every
        // discovery: its YouTube Music preview next, when that can run now.
        let urgent = a.wanted.iter().any(|(t, v)| v.is_some() && !a.previewed.contains(t) && !index.read().contains(*t));
        if wanted.is_none() && urgent && self.youtube_preview_ready() {
            return false;
        }
        let Some(track) = wanted.or_else(|| self.discovery_preview_next(&skip, false, None).map(|(t, _)| t)) else {
            let idle = self.streaming.schedule.analysis_idle;
            self.streaming.analysis.free_idle_until = Some(Instant::now() + idle);
            return false;
        };
        let Some(row) = self.lib.get_track_row(track).ok().flatten() else {
            self.streaming.analysis.free_tried.insert(track);
            return false;
        };
        let duration = self.lib.get_track(track).ok().flatten().and_then(|t| t.duration_ms).map(|ms| Duration::from_millis(u64::from(ms)));
        let refs = self.lib.stream_refs(track).unwrap_or_default();
        let deezer = refs.iter().find(|r| r.id.service == Service::Deezer).and_then(|r| r.id.id.parse::<u64>().ok());
        // Its ISRC, when it is known, finds the very song on Deezer (its
        // public lookup, no login).
        let isrc = self.lib.get_track(track).ok().flatten().and_then(|t| t.isrc).or_else(|| refs.iter().find_map(|r| r.isrc.clone()));
        let (artist, title) = (row.artist.clone(), row.title.clone());
        self.streaming.analysis.free_tried.insert(track);
        self.streaming.analysis.busy = Some((track, Step::Free));
        self.count_free();
        log::info!("stream analysis: looking for a free preview of new music {track}");
        self.streaming.spawn(async move {
            let by_isrc = match (deezer, isrc.as_deref()) {
                (None, Some(code)) => meta.deezer.preview_by_isrc(code).await.ok().flatten(),
                _ => None,
            };
            let url = match by_isrc {
                Some(url) => Ok(Some(url)),
                None => meta.deezer.preview_url(deezer, &artist, &title, duration).await,
            };
            let found = match url {
                Ok(Some(url)) => match meta.deezer.download_preview(&url).await {
                    Ok(bytes) if !bytes.is_empty() => Some((bytes, ext_of(&url))),
                    _ => None,
                },
                _ => None,
            };
            Msg::FreePreview { track, found }
        });
        true
    }

    /// The free preview of `track` was looked for: found, it is analysed
    /// as a preview (none of YouTube's day spent); else the song waits for
    /// one of YouTube Music's previews if it is worth one.
    pub(crate) fn free_preview_found(&mut self, track: TrackId, found: Option<(Vec<u8>, String)>) {
        if self.streaming.analysis.busy != Some((track, Step::Free)) {
            return;
        }
        self.streaming.analysis.busy = None;
        let Some(bytes) = found else {
            log::debug!("stream analysis: no free preview of {track}");
            // Kept only while YouTube Music's preview can still follow.
            self.wanted_prune();
            self.analysis_wait(FREE_MISS_GAP);
            return;
        };
        self.streaming.analysis.wanted.retain(|(t, _)| *t != track);
        if !self.stream_analysis_on() {
            return;
        }
        log::info!("stream analysis: a free preview of new music {track} from Deezer");
        let job = Job {
            track,
            video: format!("deezer-preview:{track}"),
            origin: Origin::Preview,
            youtube: None,
            bytes: Some(bytes),
            listening: self.streaming.held.listening.clone(),
            model: None,
        };
        if self.analysis_job(job) {
            self.streaming.analysis.busy = Some((track, Step::Analysing));
        }
    }

    /// Downloads and analyses `video` for `track` (one of the day's).
    fn analysis_fetch(&mut self, track: TrackId, video: String) {
        let Some(youtube) = self.service(Service::Youtube) else { return };
        log::info!("stream analysis: {track} from YouTube {video}");
        let job = Job {
            track,
            video,
            origin: Origin::Background,
            youtube: Some(youtube),
            bytes: None,
            listening: self.streaming.held.listening.clone(),
            model: None,
        };
        if self.analysis_job(job) {
            self.count_download();
            self.streaming.analysis.busy = Some((track, Step::Analysing));
        }
    }

    /// A YouTube lookup answered ([`Self::resolved`]); one the background
    /// analysis asked for goes on: the song is analysed, a miss is marked
    /// (the lookup keeps its own 5-day miss), a rate limit waits.
    pub(crate) fn analysis_resolved(&mut self, track: TrackId, found: &Lookup) {
        if self.streaming.analysis.busy != Some((track, Step::Resolving)) {
            return;
        }
        self.streaming.analysis.busy = None;
        match found {
            Ok(Some(hit)) => {
                // YouTube Music began to play meanwhile: the song (attached
                // now) is taken again once it stops.
                if self.youtube_listened() {
                    log::info!("stream analysis: {track} waits, YouTube Music plays");
                    return;
                }
                if self.stream_analysis_on() && self.analysis_downloads() < self.streaming.schedule.analysis_daily {
                    self.analysis_fetch(track, hit.id.id.clone());
                }
            }
            Ok(None) => {
                if let Err(e) = self.lib.mark_stream_analysis_failed(track, "no song on YouTube Music") {
                    log::warn!("stream analysis: marking {track}: {e}");
                }
                self.streaming.analysis.recount = true;
                let gap = self.streaming.schedule.analysis_gap;
                self.analysis_after(gap);
            }
            Err(retry_after) => self.analysis_limited(*retry_after),
        }
    }

    /// YouTube Music asked to wait: the next track after the pause.
    fn analysis_limited(&mut self, retry_after: Option<Duration>) {
        let wait = retry_after
            .or_else(|| self.pause_left(Service::Youtube))
            .unwrap_or(self.streaming.schedule.rate_limit_unknown);
        log::info!("stream analysis: rate limited, again in {} s", wait.as_secs());
        self.analysis_wait(wait + super::schedule::wake_jitter());
    }

    /// A job of the analysis thread ended.
    pub(crate) fn stream_analysed(&mut self, d: Done) {
        let a = &mut self.streaming.analysis;
        if d.origin == Origin::Played {
            a.played.remove(&d.track);
        }
        let mine = d.origin != Origin::Played && a.busy.is_some_and(|(t, _)| t == d.track);
        if mine {
            a.busy = None;
        }
        let gap = self.streaming.schedule.analysis_gap;
        match d.outcome {
            Outcome::Stored(stored) => {
                log::info!("stream analysis: {} analysed ({:?}, stored: {stored})", d.track, d.origin);
                self.publish_analysis_counts();
                // A new song placed by its sound: the next build takes it,
                // and the radio playing looks at it again if it queued it.
                if d.origin == Origin::Preview {
                    self.recs_dirty(gap);
                    self.radio_preview_heard(d.track);
                }
                if mine {
                    self.analysis_after(gap);
                }
            }
            Outcome::NotCached
 | Outcome::Stopped => {}
            Outcome::RateLimited(retry_after) => {
                if mine {
                    self.analysis_limited(retry_after);
                }
            }
            Outcome::Transient(e) => {
                log::info!("stream analysis: {} not now: {e}", d.track);
                if mine {
                    let idle = self.streaming.schedule.analysis_idle;
                    self.analysis_after(idle);
                }
            }
            Outcome::Failed(e) => {
                log::info!("stream analysis: cannot analyse {}: {e}", d.track);
                if d.origin == Origin::Background {
                    if let Err(e) = self.lib.mark_stream_analysis_failed(d.track, &e) {
                        log::warn!("stream analysis: marking {}: {e}", d.track);
                    }
                    self.publish_analysis_counts();
                }
                if mine {
                    self.analysis_after(gap);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// More songs asked for than the queue holds, YouTube Music's day used
    /// up: each free lookup, found or missed, retires its song, so the queue
    /// never stays full of songs no preview will hear; a YouTube Music song
    /// a free preview missed waits for its preview while one can follow.
    #[test]
    fn songs_looked_for_free_leave_the_queue() {
        let songs: Vec<(TrackId, Option<String>)> = (0..WANTED_MAX + 36).map(|k| (TrackId::new(), (k % 2 == 0).then(|| format!("v{k}")))).collect();
        let mut wanted: VecDeque<(TrackId, Option<String>)> = songs.iter().cloned().collect();
        let none = |_: TrackId| false;
        // Nothing looked for yet: all stay.
        prune_wanted(&mut wanted, none, &HashSet::new(), false);
        assert_eq!(wanted.len(), songs.len());
        // Every one looked for free, YouTube Music's day over: none stay.
        let tried: HashSet<TrackId> = songs.iter().map(|s| s.0).collect();
        prune_wanted(&mut wanted, none, &tried, false);
        assert!(wanted.is_empty());
        // With a YouTube Music preview left: its songs wait, the others go.
        let mut wanted: VecDeque<(TrackId, Option<String>)> = songs.iter().cloned().collect();
        prune_wanted(&mut wanted, none, &tried, true);
        assert!(!wanted.is_empty() && wanted.iter().all(|(_, v)| v.is_some()));
        // Heard meanwhile (a free preview found): gone.
        let heard: HashSet<TrackId> = wanted.iter().map(|w| w.0).collect();
        prune_wanted(&mut wanted, |t| heard.contains(&t), &HashSet::new(), true);
        assert!(wanted.is_empty());
    }
}
