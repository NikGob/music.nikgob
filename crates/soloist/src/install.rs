//! Spotify's own Soloist builds: where they come from, how one is unpacked
//! and checked, and which one runs.
//!
//! Builds are downloaded only from `soloist-builds.spotifycdn.com` (Spotify
//! asks apps not to redistribute them). Each lives in a folder named after
//! the SHA-256 of its executable and is never written again: a pointer file
//! (`current.json`) says which one runs, so a renewal is a new folder plus a
//! new pointer and never touches an executable that may be running
//! (Windows would refuse anyway). A renewal waits in `pending.json` until
//! the player is idle.
//!
//! A build expires 90 days after its build date, which `--version` prints
//! («soloist 1.2.0 build 1786000000 …»); an expired one exits with code 10.
//! Renewal starts in the last 30 days, at most once a day.

use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nikgob_core::i18n::backend::SoloistArchiveFault;
use nikgob_core::i18n::{Locale, backend as i18n_backend};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const CDN: &str = "soloist-builds.spotifycdn.com";
pub const DAY: u64 = 86_400;
/// How long a build lives after its build date.
pub const LIFETIME: u64 = 90 * DAY;
/// A renewal is looked for this long before a build expires.
pub const RENEW_BEFORE: u64 = 30 * DAY;
/// Bounds on what is downloaded and unpacked (builds are ~20 MB).
const ARCHIVE_LIMIT: u64 = 120 * 1024 * 1024;
const UNPACKED_LIMIT: u64 = 240 * 1024 * 1024;
const MAX_ENTRIES: usize = 64;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
/// The pointer to the build that runs.
pub const CURRENT: &str = "current.json";
/// A renewal waiting for the player to be idle.
pub const PENDING: &str = "pending.json";

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("Spotify does not publish Soloist for this system")]
    Unsupported,
    /// The bytes left Spotify's CDN on the way (a redirect away).
    #[error("download failed: the download left Spotify's server")]
    Redirected,
    #[error("download failed: {0}")]
    Network(String),
    /// The unpacking library said this about the archive, verbatim.
    #[error("the Soloist archive is wrong: {0}")]
    ArchiveText(String),
    #[error("the Soloist archive is wrong: {0}")]
    Archive(i18n_backend::SoloistArchiveFault),
    #[error("Soloist will not run on this computer")]
    WontRun,
    #[error("Spotify has only an expired Soloist build for now")]
    Expired,
    #[error("the Soloist build has no Spotify signature: {0}")]
    Signature(i18n_backend::SoloistSignatureFault),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

impl InstallError {
    /// The message the settings show for a failed download, in `loc` (first
    /// letter upper-case; the caller capitalises as before).
    pub fn message_in(&self, loc: Locale) -> String {
        let s = i18n_backend::backend_in(loc);
        match self {
            Self::Unsupported => s.soloist_unsupported.to_owned(),
            Self::Redirected => s.soloist_redirected.to_owned(),
            Self::Network(detail) => i18n_backend::fmt_soloist_network_in(loc, detail),
            Self::ArchiveText(detail) => i18n_backend::fmt_soloist_archive_text_in(loc, detail),
            Self::Archive(fault) => i18n_backend::fmt_soloist_archive_in(loc, *fault),
            Self::WontRun => s.soloist_wont_run.to_owned(),
            Self::Expired => s.soloist_only_expired.to_owned(),
            Self::Signature(fault) => i18n_backend::fmt_soloist_signature_in(loc, fault),
            Self::Io(e) => e.to_string(),
        }
    }
}

/// The archive of this platform on the CDN.
pub const fn artifact() -> Option<&'static str> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Some("soloist_release_windows_x86_64.zip")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("soloist_release_x86_64.tar.gz")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("soloist_release_arm64.tar.gz")
    } else if cfg!(all(target_os = "linux", target_arch = "arm")) {
        Some("soloist_release_arm32.tar.gz")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("soloist_release_osx_arm64.tar.gz")
    } else {
        None
    }
}

pub fn url() -> Option<String> {
    artifact().map(|a| format!("https://{CDN}/{a}"))
}

