//! The runtime between the core services and a design system.
//!
//! One worker thread executes [`Command`]s from the UI against the library,
//! the play queue and the playback backend, and publishes the result into a
//! shared [`ViewState`]. The UI reads that state each frame under a short
//! read lock and never calls the library or the audio engine itself.

mod align;
mod analysis;
mod api;
mod artist_names;
mod quality;
mod same_audio;
mod better_covers;
mod cover_refresh;
mod stream_covers;
mod convert;
mod enrich;
mod gain;
mod hd_covers;
mod heap;
pub use heap::release_thread_memory;
mod home;
mod home_build;
mod home_v2;
mod import;
mod inbox_torrents;
mod library_load;
mod library_search;
mod lyrics;
mod mesh;
mod connect;
mod replica;
mod lyrics_keep;
mod playlist_cover;
mod recs;
mod running;
mod search;
mod soulseek;
mod quiet;
pub use quiet::set_enabled as set_background_quiet;
mod tags;
mod torrents;
pub mod queue;
mod session;
mod sharing;
mod sleep;
mod storage;
mod db_copy;
mod duck;
pub use duck::DUCK_RESTORE_FADE;
mod sound_model;
mod editions;
mod engine;
mod streams;
mod api_types;
mod devices;
mod dispatch;
mod library_ops;
mod playback;
mod prefs_ops;
#[cfg(test)]
mod perf;
#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use crossbeam_channel::{Receiver, Sender, select, tick};
use nikgob_core::model::{EventContext, EventOrigin, SourceId, SourceKind, TrackId};
use nikgob_core::streaming::{Service as StreamKind, StreamService};
use nikgob_library::Library;
use nikgob_view::{Command, Status, Toast, ViewState};
use parking_lot::RwLock;

pub use api_types::*;
pub use convert::Resolver;
pub use storage::free_bytes;
pub use import::ImportSetup;
pub use lyrics::memory as lyrics_memory;
pub use nikgob_sources::khinsider::PageBrowser;
pub use recs::{
    HomePreview, ModelPreview, PreviewTile, preview_home, preview_home_with, preview_ready_tiles, preview_replay, preview_replay_with,
    preview_tile_days,
};
pub use nikgob_import::ImportConfig;
pub use nikgob_metadata::{Metadata, MetadataConfig};
pub use streams::{KeptDiscography, SOUNDCLOUD_WINDOW_HINT, StreamSchedule, WINDOW_HINT, WebLogin, WebLoginError, kept_discography, window_hint};
use import::{IMPORT_ID, ImportService, Internal};
use playback::Heard;
use queue::{Entry, Queue};

/// How many of the tracks before the current one the queue view lists.
const HISTORY_SHOWN: usize = 50;
/// The entries kept with who added or picked them before those gone from
/// the queue are let go.
const ENTRY_ACTORS_KEPT: usize = 64;

/// A turn of the worker's loop longer than this is logged: while it lasts
/// no command is answered and no playback event is seen.
const SLOW_TURN: Duration = Duration::from_millis(250);

/// One turn of the worker's loop, timed (see [`SLOW_TURN`]).
#[derive(Default)]
struct Turn {
    began: Option<(Instant, String)>,
    laps: Vec<(&'static str, Instant)>,
}

impl Turn {
    fn begin(&mut self, what: impl FnOnce() -> String) {
        self.began = Some((Instant::now(), what()));
    }

    /// A part of the turn ends here.
    fn lap(&mut self, part: &'static str) {
        self.laps.push((part, Instant::now()));
    }

    fn end(self) {
        let Some((at, what)) = self.began else { return };
        let took = at.elapsed();
        if took < SLOW_TURN {
            return;
        }
        let mut from = at;
        let parts: Vec<String> = self
            .laps
            .iter()
            .map(|(part, to)| {
                let ms = to.duration_since(from).as_millis();
                from = *to;
                format!("{part} {ms} ms")
            })
            .collect();
        let parts = if parts.is_empty() { String::new() } else { format!(" ({})", parts.join(", ")) };
        log::warn!("the player's loop was held for {} ms by {what}{parts}", took.as_millis());
    }
}

/// The first word of a value's `Debug` (an enum's variant), without
/// writing out the rest.
fn head_of(value: &impl std::fmt::Debug) -> String {
    struct Head(String);
    impl std::fmt::Write for Head {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            for c in s.chars() {
                if !(c.is_alphanumeric() || c == '_') || self.0.len() >= 40 {
                    return Err(std::fmt::Error);
                }
                self.0.push(c);
            }
            Ok(())
        }
    }
    let mut head = Head(String::new());
    let _ = std::fmt::write(&mut head, format_args!("{value:?}"));
    head.0
}

/// A toast no snackbar took away is dropped after this long.
const TOAST_KEPT: Duration = Duration::from_secs(60);

