//! music.nikgob: the desktop player.
//!
//! Opens the library, starts the audio engine and the runtime, and draws
//! the MD3E interface. Closing the window hides the app to the tray while
//! music keeps playing: the window only hides and comes back at once
//! («Быстрое открытие из трея»), or, with that off, it is destroyed and
//! made again (with its textures) on show.

#![windows_subsystem = "windows"]

mod audio;
mod autostart;
mod cover_cache;
mod demo;
mod desktop;
mod fallback_fonts;
mod frames;
mod geometry;
mod logfile;
mod pacing;
mod shot;
mod shot_fakes;
mod streams;
mod sysmotion;
mod titlebar;
mod web_images;
mod ytlogin;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use fastframe_shell::{Closed, Headless, Resident, Shell, Waker};
use nikgob_md3e::screens::Md3eUi;
use nikgob_runtime::{NullPlayback, Playback, Runtime, RuntimeConfig, StreamSchedule};
use nikgob_view::{Command, Prefs};

/// Where things live on disk (PLAN §5).
struct Paths {
    db: PathBuf,
    root: PathBuf,
    prefs: PathBuf,
    settings: Option<PathBuf>,
}

fn paths() -> Paths {
    let dirs = directories::ProjectDirs::from("dev", "nikgob", "music.nikgob");
    let data = dirs
        .as_ref()
        .map_or_else(|| PathBuf::from("."), |d| d.data_dir().to_owned());
    let config = dirs
        .as_ref()
        .map_or_else(|| PathBuf::from("."), |d| d.config_dir().to_owned());
    let root = directories::UserDirs::new()
        .and_then(|u| u.audio_dir().map(|a| a.join("music.nikgob")))
        .unwrap_or_else(|| data.join("library"));
    Paths {
        db: data.join("library.sqlite"),
        root,
        prefs: config.join("prefs.json"),
        settings: nikgob_core::settings::Settings::default_path().ok(),
    }
}

/// A windowed app has no stderr: panics (thread, message, place, backtrace)
/// are appended to `crash.log` next to the library, then handled as usual.
fn log_panics(dir: PathBuf) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        use std::io::Write;
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let thread = std::thread::current();
        let report = format!(
            "--- panic at unix {at} in thread '{}'\n{info}\n{}\n",
            thread.name().unwrap_or("?"),
            std::backtrace::Backtrace::force_capture(),
        );
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("crash.log"))
        {
            let _ = f.write_all(report.as_bytes());
        }
        log::error!("{info}");
        default(info);
    }));
}

/// The prefs of a prefs file's bytes; without a file (the first start)
/// the defaults, and Главная asks for the name.
fn prefs_of(bytes: Option<&[u8]>) -> Prefs {
    match bytes {
        Some(b) => parse_prefs(b).unwrap_or_default(),
        None => Prefs { name_prompt: true, ..Prefs::default() },
    }
}

fn parse_prefs(bytes: &[u8]) -> serde_json::Result<Prefs> {
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    let slot = if value.get("lyrics_mega").and_then(|v| v.as_bool()) == Some(true) {
        nikgob_view::NpSlot::Lyrics
    } else if value.get("np_pane").and_then(|v| v.as_bool()) == Some(true) {
        nikgob_view::NpSlot::Queue
    } else {
        nikgob_view::NpSlot::None
    };
    let legacy = value.get("np_slot").is_none();
    // Prefs from before a change of the lyrics defaults take the new ones.
    let lyrics_rev = value.get("lyrics_rev").and_then(|v| v.as_u64()).map_or(0, |r| r.min(u64::from(u8::MAX)) as u8);
    let mut prefs: Prefs = serde_json::from_value(value)?;
    if legacy { prefs.np_slot = slot; }
    prefs.upgrade_lyrics(lyrics_rev);
    Ok(prefs)
}

