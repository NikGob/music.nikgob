//! «Обновить обложку» of a library album. Its art is picked again from its
//! own files (each track's picture, its master's, the cover it shows and
//! the cover images of its own folder) and looked up online
//! ([`hd_covers::find_all`]): the services' releases linked to it (the same
//! release, whatever kind a service calls it), the Cover Art Archive by its
//! ids when MusicBrainz says they are of its kind, the album of its title
//! on the services and in the iTunes Store, never a single or an EP for an
//! album of more songs unless its songs are counted as many. Each found is
//! downloaded one by one at the largest size its CDN serves and measured
//! by its bytes: the first that comes (a linked release's, else the one of
//! the very title of the first source) is the anchor, and a larger one
//! takes its place only as the same picture (a small colour-aware look,
//! [`same_picture`]). That is the album's art online. It wins over the
//! pictures of the album's files — a file may hold the art of the single
//! the album was once taken for — unless one of them is the same picture
//! and larger. Without it, the
//! largest picture of its files that is no frame of a video.
//!
//! Where the art goes: the album's folder, when that folder is the album's
//! alone (in the library, not the masters' or the app's, every file of the
//! album in it, its copies too, and nothing of another album): its
//! `cover.jpg` and `ambient.png`. Otherwise a file of the album's own in
//! the app's covers folder (`.musiclib/covers/release-<id>-<time>.jpg`, a
//! fresh name each time: the UI keeps pictures by their address), the one
//! before it kept. Every file is written whole or not at all (staged beside
//! it, flushed, renamed over it). In the album's folder the cover it showed
//! is staged beside it and becomes `cover.prev.jpg` only once the library
//! took the new one; when the library cannot, the old cover comes back. An
//! album without files (a service's) shows the url found, and its picture
//! is asked anew.
//!
//! A lookup that failed changes nothing. The palette and the texture are
//! made again from the picked picture every time. The library takes the
//! cover ref, the palette, what the background lookups keep for the album
//! and the decision ([`decision_key`]) in one transaction: the lookups
//! leave the album's cover alone while it shows that cover, and one of
//! theirs that began before it is not kept. Everything but the decision of
//! where to write runs off the player's loop.

use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::time::Instant;

use nikgob_core::cover_frame;
use nikgob_core::model::{Palette, ReleaseId, SourceKind};
use nikgob_core::streaming::{Service, StreamService};
use nikgob_import::cover::{ProcessedCover, derive_visuals, process_cover};
use nikgob_import::sanitize::long_path;
use nikgob_library::{Library, is_web_ref, now_ms};
use nikgob_metadata::Metadata;

use crate::Worker;
use crate::better_covers;
use crate::hd_covers;
use crate::import::Internal;

const COVER_FILE: &str = "cover.jpg";
/// The cover a refresh replaced in an album's folder.
const PREV_FILE: &str = "cover.prev.jpg";
const AMBIENT_FILE: &str = "ambient.png";
/// The app's folder in the library; its covers folder.
const APP_DIR: &str = ".musiclib";
const MANAGED_DIR: &str = ".musiclib/covers";
/// Names of the cover images in a folder (as the importer reads them).
const COVER_NAMES: &[&str] = &["cover", "folder", "front", "album", "albumart"];
/// The online lookup and download, at most.
const ONLINE_TIMEOUT: Duration = Duration::from_secs(90);
const GUARD: Duration = Duration::from_secs(30);
/// Without the importer's settings.
const MAX_EDGE: u32 = 1200;
const FALLBACK_SEED: u32 = 0xFF67_50A4;
/// Two pictures are one when each colour's steps differ in at most this
/// many of 64 and their mean colours are at most this far apart.
const SAME_BITS: u32 = 8;
const SAME_COLOUR: u32 = 40;
/// A step of light this large is plain to see; a picture needs this many
/// of its 128 to be told from another.
const STEP: u32 = 3;
const MIN_STEPS: u32 = 24;
/// A picture is decoded up to this long side and this much memory (a
/// 3000 px cover of the iTunes Store takes 36 MB).
const MAX_SIDE: u32 = 4000;
const MAX_ALLOC: u64 = 128 << 20;
/// The covers downloaded per refresh, at most; the time each has.
const MAX_CANDIDATES: usize = 6;
const IMAGE_BUDGET: Duration = Duration::from_secs(25);
/// The time of [`ONLINE_TIMEOUT`] the lookups leave for the downloads.
const DOWNLOAD_TIME: Duration = Duration::from_secs(30);
/// How long a refresh waits for another one's heavy work.
const HEAVY_WAIT: Duration = Duration::from_secs(120);

/// One refresh at a time downloads and decodes pictures (those found
/// online and its files'), until its last decode ended.
static HEAVY: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

/// Staging files of this run, counted.
static STAGING: AtomicU64 = AtomicU64::new(0);
/// The album folders a refresh writes into right now.
static FOLDERS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The albums being refreshed.
pub(crate) type Busy = HashSet<ReleaseId>;

/// The library cache key of the refresh decided for `id`: the cover ref it
/// left the album with.
pub(crate) fn decision_key(id: ReleaseId) -> String {
    format!("cover_refreshed.{id}")
}

/// Whether «Обновить обложку» decided the cover of `id` and the album still
/// shows that cover (`current`, its ref).
pub(crate) fn decided(lib: &Library, id: ReleaseId, current: Option<&str>) -> bool {
    lib.cache_get(&decision_key(id)).ok().flatten().is_some_and(|(_, v)| v == current.unwrap_or(""))
}

/// What a refresh of an album came to.
#[derive(Debug)]
pub(crate) enum Refreshed {
    /// A picture was picked: its file is in place.
    File(Picked),
    /// The album has no files: it shows this url (asked anew).
    Web(String),
    /// Nothing to pick from.
    Nothing,
}

/// The picked picture of an album with files.
#[derive(Debug)]
pub(crate) struct Picked {
    cover_ref: String,
    seed: Option<u32>,
    ambient: Vec<u8>,
    /// The url it came from, when it came from online.
    url: Option<String>,
    /// What the album shows changes (else it shows that picture already).
    changed: bool,
    /// The cover it showed, staged, and where it goes once the library
    /// took the new one.
    backup: Option<(PathBuf, PathBuf)>,
    /// How the files go back when the library cannot take it.
    undo: Undo,
}

#[derive(Debug, PartialEq)]
enum Undo {
    Nothing,
    /// The staged copy of the cover goes back over it.
    Restore { staged: PathBuf, target: PathBuf },
    /// The new file of the covers folder goes.
    Remove(PathBuf),
}

/// Where a refresh writes.
#[derive(Clone, Debug, PartialEq)]
enum Dest {
    /// The album's own folder.
    Folder(PathBuf),
    /// A file of its own in the app's covers folder (this one).
    Managed(PathBuf),
    /// No files: a url.
    Web,
}

/// What the refresh of one album works with.
struct Job {
    id: ReleaseId,
    dest: Dest,
    /// The library folder refs are relative to.
    root: PathBuf,
    /// Its cover ref, and the file when it is one.
    current: Option<String>,
    current_file: Option<PathBuf>,
    /// The url it shows: the one the lookups found, or its own.
    shown_url: Option<String>,
    /// Audio files whose pictures count, the masters first.
    files: Vec<PathBuf>,
    /// Folders whose cover images count (its own folder only).
    dirs: Vec<PathBuf>,
    want: hd_covers::Want,
    max_edge: u32,
    /// The name of a new file in the covers folder.
    stamp: i64,
}

/// A path as compared: forward slashes, no slash at the end, lower case on
/// Windows.
fn norm(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = s.trim_end_matches('/');
    if cfg!(windows) { s.to_lowercase() } else { s.to_owned() }
}

/// Whether `p` is `dir` or lies under it.
fn within(p: &Path, dir: &Path) -> bool {
    let (p, d) = (norm(p), norm(dir));
    p == d || p.starts_with(&format!("{d}/"))
}

/// A locator or a cover ref as a path: relative ones under `root`.
fn resolve(root: &Path, locator: &str) -> PathBuf {
    let p = Path::new(locator);
    if p.is_absolute() { p.to_owned() } else { root.join(p) }
}

/// One refresh at a time writes into an album folder: held while it does.
struct Claim(String);

impl Claim {
    fn take(dir: &Path) -> Option<Self> {
        let key = norm(dir);
        let mut held = FOLDERS.lock().unwrap_or_else(PoisonError::into_inner);
        if held.contains(&key) {
            return None;
        }
        held.push(key.clone());
        Some(Self(key))
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        FOLDERS.lock().unwrap_or_else(PoisonError::into_inner).retain(|k| *k != self.0);
    }
}

