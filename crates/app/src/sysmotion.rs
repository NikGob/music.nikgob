//! The OS-wide "less animation" switch (Windows: Settings → Accessibility →
//! Visual effects → Animation effects; GNOME: Accessibility → Reduce
//! Animation; KDE: the animation speed slider at its far end), read so
//! reduced motion follows it.

/// True when the user turned animations off system-wide.
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn reduced() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
    };
    let mut on: i32 = 1;
    // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL into the pointed-to
    // i32, which lives for the whole call.
    let ok = unsafe {
        SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut on).cast(), 0)
    };
    ok != 0 && on == 0
}

/// True when the user turned animations off system-wide. The interface asks
/// every few seconds on its own thread, and GNOME's switch is read by running
/// `gsettings`, so each call answers with what the last read found and
/// starts the next read in the background.
#[cfg(target_os = "linux")]
pub fn reduced() -> bool {
    use std::sync::atomic::{AtomicBool, Ordering};
    static REDUCED: AtomicBool = AtomicBool::new(false);
    static READING: AtomicBool = AtomicBool::new(false);
    if !READING.swap(true, Ordering::AcqRel) {
        let spawned = std::thread::Builder::new().name("reduced-motion".into()).spawn(|| {
            // Either desktop's switch counts: GNOME's key reads "on" on KDE
            // and KDE's file does not exist on GNOME, so the first answer
            // that says "on" must not hide the other's "off".
            let off = linux::gnome_off() == Some(true) || linux::kde_off() == Some(true);
            REDUCED.store(off, Ordering::Relaxed);
            READING.store(false, Ordering::Release);
        });
        if spawned.is_err() {
            READING.store(false, Ordering::Release);
        }
    }
    REDUCED.load(Ordering::Relaxed)
}

#[cfg(target_os = "linux")]
mod linux {
    use std::process::{Command, Stdio};

    /// GNOME's `enable-animations` (also what GNOME-based desktops and
    /// GTK apps go by); `None` without gsettings or the schema.
    pub fn gnome_off() -> Option<bool> {
        let out = Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "enable-animations"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        out.status.success().then(|| parse_gsettings(&String::from_utf8_lossy(&out.stdout)))?
    }

    /// KDE's animation speed: `AnimationDurationFactor=0` under `[KDE]` in
    /// kdeglobals means no animation.
    pub fn kde_off() -> Option<bool> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;
        parse_kdeglobals(&std::fs::read_to_string(config.join("kdeglobals")).ok()?)
    }

    pub fn parse_gsettings(out: &str) -> Option<bool> {
        match out.trim() {
            "false" => Some(true),
            "true" => Some(false),
            _ => None,
        }
    }

    pub fn parse_kdeglobals(text: &str) -> Option<bool> {
        let mut inside = false;
        for l in text.lines().map(str::trim) {
            if l.starts_with('[') {
                inside = l == "[KDE]";
            } else if inside
                && let Some(("AnimationDurationFactor", v)) = l.split_once('=').map(|(k, v)| (k.trim(), v.trim()))
            {
                return v.parse::<f32>().ok().map(|f| f <= 0.0);
            }
        }
        None
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn gnome_and_kde_switches() {
            assert_eq!(parse_gsettings("false\n"), Some(true));
            assert_eq!(parse_gsettings("true\n"), Some(false));
            assert_eq!(parse_gsettings(""), None);
            assert_eq!(parse_kdeglobals("[General]\nX=1\n[KDE]\nAnimationDurationFactor=0\n"), Some(true));
            assert_eq!(parse_kdeglobals("[KDE]\nAnimationDurationFactor=0.5\n"), Some(false));
            assert_eq!(parse_kdeglobals("[Other]\nAnimationDurationFactor=0\n"), None);
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn reduced() -> bool {
    false
}
