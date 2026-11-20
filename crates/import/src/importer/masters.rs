use super::*;
use super::commit::Replaced;

/// One writer of the masters folder at a time: the importer placing a
/// master, a move of the whole folder, the cleanup of a stopped torrent
/// taking its masters out of Downloads, a purge or a wrong file taking a
/// master away. Shared by every importer (a new
/// configuration makes a new one). Only blocking threads take it, never the
/// player loop: a holder may copy and hash a whole disc image.
pub fn master_writes() -> MutexGuard<'static, ()> {
    static WRITES: Mutex<()> = Mutex::new(());
    lock(&WRITES)
}

/// What one committed file changed, so a cancelled job can take it back.
pub(super) struct Undo {
    pub(super) track: TrackId,
    /// The track as it was before (`None`: the import made it).
    pub(super) before: Option<Track>,
    pub(super) source: SourceId,
    /// The file it put in the library.
    pub(super) dest: PathBuf,
    /// A master file it put in the masters folder (not one that was there).
    pub(super) master_file: Option<PathBuf>,
    pub(super) master_hash: Option<String>,
    /// The track's master record before this import registered its own
    /// (`None`: it had none), put back when [`Self::master_hash`] is set.
    pub(super) master_before: Option<Master>,
    /// The master record this import wrote: only while the track's record
    /// is still this one is it taken back (a later import of the same track
    /// may have registered its own since).
    pub(super) master_registered: Option<Master>,
    /// The master's sidecar as it was before this import wrote it (`None`:
    /// there was none); a shared image's sidecar names earlier tracks too.
    pub(super) sidecar_before: Option<Vec<u8>>,
    /// The sidecar as this import wrote it: only while it is still this is
    /// the earlier one put back.
    pub(super) sidecar_written: Option<Vec<u8>>,
    pub(super) replaced: Replaced,
}

/// Takes back what a cancelled job committed, the last file first: its
/// files go to the Trash (never deleted: an Inbox file may have been the
/// only copy), its rows go, a file it replaced comes back. The master
/// this import wrote goes only when no master row keeps it any more
/// (another import may have taken the very file as its own master): its
/// row and that look happen under the masters' lock. Returns how many
/// files were taken back.
pub(super) fn roll_back(inner: &Inner, journal: Vec<Undo>) -> usize {
    let (cfg, lib) = (&inner.cfg, &inner.library);
    let trash = |f: &Path| {
        if long_path(f).exists() {
            match trash_file(&cfg.trash_dir, f) {
                Ok(to) => log::info!("import cancelled: {} moved to {}", f.display(), to.display()),
                Err(e) => log::warn!("import cancelled: {} stays: {e}", f.display()),
            }
        }
    };
    let mut n = 0;
    for u in journal.into_iter().rev() {
        trash(&u.dest);
        trash(&u.dest.with_extension("lrc"));
        {
            let _writes = master_writes();
            let masters = cfg.masters_dir();
            if let Some(hash) = &u.master_hash {
                // Only the record this import wrote goes back: one a later
                // import of the track wrote since stays as it is.
                let bare = |m: &Master| Master { replicas: Vec::new(), ..m.clone() };
                let current = lib.get_master(u.track).ok().flatten();
                let ours = current.as_ref().map(bare) == u.master_registered.as_ref().map(bare);
                if ours {
                    let _ = lib.delete_master(u.track);
                    if let Some(m) = &u.master_before
                        && let Err(e) = lib.put_master(m)
                    {
                        log::warn!("import cancelled: the master of {} before is not put back: {e}", u.track);
                    }
                } else {
                    log::info!("import cancelled: the master of {} changed since, it stays", u.track);
                }
                // The sidecar goes back to what it was, while it is still
                // what this import wrote: gone when this import made it and
                // no other master has the file, else as it was (other tracks
                // of an image need it).
                let sidecar = masters.join(format!("{hash}.json"));
                let still_ours = u.sidecar_written.is_some()
                    && std::fs::read(long_path(&sidecar)).ok() == u.sidecar_written;
                match &u.sidecar_before {
                    Some(bytes) if still_ours => {
                        if let Err(e) = master_files::write_whole(&sidecar, bytes) {
                            log::warn!("import cancelled: {} is not put back: {e}", sidecar.display());
                        }
                    }
                    None if still_ours && lib.master_users(hash).unwrap_or(1) == 0 => {
                        let _ = fsutil::remove_file(&sidecar);
                    }
                    _ => {}
                }
            }
            if let Some(file) = &u.master_file {
                let flat = file.parent() == Some(masters.as_path())
                    && u.master_hash.as_deref().is_some_and(|h| file.file_stem().is_some_and(|s| s.to_string_lossy() == h));
                let kept_by_others = if flat {
                    u.master_hash.as_deref().is_none_or(|h| lib.master_users(h).unwrap_or(1) > 0)
                } else {
                    lib.masters_at(&file.to_string_lossy()).unwrap_or(1) > 0
                };
                if kept_by_others {
                    log::info!("import cancelled: {} stays, another master has it", file.display());
                } else {
                    trash(file);
                    // The cue sheet kept with an image master goes with it.
                    if flat {
                        trash(&file.with_extension("cue"));
                    }
                    // The album folder a master kept by name was the first
                    // in goes with it when nothing but its cover is left.
                    if let Some(dir) = file.parent() {
                        fsutil::prune_empty_dirs(dir, &masters);
                    }
                }
            }
        }
        if let Err(e) = lib.forget_source(u.source) {
            log::warn!("import cancelled: the row of {} stays: {e}", u.dest.display());
        }
        u.replaced.undo(lib);
        match &u.before {
            Some(t) => {
                if let Err(e) = lib.put_track_back(t) {
                    log::warn!("import cancelled: {} keeps its new tags: {e}", t.title);
                }
            }
            None => {
                if let Err(e) = lib.forget_new_track(u.track) {
                    log::warn!("import cancelled: track {} stays: {e}", u.track);
                }
            }
        }
        if let Some(dir) = u.dest.parent() {
            fsutil::prune_empty_dirs(dir, &cfg.library_dir);
        }
        n += 1;
    }
    // Albums and artists the job made may be gone: the cache reads again.
    *lock(&inner.store) = Store::new(lib.clone());
    n
}