/// The folder that is release `id`'s alone, where its cover may be
/// written: the one folder every local file of it (`local`: its copies
/// too) is in, else its cover file's; under the library `root`, not the
/// masters' nor the app's; no file of another release (a track or a
/// cover, `places`) in it.
fn owned_folder(
    id: ReleaseId,
    local: &[PathBuf],
    cover: Option<&Path>,
    root: &Path,
    masters: Option<&Path>,
    places: &[(ReleaseId, PathBuf)],
) -> Option<PathBuf> {
    let dir = match local.first() {
        Some(f) => f.parent()?.to_owned(),
        None => cover?.parent()?.to_owned(),
    };
    let key = norm(&dir);
    let in_dir = |p: &Path| p.parent().is_some_and(|d| norm(d) == key);
    let ours = local.iter().all(|f| in_dir(f));
    let placed = within(&dir, root) && norm(&dir) != norm(root) && !masters.is_some_and(|m| within(&dir, m));
    let app = dir.components().any(|c| c.as_os_str() == APP_DIR);
    let shared = places.iter().any(|(r, p)| *r != id && in_dir(p));
    (ours && placed && !app && !shared).then_some(dir)
}

/// What the refresh of release `id` works with, read from the library (off
/// the player's loop); `None` when it is gone.
fn gather(lib: &Library, id: ReleaseId, root: &Path, masters: Option<&Path>, max_edge: u32, stamp: i64) -> Option<Job> {
    let r = lib.get_release(id).ok().flatten()?;
    let mut files: Vec<PathBuf> = Vec::new();
    let mut local: Vec<PathBuf> = Vec::new();
    for t in lib.tracks_of_release(id).unwrap_or_default() {
        if let Some(path) = lib.get_master(t.id).ok().flatten().and_then(|m| m.file_path) {
            files.push(PathBuf::from(path));
        }
        // Every copy of it, not only the one that plays.
        for s in lib.sources_of_track(t.id).unwrap_or_default() {
            if s.kind == SourceKind::LocalFile && s.trashed_at.is_none() {
                local.push(resolve(root, &s.locator));
            }
        }
    }
    let current_file = r.cover_ref.as_deref().filter(|c| !is_web_ref(c)).map(|c| resolve(root, c));
    // Unknown who else is in a folder: none is the album's alone.
    let owned = match lib.release_file_places() {
        Ok(places) => {
            let places: Vec<(ReleaseId, PathBuf)> = places.into_iter().map(|(r, p)| (r, resolve(root, &p))).collect();
            owned_folder(id, &local, current_file.as_deref(), root, masters, &places)
        }
        Err(e) => {
            log::warn!("cover refresh of «{}»: the places of the library's files: {e}", r.title);
            None
        }
    }
    .filter(|d| long_path(d).is_dir());
    let dest = match owned {
        Some(d) => Dest::Folder(d),
        None if local.is_empty() && current_file.is_none() => Dest::Web,
        None => Dest::Managed(root.join(MANAGED_DIR)),
    };
    let dirs = match &dest {
        Dest::Folder(d) => vec![d.clone()],
        Dest::Managed(_) | Dest::Web => Vec::new(),
    };
    let shown_url = better_covers::cached(lib, better_covers::Target::Release(id), r.cover_ref.as_deref())
        .or_else(|| r.cover_ref.clone().filter(|c| is_web_ref(c)));
    files.extend(local);
    Some(Job {
        id,
        dest,
        root: root.to_owned(),
        current: r.cover_ref.clone(),
        current_file,
        shown_url,
        files,
        dirs,
        want: hd_covers::want_of(lib, &r, vec![r.artist_credit.display()]),
        max_edge,
        stamp,
    })
}

/// The cover images of `dir` (see [`COVER_NAMES`]; `cover.prev.jpg` is
/// none).
fn cover_images(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(long_path(dir)) else { return Vec::new() };
    let mut out: Vec<(usize, PathBuf)> = rd
        .flatten()
        .map(|e| e.path())
        .filter_map(|p| {
            let stem = p.file_stem()?.to_string_lossy().to_lowercase();
            let ext = p.extension()?.to_string_lossy().to_lowercase();
            let rank = COVER_NAMES.iter().position(|n| *n == stem)?;
            let name = p.file_name()?;
            matches!(ext.as_str(), "jpg" | "jpeg" | "png").then(|| (rank, dir.join(name)))
        })
        .collect();
    out.sort();
    out.into_iter().map(|(_, p)| p).collect()
}

/// What a picture looks like, small: for each colour whether each of 9×9
/// is lighter than the next to its right and than the next below it, its
/// mean colour, and how many of its 128 steps of light are plain to see.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Look {
    across: [u64; 3],
    down: [u64; 3],
    mean: [u8; 3],
    steps: u32,
}

/// The [`Look`] of a picture. A picture keeps it at any size and
/// compression; another picture has another, and so does the same drawing
/// in other colours or turned upside down.
pub(crate) fn look(img: &image::DynamicImage) -> Look {
    let small = img.thumbnail(64, 64).resize_exact(9, 9, image::imageops::FilterType::Triangle).to_rgb8();
    let light = |p: [u8; 3]| (299 * u32::from(p[0]) + 587 * u32::from(p[1]) + 114 * u32::from(p[2])) / 1000;
    let mut across = [0u64; 3];
    let mut down = [0u64; 3];
    let mut sum = [0u32; 3];
    let mut steps = 0;
    for y in 0..9 {
        for x in 0..9 {
            let p = small.get_pixel(x, y).0;
            for (s, v) in sum.iter_mut().zip(p) {
                *s += u32::from(v);
            }
            if x == 8 || y == 8 {
                continue;
            }
            let (right, below) = (small.get_pixel(x + 1, y).0, small.get_pixel(x, y + 1).0);
            for c in 0..3 {
                across[c] = (across[c] << 1) | u64::from(p[c] > right[c]);
                down[c] = (down[c] << 1) | u64::from(p[c] > below[c]);
            }
            steps += u32::from(light(p).abs_diff(light(right)) >= STEP) + u32::from(light(p).abs_diff(light(below)) >= STEP);
        }
    }
    Look { across, down, mean: sum.map(|s| u8::try_from(s / 81).unwrap_or(u8::MAX)), steps }
}

/// Whether two looks are of one picture: each colour's steps alike both
/// ways ([`SAME_BITS`]), the mean colours near ([`SAME_COLOUR`]). A picture
/// with too little to tell (a flat colour, a faint gradient: fewer than
/// [`MIN_STEPS`] plain steps) is never the same as any.
fn same_picture(a: Option<&Look>, b: Option<&Look>) -> bool {
    let (Some(a), Some(b)) = (a, b) else { return false };
    let told = |l: &Look| l.steps >= MIN_STEPS;
    let near = a.mean.iter().zip(b.mean).map(|(x, y)| u32::from(x.abs_diff(y)).pow(2)).sum::<u32>() <= SAME_COLOUR * SAME_COLOUR;
    let alike = |x: &[u64; 3], y: &[u64; 3]| x.iter().zip(y).all(|(p, q)| (p ^ q).count_ones() <= SAME_BITS);
    told(a) && told(b) && near && alike(&a.across, &b.across) && alike(&a.down, &b.down)
}

/// The size of the picture in `bytes` from its header, when it is one.
fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?.into_dimensions().ok()
}

