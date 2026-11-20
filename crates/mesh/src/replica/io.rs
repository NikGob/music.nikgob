use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use futures_util::StreamExt as _;
use tokio::io::AsyncWriteExt as _;
use super::*;
use super::plan::stamp;

// --------------------------------------------------------------- a pull

/// The folder of the parts, made if need be only inside the library
/// folder: its real path.
fn parts_dir(library: &Path) -> std::io::Result<PathBuf> {
    nikgob_import::fsutil::create_dir_inside(&library.join(".musiclib").join("replica"), library)
}

/// Opening never follows a link at the end of the path (Unix
/// `O_NOFOLLOW`, and no waiting on a pipe).
#[cfg(unix)]
fn no_follow(opts: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt as _;
    opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
}

/// Opening takes a link or another reparse point itself, not what it
/// points to (then refused: [`reparse`]).
#[cfg(windows)]
fn no_follow(opts: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt as _;
    opts.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
}

#[cfg(not(any(unix, windows)))]
fn no_follow(_: &mut OpenOptions) {}

/// Whether `m` is a reparse point's (a link, a mount point…).
#[cfg(windows)]
fn reparse(m: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn reparse(_: &Metadata) -> bool {
    false
}

/// Whether `m` (not followed) is a plain file's: no link, no reparse point.
pub(super) fn plain_meta(m: &Metadata) -> bool {
    m.file_type().is_file() && !reparse(m)
}

/// Opens `path` with `opts`, never through a link at its end, and only
/// when it is a plain file.
fn plain_open(path: &Path, opts: &mut OpenOptions) -> std::io::Result<std::fs::File> {
    no_follow(opts);
    let f = opts.open(path)?;
    if !plain_meta(&f.metadata()?) {
        return Err(std::io::Error::other("not a regular file"));
    }
    Ok(f)
}

/// Opens the part at `path` to write from `offset` on (what is after it
/// dropped), never through a link: one not there yet is made (only when
/// nothing has its name, `offset` 0); one there must be a plain file.
pub(crate) fn open_part(path: &Path, offset: u64) -> std::io::Result<std::fs::File> {
    let mut f = match plain_open(path, OpenOptions::new().write(true)) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && offset == 0 => plain_open(path, OpenOptions::new().write(true).create_new(true))?,
        Err(e) => return Err(e),
    };
    f.set_len(offset)?;
    f.seek(SeekFrom::Start(offset))?;
    Ok(f)
}

/// The BLAKE3 (hex) of the plain file at `path` (never through a link).
pub(super) fn blake3_plain(path: &Path) -> std::io::Result<String> {
    let mut h = blake3::Hasher::new();
    h.update_reader(plain_open(path, OpenOptions::new().read(true))?)?;
    Ok(h.finalize().to_hex().to_string())
}

/// A part in the parts' folder: the file being pulled
/// (`{key}.{token}.part`) and its note (`.meta`: the file's hash and its
/// extension). The token is random: no one makes a file of that name
/// ahead of it.
#[derive(Clone, Debug)]
pub(crate) struct Part {
    pub(crate) file: PathBuf,
    pub(crate) note: PathBuf,
}

impl Part {
    /// A new part of `key` in `dir`.
    pub(crate) fn new(dir: &Path, key: &str) -> Result<Self, Failed> {
        let mut b = [0u8; 16];
        getrandom::fill(&mut b).map_err(|e| Failed::Bad(format!("no randomness: {e}")))?;
        Ok(Self::named(dir, key, &data_encoding::HEXLOWER.encode(&b)))
    }

    pub(super) fn named(dir: &Path, key: &str, token: &str) -> Self {
        Self { file: dir.join(format!("{key}.{token}.part")), note: dir.join(format!("{key}.{token}.meta")) }
    }

    /// Its file and its note go (a link of that name goes itself).
    fn remove(&self) {
        let _ = std::fs::remove_file(&self.file);
        let _ = std::fs::remove_file(&self.note);
    }
}

/// Writes a part's note, a new file put over nothing: the file's hash and
/// its extension.
pub(crate) fn write_note(note: &Path, hash: &str, ext: &str) -> std::io::Result<()> {
    let mut f = plain_open(note, OpenOptions::new().write(true).create_new(true))?;
    f.write_all(format!("{hash}\n{ext}").as_bytes())?;
    f.sync_all()
}

