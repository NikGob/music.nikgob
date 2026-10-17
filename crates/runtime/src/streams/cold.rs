//! Cold start (docs/stage3.md §0 "Холодный старт", §3.6): the history, likes
//! and playlists of a service brought in from its API
//! ([`Command::ImportFromService`](nikgob_view::Command::ImportFromService))
//! or from its export files (`ImportExportFiles`, formats in
//! [`super::export`]). Each import is a job in "Загрузки" with a progress
//! line and ends with a summary toast.
//!
//! * History → events only (`start`, then `complete` or `skip` where the
//!   source tells), on stream tracks that stay outside the library ("seen").
//!   Never likes, never the wishlist.
//! * Likes → library likes of stream tracks plus wishlist items that wait
//!   ("ждёт", origin `import`) for the user's "скачать". From the API they go
//!   through the like pull ([`Worker::apply_remote_likes`]), so the first
//!   pull and the import never both take them. A like the user took back
//!   is not liked again, and a dismissed wishlist item never comes back.
//! * Playlists → local playlists. From the API they are linked to the
//!   service (`playlist_remote`, mode [`Prefs::playlist_sync_default`]); a
//!   playlist from a file is a snapshot ("разово"), linked when the file
//!   names its id, otherwise remembered by an import key.
//!
//! Running an import again adds nothing twice: events are deduplicated by
//! `(ts, track, type)`, tracks are found by their ids in the services,
//! playlists by their link or import key.
//!
//! Parsing and the history (the bulk: tens of thousands of plays) run on a
//! blocking task that writes the library itself; likes and playlists need
//! the wishlist and the view, so the worker takes them in small steps, one
//! message per step, to stay responsive. Tracks an export file made without
//! their length are completed later from the service, a few at a time
//! ([`Worker::enrich_tick`]): Spotify's quota is shared by every user of
//! the public app.
//!
//! [`Prefs::playlist_sync_default`]: nikgob_view::Prefs::playlist_sync_default

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use nikgob_core::model::{
    DeviceId, Event, EventContext, EventType, LikeTarget, PlaylistId, TrackId, UnixMs,
    WishlistOrigin,
};
use nikgob_core::streaming::{
    AlbumRef, ArtistRef, EntityKind, Played, PlaylistSyncMode, RemoteId, Service, StreamError,
    StreamPlaylist, StreamService, StreamTrack,
};
use nikgob_library::{Library, PlaylistRemote, event_id, sort_key};
use nikgob_view as v;

use super::export::{self, ExportData, Item, Outcome, Play};
use super::{Msg, error_text};
use crate::Worker;
use crate::import::{IMPORT_ID, Internal};

/// Bit of the ids of cold-start jobs (inside the import id space, so the
/// torrent list leaves them alone).
pub(crate) const STREAM_JOB: u64 = 1 << 46;
/// Device of imported events.
pub(crate) const IMPORT_DEVICE: &str = "import";
/// Likes handled per worker step.
const LIKES_PER_STEP: usize = 200;
/// Plays written per transaction.
const HISTORY_BATCH: usize = 1_000;
/// Video ids looked up on YouTube Music for an export without titles, one
/// per [`YT_LOOKUP_EVERY`] (stopping at a rate limit).
const YT_LOOKUPS: usize = 3_000;
const YT_LOOKUP_EVERY: Duration = Duration::from_secs(1);
/// Tracks completed from a service per batch (one request for Spotify:
/// `tracks(ids)`; YouTube asks one by one), and the pause between batches
/// (Spotify's quota is shared).
const ENRICH_BATCH_SPOTIFY: u32 = 50;
const ENRICH_BATCH_YOUTUBE: u32 = 10;
/// Nothing to complete: look again after this.
const ENRICH_IDLE: Duration = Duration::from_secs(10 * 60);
/// A play the app recorded itself within this of a play the API reports
/// (plus the track's length) is the same play.
const SAME_PLAY_MS: i64 = 120_000;
/// Finished jobs kept in the view.
const HISTORY_KEEP: usize = 20;

/// Background results of the cold-start imports.
pub(crate) enum ColdMsg {
    Progress {
        job: u64,
        subtitle: String,
        progress: Option<f32>,
    },
    /// An export is read and its history stored.
    Parsed {
        job: u64,
        result: Result<Box<Parsed>, String>,
    },
    /// What the API gave.
    Fetched {
        job: u64,
        service: Service,
        fetched: Box<Fetched>,
    },
    /// Do the next step of a job.
    Step(u64),
    Enriched {
        service: Service,
        results: Vec<(TrackId, String, Result<StreamTrack, StreamError>)>,
    },
}

/// A track an import wants in the library.
#[derive(Clone, Debug)]
pub(crate) enum Wanted {
    Stream(Box<StreamTrack>),
    /// No id in the file: only a known track of that name will do.
    Named { artist: String, title: String },
}

