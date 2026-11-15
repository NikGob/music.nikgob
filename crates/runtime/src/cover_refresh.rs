//! «Обновить обложку» of a library album. Its art is picked again from its
//! own files (each track's picture, its master's, the cover it shows and
//! the cover images of its own folder) and looked up online as
//! [`hd_covers::find`] does: the Cover Art Archive by its ids when
//! MusicBrainz says they are of its kind, the Deezer album linked to it
//! unless that is a single, the album of its title on the services, never
//! a single or an EP for an album of more songs. The album's art found
//! online wins over its files' when it is at least as large: a file may
//! hold the art of the single the album was once taken for. Else the
//! largest picture of its files that is no frame of a video.
//!
//! Where the art goes: the album's folder, when that folder is the album's
//! alone (in the library, not the masters' or the app's, every file of the
//! album in it and nothing of another album): its `cover.jpg`, the one it
//! replaces kept as `cover.prev.jpg` (one step back), and `ambient.png`.
//! Otherwise a file of the album's own in the app's covers folder
//! (`.musiclib/covers/release-<id>-<time>.jpg`, a fresh name each time: the
//! UI keeps pictures by their address), the one before it kept. Every file
//! is written whole or not at all (staged beside it, flushed, renamed over
//! it). An album without files (a service's) shows the url found.
//!
//! A lookup that failed changes nothing. What the album shows is compared
//! with what was picked: the same picture with its palette and texture in
//! place is nothing to do. The decision is kept
//! ([`decision_key`]): the background lookups leave the album's cover alone
//! while it shows that cover, and a lookup of theirs that began before it
//! is not kept.

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

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
/// Without the importer's settings.
const MAX_EDGE: u32 = 1200;
const FALLBACK_SEED: u32 = 0xFF67_50A4;

/// Staging files of this run, counted.
static STAGING: AtomicU64 = AtomicU64::new(0);

/// The albums being refreshed, with the folder each writes into: one
/// refresh per album and per folder.
pub(crate) type Busy = HashMap<ReleaseId, Option<String>>;

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
    /// Its cover is the file `cover_ref` (written now, or there already),
    /// with this palette seed and texture; the url it came from when it
    /// came from online.
    File { cover_ref: String, seed: Option<u32>, ambient: Vec<u8>, url: Option<String> },
    /// The album has no files: it shows this url.
    Web(String),
    /// What it shows is what was picked.
    Same,
    /// Nothing to pick from.
    Nothing,
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
    has_palette: bool,
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

/// The folder that is release `id`'s alone, where its cover may be
/// written: the one folder every local file of it (`local`) is in, else
/// its cover file's; under the library `root`, not the masters' nor the
/// app's; no file of another release (a track or a cover, `places`) in it.
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
            matches!(ext.as_str(), "jpg" | "jpeg" | "png").then(|| (rank, dir.join(p.file_name()?)))
        })
        .collect();
    out.sort();
    out.into_iter().map(|(_, p)| p).collect()
}

