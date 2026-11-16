//! The player behind the phone's window, as a phone runs it: the library
//! and prefs of its own data dir, the engine, the runtime with no heavy
//! passes in the background (their results come from the computer or the
//! Pi), the local API and the owner's devices as a phone. Started the way
//! the app and `nikgobd` start theirs.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use nikgob_core::streaming::{FakeService, Service, StreamService};
use nikgob_runtime::{NullPlayback, Playback, Runtime, RuntimeConfig, RuntimeOptions, StreamSchedule};
use nikgob_view::{MeshKind, Prefs};

/// How the phone was asked to start.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// `--data <dir>`: its data dir (another phone beside the first, say).
    pub data: Option<PathBuf>,
    /// `--demo` or `NIKGOB_DEMO=1`: the generated demo library in a
    /// throwaway dir, the fake services.
    pub demo: bool,
    /// `--name <name>`: the name the other devices see.
    pub name: Option<String>,
    /// `--no-mesh`: not among the owner's devices.
    pub no_mesh: bool,
}

/// Where the phone keeps its things.
struct Paths {
    data: PathBuf,
    db: PathBuf,
    prefs: PathBuf,
    settings: PathBuf,
    music: PathBuf,
}

impl Paths {
    fn of(data: PathBuf) -> Self {
        Self {
            db: data.join("library.sqlite"),
            prefs: data.join("prefs.json"),
            settings: data.join("settings.toml"),
            music: data.join("Music"),
            data,
        }
    }
}

/// The phone's own data dir: never the desktop player's.
fn default_data() -> PathBuf {
    directories::ProjectDirs::from("dev", "nikgob", "music.nikgob-phone")
        .map_or_else(|| PathBuf::from("nikgob-phone"), |d| d.data_dir().to_owned())
}

/// The running player and what runs beside it.
pub struct Host {
    pub runtime: Runtime,
    pub analyzer: Option<nikgob_playback::AnalyzerSlot>,
    /// The window's context, once there is a window: the runtime's wake
    /// repaints it.
    pub window: Arc<OnceLock<egui::Context>>,
    api: Mutex<Option<nikgob_api::ApiHandle>>,
    mesh: Mutex<Option<nikgob_mesh::MeshHandle>>,
}

