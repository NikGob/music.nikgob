//! Sound analysis of stream-only tracks (docs/stage4.md §2): the liked and
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
//!   YouTube Music is not paused, the periodic network work is not idle and
//!   nothing waits for its sound (Buffering). A rate limit is not a failure:
//!   the job waits for the pause to end (docs/stage3.md §9). A real failure
//!   (no song, a video that does not play or decode) is marked and tried
//!   again after [`StreamSchedule::miss_ttl`].
//! - **Previews of new music.** When the library's queue is empty, the
//!   discoveries found for «Новое для тебя» get a preview analysed: the
//!   first [`PREVIEW_BYTES`] of the song (about two minutes), at most
//!   [`PREVIEW_DAILY`] a day apart from the library's downloads, so new
//!   music is placed by its sound (recs-v2 §11.3).
//!
//! Both run on one thread of their own at the lowest priority, like the file
//! job; the worker only hands it jobs and hears back [`Done`]. With
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
use nikgob_recs::ANALYZED_WITH;
use nikgob_view as v;

use super::Msg;
use super::resolve::Lookup;
use crate::Worker;
use crate::analysis::{FileError, SoundIndex, analyze_bytes, lower_priority, store_vector};
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
/// Bytes of a new song's preview (about two minutes of YouTube's Opus).
const PREVIEW_BYTES: usize = 3 << 20;
/// Previews of new music analysed a day.
const PREVIEW_DAILY: u32 = 30;
/// Library cache key of the previews analysed today: `day:count`.
const PREVIEW_DAY_KEY: &str = "stream_analysis.preview_day";

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
    youtube: Arc<dyn StreamService>,
}

/// What the background job is waiting for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// The track's song on YouTube Music ([`Msg::Resolved`]).
    Resolving,
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
    /// New songs whose preview was tried in this run.
    previewed: HashSet<TrackId>,
    /// The progress is counted again on the next tick.
    recount: bool,
}

impl StreamAnalysis {
    /// Stops the job in flight; nothing more is analysed.
    pub(crate) fn shutdown(&mut self) {
        self.off.store(true, Ordering::Relaxed);
        self.jobs = None;
    }
}

/// The analysis thread: one job at a time, until the worker goes.
fn run(jobs: Receiver<Job>, lib: Library, index: SoundIndex, tx: Sender<Internal>, off: Arc<AtomicBool>) {
    lower_priority();
    for job in jobs {
        let started = Instant::now();
        let outcome = work(&job, &lib, &index, &off);
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
    }
}

fn work(job: &Job, lib: &Library, index: &SoundIndex, off: &AtomicBool) -> Outcome {
    if off.load(Ordering::Relaxed) {
        return Outcome::Stopped;
    }
    let audio = match (job.youtube.cached_audio(&job.video), job.origin) {
        (Some(a), _) => a,
        (None, Origin::Played) => return Outcome::NotCached,
        (None, Origin::Background | Origin::Preview) => match job.youtube.open_audio(&job.video) {
            Ok(a) => a,
            Err(e) => return stream_outcome(e),
        },
    };
    let ext = audio.format_hint();
    let max = if job.origin == Origin::Preview { PREVIEW_BYTES } else { MAX_BYTES };
    let bytes = match read_all(audio, max, off) {
        Ok(b) => b,
        Err(o) => return o,
    };
    match analyze_bytes(bytes, ext, &format!("youtube:{}", job.video), off) {
        Ok(vector) => match store_vector(lib, index, job.track, vector, AnalysisSource::Youtube) {
            Ok(stored) => Outcome::Stored(stored),
            Err(e) => Outcome::Transient(format!("storing: {e}")),
        },
        Err(FileError::Stopped) => Outcome::Stopped,
        Err(FileError::Unavailable(e)) => Outcome::Transient(e),
        Err(FileError::Undecodable(e)) => Outcome::Failed(e),
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
        | StreamError::Unsupported
        | StreamError::PremiumRequired
        | StreamError::Parse(_) => Outcome::Failed(e.to_string()),
    }
}

impl Worker {
    fn stream_analysis_on(&self) -> bool {
        self.state.read().prefs.stream_analysis
    }

    /// Hands a job to the analysis thread (started on the first one).
    fn analysis_job(&mut self, job: Job) -> bool {
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
    /// stream's).
    fn analysed(&self, track: TrackId) -> bool {
        self.lib
            .get_analysis(track)
            .ok()
            .flatten()
            .is_some_and(|a| a.analyzed_with == ANALYZED_WITH)
    }

    /// The playing entry of `track` ended (or was skipped) while it played
    /// from `source`: a YouTube video whose whole audio is in the disk
    /// cache is analysed from there (no request).
    pub(crate) fn analyse_played(&mut self, track: TrackId, source: SourceId) {
        if !self.stream_analysis_on() || self.streaming.analysis.played.contains(&track) {
            return;
        }
        let Some(src) = self.lib.get_source(source).ok().flatten() else { return };
        if src.kind != SourceKind::Youtube || self.analysed(track) {
            return;
        }
        let Some(youtube) = self.service(Service::Youtube) else { return };
        let job = Job {
            track,
            video: src.locator,
            origin: Origin::Played,
            youtube,
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
        if self.service(Service::Youtube).is_none() || !self.has_audio(Service::Youtube) {
            return;
        }
        if let Some(left) = self.pause_left(Service::Youtube) {
            self.analysis_wait(left + super::schedule::wake_jitter());
            return;
        }
        // Playback and its lookups go first.
        if self.streams_idle() || self.status == v::Status::Buffering || !self.streaming.resolving.is_empty() {
            return;
        }
        let daily = self.streaming.schedule.analysis_daily;
        if self.analysis_downloads() >= daily {
            self.preview_step();
            return;
        }
        let retry_before = now_ms() - self.streaming.schedule.miss_ttl.as_millis() as i64;
        let todo = match self.lib.streams_needing_analysis(ANALYZED_WITH, retry_before, BATCH) {
            Ok(t) => t,
            Err(e) => {
                log::warn!("stream analysis: reading the queue: {e}");
                return;
            }
        };
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
        if self.preview_step() {
            return;
        }
        let idle = self.streaming.schedule.analysis_idle;
        self.streaming.analysis.idle_until = Some(now + idle);
    }

    /// The library's queue is done for now: the next new song's preview,
    /// if today has room for one.
    fn preview_step(&mut self) -> bool {
        if self.previews_today() >= PREVIEW_DAILY {
            return false;
        }
        let tried = self.streaming.analysis.previewed.clone();
        let Some((track, video)) = self.discovery_preview_next(&tried) else { return false };
        let Some(youtube) = self.service(Service::Youtube) else { return false };
        log::info!("stream analysis: a preview of new music {track} from YouTube {video}");
        self.streaming.analysis.previewed.insert(track);
        let job = Job {
            track,
            video,
            origin: Origin::Preview,
            youtube,
        };
        if !self.analysis_job(job) {
            return false;
        }
        self.count_preview();
        self.streaming.analysis.busy = Some((track, Step::Analysing));
        true
    }

    /// Downloads and analyses `video` for `track` (one of the day's).
    fn analysis_fetch(&mut self, track: TrackId, video: String) {
        let Some(youtube) = self.service(Service::Youtube) else { return };
        log::info!("stream analysis: {track} from YouTube {video}");
        let job = Job {
            track,
            video,
            origin: Origin::Background,
            youtube,
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
                // A new song placed by its sound: the next build takes it.
                if d.origin == Origin::Preview {
                    self.recs_dirty(gap);
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