#[derive(Serialize)]
struct MasterSidecar<'a> {
    track_id: TrackId,
    /// Every track this one file is the master of, when it is several (a
    /// single-file rip of a disc is the master of all its tracks).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    track_ids: Vec<TrackId>,
    codec: &'a Codec,
    file_hash: &'a str,
    size_bytes: u64,
    origin: &'a SourceOrigin,
    /// Where the lossless master is: in the masters folder under its name
    /// (or `{file_hash}.<ext>` for an older one or an image), or a torrent's
    /// payload, which moves into the masters folder when its torrent is
    /// cleaned up: readers of a torrent's must look there and not trust
    /// this field.
    path: String,
    original_name: String,
    /// The cue sheet of a single-file rip, kept as `{file_hash}.cue`, by
    /// the name it came with.
    #[serde(skip_serializing_if = "Option::is_none")]
    cue: Option<String>,
    quality_score: i32,
}

/// The tracks an earlier sidecar of the same file names.
#[derive(serde::Deserialize)]
struct SidecarTracks {
    track_id: TrackId,
    #[serde(default)]
    track_ids: Vec<TrackId>,
}

/// What [`register_master`] did.
pub(super) struct Registered {
    pub(super) master: Master,
    /// The file this call wrote, if it wrote one (for a cancel).
    pub(super) written: Option<PathBuf>,
    /// The sidecar as this call wrote it (a cancel puts the earlier one
    /// back only while it is still this).
    pub(super) sidecar: Option<Vec<u8>>,
}

