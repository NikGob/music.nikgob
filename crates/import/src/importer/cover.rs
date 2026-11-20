use super::*;

const COVER_NAMES: &[&str] = &["cover", "folder", "front", "album", "albumart"];
pub(super) const COVER_FILE: &str = "cover.jpg";
const AMBIENT_FILE: &str = "ambient.png";

impl Importer {
    /// Embedded cover, a picture next to the file, the album folder's cover, then
    /// the Cover Art Archive / Deezer. `None` is fine.
    ///
    /// A picture that is a frame of a video (a file saved from YouTube
    /// embeds the clip's thumbnail) gives way to the album art online and to
    /// the art the target track's album already shows (the stream's); only
    /// when there is none it is kept, without its black bars.
    pub(super) async fn prepare_cover(
        &self,
        a: &Analyzed,
        id: &Identity,
        album_dir: &Path,
    ) -> Option<ProcessedCover> {
        let max = self.inner.cfg.cover_max_edge;
        let (embedded, src_dir, album_dir) = (
            a.tags.cover.clone(),
            a.master_path().parent().map(Path::to_path_buf),
            album_dir.to_path_buf(),
        );
        let local = blocking(move || {
            let mut sources: Vec<Vec<u8>> = Vec::new();
            sources.extend(embedded);
            if let Some(dir) = src_dir {
                sources.extend(find_cover_file(&dir));
            }
            if let Ok(b) = std::fs::read(long_path(&album_dir.join(COVER_FILE))) {
                sources.push(b);
            }
            let mut frame = None;
            for b in sources {
                let Ok(c) = process_cover(&b, max) else { continue };
                if !cover_frame::is_frame(c.width, c.height) {
                    return Ok((Some(c), None));
                }
                frame = frame.or(Some(c));
            }
            Ok((None, frame))
        })
        .await
        .unwrap_or((None, None));
        let (local, frame) = local;
        if local.is_some() {
            return local;
        }
        let online = self.online_cover(a, id).await;
        match (online, frame) {
            (Some(c), _) => Some(c),
            (None, Some(f)) => blocking(move || Ok(cut_bars(f))).await.ok(),
            (None, None) => None,
        }
    }

    /// The album art online (the Cover Art Archive, the identity's cover
    /// url), else the web art the target track's album has.
    async fn online_cover(&self, a: &Analyzed, id: &Identity) -> Option<ProcessedCover> {
        let max = self.inner.cfg.cover_max_edge;
        let meta = self.inner.metadata.as_ref()?;
        if !self.inner.cfg.fetch_cover_online {
            return None;
        }
        let timeout = self.inner.cfg.online_timeout;
        let bytes = if let Some(rel) = &id.release_mbid {
            tokio::time::timeout(
                timeout,
                meta.cover_art
                    .fetch_front_with_fallback(rel, id.release_group_mbid.as_deref(), max),
            )
            .await
            .ok()
            .and_then(|r| r.ok())
            .flatten()
            .map(|c| c.bytes)
        } else {
            None
        };
        let target_art = {
            let (lib, targets) = (self.inner.library.clone(), a.targets.clone());
            blocking(move || Ok(target_web_cover(&lib, &targets))).await.ok().flatten()
        };
        let mut bytes = bytes;
        for url in [id.cover_url.clone(), target_art].into_iter().flatten() {
            if bytes.is_some() {
                break;
            }
            bytes = tokio::time::timeout(timeout, meta.cover_art.download(&large_web_cover(&url)))
                .await
                .ok()
                .and_then(|r| r.ok());
        }
        let bytes = bytes?;
        let c = blocking(move || process_cover(&bytes, max)).await.ok()?;
        (!cover_frame::is_frame(c.width, c.height)).then_some(c)
    }
}

/// The web cover (a stream's album art) of the first target track's album.
fn target_web_cover(lib: &Library, targets: &ImportTargets) -> Option<String> {
    targets.tracks.iter().find_map(|t| {
        let release = lib.get_track(*t).ok().flatten()?.release_id?;
        lib.get_release(release).ok().flatten()?.cover_ref.filter(|c| nikgob_library::is_web_ref(c))
    })
}

/// A YouTube Music cover url (`…=w544-h544-l90-rj`) asked at 1200 px; any
/// other url as it is.
pub(super) fn large_web_cover(url: &str) -> String {
    match url.rsplit_once('=') {
        Some((base, size)) if url.contains("googleusercontent.com") && size.starts_with('w') => {
            format!("{base}=w1200-h1200-l90-rj")
        }
        _ => url.to_owned(),
    }
}

/// A frame of a video without its black bars (the art of a song uploaded
/// with it), re-encoded; as it is when it has none or does not decode.
pub(super) fn cut_bars(c: ProcessedCover) -> ProcessedCover {
    let Ok(img) = image::load_from_memory(&c.jpeg) else { return c };
    let rgba = img.to_rgba8();
    let Some((x, y, w, h)) = cover_frame::content(rgba.width(), rgba.height(), rgba.as_raw()) else { return c };
    let cut = image::DynamicImage::ImageRgba8(rgba).crop_imm(x, y, w, h);
    let mut png = Vec::new();
    if cut.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).is_err() {
        return c;
    }
    process_cover(&png, w.max(h)).unwrap_or(c)
}

/// Writes `cover.jpg` and `ambient.png` when the album folder has none or a
/// smaller cover. Returns whether the cover changed.
pub(super) fn write_cover_files(dir: &Path, cover: &ProcessedCover) -> Result<bool> {
    let target = dir.join(COVER_FILE);
    let replace = match image::image_dimensions(long_path(&target)) {
        Ok((w, h)) => cover.width.max(cover.height) > w.max(h),
        Err(_) => true,
    };
    if !replace {
        return Ok(false);
    }
    let visuals = derive_visuals(&cover.jpeg)?;
    let tmp = dir.join(".cover.nikgob-tmp.jpg");
    std::fs::write(long_path(&tmp), &cover.jpeg).map_err(fsutil::io_err(&tmp))?;
    fsutil::move_file(&tmp, &target)?;
    let ambient = dir.join(AMBIENT_FILE);
    std::fs::write(long_path(&ambient), &visuals.ambient_png).map_err(fsutil::io_err(&ambient))?;
    Ok(true)
}

fn find_cover_file(dir: &Path) -> Option<Vec<u8>> {
    let rd = std::fs::read_dir(long_path(dir)).ok()?;
    let mut best: Option<(usize, PathBuf)> = None;
    for e in rd.flatten() {
        let path = e.path();
        let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().to_lowercase()) else {
            continue;
        };
        let ext = path
            .extension()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if !matches!(ext.as_str(), "jpg" | "jpeg" | "png") {
            continue;
        }
        if let Some(rank) = COVER_NAMES.iter().position(|n| *n == stem)
            && best.as_ref().is_none_or(|(r, _)| rank < *r)
        {
            best = Some((rank, path));
        }
    }
    std::fs::read(long_path(&best?.1)).ok()
}
