//! music.nikgob: the desktop player.
//!
//! Opens the library, starts the audio engine and the runtime, and draws
//! the MD3E interface. Closing the window hides the app to the tray while
//! music keeps playing: the window only hides and comes back at once
//! («Быстрое открытие из трея»), or, with that off, it is destroyed and
//! made again (with its textures) on show.

#![windows_subsystem = "windows"]

mod autostart;
mod cover_cache;
#[cfg(any(feature = "dev", test))]
mod demo;
mod desktop;
mod icon_art;
mod fallback_fonts;
#[cfg(feature = "dev")]
mod frames;
mod geometry;
mod logfile;
mod memory;
mod pacing;
#[cfg(feature = "dev")]
mod profile;
mod quit;
#[cfg(feature = "dev")]
mod shot;
#[cfg(feature = "dev")]
mod shot_fakes;
mod streams;
#[cfg(feature = "dev")]
mod sweep;
mod sysmotion;
mod titlebar;
mod scwidget;
mod khpage;
mod web_images;
mod windowing;
mod ytlogin;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use fastframe_shell::{Closed, Headless, Resident, Shell, Waker};
use nikgob_md3e::screens::Md3eUi;
use nikgob_runtime::{NullPlayback, Playback, Runtime, RuntimeConfig, StreamSchedule};
use nikgob_view::{Command, Prefs};

/// Where things live on disk.
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
/// are appended to `crash.log` next to the library, scrubbed as the log is
/// ([`logfile::scrub`]), then handled as usual.
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
            .open(dir.join(logfile::CRASH))
        {
            let _ = f.write_all(logfile::scrub(&report).as_bytes());
        }
        log::error!("{info}");
        default(info);
    }));
}