/// After we move the position ourselves (seek, start), a report the engine
/// sent before it heard about that can still be on its way: such a far-off
/// report is ignored for this long.
const POSITION_SETTLE: Duration = Duration::from_millis(500);

struct Worker {
    lib: Library,
    res: Resolver,
    playback: engine::Engine,
    state: Arc<RwLock<ViewState>>,
    wake: Arc<dyn Fn() + Send + Sync>,
    queue: Queue,
    /// What the backend is playing (entry id) and since when.
    playing: Option<Entry>,
    /// The entry we just handed to `playback.play()`: the backend reports
    /// that start too (as `Started` or `TrackChanged`), and it must not be
    /// mistaken for a gapless repeat-one loop of the same entry.
    restarted: Option<u64>,
    status: Status,
    position: Duration,
    position_at: Instant,
    /// How fast `position` moves from `position_at` (1 normal; a little off
    /// while it meets the heard position, 0 while the sound stalls).
    rate: f64,
    /// How fast the position of the playing track moves when all is well:
    /// its speed (1 as recorded).
    speed: f64,
    /// The version the playing entry was handed to the engine as, and
    /// whether it keeps its timbre.
    playing_fx: (nikgob_view::SpeedVariant, bool),
    /// The engine's last position report, to tell a stall.
    reported: Option<(u64, Duration)>,
    /// When we last moved `position` ourselves (see [`POSITION_SETTLE`]).
    jumped_at: Instant,
    volume: f32,
    muted: bool,
    /// The queue came back from the last run, paused here; nothing is
    /// loaded in the backend until play (see [`session`]).
    resume_at: Option<Duration>,
    /// The source of the playing entry failed mid-track (entry id, where it
    /// got to): the next one starts there, not from the top.
    failover: Option<(u64, Duration)>,
    /// The entry whose YouTube stream was opened once more after its URLs
    /// kept being refused mid-track (once per entry).
    stream_retried: Option<u64>,
    /// The entry whose DRM-only SoundCloud source went to SoundCloud's own
    /// player (once per entry, see [`OFFICIAL_PLAYER_ROUTE`]).
    official_player_tried: Option<u64>,
    /// `session.json` (volume and queue between runs) and what was last
    /// written to it.
    session_path: Option<PathBuf>,
    session_saved: session::Saver,
    /// Editions of one album found last time (see [`editions`]).
    editions: Vec<nikgob_import::EditionGroup>,
    services: Vec<Box<dyn Service>>,
    toast_id: u64,
    /// What «Очистить» last took out of the queue, for its «Отменить»: the
    /// entry playing then, what was before it and what came after it.
    cleared_queue: Option<(Option<u64>, Vec<TrackId>, Vec<TrackId>)>,
    revision: u64,
    play_context: EventContext,
    /// What the queue plays (`release:<id>`, `playlist:<id>`, a mix, a
    /// radio's seed): the history's `context_id`.
    play_context_id: Option<String>,
    /// The page of the queue the last run left (`release:<id>`…, see
    /// [`context_page`]) and the queue generation it came back in: the
    /// header's link while that queue plays.
    restored_page: Option<(u64, String)>,
    /// Who chose the next start, when the command that starts it knows.
    next_origin: Option<EventOrigin>,
    /// Who started the queue that plays: an API request's token (the voice
    /// assistant, an agent, a guest), `None` the owner. Recorded with each
    /// play of it…
    queue_actor: Option<nikgob_core::settings::ApiActor>,
    /// …but an entry added or picked by someone else plays as them (a
    /// guest's song in the owner's queue is the guest's, the next one the
    /// owner's again)…
    entry_actor: std::collections::HashMap<u64, Option<nikgob_core::settings::ApiActor>>,
    /// …and the playing entry's, from its start.
    play_actor: Option<nikgob_core::settings::ApiActor>,
    /// The queue generation of the last start: a start in another queue
    /// is the user's choice.
    origin_gen: Option<u64>,
    /// The playing entry's end is recorded already.
    ended: bool,
    /// How the playing entry is heard so far (its end's attention and
    /// volume).
    heard: Heard,
    /// Entries «Автовоспроизведение» added to continue an album's world…
    autoplay_entries: std::collections::HashSet<u64>,
    /// …in the queue of this generation (saved with the queue).
    autoplay_gen: Option<u64>,
    /// The prefs changed in a way that is written at the next tick (and at
    /// the end) rather than at once (the shuffle button's rule), or a write
    /// of them failed: written again at the next tick and at the end, until
    /// one succeeds.
    prefs_unsaved: bool,
    /// The last write of the prefs failed (its error was said once).
    prefs_failing: bool,
    prefs_path: Option<PathBuf>,
    settings_path: Option<PathBuf>,
    exported_rev: u64,
    last_export: Instant,
    /// Where the last export went and [`Library::userdata_fingerprint`] then.
    exported_fp: Option<(PathBuf, Vec<i64>)>,
    import: Option<ImportService>,
    /// Registered streaming services (see [`RuntimeConfig::streams`]).
    streams: Vec<Arc<dyn StreamService>>,
    /// Sources of the playing entry that failed to open, and the one asked
    /// for last (see [`Worker::request`]).
    tried: Vec<SourceId>,
    playing_source: Option<(SourceId, SourceKind)>,
    /// What recent stream items really are, by item id (see
    /// [`PlaybackEvent::Quality`]; the preloaded next one opens early).
    qualities: Vec<(u64, QualityLabel)>,
    indexer: Option<Arc<dyn nikgob_core::indexer::Indexer>>,
    /// Download sources other than the indexer's torrents (khinsider, the
    /// free services, Soulseek), whether on or not.
    sources: Vec<Arc<dyn nikgob_core::source::Source>>,
    /// khinsider, so a webview can be given to it after start.
    khinsider: Option<Arc<nikgob_sources::khinsider::Khinsider>>,
    /// Qobuz and Tidal, so `sources.toml` can change their lists.
    qobuz: Option<Arc<nikgob_sources::qobuz::Qobuz>>,
    tidal: Option<Arc<nikgob_sources::tidal::Tidal>>,
    /// «Барахолка», so `sources.toml` can change its sites.
    junkyard: Option<Arc<nikgob_sources::junkyard::Junkyard>>,
    /// `sources.toml`'s mtime when it was last read.
    sources_mtime: Option<SystemTime>,
    candidates: Vec<nikgob_core::indexer::Candidate>,
    torrents: torrents::Torrents,
    /// Accounts, stream search and pages, likes sync, wishlist, sharing.
    streaming: streams::Streams,
    /// The background sound analysis and the vector index it fills.
    analysis: analysis::FileAnalysis,
    /// The background spectrum check of the files (fake lossless).
    quality: quality::QualityJob,
    /// Files that hold another song's audio: the background check, the
    /// pairs in «Подтверди», «В файле другая песня».
    same_audio: same_audio::SameAudioJob,
    /// Mixes, radio, discovery (stage 4).
    recs: recs::RecsState,
    /// Albums whose larger cover was looked up in this run.
    hd_asked: hd_covers::Asked,
    /// Albums whose «Обновить обложку» runs.
    cover_refresh: cover_refresh::Busy,
    /// Albums and clips whose real art was looked up in this run.
    better_asked: better_covers::Asked,
    /// Real art of the stream tracks' video thumbnails on screen.
    stream_covers: stream_covers::StreamCovers,
    /// The album the queue plays (and the queue's generation then, and
    /// whether it plays whole): its tracks the services add later join the
    /// queue when it plays whole; playing the user's songs of it, only a
    /// new one of them does.
    album_queue: Option<(nikgob_core::model::ReleaseId, u64, bool)>,
    /// The Soulseek source (settings.toml `[soulseek]`).
    soulseek: soulseek::SoulseekService,
    /// «Загрузки → Раздача»: speeds and what was given away.
    sharing: sharing::Sharing,
    /// The lyrics aligner's thread.
    align: align::AlignService,
    /// Where its models are downloaded.
    models_dir: Option<PathBuf>,
    align_was_playing: bool,
    /// New likes of a large import that wait for their wishlist items
    /// ([`Worker::wish_for_likes`]), and how many items they made so far.
    wishes_later: std::collections::VecDeque<(TrackId, nikgob_core::model::WishlistOrigin)>,
    wishes_made: usize,
    /// The worker's own inbox (for threads started later).
    internal_tx: Sender<Internal>,
    /// The turn of the loop going on, timed: its parts are marked with
    /// [`Turn::lap`] wherever they are ([`SLOW_TURN`]).
    turn: Turn,
    /// The home page's builder thread ([`home_build`]).
    home_build: home_build::HomeBuild,
    /// Work background threads hand back to the loop ([`Later`]).
    later_tx: Sender<Later>,
    later_rx: Receiver<Later>,
    /// The file of the next track whose tags are read off the loop
    /// ([`Worker::read_facts_ahead`]).
    reading_ahead: Option<PathBuf>,
    /// Files' tags read off the loop ([`gain`]).
    facts_reads: gain::FactsReads,
    /// The playing track whose details wait for its file's tags, and that
    /// file ([`Worker::details_after_read`]).
    details_waiting: Option<(PathBuf, TrackId)>,
    /// The library's part of the search, read off the loop ([`library_search`]).
    library_search: library_search::LibrarySearch,
    /// The library's pages, read off the loop ([`library_load`]).
    library_load: library_load::LibraryLoad,
    /// «Таймер сна» ([`sleep`]).
    sleep: sleep::Sleep,
    /// How much the music takes («Загрузки», [`storage`]).
    storage: storage::Storage,
    /// The day's copy of the database next to the masters.
    db_copies: db_copy::DbCopies,
    /// The heavy background passes run ([`RuntimeOptions::background_compute`]).
    compute: bool,
    /// The user data is exported next to the library ([`RuntimeOptions::userdata_export`]).
    userdata_export: bool,
    /// The sound lowered for a while ([`duck`]).
    duck: duck::DuckState,
    /// The local API's settings and the tokens' uses ([`api`]).
    api: api::ApiKeep,
    /// The owner's devices as kept here ([`mesh`]).
    mesh: mesh::MeshKeep,
    /// Playing on another device ([`connect`]).
    connect: connect::ConnectKeep,
    /// The masters that may go once the devices hold copies ([`replica`]).
    replica: replica::ReplicaKeep,
}

