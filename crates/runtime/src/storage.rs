//! «Загрузки»' header: how much space the music takes and what is left of
//! the disks it is on ([`v::StorageView`]).
//!
//! Counted on a thread of its own, never on the loop: the library's files
//! from the sizes the import recorded (a file without one is looked at on
//! disk, metadata only), the lossless originals by a walk of their folder
//! (only it), the free space by one call per disk. Counted again a little
//! after the library changes (an import, a download, a delete: they come in
//! bursts) and now and then for the free space; one count at a time, the
//! newest wins.

use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use nikgob_library::Library;
use nikgob_view as v;

use crate::{Later, Worker};

/// After a change of the library, counted once it has been quiet this long.
const QUIET: Duration = Duration::from_secs(5);
/// Changes that never go quiet (files pulled one after another): counted
/// at least this often while they come.
const BUSY: Duration = Duration::from_secs(30);
/// The free space changes by itself (downloads, other programs): counted
/// again this often anyway.
const REFRESH: Duration = Duration::from_secs(10 * 60);

/// The worker's side of the count.
pub(crate) struct Storage {
    /// The library changed since the count (and when it last did).
    changed: Option<Instant>,
    /// The first change not counted yet.
    pending: Option<Instant>,
    /// A count is on its way.
    busy: bool,
    /// When the last count was started.
    last: Option<Instant>,
}

impl Default for Storage {
    fn default() -> Self {
        // The first count right after the start.
        let at = Instant::now().checked_sub(QUIET).unwrap_or_else(Instant::now);
        Self { changed: Some(at), pending: Some(at), busy: false, last: None }
    }
}

impl Storage {
    /// Whether a count is due at `now`.
    fn due(&self, now: Instant) -> bool {
        if self.busy {
            return false;
        }
        let changed = self.changed.is_some_and(|at| now.saturating_duration_since(at) >= QUIET)
            || self.pending.is_some_and(|at| now.saturating_duration_since(at) >= BUSY);
        let stale = self.last.is_none_or(|at| now.saturating_duration_since(at) >= REFRESH);
        changed || stale
    }
}

impl Worker {
    /// The library changed: counted again once it is quiet.
    pub(crate) fn storage_changed(&mut self) {
        let now = Instant::now();
        self.storage.changed = Some(now);
        self.storage.pending.get_or_insert(now);
    }

    /// Once a second: starts a count when one is due.
    pub(crate) fn storage_poll(&mut self) {
        let now = Instant::now();
        if !self.storage.due(now) {
            return;
        }
        let (library_dir, masters_dir) = {
            let s = self.state.read();
            (s.prefs.library_dir.clone(), s.prefs.masters_dir.clone())
        };
        let root = self.res.root().to_owned();
        let library_dir = if library_dir.as_os_str().is_empty() { root.clone() } else { library_dir };
        let lib = self.lib.clone();
        let later = self.later_tx.clone();
        self.storage.busy = true;
        self.storage.changed = None;
        self.storage.pending = None;
        self.storage.last = Some(now);
        let spawned = std::thread::Builder::new().name("nikgob-storage".into()).spawn(move || {
            let view = count(&lib, &root, &library_dir, &masters_dir);
            let _ = later.send(Later::new("the music's size", move |w| w.storage_counted(view)));
        });
        if let Err(e) = spawned {
            log::warn!("storage: the count did not start: {e}");
            self.storage.busy = false;
        }
    }

    fn storage_counted(&mut self, view: Option<v::StorageView>) {
        self.storage.busy = false;
        let Some(view) = view else { return };
        if self.state.read().downloads.storage == view {
            return;
        }
        self.publish(|s| s.downloads.storage = view);
    }
}