pub const EXE: &str = if cfg!(windows) { "soloist.exe" } else { "soloist" };

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// One unpacked build.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Build {
    pub version: String,
    /// Build date, unix seconds.
    pub built: u64,
    /// When the CDN was last asked for a newer one.
    pub checked: u64,
    /// SHA-256 of the executable, hex: also its folder's name.
    pub digest: String,
}

impl Build {
    pub fn exe(&self, root: &Path) -> PathBuf {
        root.join(&self.digest).join(EXE)
    }

    pub const fn expires(&self) -> u64 {
        self.built + LIFETIME
    }

    /// Not expired, not from the future (a wrong clock), a sane digest.
    pub fn usable(&self, at: u64) -> bool {
        self.built <= at + DAY
            && at < self.expires()
            && self.digest.len() == 64
            && self.digest.bytes().all(|b| b.is_ascii_hexdigit())
    }

    /// In its last 30 days and not asked about today.
    pub fn renewal_due(&self, at: u64) -> bool {
        at + RENEW_BEFORE >= self.expires() && at.saturating_sub(self.checked) >= DAY
    }
}

/// «soloist 1.2.0 build 1786000000 (…) windows x86_64» → version, build date.
pub fn parse_version(text: &str) -> Option<(String, u64)> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let at = words.iter().position(|w| w.eq_ignore_ascii_case("soloist"))?;
    let version = words.get(at + 1)?.trim_start_matches('v').to_owned();
    let built = words
        .iter()
        .position(|w| w.eq_ignore_ascii_case("build"))
        .and_then(|i| words.get(i + 1))
        .and_then(|w| w.trim_matches(|c: char| !c.is_ascii_digit()).parse::<u64>().ok())
        .filter(|b| *b > 1_600_000_000)?;
    Some((version, built))
}

fn digest_of(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}

/// The build a pointer names, if it is usable and its executable is the
/// one that was unpacked.
pub fn read_pointer(root: &Path, name: &str) -> Option<Build> {
    let build: Build = serde_json::from_slice(&std::fs::read(root.join(name)).ok()?).ok()?;
    if !build.usable(now()) {
        return None;
    }
    (digest_of(&build.exe(root)).ok()? == build.digest).then_some(build)
}

/// Whatever a pointer says, unchecked (to tell «expired» from «none»).
pub fn read_pointer_raw(root: &Path, name: &str) -> Option<Build> {
    serde_json::from_slice(&std::fs::read(root.join(name)).ok()?).ok()
}