/// Every picture of the album's own: those in the audio `files`, the
/// `images`, and the cover images in `dirs`, each once, in that order.
fn own_art(files: &[PathBuf], images: &[PathBuf], dirs: &[PathBuf], max_edge: u32) -> Vec<ProcessedCover> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut take = |bytes: Vec<u8>| {
        let mut h = DefaultHasher::new();
        bytes.hash(&mut h);
        if seen.insert(h.finish())
            && let Ok(c) = process_cover(&bytes, max_edge)
        {
            out.push(c);
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
    out
}

fn side(c: &ProcessedCover) -> u32 {
    c.width.max(c.height)
}

/// The art the album takes: `online` (its own, by the lookup) when it is at
/// least as large as the largest of `local`, else that one; frames of a
/// video never. The first of equals wins.
fn pick(local: Vec<ProcessedCover>, online: Option<(String, ProcessedCover)>) -> Option<(ProcessedCover, Option<String>)> {
    let mut best: Option<ProcessedCover> = None;
    for c in local.into_iter().filter(|c| !cover_frame::is_frame(c.width, c.height)) {
        if best.as_ref().is_none_or(|b| side(&c) > side(b)) {
            best = Some(c);
        }
    }
    match online.filter(|(_, c)| !cover_frame::is_frame(c.width, c.height)) {
        Some((url, c)) if best.as_ref().is_none_or(|b| side(&c) >= side(b)) => Some((c, Some(url))),
        _ => best.map(|c| (c, None)),
    }
}

/// A name beside `target` that no other write takes.
fn staging(target: &Path) -> PathBuf {
    let n = STAGING.fetch_add(1, Ordering::Relaxed);
    let name = target.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    target.with_file_name(format!(".{name}.{}-{}-{n}.tmp", std::process::id(), now_ms()))
}

/// Renames the staged `tmp` over `target`, flushed first; on a failure
/// `tmp` goes and `target` stays as it was.
fn swap_in(tmp: &Path, target: &Path, fill: impl FnOnce(&Path) -> std::io::Result<()>) -> std::io::Result<()> {
    let done = fill(tmp)
        .and_then(|()| std::fs::OpenOptions::new().write(true).open(long_path(tmp))?.sync_all())
        .and_then(|()| std::fs::rename(long_path(tmp), long_path(target)));
    if done.is_err() {
        let _ = std::fs::remove_file(long_path(tmp));
    }
    done
}

/// Writes `bytes` as `target`, whole or not at all.
fn write_atomic(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    swap_in(&staging(target), target, |tmp| {
        let mut f = std::fs::File::create(long_path(tmp))?;
        f.write_all(bytes)
    })
}

/// Copies `target` to `backup`, whole or not at all.
fn back_up(target: &Path, backup: &Path) -> std::io::Result<()> {
    if !long_path(target).is_file() {
        return Ok(());
    }
    swap_in(&staging(backup), backup, |tmp| std::fs::copy(long_path(target), long_path(tmp)).map(|_| ()))
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
fn settle_files(job: &Job, online: Result<Option<(String, ProcessedCover)>, String>) -> Result<Refreshed, String> {
    let online = online?;
    let managed = match &job.dest {
        Dest::Web => {
            return Ok(match online {
                Some((url, _)) if job.shown_url.as_deref() == Some(url.as_str()) => Refreshed::Same,
                Some((url, _)) => Refreshed::Web(url),
                None => Refreshed::Nothing,
            });
        }
        Dest::Folder(_) => None,
        Dest::Managed(m) => Some(m),
    };
    let images: Vec<PathBuf> = job.current_file.iter().cloned().collect();
    let Some((cover, url)) = pick(own_art(&job.files, &images, &job.dirs, job.max_edge), online) else {
        return Ok(Refreshed::Nothing);
    };
    let read = |p: &Path| std::fs::read(long_path(p)).ok();
    let holds = |p: &Path| read(p).as_deref() == Some(&cover.jpeg[..]);
    // What it shows already, with its palette and texture.
    if let Some(f) = job.current_file.as_deref()
        && job.shown_url.is_none()
        && job.has_palette
        && holds(f)
        && [f.with_extension("ambient.png"), f.with_file_name(AMBIENT_FILE)].iter().any(|a| long_path(a).is_file())
    {
        return Ok(Refreshed::Same);
    }
    let own_managed = |f: &&PathBuf| {
        managed.is_some_and(|m| f.parent().is_some_and(|p| norm(p) == norm(m)))
            && f.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&format!("release-{}-", job.id)))
    };
    let (target, fresh) = match (&job.dest, managed) {
        (Dest::Folder(d), _) => (d.join(COVER_FILE), false),
        (_, Some(m)) => match job.current_file.as_ref().filter(own_managed).filter(|f| holds(f)) {
            Some(f) => (f.clone(), false),
            None => (m.join(format!("release-{}-{}.jpg", job.id, job.stamp)), true),
        },
        (_, None) => return Ok(Refreshed::Nothing),
    };
    let ambient_path = ambient_of(&target, &job.dest);
    let same_bytes = !fresh && holds(&target);
    let ambient_there = long_path(&ambient_path).is_file();
    let visuals = derive_visuals(&cover.jpeg).map_err(|e| e.to_string())?;
    if !same_bytes {
        match &job.dest {
            Dest::Folder(d) => back_up(&target, &d.join(PREV_FILE)).map_err(|e| format!("{}: {e}", target.display()))?,
            Dest::Managed(m) => std::fs::create_dir_all(long_path(m)).map_err(|e| format!("{}: {e}", m.display()))?,
            Dest::Web => {}
        }
        write_atomic(&target, &cover.jpeg).map_err(|e| format!("{}: {e}", target.display()))?;
    }
    // The cover is in place: a texture that cannot be written is only a
    // missing glow.
    if (!same_bytes || !ambient_there)
        && let Err(e) = write_atomic(&ambient_path, &visuals.ambient_png)
    {
        log::warn!("cover refresh: {}: {e}", ambient_path.display());
    }
    if fresh && let Some(m) = managed {
        let mut keep = vec![target.as_path()];
        keep.extend(job.current_file.as_deref());
        prune_managed(m, job.id, &keep);
    }
    Ok(Refreshed::File { cover_ref: cover_ref_of(&target, &job.root), seed: visuals.seed_argb, ambient: visuals.ambient_png, url })
}