/// The long side of the picture in `bytes` as it came and its [`Look`]:
/// none when it is no picture, is larger than [`MAX_SIDE`] or needs more
/// than [`MAX_ALLOC`] to decode.
fn measure(bytes: &[u8]) -> Option<(u32, Look)> {
    let (w, h) = dimensions(bytes)?;
    if w.max(h) > MAX_SIDE {
        return None;
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let img = reader.decode().ok()?;
    Some((img.width().max(img.height()), look(&img)))
}

/// A picture of the album's own files: as a cover, with its long side as
/// it came and its look.
#[derive(Clone, Debug)]
struct Own {
    cover: ProcessedCover,
    side: u32,
    look: Option<Look>,
}

/// The album's art found online, downloaded.
#[derive(Clone, Debug)]
struct Online {
    /// The url the album keeps: one that came ([`keep_url`]).
    keep: String,
    cover: ProcessedCover,
    /// Its long side as it came, and its look.
    side: u32,
    look: Option<Look>,
}

/// The pictures of the album's own [`pick`] may take, of all those in the
/// audio `files`, the `images` and the cover images in `dirs` (each once,
/// in that order): the largest, and the largest that is the same picture
/// as the art found `online`. Each is measured within the caps
/// ([`measure`]) before it is made a cover: one larger, or a frame of a
/// video, is none.
fn own_art(files: &[PathBuf], images: &[PathBuf], dirs: &[PathBuf], max_edge: u32, online: Option<&Look>) -> Vec<Own> {
    let mut seen = HashSet::new();
    let mut best: Option<Own> = None;
    let mut same: Option<Own> = None;
    let mut take = |bytes: Vec<u8>| {
        let mut h = DefaultHasher::new();
        bytes.hash(&mut h);
        if !seen.insert(h.finish()) || dimensions(&bytes).is_none_or(|(width, height)| cover_frame::is_frame(width, height)) {
            return;
        }
        let Some((side, look)) = measure(&bytes) else { return };
        let larger = best.as_ref().is_none_or(|b| side > b.side);
        let larger_same = same_picture(Some(&look), online) && same.as_ref().is_none_or(|s| side > s.side);
        if !(larger || larger_same) {
            return;
        }
        let Ok(cover) = process_cover(&bytes, max_edge) else { return };
        let own = Own { cover, side, look: Some(look) };
        if larger_same {
            same = Some(own.clone());
        }
        if larger {
            best = Some(own);
        }
    };
    for f in files {
        match nikgob_import::tags::read_tags(f) {
            Ok((tags, _)) => tags.cover.into_iter().for_each(&mut take),
            Err(e) => log::debug!("cover refresh: the picture of {}: {e}", f.display()),
        }
    }
    for p in images.iter().cloned().chain(dirs.iter().flat_map(|d| cover_images(d))) {
        if let Ok(b) = std::fs::read(long_path(&p)) {
            take(b);
        }
    }
    best.into_iter().chain(same).collect()
}

/// The art the album takes, frames of a video never: its art found
/// `online` (verified by the lookup) over the pictures of its files, which
/// may be a single's, unless one of them is the same picture and larger as
/// it came (the largest such); without it, the largest of `local`. The
/// first of equals wins. With it, the url the lookups keep.
fn pick(local: Vec<Own>, online: Option<Online>) -> Option<(ProcessedCover, Option<String>)> {
    let shown = |c: &ProcessedCover| !cover_frame::is_frame(c.width, c.height);
    let online = online.filter(|o| shown(&o.cover));
    let mut best: Option<Own> = None;
    let mut same: Option<Own> = None;
    for c in local.into_iter().filter(|c| shown(&c.cover)) {
        if let Some(o) = &online
            && same_picture(c.look.as_ref(), o.look.as_ref())
            && c.side > o.side
            && same.as_ref().is_none_or(|s| c.side > s.side)
        {
            same = Some(c.clone());
        }
        if best.as_ref().is_none_or(|b| c.side > b.side) {
            best = Some(c);
        }
    }
    match (online, same) {
        (Some(_), Some(c)) => Some((c.cover, None)),
        (Some(o), None) => Some((o.cover, Some(o.keep))),
        (None, _) => best.map(|c| (c.cover, None)),
    }
}

/// A name beside `target` that no other write takes.
fn staging(target: &Path) -> PathBuf {
    let n = STAGING.fetch_add(1, Ordering::Relaxed);
    let name = target.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    target.with_file_name(format!(".{name}.{}-{}-{n}.tmp", std::process::id(), now_ms()))
}

/// Fills the staging file `tmp` and flushes it to the disk; on a failure
/// it goes.
fn stage(tmp: &Path, fill: impl FnOnce(&Path) -> std::io::Result<()>) -> std::io::Result<()> {
    let done = fill(tmp).and_then(|()| std::fs::OpenOptions::new().write(true).open(long_path(tmp))?.sync_all());
    if done.is_err() {
        let _ = std::fs::remove_file(long_path(tmp));
    }
    done
}

/// Renames `tmp` over `target`; on a failure `tmp` goes and `target` stays
/// as it was.
fn swap(tmp: &Path, target: &Path) -> std::io::Result<()> {
    let done = std::fs::rename(long_path(tmp), long_path(target));
    if done.is_err() {
        let _ = std::fs::remove_file(long_path(tmp));
    }
    done
}

/// Writes `bytes` as `target`, whole or not at all.
fn write_atomic(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = staging(target);
    stage(&tmp, |tmp| {
        let mut f = std::fs::File::create(long_path(tmp))?;
        f.write_all(bytes)
    })?;
    swap(&tmp, target)
}

/// The texture beside a cover: `ambient.png` in an album's folder, the
/// cover's own name with `.ambient.png` in the covers folder.
fn ambient_of(cover: &Path, dest: &Dest) -> PathBuf {
    match dest {
        Dest::Folder(d) => d.join(AMBIENT_FILE),
        Dest::Managed(_) | Dest::Web => cover.with_extension("ambient.png"),
    }
}

/// `path` as a cover ref: relative to the library `root` with forward
/// slashes, else as it is.
fn cover_ref_of(path: &Path, root: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

/// The covers of release `id` in the covers folder `dir` but those `keep`
/// (with their textures) go.
fn prune_managed(dir: &Path, id: ReleaseId, keep: &[&Path]) {
    let prefix = format!("release-{id}-");
    let keep: Vec<String> = keep.iter().flat_map(|k| [norm(k), norm(&k.with_extension("ambient.png"))]).collect();
    for e in std::fs::read_dir(long_path(dir)).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = dir.join(&name);
        if name.starts_with(&prefix) && !keep.contains(&norm(&path)) {
            let _ = std::fs::remove_file(long_path(&path));
        }
    }
}

/// The files' part of a refresh, with what was found `online` (Err: the
/// lookup failed, nothing changes).
fn settle_files(job: &Job, online: Result<Option<Online>, String>) -> Result<Refreshed, String> {
    let online = online?;
    let (own_dir, managed) = match &job.dest {
        Dest::Web => return Ok(online.map_or(Refreshed::Nothing, |o| Refreshed::Web(o.keep))),
        Dest::Folder(d) => (Some(d), None),
        Dest::Managed(m) => (None, Some(m)),
    };
    let images: Vec<PathBuf> = job.current_file.iter().cloned().collect();
    let local = own_art(&job.files, &images, &job.dirs, job.max_edge, online.as_ref().and_then(|o| o.look.as_ref()));
    let Some((cover, url)) = pick(local, online) else {
        return Ok(Refreshed::Nothing);
    };
    let holds = |p: &Path| std::fs::read(long_path(p)).ok().as_deref() == Some(&cover.jpeg[..]);
    let visuals = derive_visuals(&cover.jpeg).map_err(|e| e.to_string())?;
    let is_own_managed = |f: &Path| {
        managed.is_some_and(|m| f.parent().is_some_and(|p| norm(p) == norm(m)))
            && f.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&format!("release-{}-", job.id)))
    };
    let mut backup = None;
    let mut undo = Undo::Nothing;
    // It shows that picture already (its own file, no url over it).
    let showing = job.shown_url.is_none() && job.current_file.as_deref().is_some_and(|f| holds(f));
    let (cover_ref, ambient_path) = if showing {
        let f = job.current_file.clone().unwrap_or_default();
        // A texture only where the album's own files are.
        let ambient = match own_dir {
            Some(d) if f.parent().is_some_and(|p| norm(p) == norm(d)) => Some(d.join(AMBIENT_FILE)),
            _ => is_own_managed(&f).then(|| f.with_extension("ambient.png")),
        };
        (job.current.clone().unwrap_or_else(|| cover_ref_of(&f, &job.root)), ambient)
    } else {
        let (target, fresh) = match (own_dir, managed) {
            (Some(d), _) => (d.join(COVER_FILE), false),
            (None, Some(m)) => match job.current_file.as_ref().filter(|f| is_own_managed(f) && holds(f)) {
                Some(f) => (f.clone(), false),
                None => (m.join(format!("release-{}-{}.jpg", job.id, job.stamp)), true),
            },
            (None, None) => return Ok(Refreshed::Nothing),
        };
        if fresh || !holds(&target) {
            if let Some(d) = own_dir {
                // The cover it shows, else the file this one replaces.
                let source = job
                    .current_file
                    .clone()
                    .filter(|f| long_path(f).is_file())
                    .or_else(|| long_path(&target).is_file().then(|| target.clone()));
                if let Some(src) = source {
                    let prev = d.join(PREV_FILE);
                    let staged = staging(&prev);
                    stage(&staged, |tmp| std::fs::copy(long_path(&src), long_path(tmp)).map(|_| ()))
                        .map_err(|e| format!("{}: {e}", src.display()))?;
                    if norm(&src) == norm(&target) {
                        undo = Undo::Restore { staged: staged.clone(), target: target.clone() };
                    }
                    backup = Some((staged, prev));
                }
            }
            if let Some(m) = managed {
                std::fs::create_dir_all(long_path(m)).map_err(|e| format!("{}: {e}", m.display()))?;
            }
            if let Err(e) = write_atomic(&target, &cover.jpeg) {
                if let Some((staged, _)) = &backup {
                    let _ = std::fs::remove_file(long_path(staged));
                }
                return Err(format!("{}: {e}", target.display()));
            }
            if fresh {
                undo = Undo::Remove(target.clone());
                if let Some(m) = managed {
                    let mut keep = vec![target.as_path()];
                    keep.extend(job.current_file.as_deref());
                    prune_managed(m, job.id, &keep);
                }
            }
        }
        (cover_ref_of(&target, &job.root), Some(ambient_of(&target, &job.dest)))
    };
    // Made again from the picked picture every time; one that cannot be
    // written is only a missing glow.
    if let Some(a) = &ambient_path
        && let Err(e) = write_atomic(a, &visuals.ambient_png)
    {
        log::warn!("cover refresh: {}: {e}", a.display());
    }
    Ok(Refreshed::File(Picked {
        cover_ref,
        seed: visuals.seed_argb,
        ambient: visuals.ambient_png,
        url,
        changed: !showing,
        backup,
        undo,
    }))
}

