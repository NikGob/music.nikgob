//! Start with Windows: the app's line under the user's `Run` key, launched
//! with [`FLAG`] so that run may stay in the tray. Task Manager's «Автозагрузка»
//! switches a line off in `StartupApproved` without removing it; such a line
//! counts as off here, and turning autostart on clears that mark.
//!
//! The switch in Settings is the user's wish and is kept in the prefs. A
//! wished-for line that is gone at start is written again: a cleaner may
//! have removed it, or it was written by a run inside a sandbox that keeps
//! its own copy of the registry, where Windows never saw it.

use std::path::Path;

/// The argument the sign-in launch carries.
pub const FLAG: &str = "--autostart";
/// The app's value under `Run` (and in `StartupApproved`).
const NAME: &str = "music.nikgob";

/// The Run line for `exe`.
fn command(exe: &Path) -> String {
    format!("\"{}\" {FLAG}", exe.display())
}

/// A `StartupApproved` value: its first byte is even while the line is on.
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

    #[cfg(test)]
    pub fn set_mark(name: &str, mark: &[u8]) {
        CURRENT_USER
            .create(APPROVED)
            .and_then(|k| k.set_bytes(name, windows_registry::Type::Bytes, mark))
            .unwrap();
    }
}

#[cfg(not(windows))]
mod sys {
    pub fn line(_name: &str) -> Option<String> {
        None
    }

    pub fn mark(_name: &str) -> Option<Vec<u8>> {
        None
    }

    pub fn write(_name: &str, _line: Option<&str>) -> std::io::Result<()> {
        Err(std::io::Error::other("autostart is only made on Windows"))
    }
}

/// What the system has for the app's line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    On,
    /// Switched off in Task Manager (or Settings → Apps → Startup).
    Off,
    /// No line at all.
    Missing,
}

fn state_as(name: &str) -> State {
    match sys::line(name) {
        None => State::Missing,
        Some(_) if approved(sys::mark(name).as_deref()) => State::On,
        Some(_) => State::Off,
    }
}

/// Whether Windows starts the app at sign-in.
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
        Some(command(&std::env::current_exe().map_err(|e| e.to_string())?))
    } else {
        None
    };
    sys::write(name, line.as_deref()).map_err(|e| e.to_string())
}

/// An enabled line follows the exe that runs now (a moved or rebuilt copy).
pub fn refresh() {
    if let (true, Some(line), Ok(exe)) = (enabled(), sys::line(NAME), std::env::current_exe())
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

    #[test]
    fn the_line_quotes_the_exe() {
        assert_eq!(
            command(Path::new(r"C:\Program Files\nikgob\nikgob.exe")),
            r#""C:\Program Files\nikgob\nikgob.exe" --autostart"#
        );
    }

    /// The real keys, under a throwaway name.
    #[cfg(windows)]
    #[test]
    fn on_off_and_task_manager() {
        let name = "music.nikgob test";
        set_as(name, true).unwrap();
        let exe = std::env::current_exe().unwrap();
        assert_eq!(sys::line(name), Some(command(&exe)));
        assert!(enabled_as(name));
        sys::set_mark(name, &[3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert!(!enabled_as(name), "switched off in Task Manager");
        set_as(name, true).unwrap();
        assert!(enabled_as(name), "turning it on clears the mark");
        set_as(name, false).unwrap();
        assert_eq!(sys::line(name), None);
        assert_eq!(sys::mark(name), None);
        assert!(!enabled_as(name));
    }

    /// The real keys, under a throwaway name.
    #[cfg(windows)]
    #[test]
    fn a_lost_line_comes_back_and_task_managers_off_stays() {
        let name = "music.nikgob reconcile test";
        set_as(name, false).unwrap();
        assert!(!reconcile_as(name, false), "nothing wished, nothing written");
        assert_eq!(sys::line(name), None);
        assert!(reconcile_as(name, true), "a wished-for line that is gone comes back");
        assert_eq!(state_as(name), State::On);
        sys::set_mark(name, &[3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
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