/// Work a background thread hands back to the worker's loop: what it is
/// (a slow turn names it) and what it does there.
pub(crate) struct Later {
    what: &'static str,
    run: Box<dyn FnOnce(&mut Worker) + Send>,
}

impl Later {
    pub(crate) fn new(what: &'static str, run: impl FnOnce(&mut Worker) + Send + 'static) -> Self {
        Self { what, run: Box::new(run) }
    }
}

/// The command changes what comes next in the queue (not a new queue).
fn edits_queue(c: &Command) -> bool {
    matches!(
        c,
        Command::PlayNext(_)
            | Command::Enqueue(_)
            | Command::PlayReleaseNext(_)
            | Command::EnqueueRelease(_)
            | Command::PlayPlaylistNext(_)
            | Command::EnqueuePlaylist(_)
            | Command::PlayArtistNext(_)
            | Command::EnqueueArtist(_)
            | Command::RemoveFromQueue(_)
            | Command::MoveInQueue { .. }
            | Command::ClearQueue
            | Command::UndoClearQueue
            | Command::ToggleShuffle
            | Command::PlayStreamNext(_)
            | Command::EnqueueStream(_)
            | Command::PlayStreamAlbumNext(_)
            | Command::EnqueueStreamAlbum(_)
            | Command::PlayStreamPlaylistNext(_)
            | Command::EnqueueStreamPlaylist(_)
            | Command::PlayStreamArtistNext(_)
            | Command::EnqueueStreamArtist(_)
            | Command::PlayMixNext { .. }
            | Command::EnqueueMix { .. }
    )
}