/// The prefs file with what the settings file and the system add: `settings`
/// as read once at the start, `devices` the audio outputs (enumerated on
/// their own thread meanwhile).
fn load_prefs(
    paths: &Paths,
    settings: Option<&nikgob_core::settings::Settings>,
    devices: std::thread::JoinHandle<Vec<String>>,
) -> Prefs {
    let mut p = prefs_of(std::fs::read(&paths.prefs).ok().as_deref());
    if p.library_dir.as_os_str().is_empty() {
        p.library_dir = paths.root.join("Music");
    }
    if p.inbox_dir.as_os_str().is_empty() {
        p.inbox_dir = paths.root.join("Inbox");
    }
    if p.masters_dir.as_os_str().is_empty() {
        p.masters_dir = p.library_dir.join(".musiclib").join("masters");
    }
    if let Some(s) = settings {
        if let Some(pr) = &s.indexer.prowlarr {
            p.prowlarr_url = pr.url.clone();
            p.prowlarr_configured = !pr.api_key.is_empty();
        }
        p.metadata_contact = s.metadata.contact.clone();
        p.acoustid_configured = s.metadata.acoustid_key.as_ref().is_some_and(|k| !k.trim().is_empty());
        p.lastfm_configured = s.metadata.lastfm_key.as_ref().is_some_and(|k| !k.trim().is_empty());
        p.spotify_personal_app = s.streaming.spotify_client_id.is_some();
        p.youtube_cookies_set = s.streaming.youtube_cookies.is_some();
    }
    p.output_devices = devices.join().unwrap_or_default();
    if p.output_device.as_ref().is_some_and(|d| !p.output_devices.contains(d)) {
        log::warn!("audio device {:?} is gone, using the default", p.output_device);
    }
    p
}

/// Everything that outlives a window.
struct Core {
    ui: Md3eUi,
    runtime: Runtime,
    desktop: desktop::Desktop,
    analyzer: Option<audio::AnalyzerSlot>,
    commands: Vec<Command>,
    shot: Option<shot::Shot>,
    /// `NIKGOB_FRAMES`: the frame-time log.
    frames: Option<frames::Frames>,
    quit: bool,
    /// A screenshot run's `reopen`: the window closes and a new one opens.
    reopen: bool,
    wants_show: bool,
    /// The window is kept but hidden in the tray (it draws nothing).
    hidden: bool,
    /// When the window was asked for from the tray: its first frame logs
    /// how long it took to show.
    show_at: Option<Instant>,
    last_frame: Instant,
    /// The frame limit, kept to whole vblanks ([`pacing`]).
    pacer: pacing::Pacer,
    /// One clock for every window: each new window's egui starts its time
    /// at zero, and animations started in the old one would sit in the
    /// future (the page stayed blank until enough clicks repainted it).
    started: Instant,
    geometry: geometry::Keeper,
    /// The last [`nikgob_view::ClipboardRequest`] copied.
    copied: u64,
    /// The window state the runtime last heard (visible, focused).
    window: Option<(bool, bool)>,
    /// The «Запускать вместе с Windows» switch, put into the system.
    autostart: autostart::Keeper,
    /// A second launch asked for the window.
    show_asked: Arc<AtomicBool>,
}

impl Core {
    /// Tells the runtime when the window is shown, hidden or (un)focused:
    /// it holds its periodic network work while hidden and idle.
    fn report_window(&mut self, visible: bool, focused: bool) {
        if self.window != Some((visible, focused)) {
            self.window = Some((visible, focused));
            self.runtime.send(Command::WindowState { visible, focused });
        }
    }

    fn poll_desktop(&mut self) {
        let req = {
            let lock = nikgob_md3e::prof::scope("state-lock");
            let state = self.runtime.state();
            drop(lock);
            self.desktop.poll(&state, &mut self.commands)
        };
        self.quit |= req.quit;
        self.wants_show |= req.show || req.toggle || self.show_asked.swap(false, Ordering::Relaxed);
        self.sync_autostart();
        self.runtime.send_all(self.commands.drain(..));
    }

