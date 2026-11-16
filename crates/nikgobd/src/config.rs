//! `nikgobd.toml`: where the headless player keeps its data and the music,
//! which output it plays on, whether the heavy background passes run, and
//! how much it logs. Every key is optional; `NIKGOBD_*` variables override
//! the file (a systemd unit's `Environment=`).

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The example file (`--print-config`), also shipped next to the binary.
pub const EXAMPLE: &str = include_str!("../../../packaging/linux/nikgobd.toml");

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// The library database, prefs, session, settings, the lock and the
    /// crash log. Default: `$XDG_DATA_HOME/nikgobd`.
    pub data_dir: Option<PathBuf>,
    /// The music (`Music/` of the player). Default: `<data_dir>/Music`.
    pub library_dir: Option<PathBuf>,
    /// The lossless masters. Default: the prefs' (first start:
    /// `<library_dir>/.musiclib/masters`).
    pub masters_dir: Option<PathBuf>,
    /// The output: a device name as the player lists it or, on Linux, a
    /// PulseAudio sink name (`alsa_output.usb-…analog-stereo`). Set here,
    /// it is chosen at every start; unset, the last one chosen over the API
    /// stays (or the system default).
    pub output_device: Option<String>,
    /// The sound analysis and the file checks in the background. Off by
    /// default: a Raspberry Pi gets their results from the PC.
    pub background_compute: bool,
    /// env_logger filters: `info`, `debug`, `info,nikgob_runtime=debug`.
    pub log: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data_dir: None,
            library_dir: None,
            masters_dir: None,
            output_device: None,
            background_compute: false,
            log: "info".to_owned(),
        }
    }
}

/// `$XDG_CONFIG_HOME/nikgob/nikgobd.toml` (`~/.config/nikgob/nikgobd.toml`).
pub fn default_path() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.config_dir().join("nikgob").join("nikgobd.toml"))
}

/// `$XDG_DATA_HOME/nikgobd` (`~/.local/share/nikgobd`).
pub fn default_data_dir() -> PathBuf {
    directories::BaseDirs::new().map_or_else(|| PathBuf::from("nikgobd-data"), |d| d.data_dir().join("nikgobd"))
}

/// Reads the config: `asked` (`--config`) must exist; the default file may
/// be missing (everything default then). The environment goes over it.
/// Returns the config and the file it came from, if any.
pub fn load(asked: Option<&Path>) -> Result<(Config, Option<PathBuf>), String> {
    let (path, must) = match asked {
        Some(p) => (Some(p.to_owned()), true),
        None => (default_path(), false),
    };
    let mut cfg = Config::default();
    let mut from = None;
    if let Some(path) = path {
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                cfg = parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
                from = Some(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !must => {}
            Err(e) => return Err(format!("{}: {e} (an example: nikgobd --print-config)", path.display())),
        }
    }
    apply_env(&mut cfg, |k| std::env::var(k).ok());
    Ok((cfg, from))
}

/// The config in a file's text, its paths with `~` expanded.
pub fn parse(text: &str) -> Result<Config, String> {
    let mut cfg: Config = toml::from_str(text).map_err(|e| e.to_string())?;
    let home = directories::BaseDirs::new().map(|d| d.home_dir().to_owned());
    for p in [&mut cfg.data_dir, &mut cfg.library_dir, &mut cfg.masters_dir].into_iter().flatten() {
        *p = expand_home(p, home.as_deref());
    }
    cfg.output_device = cfg.output_device.map(|d| d.trim().to_owned()).filter(|d| !d.is_empty());
    Ok(cfg)
}

/// `NIKGOBD_DATA_DIR`, `NIKGOBD_LIBRARY_DIR`, `NIKGOBD_MASTERS_DIR`,
/// `NIKGOBD_OUTPUT_DEVICE`, `NIKGOBD_BACKGROUND_COMPUTE` (`1`/`0`,
/// `true`/`false`) and `NIKGOBD_LOG` over the file; an empty value is
/// none.
pub fn apply_env(cfg: &mut Config, var: impl Fn(&str) -> Option<String>) {
    let var = |k: &str| var(k).map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
    if let Some(v) = var("NIKGOBD_DATA_DIR") {
        cfg.data_dir = Some(PathBuf::from(v));
    }
    if let Some(v) = var("NIKGOBD_LIBRARY_DIR") {
        cfg.library_dir = Some(PathBuf::from(v));
    }
    if let Some(v) = var("NIKGOBD_MASTERS_DIR") {
        cfg.masters_dir = Some(PathBuf::from(v));
    }
    if let Some(v) = var("NIKGOBD_OUTPUT_DEVICE") {
        cfg.output_device = Some(v);
    }
    if let Some(v) = var("NIKGOBD_BACKGROUND_COMPUTE") {
        match v.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => cfg.background_compute = true,
            "0" | "false" | "no" | "off" => cfg.background_compute = false,
            other => eprintln!("NIKGOBD_BACKGROUND_COMPUTE={other}: not a yes or no, left as it was"),
        }
    }
    if let Some(v) = var("NIKGOBD_LOG") {
        cfg.log = v;
    }
}