/// A part's note: the file's hash and its extension (none: no note, or
/// not one).
pub(super) fn read_note(note: &Path) -> Option<(String, String)> {
    let f = plain_open(note, OpenOptions::new().read(true)).ok()?;
    let mut s = String::new();
    f.take(NOTE_MAX).read_to_string(&mut s).ok()?;
    let (h, e) = s.split_once('\n')?;
    Some((clean_hash(h)?, clean_ext(e)))
}

/// The part of `key` an earlier try left in `dir`, found by listing it
/// (only a plain file counts; a link of such a name is never opened): the
/// one whose note says it is of the file `hash`, with its extension and
/// its length. The other parts of `key` go, and the notes whose part
/// never came.
pub(crate) fn find_part(dir: &Path, key: &str, hash: &str) -> Option<(Part, String, u64)> {
    let prefix = format!("{key}.");
    let mut found: Option<(Part, String, u64)> = None;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix(&prefix) else { continue };
        let (token, is_part) = match (rest.strip_suffix(".part"), rest.strip_suffix(".meta")) {
            (Some(t), _) => (t, true),
            (None, Some(t)) => (t, false),
            (None, None) => continue,
        };
        if token.len() != 32 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let part = Part::named(dir, key, token);
        if !is_part {
            if std::fs::symlink_metadata(&part.file).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
                let _ = std::fs::remove_file(&part.note);
            }
            continue;
        }
        let Some(len) = std::fs::symlink_metadata(&part.file).ok().filter(plain_meta).map(|m| m.len()) else { continue };
        match read_note(&part.note) {
            Some((h, ext)) if found.is_none() && h == hash => found = Some((part, ext, len)),
            _ => part.remove(),
        }
    }
    found
}

/// The part is the file `hash`; one that is not is deleted with its note.
pub(crate) fn verify_part(part: &Path, meta: &Path, hash: &str) -> Result<(), String> {
    let sum = blake3_plain(part).map_err(|e| format!("{}: {e}", part.display()))?;
    if sum != hash {
        let _ = std::fs::remove_file(part);
        let _ = std::fs::remove_file(meta);
        return Err(nikgob_core::i18n::mesh().file_damaged.to_owned());
    }
    Ok(())
}

/// Writes `resp`'s body to `part` (after its first `offset` bytes, or over
/// it when `offset` is 0; never through a link: [`open_part`]), paced, at
/// most `limit` bytes in all; stops between two pieces when told or when
/// its plan (`era`) is no longer the one ([`Failed::Stopped`]), or when a
/// write would take the part's disk past its reserve, the other pulls'
/// bytes kept (`room`; [`Failed::NoRoom`]); the part stays. How long the
/// part is then.
pub(crate) async fn receive_paced(resp: http::Response<Body>, part: &Path, offset: u64, limit: u64, era: u64, ctl: &Ctl, room: &mut Reserve) -> Result<u64, Failed> {
    let io = |e: std::io::Error| Failed::Bad(format!("{}: {e}", part.display()));
    let disk = part.parent().unwrap_or(part).to_owned();
    let mut file = {
        let p = part.to_owned();
        let f = tokio::task::spawn_blocking(move || open_part(&p, offset)).await.map_err(|e| Failed::Bad(e.to_string()))?.map_err(io)?;
        tokio::fs::File::from_std(f)
    };
    let mut len = offset;
    // The disk as last looked at: its free bytes less what the other
    // pulls keep (None: not known), and what this one wrote since.
    let mut seen: Option<Option<u64>> = None;
    let mut since_look = 0u64;
    let mut body = resp.into_body().into_data_stream();
    loop {
        if ctl.halted(era) {
            file.flush().await.map_err(io)?;
            return Err(Failed::Stopped);
        }
        match tokio::time::timeout(STALL, body.next()).await {
            Ok(Some(Ok(bytes))) => {
                let n = bytes.len() as u64;
                len += n;
                if len > limit {
                    file.flush().await.map_err(io)?;
                    return Err(Failed::Bad("the file is larger than expected".to_owned()));
                }
                if seen.is_none() || since_look >= ROOM_STEP {
                    let others = room.others();
                    seen = Some(nikgob_runtime::free_bytes(&disk).map(|free| free.saturating_sub(others)));
                    since_look = 0;
                }
                // Each write weighed: what the disk would keep after it.
                let keeps = seen.flatten().is_some_and(|free| free.saturating_sub(since_look).saturating_sub(n) >= ctl.keep_free);
                if !keeps {
                    file.flush().await.map_err(io)?;
                    return Err(Failed::NoRoom);
                }
                file.write_all(&bytes).await.map_err(io)?;
                since_look += n;
                room.wrote(n);
                ctl.received.fetch_add(n, Ordering::Relaxed);
                ctl.pace.take(bytes.len()).await;
            }
            Ok(Some(Err(e))) => {
                file.flush().await.map_err(io)?;
                return Err(Failed::Bad(format!("the transfer broke off: {e}")));
            }
            Ok(None) => break,
            Err(_) => {
                file.flush().await.map_err(io)?;
                return Err(Failed::Bad("the transfer stalled".to_owned()));
            }
        }
    }
    file.flush().await.map_err(io)?;
    file.sync_all().await.map_err(io)?;
    Ok(len)
}