/// Whether `c` adds songs to the queue that plays (after the current one
/// or at its end), its own entries for who added them
/// ([`Worker::entry_actor`]).
fn adds_to_queue(c: &Command) -> bool {
    matches!(
        c,
        Command::PlayNext(_)
            | Command::Enqueue(_)
            | Command::PlayReleaseNext(_)
            | Command::EnqueueRelease(_)
            | Command::PlayPlaylistNext(_)
            | Command::EnqueuePlaylist(_)
            | Command::PlayArtistNext(_)
            | Command::EnqueueArtist(_)
            | Command::PlayStreamNext(_)
            | Command::EnqueueStream(_)
            | Command::PlayStreamAlbumNext(_)
            | Command::EnqueueStreamAlbum(_)
            | Command::PlayStreamPlaylistNext(_)
            | Command::EnqueueStreamPlaylist(_)
            | Command::PlayStreamArtistNext(_)
            | Command::EnqueueStreamArtist(_)
            | Command::PlayMixNext { .. }
            | Command::EnqueueMix { .. }
    )
}

enum Msg {
    Cmd(Command),
    Service(Box<dyn Service>),
    Indexer(Arc<dyn nikgob_core::indexer::Indexer>),
    Source(Arc<dyn nikgob_core::source::Source>),
    ShareBase(String),
    GraphMetadata(Box<nikgob_metadata::Metadata>),
    WebLogin(StreamKind, Arc<dyn WebLogin>),
    KhinsiderBrowser(Arc<dyn nikgob_sources::khinsider::PageBrowser>),
    /// The mesh service takes the network commands from now on.
    MeshLink(Arc<dyn Fn(Command) + Send + Sync>),
    /// The forgets still waiting for their snackbar go to the mesh service
    /// now; answered once they are passed on (the mesh is about to stop).
    MeshFlush(Sender<()>),
    /// The mesh service does the network part of playing on another device
    /// from now on.
    ConnectLink(connect::ConnectLink),
    Shutdown,
    /// The kind of device this runs on ([`Runtime::set_device_kind`]).
    DeviceKind(nikgob_view::MeshKind),
}