/// The prefs of a prefs file's bytes; without a file (the first start)
/// those of [`Prefs::first_start`].
fn prefs_of(bytes: Option<&[u8]>) -> Prefs {
    match bytes {
        Some(b) => parse_prefs(b).unwrap_or_default(),
        None => Prefs::first_start(),
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
    // Prefs from before the presets keep the values of their own once.
    let settings_rev = value.get("settings_rev").and_then(|v| v.as_u64()).map_or(0, |r| r.min(u64::from(u8::MAX)) as u8);
    // «Перемешивание остаётся включённым» became a rule by the kind of
    // list: off, every kind keeps playing in order.
    let keep_shuffle = value.get("list_shuffle").is_none().then(|| value.get("keep_shuffle").and_then(|v| v.as_bool())).flatten();
    // Own playlists started shuffled by default and fought the button
    // (shuffle turned off came back on at «Любимые»): that old default
    // becomes «По кнопке» once; a rule the user chose since stays.
    let shuffle_rev = value.get("shuffle_rev").and_then(|v| v.as_u64()).map_or(0, |r| r.min(u64::from(u8::MAX)) as u8);
    // «Альбом показывает» was «Автоматически» by default (an album not
    // added opened on the user's songs): that old default becomes «Всегда
    // весь альбом» once; a mode the user chose since stays.
    let album_rev = value.get("album_rev").and_then(|v| v.as_u64()).map_or(0, |r| r.min(u64::from(u8::MAX)) as u8);
    // «Только на процессоре» became «Процессор» of «На чём считать анализ
    // звука».
    let cpu_only = value.get("sound_device").is_none() && value.get("sound_cpu_only").and_then(|v| v.as_bool()) == Some(true);
    // A field this build cannot read takes its default, not the whole file.
    let mut prefs = Prefs::from_json_lenient(value)?;
    if legacy { prefs.np_slot = slot; }
    if let Some(keep) = keep_shuffle {
        prefs.list_shuffle = nikgob_view::ListShuffles::from_keep(keep);
    }
    if shuffle_rev < 1 && prefs.list_shuffle.own_playlists == nikgob_view::ListShuffle::Shuffled {
        prefs.list_shuffle.own_playlists = nikgob_view::ListShuffle::Button;
    }
    prefs.shuffle_rev = nikgob_view::SHUFFLE_REV;
    if album_rev < 1 && prefs.album_mode == nikgob_view::AlbumMode::Auto {
        prefs.album_mode = nikgob_view::AlbumMode::Whole;
    }
    prefs.album_rev = nikgob_view::ALBUM_REV;
    if cpu_only {
        prefs.sound_device = nikgob_view::SoundDevice::Cpu;
    }
    prefs.upgrade_lyrics(lyrics_rev);
    // A developer build opens the settings in «Подробно» the first time;
    // a normal/release build in «Просто».
    nikgob_view::presets::upgrade(&mut prefs, settings_rev, nikgob_md3e::screens::brand::own_build());
    Ok(prefs)
}

/// The first run shows at this start ([`nikgob_view::first_run::due`]):
/// pending in the prefs (a new install), and either already on screen
/// before (the app quit midway) or nothing says the player was used: no
/// music or plays in the library, no service signed in, no YouTube Music
/// cookies or Spotify key in settings.toml (its sign-in comes back from
/// them only after this). `NIKGOB_FIRST_RUN=1` shows it anyway in a demo
/// run or the developer profile (`beside`), never in the real player; there
/// a first run left midway does not come back by itself, so a screenshot's
/// never leaks into the next run.
fn first_run_at_start(
    prefs: &Prefs,
    library: &nikgob_library::Library,
    streams: &streams::Services,
    secrets: &nikgob_core::settings::StreamingSettings,
    beside: bool,
) -> bool {
    if beside && std::env::var_os("NIKGOB_FIRST_RUN").is_some() {
        return true;
    }
    if !prefs.first_run {
        return false;
    }
    let started = prefs.first_run_started && !beside;
    let tracks = library.count_tracks(nikgob_library::TrackFilter::All).unwrap_or(0);
    let events = library.count_events().unwrap_or(0);
    let signed_in = streams.iter().any(|s| s.account().is_logged_in());
    let stored = secrets.youtube_cookies.as_deref().is_some_and(|c| !c.trim().is_empty())
        || secrets.spotify_client_id.as_deref().is_some_and(|c| !c.trim().is_empty());
    let used = tracks > 0 || events > 0 || signed_in || stored;
    let due = nikgob_view::first_run::due(true, started, used);
    if !due {
        log::info!("first run: not shown, the player was used before ({tracks} tracks, {events} plays, signed in: {signed_in}, stored sign-in: {stored})");
    }
    due
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
        p.listenbrainz_configured = s.metadata.listenbrainz_token.as_ref().is_some_and(|k| !k.trim().is_empty());
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
    /// The owner's devices (before the API, whose routes its tunnel uses:
    /// it stops first).
    mesh: Option<nikgob_mesh::MeshHandle>,
    /// The local REST API (before the runtime: it stops first).
    api: Option<nikgob_api::ApiHandle>,
    runtime: Runtime,
    desktop: desktop::Desktop,
    analyzer: Option<nikgob_playback::AnalyzerSlot>,
    commands: Vec<Command>,
    #[cfg(feature = "dev")]
    shot: Option<shot::Shot>,
    /// `NIKGOB_FRAMES`: the frame-time log.
    #[cfg(feature = "dev")]
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
    /// A `--quit` launch asked the player to quit, as «Выход» does.
    quit_asked: Arc<AtomicBool>,
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
        self.quit |= req.quit || self.quit_asked.swap(false, Ordering::Relaxed);
        self.wants_show |= req.show || req.toggle || self.show_asked.swap(false, Ordering::Relaxed);
        self.sync_autostart();
        send_commands(&self.runtime, &mut self.commands);
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

/// Hands the commands to the runtime; the developer profile's own
/// («Открыть», «Сбросить») the app does itself.
fn send_commands(runtime: &Runtime, commands: &mut Vec<Command>) {
    commands.retain(|c| match c {
        #[cfg(feature = "dev")]
        Command::DevProfile(action) => {
            profile::act(*action);
            false
        }
        #[cfg(not(feature = "dev"))]
        Command::DevProfile(_) => false,
        _ => true,
    });
    runtime.send_all(commands.drain(..));
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
        memory::log_now("tray");
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
        memory::log_now("exit");
        self.geometry.flush();
        // Before the runtime: the devices still being forgotten are passed
        // to the mesh and revoked, on disk, before it stops.
        if let Some(mesh) = self.mesh.take() {
            mesh.stop();
        }
        if let Some(api) = self.api.take() {
            api.stop();
        }
        self.runtime.send(Command::Pause);
        // Soloist would also go with the process (its job object); stopping
        // it here lets it leave Spotify Connect cleanly.
        streams::shutdown();
    }
}

/// The largest texture side the app asks of egui (the font atlas's width).
const MAX_TEXTURE_SIDE: usize = 8192;

/// How long a window brought back from the tray asks for frames until one
/// is drawn.
const SHOW_TRIES: Duration = Duration::from_secs(1);

struct Window {
    core: fastframe_shell::Held<Core>,
    scrolling: fastframe_scroll::Scrolling,
    recovery_checked: bool,
    /// When the system animation setting was last read.
    motion_checked: Option<Instant>,
    titlebar: titlebar::TitleBar,
    /// Whether this window can hide in the tray and come back
    /// ([`windowing::can_hide`]: not on Wayland), asked on its first frame.
    hides: Option<bool>,
    /// The window hid in this pass: its frame is still drawn whole, so it
    /// shows that, not an empty one, when it comes back.
    hiding: bool,
    /// When the window was asked back from the tray, until it has drawn.
    showing: Option<Instant>,
    /// Windows' cloaking (another virtual desktop).
    cloak: windowing::Cloak,
}

impl eframe::App for Window {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        let wall = self.core.started.elapsed().as_secs_f64();
        #[cfg(feature = "dev")]
        let wall = self.core.shot.as_mut().and_then(|s| s.synthetic_time(wall)).unwrap_or(wall);
        raw_input.time = Some(wall);
        // egui makes the font atlas as wide as the largest texture the GPU
        // takes (16384) and lets it grow as tall before starting over: up to
        // a gigabyte in memory, and on the GPU again. Nothing here needs
        // textures that large (covers stop at 4096).
        raw_input.max_texture_side = Some(raw_input.max_texture_side.unwrap_or(MAX_TEXTURE_SIDE).min(MAX_TEXTURE_SIDE));
        #[cfg(feature = "dev")]
        if let Some(s) = self.core.shot.as_mut() {
            s.raw_input(raw_input);
        }
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.core.pacer.begin();
        #[cfg(feature = "dev")]
        if let Some(f) = self.core.frames.as_mut() {
            f.begin();
            f.caused_by(ctx);
        }
        // A screenshot run stays off the screen on purpose.
        let offscreen = {
            #[cfg(feature = "dev")]
            {
                shot::offscreen()
            }
            #[cfg(not(feature = "dev"))]
            {
                false
            }
        };
        if !std::mem::replace(&mut self.recovery_checked, true) && !offscreen {
            fastframe_shell::window::recover_offscreen(ctx, frame);
        }
        if self.motion_checked.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) {
            self.motion_checked = Some(Instant::now());
            self.core.ui.set_system_reduced_motion(sysmotion::reduced());
        }
        let hides = match self.hides {
            Some(h) => h,
            None => {
                let h = windowing::can_hide(frame);
                self.hides = Some(h);
                self.core.ui.set_window_can_hide(h);
                h
            }
        };
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
        // and only when something wakes it: the tray, the runtime. Where a
        // window cannot hide (Wayland) it closes and is made again on show.
        if close && !core.quit && !core.reopen && hides && core.desktop.can_hide() && core.runtime.state().prefs.tray_keep_window {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            core.hidden = true;
            self.hiding = true;
            self.showing = None;
            core.geometry.flush();
            log::info!("window hidden to the tray (kept)");
            // Hidden, only the playing cover stays on the card.
            nikgob_md3e::covers::trim_to_current(ctx);
            memory::log_now("tray");
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
                // Drawn with what it had (no rebuild), once it is shown.
                self.showing = Some(Instant::now());
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        // Frames until the window brought back has drawn one: `Visible(true)`
        // takes effect only after this pass, X11 maps the window a moment
        // later still, and until then eframe runs only this logic (a few
        // times a second for a hidden window), so one request made before
        // the window is up could be spent on a pass that draws nothing.
        // A window that stays covered draws nothing either: asked for a
        // second at most.
        if let Some(at) = self.showing {
            if at.elapsed() < SHOW_TRIES {
                ctx.request_repaint_after(Duration::from_millis(16));
            } else {
                self.showing = None;
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // Kept in the tray: eframe still calls this for a hidden window
        // (about ten times a second while something asks for frames), so
        // nothing is drawn and nothing animates until it is shown again.
        // The pass that hides it still draws, so the window's last frame is
        // the interface and it comes back with that, not an empty frame.
        if self.core.hidden && !std::mem::take(&mut self.hiding) {
            return;
        }
        self.hiding = false;
        self.showing = None;
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
            #[cfg(feature = "dev")]
            let faked = core.shot.as_ref().and_then(|s| s.overlay(&state));
            #[cfg(not(feature = "dev"))]
            let faked: Option<nikgob_view::ViewState> = None;
            let draw = nikgob_md3e::prof::scope("frame");
            core.ui.frame(ui, faked.as_ref().unwrap_or(&*state), &mut core.commands);
            drop(draw);
            // The runtime asks for the clipboard (a share link); copied once.
            if let Some(c) = state.clipboard.as_ref().filter(|c| c.id != core.copied) {
                core.copied = c.id;
                ui.ctx().copy_text(c.text.clone());
            }
            self.titlebar.sync(ui.ctx(), frame, state.prefs.theme);
            #[cfg(feature = "dev")]
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
        send_commands(&core.runtime, &mut core.commands);
        static FIRST: AtomicBool = AtomicBool::new(true);
        if FIRST.swap(false, Ordering::Relaxed) {
            stage("first frame");
        }
        if let Some(t) = core.show_at.take() {
            log::info!("window: shown from the tray, first frame drawn in {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
        }
        #[cfg(feature = "dev")]
        if let Some(f) = core.frames.as_mut() {
            let atlas = ui.ctx().fonts(|f| f.font_image_size());
            f.end(frame.info().cpu_usage, core.shot.as_ref().map_or("live", |s| s.label()), atlas);
        }

        // Frame limit: only matters while something animates continuously.
        // A window Windows keeps out of sight (cloaked: on another virtual
        // desktop) draws a few frames a second; one in sight, focused or
        // not, keeps its full rate.
        let wait = if self.cloak.cloaked(frame) {
            core.pacer.idle();
            windowing::CLOAKED_FRAME
        } else {
            core.pacer.wait(frame, fps, core.last_frame.elapsed())
        };
        if !ui.ctx().has_requested_repaint() {
            core.pacer.idle();
        } else if !wait.is_zero() {
            std::thread::sleep(wait);
            #[cfg(feature = "dev")]
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
    // The allocator's options before anything else allocates much.
    memory::tune();
    LAUNCHED.get_or_init(Instant::now);
    if std::env::args().nth(1).is_some_and(|a| a == "--version" || a == "-V") {
        println!("music.nikgob {} (https://github.com/NikGob/music.nikgob)", env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }
    // `nikgob mcp`: only the MCP server on stdin/stdout for an agent on
    // this computer; it talks to the running player over its API. No
    // window, no lock, no log file; stdout is the protocol, logs go to
    // stderr. A windowed exe gets the pipes its client hands it.
    if std::env::args().nth(1).is_some_and(|a| a == "mcp") {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).target(env_logger::Target::Stderr).init();
        std::process::exit(nikgob_mcp::stdio::main());
    }
    // `--youtube-login` / `--soundcloud-login`: this run is only a sign-in
    // window; `--khinsider-page`: a hidden page read; `--sc-widget`: only
    // SoundCloud's player host. They log to stderr, which the app copies
    // into its own log; they must not write the app's log file themselves.
    if std::env::args().any(|a| a == "--youtube-login" || a == "--soundcloud-login") {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
        if let Some(code) = ytlogin::child_main() {
            std::process::exit(code);
        }
    }
    if std::env::args().any(|a| a == "--khinsider-page") {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
        if let Some(code) = khpage::child_main() {
            std::process::exit(code);
        }
    }
    if std::env::args().nth(1).is_some_and(|a| a == nikgob_sc_widget::protocol::FLAG) {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
        if let Some(code) = scwidget::child_main() {
            std::process::exit(code);
        }
    }
    // The developer profile (`--demo-profile`, `--features dev`): its own
    // folder, slot and library, beside the real player.
    #[cfg(feature = "dev")]
    let profile = profile::requested();
    #[cfg(not(feature = "dev"))]
    let profile = false;
    #[cfg(feature = "dev")]
    let slot = if profile { profile::slot() } else { fastframe_instance::Slot::new("music.nikgob") };
    #[cfg(not(feature = "dev"))]
    let slot = fastframe_instance::Slot::new("music.nikgob");
    // `--quit`: the running player (or profile) quits as from the tray's
    // «Выход» (its session saved) and this launch waits until it is gone,
    // so a script can replace the exe or restart it; nothing else starts.
    if quit::requested() {
        quit::run(&slot);
    }
    logfile::init();
    stage("log");
    memory::start();
    // `--sweep-list out.json`: the settings sweep's list of every row, from
    // the code and the prefs of `NIKGOB_DATA` (or the defaults); no window.
    #[cfg(feature = "dev")]
    if let Some(path) = sweep::list_path() {
        // The prefs the run itself starts from: the data dir's (the demo's
        // own dir with `NIKGOB_DEMO`), read as the app reads them — a dir
        // without a prefs.json starts as a first start, not as the
        // defaults.
        let dir = std::env::var_os("NIKGOB_DATA")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("NIKGOB_DEMO").map(|_| std::env::temp_dir().join("nikgob-demo")));
        let base = match dir {
            Some(d) => prefs_of(std::fs::read(d.join("prefs.json")).ok().as_deref()),
            None => Prefs::default(),
        };
        match sweep::write_list(&path, &base) {
            Ok(n) => {
                log::info!("sweep: {n} settings listed in {}", path.display());
                std::process::exit(0);
            }
            Err(e) => {
                log::error!("sweep: cannot write {}: {e}", path.display());
                eprintln!("cannot write {}: {e}", path.display());
                std::process::exit(1);
            }
        }
    }
    // Meanwhile, while the library opens and the window is made: the
    // fallback faces (the window's creator waits for them if need be) and
    // the audio outputs (the prefs wait for them).
    let _ = std::thread::Builder::new().name("fallback-fonts".into()).spawn(|| {
        fallback_fonts::faces();
    });
    let devices = std::thread::spawn(|| nikgob_audio::output_device_names().unwrap_or_default());
    #[allow(unused_mut)]
    let mut paths = paths();
    // `NIKGOB_DATA=dir`: a demo run on a library already in `dir` (a copy
    // of a real one, say) instead of the generated one.
    #[cfg(feature = "dev")]
    let data_dir_asked = std::env::var_os("NIKGOB_DATA").map(PathBuf::from).filter(|_| !profile);
    #[cfg(feature = "dev")]
    let demo = !profile && (std::env::var_os("NIKGOB_DEMO").is_some() || data_dir_asked.is_some());
    #[cfg(not(feature = "dev"))]
    let demo = false;
    // Demo runs and the profile live beside the real app: no autostart, no
    // start in the tray.
    let beside = demo || profile;
    #[cfg(feature = "dev")]
    let sweeping = sweep::active();
    #[cfg(not(feature = "dev"))]
    let sweeping = false;
    // A settings sweep writes the prefs of the library it runs on: never
    // the real profile's.
    #[cfg(feature = "dev")]
    if sweeping && !demo {
        log::error!("sweep: runs only on a copy (NIKGOB_DATA=dir) or the demo (NIKGOB_DEMO=1)");
        eprintln!("the settings sweep runs only with NIKGOB_DATA=<a copy> or NIKGOB_DEMO=1");
        std::process::exit(2);
    }
    let at_sign_in = std::env::args().any(|a| a == autostart::FLAG);
    // One player per user: a second launch (a click on the shortcut while
    // the first waits in the tray) shows the running one's window and ends.
    // Demo and screenshot runs live beside the real app; the profile has a
    // slot of its own.
    let show_asked = Arc::new(AtomicBool::new(false));
    let quit_asked = Arc::new(AtomicBool::new(false));
    let waker = Waker::default();
    let _instance = if demo {
        None
    } else {
        let (asked, quit) = (Arc::clone(&show_asked), Arc::clone(&quit_asked));
        let w = waker.clone();
        match slot.claim("show", move |request| {
            let flag = match request {
                "show" => &asked,
                "quit" => {
                    log::info!("another launch asked to quit (--quit)");
                    &quit
                }
                // «Сбросить» waits for the profile to be gone.
                "ping" => return Some("ok".to_owned()),
                _ => return None,
            };
            flag.store(true, Ordering::Relaxed);
            w.wake();
            Some("ok".to_owned())
        }) {
            fastframe_instance::Claim::First(guard) => Some(guard),
            other => {
                log::info!("music.nikgob is already running: {other:?}");
                return Ok(());
            }
        }
    };
    #[cfg(feature = "dev")]
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
    #[cfg(feature = "dev")]
    if profile {
        let dir = profile::dir();
        profile::prepare(&dir);
        paths = profile::paths(&dir);
        nikgob_view::dev::set_profile(true);
        nikgob_playback::play_missing_files_silent();
        log::info!("developer profile in {}", dir.display());
    }
    // A sweep on a copy of the profile plays its file-less tracks as the
    // profile does, so the screens show a track running.
    #[cfg(feature = "dev")]
    if sweeping {
        nikgob_playback::play_missing_files_silent();
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
    #[cfg(feature = "dev")]
    if demo && data_dir_asked.is_none() {
        demo::seed(&library, &paths.root.join("Music"));
    }
    #[cfg(feature = "dev")]
    if profile {
        profile::seed_if_empty(&library, &paths.root);
        // «Сбросить»: seeded, and nothing else.
        if profile::seed_only() {
            return Ok(());
        }
    }
    stage("library");
    let settings = paths.settings.as_ref().and_then(|s| nikgob_core::settings::Settings::load(s).ok());
    let mut prefs = load_prefs(&paths, settings.as_ref(), devices);
    nikgob_core::i18n::apply_pref(prefs.language);
    stage("prefs");
    // The system says whether autostart is on (Task Manager may have turned
    // it off), and a wished-for line that is gone is written again; demo
    // runs and the profile leave it alone.
    let autostart_system = (!beside).then(|| autostart::reconcile(prefs.autostart));
    if let Some(on) = autostart_system {
        prefs.autostart = on;
    }
    let start_hidden = at_sign_in && !beside && prefs.autostart_hidden;

    // Streaming services: each keeps its state in its own folder of the
    // data dir (the demo's own, in demo mode).
    let settings = settings.unwrap_or_default();
    let secrets = settings.streaming.clone();
    let data_dir = paths.db.parent().map_or_else(|| PathBuf::from("."), std::path::Path::to_path_buf);
    #[cfg(feature = "dev")]
    let streams = if demo {
        streams::demo(&data_dir, &secrets)
    } else {
        streams::real(&data_dir, &secrets)
    };
    #[cfg(not(feature = "dev"))]
    let streams = streams::real(&data_dir, &secrets);
    let relogin = streams::relogin(&streams, &secrets);
    stage("streams");
    prefs.first_run = first_run_at_start(&prefs, &library, &streams, &secrets, beside);
    // Covers of the services, kept on disk between starts.
    let covers_dir = data_dir.join("cache").join("covers");

    // Audio: the real engine unless disabled.
    let (backend, links): (Box<dyn Playback>, _) = if std::env::var_os("NIKGOB_NO_AUDIO").is_some() {
        (Box::new(NullPlayback), None)
    } else {
        // On Linux a device may also be named by its PulseAudio sink name,
        // which the list does not show: the engine finds it (or plays on
        // the default output while it is not there).
        let device = prefs
            .output_device
            .clone()
            .filter(|d| prefs.output_devices.contains(d) || cfg!(target_os = "linux"));
        let live = nikgob_playback::LivePlayers { soloist: streams::soloist(), widget: streams::sc_widget() };
        match nikgob_playback::Engine::start(device, streams.clone(), live) {
            Ok((engine, links)) => (Box::new(engine), Some(links)),
            Err(e) => {
                log::error!("audio engine failed to start: {e}");
                // Plays nothing and says so: the view's `no_audio`, the API's 503.
                (Box::new(nikgob_runtime::NoAudio), None)
            }
        }
    };
    stage("audio");

    let wake = {
        let w = waker.clone();
        move || w.wake()
    };
    // The local REST API is woken by the runtime too, with the window.
    let api_wake = nikgob_api::Wake::new();
    let runtime_wake = {
        let (w, a) = (wake.clone(), api_wake.clone());
        move || {
            w();
            a.wake();
        }
    };
    let api_library = library.clone();
    let mesh_library = library.clone();
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
        // A plain fake becomes an MP3 on its way in only when the user lets
        // that happen without asking.
        cfg.convert_fake_lossless = prefs.convert_fake_lossless && prefs.fake_flac_auto;
        cfg.fallback_seed_argb = prefs.fallback_seed;
        // A demo with the real services asks the real online metadata too
        // (covers, lyrics, the graph of similar music).
        let offline = (demo && !streams::real_in_demo()) || std::env::var_os("NIKGOB_OFFLINE").is_some() || sweeping;
        cfg.fetch_lyrics = !offline;
        cfg.fetch_cover_online = !offline;
        let mut meta = nikgob_runtime::MetadataConfig::new(settings.metadata.contact);
        if let Some(k) = settings.metadata.acoustid_key.filter(|k| !k.trim().is_empty()) {
            meta = meta.with_acoustid_key(k);
        }
        if let Some(k) = settings.metadata.lastfm_key {
            meta = meta.with_lastfm_key(k);
        }
        if let Some(t) = settings.metadata.listenbrainz_token {
            meta = meta.with_listenbrainz_token(t);
        }
        nikgob_runtime::ImportSetup {
            config: cfg,
            metadata: (!offline).then_some(meta),
            watch_inbox: !demo,
            torrent_state_dir: paths.db.with_file_name("torrents"),
            // The profile neither downloads nor seeds torrents.
            torrent_offline: demo || profile,
            // A sweep neither downloads nor runs the lyrics models.
            models_dir: (!sweeping).then(|| paths.db.with_file_name("models")),
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
    // `--sweep` / `--sweep-baseline`: the settings sweep drives the window.
    #[cfg(feature = "dev")]
    let sweep_shot = sweep::shot(&prefs);
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
        Arc::new(runtime_wake),
        schedule,
    );
    // It listens only once switched on in Settings → Межсеть; a settings
    // sweep never starts it.
    let api = if sweeping {
        None
    } else {
        let media_roots = vec![covers_dir.clone()];
        match nikgob_api::start(nikgob_api::ApiContext { runtime: runtime.handle(), library: api_library, wake: api_wake.clone(), media_roots }) {
            Ok(h) => Some(h),
            Err(e) => {
                log::error!("api: not started: {e}");
                None
            }
        }
    };
    // The owner's devices: on unless `[mesh] enabled = false`; never in a
    // settings sweep, nor in a demo unless NIKGOB_MESH is set (a demo would
    // show up on the home network as another device).
    let mesh = if sweeping || (demo && std::env::var_os("NIKGOB_MESH").is_none()) {
        None
    } else {
        let ctx = nikgob_mesh::MeshContext {
            data_dir: data_dir.clone(),
            device_name: nikgob_mesh::default_name(),
            kind: nikgob_view::MeshKind::Pc,
            runtime: runtime.handle(),
            api: api.as_ref().map(nikgob_api::ApiHandle::local),
            wake: api_wake.clone(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            headless: false,
            library: Some(mesh_library),
        };
        match nikgob_mesh::start(ctx) {
            Ok(h) => Some(h),
            Err(e) => {
                log::error!("mesh: not started: {e}");
                None
            }
        }
    };
    if let Some(l) = &links {
        l.connect(runtime.playback_sender());
    }
    // «Войти через Google»: the sign-in window, its profile in youtube/webview;
    // SoundCloud's in soundcloud/webview.
    if let Some(login) = ytlogin::WindowLogin::new(ytlogin::Site::Youtube, &data_dir.join("youtube")) {
        runtime.set_web_login(nikgob_core::streaming::Service::Youtube, Arc::new(login));
    }
    if let Some(login) = ytlogin::WindowLogin::new(ytlogin::Site::Soundcloud, &data_dir.join("soundcloud")) {
        runtime.set_web_login(nikgob_core::streaming::Service::Soundcloud, Arc::new(login));
    }
    // khinsider's check, when the HTTP client still sees it: a hidden
    // webview with its own profile. No WebView2 / WebKit: the source stays
    // on the HTTP answer.
    #[cfg(any(windows, target_os = "linux"))]
    if let Some(page) = khpage::HiddenPage::new(&data_dir.join("khinsider")) {
        runtime.set_khinsider_browser(Arc::new(page));
    }
    if let Some(c) = relogin {
        runtime.send(c);
    }
    if let Some(s) = streams::soundcloud_originals() {
        runtime.add_source(s);
    }
    stage("runtime");
    #[allow(unused_mut)]
    let mut ui = Md3eUi::new();
    // A screenshot run off the screen never covers the user's screen.
    #[cfg(feature = "dev")]
    {
        ui.set_fullscreen_allowed(!shot::offscreen());
        ui.set_synthetic_levels(std::env::var_os("NIKGOB_VIZ_SYNTH").is_some());
    }
    #[cfg(feature = "dev")]
    let geom_path = shot::window_size().is_none().then(|| paths.prefs.with_file_name("window.json"));
    #[cfg(not(feature = "dev"))]
    let geom_path = Some(paths.prefs.with_file_name("window.json"));
    let core = Core {
        ui,
        mesh,
        api,
        runtime,
        desktop: desktop::Desktop::start(wake),
        analyzer: links.map(|l| l.analyzer),
        commands: Vec::new(),
        #[cfg(feature = "dev")]
        shot: shot::Shot::from_env().or(sweep_shot),
        #[cfg(feature = "dev")]
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
        quit_asked,
        geometry: geometry::Keeper::load(geom_path),
    };

    stage("desktop");
    Shell::new(core, &waker).start_hidden(start_hidden).run(|lease| {
        let covers_dir = covers_dir.clone();
        let covers_wake = waker.clone();
        #[allow(unused_mut)]
        let mut viewport = lease.peek(|core| core.geometry.apply(egui::ViewportBuilder::default()));
        #[cfg(feature = "dev")]
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
                web_images::WebImages::install(&ctx, covers_dir, covers_wake);
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
                    hides: None,
                    hiding: false,
                    showing: None,
                    cloak: windowing::Cloak::default(),
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
        // (Word timing for the playing song came back on with revision 3.)
        assert!(!p.lyrics_words_all && p.lyrics_words_playing && p.lyrics_lines_auto && !p.lyrics_recognize);
        assert_eq!(p.lyrics_rev, nikgob_view::LYRICS_REV);
        // The control under the lyrics opens its window: the old default
        // («Меню») moves once, a chosen window or page stays, and «Меню»
        // chosen since stays too.
        assert_eq!(p.lyrics_ui, 1);
        assert_eq!(parse_prefs(br#"{"lyrics_ui":0,"lyrics_rev":1}"#).unwrap().lyrics_ui, 1);
        assert_eq!(parse_prefs(br#"{"lyrics_ui":2,"lyrics_rev":1}"#).unwrap().lyrics_ui, 2);
        assert_eq!(parse_prefs(br#"{"lyrics_ui":0,"lyrics_rev":2}"#).unwrap().lyrics_ui, 0);
        assert_eq!(prefs_of(None).lyrics_ui, 1);
        // Everything else stays as it was.
        assert!(p.lyrics_check && !p.lyrics_gpu);
        // Saved again, the user's own choices hold.
        let mine = Prefs { lyrics_words_all: true, lyrics_words_playing: true, lyrics_lines_auto: false, lyrics_recognize: true, ..p };
        let again = parse_prefs(&serde_json::to_vec(&mine).unwrap()).unwrap();
        assert_eq!(again, mine);
        // A first start has the new defaults too.
        let d = prefs_of(None);
        assert!(!d.lyrics_words_all && d.lyrics_words_playing && d.lyrics_lines_auto && !d.lyrics_recognize);
    }

    /// Revision 3: word highlighting is the default experience, so prefs
    /// written before it get word timing for the playing song once, and
    /// the lead moves from the old default to the new one (a lead the user
    /// set stays); prefs written since keep what they hold.
    #[test]
    fn word_timing_and_the_new_lead_come_once() {
        let old = parse_prefs(br#"{"lyrics_words_playing":false,"lyrics_lead":0.1,"lyrics_rev":2}"#).unwrap();
        assert!(old.lyrics_words_playing);
        assert!((old.lyrics_lead - nikgob_view::LYRICS_LEAD).abs() < 1e-6);
        assert_eq!(old.lyrics_rev, nikgob_view::LYRICS_REV);
        let set = parse_prefs(br#"{"lyrics_lead":-0.15,"lyrics_rev":2}"#).unwrap();
        assert!((set.lyrics_lead + 0.15).abs() < 1e-6, "chosen: stays");
        let since = parse_prefs(br#"{"lyrics_words_playing":false,"lyrics_lead":0.1,"lyrics_rev":3}"#).unwrap();
        assert!(!since.lyrics_words_playing && (since.lyrics_lead - 0.1).abs() < 1e-6, "chosen since: stays");
        let again = parse_prefs(&serde_json::to_vec(&old).unwrap()).unwrap();
        assert_eq!(again, old);
        let d = prefs_of(None);
        assert!(d.lyrics_words_playing && (d.lyrics_lead - nikgob_view::LYRICS_LEAD).abs() < 1e-6);
        // The soft edges are on (the «Караоке» look), with gentle settings.
        assert!(d.lyrics_soft_edges && d.lyrics_soft_strength < 0.5 && d.lyrics_soft_depth < 0.35 && d.lyrics_soft_fade < 0.6);
    }


    /// «По словам» was a fourth lyrics animation (`lyrics_style` 3); it is
    /// the unit of every animation now: such prefs move to the wave by
    /// words once, whatever their revision, and the other animations keep
    /// their unit.
    #[test]
    fn the_old_words_animation_moves_to_the_wave_by_words_once() {
        for rev in ["", r#","lyrics_rev":1"#, r#","lyrics_rev":2"#] {
            let p = parse_prefs(format!(r#"{{"lyrics_style":3{rev}}}"#).as_bytes()).unwrap();
            assert_eq!((p.lyrics_style, p.lyrics_by_words), (2, true), "{rev}");
            // Saved and read again it stays the wave by words.
            let again = parse_prefs(&serde_json::to_vec(&p).unwrap()).unwrap();
            assert_eq!(again, p);
        }
        for style in 0..3u8 {
            for words in [false, true] {
                let json = format!(r#"{{"lyrics_style":{style},"lyrics_by_words":{words},"lyrics_rev":2}}"#);
                let p = parse_prefs(json.as_bytes()).unwrap();
                assert_eq!((p.lyrics_style, p.lyrics_by_words), (style, words));
            }
        }
        // Old prefs without the new fields: by letters, soft edges on.
        let p = parse_prefs(br#"{"lyrics_style":2,"lyrics_rev":2}"#).unwrap();
        assert!(!p.lyrics_by_words && p.lyrics_soft_edges && (p.lyrics_soft_strength - 1.0).abs() < 1e-6);
        let d = prefs_of(None);
        assert!(d.lyrics_style == 0 && !d.lyrics_by_words && d.lyrics_soft_edges);
    }

    /// Revision 4: «По слогам» is the unit of the lyrics' animations; prefs
    /// from before it at «По буквам» (the old default) move to it, «По
    /// словам» chosen stays, and what is chosen since stays.
    #[test]
    fn syllables_become_the_unit_once() {
        use nikgob_view::{LYRICS_BY_LETTERS, LYRICS_BY_SYLLABLES, LYRICS_BY_WORDS};
        assert_eq!(parse_prefs(br#"{"lyrics_by_words":false,"lyrics_rev":3}"#).unwrap().lyrics_unit, LYRICS_BY_SYLLABLES);
        assert_eq!(parse_prefs(br#"{"lyrics_by_words":true,"lyrics_rev":3}"#).unwrap().lyrics_unit, LYRICS_BY_WORDS);
        assert_eq!(parse_prefs(br#"{"lyrics_style":3}"#).unwrap().lyrics_unit, LYRICS_BY_WORDS);
        assert_eq!(parse_prefs(br#"{"lyrics_unit":0,"lyrics_rev":4}"#).unwrap().lyrics_unit, LYRICS_BY_LETTERS);
        assert_eq!(prefs_of(None).lyrics_unit, LYRICS_BY_SYLLABLES);
    }

    /// «Только на процессоре» became «Процессор» of «На чём считать анализ
    /// звука»: on, it reads as the CPU; off or never written, «Автоматически»;
    /// a device written since stays.
    #[test]
    fn the_cpu_only_switch_becomes_the_device() {
        use nikgob_view::SoundDevice;
        assert_eq!(parse_prefs(br#"{"sound_cpu_only":true}"#).unwrap().sound_device, SoundDevice::Cpu);
        assert_eq!(parse_prefs(br#"{"sound_cpu_only":false}"#).unwrap().sound_device, SoundDevice::Auto);
        assert_eq!(parse_prefs(br#"{}"#).unwrap().sound_device, SoundDevice::Auto);
        assert_eq!(parse_prefs(br#"{"sound_cpu_only":true,"sound_device":"Gpu"}"#).unwrap().sound_device, SoundDevice::Gpu);
        let p = Prefs { sound_device: SoundDevice::Gpu, ..Prefs::default() };
        assert_eq!(parse_prefs(&serde_json::to_vec(&p).unwrap()).unwrap().sound_device, SoundDevice::Gpu);
    }

    /// Revision 5 hid the player's lyrics button, revision 6 shows it again
    /// by default: prefs from before 6 show it once, a button hidden since
    /// stays hidden.
    #[test]
    fn the_lyrics_button_comes_back_once() {
        let old = parse_prefs(br#"{"player_buttons":{"lyrics":false,"queue":true},"lyrics_rev":5}"#).unwrap();
        assert!(old.player_buttons.lyrics && old.player_buttons.queue);
        assert_eq!(old.lyrics_rev, nikgob_view::LYRICS_REV);
        let older = parse_prefs(br#"{"player_buttons":{"lyrics":true},"lyrics_rev":4}"#).unwrap();
        assert!(older.player_buttons.lyrics);
        let since = parse_prefs(br#"{"player_buttons":{"lyrics":false},"lyrics_rev":6}"#).unwrap();
        assert!(!since.player_buttons.lyrics, "hidden since: stays");
        assert!(prefs_of(None).player_buttons.lyrics);
        let again = parse_prefs(&serde_json::to_vec(&since).unwrap()).unwrap();
        assert_eq!(again, since);
    }

    /// «Перемешивание остаётся включённым» off becomes every kind of list
    /// in order; on (or never written), each kind's default; a rule written
    /// since stays.
    #[test]
    fn the_old_shuffle_switch_becomes_the_kinds_once() {
        use nikgob_view::{ListKind, ListShuffle, ListShuffles};
        let off = parse_prefs(br#"{"keep_shuffle":false}"#).unwrap();
        for kind in ListKind::ALL {
            assert_eq!(off.list_shuffle.of(kind), ListShuffle::InOrder, "{kind:?}");
        }
        assert_eq!(parse_prefs(br#"{"keep_shuffle":true}"#).unwrap().list_shuffle, ListShuffles::default());
        assert_eq!(parse_prefs(br#"{"theme":"Dark"}"#).unwrap().list_shuffle, ListShuffles::default());
        let mut p = off.clone();
        p.list_shuffle.set(ListKind::Mix, ListShuffle::Shuffled);
        let mut written = serde_json::to_value(&p).unwrap();
        written.as_object_mut().unwrap().insert("keep_shuffle".into(), serde_json::Value::Bool(false));
        let again = parse_prefs(&serde_json::to_vec(&written).unwrap()).unwrap();
        assert_eq!(again.list_shuffle, p.list_shuffle);
        assert!(!serde_json::to_value(&again).unwrap().as_object().unwrap().contains_key("keep_shuffle"));
    }

    /// Own playlists shuffled by the old default become «По кнопке» once;
    /// a rule chosen since (or another than the old default) stays.
    #[test]
    fn own_playlists_follow_the_button_once() {
        use nikgob_view::{ListKind, ListShuffle};
        let old = br#"{"list_shuffle":{"mixes":"Shuffled","own_playlists":"Shuffled","albums":"InOrder","ready_playlists":"InOrder"}}"#;
        let p = parse_prefs(old).unwrap();
        assert_eq!(p.list_shuffle.of(ListKind::OwnPlaylist), ListShuffle::Button);
        assert_eq!(p.list_shuffle.of(ListKind::Mix), ListShuffle::Shuffled);
        assert_eq!(p.shuffle_rev, nikgob_view::SHUFFLE_REV);
        let in_order = br#"{"list_shuffle":{"own_playlists":"InOrder"}}"#;
        assert_eq!(parse_prefs(in_order).unwrap().list_shuffle.of(ListKind::OwnPlaylist), ListShuffle::InOrder);
        let mut chosen = p.clone();
        chosen.list_shuffle.set(ListKind::OwnPlaylist, ListShuffle::Shuffled);
        let again = parse_prefs(&serde_json::to_vec(&chosen).unwrap()).unwrap();
        assert_eq!(again.list_shuffle.of(ListKind::OwnPlaylist), ListShuffle::Shuffled, "chosen since: stays");
    }

    /// «Автоматически» stored as the old default of «Альбом показывает»
    /// becomes «Всегда весь альбом» once; a mode chosen since stays, and so
    /// does «Только мои песни».
    #[test]
    fn albums_open_whole_once() {
        use nikgob_view::AlbumMode;
        let p = parse_prefs(br#"{"album_mode":"auto"}"#).unwrap();
        assert_eq!(p.album_mode, AlbumMode::Whole);
        assert_eq!(p.album_rev, nikgob_view::ALBUM_REV);
        assert_eq!(parse_prefs(br#"{"theme":"Dark"}"#).unwrap().album_mode, AlbumMode::Whole);
        assert_eq!(parse_prefs(br#"{"album_mode":"mine"}"#).unwrap().album_mode, AlbumMode::Mine);
        let mut chosen = p.clone();
        chosen.album_mode = AlbumMode::Auto;
        let again = parse_prefs(&serde_json::to_vec(&chosen).unwrap()).unwrap();
        assert_eq!(again.album_mode, AlbumMode::Auto, "chosen since: stays");
        assert_eq!(prefs_of(None).album_mode, AlbumMode::Whole);
    }

    /// Prefs saved with the ring's old rows (on a pause, the turn, its way
    /// and speed, in time) read whole: the ring and its look stay, the old
    /// keys are let go, and «Свои» kept with them still brings the rest
    /// back.
    #[test]
    fn prefs_with_the_old_ring_rows_still_read() {
        use nikgob_view::presets::Area;
        let old = br#"{"ring":true,"ring_style":"Bars","ring_on_pause":true,"spin_ring":false,"ring_spin_s":20.0,
            "ring_ccw":true,"ring_beat":false,"fps_limit":90,"settings_rev":1,
            "custom_presets":{"Track":[["CoverShape",{"Shape":"Heart"}],["Ring",{"Bool":true}],["RingBeat",{"Bool":false}],["SpinRing",{"Bool":true}]]}}"#;
        let p = parse_prefs(old).unwrap();
        assert!(p.ring);
        assert_eq!(p.ring_style, nikgob_view::RingStyle::Bars);
        assert_eq!(p.fps_limit, 90);
        let written = serde_json::to_value(&p).unwrap();
        for key in ["ring_on_pause", "spin_ring", "ring_spin_s", "ring_ccw", "ring_beat"] {
            assert!(written.get(key).is_none(), "{key}");
        }
        let back = Area::Track.custom(&p).expect("«Свои» kept");
        assert_eq!(back.cover_shape, nikgob_view::CoverShape::Heart);
    }

    /// «Настройки текста…» is offered under the lyrics until it is picked
    /// once, and stays gone after a restart.
    #[test]
    fn the_lyrics_settings_row_goes_once_picked() {
        let mut p = parse_prefs(br#"{"theme":"Dark"}"#).unwrap();
        assert!(!p.lyrics_settings_seen, "prefs from before: still offered");
        assert!(!prefs_of(None).lyrics_settings_seen);
        nikgob_view::PrefPatch::LyricsSettingsSeen.apply(&mut p);
        let kept = parse_prefs(&serde_json::to_vec(&p).unwrap()).unwrap();
        assert!(kept.lyrics_settings_seen);
    }

    /// Prefs from before the presets (a made-up sample, not anyone's
    /// file) read and written again differ only by the new keys; the areas
    /// that match no preset keep their values for «Свои» once.
    #[test]
    fn old_prefs_gain_only_the_new_keys() {
        use nikgob_view::presets::{Area, SETTINGS_REV};
        const NEW: [&str; 6] = ["settings_deep", "custom_presets", "settings_rev", "cover_spin_s", "cover_pulse_strength", "home_sections"];
        let mut sample = serde_json::to_value(Prefs {
            fps_limit: 90,
            seed_ratio: 2.0,
            cover_shape: nikgob_view::CoverShape::Heart,
            download_sources: vec![nikgob_view::SourceSwitch { key: nikgob_view::SourceKey::Soulseek, on: true }],
            library_dir: PathBuf::from("music"),
            ..Prefs::default()
        })
        .unwrap();
        let old = sample.as_object_mut().unwrap();
        for key in NEW {
            old.remove(key);
        }
        let old = old.clone();
        let read = parse_prefs(&serde_json::to_vec(&old).unwrap()).unwrap();
        let written = serde_json::to_value(&read).unwrap();
        let written = written.as_object().unwrap();
        for (key, value) in &old {
            assert_eq!(written.get(key), Some(value), "{key}");
        }
        for key in written.keys() {
            assert!(old.contains_key(key) || NEW.contains(&key.as_str()), "{key}");
        }
        assert_eq!(read.settings_rev, SETTINGS_REV);
        assert_eq!(read.settings_deep, nikgob_md3e::screens::brand::own_build());
        let kept: Vec<_> = read.custom_presets.keys().copied().collect();
        assert_eq!(kept, [Area::Recs, Area::Lyrics, Area::Track, Area::Sources, Area::Seeding, Area::Load]);
        // «Свои» gives the same values back.
        assert_eq!(Area::Track.custom(&read).unwrap(), read);
        // Read again, nothing more changes.
        let again = parse_prefs(&serde_json::to_vec(&read).unwrap()).unwrap();
        assert_eq!(again, read);
        // A first start keeps nothing and opens in «Просто».
        let first = prefs_of(None);
        assert!(first.custom_presets.is_empty() && !first.settings_deep);
    }

    /// The first run shows on a new install only: not on an upgrade (its
    /// prefs file has no pending first run), not where the player was used
    /// (a library with music, a service signed in, YouTube Music cookies
    /// stored) though the prefs file is gone, not once finished; one left
    /// midway comes back whatever it brought itself; «Показать первый
    /// запуск» shows it again.
    #[test]
    fn the_first_run_shows_on_a_new_install_only() {
        use nikgob_core::settings::StreamingSettings;
        use nikgob_view::PrefPatch;
        let empty = nikgob_library::Library::open_in_memory().unwrap();
        let none: streams::Services = Vec::new();
        let no_secrets = StreamingSettings::default();
        let at_start = |p: &Prefs, lib: &nikgob_library::Library, s: &streams::Services, sec: &StreamingSettings| first_run_at_start(p, lib, s, sec, false);
        let new = prefs_of(None);
        assert!(new.first_run && !new.first_run_started);
        assert!(at_start(&new, &empty, &none, &no_secrets), "a new install");
        let old = prefs_of(Some(br#"{"theme":"Dark","name_prompt":false}"#));
        assert!(!old.first_run && !at_start(&old, &empty, &none, &no_secrets), "an upgrade");
        let dir = tempfile::tempdir().unwrap();
        let used = nikgob_library::Library::open_in_memory().unwrap();
        demo::seed_with(&used, &dir.path().join("demo"), false);
        assert!(!at_start(&new, &used, &none, &no_secrets), "a library with music, its prefs file lost");
        let signed = streams::demo(&dir.path().join("services"), &Default::default());
        assert!(signed.iter().any(|s| s.account().is_logged_in()));
        assert!(!at_start(&new, &empty, &signed, &no_secrets), "a service signed in");
        let cookies = StreamingSettings { youtube_cookies: Some("SID=1".into()), ..StreamingSettings::default() };
        assert!(!at_start(&new, &empty, &none, &cookies), "YouTube Music cookies stored, its sign-in not back yet");
        // On screen once, then the app quit: its own sign-in and folder do
        // not end it, through a prefs file written and read again.
        let mut midway = new.clone();
        PrefPatch::FirstRunStarted.apply(&mut midway);
        let midway = prefs_of(Some(&serde_json::to_vec(&midway).unwrap()));
        assert!(at_start(&midway, &used, &signed, &cookies), "left midway: shown again");
        assert!(!first_run_at_start(&midway, &used, &none, &no_secrets, true), "not in a demo run or the profile");
        let mut done = midway.clone();
        PrefPatch::FirstRun(false).apply(&mut done);
        let done = prefs_of(Some(&serde_json::to_vec(&done).unwrap()));
        assert!(!done.first_run && !done.first_run_started);
        assert!(!at_start(&done, &empty, &none, &no_secrets), "finished: never again");
        let mut again = done.clone();
        PrefPatch::FirstRun(true).apply(&mut again);
        assert!(again.first_run, "the developer's row shows it again");
        assert!(prefs_of(Some(&serde_json::to_vec(&again).unwrap())).first_run);
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
