//! music.nikgob: the desktop player.
//!
//! Opens the library, starts the audio engine and the runtime, and draws
//! the MD3E interface. Closing the window hides the app to the tray while
//! music keeps playing; the window (and its textures) is made again on show.

#![windows_subsystem = "windows"]

mod audio;
mod demo;
mod desktop;
mod shot;
mod sysmotion;

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

fn load_prefs(paths: &Paths) -> Prefs {
    let mut p: Prefs = std::fs::read(&paths.prefs)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    if p.library_dir.as_os_str().is_empty() {
        p.library_dir = paths.root.join("Music");
    }
    if p.inbox_dir.as_os_str().is_empty() {
        p.inbox_dir = paths.root.join("Inbox");
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
}

impl Core {
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
        self.runtime.send(Command::Pause);
    }
}

struct Window {
    core: fastframe_shell::Held<Core>,
    scrolling: fastframe_scroll::Scrolling,
    recovery_checked: bool,
    /// When the system animation setting was last read.
    motion_checked: Option<Instant>,
}

impl eframe::App for Window {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
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

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
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
            core.ui.frame(ui, &state, &mut core.commands);
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

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let mut paths = paths();
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
        paths.settings = None;
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

    // Audio: the real engine unless disabled.
    let (backend, links): (Box<dyn Playback>, _) = if std::env::var_os("NIKGOB_NO_AUDIO").is_some() {
        (Box::new(NullPlayback), None)
    } else {
        let device = prefs
            .output_device
            .clone()
            .filter(|d| prefs.output_devices.contains(d));
        match audio::Engine::start(device) {
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
        },
        backend,
        Arc::new(wake.clone()),
    );
    if let Some(l) = &links {
        l.connect(runtime.playback_sender());
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
    };

    Shell::new(core, &waker).run(|lease| {
        eframe::run_native(
            "music.nikgob",
            eframe::NativeOptions {
                viewport: egui::ViewportBuilder::default()
                    .with_title("music.nikgob")
                    .with_app_id("music.nikgob")
                    .with_icon(egui::IconData {
                        rgba: desktop::icon_rgba(64),
                        width: 64,
                        height: 64,
                    })
                    .with_inner_size([1280.0, 800.0])
                    .with_min_inner_size([360.0, 560.0]),
                ..Default::default()
            },
            Box::new(move |cc| {
                let ctx = cc.egui_ctx.clone();
                let mut fonts = egui::FontDefinitions::default();
                nikgob_md3e::typography::install_fonts(&mut fonts);
                fastframe_text::detect().apply_to(&mut fonts);
                ctx.set_fonts(fonts);
                nikgob_md3e::icons::install(&ctx);
                nikgob_md3e::Theme::new(nikgob_md3e::ColorScheme::baseline(true), true).install(&ctx);
                let mut core = lease.take(&ctx);
                core.desktop.attach();
                Ok(Box::new(Window {
                    core,
                    scrolling: fastframe_scroll::Scrolling::default(),
                    recovery_checked: false,
                    motion_checked: None,
                }))
            }),
        )
    })
}
