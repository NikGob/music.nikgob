//! The files of the mesh in the data dir (`mesh/`): this instance's key
//! (`node.key`), the ring's key on the device that made the ring
//! (`ring.key`), the ring itself (`ring.json`) and, on a headless instance,
//! the pairing code it shows (`pair-code`). Secrets are written with mode
//! 0600 on unix, every file atomically (a temporary file renamed over).

use std::io::Write;
use std::path::{Path, PathBuf};

use iroh::SecretKey;

/// Where the mesh keeps its files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeshPaths {
    pub dir: PathBuf,
}

impl MeshPaths {
    /// `<data_dir>/mesh`.
    pub fn of(data_dir: &Path) -> Self {
        Self { dir: data_dir.join("mesh") }
    }

    pub fn node_key(&self) -> PathBuf {
        self.dir.join("node.key")
    }

    pub fn ring_key(&self) -> PathBuf {
        self.dir.join("ring.key")
    }

    pub fn ring(&self) -> PathBuf {
        self.dir.join("ring.json")
    }

    pub fn pair_code(&self) -> PathBuf {
        self.dir.join("pair-code")
    }
}

/// 32 random bytes from the system.
pub(crate) fn random32() -> std::io::Result<[u8; 32]> {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).map_err(|e| std::io::Error::other(format!("no randomness: {e}")))?;
    Ok(b)
}

/// Writes `bytes` to `path` through a temporary file beside it (mode 0600
/// on unix when `private`), then renames it over.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8], private: bool) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let result = (|| {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        if private {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        #[cfg(not(unix))]
        let _ = private;
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// A key file: 32 raw bytes.
pub(crate) fn read_key(path: &Path) -> std::io::Result<Option<SecretKey>> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let arr: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{}: not a key", path.display())))?;
            Ok(Some(SecretKey::from_bytes(&arr)))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// A new random key, written to `path` (0600).
pub(crate) fn new_key(path: &Path) -> std::io::Result<SecretKey> {
    let bytes = random32()?;
    write_atomic(path, &bytes, true)?;
    Ok(SecretKey::from_bytes(&bytes))
}

/// This instance's key, made on the first start.
pub fn node_key(paths: &MeshPaths) -> std::io::Result<SecretKey> {
    match read_key(&paths.node_key())? {
        Some(k) => Ok(k),
        None => {
            let k = new_key(&paths.node_key())?;
            log::info!("mesh: a new node key, id {}", k.public());
            Ok(k)
        }
    }
}

/// This instance is no longer in its ring: the ring's files go (the node
/// key stays: it is still this device).
pub(crate) fn wipe_ring(paths: &MeshPaths) {
    for p in [paths.ring(), paths.ring_key()] {
        match std::fs::remove_file(&p) {
            Ok(()) => log::info!("mesh: removed {}", p.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("mesh: cannot remove {}: {e}", p.display()),
        }
    }
}

/// The code a headless instance shows, for `nikgobd pair-code` (0600);
/// `None` removes the file.
pub(crate) fn write_code(paths: &MeshPaths, code: Option<&str>) {
    let p = paths.pair_code();
    let r = match code {
        Some(c) => write_atomic(&p, format!("{c}\n").as_bytes(), true),
        None => match std::fs::remove_file(&p) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        },
    };
    if let Err(e) = r {
        log::warn!("mesh: {}: {e}", p.display());
    }
}

/// The code a running instance of `data_dir` shows (`nikgobd pair-code`):
/// `None` when it is in a ring or does not run.
pub fn read_pair_code(data_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(MeshPaths::of(data_dir).pair_code()).ok()?;
    let code = text.trim();
    (code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit())).then(|| code.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_node_key_is_made_once_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let paths = MeshPaths::of(dir.path());
        let a = node_key(&paths).unwrap();
        let b = node_key(&paths).unwrap();
        assert_eq!(a.public(), b.public());
        assert_eq!(std::fs::read(paths.node_key()).unwrap().len(), 32);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(paths.node_key()).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        std::fs::write(paths.node_key(), b"short").unwrap();
        assert!(node_key(&paths).is_err(), "a broken key is an error, never replaced silently");
    }

    #[test]
    fn the_code_file_is_written_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let paths = MeshPaths::of(dir.path());
        assert_eq!(read_pair_code(dir.path()), None);
        write_code(&paths, Some("042917"));
        assert_eq!(read_pair_code(dir.path()).as_deref(), Some("042917"));
        write_code(&paths, None);
        assert_eq!(read_pair_code(dir.path()), None);
        write_code(&paths, None);
    }
}
