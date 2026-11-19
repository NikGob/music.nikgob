//! The player's own builds this computer can hand to the other devices of
//! the home: the files the release scripts leave in `releases/` (next to
//! the program, or where `NIKGOB_RELEASES` points), served by the local API
//! to a device that has an invite.
//!
//! How a Linux device installs the headless player from here, without the
//! invite ever crossing the network:
//!
//! ```text
//! curl -fsSL "http://<host>/install/nikgobd.sh?k=<key>" -o /tmp/nikgobd-install.sh \
//!   && echo "<sha256>  /tmp/nikgobd-install.sh" | sha256sum -c --quiet - \
//!   && sh /tmp/nikgobd-install.sh <ticket>
//! ```
//!
//! The download key is derived one way from the invite shown now
//! ([`download_key`]): it opens the files and nothing else. The line, read
//! off this computer's screen, pins the script by its SHA-256 (made here
//! over the very bytes the API serves for that key: [`install_script`]);
//! the script pins each build it offers by its SHA-256 and checks the
//! download before unpacking it. The ticket goes to the script as its
//! argument, on the device itself.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

/// The environment variable naming another folder of builds.
pub const RELEASES_ENV: &str = "NIKGOB_RELEASES";

/// The architectures the headless player is built for, as `uname -m`
/// names them.
pub const LINUX_ARCHES: [&str; 2] = ["aarch64", "x86_64"];

/// The install script's name under `/install/`.
pub const INSTALL_SCRIPT: &str = "nikgobd.sh";

/// Where the install line keeps the script while it checks it.
pub const SCRIPT_TMP: &str = "/tmp/nikgobd-install.sh";

/// The context of [`download_key`] (fixed: a key made by another build of
/// the same version must match).
const KEY_CONTEXT: &str = "music.nikgob 2026-11-16 install download key";

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

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// The key that opens `/install/` while the invite `ticket` is shown: 16
/// bytes of a keyed BLAKE3 of it, in hex. It cannot be turned back into
/// the ticket, so it may travel in a URL and stand in the script.
pub fn download_key(ticket: &str) -> String {
    hex(&blake3::derive_key(KEY_CONTEXT, ticket.as_bytes())[..16])
}

/// `a == b` without stopping at the first difference.
pub fn same_secret(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The SHA-256 of `bytes`, in hex (what `sha256sum` prints).
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    hex(sha2::Sha256::digest(bytes).as_slice())
}

/// The SHA-256 of the file at `path`, read in pieces.
pub fn file_sha256(path: &Path) -> std::io::Result<String> {
    use sha2::Digest as _;
    use std::io::Read as _;
    let mut f = std::fs::File::open(path)?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(h.finalize().as_slice()))
}

/// A Linux build this computer has, with its checksum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinuxBuild {
    pub arch: &'static str,
    /// SHA-256 of the tarball, hex.
    pub sha256: String,
}

/// A tarball's checksum, kept while its size and time stay the same.
struct Hashed {
    path: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
    sha256: String,
}

/// The checksums made so far (a few tarballs at most).
static HASHED: Mutex<Vec<Hashed>> = Mutex::new(Vec::new());

/// How many checksums are kept.
const HASHED_MAX: usize = 8;

/// The checksum of the build at `path`, made once per version of the file
/// (a tarball is tens of megabytes: never on a UI or player thread).
fn hashed(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let (len, modified) = (meta.len(), meta.modified().ok());
    if let Some(h) = HASHED.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|h| h.path == path && h.len == len && h.modified == modified) {
        return Some(h.sha256.clone());
    }
    let sha256 = match file_sha256(path) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("releases: a build could not be read: {e}");
            return None;
        }
    };
    let mut kept = HASHED.lock().unwrap_or_else(PoisonError::into_inner);
    kept.retain(|h| h.path != path);
    kept.push(Hashed { path: path.to_owned(), len, modified, sha256: sha256.clone() });
    if kept.len() > HASHED_MAX {
        kept.remove(0);
    }
    Some(sha256)
}

/// The Linux builds of `version` in `dir`, with their checksums, in the
/// order of [`LINUX_ARCHES`]. Reads the files the first time (slow).
pub fn linux_builds(dir: &Path, version: &str) -> Vec<LinuxBuild> {
    LINUX_ARCHES
        .iter()
        .copied()
        .filter_map(|arch| {
            let path = find_in(dir, &nikgobd_tarball(version, arch))?;
            Some(LinuxBuild { arch, sha256: hashed(&path)? })
        })
        .collect()
}