/// A playlist waiting to be created or synced.
pub(crate) struct PendingPlaylist {
    service: Service,
    name: String,
    remote: Option<RemoteId>,
    tracks: Vec<Wanted>,
    /// The playlist as the API sent it.
    fetched: Option<StreamPlaylist>,
}

pub(crate) struct Parsed {
    plays: usize,
    new_plays: usize,
    likes: Vec<Wanted>,
    playlists: Vec<PendingPlaylist>,
    files: usize,
    problems: Vec<String>,
}

pub(crate) struct Fetched {
    liked: Result<Vec<StreamTrack>, StreamError>,
    playlists: Vec<StreamPlaylist>,
    /// Playlists that could not be fetched.
    missed: usize,
    recent: Result<Vec<Played>, StreamError>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    pub(crate) plays: usize,
    pub(crate) new_plays: usize,
    pub(crate) likes: usize,
    pub(crate) new_likes: usize,
    pub(crate) playlists: usize,
    pub(crate) new_playlists: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Origin {
    Service(Service),
    Files(PathBuf),
}

struct ColdJob {
    origin: Origin,
    title: String,
    subtitle: String,
    progress: Option<f32>,
    counts: Counts,
    likes: VecDeque<Wanted>,
    playlists: VecDeque<PendingPlaylist>,
    /// Steps in total, for the progress bar of the worker part.
    steps: usize,
    problems: Vec<String>,
}

#[derive(Default)]
struct Enrich {
    busy: bool,
    next_at: Option<Instant>,
    tried: HashSet<TrackId>,
}

#[derive(Default)]
pub(crate) struct ColdState {
    jobs: BTreeMap<u64, ColdJob>,
    history: VecDeque<v::Job>,
    next: u64,
    enrich: HashMap<Service, Enrich>,
}

/// `1 240`: thousands grouped with a no-break space.
pub(crate) fn group(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push('\u{a0}');
        }
        out.push(c);
    }
    out
}

/// Russian plural: 1 лайк, 2 лайка, 5 лайков.
pub(crate) fn plural(n: usize, one: &'static str, few: &'static str, many: &'static str) -> &'static str {
    let (d, h) = (n % 10, n % 100);
    if d == 1 && h != 11 {
        one
    } else if (2..=4).contains(&d) && !(12..=14).contains(&h) {
        few
    } else {
        many
    }
}

/// "3 прослушивания, 2 плейлиста": the counts that are not zero.
fn count_line(plays: usize, likes: usize, playlists: usize) -> String {
    let parts: Vec<String> = [
        (plays, ["прослушивание", "прослушивания", "прослушиваний"]),
        (likes, ["лайк", "лайка", "лайков"]),
        (playlists, ["плейлист", "плейлиста", "плейлистов"]),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, [one, few, many])| format!("{} {}", group(n), plural(n, one, few, many)))
    .collect();
    if parts.is_empty() { "ничего".into() } else { parts.join(", ") }
}

/// The final toast: what is new ("Импорт: 1 240 прослушиваний, 312 лайков,
/// 9 плейлистов"), or that nothing was.
pub(crate) fn summary(c: &Counts) -> String {
    if c.new_plays + c.new_likes + c.new_playlists == 0 {
        if c.plays + c.likes + c.playlists == 0 {
            return "Импорт: ничего не нашлось".into();
        }
        return "Импорт: ничего нового — всё уже было".into();
    }
    let mut text = format!("Импорт: {}", count_line(c.new_plays, c.new_likes, c.new_playlists));
    // The likes came with the login already (its first pull).
    if c.new_likes == 0 && c.likes > 0 {
        text.push_str(&format!(" · лайки уже здесь ({})", group(c.likes)));
    }
    text
}

/// A stream track from what an export file says.
fn stream_of(i: &Item, id: &str) -> StreamTrack {
    let title = if i.title.is_empty() { id } else { &i.title };
    let mut t = StreamTrack::new(RemoteId::new(i.service, id), title);
    if !i.artist.is_empty() {
        t.artists = vec![ArtistRef::new(None, i.artist.clone())];
    }
    t.album = i.album.as_ref().map(|a| AlbumRef::new(None, a.clone()));
    t
}

fn wanted(i: &Item, full: &HashMap<String, StreamTrack>) -> Wanted {
    match &i.id {
        Some(id) => Wanted::Stream(Box::new(full.get(id).cloned().unwrap_or_else(|| stream_of(i, id)))),
        None => Wanted::Named {
            artist: i.artist.clone(),
            title: i.title.clone(),
        },
    }
}