/// Registers the lossless original as the track's master: torrent payloads
/// stay where they are (they keep seeding); anything else is copied into
/// the masters folder under its name ([`master_files::master_target`]:
/// album artist, album, number and title, the album's `cover.jpg` beside
/// it), never over another file. A file that is a master already (an older
/// `{hash}.<ext>` holding that very file, or one kept by name) is not
/// written again. A track cut out of a single-file rip has the image as its
/// master, shared with the other tracks of the image: the image stays
/// `{hash}.<ext>` with its cue sheet as `{hash}.cue` (a sheet that cannot
/// be kept fails the registration: the sheet is what cuts the image), and
/// is never written over a different file of that name. The master's row
/// is written here, under the masters' lock and durably (a power cut after
/// it keeps it), and only then does an Inbox file go: until the library
/// knows the copy, the Inbox file stays (a failed row takes the copy
/// written now back).
pub(super) fn register_master(
    inner: &Inner,
    a: &Analyzed,
    hash: &str,
    track_id: TrackId,
    id: &Identity,
    cover: Option<&Path>,
) -> Result<Registered> {
    let cfg = &inner.cfg;
    let _writes = master_writes();
    let src = a.master_path();
    let image = a.cue.as_ref().map(|s| &s.image);
    let codec = image.map_or(&a.info.codec, |i| &i.info.codec);
    let size = match image {
        Some(i) => i.size,
        None => fsutil::file_size(src)?,
    };
    let take = src.starts_with(&cfg.inbox_dir);
    let mut written = None;
    let mut file_path = None;
    // A copy written now goes again when what follows fails (the source
    // stays the original).
    let undo_written = |written: &Option<PathBuf>| {
        if let Some(w) = written
            && long_path(src).exists()
        {
            let _ = fsutil::remove_file(w);
        }
    };
    let path = match &a.origin {
        SourceOrigin::Torrent { .. } => src.to_path_buf(),
        _ => {
            let dir = cfg.masters_dir();
            fsutil::create_dir_all(&dir)?;
            let ext = src
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_else(|| "flac".to_owned());
            let flat = dir.join(format!("{hash}.{ext}"));
            let flat_there = long_path(&flat).exists();
            if flat_there && master_files::same_content(&flat, size, hash) {
                // This very file, kept the older way: as before.
                flat
            } else if image.is_some() {
                if flat_there {
                    return Err(ImportError::Other(format!(
                        "{} holds another file: the image is not written over it",
                        flat.display()
                    )));
                }
                let placed = master_files::place_master(src, &flat, hash)?;
                if placed.path != flat {
                    if placed.written {
                        let _ = fsutil::remove_file(&placed.path);
                    }
                    return Err(ImportError::Other(format!("{} was taken meanwhile", flat.display())));
                }
                if placed.written {
                    written = Some(flat.clone());
                }
                flat
            } else if let Some(kept) = master_files::existing_copy(&inner.library, &dir, hash, size) {
                file_path = Some(kept.to_string_lossy().into_owned());
                kept
            } else {
                let tag = master_files::hi_res_tag(a.info.bit_depth, a.info.sample_rate);
                // A masters folder too long for a name: the hash names it.
                let target = master_files::master_target(&dir, &layout_input(id), tag.as_deref(), &ext)
                    .unwrap_or_else(|| flat.clone());
                let placed = master_files::place_master(src, &target, hash)?;
                if placed.written {
                    written = Some(placed.path.clone());
                    if let (Some(album), Some(cover)) = (placed.path.parent(), cover) {
                        master_files::keep_cover(album, cover);
                    }
                }
                file_path = Some(placed.path.to_string_lossy().into_owned());
                placed.path
            }
        }
    };
    let dir = cfg.masters_dir();
    if let Err(e) = fsutil::create_dir_all(&dir) {
        undo_written(&written);
        return Err(e);
    }
    let sidecar_path = dir.join(format!("{hash}.json"));
    // The sheet goes with the image (never over one kept before), and the
    // image's sidecar names every track of it.
    let (cue, track_ids) = match image {
        Some(i) => {
            if let Err(e) = cue_import::keep_sheet(&dir, &i.cue, hash) {
                undo_written(&written);
                return Err(ImportError::Other(format!("the cue sheet {} is not kept with its image: {e}", i.cue.display())));
            }
            let mut ids: Vec<TrackId> = std::fs::read(long_path(&sidecar_path))
                .ok()
                .and_then(|b| serde_json::from_slice::<SidecarTracks>(&b).ok())
                .map(|s| std::iter::once(s.track_id).chain(s.track_ids).collect())
                .unwrap_or_default();
            ids.push(track_id);
            ids.sort();
            ids.dedup();
            (i.cue.file_name().map(|n| n.to_string_lossy().into_owned()), ids)
        }
        None => (None, Vec::new()),
    };
    let sidecar = MasterSidecar {
        track_id,
        track_ids,
        codec,
        file_hash: hash,
        size_bytes: size,
        origin: &a.origin,
        path: path.to_string_lossy().into_owned(),
        original_name: src
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        cue,
        quality_score: a.score,
    };
    // The sidecar is a second record beside the row: one that cannot be
    // written does not stop the master. It is written whole or not at all.
    let sidecar_bytes = match serde_json::to_vec_pretty(&sidecar) {
        Ok(json) => match master_files::write_whole(&sidecar_path, &json) {
            Ok(()) => Some(json),
            Err(e) => {
                log::warn!("the sidecar {} is not written: {e}", sidecar_path.display());
                None
            }
        },
        Err(e) => {
            log::warn!("the sidecar {} is not written: {e}", sidecar_path.display());
            None
        }
    };
    let master = Master {
        track_id,
        codec: codec.clone(),
        file_hash: hash.to_owned(),
        size_bytes: size,
        origin: Some(a.origin.clone()),
        file_path,
        replicas: Vec::new(),
    };
    // Durably: the Inbox file goes right after, and a power cut must not
    // take the row back once it has gone.
    if let Err(e) = inner.library.put_master_durable(&master) {
        // The Inbox file stays the original: the copy made now goes again.
        undo_written(&written);
        return Err(e.into());
    }
    if take
        && long_path(src).exists()
        && let Err(e) = fsutil::remove_file(src)
    {
        log::warn!("the master of {} is kept at {}, the Inbox file stays: {e}", src.display(), path.display());
    }
    Ok(Registered { master, written, sidecar: sidecar_bytes })
}