pub(super) fn header<'a>(r: &'a http::Response<Body>, name: &str) -> Option<&'a str> {
    r.headers().get(name).and_then(|v| v.to_str().ok())
}

/// Pulls `job` (exactly its file, by its hash) from the devices `from`
/// (the first that has it), keeps it and makes it this device's own. Its
/// bytes.
#[allow(clippy::too_many_arguments)]
pub(super) async fn pull(sh: &Arc<Shared>, lib: &Library, me: &str, dirs: &Dirs, job: &Job, from: &[String], era: u64, ctl: &Ctl, room: &mut Reserve) -> Result<u64, Failed> {
    let library = dirs.library.clone();
    let dir = tokio::task::spawn_blocking(move || parts_dir(&library))
        .await
        .map_err(|e| Failed::Bad(e.to_string()))?
        .map_err(|e| Failed::Bad(format!("parts folder: {e}")))?;
    let key = job.key();
    let want = job.hash().to_owned();
    // A part of an earlier try goes on where it stopped when its note says
    // it is of this very file; the others of this job go.
    let found = {
        let (d, k, w) = (dir.clone(), key.clone(), want.clone());
        tokio::task::spawn_blocking(move || find_part(&d, &k, &w)).await.map_err(|e| Failed::Bad(e.to_string()))?
    };
    let (mut part, mut ext, mut offset) = match found {
        Some((p, e, len)) => (p, Some(e), len),
        None => (Part::new(&dir, &key)?, None, 0),
    };
    // A whole part left (kept, then not made the device's own): kept now.
    if let Some(e) = ext.clone()
        && offset > 0
        && offset == job.size()
    {
        let (p, m, h) = (part.file.clone(), part.note.clone(), want.clone());
        let whole = tokio::task::spawn_blocking(move || verify_part(&p, &m, &h)).await.map_err(|e| Failed::Bad(e.to_string()))?;
        if whole.is_ok() {
            ctl.received.fetch_add(offset, Ordering::Relaxed);
            let r = Room { keep: ctl.keep_free, others: room.others() };
            return keep(lib, me, dirs, job, &part, &want, &e, offset, r).await.map(|()| offset);
        }
        (part, ext, offset) = (Part::new(&dir, &key)?, None, 0);
    }
    // Exactly the planned file: its size when it is known.
    let limit = if job.size() > 0 { job.size() } else { FILE_MAX };
    let mut not_there = 0usize;
    'devices: for peer in from {
        for _ in 0..3 {
            let uri = match job {
                Job::Track(w) => format!("/v1/media/tracks/{}/audio?profile={}&hash={}", w.track_id, nikgob_view::MeshFormat::Orig.key(), w.file_hash),
                Job::Master { item, .. } => format!("/v1/replica/masters/{}", item.hash),
            };
            let mut b = http::Request::get(uri);
            if offset > 0 {
                b = b.header(http::header::RANGE, format!("bytes={offset}-"));
            }
            let req = b.body(Body::empty()).map_err(|e| Failed::Bad(e.to_string()))?;
            let resp = match tokio::time::timeout(HEAD_WAIT, sh.request(peer, req)).await {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    log::debug!("replica: {key} from {peer}: {e}");
                    continue 'devices;
                }
                Err(_) => continue 'devices,
            };
            let status = resp.status();
            if status == http::StatusCode::NOT_FOUND {
                not_there += 1;
                continue 'devices;
            }
            // The part is no part of the file there, or the answer is not
            // the part asked for: a new part, from the start.
            let wrong_part = status == http::StatusCode::PARTIAL_CONTENT && !range_starts_at(header(&resp, "content-range"), offset);
            if status == http::StatusCode::RANGE_NOT_SATISFIABLE || wrong_part {
                drop(resp);
                part.remove();
                (part, ext, offset) = (Part::new(&dir, &key)?, None, 0);
                continue;
            }
            if !status.is_success() {
                return Err(Failed::Bad(format!("answer {status}")));
            }
            let Some(hash) = header(&resp, nikgob_api::HASH_HEADER).and_then(clean_hash) else {
                return Err(Failed::Bad("that device did not say the file's hash".to_owned()));
            };
            if hash != want {
                return Err(Failed::Bad("not this file".to_owned()));
            }
            let e = clean_ext(header(&resp, nikgob_api::EXT_HEADER).unwrap_or(""));
            let from_byte = if status == http::StatusCode::PARTIAL_CONTENT { offset } else { 0 };
            if ext.is_none() {
                let (n, h, x) = (part.note.clone(), hash.clone(), e.clone());
                tokio::task::spawn_blocking(move || write_note(&n, &h, &x))
                    .await
                    .map_err(|e| Failed::Bad(e.to_string()))?
                    .map_err(|err| Failed::Bad(format!("{}: {err}", part.note.display())))?;
                ext = Some(e.clone());
            }
            ctl.received.fetch_add(from_byte, Ordering::Relaxed);
            let len = receive_paced(resp, &part.file, from_byte, limit, era, ctl, room).await?;
            let (p, m, h) = (part.file.clone(), part.note.clone(), hash.clone());
            tokio::task::spawn_blocking(move || verify_part(&p, &m, &h)).await.map_err(|e| Failed::Bad(e.to_string()))?.map_err(Failed::Bad)?;
            let e = ext.clone().unwrap_or(e);
            let r = Room { keep: ctl.keep_free, others: room.others() };
            keep(lib, me, dirs, job, &part, &hash, &e, len, r).await?;
            return Ok(len);
        }
    }
    if not_there > 0 { Err(Failed::NotThere) } else { Err(Failed::Bad("no device gave the file".to_owned())) }
}

