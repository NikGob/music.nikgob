//! «Обновить обложку» of a library album. Its art is picked again from its
//! own files (each track's picture, its master's, and the cover images
//! beside them) and looked up online as [`hd_covers::find`] does: the Cover
//! Art Archive by its ids, the Deezer album linked to it unless that is a
//! single, the album of its title on the services, never a single or an EP
//! for an album of more songs. The album's art found online wins over its
//! files' when it is at least as large: a file may hold the art of the
//! single the album was once taken for. Else the largest picture of its
//! files that is no frame of a video.
//!
//! The art becomes the album folder's `cover.jpg`, the one it replaces kept
//! beside it as `cover.prev.jpg` (one step back, the next refresh writes
//! over it), with its ambient texture and palette. An album without a
//! folder (a service's) shows the url found. What the background lookups
//! found for the album goes, and its cover counts as asked, so they do not
//! bring the old art back.

use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use nikgob_core::cover_frame;
use nikgob_core::model::{Palette, ReleaseId, SourceKind};
use nikgob_core::streaming::{Service, StreamService};
use nikgob_import::cover::{ProcessedCover, derive_visuals, process_cover};
use nikgob_import::sanitize::long_path;
use nikgob_library::is_web_ref;
use nikgob_metadata::Metadata;

use crate::Worker;
use crate::better_covers;
use crate::hd_covers;
use crate::import::Internal;

const COVER_FILE: &str = "cover.jpg";
/// The cover a refresh replaced.
const PREV_FILE: &str = "cover.prev.jpg";
const AMBIENT_FILE: &str = "ambient.png";
/// Names of the cover images in a folder (as the importer reads them).
const COVER_NAMES: &[&str] = &["cover", "folder", "front", "album", "albumart"];
/// The online lookup and download, at most.
const ONLINE_TIMEOUT: Duration = Duration::from_secs(90);
/// Without the importer's settings.
const MAX_EDGE: u32 = 1200;
const FALLBACK_SEED: u32 = 0xFF67_50A4;

/// The albums being refreshed.
pub(crate) type Busy = HashSet<ReleaseId>;

/// What a refresh of an album came to.
#[derive(Debug)]
pub(crate) enum Refreshed {
    /// A new `cover.jpg` in the album folder (`cover_ref`, its ref), and
    /// the url it came from when it came from online.
    File { cover_ref: String, seed: Option<u32>, ambient: Vec<u8>, url: Option<String> },
    /// The album has no folder: it shows this url.
    Web(String),
    /// Nothing better than what it shows.
    Same,
}

