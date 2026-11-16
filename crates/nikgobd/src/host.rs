//! The headless player itself: the library and the prefs of its data dir,
//! the engine on the output asked for, and the runtime that drives it —
//! the same loop the app's window drives, with no streaming services, no
//! Soloist or SoundCloud player, no import or downloads (local songs only
//! in this version) and, unless asked, no heavy background passes.
//!
//! What a server next to it needs is here: the [`Runtime`] (commands in,
//! the state out), a [`Library`] of its own, the settings, and [`Wake`] to
//! hear of every change of the state.

use std::path::Path;
use std::sync::Arc;

use nikgob_core::settings::Settings;
use nikgob_library::Library;
use nikgob_runtime::{NullPlayback, Playback, Runtime, RuntimeConfig, RuntimeOptions, StreamSchedule};
use nikgob_view::Prefs;

use crate::config::{Config, Paths};
use crate::lock::DataLock;
use crate::wake::Wake;

// The library, the settings and `wake` are for the server that starts next
// to the player; nothing reads them before it does.
#[allow(dead_code)]
pub struct Host {
    /// The player's loop. Shared, so a server can hold it too; it stops at
    /// [`Host::shutdown`] whoever still holds it.
    pub runtime: Arc<Runtime>,
    /// The library, for reads off the player's loop (the runtime has its
    /// own handle on the same database).
    pub library: Library,
    /// `settings.toml` of the data dir, as read at the start.
    pub settings: Settings,
    pub paths: Paths,
    /// Callbacks for every change of the runtime's state.
    pub wake: Wake,
    _lock: DataLock,
}