    /// Puts a changed autostart switch into the system; one it refused
    /// flips back in Settings.
    fn sync_autostart(&mut self) {
        let system = {
            let state = self.runtime.state();
            self.autostart.sync(state.prefs.autostart)
        };
        if let Some(system) = system {
            self.commands.push(Command::SetPref(nikgob_view::PrefPatch::Autostart(system)));
        }
    }
}

impl Resident for Core {
    /// A start at sign-in may stay in the tray, if the tray is there.
    fn start_hidden(&mut self) -> bool {
        self.desktop.can_hide()
    }

    fn closed(&self) -> Closed {
        if self.reopen {
            Closed::Reopen
        } else if !self.quit && self.desktop.can_hide() {
            Closed::Hide
        } else {
            Closed::Quit
        }
    }

    fn window_gone(&mut self) {
        self.report_window(false, false);
        self.geometry.flush();
        self.wants_show = false;
    }

    fn headless_frame(&mut self, _ctx: &egui::Context) -> Headless {
        self.poll_desktop();
        if self.quit {
            Headless::Quit
        } else if self.wants_show {
            self.show_at = Some(Instant::now());
            Headless::Show
        } else {
            Headless::Wait
        }
    }

    fn shutdown(&mut self) {
        self.geometry.flush();
        self.runtime.send(Command::Pause);
    }
}

/// The largest texture side the app asks of egui (the font atlas's width).
const MAX_TEXTURE_SIDE: usize = 8192;

struct Window {
    core: fastframe_shell::Held<Core>,
    scrolling: fastframe_scroll::Scrolling,
    recovery_checked: bool,
    /// When the system animation setting was last read.
    motion_checked: Option<Instant>,
    titlebar: titlebar::TitleBar,
}