/// The checked part put in its place and made this device's own (on the
/// blocking pool), only when the disk it goes to keeps `room` after it.
#[allow(clippy::too_many_arguments)]
async fn keep(lib: &Library, me: &str, dirs: &Dirs, job: &Job, part: &Part, hash: &str, ext: &str, size: u64, room: Room) -> Result<(), Failed> {
    let (lib, me, dirs, job, part, hash, ext) = (lib.clone(), me.to_owned(), dirs.clone(), job.clone(), part.clone(), hash.to_owned(), ext.to_owned());
    tokio::task::spawn_blocking(move || -> Result<(), Failed> {
        let placed = match &job {
            Job::Track(w) => place_song(&lib, &dirs.library, &me, w, &part.file, &hash, &ext, size, room),
            Job::Master { item, .. } => place_master(&lib, &dirs.masters, &me, item, &part.file, &ext, room),
        }?;
        let _ = std::fs::remove_file(&part.note);
        log::info!("replica: {} kept", placed.display());
        Ok(())
    })
    .await
    .map_err(|e| Failed::Bad(e.to_string()))?
}

/// Whether `path` holds exactly the file of `size` bytes and `hash`.
fn same_file(path: &Path, size: u64, hash: &str) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() == size) && blake3_plain(path).is_ok_and(|h| h == hash)
}


/// Puts the checked `part` at `target`, or at `target (2)`… when the name
/// is another file's — never over anything, its folders made only inside
/// `root`, only when the disk it goes to keeps `room` after its bytes,
/// whatever disk that is (a move on one disk is weighed as a copy too;
/// [`Failed::NoRoom`]: the part stays), a copy across disks checked before
/// the part goes;
/// the very file there already (a plain file inside `root`) is taken as it
/// is (the part goes). Where it is, and whether the part was moved there
/// (false: the file was there already).
pub(crate) fn put_in_place(part: &Path, target: &Path, hash: &str, size: u64, root: &Path, room: Room) -> Result<(PathBuf, bool), Failed> {
    let dir = target.parent().ok_or_else(|| Failed::Bad(format!("{} has no folder", target.display())))?;
    nikgob_import::fsutil::create_dir_inside(dir, root).map_err(|e| Failed::Bad(e.to_string()))?;
    if nikgob_import::fsutil::plain_file_inside(target, root) && same_file(target, size, hash) {
        let _ = std::fs::remove_file(part);
        return Ok((target.to_owned(), false));
    }
    if !room.fits(dir, size) {
        return Err(Failed::NoRoom);
    }
    let placed = nikgob_import::fsutil::move_unique_verified(part, target, hash).map_err(|e| Failed::Bad(e.to_string()))?;
    if !nikgob_import::fsutil::plain_file_inside(&placed, root) {
        let _ = nikgob_import::fsutil::move_new_verified(&placed, part, hash);
        return Err(Failed::Bad(format!("{} is outside {}", placed.display(), root.display())));
    }
    Ok((placed, true))
}

