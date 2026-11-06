//! Start with the system: on Windows the app's line under the user's `Run`
//! key, on Linux an XDG autostart entry, launched with [`FLAG`] so that run
//! may stay in the tray. Task Manager's «Автозагрузка» switches a line off in
//! `StartupApproved` without removing it, and a desktop's startup settings
//! switch an entry off with `Hidden=true` or `X-GNOME-Autostart-enabled=false`;
//! such a line counts as off here, and turning autostart on clears that mark.
//!
//! The switch in Settings is the user's wish and is kept in the prefs. A
//! wished-for line that is gone at start is written again: a cleaner may
//! have removed it, or it was written by a run inside a sandbox that keeps
//! its own copy of the registry, where Windows never saw it.

use std::path::Path;

/// The argument the sign-in launch carries.
pub const FLAG: &str = "--autostart";
/// The app's value under `Run` (and in `StartupApproved`); on Linux the
/// autostart entry's file name, which is also the window's app id.
const NAME: &str = "music.nikgob";

/// The Run line for `exe`.
#[cfg(not(target_os = "linux"))]
fn command(exe: &Path) -> String {
    format!("\"{}\" {FLAG}", exe.display())
}

/// The entry's `Exec` value for `exe`: the path quoted as the desktop entry
/// spec asks, with the file format's own backslash escapes on top (so a
/// backslash inside the quotes is written four times, a quote, backtick or
/// dollar gets two before it), and `%` doubled so it is not a field code.
#[cfg(target_os = "linux")]
fn command(exe: &Path) -> String {
    let mut quoted = String::new();
    for c in exe.to_string_lossy().chars() {
        match c {
            '\\' => quoted.push_str(r"\\\\"),
            '"' | '`' | '$' => {
                quoted.push_str(r"\\");
                quoted.push(c);
            }
            '%' => quoted.push_str("%%"),
            _ => quoted.push(c),
        }
    }
    format!("\"{quoted}\" {FLAG}")
}

/// A `StartupApproved` value: its first byte is even while the line is on.
#[cfg(any(windows, test))]
fn approved(mark: Option<&[u8]>) -> bool {
    mark.and_then(<[u8]>::first).is_none_or(|b| b & 1 == 0)
}

#[cfg(windows)]
mod sys {
    use windows_registry::CURRENT_USER;

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

    pub fn line(name: &str) -> Option<String> {
        CURRENT_USER.open(RUN).ok()?.get_string(name).ok()
    }

    pub fn mark(name: &str) -> Option<Vec<u8>> {
        Some(CURRENT_USER.open(APPROVED).ok()?.get_value(name).ok()?.to_vec())
    }

    /// Switched off in Task Manager (or Settings → Apps → Startup).
    pub fn switched_off(name: &str) -> bool {
        !super::approved(mark(name).as_deref())
    }

    pub fn write(name: &str, line: Option<&str>) -> windows_registry::Result<()> {
        let run = CURRENT_USER.create(RUN)?;
        match line {
            Some(l) => run.set_string(name, l)?,
            None => {
                let _ = run.remove_value(name);
            }
        }
        // Without its mark the line is on; an old "off" must not outlive it.
        if let Ok(k) = CURRENT_USER.options().read().write().open(APPROVED) {
            let _ = k.remove_value(name);
        }
        Ok(())
    }

    /// Switches the line off as Task Manager does.
    #[cfg(test)]
    pub fn switch_off(name: &str) {
        let mark = [3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        CURRENT_USER
            .create(APPROVED)
            .and_then(|k| k.set_bytes(name, windows_registry::Type::Bytes, &mark))
            .unwrap();
    }
}

/// The XDG autostart entry, `$XDG_CONFIG_HOME/autostart/<name>.desktop`
/// (`~/.config` when the variable is unset or not absolute, as the spec
/// says). Desktops switch an entry off without removing it: `Hidden=true`
/// (the spec's own way, and KDE's) or `X-GNOME-Autostart-enabled=false`
/// (GNOME). Writing the entry anew drops both.
#[cfg(target_os = "linux")]
mod sys {
    use std::path::PathBuf;

    /// Tests keep their entries in a folder of their own, never in the
    /// user's autostart.
    fn dir() -> Option<PathBuf> {
        if cfg!(test) {
            return Some(std::env::temp_dir().join(format!("nikgob-autostart-test-{}", std::process::id())));
        }
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(config.join("autostart"))
    }

    fn file(name: &str) -> Option<PathBuf> {
        Some(dir()?.join(format!("{name}.desktop")))
    }

    /// The `[Desktop Entry]` group's keys and values, in order.
    fn entry(name: &str) -> Option<Vec<(String, String)>> {
        let text = std::fs::read_to_string(file(name)?).ok()?;
        let mut keys = Vec::new();
        let mut inside = false;
        for l in text.lines().map(str::trim) {
            if l.starts_with('[') {
                inside = l == "[Desktop Entry]";
            } else if inside && let Some((k, v)) = l.split_once('=') {
                keys.push((k.trim().to_owned(), v.trim().to_owned()));
            }
        }
        Some(keys)
    }