impl Host {
    /// Opens the data dir (taking its lock), the library, the prefs and the
    /// engine, and starts the runtime.
    pub fn start(cfg: &Config, paths: Paths) -> Result<Self, String> {
        for dir in [&paths.data, &paths.library_dir] {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let lock = DataLock::acquire(&paths.lock)?;
        let library = Library::open(&paths.db).map_err(|e| format!("cannot open the library at {}: {e}", paths.db.display()))?;
        let mut settings = Settings::load(&paths.settings).unwrap_or_else(|e| {
            log::warn!("{e}; going on without it");
            Settings::default()
        });
        apply_api(&mut settings, cfg, &paths.settings);
        let prefs = prefs_for(&paths, cfg, nikgob_audio::output_device_names().unwrap_or_else(|e| {
            log::warn!("audio devices: {e}");
            Vec::new()
        }));
        log::info!("music in {}", prefs.library_dir.display());
        log::info!(
            "output: {}",
            prefs.output_device.as_deref().unwrap_or("the system default")
        );
        if !prefs.output_devices.is_empty() {
            log::info!("outputs here: {}", prefs.output_devices.join(" | "));
        }

        let backend: Box<dyn Playback>;
        let links;
        if std::env::var_os("NIKGOB_NO_AUDIO").is_some() {
            log::info!("NIKGOB_NO_AUDIO: playing nowhere");
            backend = Box::new(NullPlayback);
            links = None;
        } else {
            match nikgob_playback::Engine::start(prefs.output_device.clone(), Vec::new(), nikgob_playback::LivePlayers::default()) {
                Ok((engine, l)) => {
                    backend = Box::new(engine);
                    links = Some(l);
                }
                Err(e) => {
                    log::error!("the audio engine did not start: {e}");
                    backend = Box::new(nikgob_runtime::NoAudio);
                    links = None;
                }
            }
        }

        let wake = Wake::default();
        let runtime = Runtime::start_with(
            RuntimeConfig {
                library: library.clone(),
                library_dir: prefs.library_dir.clone(),
                prefs,
                prefs_path: Some(paths.prefs.clone()),
                settings_path: Some(paths.settings.clone()),
                // Playback only: no Inbox, no downloads, no lyrics lookups,
                // nothing that moves files.
                import: None,
                // Local songs only in this version.
                streams: Vec::new(),
            },
            backend,
            wake.callback(),
            StreamSchedule::default(),
            RuntimeOptions {
                background_compute: cfg.background_compute,
                // Never into a library folder the app may export to; the
                // user data goes between devices by the sync.
                userdata_export: false,
            },
        );
        if let Some(l) = &links {
            l.connect(runtime.playback_sender());
        }
        if !cfg.background_compute {
            log::info!("background compute off (background_compute = true in the config turns it on)");
        }
        Ok(Self {
            runtime: Arc::new(runtime),
            library,
            settings,
            paths,
            wake,
            _lock: lock,
        })
    }

    /// Stops the runtime as the app does when it quits (the session and the
    /// prefs saved, the engine stopped), then lets the data dir go.
    pub fn shutdown(self) {
        self.runtime.shutdown();
    }
}

/// The config's API switch, address and port over `[api]` of settings.toml,
/// saved before the runtime reads it.
fn apply_api(settings: &mut Settings, cfg: &Config, path: &Path) {
    let before = (settings.api.enabled, settings.api.address.clone(), settings.api.port);
    settings.api.enabled = cfg.api;
    if let Some(a) = &cfg.api_address {
        settings.api.address = a.trim().to_owned();
    }
    if let Some(p) = cfg.api_port {
        settings.api.port = p;
    }
    if before != (settings.api.enabled, settings.api.address.clone(), settings.api.port)
        && let Err(e) = settings.save(path)
    {
        log::warn!("cannot save the API settings to {}: {e}", path.display());
    }
}

/// The prefs of the data dir (a first start's when there are none yet)
/// with what the config sets: the folders, and the output when it names
/// one; `devices` are the outputs there are.
fn prefs_for(paths: &Paths, cfg: &Config, devices: Vec<String>) -> Prefs {
    let mut p = read_prefs(&paths.prefs);
    p.library_dir = paths.library_dir.clone();
    if p.inbox_dir.as_os_str().is_empty() {
        p.inbox_dir = paths.data.join("Inbox");
    }
    if let Some(m) = &paths.masters_dir {
        p.masters_dir = m.clone();
    }
    p.output_devices = devices;
    if let Some(d) = &cfg.output_device {
        p.output_device = Some(d.clone());
    }
    p
}

fn read_prefs(path: &Path) -> Prefs {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            log::warn!("{}: {e}; the defaults instead", path.display());
            Prefs::first_start()
        }),
        Err(_) => Prefs::first_start(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn the_config_sets_the_folders_and_the_output_over_the_saved_prefs() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config {
            data_dir: Some(dir.path().to_owned()),
            library_dir: Some(PathBuf::from("/mnt/ssd/Music")),
            masters_dir: Some(PathBuf::from("/mnt/ssd/masters")),
            output_device: Some("alsa_output.usb-3142_fifine_Headset-00.analog-stereo".into()),
            ..Config::default()
        };
        let paths = Paths::of(&cfg);
        let saved = Prefs { output_device: Some("HDMI".into()), library_dir: PathBuf::from("/old"), ..Prefs::default() };
        std::fs::write(&paths.prefs, serde_json::to_vec(&saved).unwrap()).unwrap();
        let p = prefs_for(&paths, &cfg, vec!["fifine Headset Analog Stereo".into()]);
        assert_eq!(p.library_dir, PathBuf::from("/mnt/ssd/Music"));
        assert_eq!(p.masters_dir, PathBuf::from("/mnt/ssd/masters"));
        assert_eq!(p.output_device.as_deref(), Some("alsa_output.usb-3142_fifine_Headset-00.analog-stereo"));
        assert_eq!(p.output_devices, ["fifine Headset Analog Stereo"]);

        // No output in the config: the one chosen last stays.
        let cfg = Config { output_device: None, ..cfg };
        assert_eq!(prefs_for(&paths, &cfg, Vec::new()).output_device.as_deref(), Some("HDMI"));
    }

    #[test]
    fn without_prefs_or_with_broken_ones_it_starts_as_a_first_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.json");
        assert_eq!(read_prefs(&path), Prefs::first_start());
        std::fs::write(&path, b"{ not json").unwrap();
        assert_eq!(read_prefs(&path), Prefs::first_start());
    }
}