impl Host {
    pub fn start(opts: &Options) -> Result<Self, String> {
        let data = match (&opts.data, opts.demo) {
            (Some(d), _) => d.clone(),
            (None, true) => std::env::temp_dir().join("nikgob-phone-demo"),
            (None, false) => default_data(),
        };
        let paths = Paths::of(data);
        // A demo library from an older seed is thrown away and seeded again.
        if opts.demo && opts.data.is_none() && !paths.data.join(crate::demo::VERSION).exists() {
            let _ = std::fs::remove_dir_all(&paths.data);
            std::fs::create_dir_all(&paths.data).map_err(|e| format!("{}: {e}", paths.data.display()))?;
            let _ = std::fs::write(paths.data.join(crate::demo::VERSION), b"");
        }
        for dir in [&paths.data, &paths.music] {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        log::info!("phone data in {}", paths.data.display());
        let library =
            nikgob_library::Library::open(&paths.db).map_err(|e| format!("cannot open the library at {}: {e}", paths.db.display()))?;
        if opts.demo && library.count_tracks(nikgob_library::TrackFilter::All).unwrap_or(0) == 0 {
            crate::demo::seed(&library, &paths.music);
        }
        let settings = nikgob_core::settings::Settings::load(&paths.settings).unwrap_or_else(|e| {
            log::info!("{e}; the defaults");
            nikgob_core::settings::Settings::default()
        });
        let mut prefs = read_prefs(&paths.prefs);
        prefs.library_dir = paths.music.clone();
        if prefs.inbox_dir.as_os_str().is_empty() {
            prefs.inbox_dir = paths.data.join("Inbox");
        }
        if prefs.masters_dir.as_os_str().is_empty() {
            prefs.masters_dir = paths.music.join(".musiclib").join("masters");
        }
        prefs.output_devices = nikgob_audio::output_device_names().unwrap_or_default();

        let streams = if opts.demo { demo_streams() } else { real_streams(&paths.data) };
        let (backend, links): (Box<dyn Playback>, _) = if std::env::var_os("NIKGOB_NO_AUDIO").is_some() {
            (Box::new(NullPlayback), None)
        } else {
            let device = prefs.output_device.clone().filter(|d| prefs.output_devices.contains(d));
            match nikgob_playback::Engine::start(device, streams.clone(), nikgob_playback::LivePlayers::default()) {
                Ok((engine, links)) => (Box::new(engine), Some(links)),
                Err(e) => {
                    log::error!("the audio engine did not start: {e}");
                    (Box::new(nikgob_runtime::NoAudio), None)
                }
            }
        };

        let window: Arc<OnceLock<egui::Context>> = Arc::new(OnceLock::new());
        let api_wake = nikgob_api::Wake::new();
        let wake = {
            let (w, a) = (window.clone(), api_wake.clone());
            move || {
                if let Some(ctx) = w.get() {
                    ctx.request_repaint();
                }
                a.wake();
            }
        };
        let import = import_setup(&paths, &prefs, &settings, opts.demo);
        let runtime = Runtime::start_with(
            RuntimeConfig {
                library: library.clone(),
                library_dir: prefs.library_dir.clone(),
                prefs,
                prefs_path: Some(paths.prefs.clone()),
                settings_path: Some(paths.settings.clone()),
                import: Some(import),
                streams,
            },
            backend,
            Arc::new(wake),
            StreamSchedule::default(),
            RuntimeOptions {
                // The sound analysis, the alignment and the file checks
                // come done from the computer or the Pi.
                background_compute: false,
                userdata_export: false,
            },
        );
        if let Some(l) = &links {
            l.connect(runtime.playback_sender());
        }
        let api = match nikgob_api::start(nikgob_api::ApiContext {
            runtime: runtime.handle(),
            library: library.clone(),
            wake: api_wake.clone(),
            media_roots: Vec::new(),
        }) {
            Ok(a) => Some(a),
            Err(e) => {
                log::error!("the API did not start: {e}");
                None
            }
        };
        // A demo shows up among the owner's devices only when asked.
        let mesh_on = !opts.no_mesh && (!opts.demo || std::env::var_os("NIKGOB_MESH").is_some());
        let mesh = if mesh_on {
            let ctx = nikgob_mesh::MeshContext {
                data_dir: paths.data.clone(),
                device_name: opts.name.clone().unwrap_or_else(|| "Телефон".to_owned()),
                kind: MeshKind::Phone,
                runtime: runtime.handle(),
                api: api.as_ref().map(nikgob_api::ApiHandle::local),
                wake: api_wake,
                version: env!("CARGO_PKG_VERSION").to_owned(),
                headless: false,
                library: Some(library),
            };
            match nikgob_mesh::start(ctx) {
                Ok(m) => Some(m),
                Err(e) => {
                    log::error!("the mesh did not start: {e}");
                    None
                }
            }
        } else {
            None
        };
        Ok(Self {
            runtime,
            analyzer: links.map(|l| l.analyzer),
            window,
            api: Mutex::new(api),
            mesh: Mutex::new(mesh),
        })
    }

    /// Stops the mesh and the API, then the player (its session and prefs
    /// saved), as the app quits.
    pub fn shutdown(&self) {
        if let Some(m) = self.mesh.lock().unwrap_or_else(PoisonError::into_inner).take() {
            m.stop();
        }
        if let Some(a) = self.api.lock().unwrap_or_else(PoisonError::into_inner).take() {
            a.stop();
        }
        self.runtime.shutdown();
    }

    /// The bass level and the bands of what plays now.
    pub fn levels(&self) -> (f32, Vec<f32>) {
        let Some(slot) = &self.analyzer else { return (0.0, Vec::new()) };
        let mut guard = slot.lock().unwrap_or_else(PoisonError::into_inner);
        match guard.as_mut() {
            Some(a) => {
                a.update();
                (a.bass_energy(), a.bands().to_vec())
            }
            None => (0.0, Vec::new()),
        }
    }
}

/// The prefs of the data dir; a first start's without them (or with a file
/// this build cannot read at all).
fn read_prefs(path: &Path) -> Prefs {
    let Ok(bytes) = std::fs::read(path) else { return Prefs::first_start() };
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .and_then(Prefs::from_json_lenient)
        .unwrap_or_else(|e| {
            log::warn!("{}: {e}; a first start's prefs", path.display());
            Prefs::first_start()
        })
}

/// The lyrics and covers of the songs that play (no Inbox to watch, no
/// torrents, no models).
fn import_setup(paths: &Paths, prefs: &Prefs, settings: &nikgob_core::settings::Settings, demo: bool) -> nikgob_runtime::ImportSetup {
    let mut cfg =
        nikgob_runtime::ImportConfig::new(prefs.library_dir.clone(), prefs.inbox_dir.clone(), paths.data.join("Downloads"), paths.data.join("Trash"));
    cfg.masters_dir = prefs.masters_dir.clone();
    let offline = demo || std::env::var_os("NIKGOB_OFFLINE").is_some();
    cfg.fetch_lyrics = !offline;
    cfg.fetch_cover_online = !offline;
    nikgob_runtime::ImportSetup {
        config: cfg,
        metadata: (!offline).then(|| nikgob_runtime::MetadataConfig::new(settings.metadata.contact.clone())),
        watch_inbox: false,
        torrent_state_dir: paths.data.join("torrents"),
        torrent_offline: true,
        models_dir: None,
    }
}

/// YouTube Music (its own sign-in, the phone's) and the Deezer catalogue.
fn real_streams(data: &Path) -> Vec<Arc<dyn StreamService>> {
    let mut out: Vec<Arc<dyn StreamService>> = Vec::new();
    let cache = data.join("cache").join("streams");
    match nikgob_youtube::Youtube::new(nikgob_youtube::YoutubeConfig::new(data.join("youtube"))) {
        Ok(yt) => {
            yt.warm_up();
            out.push(yt.with_audio_cache(cache.join("youtube")));
        }
        Err(e) => log::error!("YouTube Music did not start: {e}"),
    }
    match nikgob_deezer::Deezer::new(nikgob_deezer::DeezerConfig::default()) {
        Ok(dz) => out.push(dz),
        Err(e) => log::error!("Deezer did not start: {e}"),
    }
    out
}

/// The demo's fake YouTube Music and Deezer (their new singles for Главная).
fn demo_streams() -> Vec<Arc<dyn StreamService>> {
    let yt = FakeService::new(Service::Youtube).with_new_release("utada", "kaze", "風の歌", 23).with_new_release("bjork", "aurora", "Aurora", 9);
    let dz = FakeService::new(Service::Deezer).with_new_release("daft", "voyager2", "Voyager 2", 12);
    yt.set_audio_secs(180.0);
    dz.set_audio_secs(180.0);
    vec![Arc::new(yt) as Arc<dyn StreamService>, Arc::new(dz) as Arc<dyn StreamService>]
}
