//! music.nikgob: the desktop player.
//!
//! Opens the library, starts the audio engine and the runtime, and draws
//! the MD3E interface. Closing the window hides the app to the tray while
//! music keeps playing; the window (and its textures) is made again on show.

#![windows_subsystem = "windows"]

mod audio;
mod cover_cache;
mod demo;
mod desktop;
mod geometry;
mod logfile;
mod shot;
mod shot_fakes;
mod streams;
mod sysmotion;
mod titlebar;
mod web_images;
mod ytlogin;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fastframe_shell::{Closed, Headless, Resident, Shell, Waker};
use nikgob_md3e::screens::Md3eUi;
use nikgob_runtime::{NullPlayback, Playback, Runtime, RuntimeConfig};
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
    let mut prefs: Prefs = serde_json::from_value(value)?;
    if legacy { prefs.np_slot = slot; }
    Ok(prefs)
}

fn load_prefs(paths: &Paths) -> Prefs {
    let mut p: Prefs = std::fs::read(&paths.prefs)
        .ok()
        .and_then(|b| parse_prefs(&b).ok())
        .unwrap_or_default();
    if p.library_dir.as_os_str().is_empty() {
        p.library_dir = paths.root.join("Music");
    }
    if p.inbox_dir.as_os_str().is_empty() {
        p.inbox_dir = paths.root.join("Inbox");
    }
    if p.masters_dir.as_os_str().is_empty() {
        p.masters_dir = p.library_dir.join(".musiclib").join("masters");
    }
    if let Some(s) = paths
        .settings
        .as_ref()
        .and_then(|s| nikgob_core::settings::Settings::load(s).ok())
    {
        if let Some(pr) = s.indexer.prowlarr {
            p.prowlarr_url = pr.url;
            p.prowlarr_configured = !pr.api_key.is_empty();
        }
        p.metadata_contact = s.metadata.contact;
        p.acoustid_configured = s.metadata.acoustid_key.is_some_and(|k| !k.trim().is_empty());
        p.spotify_personal_app = s.streaming.spotify_client_id.is_some();
        p.youtube_cookies_set = s.streaming.youtube_cookies.is_some();
    }
    p.output_devices = nikgob_audio::output_device_names().unwrap_or_default();
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
    quit: bool,
    wants_show: bool,
    last_frame: Instant,
    /// One clock for every window: each new window's egui starts its time
    /// at zero, and animations started in the old one would sit in the
    /// future (the page stayed blank until enough clicks repainted it).
    started: Instant,
    geometry: geometry::Keeper,
    /// The last [`nikgob_view::ClipboardRequest`] copied.
    copied: u64,
    /// The window state the runtime last heard (visible, focused).
    window: Option<(bool, bool)>,
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
            let state = self.runtime.state();
            self.desktop.poll(&state, &mut self.commands)
        };
        self.quit |= req.quit;
        self.wants_show |= req.show || req.toggle;
        self.runtime.send_all(self.commands.drain(..));
    }
}

impl Resident for Core {
    fn closed(&self) -> Closed {
        if !self.quit && self.desktop.can_hide() {
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
        if let Some(s) = self.core.shot.as_mut() {
            s.raw_input(raw_input);
        }
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if !std::mem::replace(&mut self.recovery_checked, true) {
            fastframe_shell::window::recover_offscreen(ctx, frame);
        }
        if self.motion_checked.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) {
            self.motion_checked = Some(Instant::now());
            self.core.ui.set_system_reduced_motion(sysmotion::reduced());
        }
        let core = &mut *self.core;
        core.geometry.observe(ctx);
        let (minimized, focused) = ctx.input(|i| {
            let v = i.viewport();
            (v.minimized.unwrap_or(false), v.focused.unwrap_or(true))
        });
        core.report_window(!minimized, focused && !minimized);
        core.poll_desktop();
        if core.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if std::mem::take(&mut core.wants_show) {
            // Tray "show/hide" while the window exists: bring it forward.
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.scrolling.apply(ui.ctx());
        let core = &mut *self.core;
        if core.ui.wants_levels()
            && let Some(slot) = &core.analyzer
            && let Some(a) = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_mut()
        {
            a.update();
            core.ui.set_levels(a.bass_energy());
            core.ui.set_bands(a.bands());
        }
        let fps = {
            let state = core.runtime.state();
            // Screenshot runs may lay states the demo cannot reach over it.
            let faked = core.shot.as_ref().and_then(|s| s.overlay(&state));
            core.ui.frame(ui, faked.as_ref().unwrap_or(&*state), &mut core.commands);
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
            }
            state.prefs.fps_limit.max(15)
        };
        core.runtime.send_all(core.commands.drain(..));

        // Frame limit: only matters while something animates continuously.
        let budget = Duration::from_secs_f64(1.0 / f64::from(fps));
        let spent = core.last_frame.elapsed();
        if spent < budget && ui.ctx().has_requested_repaint() {
            std::thread::sleep(budget - spent);
        }
        core.last_frame = Instant::now();
    }
}