/// The count: the library's files (`root` resolves their locators), the
/// originals in `masters_dir`, the disks of `library_dir` and `masters_dir`.
/// None when the library could not be read.
fn count(lib: &Library, root: &Path, library_dir: &Path, masters_dir: &Path) -> Option<v::StorageView> {
    let started = Instant::now();
    let sizes = match lib.local_file_sizes() {
        Ok(s) => s,
        Err(e) => {
            log::warn!("storage: the library's files could not be read: {e}");
            return None;
        }
    };
    let unknown: u64 = sizes.unknown.iter().filter_map(|l| file_len(&resolve(root, l))).sum();
    let library = sizes.known.saturating_add(unknown);
    // The originals inside the library's folder are not its files either:
    // the walk counts them wherever they are.
    let masters = if masters_dir.as_os_str().is_empty() { 0 } else { folder_size(masters_dir) };
    let mut places = vec![library_dir.to_owned()];
    if masters > 0 {
        places.push(masters_dir.to_owned());
    }
    // Files kept outside the library's folder may be on other disks: their
    // folders, each once.
    places.extend(outside_folders(&sizes.absolute, library_dir, masters_dir));
    let disks = disks_of(&places);
    log::debug!(
        "storage: {} files ({} looked at), originals {masters} B, {} disks, in {:?}",
        library,
        sizes.unknown.len(),
        disks.len(),
        started.elapsed()
    );
    Some(v::StorageView { known: true, library, masters, disks })
}

/// Absolute path of a source locator (relative ones are under the library).
fn resolve(root: &Path, locator: &str) -> PathBuf {
    let p = Path::new(locator);
    if p.is_absolute() { p.to_owned() } else { root.join(p) }
}

fn file_len(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().filter(|m| m.is_file()).map(|m| m.len())
}

/// The bytes of the audio under `dir`, the disc images (APE, WavPack) a cue
/// sheet cuts included (the sidecars beside the originals and any other
/// file are not music, nor are dot names: `.nikgob-backup`'s copies of the
/// database and the like). Links are not followed.
fn folder_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    let mut todo = vec![dir.to_owned()];
    while let Some(d) = todo.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let Ok(kind) = e.file_type() else { continue };
            if kind.is_dir() {
                todo.push(e.path());
            } else if kind.is_file()
                && (nikgob_import::fsutil::is_audio_file(&e.path())
                    || nikgob_import::fsutil::is_sheet_image_file(&e.path()))
            {
                total = total.saturating_add(e.metadata().map(|m| m.len()).unwrap_or(0));
            }
        }
    }
    total
}

/// The folders of the files at `absolute` that are not under the library
/// or the originals, each once.
fn outside_folders(absolute: &[String], library_dir: &Path, masters_dir: &Path) -> Vec<PathBuf> {
    let mut folders = std::collections::BTreeSet::new();
    for l in absolute {
        let p = Path::new(l);
        if p.starts_with(library_dir) || (!masters_dir.as_os_str().is_empty() && p.starts_with(masters_dir)) {
            continue;
        }
        if let Some(dir) = p.parent() {
            folders.insert(dir.to_owned());
        }
    }
    folders.into_iter().collect()
}

/// The disks `places` are on, each once (by [`disk_key`]), in the order of
/// the places and named by the first of each; one free-space call a disk.
fn disks_of(places: &[PathBuf]) -> Vec<v::DiskSpace> {
    let mut seen: Vec<String> = Vec::new();
    let mut disks = Vec::new();
    for p in places {
        let Some(at) = existing(p) else { continue };
        let Some(key) = disk_key(&at) else { continue };
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        if let Some((free, total)) = space(&at) {
            disks.push(v::DiskSpace { name: disk_name(p), free, total });
        }
    }
    disks
}

/// `path` or the nearest folder above it that exists (a folder not made
/// yet is on its parent's disk).
fn existing(path: &Path) -> Option<PathBuf> {
    path.ancestors().find(|p| !p.as_os_str().is_empty() && p.exists()).map(Path::to_owned)
}

/// The drive of a path on Windows (`C:`, a share's name).
fn drive_of(path: &Path) -> Option<String> {
    match path.components().next()? {
        Component::Prefix(p) => {
            use std::path::Prefix;
            Some(match p.kind() {
                Prefix::Disk(l) | Prefix::VerbatimDisk(l) => format!("{}:", char::from(l).to_ascii_uppercase()),
                Prefix::UNC(_, share) | Prefix::VerbatimUNC(_, share) => share.to_string_lossy().into_owned(),
                _ => p.as_os_str().to_string_lossy().into_owned(),
            })
        }
        _ => None,
    }
}

/// How the header names the disk of `place` when there are several: its
/// drive on Windows, else the folder's own name.
fn disk_name(place: &Path) -> String {
    drive_of(place)
        .or_else(|| place.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| place.to_string_lossy().into_owned())
}

/// What tells two disks apart.
#[cfg(unix)]
fn disk_key(at: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(at).ok().map(|m| m.dev().to_string())
}