impl eframe::App for Window {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        raw_input.time = Some(self.core.started.elapsed().as_secs_f64());
        // egui makes the font atlas as wide as the largest texture the GPU
        // takes (16384) and lets it grow as tall before starting over: up to
        // a gigabyte in memory, and on the GPU again. Nothing here needs
        // textures that large (covers stop at 4096).
        raw_input.max_texture_side = Some(raw_input.max_texture_side.unwrap_or(MAX_TEXTURE_SIDE).min(MAX_TEXTURE_SIDE));
        if let Some(s) = self.core.shot.as_mut() {
            s.raw_input(raw_input);
        }
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.core.pacer.begin();
        if let Some(f) = self.core.frames.as_mut() {
            f.begin();
        }
        // A screenshot run stays off the screen on purpose.
        if !std::mem::replace(&mut self.recovery_checked, true) && !shot::offscreen() {
            fastframe_shell::window::recover_offscreen(ctx, frame);
        }
        if self.motion_checked.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) {
            self.motion_checked = Some(Instant::now());
            self.core.ui.set_system_reduced_motion(sysmotion::reduced());
        }
        let core = &mut *self.core;
        if !core.hidden {
            core.geometry.observe(ctx);
        }
        let (minimized, focused, close) = ctx.input(|i| {
            let v = i.viewport();
            (v.minimized.unwrap_or(false), v.focused.unwrap_or(true), v.close_requested())
        });
        // «Быстрое открытие из трея»: the window hides instead of closing.
        // Hidden, eframe runs only this logic (no drawing, no animation),
        // and only when something wakes it: the tray, the runtime.
        if close && !core.quit && !core.reopen && core.desktop.can_hide() && core.runtime.state().prefs.tray_keep_window {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            core.hidden = true;
            core.geometry.flush();
            log::info!("window hidden to the tray (kept)");
        }
        let shown = !minimized && !core.hidden;
        core.report_window(shown, focused && shown);
        core.poll_desktop();
        if core.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if std::mem::take(&mut core.wants_show) {
            // Tray "show/hide" while the window exists: bring it forward.
            if std::mem::take(&mut core.hidden) {
                core.show_at = Some(Instant::now());
                // Drawn at once with what it had: no rebuild.
                ctx.request_repaint();
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // Kept in the tray: eframe still calls this for a hidden window
        // (about ten times a second while something asks for frames), so
        // nothing is drawn and nothing animates until it is shown again.
        if self.core.hidden {
            return;
        }
        self.scrolling.apply(ui.ctx());
        let core = &mut *self.core;
        let levels = nikgob_md3e::prof::scope("levels");
        if core.ui.wants_levels()
            && let Some(slot) = &core.analyzer
            && let Some(a) = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_mut()
        {
            a.update();
            core.ui.set_levels(a.bass_energy());
            core.ui.set_bands(a.bands());
        }
        drop(levels);
        let fps = {
            let lock = nikgob_md3e::prof::scope("state-lock");
            let state = core.runtime.state();
            drop(lock);
            // Screenshot runs may lay states the demo cannot reach over it.
            let faked = core.shot.as_ref().and_then(|s| s.overlay(&state));
            let draw = nikgob_md3e::prof::scope("frame");
            core.ui.frame(ui, faked.as_ref().unwrap_or(&*state), &mut core.commands);
            drop(draw);
            // The runtime asks for the clipboard (a share link); copied once.
            if let Some(c) = state.clipboard.as_ref().filter(|c| c.id != core.copied) {
                core.copied = c.id;
                ui.ctx().copy_text(c.text.clone());
            }
            self.titlebar.sync(ui.ctx(), frame, state.prefs.theme);
            if let Some(s) = core.shot.as_mut() {
                if s.frame(ui.ctx(), &mut core.ui, &state, &mut core.commands) {
                    core.quit = true;
                }
                if s.take_reopen() {
                    core.reopen = true;
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if let Some(after) = s.take_hide() {
                    // As the window's ✕, then the tray's click a moment later.
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    let (asked, ctx) = (Arc::clone(&core.show_asked), ui.ctx().clone());
                    std::thread::spawn(move || {
                        std::thread::sleep(after);
                        asked.store(true, Ordering::Relaxed);
                        ctx.request_repaint();
                    });
                }
            }
            state.prefs.fps_limit.max(15)
        };
        core.runtime.send_all(core.commands.drain(..));
        static FIRST: AtomicBool = AtomicBool::new(true);
        if FIRST.swap(false, Ordering::Relaxed) {
            stage("first frame");
        }
        if let Some(t) = core.show_at.take() {
            log::info!("window: shown from the tray, first frame drawn in {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
        }
        if let Some(f) = core.frames.as_mut() {
            let atlas = ui.ctx().fonts(|f| f.font_image_size());
            f.end(frame.info().cpu_usage, core.shot.as_ref().map_or("live", |s| s.label()), atlas);
        }

        // Frame limit: only matters while something animates continuously.
        let wait = core.pacer.wait(frame, fps, core.last_frame.elapsed());
        if !ui.ctx().has_requested_repaint() {
            core.pacer.idle();
        } else if !wait.is_zero() {
            std::thread::sleep(wait);
            if let Some(f) = core.frames.as_mut() {
                f.slept(wait);
            }
        }
        core.last_frame = Instant::now();
    }
}

/// Installed fonts for the scripts Roboto Flex does not draw (CJK, Arabic,
/// ...), behind every family, so Japanese titles are not boxes.
fn add_system_fallbacks(fonts: &mut egui::FontDefinitions) {
    for f in fallback_fonts::faces() {
        let mut data = egui::FontData::from_static(f.bytes);
        data.index = f.index;
        data.tweak.scale = f.scale;
        data.tweak.y_offset_factor = f.y_offset_factor;
        fonts.font_data.insert(f.name.clone(), Arc::new(data));
        for family in fonts.families.values_mut() {
            family.push(f.name.clone());
        }
    }
}

/// When the process started, for the startup stages in the log.
static LAUNCHED: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// Logs how long after the start a stage of the startup ended.
fn stage(name: &str) {
    let ms = LAUNCHED.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0;
    log::info!("startup: {name} at {ms:.0} ms");
}

fn main() -> eframe::Result {
    LAUNCHED.get_or_init(Instant::now);
    // `--youtube-login`: this run is only the Google sign-in window. It logs to
    // stderr, which the app copies into its own log; it must not write the
    // app's log file itself.
    if std::env::args().any(|a| a == "--youtube-login") {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
        if let Some(code) = ytlogin::child_main() {
            std::process::exit(code);
        }
    }
    logfile::init();
    stage("log");
    // Meanwhile, while the library opens and the window is made: the
    // fallback faces (the window's creator waits for them if need be) and
    // the audio outputs (the prefs wait for them).
    let _ = std::thread::Builder::new().name("fallback-fonts".into()).spawn(|| {
        fallback_fonts::faces();
    });
    let devices = std::thread::spawn(|| nikgob_audio::output_device_names().unwrap_or_default());
    let mut paths = paths();
    // `NIKGOB_DATA=dir`: a demo run on a library already in `dir` (a copy
    // of a real one, say) instead of the generated one.
    let data_dir_asked = std::env::var_os("NIKGOB_DATA").map(PathBuf::from);
    let demo = std::env::var_os("NIKGOB_DEMO").is_some() || data_dir_asked.is_some();
    let at_sign_in = std::env::args().any(|a| a == autostart::FLAG);
    // One player per user: a second launch (a click on the shortcut while
    // the first waits in the tray) shows the running one's window and ends.
    // Demo and screenshot runs live beside the real app.
    let show_asked = Arc::new(AtomicBool::new(false));
    let waker = Waker::default();
    let _instance = if demo {
        None
    } else {
        let asked = Arc::clone(&show_asked);
        let w = waker.clone();
        match fastframe_instance::Slot::new("music.nikgob").claim("show", move |request| {
            (request == "show").then(|| {
                asked.store(true, Ordering::Relaxed);
                w.wake();
                "ok".to_owned()
            })
        }) {
            fastframe_instance::Claim::First(guard) => Some(guard),
            other => {
                log::info!("music.nikgob is already running: {other:?}");
                return Ok(());
            }
        }
    };
    if demo {
        let dir = data_dir_asked.clone().unwrap_or_else(|| std::env::temp_dir().join("nikgob-demo"));
        // A demo library from an older seed (duplicate artists, old covers)
        // is thrown away and seeded again.
        let marker = dir.join(demo::VERSION);
        if data_dir_asked.is_none() && !marker.exists() {
            let _ = std::fs::remove_dir_all(&dir);
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(&marker, b"");
        }
        paths.db = dir.join("library.sqlite");
        paths.root = dir.join("root");
        paths.prefs = dir.join("prefs.json");
        // The real services keep what is typed in next to the demo library.
        paths.settings = streams::real_in_demo().then(|| dir.join("settings.toml"));
    }
    // Next to the library in use: a demo's crash stays in the demo's dir.
    if let Some(dir) = paths.db.parent() {
        log_panics(dir.to_owned());
    }
    for sub in ["Music", "Inbox", "Downloads", "Trash"] {
        let _ = std::fs::create_dir_all(paths.root.join(sub));
    }
    if let Some(dir) = paths.prefs.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let library = match nikgob_library::Library::open(&paths.db) {
        Ok(l) => l,
        Err(e) => {
            log::error!("cannot open library at {}: {e}", paths.db.display());
            std::process::exit(1);
        }
    };
    if demo && data_dir_asked.is_none() {
        demo::seed(&library, &paths.root.join("Music"));
    }
    stage("library");
    let settings = paths.settings.as_ref().and_then(|s| nikgob_core::settings::Settings::load(s).ok());
    let mut prefs = load_prefs(&paths, settings.as_ref(), devices);
    stage("prefs");
    // The system says whether autostart is on (Task Manager may have turned
    // it off), and a wished-for line that is gone is written again; demo
    // runs leave it alone.
    let autostart_system = (!demo).then(|| autostart::reconcile(prefs.autostart));
    if let Some(on) = autostart_system {
        prefs.autostart = on;
    }
    let start_hidden = at_sign_in && !demo && prefs.autostart_hidden;

    // Streaming services: each keeps its state in its own folder of the
    // data dir (the demo's own, in demo mode).
    let settings = settings.unwrap_or_default();
    let secrets = settings.streaming.clone();
    let data_dir = paths.db.parent().map_or_else(|| PathBuf::from("."), std::path::Path::to_path_buf);
    let streams = if demo {
        streams::demo(&data_dir, &secrets)
    } else {
        streams::real(&data_dir, &secrets)
    };
    let relogin = streams::relogin(&streams, &secrets);
    stage("streams");
    // Covers of the services, kept on disk between starts.
    let covers_dir = data_dir.join("cache").join("covers");

    // Audio: the real engine unless disabled.
    let (backend, links): (Box<dyn Playback>, _) = if std::env::var_os("NIKGOB_NO_AUDIO").is_some() {
        (Box::new(NullPlayback), None)
    } else {
        let device = prefs
            .output_device
            .clone()
            .filter(|d| prefs.output_devices.contains(d));
        match audio::Engine::start(device, streams.clone()) {
            Ok((engine, links)) => (Box::new(engine), Some(links)),
            Err(e) => {
                log::error!("audio engine failed to start: {e}");
                (Box::new(NullPlayback), None)
            }
        }
    };
    stage("audio");

    let wake = {
        let w = waker.clone();
        move || w.wake()
    };
    let import = {
        let mut cfg = nikgob_runtime::ImportConfig::new(
            prefs.library_dir.clone(),
            prefs.inbox_dir.clone(),
            paths.root.join("Downloads"),
            paths.root.join("Trash"),
        );
        cfg.masters_dir = prefs.masters_dir.clone();
        cfg.opus_bitrate_kbps = prefs.opus_bitrate;
        cfg.check_fake_lossless = prefs.check_fake_lossless;
        cfg.fallback_seed_argb = prefs.fallback_seed;
        // A demo with the real services asks the real online metadata too
        // (covers, lyrics, the graph of similar music).
        let offline = (demo && !streams::real_in_demo()) || std::env::var_os("NIKGOB_OFFLINE").is_some();
        cfg.fetch_lyrics = !offline;
        cfg.fetch_cover_online = !offline;
        let mut meta = nikgob_runtime::MetadataConfig::new(settings.metadata.contact);
        if let Some(k) = settings.metadata.acoustid_key.filter(|k| !k.trim().is_empty()) {
            meta = meta.with_acoustid_key(k);
        }
        if let Some(k) = settings.metadata.lastfm_key {
            meta = meta.with_lastfm_key(k);
        }
        nikgob_runtime::ImportSetup {
            config: cfg,
            metadata: (!offline).then_some(meta),
            watch_inbox: !demo,
            torrent_state_dir: paths.db.with_file_name("torrents"),
            torrent_offline: demo,
            models_dir: Some(paths.db.with_file_name("models")),
        }
    };
    // The demo's discovery and the services' suggestions ask the fake
    // services quickly, so the home's «Новое для тебя» and the suggestions
    // line of «Источники» are there within seconds for screenshots.
    let schedule = if demo && !streams::real_in_demo() {
        StreamSchedule {
            discover_gap: (Duration::from_millis(100), Duration::from_millis(200)),
            graph_gap_youtube: (Duration::from_millis(100), Duration::from_millis(200)),
            graph_gap_deezer: (Duration::from_millis(100), Duration::from_millis(200)),
            suggest_first: Duration::from_secs(2),
            ..StreamSchedule::default()
        }
    } else {
        StreamSchedule::default()
    };
    let runtime = Runtime::start_scheduled(
        RuntimeConfig {
            library,
            library_dir: prefs.library_dir.clone(),
            prefs,
            prefs_path: Some(paths.prefs.clone()),
            settings_path: paths.settings.clone(),
            import: Some(import),
            streams,
        },
        backend,
        Arc::new(wake.clone()),
        schedule,
    );
    if let Some(l) = &links {
        l.connect(runtime.playback_sender());
    }
    // «Войти через Google»: the sign-in window, its profile in youtube/webview.
    if let Some(login) = ytlogin::WindowLogin::new(&data_dir.join("youtube")) {
        runtime.set_web_login(nikgob_core::streaming::Service::Youtube, Arc::new(login));
    }
    if let Some(c) = relogin {
        runtime.send(c);
    }
    stage("runtime");
    let core = Core {
        ui: Md3eUi::new(),
        runtime,
        desktop: desktop::Desktop::start(wake),
        analyzer: links.map(|l| l.analyzer),
        commands: Vec::new(),
        shot: shot::Shot::from_env(),
        frames: frames::Frames::from_env(),
        quit: false,
        reopen: false,
        wants_show: false,
        hidden: false,
        show_at: None,
        last_frame: Instant::now(),
        pacer: pacing::Pacer::default(),
        started: Instant::now(),
        // Screenshot runs get their own size and leave the real one alone.
        copied: 0,
        window: None,
        autostart: autostart::Keeper::new(autostart_system),
        show_asked,
        geometry: geometry::Keeper::load(
            shot::window_size().is_none().then(|| paths.prefs.with_file_name("window.json")),
        ),
    };

    stage("desktop");
    Shell::new(core, &waker).start_hidden(start_hidden).run(|lease| {
        let covers_dir = covers_dir.clone();
        let mut viewport = lease.peek(|core| core.geometry.apply(egui::ViewportBuilder::default()));
        if shot::offscreen() {
            viewport = viewport.with_position(shot::OFFSCREEN).with_active(false).with_taskbar(false);
        }
        stage("shell");
        eframe::run_native(
            "music.nikgob",
            eframe::NativeOptions {
                viewport: viewport
                    .with_title("music.nikgob")
                    .with_app_id("music.nikgob")
                    .with_icon(egui::IconData {
                        rgba: desktop::icon_rgba(64),
                        width: 64,
                        height: 64,
                    })
                    .with_min_inner_size(geometry::MIN_SIZE),
                ..Default::default()
            },
            Box::new(move |cc| {
                stage("window");
                let ctx = cc.egui_ctx.clone();
                let mut fonts = egui::FontDefinitions::default();
                nikgob_md3e::typography::install_fonts(&mut fonts);
                add_system_fallbacks(&mut fonts);
                fastframe_text::detect().apply_to(&mut fonts);
                ctx.set_fonts(fonts);
                stage("fonts");
                nikgob_md3e::icons::install(&ctx);
                web_images::WebImages::install(&ctx, covers_dir);
                stage("web covers");
                nikgob_md3e::Theme::new(nikgob_md3e::ColorScheme::baseline(true), true).install(&ctx);
                let mut core = lease.take(&ctx);
                core.reopen = false;
                core.desktop.attach();
                Ok(Box::new(Window {
                    core,
                    scrolling: fastframe_scroll::Scrolling::default(),
                    recovery_checked: false,
                    motion_checked: None,
                    titlebar: titlebar::TitleBar::default(),
                }))
            }),
        )
    })
}

#[cfg(test)]
mod prefs_tests {
    use super::*;
    use nikgob_view::NpSlot;

    #[test]
    fn migrates_legacy_slots_and_preserves_explicit_new_slots() {
        for (json, slot) in [
            (r#"{}"#, NpSlot::None),
            (r#"{"lyrics_mega":true}"#, NpSlot::Lyrics),
            (r#"{"np_pane":true}"#, NpSlot::Queue),
            (r#"{"lyrics_mega":true,"np_pane":true,"old_unknown":42}"#, NpSlot::Lyrics),
            (r#"{"lyrics_mega":false,"np_pane":false}"#, NpSlot::None),
            (r#"{"np_slot":"None","lyrics_mega":true}"#, NpSlot::None),
            (r#"{"np_slot":"Info","np_pane":true}"#, NpSlot::Info),
        ] {
            assert_eq!(parse_prefs(json.as_bytes()).unwrap().np_slot, slot);
        }
        for slot in [NpSlot::None, NpSlot::Lyrics, NpSlot::Queue, NpSlot::Info] {
            let prefs = Prefs { np_slot: slot, ..Prefs::default() };
            let json = serde_json::to_vec(&prefs).unwrap();
            assert_eq!(parse_prefs(&json).unwrap(), prefs);
            let value: serde_json::Value = serde_json::from_slice(&json).unwrap();
            assert!(value.get("lyrics_mega").is_none());
            assert!(value.get("np_pane").is_none());
        }
    }

    /// Prefs saved by a build with the old lyrics defaults (word timing for
    /// every song on, no line timing of plain texts) get the new defaults
    /// once; prefs saved since keep what the user chose.
    #[test]
    fn old_lyrics_defaults_move_to_the_new_ones_once() {
        let old = r#"{"lyrics_check":true,"lyrics_words_all":true,"lyrics_recognize":true,"lyrics_gpu":false}"#;
        let p = parse_prefs(old.as_bytes()).unwrap();
        assert!(!p.lyrics_words_all && !p.lyrics_words_playing && p.lyrics_lines_auto && !p.lyrics_recognize);
        assert_eq!(p.lyrics_rev, nikgob_view::LYRICS_REV);
        // The control under the lyrics opens its menu until «Вид» says else.
        assert_eq!(p.lyrics_ui, 0);
        assert_eq!(parse_prefs(br#"{"lyrics_ui":2,"lyrics_rev":1}"#).unwrap().lyrics_ui, 2);
        // Everything else stays as it was.
        assert!(p.lyrics_check && !p.lyrics_gpu);
        // Saved again, the user's own choices hold.
        let mine = Prefs { lyrics_words_all: true, lyrics_words_playing: true, lyrics_lines_auto: false, lyrics_recognize: true, ..p };
        let again = parse_prefs(&serde_json::to_vec(&mine).unwrap()).unwrap();
        assert_eq!(again, mine);
        // A first start has the new defaults too.
        let d = prefs_of(None);
        assert!(!d.lyrics_words_all && !d.lyrics_words_playing && d.lyrics_lines_auto && !d.lyrics_recognize);
    }

    /// Главная asks for the name on the first start only: not with a prefs
    /// file from before, not once «Готово» or «Пропустить» was said.
    #[test]
    fn the_name_is_asked_on_the_first_start_only() {
        use nikgob_view::PrefPatch;
        let mut first = prefs_of(None);
        assert!(first.name_prompt && first.user_name.is_empty());
        assert!(!prefs_of(Some(br#"{"theme":"Dark"}"#)).name_prompt, "a prefs file from before");
        let mut skipped = first.clone();
        PrefPatch::NamePrompt(false).apply(&mut skipped);
        let kept = serde_json::to_vec(&skipped).unwrap();
        assert!(!prefs_of(Some(&kept)).name_prompt, "skipped: never again");
        PrefPatch::UserName("  Ник  ".into()).apply(&mut first);
        let kept = prefs_of(Some(&serde_json::to_vec(&first).unwrap()));
        assert_eq!((kept.user_name.as_str(), kept.name_prompt), ("Ник", false));
    }
}