    pub fn line(name: &str) -> Option<String> {
        let keys = entry(name)?;
        Some(keys.into_iter().find(|(k, _)| k == "Exec").map(|(_, v)| v).unwrap_or_default())
    }

    pub fn switched_off(name: &str) -> bool {
        entry(name).is_some_and(|keys| {
            keys.iter().any(|(k, v)| {
                (k == "Hidden" && v.eq_ignore_ascii_case("true"))
                    || (k == "X-GNOME-Autostart-enabled" && v.eq_ignore_ascii_case("false"))
            })
        })
    }

    pub fn write(name: &str, line: Option<&str>) -> std::io::Result<()> {
        let path = file(name).ok_or_else(|| std::io::Error::other("no home folder for the autostart entry"))?;
        let Some(line) = line else {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        };
        if line.contains(['\n', '\r']) {
            return Err(std::io::Error::other("the program's path does not fit an autostart entry"));
        }
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let text = format!(
            "[Desktop Entry]\nType=Application\nName=music.nikgob\nExec={line}\nIcon=music.nikgob\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
        );
        // Whole or not at all: a session that starts meanwhile reads one or
        // the other.
        let tmp = path.with_extension("desktop.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &path)
    }

    /// Switches the entry off as the spec (and KDE) does.
    #[cfg(test)]
    pub fn switch_off(name: &str) {
        let path = file(name).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, format!("{text}Hidden=true\n")).unwrap();
    }

    /// Switches the entry off as GNOME does.
    #[cfg(test)]
    pub fn switch_off_gnome(name: &str) {
        let path = file(name).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace("X-GNOME-Autostart-enabled=true", "X-GNOME-Autostart-enabled=false")).unwrap();
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod sys {
    pub fn line(_name: &str) -> Option<String> {
        None
    }

    pub fn switched_off(_name: &str) -> bool {
        false
    }

    pub fn write(_name: &str, _line: Option<&str>) -> std::io::Result<()> {
        Err(std::io::Error::other("autostart is only made on Windows and Linux"))
    }
}

/// What the system has for the app's line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    On,
    /// Switched off in Task Manager (or Settings → Apps → Startup), or in a
    /// Linux desktop's startup settings.
    Off,
    /// No line at all.
    Missing,
}

fn state_as(name: &str) -> State {
    match sys::line(name) {
        None => State::Missing,
        Some(_) if !sys::switched_off(name) => State::On,
        Some(_) => State::Off,
    }
}

/// Whether the system starts the app at sign-in.
pub fn enabled() -> bool {
    enabled_as(NAME)
}

fn enabled_as(name: &str) -> bool {
    state_as(name) == State::On
}

/// At start: what the switch shows, given what the prefs `wanted`. The
/// system's own "off" wins; a wished-for line that is missing is written
/// again for this exe.
pub fn reconcile(wanted: bool) -> bool {
    refresh();
    reconcile_as(NAME, wanted)
}

fn reconcile_as(name: &str, wanted: bool) -> bool {
    match state_as(name) {
        State::On => true,
        State::Off => false,
        State::Missing if wanted => match set_as(name, true) {
            Ok(()) => {
                log::warn!("autostart was on but its Run line was gone; written again");
                enabled_as(name)
            }
            Err(e) => {
                log::warn!("cannot write the missing autostart line: {e}");
                false
            }
        },
        State::Missing => false,
    }
}

/// Turns autostart on (for this exe) or off.
pub fn set(on: bool) -> Result<(), String> {
    set_as(NAME, on)
}

fn set_as(name: &str, on: bool) -> Result<(), String> {
    let line = if on {
        Some(command(&program().map_err(|e| e.to_string())?))
    } else {
        None
    };
    sys::write(name, line.as_deref()).map_err(|e| e.to_string())
}

/// The program the line starts: this exe, or on Linux the Nix wrapper that
/// ran it. Nix's wrappers run the binary as `.nikgob-wrapped` with the
/// wrapper's own `argv[0]` (a path, or the bare name it was found by on
/// `PATH`), and the binary alone lacks what the wrapper sets up (GTK's
/// schemas, GIO's TLS module), so the entry starts the wrapper.
fn program() -> std::io::Result<std::path::PathBuf> {
    let exe = std::env::current_exe()?;
    #[cfg(target_os = "linux")]
    if exe
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.') && n.ends_with("-wrapped"))
        && let Some(wrapper) = std::env::args_os().next().map(std::path::PathBuf::from).and_then(|p| {
            if p.is_absolute() {
                Some(p)
            } else if p.components().count() == 1 {
                std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(&p)).find(|c| c.is_file())
            } else {
                None
            }
        })
        && wrapper.is_file()
    {
        return Ok(wrapper);
    }
    Ok(exe)
}