#[cfg(not(unix))]
fn disk_key(at: &Path) -> Option<String> {
    volume_of(at).or_else(|| Some(at.to_string_lossy().into_owned()))
}

/// The volume of a path on Windows as a key: the drive (`c:`), or the
/// whole share (`\\server\share`: shares of the same name on two servers
/// are two disks).
#[cfg_attr(unix, allow(dead_code))]
fn volume_of(path: &Path) -> Option<String> {
    use std::path::Prefix;
    let Component::Prefix(p) = path.components().next()? else { return None };
    Some(match p.kind() {
        Prefix::Disk(l) | Prefix::VerbatimDisk(l) => format!("{}:", char::from(l).to_ascii_lowercase()),
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
            format!(r"\\{}\{}", server.to_string_lossy(), share.to_string_lossy()).to_lowercase()
        }
        _ => p.as_os_str().to_string_lossy().to_lowercase(),
    })
}

/// Free bytes (for this user) of the disk `at` is on, `at` existing (the
/// copies from the other devices stop short of a full disk).
pub fn free_bytes(at: &Path) -> Option<u64> {
    space(at).map(|(free, _)| free)
}

/// Free (for this user) and total bytes of the disk `at` is on.
#[cfg(unix)]
fn space(at: &Path) -> Option<(u64, u64)> {
    let s = rustix::fs::statvfs(at).ok()?;
    Some((s.f_bavail.saturating_mul(s.f_frsize), s.f_blocks.saturating_mul(s.f_frsize)))
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn space(at: &Path) -> Option<(u64, u64)> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = at.as_os_str().encode_wide().chain(Some(0)).collect();
    let (mut free, mut total, mut total_free) = (0u64, 0u64, 0u64);
    // SAFETY: `wide` is a NUL-terminated UTF-16 path and the three out
    // values live through the call, which only writes them.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, &mut total_free) };
    (ok != 0).then_some((free, total))
}