/// Summary of relocating lossless masters to a new directory/drive.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelocateReport {
    /// Lossless audio files copied, verified by BLAKE3 hash, and removed from `old_dir`.
    pub audio_moved: usize,
    /// Sidecar `.json` files moved and updated to point to `new_dir`.
    pub sidecars_moved: usize,
    /// Total audio bytes transferred and verified.
    pub bytes_moved: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct OwnedMasterSidecar {
    track_id: TrackId,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    track_ids: Vec<TrackId>,
    codec: Codec,
    file_hash: String,
    size_bytes: u64,
    origin: SourceOrigin,
    path: String,
    original_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cue: Option<String>,
    quality_score: i32,
}

/// Safely migrates all lossless master files and `.json` sidecars from `old_dir`
/// to `new_dir`:
/// 1. Probes `new_dir` for write access.
/// 2. Copies each audio file to a `.nikgob-part` temporary file in `new_dir`,
///    flushes to disk (`sync_all`), verifies its byte size and BLAKE3 hash,
///    renames into place, and deletes the original in `old_dir` only after
///    verification succeeds.
/// 3. Moves and updates `.json` sidecars and rewrites `master.origin_path` in
///    the library database.
/// 4. Removes `old_dir` if it becomes empty.
///
/// Only the flat `{file_hash}.<ext>` masters right in `old_dir` move. While
/// any master is kept by name in an album folder of `old_dir`, nothing moves
/// and the move is refused (an error): those masters would stay on the old
/// disk, and a disk taken away for the new one would take them along.
pub fn relocate_masters<F>(
    lib: &Library,
    old_dir: &Path,
    new_dir: &Path,
    mut on_progress: F,
) -> Result<RelocateReport>
where
    F: FnMut(usize, usize, &Path),
{
    let _writes = master_writes();
    if old_dir != new_dir {
        let named = lib
            .master_file_paths()?
            .into_iter()
            .filter(|p| Path::new(p).starts_with(old_dir))
            .count();
        if named > 0 {
            return Err(ImportError::NamedMastersInAlbumFolders {
                dir: old_dir.to_path_buf(),
                count: named,
            });
        }
    }
    fsutil::create_dir_all(new_dir)?;
    let probe = new_dir.join(".nikgob-write-probe");
    std::fs::write(long_path(&probe), b"ok").map_err(fsutil::io_err(&probe))?;
    fsutil::remove_file(&probe)?;

    if old_dir == new_dir || !long_path(old_dir).exists() {
        return Ok(RelocateReport::default());
    }
    if let (Ok(a), Ok(b)) = (
        std::fs::canonicalize(long_path(old_dir)),
        std::fs::canonicalize(long_path(new_dir)),
    ) {
        if a == b {
            return Ok(RelocateReport::default());
        }
    }

    let rd = std::fs::read_dir(long_path(old_dir)).map_err(fsutil::io_err(old_dir))?;
    let mut audio_files = Vec::new();
    let mut sidecar_files = Vec::new();
    for entry in rd.flatten() {
        let path = old_dir.join(entry.file_name());
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            if name.ends_with(".nikgob-part") || name.ends_with(".nikgob-write-probe") {
                let _ = fsutil::remove_file(&path);
            }
            continue;
        }
        let Ok(meta) = std::fs::metadata(long_path(&path)) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        {
            sidecar_files.push(path);
        } else if fsutil::is_audio_file(&path)
            || fsutil::is_cue_file(&path)
            || cue_import::IMAGE_EXTENSIONS.contains(&path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default().as_str())
        {
            // Lossless files, and the cue sheets kept with images.
            audio_files.push(path);
        }
    }
    audio_files.sort();
    sidecar_files.sort();

    let mut report = RelocateReport::default();
    let total = audio_files.len();

    for (idx, src) in audio_files.iter().enumerate() {
        on_progress(idx, total, src);
        let size = fsutil::file_size(src)?;
        let stem = src
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let sidecar_path = old_dir.join(format!("{stem}.json"));
        let sidecar_hash = std::fs::read(long_path(&sidecar_path))
            .ok()
            .and_then(|b| serde_json::from_slice::<OwnedMasterSidecar>(&b).ok())
            .map(|s| s.file_hash);
        // A kept cue sheet is named by its image's hash, not its own.
        let expected_hash = sidecar_hash
            .or_else(|| {
                (stem.len() == 64 && stem.bytes().all(|b| b.is_ascii_hexdigit()))
                    .then(|| stem.to_ascii_lowercase())
            })
            .filter(|_| !fsutil::is_cue_file(src));

        let dst = new_dir.join(src.file_name().unwrap());
        fsutil::move_file_verified(src, &dst, expected_hash.as_deref())?;
        report.audio_moved += 1;
        report.bytes_moved += size;
    }

    for src_json in &sidecar_files {
        let file_name = src_json.file_name().unwrap();
        let dst_json = new_dir.join(file_name);
        let raw = std::fs::read(long_path(src_json)).map_err(fsutil::io_err(src_json))?;
        let out_bytes = match serde_json::from_slice::<OwnedMasterSidecar>(&raw) {
            Ok(mut sc) => {
                let p = Path::new(&sc.path);
                // Only a flat master moved with the folder; one kept by name
                // stays where it is, at the path its sidecar names.
                if let Ok(rel) = p.strip_prefix(old_dir)
                    && rel.components().count() == 1
                {
                    sc.path = new_dir.join(rel).to_string_lossy().into_owned();
                }
                serde_json::to_vec_pretty(&sc).unwrap_or(raw)
            }
            Err(_) => raw,
        };
        let tmp = dst_json.with_file_name(format!(
            ".{}.nikgob-part",
            file_name.to_string_lossy()
        ));
        {
            use std::io::Write;
            let mut f = std::fs::File::create(long_path(&tmp)).map_err(fsutil::io_err(&tmp))?;
            f.write_all(&out_bytes).map_err(fsutil::io_err(&tmp))?;
            f.sync_all().map_err(fsutil::io_err(&tmp))?;
        }
        let verified_size = fsutil::file_size(&tmp)?;
        if verified_size != out_bytes.len() as u64 {
            let _ = fsutil::remove_file(&tmp);
            return Err(ImportError::SidecarVerificationFailed { path: dst_json });
        }
        std::fs::rename(long_path(&tmp), long_path(&dst_json)).map_err(|e| {
            let _ = fsutil::remove_file(&tmp);
            ImportError::io(&dst_json, e)
        })?;
        fsutil::remove_file(src_json)?;
        report.sidecars_moved += 1;
    }

    let _ = lib.relocate_master_paths(old_dir, new_dir);

    if let Ok(mut rd) = std::fs::read_dir(long_path(old_dir)) {
        if rd.next().is_none() {
            let _ = std::fs::remove_dir(long_path(old_dir));
        }
    }

    Ok(report)
}
