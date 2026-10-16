//! Playlist covers: a custom picture, a 2x2 collage of album covers, or a
//! "smart" pick (collage with 4+ different covers, else the most common one,
//! else a generated gradient). Collages and gradients are rendered once into
//! `.musiclib/covers/auto/` under a name derived from what they show, so every
//! place that draws a cover image (cards, rows, headers) shows them as is.

use std::collections::HashMap;
use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
use std::path::{Path, PathBuf};

use image::{Rgb, RgbImage, imageops};
use nikgob_core::model::ReleaseId;
use nikgob_view::PlaylistCoverMode;

/// Stored in `playlist.cover` for "collage" (NULL = smart, else a picture).
pub const COLLAGE_COVER: &str = "@collage";

/// Where generated covers live, relative to the library root.
pub const AUTO_DIR: &str = ".musiclib/covers/auto";

const SIDE: u32 = 512;

/// The cover mode a stored `playlist.cover` value means.
pub fn mode_of(stored: Option<&str>) -> PlaylistCoverMode {
    match stored {
        None => PlaylistCoverMode::Smart,
        Some(COLLAGE_COVER) => PlaylistCoverMode::Collage,
        Some(_) => PlaylistCoverMode::Custom,
    }
}

/// What a collage or smart cover shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    /// One album's cover.
    Single(ReleaseId),
    /// Up to four different covers, 2x2 (fewer repeat crosswise).
    Collage(Vec<PathBuf>),
    /// No covers at all: a generated gradient.
    Placeholder,
}

/// `covers`: `(release, cover file)` per track, in playlist order.
pub fn pick(covers: &[(ReleaseId, PathBuf)], mode: PlaylistCoverMode) -> Pick {
    let mut distinct: Vec<(ReleaseId, &PathBuf)> = Vec::new();
    let mut count: HashMap<ReleaseId, usize> = HashMap::new();
    for (r, p) in covers {
        let n = count.entry(*r).or_default();
        if *n == 0 && !distinct.iter().any(|(_, q)| *q == p) {
            distinct.push((*r, p));
        }
        *n += 1;
    }
    let collage = || Pick::Collage(distinct.iter().take(4).map(|(_, p)| (*p).clone()).collect());
    match (mode, distinct.len()) {
        (_, 0) => Pick::Placeholder,
        (PlaylistCoverMode::Collage, n) if n >= 2 => collage(),
        (PlaylistCoverMode::Smart, n) if n >= 4 => collage(),
        _ => {
            // The most common album; the earliest one among equals.
            let best = distinct
                .iter()
                .max_by_key(|(r, _)| (count[r], std::cmp::Reverse(distinct.iter().position(|(q, _)| q == r))))
                .map(|(r, _)| *r)
                .expect("not empty");
            Pick::Single(best)
        }
    }
}

fn hash(key: &impl std::hash::Hash) -> u64 {
    BuildHasherDefault::<DefaultHasher>::default().hash_one(key)
}

/// The collage file for these covers under `root`, rendered if missing.
/// `None` when no cover could be read.
pub fn collage(root: &Path, covers: &[PathBuf]) -> Option<PathBuf> {
    // Name from the files and their sizes: a replaced cover makes a new collage.
    let key: Vec<(String, u64)> = covers
        .iter()
        .map(|p| (p.display().to_string(), std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)))
        .collect();
    let out = root.join(AUTO_DIR).join(format!("collage-{:016x}.jpg", hash(&key)));
    if out.exists() {
        return Some(out);
    }
    let tile = SIDE / 2;
    let tiles: Vec<RgbImage> = covers
        .iter()
        .filter_map(|p| image::open(p).map_err(|e| log::debug!("collage {}: {e}", p.display())).ok())
        .map(|img| img.resize_to_fill(tile, tile, imageops::FilterType::Triangle).to_rgb8())
        .collect();
    if tiles.is_empty() {
        return None;
    }
    // 2 covers: a b / b a; 3: a b / c a.
    let order: [usize; 4] = match tiles.len() {
        1 => [0, 0, 0, 0],
        2 => [0, 1, 1, 0],
        3 => [0, 1, 2, 0],
        _ => [0, 1, 2, 3],
    };
    let mut canvas = RgbImage::new(SIDE, SIDE);
    for (slot, &i) in order.iter().enumerate() {
        let (x, y) = ((slot as u32 % 2) * tile, (slot as u32 / 2) * tile);
        imageops::replace(&mut canvas, &tiles[i], i64::from(x), i64::from(y));
    }
    save(&canvas, &out).then_some(out)
}

