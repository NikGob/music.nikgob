//! Covers that are frames of a video rather than album art.
//!
//! YouTube gives a video's picture as a 16:9 frame (`hq720`, `maxresdefault`)
//! or a 4:3 one with black bars above and below (`hqdefault`); a song
//! uploaded with its album art is often that square art between black bars
//! at the sides. [`content`] finds the picture inside such bars, so the art
//! can be cut out of its frame; [`is_frame`] tells a cover that is still
//! wide after that, which is a frame of the video, worth replacing by the
//! real art.

/// How far from square a cover may be and still be album art (scans of
/// booklets, a few pixels cut by an encoder).
pub const SQUARE_SLACK: f32 = 0.1;

/// A channel value at or under this is black (JPEG noise lifts true black
/// a little).
const DARK: u8 = 28;
/// The share of a row's or a column's pixels that must be black for it to
/// be a bar (a logo or a subtitle may cross one).
const BAR_SHARE: f32 = 0.97;
/// Bars thinner than this share of the side are not cut: a dark edge of the
/// art itself.
const MIN_BAR: f32 = 0.04;

/// Whether a picture of `width` × `height` is about square.
pub fn squarish(width: u32, height: u32) -> bool {
    if width == 0 || height == 0 {
        return false;
    }
    let r = width as f32 / height as f32;
    (1.0 - SQUARE_SLACK..=1.0 / (1.0 - SQUARE_SLACK)).contains(&r)
}

/// Whether a cover of `width` × `height` (after [`content`]) is a frame of a
/// video: 4:3 or wider. A tall picture is a scan of a booklet, not a frame.
pub fn is_frame(width: u32, height: u32) -> bool {
    width > 0 && height > 0 && width as f32 >= 1.25 * height as f32
}

/// The picture inside black bars of an RGBA image (`rgba` holds `width` ×
/// `height` pixels, 4 bytes each), as `(x, y, width, height)`; `None` when
/// the image is about square already, has no bars, or is all black. Bars are
/// cut on both sides of an axis only together (a letterbox or a pillarbox),
/// each at least [`MIN_BAR`] of the side.
pub fn content(width: u32, height: u32, rgba: &[u8]) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || rgba.len() < w * h * 4 || squarish(width, height) {
        return None;
    }
    let dark = |x: usize, y: usize| {
        let i = (y * w + x) * 4;
        rgba[i] <= DARK && rgba[i + 1] <= DARK && rgba[i + 2] <= DARK
    };
    let row_bar = |y: usize| (0..w).filter(|&x| dark(x, y)).count() as f32 >= BAR_SHARE * w as f32;
    let col_bar = |x: usize| (0..h).filter(|&y| dark(x, y)).count() as f32 >= BAR_SHARE * h as f32;
    let top = (0..h).take_while(|&y| row_bar(y)).count();
    if top == h {
        return None;
    }
    let bottom = (0..h).rev().take_while(|&y| row_bar(y)).count();
    let left = (0..w).take_while(|&x| col_bar(x)).count();
    let right = (0..w).rev().take_while(|&x| col_bar(x)).count();
    let min_v = (MIN_BAR * h as f32).ceil() as usize;
    let min_h = (MIN_BAR * w as f32).ceil() as usize;
    let (top, bottom) = if top >= min_v && bottom >= min_v { (top, bottom) } else { (0, 0) };
    let (left, right) = if left >= min_h && right >= min_h { (left, right) } else { (0, 0) };
    if top + bottom + left + right == 0 {
        return None;
    }
    let (cw, ch) = (w - left - right, h - top - bottom);
    // What is left must still be a picture.
    if cw * 4 < w || ch * 4 < h {
        return None;
    }
    Some((left as u32, top as u32, cw as u32, ch as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `w` × `h` black image with a grey picture at `(x, y, pw, ph)`.
    fn framed(w: u32, h: u32, (x, y, pw, ph): (u32, u32, u32, u32)) -> Vec<u8> {
        let mut out = vec![0u8; (w * h * 4) as usize];
        for py in 0..h {
            for px in 0..w {
                let i = ((py * w + px) * 4) as usize;
                let inside = px >= x && px < x + pw && py >= y && py < y + ph;
                let v = if inside { 120 + ((px + py) % 60) as u8 } else { (px % 7) as u8 };
                out[i..i + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        out
    }

    #[test]
    fn square_art_comes_out_of_a_pillarbox() {
        let img = framed(1280, 720, (280, 0, 720, 720));
        assert_eq!(content(1280, 720, &img), Some((280, 0, 720, 720)));
        assert!(!is_frame(720, 720));
    }

    #[test]
    fn a_letterboxed_thumbnail_loses_its_bars() {
        // hqdefault: 480 × 360 with a 16:9 picture.
        let img = framed(480, 360, (0, 45, 480, 270));
        let (_, _, w, h) = content(480, 360, &img).unwrap();
        assert_eq!((w, h), (480, 270));
        assert!(is_frame(w, h), "a frame of the video still");
    }

    #[test]
    fn covers_without_bars_stay() {
        // Square art is never cut, even with a dark border of its own.
        let img = framed(600, 600, (40, 40, 520, 520));
        assert_eq!(content(600, 600, &img), None);
        // A plain frame, a dark edge on one side only, an all-black frame.
        assert_eq!(content(1280, 720, &framed(1280, 720, (0, 0, 1280, 720))), None);
        assert_eq!(content(1280, 720, &framed(1280, 720, (100, 0, 1180, 720))), None);
        assert_eq!(content(1280, 720, &vec![0u8; 1280 * 720 * 4]), None);
        assert!(is_frame(1280, 720));
        assert!(is_frame(853, 480));
        assert!(is_frame(480, 360));
        assert!(!is_frame(604, 600));
        assert!(!is_frame(604, 800), "a booklet's scan");
        assert!(!is_frame(0, 0));
    }
}