impl Worker {
    fn new(
        config: RuntimeConfig,
        playback: Box<dyn Playback>,
        state: Arc<RwLock<ViewState>>,
        wake: Arc<dyn Fn() + Send + Sync>,
        internal: Sender<Internal>,
    ) -> Self {
        let (torrent_state, torrent_offline) = config
            .import
            .as_ref()
            .map(|i| (i.torrent_state_dir.clone(), i.torrent_offline))
            .unwrap_or_default();
        let slsk_state = config.import.as_ref().map(|i| i.torrent_state_dir.with_file_name("soulseek"));
        let sharing = sharing::Sharing::new(config.import.as_ref().and_then(|i| i.torrent_state_dir.parent()));
        let soulseek = soulseek::SoulseekService::new(config.library_dir.clone(), slsk_state);
        let streaming = streams::Streams::new(internal.clone());
        let analysis = analysis::FileAnalysis::new(internal.clone());
        let quality = quality::QualityJob::new(internal.clone());
        let same_audio = same_audio::SameAudioJob::new(internal.clone());
        let recs = recs::RecsState::new(internal.clone());
        let models_dir = config.import.as_ref().and_then(|i| i.models_dir.clone());
        let internal_tx = internal.clone();
        let import = config
            .import
            .and_then(|setup| ImportService::start(setup, config.library.clone(), internal));
        let mut playback = playback;
        nikgob_core::i18n::apply_pref(config.prefs.language);
        let session_path = config.prefs_path.as_ref().map(|p| p.with_file_name("session.json"));
        let saved = session_path.as_deref().map(session::Session::load).unwrap_or_default();
        let volume = saved.volume.unwrap_or(0.8).clamp(0.0, 1.0);
        playback.set_volume(if saved.muted { 0.0 } else { volume });
        playback.configure(crossfade(&config.prefs), config.prefs.normalization);
        playback.set_equalizer(config.prefs.eq_on, config.prefs.eq_gains);
        {
            // The versions chosen for tracks («Скорость»).
            let chosen: std::collections::HashMap<TrackId, nikgob_view::SpeedVariant> = config
                .library
                .track_fx()
                .unwrap_or_else(|e| {
                    log::warn!("the speeds of the tracks could not be read: {e}");
                    Vec::new()
                })
                .into_iter()
                .filter_map(|(track, key)| Some((track, nikgob_view::SpeedVariant::from_key(&key)?)))
                .collect();
            // Their names, for «Не нравится и скорость».
            let names: std::collections::HashMap<TrackId, nikgob_view::DislikedTrack> =
                chosen.keys().filter_map(|t| speed_name(&config.library, *t).map(|n| (*t, n))).collect();
            let mut st = state.write();
            let enabled = st.prefs.speed_enabled;
            st.speeds = nikgob_view::TrackSpeeds { enabled, chosen: Arc::new(chosen) };
            st.speed_names = Arc::new(names);

        }
        config.library.set_album_likes_tracks(config.prefs.album_like_tracks);
        let web = search::web_sources_from(config.settings_path.as_deref());
        let sources_mtime = config.settings_path.as_ref().and_then(|p| {
            std::fs::metadata(nikgob_sources::config::beside(p)).and_then(|m| m.modified()).ok()
        });
        let res = Resolver::new(config.library.clone(), config.library_dir);
        let home_build = home_build::HomeBuild::start(&res, config.prefs_path.as_deref());
        let (later_tx, later_rx) = crossbeam_channel::unbounded();
        let library_search = library_search::LibrarySearch::start(&res, later_tx.clone());
        let library_load = library_load::LibraryLoad::start(&res, later_tx.clone());
        Self {
            res,
            lib: config.library,
            playback: engine::Engine::new(playback),
            state,
            wake,
            queue: Queue::default(),
            playing: None,
            restarted: None,
            status: Status::Stopped,
            position: Duration::ZERO,
            position_at: Instant::now(),
            rate: 1.0,
            speed: 1.0,
            playing_fx: (nikgob_view::SpeedVariant::Normal, false),
            reported: None,
            jumped_at: Instant::now(),
            volume,
            muted: saved.muted,
            resume_at: None,
            failover: None,
            stream_retried: None,
            official_player_tried: None,
            session_path,
            session_saved: session::Saver::default(),
            editions: Vec::new(),
            services: Vec::new(),
            toast_id: 0,
            cleared_queue: None,
            revision: 0,
            play_context: EventContext::Other,
            play_context_id: None,
            restored_page: None,
            next_origin: None,
            queue_actor: None,
            entry_actor: std::collections::HashMap::new(),
            play_actor: None,
            origin_gen: None,
            ended: true,
            heard: Heard::default(),
            autoplay_entries: Default::default(),
            autoplay_gen: None,
            prefs_unsaved: false,
            prefs_failing: false,
            prefs_path: config.prefs_path,
            settings_path: config.settings_path.clone(),
            exported_rev: 0,
            last_export: Instant::now(),
            exported_fp: None,
            indexer: search::indexer_from(config.settings_path.as_deref()),
            sources: web.list,
            khinsider: web.khinsider,
            qobuz: web.qobuz,
            tidal: web.tidal,
            junkyard: web.junkyard,
            sources_mtime,
            candidates: Vec::new(),
            torrents: torrents::Torrents::new(torrent_state, torrent_offline),
            import,
            streams: config.streams,
            tried: Vec::new(),
            playing_source: None,
            qualities: Vec::new(),
            streaming,
            analysis,
            quality,
            same_audio,
            hd_asked: Default::default(),
            cover_refresh: Default::default(),
            better_asked: Default::default(),
            stream_covers: Default::default(),
            album_queue: None,
            recs,
            soulseek,
            sharing,
            align: Default::default(),
            models_dir,
            align_was_playing: false,
            wishes_later: Default::default(),
            wishes_made: 0,
            internal_tx,
            turn: Turn::default(),
            home_build,
            later_tx,
            later_rx,
            reading_ahead: None,
            facts_reads: gain::FactsReads::default(),
            details_waiting: None,
            library_search,
            library_load,
            sleep: Default::default(),
            storage: Default::default(),
            db_copies: Default::default(),
            compute: true,
            userdata_export: true,
            duck: Default::default(),
            api: Default::default(),
            mesh: Default::default(),
            connect: Default::default(),
            replica: Default::default(),
        }
    }