/// Looks up YouTube video ids an export lists without a title: one a
/// second, stopping at a rate limit (the rest keep the id as their title
/// and are completed later, a few at a time).
async fn youtube_lookups(svc: Arc<dyn StreamService>, ids: Vec<String>) -> HashMap<String, StreamTrack> {
    let mut out = HashMap::new();
    let total = ids.len();
    for (i, id) in ids.into_iter().enumerate() {
        if i > 0 {
            tokio::time::sleep(YT_LOOKUP_EVERY).await;
        }
        match svc.track(&id).await {
            Ok(t) => {
                out.insert(id, t);
            }
            Err(e @ StreamError::RateLimited { .. }) => {
                log::warn!("youtube: export look-ups stopped after {i} of {total}: {e}");
                break;
            }
            Err(_) => {}
        }
    }
    out
}

/// Stores the plays of an export as events; returns how many were new.
fn store_history(
    lib: &Library,
    plays: &[Play],
    full: &HashMap<String, StreamTrack>,
    progress: &dyn Fn(usize),
) -> Result<usize, nikgob_library::LibraryError> {
    let device = DeviceId::from(IMPORT_DEVICE);
    let mut named: HashMap<(Service, String, String), TrackId> = HashMap::new();
    let mut new = 0;
    for (n, chunk) in plays.chunks(HISTORY_BATCH).enumerate() {
        let mut streams: Vec<StreamTrack> = Vec::new();
        let mut index: HashMap<RemoteId, usize> = HashMap::new();
        for p in chunk {
            if let Some(id) = &p.item.id {
                let remote = RemoteId::new(p.item.service, id.clone());
                if let std::collections::hash_map::Entry::Vacant(e) = index.entry(remote) {
                    e.insert(streams.len());
                    streams.push(full.get(id).cloned().unwrap_or_else(|| stream_of(&p.item, id)));
                }
            }
        }
        let ids = lib.ensure_stream_tracks(&streams)?;
        let mut events = Vec::with_capacity(chunk.len() * 2);
        for p in chunk {
            let track = match &p.item.id {
                Some(id) => ids[index[&RemoteId::new(p.item.service, id.clone())]],
                None => {
                    let key = (p.item.service, sort_key(&p.item.artist), sort_key(&p.item.title));
                    match named.get(&key) {
                        Some(t) => *t,
                        None => {
                            let album = p.item.album.as_deref();
                            let t = lib.seen_track_by_name(p.item.service, &p.item.artist, &p.item.title, album)?;
                            named.insert(key, t);
                            t
                        }
                    }
                }
            };
            let kind = Some(p.item.service.source_kind());
            let position = p.ms_played.map(|ms| u32::try_from(ms).unwrap_or(u32::MAX));
            let mut push = |ts: UnixMs, t: EventType| {
                events.push(Event {
                    id: event_id(ts, track, t),
                    ts,
                    track_id: track,
                    source_kind: kind,
                    event_type: t,
                    position_ms: if t == EventType::Start { Some(0) } else { position },
                    context: EventContext::Other,
                    device: device.clone(),
                });
            };
            push(p.start_ms, EventType::Start);
            let end = p.start_ms + p.ms_played.unwrap_or(0) as i64;
            match p.outcome {
                Outcome::Complete => push(end, EventType::Complete),
                Outcome::Skip => push(end, EventType::Skip),
                Outcome::Unknown => {}
            }
        }
        // A play is new when its start is.
        let (starts, ends): (Vec<Event>, Vec<Event>) =
            events.into_iter().partition(|e| e.event_type == EventType::Start);
        new += lib.import_events(&starts)?;
        lib.import_events(&ends)?;
        progress((n + 1) * HISTORY_BATCH);
    }
    Ok(new)
}

fn send(tx: &Sender<Internal>, m: ColdMsg) {
    let _ = tx.send(Internal::Stream(Msg::Cold(m)));
}