/// An enabled line follows the exe that runs now (a moved or rebuilt copy).
pub fn refresh() {
    if let (true, Some(line), Ok(exe)) = (enabled(), sys::line(NAME), program())
        && line != command(&exe)
        && let Err(e) = set(true)
    {
        log::warn!("cannot point autostart at {}: {e}", exe.display());
    }
}

/// Applies the switch in Settings to the system, once per change.
pub struct Keeper {
    /// What the system has; `None` leaves the system alone (demo runs).
    applied: Option<bool>,
    /// The value the system last refused: not asked again while the prefs
    /// still carry it (the flip back takes the runtime a moment).
    refused: Option<bool>,
}

impl Keeper {
    pub fn new(system: Option<bool>) -> Self {
        Self { applied: system, refused: None }
    }

    /// The value the prefs should show when the system refused `wanted`.
    pub fn sync(&mut self, wanted: bool) -> Option<bool> {
        let applied = self.applied.as_mut()?;
        if *applied == wanted {
            self.refused = None;
            return None;
        }
        if self.refused == Some(wanted) {
            return None;
        }
        match set(wanted) {
            Ok(()) => {
                *applied = wanted;
                None
            }
            Err(e) => {
                log::warn!("cannot turn autostart {}: {e}", if wanted { "on" } else { "off" });
                self.refused = Some(wanted);
                Some(*applied)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_manager_marks() {
        assert!(approved(None));
        assert!(approved(Some(&[])));
        assert!(approved(Some(&[2, 0, 0, 0])));
        assert!(approved(Some(&[6, 0, 0, 0])));
        assert!(!approved(Some(&[3, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8])));
        assert!(!approved(Some(&[7, 0])));
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn the_line_quotes_the_exe() {
        assert_eq!(
            command(Path::new(r"C:\Program Files\nikgob\nikgob.exe")),
            r#""C:\Program Files\nikgob\nikgob.exe" --autostart"#
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_entry_quotes_the_exe() {
        assert_eq!(command(Path::new("/opt/music nikgob/nikgob")), r#""/opt/music nikgob/nikgob" --autostart"#);
        assert_eq!(command(Path::new(r#"/a$b/"c"/100%/d\e"#)), r#""/a\\$b/\\"c\\"/100%%/d\\\\e" --autostart"#);
    }

    /// The real keys (on Linux, entries in a test folder), under a throwaway
    /// name.
    #[cfg(any(windows, target_os = "linux"))]
    #[test]
    fn on_off_and_task_manager() {
        let name = "music.nikgob test";
        set_as(name, true).unwrap();
        let exe = program().unwrap();
        assert_eq!(sys::line(name), Some(command(&exe)));
        assert!(enabled_as(name));
        sys::switch_off(name);
        assert!(!enabled_as(name), "switched off in Task Manager");
        set_as(name, true).unwrap();
        assert!(enabled_as(name), "turning it on clears the mark");
        assert!(!sys::switched_off(name));
        set_as(name, false).unwrap();
        assert_eq!(sys::line(name), None);
        assert!(!sys::switched_off(name));
        assert!(!enabled_as(name));
        #[cfg(windows)]
        assert_eq!(sys::mark(name), None);
    }

    /// GNOME's own switch counts as off too.
    #[cfg(target_os = "linux")]
    #[test]
    fn gnomes_off_is_off() {
        let name = "music.nikgob gnome test";
        set_as(name, true).unwrap();
        assert_eq!(state_as(name), State::On);
        sys::switch_off_gnome(name);
        assert_eq!(state_as(name), State::Off);
        set_as(name, true).unwrap();
        assert_eq!(state_as(name), State::On);
        set_as(name, false).unwrap();
        assert_eq!(state_as(name), State::Missing);
    }

    /// The real keys (on Linux, entries in a test folder), under a throwaway
    /// name.
    #[cfg(any(windows, target_os = "linux"))]
    #[test]
    fn a_lost_line_comes_back_and_task_managers_off_stays() {
        let name = "music.nikgob reconcile test";
        set_as(name, false).unwrap();
        assert!(!reconcile_as(name, false), "nothing wished, nothing written");
        assert_eq!(sys::line(name), None);
        assert!(reconcile_as(name, true), "a wished-for line that is gone comes back");
        assert_eq!(state_as(name), State::On);
        sys::switch_off(name);
        assert!(!reconcile_as(name, true), "Task Manager's off wins over the prefs");
        assert_eq!(state_as(name), State::Off);
        set_as(name, false).unwrap();
        assert_eq!(state_as(name), State::Missing);
    }

    #[test]
    fn a_keeper_without_a_system_does_nothing() {
        let mut k = Keeper::new(None);
        assert_eq!(k.sync(true), None);
        assert_eq!(k.applied, None);
        let mut k = Keeper::new(Some(true));
        assert_eq!(k.sync(true), None);
    }
}