/// A host[:port] that may stand in a shell script and a URL.
pub fn plain_host(host: &str) -> bool {
    !host.is_empty() && host.len() <= 255 && host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b':' | b'-' | b'[' | b']'))
}

/// A download key as it may stand in the script: hex only.
pub fn plain_key(key: &str) -> bool {
    key.len() == 32 && key.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The install script the API serves at `http://<host>/install/nikgobd.sh`
/// for download key `key`: it offers exactly `builds` (the others end with
/// one line), downloads the machine's from `host`, checks its SHA-256,
/// installs it in `~/.local/bin` with the example config (when there is
/// none) and the systemd user unit, pairs with the ticket given as its
/// argument and starts the service. POSIX sh; no sudo, no linger (it says
/// how), no sound settings. The same arguments give the same bytes: the
/// install line pins them by their checksum.
pub fn install_script(host: &str, key: &str, version: &str, builds: &[LinuxBuild]) -> String {
    let mut sums = String::new();
    for b in builds {
        sums.push_str(&format!("  {arch}) sum='{sum}' ;;\n", arch = b.arch, sum = b.sha256));
    }
    format!(
        r#"#!/bin/sh
# nikgobd {version}: the music.nikgob player without a window, from {host}.
set -eu
ticket="${{1:-}}"
if [ -z "$ticket" ]; then
  echo "nikgobd: no ticket given (sh nikgobd-install.sh <ticket>)" >&2; exit 1
fi
base='http://{host}'
key='{key}'
version='{version}'
case "$(uname -m)" in
  aarch64|arm64) arch=aarch64 ;;
  x86_64|amd64) arch=x86_64 ;;
  *) arch="$(uname -m)" ;;
esac
case "$arch" in
{sums}  *) echo "No build for $arch" >&2; exit 1 ;;
esac
name="nikgobd-$version-linux-$arch"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL -o "$tmp/$name.tar.gz" "$base/install/$name.tar.gz?k=$key" || {{ echo "nikgobd: the build did not download" >&2; exit 1; }}
if ! echo "$sum  $tmp/$name.tar.gz" | sha256sum -c - >/dev/null 2>&1; then
  echo "nikgobd: the build does not match its checksum" >&2; exit 1
fi
tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
bin="$HOME/.local/bin"
conf="${{XDG_CONFIG_HOME:-$HOME/.config}}"
mkdir -p "$bin" "$conf/nikgob" "$conf/systemd/user"
if command -v systemctl >/dev/null 2>&1; then
  systemctl --user stop nikgobd 2>/dev/null || true
fi
install -m 755 "$tmp/$name/nikgobd" "$bin/nikgobd"
if [ ! -f "$conf/nikgob/nikgobd.toml" ]; then
  sed -e 's/^api_address/#api_address/' -e 's/^name =/#name =/' -e 's/^kind =/#kind =/' -e 's/^output_device/#output_device/' \
    "$tmp/$name/nikgobd.toml" > "$conf/nikgob/nikgobd.toml"
fi
install -m 644 "$tmp/$name/nikgobd.service" "$conf/systemd/user/nikgobd.service"
"$bin/nikgobd" pair "$ticket"
systemctl --user daemon-reload
systemctl --user enable --now nikgobd >/dev/null
echo "nikgobd $version installed and paired"
if [ "$(loginctl show-user "$(id -un)" -p Linger --value 2>/dev/null || true)" != yes ]; then
  echo "to keep it running without a login session: sudo loginctl enable-linger $(id -un)"
fi
"#
    )
}

/// The line to copy onto a Linux device: downloads the script with `key`
/// from `host`, checks it against `script_sha256` and runs it with
/// `ticket`.
pub fn install_command(host: &str, key: &str, script_sha256: &str, ticket: &str) -> String {
    format!(
        "curl -fsSL \"http://{host}/install/{script}?k={key}\" -o {tmp} && echo \"{script_sha256}  {tmp}\" | sha256sum -c --quiet - && sh {tmp} {ticket}",
        script = INSTALL_SCRIPT,
        tmp = SCRIPT_TMP,
    )
}