/// After the library took the picked cover (`saved`) or could not: the
/// staged cover becomes `cover.prev.jpg`, or the files go back.
fn finish(p: &Picked, saved: bool) {
    let staged = p.backup.as_ref().map(|(s, _)| s);
    if saved {
        if let Some((staged, prev)) = &p.backup
            && let Err(e) = swap(staged, prev)
        {
            log::warn!("cover refresh: {}: {e}", prev.display());
        }
        return;
    }
    match &p.undo {
        Undo::Restore { staged, target } => {
            if let Err(e) = swap(staged, target) {
                log::warn!("cover refresh: putting {} back: {e}", target.display());
            }
        }
        Undo::Remove(file) => {
            let _ = std::fs::remove_file(long_path(file));
            let _ = std::fs::remove_file(long_path(&file.with_extension("ambient.png")));
            if let Some(s) = staged {
                let _ = std::fs::remove_file(long_path(s));
            }
        }
        Undo::Nothing => {
            if let Some(s) = staged {
                let _ = std::fs::remove_file(long_path(s));
            }
        }
    }
}

/// The best cover found online so far, downloaded.
struct Fetched {
    /// The rendition that came, and what the stage of it takes.
    url: String,
    stage: String,
    bytes: Vec<u8>,
    /// Its long side as it came, and its look.
    side: u32,
    look: Look,
}

/// The heavy work of a refresh held ([`HEAVY`]): each blocking decode keeps
/// a clone, so it is let go only once the last decode ended, even when the
/// refresh was given up.
type Hold = Arc<dyn std::any::Any + Send + Sync>;

/// `bytes` measured within the caps off the loop, the heavy work held.
async fn measured(bytes: Vec<u8>, hold: &Hold) -> Option<(Vec<u8>, (u32, Look))> {
    let hold = hold.clone();
    tokio::task::spawn_blocking(move || {
        let _hold = hold;
        measure(&bytes).map(|m| (bytes, m))
    })
    .await
    .ok()
    .flatten()
}

/// The picture of `found` by `fetch`: its largest rendition that comes
/// and is a picture within the limits ([`measure`]), each in the time one
/// image has and none past `deadline`. A frame of a video is none (at any
/// size). A failure `fetch` calls a trouble of the url the source gave, or
/// no answer in time, goes into `trouble`.
async fn fetch_one<F, Fut>(found: &hd_covers::Found, fetch: &F, deadline: Instant, hold: &Hold, trouble: &mut Option<String>) -> Option<Fetched>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<u8>, (String, bool)>>,
{
    let last = found.urls.len().saturating_sub(1);
    for (i, url) in found.urls.iter().enumerate() {
        let until = deadline.min(Instant::now() + IMAGE_BUDGET);
        let bytes = match tokio::time::timeout_at(until, fetch(url.clone())).await {
            Ok(Ok(bytes)) => bytes,
            Ok(Err((e, hard))) => {
                log::debug!("cover refresh: {url}: {e}");
                if hard && i == last {
                    trouble.get_or_insert(format!("{url}: {e}"));
                }
                continue;
            }
            Err(_) => {
                log::debug!("cover refresh: {url}: no answer in time");
                trouble.get_or_insert(format!("{url}: no answer in time"));
                continue;
            }
        };
        if dimensions(&bytes).is_some_and(|(w, h)| cover_frame::is_frame(w, h)) {
            log::debug!("cover refresh: {url} is a frame of a video");
            return None;
        }
        match measured(bytes, hold).await {
            Some((bytes, (side, look))) => {
                return Some(Fetched { url: url.clone(), stage: found.stage.clone(), bytes, side, look });
            }
            None => log::debug!("cover refresh: {url} is no picture it takes"),
        }
    }
    None
}

/// The album's art among `found`, taken in their order
/// ([`hd_covers::find_all`]; at most [`MAX_CANDIDATES`] downloaded) until
/// `deadline`: the first that comes as a picture is the anchor — the
/// linked release's, else the one of the very title of the first source;
/// a later one takes its place only when it is the same picture
/// ([`same_picture`]) and larger. Other editions never win by their size;
/// one that only lends its size ([`hd_covers::Found::lends`]) is never the
/// anchor. Only the best one's bytes are kept. Err: none came and a
/// source could not answer, or time ran out first.
async fn select<F, Fut>(found: Vec<hd_covers::Found>, fetch: &F, deadline: Instant, hold: &Hold) -> Result<Option<Fetched>, String>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<u8>, (String, bool)>>,
{
    let mut anchor: Option<Look> = None;
    let mut best: Option<Fetched> = None;
    let mut trouble = None;
    let mut downloads = 0;
    for f in &found {
        if f.lends && anchor.is_none() {
            continue;
        }
        if downloads == MAX_CANDIDATES {
            break;
        }
        if Instant::now() >= deadline {
            log::info!("cover refresh: out of time, the best found so far");
            trouble.get_or_insert("the downloads ran out of time".to_owned());
            break;
        }
        downloads += 1;
        let Some(got) = fetch_one(f, fetch, deadline, hold, &mut trouble).await else { continue };
        match &anchor {
            None => {
                anchor = Some(got.look);
                best = Some(got);
            }
            Some(a) => {
                if same_picture(Some(a), Some(&got.look)) && best.as_ref().is_none_or(|b| got.side > b.side) {
                    best = Some(got);
                }
            }
        }
    }
    match (best, trouble) {
        (Some(b), _) => Ok(Some(b)),
        (None, Some(e)) => Err(e),
        (None, None) => Ok(None),
    }
}

/// The url the album keeps of `best`: what the stage takes when that
/// comes (in the time left) as a picture within the caps, no frame, the
/// same picture as `best`; else the rendition that came. Never one nobody
/// has seen come.
async fn keep_url<F, Fut>(best: &Fetched, fetch: &F, deadline: Instant, hold: &Hold) -> String
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<u8>, (String, bool)>>,
{
    if best.stage == best.url || Instant::now() >= deadline {
        return best.url.clone();
    }
    let until = deadline.min(Instant::now() + IMAGE_BUDGET);
    let Ok(Ok(bytes)) = tokio::time::timeout_at(until, fetch(best.stage.clone())).await else { return best.url.clone() };
    if dimensions(&bytes).is_none_or(|(w, h)| cover_frame::is_frame(w, h)) {
        return best.url.clone();
    }
    match measured(bytes, hold).await {
        Some((_, (_, look))) if same_picture(Some(&best.look), Some(&look)) => best.stage.clone(),
        _ => best.url.clone(),
    }
}