#[cfg(not(any(unix, windows)))]
fn space(_: &Path) -> Option<(u64, u64)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_core::model::{Artist, ArtistCredit, Codec, Source, SourceKind, Track};

    #[test]
    fn a_count_is_due_after_a_quiet_change_or_when_stale() {
        let now = Instant::now();
        let mut s = Storage::default();
        assert!(s.due(now), "the first count at once");
        s.busy = true;
        assert!(!s.due(now), "one at a time");
        s = Storage { changed: None, pending: None, busy: false, last: Some(now) };
        assert!(!s.due(now + Duration::from_secs(60)));
        assert!(s.due(now + REFRESH));
        s.changed = Some(now);
        s.pending = Some(now);
        assert!(!s.due(now + Duration::from_secs(2)), "an import still landing");
        assert!(s.due(now + QUIET));
    }

    #[test]
    fn changes_that_never_go_quiet_are_counted_now_and_then() {
        let now = Instant::now();
        let mut s = Storage { changed: None, pending: Some(now), busy: false, last: Some(now) };
        // A pulled file every few seconds: never quiet for long.
        s.changed = Some(now + BUSY - Duration::from_secs(1));
        assert!(!s.due(now + BUSY - Duration::from_secs(1)));
        assert!(s.due(now + BUSY), "counted while the files still come");
    }

    #[test]
    fn the_count_sums_recorded_sizes_files_without_one_and_the_originals() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Music");
        let masters = dir.path().join("FLAC");
        std::fs::create_dir_all(root.join("A")).unwrap();
        std::fs::create_dir_all(masters.join("ab")).unwrap();
        std::fs::write(root.join("A").join("3.flac"), vec![0u8; 700]).unwrap();
        std::fs::write(masters.join("ab").join("x.flac"), vec![0u8; 5_000]).unwrap();
        std::fs::write(masters.join("ab").join("x.json"), vec![0u8; 300]).unwrap();

        let lib = Library::open_in_memory().unwrap();
        let a = lib.upsert_artist(&Artist::new("A")).unwrap();
        let credit = ArtistCredit::single(a.name.clone(), Some(a.id));
        for (n, locator, size) in [(1, "A/1.flac", Some(1_000)), (2, "A/2.flac", Some(2_000)), (3, "A/3.flac", None)] {
            let t = lib.upsert_track(&Track::new(format!("{n}"), credit.clone())).unwrap();
            let mut s = Source::new(t.id, SourceKind::LocalFile, locator, Codec::Flac);
            s.size_bytes = size;
            lib.add_source(&s).unwrap();
        }
        let view = count(&lib, &root, &root, &masters).unwrap();
        assert!(view.known);
        assert_eq!(view.library, 3_700);
        assert_eq!(view.masters, 5_000);
        assert_eq!(view.music(), 8_700);
        // Both folders are on the temp folder's disk: one disk.
        assert_eq!(view.disks.len(), 1, "{:?}", view.disks);
        assert!(view.disks[0].total >= view.disks[0].free && view.disks[0].total > 0);
        // A masters folder not made yet: nothing, and no second disk.
        let view = count(&lib, &root, &root, &dir.path().join("none")).unwrap();
        assert_eq!((view.masters, view.disks.len()), (0, 1));
    }

    /// The originals' folder keeps out its dot names: `.nikgob-backup` is
    /// the database's own, not music, and a dot file or folder inside is
    /// no original either. Only audio counts — a cue sheet's APE or WavPack
    /// image too: covers, sheets and anything else beside them are not.
    #[test]
    fn the_originals_folder_count_skips_dot_names_and_other_files() {
        let dir = tempfile::tempdir().unwrap();
        let masters = dir.path().join("FLAC");
        std::fs::create_dir_all(masters.join("ab")).unwrap();
        std::fs::create_dir_all(masters.join(".nikgob-backup")).unwrap();
        std::fs::create_dir_all(masters.join("ab").join(".hidden")).unwrap();
        std::fs::write(masters.join("ab").join("x.flac"), vec![0u8; 5_000]).unwrap();
        std::fs::write(masters.join("ab").join("disc.ape"), vec![0u8; 2_000]).unwrap();
        std::fs::write(masters.join("ab").join("disc.wv"), vec![0u8; 1_000]).unwrap();
        std::fs::write(masters.join("ab").join(".y.flac"), vec![0u8; 700]).unwrap();
        std::fs::write(masters.join("ab").join(".hidden").join("z.flac"), vec![0u8; 900]).unwrap();
        std::fs::write(masters.join("ab").join("cover.jpg"), vec![0u8; 800]).unwrap();
        std::fs::write(masters.join("ab").join("x.cue"), vec![0u8; 100]).unwrap();
        std::fs::write(masters.join("ab").join("x.json"), vec![0u8; 300]).unwrap();
        std::fs::write(masters.join("library-1.sqlite"), vec![0u8; 4_000]).unwrap();
        std::fs::write(masters.join(".nikgob-backup").join("library-1.sqlite"), vec![0u8; 12_000]).unwrap();
        assert_eq!(folder_size(&masters), 8_000);
    }

    #[test]
    fn files_outside_the_library_bring_their_folders() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("Music");
        let other = dir.path().join("Other");
        let abs = |p: PathBuf| p.to_string_lossy().into_owned();
        let files = [abs(lib.join("A").join("1.flac")), abs(other.join("x").join("2.flac")), abs(other.join("x").join("3.flac"))];
        assert_eq!(outside_folders(&files, &lib, Path::new("")), vec![other.join("x")]);
        // On the same disk as the library: one disk still.
        std::fs::create_dir_all(other.join("x")).unwrap();
        std::fs::create_dir_all(&lib).unwrap();
        assert_eq!(disks_of(&[lib.clone(), other.join("x")]).len(), 1);
    }

    #[test]
    fn shares_of_one_name_on_two_servers_are_two_disks() {
        let a = volume_of(Path::new(r"\\server-a\music\x.flac"));
        let b = volume_of(Path::new(r"\\server-b\music\x.flac"));
        #[cfg(windows)]
        {
            assert_eq!(a.as_deref(), Some(r"\\server-a\music"));
            assert_ne!(a, b);
            assert_eq!(volume_of(Path::new(r"C:\Music")), volume_of(Path::new(r"\\?\c:\x")));
        }
        #[cfg(unix)]
        assert_eq!((a, b), (None, None));
    }

    #[test]
    fn disks_are_named_by_drive_or_folder() {
        assert_eq!(disk_name(Path::new("Music")), "Music");
        #[cfg(windows)]
        {
            assert_eq!(disk_name(Path::new(r"E:\Music\FLAC")), "E:");
            assert_eq!(disk_name(Path::new(r"\\?\c:\Music")), "C:");
        }
        #[cfg(unix)]
        assert_eq!(disk_name(Path::new("/mnt/data/FLAC")), "FLAC");
    }
}