    fn run(&mut self, rx: Receiver<Msg>, pb: Receiver<PlaybackEvent>, internal: Receiver<Internal>) {
        let ticker = tick(self.streaming.schedule.tick);
        let fast = tick(Duration::from_secs(1));
        let homes = self.home_build.done.clone();
        let later = self.later_rx.clone();
        // The last run's home page at once; this run's is built while the
        // rest below starts (the command the start sent builds it again
        // once the loop runs).
        self.show_kept_home();
        self.load_home();
        // Once each, after a backup: doubles of a song, an artist or an
        // album become one, and every album has a line per place it counts.
        self.lib.repair_catalog();
        // Once: the videos that are no music an old import left in its
        // playlists go from them (entries only).
        match self.lib.drop_imported_non_music(streams::music_video_title) {
            Ok(0) => {}
            Ok(n) => log::info!("library: repair of imported non-music entries: {n}"),
            Err(e) => log::warn!("library: repair of imported non-music entries: {e}"),
        }
        self.export_if_stale();
        self.load_torrent_history();
        self.publish_downloads();
        self.resume_torrents();
        self.soulseek_apply();
        self.start_enrich();
        self.restore_session();
        self.offer_editions();
        self.wish_restart();
        self.publish_sources();
        self.api_load();
        self.mesh_load();
        self.start_streams();
        if self.compute {
            self.start_analysis();
            self.start_quality();
            self.start_same_audio();
            self.start_align();
        } else {
            log::info!("background compute is off: no sound analysis, lyrics alignment or file checks here");
            // The vectors the other devices made, as the sync brought them.
            let prefs = self.state.read().prefs.clone();
            self.analysis.load_synced(self.lib.clone(), prefs);
        }
        self.dislikes_load(false);
        loop {
            // What this turn of the loop does, for [`SLOW_TURN`].
            self.turn = Turn::default();
            // The duck's end, to the millisecond rather than at a poll.
            let duck_end = match self.duck.deadline() {
                Some(at) => crossbeam_channel::at(at),
                None => crossbeam_channel::never(),
            };
            select! {
                recv(rx) -> m => match m {
                    Ok(Msg::Cmd(c)) => {
                        self.turn.begin(|| format!("command {}", head_of(&c)));
                        self.user_command(c);
                    }
                    Ok(Msg::Service(s)) => self.services.push(s),
                    Ok(Msg::Indexer(i)) => {
                        self.turn.begin(|| "the indexer".to_owned());
                        self.indexer = Some(i);
                        self.publish_sources();
                        self.wish_sources_ready();
                        self.wish_tick();
                    }
                    Ok(Msg::Source(s)) => {
                        self.turn.begin(|| "a source".to_owned());
                        // One per kind: a new one of a kind replaces the old.
                        self.sources.retain(|x| x.key() != s.key());
                        self.sources.push(s);
                        self.publish_sources();
                        self.wish_sources_ready();
                        self.wish_tick();
                    }
                    Ok(Msg::ShareBase(base)) => self.set_share_base(&base),
                    Ok(Msg::GraphMetadata(m)) => self.set_graph_metadata(*m),
                    Ok(Msg::WebLogin(service, login)) => self.set_web_login(service, login),
                    Ok(Msg::KhinsiderBrowser(browser)) => {
                        if let Some(k) = &self.khinsider {
                            k.set_browser(browser);
                        }
                    }
                    Ok(Msg::MeshLink(f)) => self.mesh.link = Some(f),
                    Ok(Msg::ConnectLink(f)) => self.connect.link = Some(f),
                    Ok(Msg::MeshFlush(done)) => {
                        self.mesh_flush_now();
                        let _ = done.send(());
                    }
                    Ok(Msg::DeviceKind(kind)) => {
                        if recs::set_own_kind(kind) {
                            log::info!("recs: this is a {} device", kind.key());
                            self.recs_dirty(Duration::ZERO);
                        }
                    }
                    Ok(Msg::Shutdown) | Err(_) => {
                        self.record_closed();
                        break;
                    }
                },
                recv(pb) -> e => if let Ok(e) = e {
                    self.turn.begin(|| format!("playback {}", head_of(&e)));
                    self.playback_event(e);
                },
                recv(internal) -> m => if let Ok(m) = m {
                    self.turn.begin(|| m.turn_name());
                    self.internal(m);
                },
                recv(homes) -> m => if let Ok(done) = m {
                    self.turn.begin(|| "a home page came".to_owned());
                    self.home_built(done);
                },
                recv(later) -> m => if let Ok(l) = m {
                    let what = l.what;
                    self.turn.begin(|| what.to_owned());
                    (l.run)(self);
                },
                recv(duck_end) -> _ => {
                    self.turn.begin(|| "the duck's end".to_owned());
                    self.duck_due();
                },
                recv(fast) -> _ => {
                    self.turn.begin(|| "the second's poll".to_owned());
                    self.soloist_poll();
                    self.turn.lap("soloist");
                    self.poll_torrents();
                    self.turn.lap("torrents");
                    self.poll_share();
                    self.turn.lap("sharing");
                    self.wish_poll_direct();
                    self.turn.lap("wishes");
                    self.reload_sources_file();
                    self.turn.lap("sources file");
                    self.sleep_poll();
                    self.storage_poll();
                    self.db_copy_poll();
                    self.turn.lap("sleep and storage");
                },
                recv(ticker) -> _ => {
                    self.turn.begin(|| "the tick".to_owned());
                    self.expire_toasts();
                    self.maybe_export(false);
                    self.turn.lap("export");
                    self.save_session();
                    self.save_prefs_due();
                    self.keep_home_due();
                    self.turn.lap("session");
                    self.stream_tick();
                    self.turn.lap("streams");
                    self.recs_tick();
                    self.turn.lap("recs");
                    self.api_tick();
                    self.turn.lap("api");
                    self.mesh_tick();
                    self.turn.lap("mesh");
                    self.replica_tick();
                    self.turn.lap("replica");
                }
            }
            std::mem::take(&mut self.turn).end();
            self.analysis.set_playing(matches!(self.status, Status::Playing | Status::Buffering));
            self.quality.set_playing(matches!(self.status, Status::Playing | Status::Buffering));
            self.same_audio.set_playing(matches!(self.status, Status::Playing | Status::Buffering));
            self.align_playing(matches!(self.status, Status::Playing | Status::Buffering));
        }
        self.save_session();
        self.save_prefs_due();
        self.api_flush();
        self.mesh_flush();
        self.session_saved.flush();
        self.recs_flush_weights(true);
        self.niche_flush();
        self.keep_home(true);
        self.maybe_export(true);
        self.playback.stop();
        self.shutdown_torrents();
        self.soulseek.shutdown();
        self.sharing.save(true);
        self.streaming.analysis.shutdown();
        self.analysis.shutdown();
        self.quality.shutdown();
        self.same_audio.shutdown();
        self.align.shutdown();
    }