/// A flat master, `{hash}.<ext>` right in `root`: only under that very
/// name (a master kept flat is found by it), never over another file, only
/// when the disk it goes to keeps `room` after its bytes.
fn put_flat(part: &Path, root: &Path, hash: &str, ext: &str, size: u64, room: Room) -> Result<(PathBuf, bool), Failed> {
    nikgob_import::fsutil::create_dir_inside(root, root).map_err(|e| Failed::Bad(e.to_string()))?;
    let target = root.join(format!("{hash}.{ext}"));
    if nikgob_import::fsutil::plain_file_inside(&target, root) && same_file(&target, size, hash) {
        let _ = std::fs::remove_file(part);
        return Ok((target, false));
    }
    if !room.fits(root, size) {
        return Err(Failed::NoRoom);
    }
    nikgob_import::fsutil::move_new_verified(part, &target, hash).map_err(|e| Failed::Bad(format!("{}: {e}", target.display())))?;
    Ok((target, true))
}

/// A pulled song's file laid out in `library` as the library names songs
/// (`Artist/YYYY - Album/NN - Title.ext`) and registered as this device's
/// (`me`) own source; its format from the device that had it; what it
/// hashes to noted for the plans. When it cannot be registered it goes
/// back to its part (kept for the next try).
#[allow(clippy::too_many_arguments)]
pub(crate) fn place_song(lib: &Library, library: &Path, me: &str, want: &Want, part: &Path, hash: &str, ext: &str, size: u64, room: Room) -> Result<PathBuf, Failed> {
    let layout = nikgob_import::master_files::layout_of_track(lib, want.track_id).ok_or_else(|| Failed::Bad("the song is not in the library yet".to_owned()))?;
    let target = layout.target_path(library, ext);
    let (placed, moved) = put_in_place(part, &target, hash, size, library, room)?;
    let of = lib.offer_of_hash(hash).ok().flatten().or_else(|| want.offers.first().cloned());
    let codec = of.as_ref().map_or_else(|| Codec::parse(ext), |o| Codec::parse(&o.codec));
    let mut s = Source::new(want.track_id, SourceKind::LocalFile, placed.to_string_lossy().into_owned(), codec);
    if let Some(o) = &of {
        s.bitrate = o.bitrate;
        s.sample_rate = o.sample_rate;
        s.bit_depth = o.bit_depth;
        s.quality_score = o.quality_score;
    }
    s.file_hash = Some(hash.to_owned());
    s.size_bytes = Some(size);
    s.added_at = nikgob_library::now_ms();
    if let Err(e) = lib.put_pulled_file(&s, me) {
        // Only what was moved goes back (a file found there stays).
        if moved {
            let _ = nikgob_import::fsutil::move_new_verified(&placed, part, hash);
        }
        return Err(Failed::Bad(e.to_string()));
    }
    stamp(lib, &placed, hash);
    Ok(placed)
}

/// A pulled master laid out in `masters` as the library keeps masters (by
/// name for one track, flat for a disc image or a track not named yet) and
/// kept as this device's (`me`); what it hashes to noted for the plans.
pub(super) fn place_master(lib: &Library, masters: &Path, me: &str, item: &MasterItem, part: &Path, ext: &str, room: Room) -> Result<PathBuf, Failed> {
    let layout = match item.tracks.as_slice() {
        [one] => nikgob_import::master_files::layout_of_track(lib, *one),
        _ => None,
    };
    let named = layout.and_then(|l| nikgob_import::master_files::master_target(masters, &l, None, ext));
    let ((placed, moved), by_name) = match named {
        Some(target) => (put_in_place(part, &target, &item.hash, item.size, masters, room)?, true),
        None => (put_flat(part, masters, &item.hash, ext, item.size, room)?, false),
    };
    let path = by_name.then(|| placed.to_string_lossy().into_owned());
    if let Err(e) = lib.put_pulled_masters(item, path.as_deref(), me) {
        if moved {
            let _ = nikgob_import::fsutil::move_new_verified(&placed, part, &item.hash);
        }
        return Err(Failed::Bad(e.to_string()));
    }
    stamp(lib, &placed, &item.hash);
    Ok(placed)
}