/// `~/x` under the home dir; anything else as it is.
fn expand_home(p: &Path, home: Option<&Path>) -> PathBuf {
    match (p.strip_prefix("~"), home) {
        (Ok(rest), Some(home)) => home.join(rest),
        _ => p.to_owned(),
    }
}

/// Where everything of one data dir lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    pub data: PathBuf,
    pub library_dir: PathBuf,
    pub masters_dir: Option<PathBuf>,
    pub db: PathBuf,
    pub prefs: PathBuf,
    pub settings: PathBuf,
    pub crash_log: PathBuf,
    pub lock: PathBuf,
}

impl Paths {
    pub fn of(cfg: &Config) -> Self {
        let data = cfg.data_dir.clone().unwrap_or_else(default_data_dir);
        Self {
            library_dir: cfg.library_dir.clone().unwrap_or_else(|| data.join("Music")),
            masters_dir: cfg.masters_dir.clone(),
            db: data.join("library.sqlite"),
            prefs: data.join("prefs.json"),
            settings: data.join("settings.toml"),
            crash_log: data.join("crash.log"),
            lock: data.join("nikgobd.lock"),
            data,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn the_example_file_parses_to_the_pi_setup() {
        let cfg = parse(EXAMPLE).expect("the example is valid");
        assert_eq!(cfg.output_device.as_deref(), Some("alsa_output.usb-3142_fifine_Headset-00.analog-stereo"));
        assert!(!cfg.background_compute);
        assert_eq!(cfg.log, "info");
    }

    #[test]
    fn an_empty_file_is_all_defaults() {
        let cfg = parse("").unwrap();
        assert_eq!(cfg, Config::default());
        assert!(!cfg.background_compute, "off unless asked");
        let paths = Paths::of(&Config { data_dir: Some(PathBuf::from("/d")), ..Config::default() });
        assert_eq!(paths.library_dir, PathBuf::from("/d").join("Music"));
        assert_eq!(paths.db, PathBuf::from("/d").join("library.sqlite"));
        assert_eq!(paths.prefs, PathBuf::from("/d").join("prefs.json"));
        assert_eq!(paths.lock, PathBuf::from("/d").join("nikgobd.lock"));
        assert_eq!(paths.masters_dir, None);
    }

    #[test]
    fn a_key_it_does_not_know_is_an_error() {
        assert!(parse("output = \"x\"").is_err());
        assert!(parse("background_compute = \"maybe\"").is_err());
    }

    #[test]
    fn the_environment_goes_over_the_file() {
        let mut cfg = parse("library_dir = \"/srv/music\"\noutput_device = \"a\"\nbackground_compute = true\n").unwrap();
        let env: HashMap<&str, &str> = [
            ("NIKGOBD_OUTPUT_DEVICE", "alsa_output.usb-x.analog-stereo"),
            ("NIKGOBD_BACKGROUND_COMPUTE", "0"),
            ("NIKGOBD_LOG", "debug"),
            ("NIKGOBD_DATA_DIR", ""),
        ]
        .into_iter()
        .collect();
        apply_env(&mut cfg, |k| env.get(k).map(|v| (*v).to_owned()));
        assert_eq!(cfg.output_device.as_deref(), Some("alsa_output.usb-x.analog-stereo"));
        assert!(!cfg.background_compute);
        assert_eq!(cfg.log, "debug");
        assert_eq!(cfg.data_dir, None, "an empty value is none");
        assert_eq!(cfg.library_dir, Some(PathBuf::from("/srv/music")), "untouched");
        // A word that is no yes or no leaves the switch.
        apply_env(&mut cfg, |k| (k == "NIKGOBD_BACKGROUND_COMPUTE").then(|| "perhaps".to_owned()));
        assert!(!cfg.background_compute);
        apply_env(&mut cfg, |k| (k == "NIKGOBD_BACKGROUND_COMPUTE").then(|| "On".to_owned()));
        assert!(cfg.background_compute);
    }

    #[test]
    fn a_tilde_is_the_home_dir() {
        let home = Path::new("/home/nik");
        assert_eq!(expand_home(Path::new("~/music"), Some(home)), PathBuf::from("/home/nik/music"));
        assert_eq!(expand_home(Path::new("/mnt/ssd"), Some(home)), PathBuf::from("/mnt/ssd"));
        assert_eq!(expand_home(Path::new("~/music"), None), PathBuf::from("~/music"));
        // A blank device is no device.
        assert_eq!(parse("output_device = \"  \"").unwrap().output_device, None);
    }
}