    /// Applies `f` under the state's write lock. The UI waits for this lock
    /// every frame, so view values are built before it (the library, the
    /// resolver and files are never touched inside `f`).
    fn publish(&mut self, f: impl FnOnce(&mut ViewState)) {
        {
            let mut st = self.state.write();
            f(&mut st);
            self.revision += 1;
            st.revision = self.revision;
        }
        (self.wake)();
    }

    pub(crate) fn locale(&self) -> nikgob_core::i18n::Locale {
        self.state.read().prefs.resolved_locale()
    }

    pub(crate) fn undo_label(&self) -> String {
        nikgob_core::i18n::common_in(self.locale()).undo.to_owned()
    }

    pub(crate) fn undo_action(&self, cmd: Command) -> (String, Command) {
        (self.undo_label(), cmd)
    }

    pub(crate) fn toast_nothing_to_play(&mut self) {
        let msg = nikgob_core::i18n::errors_in(self.locale()).nothing_to_play;
        self.toast_err(msg, None);
    }

    pub(crate) fn toast_failed_save_prefs(&mut self) {
        let msg = nikgob_core::i18n::errors_in(self.locale()).failed_save_prefs;
        self.toast_err(msg, None);
    }

    pub(crate) fn toast_info(&mut self, text: impl Into<String>, action: Option<(String, Command)>) {
        self.push_toast(nikgob_view::ToastKind::Info, text.into(), action, None, false);
    }

    pub(crate) fn toast_ok(&mut self, text: impl Into<String>, action: Option<(String, Command)>) {
        self.push_toast(nikgob_view::ToastKind::Success, text.into(), action, None, false);
    }