/// The album's art online: the covers [`hd_covers::find_all`] finds in
/// [`ONLINE_TIMEOUT`] less the time kept for the downloads
/// ([`DOWNLOAD_TIME`]), downloaded one by one ([`select`]) within it, the
/// heavy work held (`hold`): when time runs out, the best that came. Err:
/// nothing came and a source could not answer, or time ran out first.
async fn online(meta: &Metadata, services: &[Arc<dyn StreamService>], w: &hd_covers::Want, max_edge: u32, hold: &Hold) -> Result<Option<Online>, String> {
    let deadline = Instant::now() + ONLINE_TIMEOUT;
    let found = hd_covers::find_all(Some(meta), services, w, true, Some(deadline - DOWNLOAD_TIME)).await?;
    if found.is_empty() {
        return Ok(None);
    }
    let fetch = |url: String| async move { meta.cover_art.download(&url).await.map_err(|e| (e.to_string(), better_covers::meta_trouble(&e))) };
    let Some(best) = select(found, &fetch, deadline, hold).await? else { return Ok(None) };
    log::info!("cover refresh of «{}»: {} of {} px online", w.title, best.url, best.side);
    let keep = keep_url(&best, &fetch, deadline, hold).await;
    let Fetched { bytes, side, look, .. } = best;
    let held = hold.clone();
    let cover = tokio::task::spawn_blocking(move || {
        let _hold = held;
        process_cover(&bytes, max_edge)
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(cover.ok().map(|cover| Online { keep, cover, side, look: Some(look) }))
}

impl Worker {
    /// «Обновить обложку» of a library album (see the module docs): read,
    /// looked up and written off the loop, [`Self::release_cover_refreshed`]
    /// takes the answer.
    pub(crate) fn refresh_release_cover(&mut self, id: ReleaseId) {
        if !self.cover_refresh.insert(id) {
            return;
        }
        let lib = self.lib.clone();
        let root = self.res.root().to_owned();
        let masters = self.import.as_ref().map(|s| s.config().masters_dir());
        let max_edge = self.import.as_ref().map_or(MAX_EDGE, |s| s.config().cover_max_edge);
        let meta = self.import.as_ref().filter(|s| s.config().fetch_cover_online).and_then(|s| s.metadata());
        let services: Vec<Arc<dyn StreamService>> =
            [Service::Deezer, Service::Youtube, Service::Spotify].into_iter().filter_map(|k| self.service(k)).collect();
        let stamp = now_ms();
        self.streaming.spawn_internal(async move {
            let job = tokio::task::spawn_blocking(move || gather(&lib, id, &root, masters.as_deref(), max_edge, stamp))
                .await
                .ok()
                .flatten();
            let Some(job) = job else { return Internal::CoverRefreshed(id, Ok(Refreshed::Nothing)) };
            log::info!("cover refresh of «{}»: into {:?}, {} files, online {}", job.want.title, job.dest, job.files.len(), meta.is_some());
            // One refresh at a time decodes pictures, until its last decode
            // ended (each holds a clone).
            let Ok(Ok(permit)) = tokio::time::timeout(HEAVY_WAIT, HEAVY.acquire()).await else {
                return Internal::CoverRefreshed(id, Err("another cover refresh took too long".to_owned()));
            };
            let hold: Hold = Arc::new(permit);
            let found = match &meta {
                // Its own steps keep within ONLINE_TIMEOUT; this is the last
                // guard.
                Some(m) => match tokio::time::timeout(ONLINE_TIMEOUT + GUARD, online(m, &services, &job.want, job.max_edge, &hold)).await {
                    Ok(r) => r,
                    Err(_) => Err("the online lookup took too long".to_owned()),
                },
                None => Ok(None),
            };
            let result = tokio::task::spawn_blocking(move || {
                let _hold = hold;
                let _claim = match &job.dest {
                    Dest::Folder(d) => Some(Claim::take(d).ok_or_else(|| format!("{} is being written by another refresh", d.display()))?),
                    Dest::Managed(_) | Dest::Web => None,
                };
                settle_files(&job, found)
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
            Internal::CoverRefreshed(id, result)
        });
    }

    /// A refresh came back. The library takes the cover ref, the palette,
    /// what the lookups keep and the decision at once; then the staged
    /// cover becomes the backup (or, when the library could not, the files
    /// go back, off the loop) and everything on screen takes it. Nothing
    /// better, or a failure: what it shows stays, with a toast.
    pub(crate) fn release_cover_refreshed(&mut self, id: ReleaseId, result: Result<Refreshed, String>) {
        self.cover_refresh.remove(&id);
        let current = self.lib.get_release(id).ok().flatten().and_then(|r| r.cover_ref);
        let now = now_ms();
        match result {
            Ok(Refreshed::File(p)) => {
                log::info!("cover refresh of {id}: {} from {}", p.cover_ref, p.url.as_deref().unwrap_or("its files"));
                let fallback = self.import.as_ref().map_or(FALLBACK_SEED, |s| s.config().fallback_seed_argb);
                let mut palette = Palette::new(id, p.seed.unwrap_or(fallback));
                palette.ambient_blur = Some(p.ambient.clone());
                let mut puts = vec![better_covers::settled(id, Some(&p.cover_ref), ""), (decision_key(id), p.cover_ref.clone())];
                let mut drops = Vec::new();
                match &p.url {
                    Some(u) => puts.push((hd_covers::key(id), u.clone())),
                    None => drops.push(hd_covers::key(id)),
                }
                let saved = self.lib.settle_cover(id, Some(&p.cover_ref), Some(&palette), now, &puts, &drops);
                let ok = saved.is_ok();
                let changed = p.changed;
                self.streaming.spawn_reporting(move |_| async move {
                    let _ = tokio::task::spawn_blocking(move || finish(&p, ok)).await;
                });
                match saved {
                    Ok(()) => {
                        self.reload_after_import();
                        if !changed {
                            self.toast("Лучшей обложки не нашлось", None);
                        }
                    }
                    Err(e) => {
                        log::warn!("cover refresh of {id}: {e}");
                        self.toast("Не получилось обновить обложку", None);
                    }
                }
            }
            Ok(Refreshed::Web(url)) => {
                log::info!("cover refresh of {id}: {url}");
                let puts = vec![
                    better_covers::settled(id, current.as_deref(), &url),
                    (hd_covers::key(id), url.clone()),
                    (decision_key(id), current.clone().unwrap_or_default()),
                ];
                match self.lib.settle_cover(id, None, None, now, &puts, &[]) {
                    Ok(()) => {
                        // The service may have put another picture at the
                        // same address.
                        self.publish(|s| s.refetch_covers = (s.refetch_covers.0 + 1, vec![url]));
                        self.reload_after_import();
                    }
                    Err(e) => {
                        log::warn!("cover refresh of {id}: {e}");
                        self.toast("Не получилось обновить обложку", None);
                    }
                }
            }
            Ok(Refreshed::Nothing) => {
                log::info!("cover refresh of {id}: nothing to pick from");
                self.toast("Лучшей обложки не нашлось", None);
            }
            Err(e) => {
                log::warn!("cover refresh of {id}: {e}");
                self.toast("Не получилось обновить обложку", None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn jpeg(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        let mut out = Vec::new();
        image::RgbImage::from_pixel(w, h, image::Rgb(rgb))
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Jpeg)
            .unwrap();
        out
    }

    fn cover(w: u32, h: u32, rgb: [u8; 3]) -> ProcessedCover {
        process_cover(&jpeg(w, h, rgb), MAX_EDGE).unwrap()
    }

    fn picked(r: Refreshed) -> Picked {
        match r {
            Refreshed::File(p) => p,
            other => panic!("{other:?}"),
        }
    }

    /// A picture of 9×8 blocks drawn from `seed` in the colour `tint`: the
    /// same seed is the same drawing at any size, another seed another.
    fn tinted(w: u32, h: u32, seed: u8, tint: [u8; 3]) -> Vec<u8> {
        let level = |bx: u32, by: u32| -> u32 {
            let mut x = (u64::from(seed) + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ u64::from(bx * 31 + by * 7 + 1).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
            x ^= x >> 29;
            x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
            x ^= x >> 32;
            u32::try_from(x % 200).unwrap() + 28
        };
        let img = image::RgbImage::from_fn(w, h, |x, y| {
            let v = level(x * 9 / w, y * 8 / h);
            image::Rgb(tint.map(|t| u8::try_from(v * u32::from(t) / 255).unwrap()))
        });
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Jpeg).unwrap();
        out
    }

    /// [`tinted`] in greys.
    fn drawing(w: u32, h: u32, seed: u8) -> Vec<u8> {
        tinted(w, h, seed, [255, 255, 255])
    }

    fn look_of(bytes: &[u8]) -> Option<Look> {
        measure(bytes).map(|(_, l)| l)
    }

    fn own(bytes: &[u8]) -> Own {
        let (side, look) = measure(bytes).unwrap();
        Own { cover: process_cover(bytes, MAX_EDGE).unwrap(), side, look: Some(look) }
    }

    fn online_bytes(url: &str, bytes: &[u8]) -> Online {
        let (side, look) = measure(bytes).unwrap();
        Online { keep: url.to_owned(), cover: process_cover(bytes, MAX_EDGE).unwrap(), side, look: Some(look) }
    }

    fn online_of(url: &str, c: &ProcessedCover) -> Online {
        let (side, look) = measure(&c.jpeg).unwrap();
        Online { keep: url.to_owned(), cover: c.clone(), side, look: Some(look) }
    }

    /// A picture looks the same at any size; another picture does not.
    #[test]
    fn a_picture_is_the_same_at_any_size_and_another_is_not() {
        let a = look_of(&drawing(1000, 1000, 7));
        assert!(same_picture(a.as_ref(), look_of(&drawing(1800, 1800, 7)).as_ref()));
        assert!(same_picture(a.as_ref(), look_of(&drawing(544, 544, 7)).as_ref()));
        assert!(!same_picture(a.as_ref(), look_of(&drawing(1000, 1000, 8)).as_ref()));
        assert!(!same_picture(a.as_ref(), None));
        assert_eq!(measure(&drawing(1800, 1200, 7)).map(|(s, _)| s), Some(1800));
        assert_eq!(measure(&drawing(4200, 100, 7)), None, "larger than it decodes");
        assert_eq!(measure(b"<html>"), None);
    }

    /// Flat colours tell nothing: never one picture, not even with the same
    /// colour. The same drawing in other colours is another picture.
    /// «рисунки»: the pale drawing is one picture at 544 and 1440 px, the
    /// dark photo of the single another.
    #[test]
    fn flat_colours_and_other_colours_are_never_the_same_picture() {
        let (red, blue) = (look_of(&jpeg(1200, 1200, [200, 20, 20])), look_of(&jpeg(1000, 1000, [20, 20, 200])));
        assert!(!same_picture(red.as_ref(), blue.as_ref()));
        assert!(!same_picture(red.as_ref(), red.as_ref()));
        let warm = look_of(&tinted(1000, 1000, 4, [255, 90, 60]));
        let cold = look_of(&tinted(1000, 1000, 4, [60, 90, 255]));
        assert!(!same_picture(warm.as_ref(), cold.as_ref()));
        assert!(same_picture(warm.as_ref(), look_of(&tinted(1400, 1400, 4, [255, 90, 60])).as_ref()));
        let pale = [230, 245, 240];
        let drawing_small = look_of(&tinted(544, 544, 2, pale));
        assert!(same_picture(drawing_small.as_ref(), look_of(&tinted(1440, 1440, 2, pale)).as_ref()));
        assert!(!same_picture(drawing_small.as_ref(), look_of(&tinted(1200, 1200, 1, [90, 110, 50])).as_ref()));
    }

    /// «рисунки»: its files hold the lead single's art (1200 px, in the
    /// first track) and the album's (544 px, in the others); online, the
    /// album's art of 1000 px. That wins over the larger single's: another
    /// picture. The same picture larger in the files wins over it; as large
    /// or smaller, not. Flat colours are never the same picture: the online
    /// one wins. A frame of a video never.
    #[test]
    fn the_album_art_found_online_beats_other_pictures_of_its_files() {
        let (single, small) = (drawing(1200, 1200, 1), drawing(544, 544, 2));
        let album = online_bytes("https://x/album.jpg", &drawing(1000, 1000, 2));
        let (got, url) = pick(vec![own(&single), own(&small)], Some(album.clone())).unwrap();
        assert_eq!((got, url.as_deref()), (album.cover.clone(), Some("https://x/album.jpg")));
        let scan = own(&drawing(1400, 1400, 2));
        let (got, url) = pick(vec![own(&single), scan.clone()], Some(album.clone())).unwrap();
        assert_eq!((got, url), (scan.cover.clone(), None));
        let as_large = own(&drawing(1000, 1000, 2));
        assert_eq!(pick(vec![as_large], Some(album.clone())).and_then(|(_, u)| u).as_deref(), Some("https://x/album.jpg"));
        let blue = online_bytes("https://x/blue.jpg", &jpeg(1000, 1000, [20, 20, 200]));
        assert_eq!(pick(vec![own(&jpeg(1200, 1200, [200, 20, 20]))], Some(blue)).and_then(|(_, u)| u).as_deref(), Some("https://x/blue.jpg"));
        // Nothing online: the largest of the files.
        assert_eq!(pick(vec![own(&small), own(&single)], None).map(|(c, _)| c), Some(own(&single).cover));
        let frame = own(&drawing(1280, 720, 3));
        assert_eq!(pick(vec![frame.clone(), own(&small)], None).map(|(c, _)| c), Some(own(&small).cover));
        assert!(pick(vec![frame], None).is_none());
        assert!(pick(Vec::new(), None).is_none());
    }

    fn found(linked: bool, urls: &[&str]) -> hd_covers::Found {
        hd_covers::Found {
            linked,
            exact: true,
            order: 0,
            lends: false,
            urls: urls.iter().map(|u| (*u).to_owned()).collect(),
            stage: (*urls.last().unwrap()).to_owned(),
        }
    }

    fn far() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }

    fn hold() -> Hold {
        Arc::new(())
    }

    /// A picture whose light grows to the right and down, or to the right
    /// and up: the same steps across, the same mean, other steps down.
    fn gradient(w: u32, h: u32, up: bool) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |x, y| {
            let y = if up { h - 1 - y } else { y };
            let v = u8::try_from(20 + 180 * x / w + 50 * y / h).unwrap();
            image::Rgb([v, v, v])
        });
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Jpeg).unwrap();
        out
    }

    /// Two gradients the same across but turned upside down are two
    /// pictures: a larger one never takes the anchor's place.
    #[tokio::test]
    async fn a_picture_upside_down_is_another() {
        let (a, b) = (gradient(600, 600, false), gradient(900, 900, true));
        assert!(!same_picture(look_of(&a).as_ref(), look_of(&b).as_ref()));
        assert!(same_picture(look_of(&a).as_ref(), look_of(&gradient(900, 900, false)).as_ref()));
        let pics: HashMap<&str, Vec<u8>> = HashMap::from([("a", a), ("b", b)]);
        let fetch = |url: String| {
            let got = pics.get(url.as_str()).cloned();
            async move { got.ok_or_else(|| ("404".to_owned(), false)) }
        };
        let got = select(vec![found(true, &["a"]), found(false, &["b"])], &fetch, far(), &hold()).await.unwrap().unwrap();
        assert_eq!((got.url.as_str(), got.side), ("a", 600));
    }

    /// A cover that only lends its size (a linked Spotify album by its
    /// public page) is never the anchor: alone it is not even downloaded;
    /// after a verified cover of the same picture it lends its size.
    #[tokio::test]
    async fn a_cover_that_lends_is_never_the_anchor() {
        let pics: HashMap<&str, Vec<u8>> = HashMap::from([("dz", drawing(600, 600, 2)), ("sp", drawing(900, 900, 2)), ("single", drawing(900, 900, 5))]);
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let fetch = |url: String| {
            calls.fetch_add(1, Ordering::Relaxed);
            let got = pics.get(url.as_str()).cloned();
            async move { got.ok_or_else(|| ("404".to_owned(), false)) }
        };
        let lends = |url: &str| hd_covers::Found { lends: true, ..found(false, &[url]) };
        assert!(select(vec![lends("single")], &fetch, far(), &hold()).await.unwrap().is_none());
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        let got = select(vec![found(false, &["dz"]), lends("sp")], &fetch, far(), &hold()).await.unwrap().unwrap();
        assert_eq!(got.url, "sp");
        let got = select(vec![found(false, &["dz"]), lends("single")], &fetch, far(), &hold()).await.unwrap().unwrap();
        assert_eq!(got.url, "dz");
    }

    /// The pictures of the album's own are measured within the caps first:
    /// one larger than [`MAX_SIDE`] and a frame of a video are none; of the
    /// others only the largest and the largest of the picture found online
    /// are kept.
    #[test]
    fn the_files_art_is_measured_within_the_caps_and_few_are_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let put = |name: &str, bytes: Vec<u8>| {
            let p = tmp.path().join(name);
            std::fs::write(&p, bytes).unwrap();
            p
        };
        let huge = put("huge.jpg", jpeg(4100, 4100, [9, 9, 9]));
        let frame = put("frame.jpg", drawing(1920, 1080, 3));
        let single = put("single.jpg", drawing(1200, 1200, 1));
        let album = put("album.jpg", drawing(544, 544, 2));
        let other = put("other.jpg", drawing(300, 300, 6));
        let online = look_of(&drawing(1000, 1000, 2));
        let kept = own_art(&[], &[huge, frame, single, album, other], &[], MAX_EDGE, online.as_ref());
        let sides: Vec<u32> = kept.iter().map(|o| o.side).collect();
        assert_eq!(sides, [1200, 544]);
        let alone = own_art(&[], &[put("only.jpg", drawing(700, 700, 4))], &[], MAX_EDGE, None);
        assert_eq!(alone.iter().map(|o| o.side).collect::<Vec<_>>(), [700]);
    }

    /// The anchor is the first that comes: the linked release's, else the
    /// first found by the very title. A larger one takes its place only as
    /// the same picture; another edition never wins by its size.
    #[tokio::test]
    async fn the_anchor_takes_only_its_own_picture_larger() {
        let pics: HashMap<&str, Vec<u8>> = HashMap::from([
            ("yt", drawing(600, 600, 2)),
            ("dz", drawing(900, 900, 2)),
            ("it", drawing(1500, 1500, 5)),
        ]);
        let fetch = |url: String| {
            let got = pics.get(url.as_str()).cloned();
            async move { got.ok_or_else(|| ("404".to_owned(), false)) }
        };
        let urls = |b: Option<Fetched>| b.map(|b| (b.url, b.side));
        let all = vec![found(true, &["yt"]), found(false, &["dz"]), found(false, &["it"])];
        assert_eq!(urls(select(all, &fetch, far(), &hold()).await.unwrap()), Some(("dz".to_owned(), 900)));
        let by_title = vec![found(false, &["dz"]), found(false, &["it"])];
        assert_eq!(urls(select(by_title, &fetch, far(), &hold()).await.unwrap()), Some(("dz".to_owned(), 900)));
        assert_eq!(urls(select(vec![found(false, &["gone"])], &fetch, far(), &hold()).await.unwrap()), None);
    }

    /// A frame of a video found first is passed over: the next cover is the
    /// anchor. At most [`MAX_CANDIDATES`] are downloaded.
    #[tokio::test]
    async fn a_frame_is_passed_over_and_the_downloads_are_few() {
        let pics: HashMap<&str, Vec<u8>> = HashMap::from([("frame", drawing(1280, 720, 3)), ("square", drawing(700, 700, 2))]);
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let fetch = |url: String| {
            calls.fetch_add(1, Ordering::Relaxed);
            let got = pics.get(url.as_str()).cloned().unwrap_or_else(|| drawing(64, 64, 9));
            async move { Ok::<_, (String, bool)>(got) }
        };
        let got = select(vec![found(true, &["frame"]), found(false, &["square"])], &fetch, far(), &hold()).await.unwrap().unwrap();
        assert_eq!(got.url, "square");
        calls.store(0, Ordering::Relaxed);
        let many: Vec<hd_covers::Found> = (0..9).map(|_| found(false, &["other"])).collect();
        assert!(select(many, &fetch, far(), &hold()).await.unwrap().is_some());
        assert_eq!(calls.load(Ordering::Relaxed), MAX_CANDIDATES);
    }

    /// Each cover comes at its largest rendition that downloads; a source
    /// that stalls past the deadline leaves the best already come; nothing
    /// come and a source that could not answer is a failure.
    #[tokio::test]
    async fn the_best_that_came_in_time_is_taken() {
        let pics: HashMap<&str, Vec<u8>> = HashMap::from([("plain", drawing(1000, 1000, 2)), ("first", drawing(500, 500, 2))]);
        let fetch = |url: String| {
            let got = pics.get(url.as_str()).cloned();
            async move {
                match url.as_str() {
                    "slow" => {
                        tokio::time::sleep(Duration::from_secs(10)).await;
                        Err(("late".to_owned(), true))
                    }
                    "big" => Err(("403".to_owned(), true)),
                    _ => got.ok_or_else(|| ("network".to_owned(), true)),
                }
            }
        };
        let got = select(vec![found(true, &["big", "plain"])], &fetch, far(), &hold()).await.unwrap().unwrap();
        assert_eq!((got.url.as_str(), got.side), ("plain", 1000));
        let soon = Instant::now() + Duration::from_millis(300);
        let got = select(vec![found(true, &["first"]), found(false, &["slow"])], &fetch, soon, &hold()).await.unwrap().unwrap();
        assert_eq!(got.url, "first");
        assert!(select(vec![found(true, &["down"])], &fetch, far(), &hold()).await.is_err());
        let soon = Instant::now() + Duration::from_millis(300);
        assert!(select(vec![found(true, &["slow"])], &fetch, soon, &hold()).await.is_err(), "time ran out with nothing come");
    }

    /// The album keeps what the stage takes once it came as a picture, no
    /// frame, the same picture as the one that won; else the rendition that
    /// came. Never a url nobody saw come.
    #[tokio::test]
    async fn the_url_kept_is_one_that_came() {
        let best = Fetched { url: "big".into(), stage: "stage".into(), bytes: Vec::new(), side: 3000, look: look_of(&drawing(64, 64, 1)).unwrap() };
        let comes = |_: String| async { Ok::<_, (String, bool)>(drawing(64, 64, 1)) };
        assert_eq!(keep_url(&best, &comes, far(), &hold()).await, "stage");
        let refused = |_: String| async { Err::<Vec<u8>, _>(("403".to_owned(), true)) };
        assert_eq!(keep_url(&best, &refused, far(), &hold()).await, "big");
        let page = |_: String| async { Ok::<_, (String, bool)>(b"<html>".to_vec()) };
        assert_eq!(keep_url(&best, &page, far(), &hold()).await, "big");
        let another = |_: String| async { Ok::<_, (String, bool)>(drawing(64, 64, 8)) };
        assert_eq!(keep_url(&best, &another, far(), &hold()).await, "big");
        let frame = |_: String| async { Ok::<_, (String, bool)>(drawing(1280, 720, 1)) };
        assert_eq!(keep_url(&best, &frame, far(), &hold()).await, "big");
    }

    /// A folder is the album's alone when all its files are there and
    /// nothing of another album is; never the masters' or the app's, never
    /// outside the library.
    #[test]
    fn only_a_folder_of_the_album_alone_is_written() {
        let root = Path::new("/lib");
        let (me, other) = (ReleaseId::new(), ReleaseId::new());
        let f = |p: &str| PathBuf::from(p);
        let mine = vec![f("/lib/A/2024 - X/01.opus"), f("/lib/A/2024 - X/02.opus")];
        let places = vec![(me, f("/lib/A/2024 - X/01.opus")), (other, f("/lib/A/2023 - Y/01.opus"))];
        assert_eq!(owned_folder(me, &mine, None, root, None, &places), Some(f("/lib/A/2024 - X")));
        let mut shared = places.clone();
        shared.push((other, f("/lib/A/2024 - X/03.opus")));
        assert_eq!(owned_folder(me, &mine, None, root, None, &shared), None);
        let mut covered = places.clone();
        covered.push((other, f("/lib/A/2024 - X/cover.jpg")));
        assert_eq!(owned_folder(me, &mine, None, root, None, &covered), None);
        let split = vec![f("/lib/A/2024 - X/01.opus"), f("/lib/A/2024 - X (CD2)/01.opus")];
        assert_eq!(owned_folder(me, &split, None, root, None, &places), None);
        let masters = vec![f("/lib/.musiclib/masters/A/X/01.flac")];
        assert_eq!(owned_folder(me, &masters, None, root, None, &[]), None);
        assert_eq!(owned_folder(me, &[f("/m/A/X/01.flac")], None, root, Some(Path::new("/m")), &[]), None);
        assert_eq!(owned_folder(me, &[f("/elsewhere/X/01.opus")], None, root, None, &[]), None);
        assert_eq!(owned_folder(me, &[f("/lib/01.opus")], None, root, None, &[]), None, "the library's root");
        assert_eq!(owned_folder(me, &[], Some(Path::new("/lib/A/X/cover.jpg")), root, None, &[]), Some(f("/lib/A/X")));
        // Its cover names a folder that is gone: its files' folder still is.
        assert_eq!(owned_folder(me, &mine, Some(Path::new("/lib/Gone/cover.jpg")), root, None, &places), Some(f("/lib/A/2024 - X")));
    }

    /// A copy of a track in another folder (not the one that plays) makes
    /// the album's folder not its alone; one folder with all of it is.
    #[test]
    fn the_copies_of_its_tracks_count_for_its_folder() {
        use nikgob_core::model::{ArtistCredit, Codec, Release, Source, Track};
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let lib = Library::open_in_memory().unwrap();
        let release = |title: &str, files: &[&str]| {
            let r = lib.upsert_release(&Release::new(title, ArtistCredit::single("X", None))).unwrap();
            let mut t = Track::new(format!("{title} song"), ArtistCredit::single("X", None));
            t.release_id = Some(r.id);
            let t = lib.upsert_track(&t).unwrap();
            for f in files {
                std::fs::create_dir_all(root.join(f).parent().unwrap()).unwrap();
                let path = root.join(f).to_string_lossy().into_owned();
                lib.add_source(&Source::new(t.id, SourceKind::LocalFile, path, Codec::Opus)).unwrap();
            }
            r.id
        };
        let two = release("Two", &["A/Two/01.opus", "B/Two/01.opus"]);
        let one = release("One", &["C/One/01.opus"]);
        let dest = |id| gather(&lib, id, root, None, MAX_EDGE, 1).unwrap().dest;
        assert!(matches!(dest(two), Dest::Managed(_)));
        assert!(matches!(dest(one), Dest::Folder(d) if norm(&d) == norm(&root.join("C/One"))));
    }

    fn want() -> hd_covers::Want {
        hd_covers::Want {
            id: ReleaseId::new(),
            mbid: None,
            group_mbid: None,
            linked: Vec::new(),
            title: "X".into(),
            artists: Vec::new(),
            release_type: nikgob_core::model::ReleaseType::Album,
            tracks: 9,
        }
    }

    fn job(root: &Path, dest: Dest, current: Option<&str>) -> Job {
        Job {
            id: ReleaseId::new(),
            dest,
            root: root.to_owned(),
            current: current.map(str::to_owned),
            current_file: current.filter(|c| !is_web_ref(c)).map(|c| root.join(c)),
            shown_url: current.filter(|c| is_web_ref(c)).map(str::to_owned),
            files: Vec::new(),
            dirs: Vec::new(),
            want: want(),
            max_edge: MAX_EDGE,
            stamp: 7,
        }
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect()
    }

    /// The album's own folder: the album's art over the single's
    /// `cover.jpg`; the single's staged, and `cover.prev.jpg` (an older one)
    /// replaced by it only once the library took the new cover. Asked
    /// again, it shows that picture already.
    #[test]
    fn a_refresh_writes_the_cover_of_its_own_folder_and_keeps_the_one_it_replaced() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let dir = root.join("A/2024 - X");
        std::fs::create_dir_all(&dir).unwrap();
        let single = jpeg(1000, 1000, [200, 20, 20]);
        std::fs::write(dir.join("cover.jpg"), &single).unwrap();
        std::fs::write(dir.join("cover.prev.jpg"), b"older").unwrap();
        let album = cover(1000, 1000, [20, 20, 200]);
        let mut j = job(root, Dest::Folder(dir.clone()), Some("A/2024 - X/cover.jpg"));
        j.dirs = vec![dir.clone()];
        let p = picked(settle_files(&j, Ok(Some(online_of("https://x/album.jpg", &album)))).unwrap());
        assert_eq!((p.cover_ref.as_str(), p.url.as_deref(), p.changed), ("A/2024 - X/cover.jpg", Some("https://x/album.jpg"), true));
        assert_eq!(std::fs::read(dir.join("cover.jpg")).unwrap(), album.jpeg);
        assert_eq!(std::fs::read(dir.join("cover.prev.jpg")).unwrap(), b"older", "not before the library took it");
        assert!(dir.join("ambient.png").is_file());
        finish(&p, true);
        assert_eq!(std::fs::read(dir.join("cover.prev.jpg")).unwrap(), single);
        assert!(leftovers(&dir).is_empty());
        let again = picked(settle_files(&j, Ok(Some(online_of("https://x/album.jpg", &album)))).unwrap());
        assert!(!again.changed && again.backup.is_none());
        let offline = picked(settle_files(&j, Ok(None)).unwrap());
        assert!(!offline.changed);
        // A lookup that failed changes nothing.
        assert!(settle_files(&j, Err("Deezer: network".into())).is_err());
    }

    /// The library could not take the new cover: the old one comes back
    /// and `cover.prev.jpg` stays as it was.
    #[test]
    fn a_cover_the_library_could_not_take_goes_back() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let dir = root.join("A/X");
        std::fs::create_dir_all(&dir).unwrap();
        let old = jpeg(500, 500, [200, 20, 20]);
        std::fs::write(dir.join("cover.jpg"), &old).unwrap();
        std::fs::write(dir.join("cover.prev.jpg"), b"older").unwrap();
        let mut j = job(root, Dest::Folder(dir.clone()), Some("A/X/cover.jpg"));
        j.dirs = vec![dir.clone()];
        let p = picked(settle_files(&j, Ok(Some(online_of("https://x/a.jpg", &cover(1000, 1000, [1, 1, 200]))))).unwrap());
        finish(&p, false);
        assert_eq!(std::fs::read(dir.join("cover.jpg")).unwrap(), old);
        assert_eq!(std::fs::read(dir.join("cover.prev.jpg")).unwrap(), b"older");
        assert!(leftovers(&dir).is_empty());
    }

    /// The backup is the cover the album shows: its `folder.png`, not an
    /// unregistered `cover.jpg` of its folder.
    #[test]
    fn the_backup_is_the_cover_it_shows() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let dir = root.join("A/X");
        std::fs::create_dir_all(&dir).unwrap();
        let shown = jpeg(300, 300, [9, 9, 9]);
        std::fs::write(dir.join("folder.jpg"), &shown).unwrap();
        std::fs::write(dir.join("cover.jpg"), jpeg(200, 200, [1, 2, 3])).unwrap();
        let mut j = job(root, Dest::Folder(dir.clone()), Some("A/X/folder.jpg"));
        j.dirs = vec![dir.clone()];
        let p = picked(settle_files(&j, Ok(Some(online_of("https://x/a.jpg", &cover(1000, 1000, [1, 1, 200]))))).unwrap());
        assert_eq!(p.cover_ref, "A/X/cover.jpg");
        assert_eq!(p.undo, Undo::Nothing, "what it showed is still there");
        finish(&p, true);
        assert_eq!(std::fs::read(dir.join("cover.prev.jpg")).unwrap(), shown);
    }

    /// A folder shared with another album is not touched: the art becomes
    /// a file of the album's own in the covers folder, a fresh name each
    /// time, the one before kept and older ones gone; when the library
    /// cannot take it, the new file goes.
    #[test]
    fn a_shared_folder_is_left_alone_and_the_cover_is_the_albums_own_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let shared = root.join("A/Singles");
        std::fs::create_dir_all(&shared).unwrap();
        let theirs = jpeg(600, 600, [200, 20, 20]);
        std::fs::write(shared.join("cover.jpg"), &theirs).unwrap();
        let managed = root.join(MANAGED_DIR);
        let mut j = job(root, Dest::Managed(managed.clone()), Some("A/Singles/cover.jpg"));
        let album = cover(1000, 1000, [20, 20, 200]);
        let p = picked(settle_files(&j, Ok(Some(online_of("https://x/album.jpg", &album)))).unwrap());
        let want_ref = format!(".musiclib/covers/release-{}-7.jpg", j.id);
        assert_eq!(p.cover_ref, want_ref);
        assert_eq!(std::fs::read(shared.join("cover.jpg")).unwrap(), theirs, "the shared folder is untouched");
        assert!(!shared.join("cover.prev.jpg").exists() && !shared.join("ambient.png").exists());
        let first = managed.join(format!("release-{}-7.jpg", j.id));
        assert_eq!(std::fs::read(&first).unwrap(), album.jpeg);
        assert!(first.with_extension("ambient.png").is_file());
        j.current = Some(want_ref.clone());
        j.current_file = Some(first.clone());
        assert!(!picked(settle_files(&j, Ok(Some(online_of("https://x/album.jpg", &album)))).unwrap()).changed);
        for (stamp, rgb) in [(8, [1, 200, 1]), (9, [1, 1, 1])] {
            j.stamp = stamp;
            let p = picked(settle_files(&j, Ok(Some(online_of("https://x/new.jpg", &cover(1100, 1100, rgb))))).unwrap());
            j.current_file = Some(root.join(&p.cover_ref));
            j.current = Some(p.cover_ref);
        }
        assert!(!first.exists(), "two steps back");
        let ninth = managed.join(format!("release-{}-9.jpg", j.id));
        assert!(managed.join(format!("release-{}-8.jpg", j.id)).exists() && ninth.exists());
        j.stamp = 10;
        let p = picked(settle_files(&j, Ok(Some(online_of("https://x/other.jpg", &cover(1200, 1200, [5, 5, 5]))))).unwrap());
        finish(&p, false);
        assert!(!managed.join(format!("release-{}-10.jpg", j.id)).exists());
        assert!(ninth.exists(), "what it shows stays");
    }

    /// A url the lookups found shows over the album's own file of the same
    /// art: the file takes its place, nothing written but the texture. The
    /// album's cover ref names another picture of its folder: the ref
    /// follows the picked `cover.jpg`.
    #[test]
    fn what_shows_is_compared_not_only_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let dir = root.join("A/X");
        std::fs::create_dir_all(&dir).unwrap();
        let art = jpeg(1000, 1000, [20, 200, 20]);
        std::fs::write(dir.join("cover.jpg"), &art).unwrap();
        let mut j = job(root, Dest::Folder(dir.clone()), Some("A/X/cover.jpg"));
        j.dirs = vec![dir.clone()];
        j.shown_url = Some("https://x/single.jpg".into());
        let p = picked(settle_files(&j, Ok(None)).unwrap());
        assert!(p.changed && p.url.is_none() && p.backup.is_none());
        assert_eq!(p.cover_ref, "A/X/cover.jpg");
        assert!(dir.join("ambient.png").is_file(), "the texture is made again");
        let mut png = Vec::new();
        image::RgbImage::from_pixel(300, 300, image::Rgb([9, 9, 9])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        std::fs::write(dir.join("folder.png"), png).unwrap();
        let mut j = job(root, Dest::Folder(dir.clone()), Some("A/X/folder.png"));
        j.dirs = vec![dir.clone()];
        let p = picked(settle_files(&j, Ok(None)).unwrap());
        assert_eq!(p.cover_ref, "A/X/cover.jpg");
        assert!(p.changed);
        let empty = root.join("B/Y");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(matches!(settle_files(&job(root, Dest::Folder(empty), None), Ok(None)).unwrap(), Refreshed::Nothing));
    }

    /// An album without files: the url found, the same one too (its
    /// picture is asked anew); a miss or a failure keeps what it shows.
    #[test]
    fn an_album_without_files_shows_the_url_found() {
        let tmp = tempfile::tempdir().unwrap();
        let web = job(tmp.path(), Dest::Web, Some("https://x/old.jpg"));
        let art = cover(1000, 1000, [9, 9, 9]);
        assert!(matches!(settle_files(&web, Ok(Some(online_of("https://x/new.jpg", &art)))).unwrap(), Refreshed::Web(u) if u == "https://x/new.jpg"));
        assert!(matches!(settle_files(&web, Ok(Some(online_of("https://x/old.jpg", &art)))).unwrap(), Refreshed::Web(u) if u == "https://x/old.jpg"));
        assert!(matches!(settle_files(&web, Ok(None)).unwrap(), Refreshed::Nothing));
        assert!(settle_files(&web, Err("x".into())).is_err());
    }

    /// A write that cannot land leaves the old cover and no staging file;
    /// a texture that cannot be written does not undo the cover.
    #[test]
    fn a_failed_write_keeps_the_old_cover_and_a_failed_texture_keeps_the_new() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("A/X");
        std::fs::create_dir_all(dir.join("cover.jpg")).unwrap();
        assert!(write_atomic(&dir.join("cover.jpg"), b"new").is_err(), "a folder is in the way");
        assert!(dir.join("cover.jpg").is_dir());
        assert!(leftovers(&dir).is_empty());
        std::fs::remove_dir(dir.join("cover.jpg")).unwrap();
        std::fs::write(dir.join("cover.jpg"), jpeg(300, 300, [1, 2, 3])).unwrap();
        std::fs::create_dir_all(dir.join("ambient.png")).unwrap();
        let mut j = job(tmp.path(), Dest::Folder(dir.clone()), Some("A/X/cover.jpg"));
        j.dirs = vec![dir.clone()];
        let album = cover(1000, 1000, [20, 20, 200]);
        let p = picked(settle_files(&j, Ok(Some(online_of("https://x/a.jpg", &album)))).unwrap());
        assert_eq!(std::fs::read(dir.join("cover.jpg")).unwrap(), album.jpeg);
        finish(&p, true);
        assert!(leftovers(&dir).is_empty());
    }

    /// One refresh at a time per folder.
    #[test]
    fn a_folder_is_written_by_one_refresh_at_a_time() {
        let dir = Path::new("/lib/claim-test");
        let first = Claim::take(dir).unwrap();
        assert!(Claim::take(dir).is_none());
        drop(first);
        assert!(Claim::take(dir).is_some());
    }
}