/// Where Windows downloads the program of `version` from `host`.
pub fn windows_url(host: &str, key: &str, version: &str) -> String {
    format!("http://{host}/install/{}?k={key}", windows_exe(version))
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

    /// The key is fixed per ticket, hex, another for another ticket, and
    /// carries nothing of the ticket.
    #[test]
    fn the_download_key_stands_for_the_ticket_one_way() {
        let t = "nkm1ybndrfg8ejkmcpqxot1uwisza345h769";
        let k = download_key(t);
        assert_eq!(k, download_key(t));
        assert!(plain_key(&k), "{k}");
        assert_ne!(k, download_key("nkm1ybndrfg8ejkmcpqxot1uwisza345h768"));
        assert!(!k.contains("ybndrfg8") && !t.contains(&k[..8]));
        assert!(same_secret(k.as_bytes(), download_key(t).as_bytes()));
        assert!(!same_secret(b"abc", b"abd") && !same_secret(b"ab", b"abc"));
        assert!(!plain_key("x'; rm -rf ~") && !plain_key(""));
    }

    #[test]
    fn checksums_are_what_sha256sum_prints() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let dir = std::env::temp_dir().join(format!("nikgob-core-sha-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tar = dir.join(nikgobd_tarball("0.0.5", "aarch64"));
        std::fs::write(&tar, b"abc").unwrap();
        assert_eq!(file_sha256(&tar).unwrap(), sha256_hex(b"abc"));
        let builds = linux_builds(&dir, "0.0.5");
        assert_eq!(builds, vec![LinuxBuild { arch: "aarch64", sha256: sha256_hex(b"abc") }], "only the build that is there");
        assert_eq!(linux_builds(&dir, "0.0.5"), builds, "the same from what is kept");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The script takes the ticket as its argument, offers only the builds
    /// given and checks the one it downloads; never root, linger or sound.
    #[test]
    fn the_script_pins_its_builds_and_wants_the_ticket_given() {
        let builds = [LinuxBuild { arch: "aarch64", sha256: "ab".repeat(32) }];
        let key = "0123456789abcdef0123456789abcdef";
        let s = install_script("192.168.0.10:47210", key, "0.0.5", &builds);
        assert_eq!(s, install_script("192.168.0.10:47210", key, "0.0.5", &builds), "the same bytes every time");
        assert!(s.starts_with("#!/bin/sh\n"));
        assert!(s.contains("ticket=\"${1:-}\""), "the ticket is the argument");
        assert!(!s.contains("nkm1"), "no ticket in the script");
        assert!(s.contains("base='http://192.168.0.10:47210'"));
        assert!(s.contains(&format!("key='{key}'")));
        assert!(s.contains(&format!("  aarch64) sum='{}' ;;\n", "ab".repeat(32))));
        assert!(!s.contains("  x86_64) sum="), "no build, not offered");
        assert!(s.contains("*) echo \"No build for $arch\" >&2; exit 1 ;;"));
        assert!(s.contains("\"$base/install/$name.tar.gz?k=$key\""));
        assert!(s.contains("echo \"$sum  $tmp/$name.tar.gz\" | sha256sum -c -"));
        assert!(s.find("sha256sum").unwrap() < s.find("tar -xzf").unwrap(), "checked before unpacked");
        assert!(s.contains("XDG_CONFIG_HOME:-$HOME/.config}"), "the braces of the shell survive the format");
        assert!(s.contains("\"$bin/nikgobd\" pair \"$ticket\""));
        assert!(s.contains("systemctl --user enable --now nikgobd"));
        assert!(s.contains("if [ ! -f \"$conf/nikgob/nikgobd.toml\" ]"), "a config there is kept");
        for line in s.lines().filter(|l| !l.trim_start().starts_with('#') && !l.contains("echo")) {
            assert!(!line.contains("sudo"), "{line}");
            assert!(!line.contains("enable-linger"), "{line}");
            assert!(!line.contains("pactl") && !line.contains("asound"), "{line}");
        }
    }

    #[test]
    fn the_line_pins_the_script_and_hands_it_the_ticket() {
        let line = install_command("192.168.0.10:47210", "0123", "ffee", "nkm1abc");
        assert_eq!(
            line,
            "curl -fsSL \"http://192.168.0.10:47210/install/nikgobd.sh?k=0123\" -o /tmp/nikgobd-install.sh \
             && echo \"ffee  /tmp/nikgobd-install.sh\" | sha256sum -c --quiet - && sh /tmp/nikgobd-install.sh nkm1abc"
        );
        assert_eq!(windows_url("192.168.0.10:47210", "0123", "0.0.5"), "http://192.168.0.10:47210/install/music.nikgob-0.0.5.exe?k=0123");
        assert!(plain_host("192.168.0.10:47210") && plain_host("[fe80::1]:47210") && !plain_host("x';id;'") && !plain_host(""));
    }
}