/// The album's art online, by [`hd_covers::find`], downloaded. Err: a
/// service could not answer.
async fn online(meta: &Metadata, services: &[Arc<dyn StreamService>], w: &hd_covers::Want, max_edge: u32) -> Result<Option<(String, ProcessedCover)>, String> {
    let Some(url) = hd_covers::find(Some(meta), services, w).await? else { return Ok(None) };
    let bytes = match meta.cover_art.download(&url).await {
        Ok(b) => b,
        Err(e) if better_covers::meta_trouble(&e) => return Err(format!("{url}: {e}")),
        Err(e) => {
            log::info!("cover refresh of «{}»: {url}: {e}", w.title);
            return Ok(None);
        }
    };
    let cover = tokio::task::spawn_blocking(move || process_cover(&bytes, max_edge)).await.map_err(|e| e.to_string())?;
    Ok(cover.ok().map(|c| (url, c)))
}

impl Worker {
    /// «Обновить обложку» of a library album (see the module docs): looked
    /// up off the loop, [`Self::release_cover_refreshed`] takes the answer.
    pub(crate) fn refresh_release_cover(&mut self, id: ReleaseId) {
        let Some(r) = self.lib.get_release(id).ok().flatten() else { return };
        if self.cover_refresh.contains_key(&id) {
            return;
        }
        let tracks = self.lib.tracks_of_release(id).unwrap_or_default();
        let mut masters: Vec<PathBuf> = Vec::new();
        let mut local: Vec<PathBuf> = Vec::new();
        for t in &tracks {
            if let Some(path) = self.lib.get_master(t.id).ok().flatten().and_then(|m| m.file_path) {
                masters.push(PathBuf::from(path));
            }
            if let Some(s) = self.lib.active_source(t.id).ok().flatten().filter(|s| s.kind == SourceKind::LocalFile) {
                local.push(self.res.path(&s.locator));
            }
        }
        let root = self.res.root().to_owned();
        let current_file = r.cover_ref.as_deref().filter(|c| !is_web_ref(c)).map(|c| self.res.path(c));
        // Unknown who else is in a folder: none is the album's alone.
        let owned = match self.lib.release_file_places() {
            Ok(places) => {
                let places: Vec<(ReleaseId, PathBuf)> = places.into_iter().map(|(r, p)| (r, self.res.path(&p))).collect();
                let masters_dir = self.import.as_ref().map(|s| s.config().masters_dir());
                owned_folder(id, &local, current_file.as_deref(), &root, masters_dir.as_deref(), &places)
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
            None => Dest::Managed(self.res.path(MANAGED_DIR)),
        };
        let folder = match &dest {
            Dest::Folder(d) => Some(norm(d)),
            Dest::Managed(_) | Dest::Web => None,
        };
        if folder.is_some() && self.cover_refresh.values().any(|f| *f == folder) {
            return;
        }
        self.cover_refresh.insert(id, folder);
        let dirs = match &dest {
            Dest::Folder(d) => vec![d.clone()],
            Dest::Managed(_) | Dest::Web => Vec::new(),
        };
        let cfg = self.import.as_ref().map(|s| s.config());
        let max_edge = cfg.map_or(MAX_EDGE, |c| c.cover_max_edge);
        let meta = self.import.as_ref().filter(|s| s.config().fetch_cover_online).and_then(|s| s.metadata());
        let services: Vec<Arc<dyn StreamService>> =
            [Service::Deezer, Service::Youtube, Service::Spotify].into_iter().filter_map(|k| self.service(k)).collect();
        let shown_url = better_covers::cached(&self.lib, better_covers::Target::Release(id), r.cover_ref.as_deref())
            .or_else(|| r.cover_ref.clone().filter(|c| is_web_ref(c)));
        masters.extend(local);
        let job = Job {
            id,
            dest,
            root,
            current: r.cover_ref.clone(),
            current_file,
            shown_url,
            has_palette: self.lib.get_palette(id).ok().flatten().is_some(),
            files: masters,
            dirs,
            want: hd_covers::want_of(&self.lib, &r, vec![r.artist_credit.display()]),
            max_edge,
            stamp: now_ms(),
        };
        log::info!("cover refresh of «{}»: into {:?}, {} files, online {}", r.title, job.dest, job.files.len(), meta.is_some());
        self.streaming.spawn_internal(async move {
            let found = match &meta {
                Some(m) => match tokio::time::timeout(ONLINE_TIMEOUT, online(m, &services, &job.want, job.max_edge)).await {
                    Ok(r) => r,
                    Err(_) => Err("the online lookup took too long".to_owned()),
                },
                None => Ok(None),
            };
            let result = tokio::task::spawn_blocking(move || settle_files(&job, found))
                .await
                .unwrap_or_else(|e| Err(e.to_string()));
            Internal::CoverRefreshed(id, result)
        });
    }

    /// A refresh came back: the album's cover, its palette, what the
    /// lookups keep for it and the decision; everything on screen takes
    /// it. Nothing to change, or a failure: what it shows stays, with a
    /// toast.
    pub(crate) fn release_cover_refreshed(&mut self, id: ReleaseId, result: Result<Refreshed, String>) {
        self.cover_refresh.remove(&id);
        let Some(mut r) = self.lib.get_release(id).ok().flatten() else { return };
        let decide = |lib: &Library, cover: Option<&str>| {
            let _ = lib.cache_put(&decision_key(id), now_ms(), cover.unwrap_or(""));
        };
        match result {
            Ok(Refreshed::File { cover_ref, seed, ambient, url }) => {
                log::info!("cover refresh of «{}»: {cover_ref} from {}", r.title, url.as_deref().unwrap_or("its files"));
                if r.cover_ref.as_deref() != Some(cover_ref.as_str()) {
                    r.cover_ref = Some(cover_ref);
                    if let Err(e) = self.lib.upsert_release(&r) {
                        log::warn!("cover refresh of «{}»: {e}", r.title);
                        self.toast("Не получилось обновить обложку", None);
                        return;
                    }
                }
                let fallback = self.import.as_ref().map_or(FALLBACK_SEED, |s| s.config().fallback_seed_argb);
                let mut palette = Palette::new(id, seed.unwrap_or(fallback));
                palette.ambient_blur = Some(ambient);
                if let Err(e) = self.lib.put_palette(&palette) {
                    log::warn!("cover refresh of «{}»: the palette: {e}", r.title);
                }
                better_covers::settle(&self.lib, id, r.cover_ref.as_deref(), "");
                hd_covers::settle(&self.lib, id, url.as_deref());
                decide(&self.lib, r.cover_ref.as_deref());
                self.reload_after_import();
            }
            Ok(Refreshed::Web(url)) => {
                log::info!("cover refresh of «{}»: {url}", r.title);
                better_covers::settle(&self.lib, id, r.cover_ref.as_deref(), &url);
                hd_covers::settle(&self.lib, id, Some(&url));
                decide(&self.lib, r.cover_ref.as_deref());
                self.reload_after_import();
            }
            Ok(Refreshed::Same) => {
                log::info!("cover refresh of «{}»: it shows the best there is", r.title);
                decide(&self.lib, r.cover_ref.as_deref());
                self.toast("Лучшей обложки не нашлось", None);
            }
            Ok(Refreshed::Nothing) => {
                log::info!("cover refresh of «{}»: nothing to pick from", r.title);
                self.toast("Лучшей обложки не нашлось", None);
            }
            Err(e) => {
                log::warn!("cover refresh of «{}»: {e}", r.title);
                self.toast("Не получилось обновить обложку", None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
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

    /// The single's art in the folder is as large as the album's found
    /// online: the album's wins. A larger picture of the files wins over a
    /// smaller one online; a frame of a video never.
    #[test]
    fn the_album_art_found_online_wins_when_it_is_as_large() {
        let single = cover(1000, 1000, [200, 20, 20]);
        let album = cover(1000, 1000, [20, 20, 200]);
        let (got, url) = pick(vec![single.clone()], Some(("https://x/album.jpg".into(), album.clone()))).unwrap();
        assert_eq!((got, url.as_deref()), (album.clone(), Some("https://x/album.jpg")));
        let scan = cover(1200, 1200, [20, 200, 20]);
        assert_eq!(pick(vec![single.clone(), scan.clone()], Some(("u".into(), album.clone()))).map(|(c, _)| c), Some(scan));
        let frame = cover(1280, 720, [0, 0, 0]);
        assert_eq!(pick(vec![frame.clone(), single.clone()], None), Some((single.clone(), None)));
        assert_eq!(pick(vec![single.clone()], Some(("u".into(), frame.clone()))).map(|(c, _)| c), Some(single));
        assert!(pick(vec![frame], None).is_none());
        assert!(pick(Vec::new(), None).is_none());
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
        // Another album's track or cover in it.
        let mut shared = places.clone();
        shared.push((other, f("/lib/A/2024 - X/03.opus")));
        assert_eq!(owned_folder(me, &mine, None, root, None, &shared), None);
        let mut covered = places.clone();
        covered.push((other, f("/lib/A/2024 - X/cover.jpg")));
        assert_eq!(owned_folder(me, &mine, None, root, None, &covered), None);
        // Its files in two folders.
        let split = vec![f("/lib/A/2024 - X/01.opus"), f("/lib/A/2024 - X (CD2)/01.opus")];
        assert_eq!(owned_folder(me, &split, None, root, None, &places), None);
        // The masters', the app's, outside the library.
        let masters = vec![f("/lib/.musiclib/masters/A/X/01.flac")];
        assert_eq!(owned_folder(me, &masters, None, root, None, &[]), None);
        assert_eq!(owned_folder(me, &[f("/m/A/X/01.flac")], None, root, Some(Path::new("/m")), &[]), None);
        assert_eq!(owned_folder(me, &[f("/elsewhere/X/01.opus")], None, root, None, &[]), None);
        assert_eq!(owned_folder(me, &[f("/lib/01.opus")], None, root, None, &[]), None, "the library's root");
        // No files: its cover's folder.
        assert_eq!(owned_folder(me, &[], Some(Path::new("/lib/A/X/cover.jpg")), root, None, &[]), Some(f("/lib/A/X")));
        // Its cover names a folder that is gone: its files' folder still is.
        assert_eq!(owned_folder(me, &mine, Some(Path::new("/lib/Gone/cover.jpg")), root, None, &places), Some(f("/lib/A/2024 - X")));
    }

    fn want() -> hd_covers::Want {
        hd_covers::Want {
            id: ReleaseId::new(),
            mbid: None,
            group_mbid: None,
            deezer: None,
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
            has_palette: true,
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
    /// `cover.jpg`, the single's kept as `cover.prev.jpg`, the texture
    /// written; asked again, it shows the best there is.
    #[test]
    fn a_refresh_writes_the_cover_of_its_own_folder_and_keeps_the_one_it_replaced() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let dir = root.join("A/2024 - X");
        std::fs::create_dir_all(&dir).unwrap();
        let single = jpeg(1000, 1000, [200, 20, 20]);
        std::fs::write(dir.join("cover.jpg"), &single).unwrap();
        let album = cover(1000, 1000, [20, 20, 200]);
        let mut j = job(root, Dest::Folder(dir.clone()), Some("A/2024 - X/cover.jpg"));
        j.dirs = vec![dir.clone()];
        let got = settle_files(&j, Ok(Some(("https://x/album.jpg".into(), album.clone())))).unwrap();
        assert!(
            matches!(&got, Refreshed::File { cover_ref, url: Some(u), .. } if cover_ref == "A/2024 - X/cover.jpg" && u == "https://x/album.jpg"),
            "{got:?}"
        );
        assert_eq!(std::fs::read(dir.join("cover.jpg")).unwrap(), album.jpeg);
        assert_eq!(std::fs::read(dir.join("cover.prev.jpg")).unwrap(), single);
        assert!(dir.join("ambient.png").is_file());
        assert!(leftovers(&dir).is_empty());
        assert!(matches!(settle_files(&j, Ok(Some(("https://x/album.jpg".into(), album)))).unwrap(), Refreshed::Same));
        // Offline: what it shows is the best there is.
        assert!(matches!(settle_files(&j, Ok(None)).unwrap(), Refreshed::Same));
        // A lookup that failed changes nothing.
        assert!(settle_files(&j, Err("Deezer: network".into())).is_err());
    }

    /// A folder shared with another album is not touched: the art becomes
    /// a file of the album's own in the covers folder, a fresh name each
    /// time, the one before kept and older ones gone.
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
        let got = settle_files(&j, Ok(Some(("https://x/album.jpg".into(), album.clone())))).unwrap();
        let want_ref = format!(".musiclib/covers/release-{}-7.jpg", j.id);
        assert!(matches!(&got, Refreshed::File { cover_ref, .. } if *cover_ref == want_ref), "{got:?}");
        assert_eq!(std::fs::read(shared.join("cover.jpg")).unwrap(), theirs, "the shared folder is untouched");
        assert!(!shared.join("cover.prev.jpg").exists() && !shared.join("ambient.png").exists());
        let first = managed.join(format!("release-{}-7.jpg", j.id));
        assert_eq!(std::fs::read(&first).unwrap(), album.jpeg);
        assert!(first.with_extension("ambient.png").is_file());
        // It shows that file now: asked again, nothing to do.
        j.current = Some(want_ref.clone());
        j.current_file = Some(first.clone());
        assert!(matches!(settle_files(&j, Ok(Some(("https://x/album.jpg".into(), album)))).unwrap(), Refreshed::Same));
        // Other art twice: the newest and the one before stay.
        for (stamp, rgb) in [(8, [1, 200, 1]), (9, [1, 1, 1])] {
            j.stamp = stamp;
            let art = cover(1100, 1100, rgb);
            let got = settle_files(&j, Ok(Some(("https://x/new.jpg".into(), art)))).unwrap();
            let Refreshed::File { cover_ref, .. } = got else { panic!("{got:?}") };
            j.current_file = Some(root.join(&cover_ref));
            j.current = Some(cover_ref);
        }
        assert!(!first.exists(), "two steps back");
        assert!(managed.join(format!("release-{}-8.jpg", j.id)).exists());
        assert!(managed.join(format!("release-{}-9.jpg", j.id)).exists());
    }

    /// A url the lookups found shows over the album's own file of the same
    /// art: the file and its palette take its place, nothing written. The
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
        std::fs::write(dir.join("ambient.png"), b"x").unwrap();
        let mut j = job(root, Dest::Folder(dir.clone()), Some("A/X/cover.jpg"));
        j.dirs = vec![dir.clone()];
        j.shown_url = Some("https://x/single.jpg".into());
        let got = settle_files(&j, Ok(None)).unwrap();
        assert!(matches!(&got, Refreshed::File { cover_ref, url: None, .. } if cover_ref == "A/X/cover.jpg"), "{got:?}");
        assert!(!dir.join("cover.prev.jpg").exists(), "nothing written");
        std::fs::write(dir.join("folder.png"), {
            let mut out = Vec::new();
            image::RgbImage::from_pixel(300, 300, image::Rgb([9, 9, 9])).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
            out
        })
        .unwrap();
        let mut j = job(root, Dest::Folder(dir.clone()), Some("A/X/folder.png"));
        j.dirs = vec![dir.clone()];
        let got = settle_files(&j, Ok(None)).unwrap();
        assert!(matches!(&got, Refreshed::File { cover_ref, .. } if cover_ref == "A/X/cover.jpg"), "{got:?}");
        // Without palette the same picture is no Same either.
        let mut j = job(root, Dest::Folder(dir.clone()), Some("A/X/cover.jpg"));
        j.dirs = vec![dir.clone()];
        j.has_palette = false;
        assert!(matches!(settle_files(&j, Ok(None)).unwrap(), Refreshed::File { .. }));
        // Nothing at all to pick from.
        let empty = root.join("B/Y");
        std::fs::create_dir_all(&empty).unwrap();
        let j = job(root, Dest::Folder(empty), None);
        assert!(matches!(settle_files(&j, Ok(None)).unwrap(), Refreshed::Nothing));
    }

    /// An album without files: the url found, unless it shows it; a miss
    /// or a failure keeps what it shows.
    #[test]
    fn an_album_without_files_shows_the_url_found() {
        let tmp = tempfile::tempdir().unwrap();
        let mut web = job(tmp.path(), Dest::Web, Some("https://x/old.jpg"));
        let art = cover(1000, 1000, [9, 9, 9]);
        assert!(matches!(settle_files(&web, Ok(Some(("https://x/new.jpg".into(), art.clone())))).unwrap(), Refreshed::Web(u) if u == "https://x/new.jpg"));
        assert!(matches!(settle_files(&web, Ok(Some(("https://x/old.jpg".into(), art.clone())))).unwrap(), Refreshed::Same));
        assert!(matches!(settle_files(&web, Ok(None)).unwrap(), Refreshed::Nothing));
        assert!(settle_files(&web, Err("x".into())).is_err());
        // It shows the lookups' url B over its own A: B found is the same.
        web.shown_url = Some("https://x/b.jpg".into());
        assert!(matches!(settle_files(&web, Ok(Some(("https://x/b.jpg".into(), art)))).unwrap(), Refreshed::Same));
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
        let got = settle_files(&j, Ok(Some(("https://x/a.jpg".into(), album.clone())))).unwrap();
        assert!(matches!(got, Refreshed::File { .. }), "{got:?}");
        assert_eq!(std::fs::read(dir.join("cover.jpg")).unwrap(), album.jpeg);
        assert!(leftovers(&dir).is_empty());
    }
}
