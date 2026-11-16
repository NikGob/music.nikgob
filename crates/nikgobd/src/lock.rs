//! One nikgobd per data dir: an exclusive lock on `nikgobd.lock`, held
//! while it runs and let go by the system when it ends, however it ends.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::Path;

/// The lock, held while this lives.
#[derive(Debug)]
pub struct DataLock {
    _file: File,
}

impl DataLock {
    /// Takes the lock at `path`, or says who holds it.
    pub fn acquire(path: &Path) -> Result<Self, String> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                let pid = std::fs::read_to_string(path).unwrap_or_default();
                let pid = pid.trim();
                let who = if pid.is_empty() { String::new() } else { format!(" (process {pid})") };
                return Err(format!("another nikgobd runs on this data dir{who}: {} is locked", path.display()));
            }
            Err(TryLockError::Error(e)) => return Err(format!("cannot lock {}: {e}", path.display())),
        }
        // Who holds it, for the message of the next one.
        let _ = file.set_len(0);
        let _ = (&file).write_all(format!("{}\n", std::process::id()).as_bytes());
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_lock_on_one_dir_is_refused_until_the_first_goes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nikgobd.lock");
        let first = DataLock::acquire(&path).expect("the first takes it");
        let second = DataLock::acquire(&path).expect_err("the second is refused");
        assert!(second.contains("another nikgobd"), "{second}");
        drop(first);
        DataLock::acquire(&path).expect("free again");
    }
}