/// What the refresh of one album works with.
struct Job {
    /// Its folder, where `cover.jpg` goes, and the ref of that file.
    dir: Option<(PathBuf, String)>,
    /// Its cover when that is a url.
    web: Option<String>,
    /// Files whose pictures count, the masters first.
    files: Vec<PathBuf>,
    /// Folders whose cover images count, the masters' first.
    dirs: Vec<PathBuf>,
    want: hd_covers::Want,
    max_edge: u32,
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

/// Every picture of the album's own: those in `files` and the cover images
/// in `dirs`, each once, in that order.
fn own_art(files: &[PathBuf], dirs: &[PathBuf], max_edge: u32) -> Vec<ProcessedCover> {
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
    for d in dirs {
        for p in cover_images(d) {
            if let Ok(b) = std::fs::read(long_path(&p)) {
                take(b);
            }
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

/// Writes `cover` as `dir`'s `cover.jpg` (the old one copied to
/// `cover.prev.jpg` first) and its ambient texture; its palette seed.
fn write(dir: &Path, cover: &ProcessedCover) -> Result<(Option<u32>, Vec<u8>), String> {
    let visuals = derive_visuals(&cover.jpeg).map_err(|e| e.to_string())?;
    let target = dir.join(COVER_FILE);
    if long_path(&target).is_file() {
        std::fs::copy(long_path(&target), long_path(&dir.join(PREV_FILE))).map_err(|e| format!("{}: {e}", target.display()))?;
    }
    let tmp = dir.join(".cover.nikgob-refresh.jpg");
    std::fs::write(long_path(&tmp), &cover.jpeg).map_err(|e| format!("{}: {e}", tmp.display()))?;
    nikgob_import::fsutil::move_file(&tmp, &target).map_err(|e| e.to_string())?;
    let ambient = dir.join(AMBIENT_FILE);
    std::fs::write(long_path(&ambient), &visuals.ambient_png).map_err(|e| format!("{}: {e}", ambient.display()))?;
    Ok((visuals.seed_argb, visuals.ambient_png))
}

/// The files' part of a refresh, with what was found `online`.
fn settle_files(job: &Job, online: Option<(String, ProcessedCover)>) -> Result<Refreshed, String> {
    let Some((dir, cover_ref)) = &job.dir else {
        return Ok(match online {
            Some((url, _)) if job.web.as_deref() != Some(url.as_str()) => Refreshed::Web(url),
            _ => Refreshed::Same,
        });
    };
    let Some((cover, url)) = pick(own_art(&job.files, &job.dirs, job.max_edge), online) else {
        return Ok(Refreshed::Same);
    };
    if std::fs::read(long_path(&dir.join(COVER_FILE))).is_ok_and(|b| b == cover.jpeg) {
        return Ok(Refreshed::Same);
    }
    let (seed, ambient) = write(dir, &cover)?;
    Ok(Refreshed::File { cover_ref: cover_ref.clone(), seed, ambient, url })
}

/// The album's art online, by [`hd_covers::find`], downloaded.
async fn online(meta: &Metadata, services: &[Arc<dyn StreamService>], w: &hd_covers::Want, max_edge: u32) -> Option<(String, ProcessedCover)> {
    let url = match hd_covers::find(Some(meta), services, w).await {
        Ok(Some(url)) => url,
        Ok(None) => return None,
        Err(e) => {
            log::info!("cover refresh of «{}»: {e}", w.title);
            return None;
        }
    };
    let bytes = match meta.cover_art.download(&url).await {
        Ok(b) => b,
        Err(e) => {
            log::info!("cover refresh of «{}»: {url}: {e}", w.title);
            return None;
        }
    };
    let cover = tokio::task::spawn_blocking(move || process_cover(&bytes, max_edge)).await.ok()?.ok()?;
    Some((url, cover))
}

impl Worker {
    /// «Обновить обложку» of a library album (see the module docs): looked
    /// up off the loop, [`Self::release_cover_refreshed`] takes the answer.
    pub(crate) fn refresh_release_cover(&mut self, id: ReleaseId) {
        let Some(r) = self.lib.get_release(id).ok().flatten() else { return };
        if !self.cover_refresh.insert(id) {
            return;
        }
        let tracks = self.lib.tracks_of_release(id).unwrap_or_default();
        let mut files: Vec<PathBuf> = Vec::new();
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut local: Vec<PathBuf> = Vec::new();
        for t in &tracks {
            if let Some(path) = self.lib.get_master(t.id).ok().flatten().and_then(|m| m.file_path) {
                files.push(PathBuf::from(path));
            }
            if let Some(s) = self.lib.active_source(t.id).ok().flatten().filter(|s| s.kind == SourceKind::LocalFile) {
                local.push(self.res.path(&s.locator));
            }
        }
        // The masters' folders, then the album's own.
        for f in &files {
            if let Some(d) = f.parent()
                && !dirs.iter().any(|x| x == d)
            {
                dirs.push(d.to_owned());
            }
        }
        let file_ref = r.cover_ref.as_deref().filter(|c| !is_web_ref(c));
        let dir = file_ref
            .map(|c| self.res.path(c))
            .or_else(|| local.first().cloned())
            .and_then(|p| p.parent().map(Path::to_owned))
            .filter(|d| long_path(d).is_dir());
        for d in dir.iter().cloned().chain(local.iter().filter_map(|p| p.parent().map(Path::to_owned))) {
            if !dirs.contains(&d) {
                dirs.push(d);
            }
        }
        files.extend(local);
        let root = self.res.root().to_owned();
        let dir = dir.map(|d| {
            let file = d.join(COVER_FILE);
            let cover_ref = file.strip_prefix(&root).unwrap_or(&file).to_string_lossy().replace('\\', "/");
            (d, cover_ref)
        });
        let cfg = self.import.as_ref().map(|s| s.config());
        let max_edge = cfg.map_or(MAX_EDGE, |c| c.cover_max_edge);
        let meta = self.import.as_ref().filter(|s| s.config().fetch_cover_online).and_then(|s| s.metadata());
        let services: Vec<Arc<dyn StreamService>> =
            [Service::Deezer, Service::Youtube, Service::Spotify].into_iter().filter_map(|k| self.service(k)).collect();
        let job = Job {
            dir,
            web: r.cover_ref.clone().filter(|c| is_web_ref(c)),
            files,
            dirs,
            want: hd_covers::want_of(&self.lib, &r, vec![r.artist_credit.display()]),
            max_edge,
        };
        log::info!("cover refresh of «{}»: {} files, {} folders, online {}", r.title, job.files.len(), job.dirs.len(), meta.is_some());
        self.streaming.spawn_internal(async move {
            let found = match &meta {
                Some(m) => tokio::time::timeout(ONLINE_TIMEOUT, online(m, &services, &job.want, job.max_edge)).await.ok().flatten(),
                None => None,
            };
            let result = tokio::task::spawn_blocking(move || settle_files(&job, found))
                .await
                .unwrap_or_else(|e| Err(e.to_string()));
            Internal::CoverRefreshed(id, result)
        });
    }

    /// A refresh came back: the album's new cover, its palette and what the
    /// lookups keep for it; everything on screen takes it. Nothing better:
    /// a toast.
    pub(crate) fn release_cover_refreshed(&mut self, id: ReleaseId, result: Result<Refreshed, String>) {
        self.cover_refresh.remove(&id);
        let Some(mut r) = self.lib.get_release(id).ok().flatten() else { return };
        // A url the lookups found was showing for it.
        let shown = better_covers::cached(&self.lib, better_covers::Target::Release(id), r.cover_ref.as_deref()).is_some();
        let changed = match result {
            Ok(Refreshed::File { cover_ref, seed, ambient, url }) => {
                log::info!("cover refresh of «{}»: {cover_ref} from {}", r.title, url.as_deref().unwrap_or("its files"));
                if r.cover_ref.as_deref() != Some(cover_ref.as_str()) {
                    r.cover_ref = Some(cover_ref);
                    if let Err(e) = self.lib.upsert_release(&r) {
                        log::warn!("cover refresh of «{}»: {e}", r.title);
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
                true
            }
            Ok(Refreshed::Web(url)) => {
                log::info!("cover refresh of «{}»: {url}", r.title);
                better_covers::settle(&self.lib, id, r.cover_ref.as_deref(), &url);
                hd_covers::settle(&self.lib, id, Some(&url));
                true
            }
            Ok(Refreshed::Same) => {
                log::info!("cover refresh of «{}»: nothing better", r.title);
                better_covers::settle(&self.lib, id, r.cover_ref.as_deref(), "");
                hd_covers::settle(&self.lib, id, None);
                shown
            }
            Err(e) => {
                log::warn!("cover refresh of «{}»: {e}", r.title);
                false
            }
        };
        if changed {
            self.reload_after_import();
        } else {
            self.toast("Лучшей обложки не нашлось", None);
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

    fn job(dir: &Path, files: Vec<PathBuf>) -> Job {
        Job {
            dir: Some((dir.to_owned(), "A/2024 - X/cover.jpg".into())),
            web: None,
            files,
            dirs: vec![dir.to_owned()],
            want: hd_covers::Want {
                id: ReleaseId::new(),
                mbid: None,
                group_mbid: None,
                deezer: None,
                title: "X".into(),
                artists: Vec::new(),
                release_type: nikgob_core::model::ReleaseType::Album,
                tracks: 9,
            },
            max_edge: MAX_EDGE,
        }
    }

    /// The refresh writes the album's art over the single's `cover.jpg`,
    /// keeps the single's as `cover.prev.jpg` and writes the ambient
    /// texture; asked again, nothing changes.
    #[test]
    fn a_refresh_writes_the_cover_and_keeps_the_one_it_replaced() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let single = jpeg(1000, 1000, [200, 20, 20]);
        std::fs::write(dir.join("cover.jpg"), &single).unwrap();
        let album = cover(1000, 1000, [20, 20, 200]);
        let got = settle_files(&job(dir, Vec::new()), Some(("https://x/album.jpg".into(), album.clone()))).unwrap();
        assert!(
            matches!(&got, Refreshed::File { cover_ref, url: Some(u), .. } if cover_ref == "A/2024 - X/cover.jpg" && u == "https://x/album.jpg"),
            "{got:?}"
        );
        assert_eq!(std::fs::read(dir.join("cover.jpg")).unwrap(), album.jpeg);
        assert_eq!(std::fs::read(dir.join("cover.prev.jpg")).unwrap(), single);
        assert!(dir.join("ambient.png").is_file());
        assert!(matches!(settle_files(&job(dir, Vec::new()), Some(("https://x/album.jpg".into(), album))).unwrap(), Refreshed::Same));
        // Offline: the folder's own cover is the best there is.
        assert!(matches!(settle_files(&job(dir, Vec::new()), None).unwrap(), Refreshed::Same));
    }

    /// Without anything online, a larger cover image beside the files takes
    /// the place of a small `cover.jpg`; `cover.prev.jpg` itself is no
    /// cover image.
    #[test]
    fn a_refresh_takes_the_largest_picture_of_the_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("cover.jpg"), jpeg(300, 300, [200, 20, 20])).unwrap();
        std::fs::write(dir.join("cover.prev.jpg"), jpeg(1200, 1200, [1, 2, 3])).unwrap();
        let scan = jpeg(900, 900, [20, 200, 20]);
        std::fs::write(dir.join("Folder.JPG"), &scan).unwrap();
        assert_eq!(cover_images(dir).len(), 2);
        let got = settle_files(&job(dir, Vec::new()), None).unwrap();
        assert!(matches!(got, Refreshed::File { url: None, .. }), "{got:?}");
        assert_eq!(std::fs::read(dir.join("cover.jpg")).unwrap(), scan);
        // An album with no folder shows the url found, unless it shows it.
        let mut web = job(dir, Vec::new());
        web.dir = None;
        web.web = Some("https://x/old.jpg".into());
        let art = cover(1000, 1000, [9, 9, 9]);
        assert!(matches!(settle_files(&web, Some(("https://x/new.jpg".into(), art.clone()))).unwrap(), Refreshed::Web(u) if u == "https://x/new.jpg"));
        assert!(matches!(settle_files(&web, Some(("https://x/old.jpg".into(), art))).unwrap(), Refreshed::Same));
        assert!(matches!(settle_files(&web, None).unwrap(), Refreshed::Same));
    }
}