pub fn write_pointer(root: &Path, name: &str, build: &Build) -> std::io::Result<()> {
    std::fs::create_dir_all(root)?;
    let tmp = root.join(format!("{name}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(build).map_err(std::io::Error::other)?)?;
    std::fs::rename(&tmp, root.join(name))
}

pub fn remove_pointer(root: &Path, name: &str) {
    let _ = std::fs::remove_file(root.join(name));
}

/// Deletes build folders not in `keep` and staging folders left by an
/// interrupted download of an earlier run (this run's own may be in use by a
/// download going on).
pub fn collect(root: &Path, keep: &[&str]) {
    let ours = std::process::id().to_string();
    for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let build = name.len() == 64 && name.bytes().all(|b| b.is_ascii_hexdigit());
        let staging = name.starts_with("staging-") && name.rsplit('-').next() != Some(ours.as_str());
        let stale = (build && !keep.contains(&name.as_str())) || staging;
        if stale && e.file_type().is_ok_and(|t| t.is_dir()) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// The archive's size, asked of the CDN without downloading it.
pub async fn archive_size(http: &reqwest::Client) -> Option<u64> {
    let r = http
        .head(url()?)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    r.headers()
        .get(reqwest::header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

/// Downloads, unpacks and checks the current build into `root`; `progress`
/// gets (bytes so far, total when known). Returns it without pointing to
/// it: the caller decides whether it runs now or waits. `purpose` names its
/// staging folder, so a renewal and a download asked for by the user never
/// unpack into the same one.
pub async fn download(
    root: &Path,
    http: &reqwest::Client,
    progress: &(dyn Fn(u64, Option<u64>) + Send + Sync),
    purpose: &str,
) -> Result<Build, InstallError> {
    let artifact = artifact().ok_or(InstallError::Unsupported)?;
    let url = format!("https://{CDN}/{artifact}");
    let net = |e: reqwest::Error| InstallError::Network(e.without_url().to_string());
    let mut response = http
        .get(&url)
        .timeout(DOWNLOAD_TIMEOUT)
        .send()
        .await
        .map_err(net)?
        .error_for_status()
        .map_err(net)?;
    // Redirects are followed; the bytes must still come from Spotify's CDN.
    if response.url().scheme() != "https" || response.url().host_str() != Some(CDN) {
        return Err(InstallError::Redirected);
    }
    let total = response.content_length();
    if total.is_some_and(|t| t > ARCHIVE_LIMIT) {
        return Err(InstallError::Archive(SoloistArchiveFault::TooLarge));
    }
    let mut bytes = Vec::with_capacity(total.unwrap_or(0) as usize);
    progress(0, total);
    let mut shown = std::time::Instant::now();
    while let Some(chunk) = response.chunk().await.map_err(net)? {
        if bytes.len() as u64 + chunk.len() as u64 > ARCHIVE_LIMIT {
            return Err(InstallError::Archive(SoloistArchiveFault::TooLarge));
        }
        bytes.extend_from_slice(&chunk);
        if shown.elapsed() >= Duration::from_millis(200) {
            progress(bytes.len() as u64, total);
            shown = std::time::Instant::now();
        }
    }
    progress(bytes.len() as u64, total);
    std::fs::create_dir_all(root)?;
    let stage = root.join(staging_name(purpose));
    let _ = std::fs::remove_dir_all(&stage);
    let zipped = artifact.ends_with(".zip");
    let unpack_to = stage.clone();
    let unpacked = tokio::task::spawn_blocking(move || unpack(&bytes, zipped, &unpack_to))
        .await
        .map_err(|e| InstallError::ArchiveText(e.to_string()))?;
    let result = match unpacked {
        Ok(()) => adopt(root, &stage).await,
        Err(e) => Err(e),
    };
    let _ = std::fs::remove_dir_all(&stage);
    result
}

/// `staging-<purpose>-<process id>`: [`collect`] leaves this run's alone.
fn staging_name(purpose: &str) -> String {
    let purpose: String = purpose.chars().filter(char::is_ascii_alphanumeric).collect();
    format!("staging-{purpose}-{}", std::process::id())
}

/// Checks Spotify's signature (Windows), runs the unpacked executable's
/// `--version`, then moves the folder to its digest's name.
async fn adopt(root: &Path, stage: &Path) -> Result<Build, InstallError> {
    let exe = stage.join(EXE);
    // Nothing unpacked runs before Windows confirmed it is Spotify's.
    #[cfg(windows)]
    {
        let path = exe.clone();
        tokio::task::spawn_blocking(move || signature::check(&path))
            .await
            .map_err(|e| InstallError::Io(std::io::Error::other(e)))?
            .map_err(InstallError::Signature)?;
    }
    let text = version_of(&exe).await.ok_or(InstallError::WontRun)?;
    let (version, built) = parse_version(&text).ok_or(InstallError::WontRun)?;
    let digest = digest_of(&exe)?;
    let build = Build {
        version,
        built,
        checked: now(),
        digest,
    };
    if !build.usable(now()) {
        return Err(InstallError::Expired);
    }
    let dest = root.join(&build.digest);
    if dest.exists() {
        // The same build is already there (the folder's name is its hash).
        if digest_of(&build.exe(root)).ok().as_deref() != Some(build.digest.as_str()) {
            let _ = std::fs::remove_dir_all(&dest);
            rename_patiently(stage, &dest).await?;
        }
    } else {
        rename_patiently(stage, &dest).await?;
    }
    Ok(build)
}

/// A folder holding an executable that just ran (or that the antivirus is
/// scanning, being new) cannot be renamed for a moment on Windows.
async fn rename_patiently(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut tries = 0;
    loop {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if tries < 40 && e.kind() == std::io::ErrorKind::PermissionDenied => {
                tries += 1;
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// What `exe --version` prints, within ten seconds.
pub async fn version_of(exe: &Path) -> Option<String> {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.arg("--version").kill_on_drop(true).stdin(std::process::Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(crate::daemon::NO_WINDOW);
    let out = tokio::time::timeout(Duration::from_secs(10), cmd.output()).await.ok()?.ok()?;
    if !out.status.success() {
        return None;
    }
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push(' ');
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Some(text)
}

/// An archive entry worth keeping: the executable and Spotify's notices, by
/// their bare names; anything with a path that climbs out is refused.
fn wanted(path: &Path) -> Result<Option<&'static str>, InstallError> {
    if !path.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir)) {
        return Err(InstallError::Archive(SoloistArchiveFault::PathOutside));
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    if name.contains('\\') || name.contains(':') {
        return Err(InstallError::Archive(SoloistArchiveFault::PathOutside));
    }
    Ok([EXE, "THIRD_PARTY_LICENSES.txt", "CHANGELOG.md", "LICENSE.txt"]
        .into_iter()
        .find(|k| *k == name))
}

fn unpack(bytes: &[u8], zipped: bool, dest: &Path) -> Result<(), InstallError> {
    std::fs::create_dir_all(dest)?;
    let mut seen: Vec<&'static str> = Vec::new();
    let mut total = 0u64;
    let mut save = |name: &'static str, size: u64, reader: &mut dyn Read| -> Result<(), InstallError> {
        total += size;
        if total > UNPACKED_LIMIT || seen.contains(&name) {
            return Err(InstallError::Archive(SoloistArchiveFault::SizeOrRepeat));
        }
        seen.push(name);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dest.join(name))?;
        let written = std::io::copy(&mut reader.take(size + 1), &mut file)?;
        if written != size {
            return Err(InstallError::Archive(SoloistArchiveFault::Truncated));
        }
        file.sync_all()?;
        Ok(())
    };
    if zipped {
        let mut archive =
            zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| InstallError::ArchiveText(e.to_string()))?;
        if archive.len() > MAX_ENTRIES {
            return Err(InstallError::Archive(SoloistArchiveFault::TooManyFiles));
        }
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| InstallError::ArchiveText(e.to_string()))?;
            if entry.is_symlink() {
                return Err(InstallError::Archive(SoloistArchiveFault::Symlink));
            }
            let Some(path) = entry.enclosed_name() else {
                return Err(InstallError::Archive(SoloistArchiveFault::PathOutside));
            };
            if entry.is_file()
                && let Some(name) = wanted(&path)?
            {
                let size = entry.size();
                save(name, size, &mut entry)?;
            }
        }
    } else {
        #[cfg(unix)]
        {
            let decoded = flate2::read::GzDecoder::new(bytes).take(UNPACKED_LIMIT + 1);
            let mut archive = tar::Archive::new(decoded);
            for (i, entry) in archive.entries()?.enumerate() {
                if i >= MAX_ENTRIES {
                    return Err(InstallError::Archive(SoloistArchiveFault::TooManyFiles));
                }
                let mut entry = entry?;
                let kind = entry.header().entry_type();
                if !(kind.is_file() || kind.is_dir()) {
                    return Err(InstallError::Archive(SoloistArchiveFault::Symlink));
                }
                let path = entry.path()?.into_owned();
                if kind.is_file()
                    && let Some(name) = wanted(&path)?
                {
                    let size = entry.size();
                    save(name, size, &mut entry)?;
                }
            }
        }
        #[cfg(not(unix))]
        return Err(InstallError::Unsupported);
    }
    if !seen.contains(&EXE) {
        return Err(InstallError::Archive(SoloistArchiveFault::NoExe));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dest.join(EXE), std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Whether a name of the signer's certificate (its display name or its
/// Organization) is exactly Spotify's, «Spotify AB» (the Windows builds are
/// signed `CN=Spotify AB, O=Spotify AB, L=Stockholm, C=SE`).
#[cfg_attr(not(windows), allow(dead_code))]
fn spotify_signer(name: &str) -> bool {
    name.trim().eq_ignore_ascii_case("Spotify AB")
}

/// Authenticode: the executable carries a valid signature that chains to a
/// root Windows trusts, and its signer is Spotify.
#[cfg(windows)]
#[allow(unsafe_code)]
mod signature {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows::Win32::Foundation::HWND;
    use windows::Win32::Security::Cryptography::{
        CERT_NAME_ATTR_TYPE, CERT_NAME_SIMPLE_DISPLAY_TYPE, CertGetNameStringW, szOID_ORGANIZATION_NAME,
    };
    use windows::Win32::Security::WinTrust::{
        WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO, WTD_CHOICE_FILE,
        WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE, WTHelperGetProvSignerFromChain,
        WTHelperProvDataFromStateData, WinVerifyTrust,
    };

    pub fn check(exe: &Path) -> Result<(), String> {
        let path: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut file = WINTRUST_FILE_INFO {
            cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: windows_core::PCWSTR(path.as_ptr()),
            ..Default::default()
        };
        let mut data = WINTRUST_DATA {
            cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
            dwUIChoice: WTD_UI_NONE,
            // Revocation lists would need the network; the chain and the
            // signer are checked offline.
            fdwRevocationChecks: WTD_REVOKE_NONE,
            dwUnionChoice: WTD_CHOICE_FILE,
            Anonymous: WINTRUST_DATA_0 { pFile: &raw mut file },
            dwStateAction: WTD_STATEACTION_VERIFY,
            ..Default::default()
        };
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        // SAFETY: `data` points at `file`, which points at `path`; all live
        // until the state is closed below.
        let status = unsafe { WinVerifyTrust(HWND::default(), &raw mut action, (&raw mut data).cast()) };
        let signer = if status == 0 { unsafe { signer_name(&data) } } else { None };
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        unsafe { WinVerifyTrust(HWND::default(), &raw mut action, (&raw mut data).cast()) };
        if status != 0 {
            return Err(nikgob_core::i18n::backend::SoloistSignatureFault::Failed(status as u32));
        }
        match signer {
            Some((name, org)) if super::spotify_signer(&name) || org.as_deref().is_some_and(super::spotify_signer) => {
                Ok(())
            }
            Some((name, _)) => Err(nikgob_core::i18n::backend::SoloistSignatureFault::SignedBy(name)),
            None => Err(nikgob_core::i18n::backend::SoloistSignatureFault::Unknown),
        }
    }

    /// The display name and the Organization of the first signer's
    /// certificate.
    ///
    /// SAFETY: `data` was verified with `WTD_STATEACTION_VERIFY` and its state
    /// is not closed yet.
    unsafe fn signer_name(data: &WINTRUST_DATA) -> Option<(String, Option<String>)> {
        unsafe {
            let provider = WTHelperProvDataFromStateData(data.hWVTStateData);
            if provider.is_null() {
                return None;
            }
            let signer = WTHelperGetProvSignerFromChain(provider, 0, false, 0);
            if signer.is_null() || (*signer).csCertChain == 0 || (*signer).pasCertChain.is_null() {
                return None;
            }
            let cert = (*(*signer).pasCertChain).pCert;
            if cert.is_null() {
                return None;
            }
            let name_of = |kind: u32, para: Option<*const core::ffi::c_void>| {
                let mut buf = [0u16; 256];
                let n = CertGetNameStringW(cert, kind, 0, para, Some(&mut buf)) as usize;
                // The count includes the closing zero.
                let name = String::from_utf16_lossy(&buf[..n.saturating_sub(1).min(buf.len())]);
                (!name.trim().is_empty()).then_some(name)
            };
            let name = name_of(CERT_NAME_SIMPLE_DISPLAY_TYPE, None)?;
            let org = name_of(CERT_NAME_ATTR_TYPE, Some(szOID_ORGANIZATION_NAME.0.cast()));
            Some((name, org))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn only_spotify_signs_soloist() {
        assert!(spotify_signer("Spotify AB"));
        assert!(spotify_signer("spotify ab"));
        assert!(!spotify_signer("Spotify, Inc."));
        assert!(!spotify_signer("Spotify AB Fake"));
        assert!(!spotify_signer("Spotifyy AB"));
        assert!(!spotify_signer("Not Spotify AB"));
        assert!(!spotify_signer(""));
    }

    #[test]
    fn staging_folders_of_this_run_are_left_to_their_downloads() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let renew = staging_name("renew");
        let download = staging_name("download");
        assert_ne!(renew, download);
        for d in [renew.as_str(), download.as_str(), "staging-renew-1", "staging-1"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        collect(root, &[]);
        assert!(root.join(&renew).exists() && root.join(&download).exists());
        assert!(!root.join("staging-renew-1").exists() && !root.join("staging-1").exists());
    }

    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(bytes).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn the_version_line_gives_the_build_date() {
        let (v, b) = parse_version("soloist 1.4.2 build 1786000000 (2026-08-05) windows x86_64").unwrap();
        assert_eq!((v.as_str(), b), ("1.4.2", 1_786_000_000));
        assert!(parse_version("soloist 1.4.2").is_none());
        assert!(parse_version("soloist 1.4.2 build soon").is_none());
        assert!(parse_version("something else build 1786000000").is_none());
    }

    #[test]
    fn expiry_and_renewal_follow_the_build_date() {
        let built = 1_786_000_000;
        let b = Build { version: "1".into(), built, checked: built, digest: "a".repeat(64) };
        assert!(b.usable(built + 89 * DAY));
        assert!(!b.usable(built + 90 * DAY), "expired after 90 days");
        assert!(!b.usable(built - 2 * DAY), "a clock before the build date");
        assert!(!b.renewal_due(built + 59 * DAY));
        assert!(b.renewal_due(built + 60 * DAY), "the last 30 days");
        let asked = Build { checked: built + 60 * DAY, ..b.clone() };
        assert!(!asked.renewal_due(built + 60 * DAY + 3600), "once a day");
        assert!(asked.renewal_due(built + 61 * DAY));
    }

    #[test]
    fn only_the_player_and_notices_are_unpacked() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("b");
        let bytes = zip_of(&[(EXE, b"exe"), ("THIRD_PARTY_LICENSES.txt", b"l"), ("extra.dll", b"x")]);
        unpack(&bytes, true, &dest).unwrap();
        assert_eq!(std::fs::read(dest.join(EXE)).unwrap(), b"exe");
        assert!(dest.join("THIRD_PARTY_LICENSES.txt").exists());
        assert!(!dest.join("extra.dll").exists());
    }

    #[test]
    fn archives_without_the_player_or_with_bad_paths_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert!(unpack(&zip_of(&[("README", b"x")]), true, &dir.path().join("a")).is_err());
        assert!(unpack(&zip_of(&[(EXE, b"a"), (&format!("x/{EXE}"), b"b")]), true, &dir.path().join("b")).is_err());
        for bad in ["../soloist", "C:\\soloist", "a/../../soloist"] {
            assert!(wanted(Path::new(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn pointers_check_the_executable() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let exe = b"the player";
        let digest = format!("{:x}", Sha256::digest(exe));
        let b = Build { version: "1".into(), built: now() - DAY, checked: now(), digest: digest.clone() };
        std::fs::create_dir_all(root.join(&digest)).unwrap();
        std::fs::write(b.exe(root), exe).unwrap();
        write_pointer(root, CURRENT, &b).unwrap();
        assert_eq!(read_pointer(root, CURRENT), Some(b.clone()));
        std::fs::write(b.exe(root), b"changed").unwrap();
        assert_eq!(read_pointer(root, CURRENT), None, "a changed executable is not run");
        std::fs::create_dir_all(root.join("staging-1")).unwrap();
        std::fs::create_dir_all(root.join("b".repeat(64))).unwrap();
        collect(root, &[&digest]);
        assert!(root.join(&digest).exists());
        assert!(!root.join("staging-1").exists());
        assert!(!root.join("b".repeat(64)).exists());
    }
}