    pub(crate) fn toast_err(&mut self, text: impl Into<String>, action: Option<(String, Command)>) {
        self.push_toast(nikgob_view::ToastKind::Error, text.into(), action, None, false);
    }

    pub(crate) fn toast_work(&mut self, text: impl Into<String>, action: Option<(String, Command)>) {
        self.push_toast(nikgob_view::ToastKind::Working, text.into(), action, None, false);
    }

    /// A toast whose button takes the user somewhere ([`nikgob_view::Link`]).
    pub(crate) fn toast_open(
        &mut self,
        kind: nikgob_view::ToastKind,
        text: impl Into<String>,
        label: &str,
        link: nikgob_view::Link,
    ) {
        self.push_toast(kind, text.into(), None, Some((label.to_owned(), link)), false);
    }

    pub(crate) fn push_toast(
        &mut self,
        kind: nikgob_view::ToastKind,
        text: String,
        action: Option<(String, Command)>,
        open: Option<(String, nikgob_view::Link)>,
        long: bool,
    ) -> u64 {
        // A like or unlike makes the undo of the one before on the same
        // thing wrong («Убрано из Любимых · Отменить» after a like again):
        // it is withdrawn, so the snackbar says what the heart shows.
        if let Some((_, Command::Like(target, _))) = &action {
            let stale = |t: &Toast| matches!(&t.action, Some((_, Command::Like(old, _))) if old == target);
            if self.state.read().toasts.iter().any(|t| stale(t) && t.text != text) {
                self.publish(|s| s.toasts.retain(|t| !stale(t) || t.text == text));
            }
        }
        // The same message while it is up: its time starts over.
        let same = self.state.read().toasts.iter().position(|t| t.text == text && t.action == action && t.open == open);
        if let Some(k) = same {
            let mut id = 0;
            self.publish(|s| {
                let t = &mut s.toasts[k];
                t.kind = kind;
                t.at = Instant::now();
                t.long |= long;
                t.again += 1;
                id = t.id;
            });
            return id;
        }
        self.toast_id += 1;
        let id = self.toast_id;
        let t = Toast {
            id,
            kind,
            text,
            action,
            open,
            at: Instant::now(),
            long,
            again: 0,
        };
        self.publish(|s| s.toasts.push(t));
        id
    }

    /// Toasts no snackbar host took away (the window was hidden): the UI
    /// shows one at a time and dismisses each as it ends, a toast waiting
    /// behind an undo too, so this is only the last resort.
    fn expire_toasts(&mut self) {
        let stale = {
            let s = self.state.read();
            // «Пока не закрою»: they stay until the user closes them.
            s.prefs.toast_s != 0 && s.toasts.iter().any(|t| t.at.elapsed() > TOAST_KEPT)
        };
        if stale {
            self.publish(|s| s.toasts.retain(|t| t.at.elapsed() <= TOAST_KEPT));
        }
    }

    fn log_err<T>(&mut self, what: &str, r: Result<T, nikgob_library::LibraryError>) -> Option<T> {
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                log::error!("{what}: {e}");
                let msg = nikgob_core::i18n::ErrorStrings::error_with_in(self.locale(), what);
                self.toast_err(msg, None);
                None
            }
        }
    }
}

/// The page a queue's context id opens (`release:<id>`, `playlist:<id>`,
/// `artist:<id>`); none for the rest (a mix, a radio, a search).
fn context_page(id: &str) -> Option<nikgob_view::Route> {
    use nikgob_view::Route;
    let (kind, id) = id.split_once(':')?;
    match kind {
        "release" => id.parse().ok().map(Route::Release),
        "playlist" => id.parse().ok().map(Route::Playlist),
        "artist" => id.parse().ok().map(Route::Artist),
        _ => None,
    }
}

/// What the library held before an edit of likes or playlists
/// ([`Worker::library_edited`]).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Held {
    /// How many tracks (`None`: unknown).
    Tracks(Option<u32>),
    /// The edit cannot move a track in or out (a playlist created or
    /// renamed, its cover).
    Same,
}

/// Crossfade length from prefs; `None` means gapless back-to-back.
/// The name of `track` as «Не нравится и скорость» lists it.
fn speed_name(lib: &Library, track: TrackId) -> Option<nikgob_view::DislikedTrack> {
    let t = lib.get_track(track).ok().flatten()?;
    Some(nikgob_view::DislikedTrack { id: track, title: t.title, artist: t.artist_credit.display() })
}

fn crossfade(p: &nikgob_view::Prefs) -> Option<Duration> {
    (p.crossfade_s > 0.0).then(|| Duration::from_secs_f32(p.crossfade_s))
}

/// Which tracks the library lists ([`nikgob_view::Prefs::library_only_downloaded`]).
fn track_filter(only_local: bool) -> nikgob_library::TrackFilter {
    if only_local {
        nikgob_library::TrackFilter::LocalOnly
    } else {
        nikgob_library::TrackFilter::All
    }
}