/// Installed fonts for the scripts Roboto Flex does not draw (CJK, Arabic,
/// ...), behind every family, so Japanese titles are not boxes.
fn add_system_fallbacks(fonts: &mut egui::FontDefinitions) {
    for f in fastframe_fonts::system::fallbacks() {
        let mut data = egui::FontData::from_static(&f.bytes);
        data.index = f.index;
        data.tweak.scale = f.scale;
        data.tweak.y_offset_factor = f.y_offset_factor;
        fonts.font_data.insert(f.name.clone(), Arc::new(data));
        for family in fonts.families.values_mut() {
            family.push(f.name.clone());
        }
    }
}

fn main() -> eframe::Result {
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
    let mut paths = paths();
    if let Some(dir) = paths.db.parent() {
        log_panics(dir.to_owned());
    }
    let demo = std::env::var_os("NIKGOB_DEMO").is_some();
    if demo {
        let dir = std::env::temp_dir().join("nikgob-demo");
        // A demo library from an older seed (duplicate artists, old covers)
        // is thrown away and seeded again.
        let marker = dir.join(demo::VERSION);
        if !marker.exists() {
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
    if demo {
        demo::seed(&library, &paths.root.join("Music"));
    }
    let prefs = load_prefs(&paths);
    let waker = Waker::default();

    // Streaming services: each keeps its state in its own folder of the
    // data dir (the demo's own, in demo mode).
    let secrets = paths
        .settings
        .as_ref()
        .and_then(|s| nikgob_core::settings::Settings::load(s).ok())
        .unwrap_or_default()
        .streaming;
    let data_dir = paths.db.parent().map_or_else(|| PathBuf::from("."), std::path::Path::to_path_buf);
    let streams = if demo {
        streams::demo(&data_dir, &secrets)
    } else {
        streams::real(&data_dir, &secrets)
    };
    let relogin = streams::relogin(&streams, &secrets);
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
        let settings = paths
            .settings
            .as_ref()
            .and_then(|s| nikgob_core::settings::Settings::load(s).ok())
            .unwrap_or_default();
        let offline = demo || std::env::var_os("NIKGOB_OFFLINE").is_some();
        cfg.fetch_lyrics = !offline;
        cfg.fetch_cover_online = !offline;
        let mut meta = nikgob_runtime::MetadataConfig::new(settings.metadata.contact);
        if let Some(k) = settings.metadata.acoustid_key.filter(|k| !k.trim().is_empty()) {
            meta = meta.with_acoustid_key(k);
        }
        nikgob_runtime::ImportSetup {
            config: cfg,
            metadata: (!offline).then_some(meta),
            watch_inbox: !demo,
            torrent_state_dir: paths.db.with_file_name("torrents"),
            torrent_offline: demo,
        }
    };
    let runtime = Runtime::start(
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
    let core = Core {
        ui: Md3eUi::new(),
        runtime,
        desktop: desktop::Desktop::start(wake),
        analyzer: links.map(|l| l.analyzer),
        commands: Vec::new(),
        shot: shot::Shot::from_env(),
        quit: false,
        wants_show: false,
        last_frame: Instant::now(),
        started: Instant::now(),
        // Screenshot runs get their own size and leave the real one alone.
        copied: 0,
        window: None,
        geometry: geometry::Keeper::load(
            shot::window_size().is_none().then(|| paths.prefs.with_file_name("window.json")),
        ),
    };

    Shell::new(core, &waker).run(|lease| {
        let covers_dir = covers_dir.clone();
        eframe::run_native(
            "music.nikgob",
            eframe::NativeOptions {
                viewport: lease.peek(|core| core.geometry.apply(egui::ViewportBuilder::default()))
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
                let ctx = cc.egui_ctx.clone();
                let mut fonts = egui::FontDefinitions::default();
                nikgob_md3e::typography::install_fonts(&mut fonts);
                add_system_fallbacks(&mut fonts);
                fastframe_text::detect().apply_to(&mut fonts);
                ctx.set_fonts(fonts);
                nikgob_md3e::icons::install(&ctx);
                web_images::WebImages::install(&ctx, covers_dir);
                nikgob_md3e::Theme::new(nikgob_md3e::ColorScheme::baseline(true), true).install(&ctx);
                let mut core = lease.take(&ctx);
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
}
