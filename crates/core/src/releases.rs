//! The player's own builds this computer can hand to the other devices of
//! the home: the files the release scripts leave in `releases/` (next to
//! the program, or where `NIKGOB_RELEASES` points), served by the local API
//! to a device that has an invite.

use std::path::{Path, PathBuf};

/// The environment variable naming another folder of builds.
pub const RELEASES_ENV: &str = "NIKGOB_RELEASES";

/// The architectures the headless player is built for, as `uname -m`
/// names them.
pub const LINUX_ARCHES: [&str; 2] = ["aarch64", "x86_64"];

/// The tarball of the headless player `version` for Linux on `arch`.
pub fn nikgobd_tarball(version: &str, arch: &str) -> String {
    format!("nikgobd-{version}-linux-{arch}.tar.gz")
}

/// The Windows program of `version`.
pub fn windows_exe(version: &str) -> String {
    format!("music.nikgob-{version}.exe")
}

/// Whether `name` is one of the files a device may download: a tarball of
/// `version` for a known architecture, or the Windows program of `version`.
/// Nothing else (no other version, no path) is ever served.
pub fn servable(name: &str, version: &str) -> bool {
    name == windows_exe(version) || LINUX_ARCHES.iter().any(|a| name == nikgobd_tarball(version, a))
}

/// Where the builds are: `NIKGOB_RELEASES`, else `releases/` next to the
/// program, else `releases/` up to three folders above it (a build run from
/// `target/<profile>/` in the repository, where the scripts put them).
pub fn releases_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(RELEASES_ENV).map(PathBuf::from).filter(|d| !d.as_os_str().is_empty()) {
        return Some(dir);
    }
    let exe = std::env::current_exe().ok()?;
    let mut at = exe.parent();
    for _ in 0..4 {
        let dir = at?.join("releases");
        if dir.is_dir() {
            return Some(dir);
        }
        at = at?.parent();
    }
    None
}

/// The file `name` in `dir`, if it is there.
pub fn find_in(dir: &Path, name: &str) -> Option<PathBuf> {
    let path = dir.join(name);
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_this_version_of_known_builds_is_served() {
        assert!(servable("nikgobd-0.0.5-linux-aarch64.tar.gz", "0.0.5"));
        assert!(servable("nikgobd-0.0.5-linux-x86_64.tar.gz", "0.0.5"));
        assert!(servable("music.nikgob-0.0.5.exe", "0.0.5"));
        assert!(!servable("nikgobd-0.0.4-linux-aarch64.tar.gz", "0.0.5"), "another version");
        assert!(!servable("nikgobd-0.0.5-linux-riscv64.tar.gz", "0.0.5"));
        assert!(!servable("../settings.toml", "0.0.5"));
        assert!(!servable("nikgobd-0.0.5-linux-aarch64.tar.gz/..", "0.0.5"));
        assert_eq!(nikgobd_tarball("1.2.3", "aarch64"), "nikgobd-1.2.3-linux-aarch64.tar.gz");
    }

    #[test]
    fn a_file_is_found_only_where_it_is() {
        let dir = std::env::temp_dir().join(format!("nikgob-core-releases-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("music.nikgob-0.0.5.exe"), b"MZ").unwrap();
        assert!(find_in(&dir, "music.nikgob-0.0.5.exe").is_some());
        assert!(find_in(&dir, "nikgobd-0.0.5-linux-aarch64.tar.gz").is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