/// The whole background part of an export import.
async fn read_export(
    job: u64,
    path: PathBuf,
    lib: Library,
    youtube: Option<Arc<dyn StreamService>>,
    tx: Sender<Internal>,
) -> Result<Box<Parsed>, String> {
    let data: ExportData = tokio::task::spawn_blocking(move || export::read_export(&path))
        .await
        .map_err(|e| e.to_string())??;
    if data.is_empty() {
        return Err(if data.problems.is_empty() {
            "Не нашёл файлов выгрузки Spotify или Google Takeout".into()
        } else {
            format!("Не получилось прочитать: {}", data.problems.join("; "))
        });
    }
    // YouTube ids without a title (playlist CSVs): ask YouTube Music.
    let mut full = HashMap::new();
    if let Some(yt) = youtube {
        let mut seen = HashSet::new();
        let blank: Vec<String> = data
            .likes
            .iter()
            .chain(data.playlists.iter().flat_map(|p| &p.items))
            .chain(data.plays.iter().map(|p| &p.item))
            .filter(|i| i.service == Service::Youtube && i.title.is_empty())
            .filter_map(|i| i.id.clone())
            .filter(|id| seen.insert(id.clone()))
            // Known already (an earlier import, a like pull): nothing to ask.
            .filter(|id| {
                lib.track_by_remote(&RemoteId::new(Service::Youtube, id.clone()))
                    .ok()
                    .flatten()
                    .is_none()
            })
            .take(YT_LOOKUPS)
            .collect();
        if !blank.is_empty() {
            send(&tx, ColdMsg::Progress {
                job,
                subtitle: format!("Узнаю названия в YouTube Music: {}", group(blank.len())),
                progress: None,
            });
            full = youtube_lookups(yt, blank).await;
        }
    }
    let plays = data.plays.len();
    let likes: Vec<Wanted> = data.likes.iter().map(|i| wanted(i, &full)).collect();
    let playlists: Vec<PendingPlaylist> = data
        .playlists
        .iter()
        .map(|p| PendingPlaylist {
            service: p.service,
            name: p.name.clone(),
            remote: p.remote.clone().map(|id| RemoteId::new(p.service, id)),
            tracks: p.items.iter().map(|i| wanted(i, &full)).collect(),
            fetched: None,
        })
        .collect();
    let files = data.files.len();
    let problems = data.problems.clone();
    let progress_tx = tx.clone();
    let new_plays = tokio::task::spawn_blocking(move || {
        let progress = |done: usize| {
            let done = done.min(plays);
            send(&progress_tx, ColdMsg::Progress {
                job,
                subtitle: format!("История: {} из {}", group(done), group(plays)),
                progress: Some(done as f32 / plays.max(1) as f32),
            });
        };
        store_history(&lib, &data.plays, &full, &progress).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(Box::new(Parsed {
        plays,
        new_plays,
        likes,
        playlists,
        files,
        problems,
    }))
}

/// The API part: likes, the user's playlists with their tracks, recent plays.
async fn fetch_service(job: u64, svc: Arc<dyn StreamService>, tx: Sender<Internal>) -> Fetched {
    let progress = |subtitle: String, progress: Option<f32>| {
        send(&tx, ColdMsg::Progress { job, subtitle, progress });
    };
    progress("Лайки…".into(), None);
    let liked = svc.liked().await;
    progress("Плейлисты…".into(), None);
    let mut mine = svc.my_playlists().await.unwrap_or_else(|e| {
        log::warn!("{}: my_playlists: {e}", svc.service());
        Vec::new()
    });
    // YouTube Music lists the liked songs as a playlist too: they are likes.
    mine.retain(|p| !(p.id.service == Service::Youtube && p.id.id == "LM"));
    let mut playlists = Vec::new();
    let mut missed = 0;
    for (i, p) in mine.iter().enumerate() {
        progress(
            format!("Плейлисты: {} из {}", i + 1, mine.len()),
            Some(i as f32 / mine.len().max(1) as f32),
        );
        let mut r = svc.playlist(&p.id.id).await;
        if let Err(StreamError::RateLimited { retry_after }) = &r {
            tokio::time::sleep(retry_after.unwrap_or(Duration::from_secs(5)).min(Duration::from_secs(30))).await;
            r = svc.playlist(&p.id.id).await;
        }
        match r {
            Ok(full) => playlists.push(full),
            Err(e) => {
                log::warn!("{}: playlist {}: {e}", svc.service(), p.id);
                missed += 1;
            }
        }
    }
    progress("Недавние прослушивания…".into(), None);
    let recent = svc.recent().await;
    Fetched {
        liked,
        playlists,
        missed,
        recent,
    }
}

impl Worker {
    fn cold_job(&mut self, origin: Origin, title: String) -> Option<u64> {
        if self.streaming.cold.jobs.values().any(|j| j.origin == origin) {
            self.toast("Этот импорт уже идёт · смотри «Загрузки»", None);
            return None;
        }
        self.streaming.cold.next += 1;
        let id = IMPORT_ID | STREAM_JOB | self.streaming.cold.next;
        self.streaming.cold.jobs.insert(
            id,
            ColdJob {
                origin,
                title,
                subtitle: "Подготовка…".into(),
                progress: None,
                counts: Counts::default(),
                likes: VecDeque::new(),
                playlists: VecDeque::new(),
                steps: 0,
                problems: Vec::new(),
            },
        );
        self.publish_cold_jobs();
        self.toast("Импорт начался · смотри «Загрузки»", None);
        Some(id)
    }

    /// `Command::ImportFromService`.
    pub(crate) fn import_from_service(&mut self, service: Service) {
        let Some(svc) = self.service(service) else {
            self.toast(self.off_text(service), None);
            return;
        };
        if !svc.account().is_logged_in() {
            self.toast(format!("Сначала войди в {}", service.display_name()), None);
            return;
        }
        let Some(job) = self.cold_job(Origin::Service(service), format!("Импорт из {}", service.display_name()))
        else {
            return;
        };
        let tx = self.streaming.internal.clone();
        self.streaming.spawn(async move {
            let fetched = Box::new(fetch_service(job, svc, tx).await);
            Msg::Cold(ColdMsg::Fetched { job, service, fetched })
        });
    }

    /// `Command::ImportExportFiles`.
    pub(crate) fn import_export_files(&mut self, path: PathBuf) {
        let name = path
            .file_name()
            .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        let Some(job) = self.cold_job(Origin::Files(path.clone()), format!("Импорт выгрузки «{name}»")) else {
            return;
        };
        let lib = self.lib.clone();
        let youtube = self.service(Service::Youtube);
        let tx = self.streaming.internal.clone();
        self.streaming.spawn(async move {
            let result = read_export(job, path, lib, youtube, tx).await;
            Msg::Cold(ColdMsg::Parsed { job, result })
        });
    }

    pub(crate) fn cold_msg(&mut self, m: ColdMsg) {
        match m {
            ColdMsg::Progress { job, subtitle, progress } => {
                if let Some(j) = self.streaming.cold.jobs.get_mut(&job) {
                    j.subtitle = subtitle;
                    j.progress = progress;
                    self.publish_cold_jobs();
                }
            }
            ColdMsg::Parsed { job, result } => self.export_parsed(job, result),
            ColdMsg::Fetched { job, service, fetched } => self.service_fetched(job, service, *fetched),
            ColdMsg::Step(job) => self.cold_step(job),
            ColdMsg::Enriched { service, results } => self.enriched(service, results),
        }
    }

    fn export_parsed(&mut self, job: u64, result: Result<Box<Parsed>, String>) {
        let Some(j) = self.streaming.cold.jobs.get_mut(&job) else { return };
        match result {
            Ok(p) => {
                log::info!(
                    "export: {} files, {} plays ({} new), {} likes, {} playlists",
                    p.files,
                    p.plays,
                    p.new_plays,
                    p.likes.len(),
                    p.playlists.len()
                );
                let p = *p;
                j.counts.plays = p.plays;
                j.counts.new_plays = p.new_plays;
                j.problems = p.problems;
                j.steps = p.likes.len().div_ceil(LIKES_PER_STEP) + p.playlists.len();
                j.likes = p.likes.into();
                j.playlists = p.playlists.into();
                self.cold_step(job);
            }
            Err(e) => self.cold_finish(job, Some(e)),
        }
    }

    fn service_fetched(&mut self, job: u64, service: Service, f: Fetched) {
        if !self.streaming.cold.jobs.contains_key(&job) {
            return;
        }
        let mut problems = Vec::new();
        let mut counts = Counts::default();
        match &f.liked {
            Ok(liked) => {
                self.streaming.learn(liked);
                counts.likes = liked.len();
                counts.new_likes = self.apply_remote_likes(service, liked, true, false);
            }
            Err(e) => problems.push(format!("лайки: {}", error_text(e))),
        }
        match &f.recent {
            Ok(recent) => {
                counts.plays = recent.len();
                counts.new_plays = self.store_recent(recent);
            }
            Err(e) => problems.push(format!("история: {}", error_text(e))),
        }
        if f.missed > 0 {
            problems.push(format!("не открылись плейлисты: {}", f.missed));
        }
        let playlists: Vec<PendingPlaylist> = f
            .playlists
            .into_iter()
            .map(|p| PendingPlaylist {
                service,
                name: p.name.clone(),
                remote: Some(p.id.clone()),
                tracks: p.tracks.iter().cloned().map(|t| Wanted::Stream(Box::new(t))).collect(),
                fetched: Some(p),
            })
            .collect();
        let Some(j) = self.streaming.cold.jobs.get_mut(&job) else { return };
        j.counts = counts;
        j.problems = problems;
        j.steps = playlists.len();
        j.playlists = playlists.into();
        if f.liked.is_err() && f.recent.is_err() && j.playlists.is_empty() {
            let e = j.problems.join("; ");
            self.cold_finish(job, Some(e));
            return;
        }
        self.cold_step(job);
    }

    /// Recent plays from the API as events (minus the plays this app
    /// recorded itself); returns how many are new.
    fn store_recent(&mut self, recent: &[Played]) -> usize {
        let tracks: Vec<StreamTrack> = recent.iter().map(|p| p.track.clone()).collect();
        self.streaming.learn(&tracks);
        let Some(ids) = self.log_err("история", self.lib.ensure_stream_tracks(&tracks)) else {
            return 0;
        };
        let device = DeviceId::from(IMPORT_DEVICE);
        let mut events = Vec::new();
        for (p, &track) in recent.iter().zip(&ids) {
            let len = p.track.duration_ms.unwrap_or(0) as i64;
            let own = self.lib.events_of_track(track, 50).unwrap_or_default();
            let same = own.iter().any(|e| {
                e.event_type == EventType::Start
                    && e.device.as_str() != IMPORT_DEVICE
                    && (e.ts - p.at_ms).abs() <= len + SAME_PLAY_MS
            });
            if same {
                continue;
            }
            let kind = Some(p.track.id.service.source_kind());
            for (ts, t, pos) in [
                (p.at_ms, EventType::Start, Some(0)),
                (p.at_ms + len, EventType::Complete, u32::try_from(len).ok()),
            ] {
                events.push(Event {
                    id: event_id(ts, track, t),
                    ts,
                    track_id: track,
                    source_kind: kind,
                    event_type: t,
                    position_ms: pos,
                    context: EventContext::Other,
                    device: device.clone(),
                });
            }
        }
        let (starts, ends): (Vec<Event>, Vec<Event>) =
            events.into_iter().partition(|e| e.event_type == EventType::Start);
        let new = self.log_err("история", self.lib.import_events(&starts)).unwrap_or(0);
        let _ = self.log_err("история", self.lib.import_events(&ends));
        new
    }

    /// One step of a job's worker part: a batch of likes, or one playlist.
    fn cold_step(&mut self, job: u64) {
        let Some(j) = self.streaming.cold.jobs.get_mut(&job) else { return };
        if !j.likes.is_empty() {
            let n = j.likes.len().min(LIKES_PER_STEP);
            let batch: Vec<Wanted> = j.likes.drain(..n).collect();
            let (found, new) = self.import_likes(&batch);
            let Some(j) = self.streaming.cold.jobs.get_mut(&job) else { return };
            j.counts.likes += found;
            j.counts.new_likes += new;
        } else if let Some(p) = j.playlists.pop_front() {
            let new = self.import_playlist(p);
            let Some(j) = self.streaming.cold.jobs.get_mut(&job) else { return };
            j.counts.playlists += 1;
            j.counts.new_playlists += usize::from(new);
        } else {
            self.cold_finish(job, None);
            return;
        }
        let Some(j) = self.streaming.cold.jobs.get_mut(&job) else { return };
        let left = j.likes.len().div_ceil(LIKES_PER_STEP) + j.playlists.len();
        j.subtitle = format!(
            "Импорт: {}",
            count_line(j.counts.plays, j.counts.likes, j.counts.playlists)
        );
        j.progress = Some(1.0 - left as f32 / j.steps.max(1) as f32);
        self.publish_cold_jobs();
        let _ = self
            .streaming
            .internal
            .send(Internal::Stream(Msg::Cold(ColdMsg::Step(job))));
    }

    /// Library tracks of what an import wants; `None` for a named track the
    /// library does not know.
    fn resolve_wanted(&mut self, wanted: &[Wanted]) -> Vec<Option<TrackId>> {
        let streams: Vec<StreamTrack> = wanted
            .iter()
            .filter_map(|w| match w {
                Wanted::Stream(t) => Some((**t).clone()),
                Wanted::Named { .. } => None,
            })
            .collect();
        self.streaming.learn(&streams);
        let mut ids = self
            .log_err("импорт", self.lib.ensure_stream_tracks(&streams))
            .unwrap_or_default()
            .into_iter();
        wanted
            .iter()
            .map(|w| match w {
                Wanted::Stream(_) => ids.next(),
                Wanted::Named { artist, title } => self.lib.track_by_name(artist, title).ok().flatten(),
            })
            .collect()
    }

    /// Likes from an export file; returns (found, new). The likes are not
    /// pushed back: they came from the service.
    fn import_likes(&mut self, batch: &[Wanted]) -> (usize, usize) {
        let mut found = 0;
        let mut new = 0;
        for track in self.resolve_wanted(batch).into_iter().flatten() {
            found += 1;
            match self.lib.like_state(LikeTarget::Track(track)).unwrap_or(None) {
                Some(false) => continue, // unliked by the user since
                Some(true) => {}
                None => {
                    if self.lib.like(track).is_err() {
                        continue;
                    }
                    new += 1;
                }
            }
            self.wish_for_like(track, WishlistOrigin::Import);
        }
        (found, new)
    }

    /// Creates (or, linked already, syncs) one imported playlist; `true`
    /// when a playlist was created.
    fn import_playlist(&mut self, p: PendingPlaylist) -> bool {
        let key = format!("{}:{}", p.service.as_str(), sort_key(&p.name));
        if let Some(remote) = &p.remote
            && let Some(link) = self.lib.playlist_by_remote(remote).ok().flatten()
        {
            let alive = self.lib.get_playlist(link.playlist_id).ok().flatten().is_some();
            if alive
                && link.sync_mode != PlaylistSyncMode::Once
                && let Some(fetched) = &p.fetched
            {
                self.apply_sync(link.playlist_id, fetched);
            }
            return false; // known (or deleted here on purpose)
        }
        let earlier = self
            .lib
            .import_key(EntityKind::Playlist, &key)
            .ok()
            .flatten()
            .map(PlaylistId::from_uuid);
        if let Some(pid) = earlier {
            let alive = self.lib.get_playlist(pid).ok().flatten().is_some();
            let linked = self.lib.playlist_remote(pid).ok().flatten().is_some();
            // A playlist of an export file meets its counterpart in the API.
            if alive
                && !linked
                && let (Some(remote), Some(fetched)) = (&p.remote, &p.fetched)
            {
                let mode = self.import_mode(fetched);
                let _ = self.lib.put_playlist_remote(&PlaylistRemote {
                    playlist_id: pid,
                    remote: remote.clone(),
                    sync_mode: mode,
                    remote_version: None,
                    synced_at: None,
                    base_json: None,
                });
                if mode != PlaylistSyncMode::Once {
                    self.apply_sync(pid, fetched);
                }
            }
            return false;
        }
        let Some(created) = self.log_err("плейлист", self.lib.create_playlist(&p.name)) else {
            return false;
        };
        let pid = created.id;
        let tracks: Vec<TrackId> = self.resolve_wanted(&p.tracks).into_iter().flatten().collect();
        let _ = self.log_err("плейлист", self.lib.playlist_add_many(pid, &tracks));
        for &t in &tracks {
            self.wish_for_like(t, WishlistOrigin::Import);
        }
        let now = self.sync_stamp(pid);
        match (&p.remote, &p.fetched) {
            (Some(remote), Some(fetched)) => {
                let ids: Vec<String> = fetched.tracks.iter().map(|t| t.id.id.clone()).collect();
                let _ = self.lib.put_playlist_remote(&PlaylistRemote {
                    playlist_id: pid,
                    remote: remote.clone(),
                    sync_mode: self.import_mode(fetched),
                    remote_version: fetched.version.clone(),
                    synced_at: Some(now),
                    base_json: serde_json::to_string(&ids).ok(),
                });
            }
            (Some(remote), None) => {
                let ids: Vec<String> = p
                    .tracks
                    .iter()
                    .filter_map(|w| match w {
                        Wanted::Stream(t) => Some(t.id.id.clone()),
                        Wanted::Named { .. } => None,
                    })
                    .collect();
                let _ = self.lib.put_playlist_remote(&PlaylistRemote {
                    playlist_id: pid,
                    remote: remote.clone(),
                    sync_mode: PlaylistSyncMode::Once,
                    remote_version: None,
                    synced_at: Some(now),
                    base_json: serde_json::to_string(&ids).ok(),
                });
            }
            (None, _) => {}
        }
        let _ = self.lib.set_import_key(EntityKind::Playlist, &key, *pid.as_uuid());
        true
    }

    /// The sync mode of a playlist imported from the API: the default, but
    /// someone else's playlist can only be followed.
    fn import_mode(&self, p: &StreamPlaylist) -> PlaylistSyncMode {
        match self.state.read().prefs.playlist_sync_default {
            PlaylistSyncMode::TwoWay if !p.editable => PlaylistSyncMode::Follow,
            m => m,
        }
    }

    fn cold_finish(&mut self, job: u64, error: Option<String>) {
        let Some(j) = self.streaming.cold.jobs.remove(&job) else { return };
        let text = match &error {
            Some(e) => format!("Импорт не удался: {e}"),
            None => summary(&j.counts),
        };
        let mut subtitle = if error.is_some() {
            text.clone()
        } else {
            format!(
                "Найдено: {} · новых: {}",
                count_line(j.counts.plays, j.counts.likes, j.counts.playlists),
                count_line(j.counts.new_plays, j.counts.new_likes, j.counts.new_playlists)
            )
        };
        if !j.problems.is_empty() && error.is_none() {
            subtitle.push_str(&format!(" · не всё: {}", j.problems.join("; ")));
        }
        let _ = self.lib.log_job("import", &j.title, &subtitle, error.as_deref());
        let cold = &mut self.streaming.cold;
        cold.history.push_front(v::Job {
            id: job,
            title: j.title,
            subtitle,
            progress: Some(1.0),
            rate: None,
            eta: None,
            paused: false,
            controllable: false,
            error,
        });
        cold.history.truncate(HISTORY_KEEP);
        self.publish_cold_jobs();
        self.toast(text, None);
        self.after_like_change();
        self.publish_wishlist();
        self.wish_tick();
        self.enrich_tick();
    }

    /// The cold-start jobs into "Загрузки" (the importer's own jobs share the
    /// id space; see [`STREAM_JOB`]).
    pub(crate) fn publish_cold_jobs(&mut self) {
        let active: Vec<v::Job> = self
            .streaming
            .cold
            .jobs
            .iter()
            .map(|(&id, j)| v::Job {
                id,
                title: j.title.clone(),
                subtitle: j.subtitle.clone(),
                progress: j.progress,
                rate: None,
                eta: None,
                paused: false,
                controllable: false,
                error: None,
            })
            .collect();
        let history: Vec<v::Job> = self.streaming.cold.history.iter().cloned().collect();
        self.publish(|s, _| {
            s.downloads.active.retain(|j| j.id & STREAM_JOB == 0);
            s.downloads.active.extend(active);
            s.downloads.history.retain(|j| j.id & STREAM_JOB == 0);
            for (i, j) in history.into_iter().enumerate() {
                s.downloads.history.insert(i, j);
            }
        });
    }

    // ------------------------------------------------------------ enrichment

    /// Completes tracks an export made without their metadata, a small batch
    /// per service at a time (Spotify only when logged in: its catalog
    /// needs the Web API token).
    pub(crate) fn enrich_tick(&mut self) {
        for &service in Service::ALL {
            let Some(svc) = self.service(service) else { continue };
            if service == Service::Spotify && !svc.account().is_logged_in() {
                continue;
            }
            let e = self.streaming.cold.enrich.entry(service).or_default();
            if e.busy || e.next_at.is_some_and(|t| Instant::now() < t) {
                continue;
            }
            let batch = match service {
                Service::Spotify => ENRICH_BATCH_SPOTIFY,
                Service::Youtube => ENRICH_BATCH_YOUTUBE,
            };
            let todo: Vec<(TrackId, String)> = self
                .lib
                .stream_tracks_to_enrich(service, batch * 3)
                .unwrap_or_default()
                .into_iter()
                .filter(|(t, _)| !e.tried.contains(t))
                .take(batch as usize)
                .collect();
            if todo.is_empty() {
                e.next_at = Some(Instant::now() + ENRICH_IDLE);
                continue;
            }
            e.busy = true;
            e.tried.extend(todo.iter().map(|(t, _)| *t));
            self.streaming.spawn(async move {
                let ids: Vec<String> = todo.iter().map(|(_, id)| id.clone()).collect();
                let results = match svc.tracks(&ids).await {
                    Ok(found) => todo
                        .into_iter()
                        .zip(found)
                        .map(|((track, id), t)| (track, id, t.ok_or(StreamError::Unavailable)))
                        .collect(),
                    // The whole batch failed (rate limited): tried again later.
                    Err(e) => todo.into_iter().map(|(track, id)| (track, id, Err(e.clone()))).collect(),
                };
                Msg::Cold(ColdMsg::Enriched { service, results })
            });
        }
    }

    fn enriched(&mut self, service: Service, results: Vec<(TrackId, String, Result<StreamTrack, StreamError>)>) {
        let pause = match service {
            Service::Spotify => self.streaming.schedule.enrich_pause.0,
            Service::Youtube => self.streaming.schedule.enrich_pause.1,
        };
        let mut next = Instant::now() + pause;
        let mut done = 0;
        for (track, id, r) in results {
            match r {
                Ok(t) => {
                    self.streaming.learn(std::slice::from_ref(&t));
                    if self.lib.upsert_stream_track(&t).is_ok() {
                        done += 1;
                    }
                }
                Err(StreamError::Unavailable) => {
                    let kind = service.source_kind();
                    for s in self.lib.sources_of_track(track).unwrap_or_default() {
                        if s.kind == kind && s.locator == id {
                            let _ = self.lib.set_source_available(s.id, false);
                        }
                    }
                }
                Err(StreamError::RateLimited { retry_after }) => {
                    next = Instant::now() + retry_after.unwrap_or(Duration::from_secs(15 * 60));
                    if let Some(e) = self.streaming.cold.enrich.get_mut(&service) {
                        e.tried.remove(&track);
                    }
                }
                Err(e) => log::debug!("{service}: completing {id}: {e}"),
            }
        }
        log::debug!("{service}: completed {done} imported tracks");
        let e = self.streaming.cold.enrich.entry(service).or_default();
        e.busy = false;
        e.next_at = Some(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn russian_counts() {
        assert_eq!(group(1240), "1\u{a0}240");
        assert_eq!(group(999), "999");
        assert_eq!(group(1_000_000), "1\u{a0}000\u{a0}000");
        assert_eq!(plural(1, "a", "b", "c"), "a");
        assert_eq!(plural(3, "a", "b", "c"), "b");
        assert_eq!(plural(11, "a", "b", "c"), "c");
        assert_eq!(plural(22, "a", "b", "c"), "b");
        let c = Counts {
            new_plays: 1240,
            new_likes: 312,
            new_playlists: 9,
            ..Counts::default()
        };
        assert_eq!(
            summary(&c),
            "Импорт: 1\u{a0}240 прослушиваний, 312 лайков, 9 плейлистов"
        );
        assert_eq!(summary(&Counts { likes: 3, ..Counts::default() }), "Импорт: ничего нового — всё уже было");
        let after_login = Counts { plays: 3, new_plays: 3, likes: 3, new_likes: 0, playlists: 2, new_playlists: 2 };
        assert_eq!(summary(&after_login), "Импорт: 3 прослушивания, 2 плейлиста · лайки уже здесь (3)");
        assert_eq!(count_line(0, 0, 0), "ничего");
    }
}