/// A soft two-colour gradient with a round blob, seeded by `seed` (the
/// playlist id), rendered if missing.
pub fn placeholder(root: &Path, seed: &impl std::hash::Hash) -> Option<PathBuf> {
    let h = hash(seed);
    let out = root.join(AUTO_DIR).join(format!("gradient-{h:016x}.png"));
    if out.exists() {
        return Some(out);
    }
    let hue = (h % 360) as f32;
    let a = hsl(hue, 0.55, 0.42);
    let b = hsl((hue + 50.0) % 360.0, 0.6, 0.62);
    let blob = hsl((hue + 25.0) % 360.0, 0.75, 0.82);
    let (cx, cy, r) = (SIDE as f32 * 0.68, SIDE as f32 * 0.34, SIDE as f32 * 0.3);
    let img = RgbImage::from_fn(SIDE, SIDE, |x, y| {
        let t = (x + y) as f32 / (2 * SIDE) as f32;
        let mut c = mix(a, b, t);
        let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
        // Antialiased edge, a little translucent.
        let k = ((r - d) / 2.0).clamp(0.0, 1.0) * 0.55;
        c = mix(c, blob, k);
        Rgb(c.map(|v| v.round() as u8))
    });
    save(&img, &out).then_some(out)
}

fn save(img: &RgbImage, out: &Path) -> bool {
    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Written aside and renamed: a half-written file must never be shown.
    let tmp = out.with_extension("tmp");
    let format = if out.extension().is_some_and(|e| e == "png") {
        image::ImageFormat::Png
    } else {
        image::ImageFormat::Jpeg
    };
    match img.save_with_format(&tmp, format).and_then(|()| std::fs::rename(&tmp, out).map_err(Into::into)) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("playlist cover {}: {e}", out.display());
            let _ = std::fs::remove_file(&tmp);
            false
        }
    }
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

fn hsl(h: f32, s: f32, l: f32) -> [f32; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [(r + m) * 255.0, (g + m) * 255.0, (b + m) * 255.0]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn covers(spec: &[(u8, usize)]) -> (Vec<ReleaseId>, Vec<(ReleaseId, PathBuf)>) {
        let ids: Vec<ReleaseId> = (0..6).map(|_| ReleaseId::new()).collect();
        let list = spec
            .iter()
            .flat_map(|&(r, n)| std::iter::repeat_n((ids[r as usize], PathBuf::from(format!("c{r}.jpg"))), n))
            .collect();
        (ids, list)
    }

    #[test]
    fn smart_cover_needs_four_albums_for_a_collage() {
        let (_, four) = covers(&[(0, 1), (1, 1), (2, 1), (3, 1), (4, 2)]);
        assert_eq!(
            pick(&four, PlaylistCoverMode::Smart),
            Pick::Collage(["c0.jpg", "c1.jpg", "c2.jpg", "c3.jpg"].map(PathBuf::from).to_vec()),
            "the first four different covers, in playlist order"
        );
        let (ids, three) = covers(&[(0, 1), (1, 3), (2, 1)]);
        assert_eq!(pick(&three, PlaylistCoverMode::Smart), Pick::Single(ids[1]), "the most common album");
        let (ids, tie) = covers(&[(2, 2), (1, 2)]);
        assert_eq!(pick(&tie, PlaylistCoverMode::Smart), Pick::Single(ids[2]), "ties: the earliest");
        assert_eq!(pick(&[], PlaylistCoverMode::Smart), Pick::Placeholder);
    }

    #[test]
    fn collage_mode_takes_two_covers_already() {
        let (_, two) = covers(&[(0, 1), (1, 5)]);
        assert_eq!(
            pick(&two, PlaylistCoverMode::Collage),
            Pick::Collage(vec![PathBuf::from("c0.jpg"), PathBuf::from("c1.jpg")])
        );
        let (ids, one) = covers(&[(3, 2)]);
        assert_eq!(pick(&one, PlaylistCoverMode::Collage), Pick::Single(ids[3]));
    }

    #[test]
    fn collage_and_gradient_are_rendered_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut files = Vec::new();
        for (i, c) in [[200u8, 30, 30], [30, 200, 30], [30, 30, 200]].iter().enumerate() {
            let p = dir.path().join(format!("{i}.png"));
            RgbImage::from_pixel(64, 48, Rgb(*c)).save(&p).unwrap();
            files.push(p);
        }
        let out = collage(dir.path(), &files).unwrap();
        let img = image::open(&out).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), (SIDE, SIDE));
        // a b / c a.
        let at = |x: u32, y: u32| img.get_pixel(x, y).0;
        assert!(at(100, 100)[0] > 150 && at(400, 100)[1] > 150 && at(100, 400)[2] > 150 && at(400, 400)[0] > 150);
        assert_eq!(collage(dir.path(), &files), Some(out.clone()), "same covers, same file");

        let g = placeholder(dir.path(), &"playlist-1").unwrap();
        assert!(g.exists());
        assert_ne!(placeholder(dir.path(), &"playlist-2").unwrap(), g);
        assert_eq!(collage(dir.path(), &[dir.path().join("missing.jpg")]), None);
    }
}
