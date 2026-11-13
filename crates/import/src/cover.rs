//! Cover handling and the visual data derived at import time: a JPEG of at most `cover_max_edge` px, the palette seed
//! (quantize + score like Android / `material-color-utilities`), and a 16x16
//! "ambient" texture, a box-downscaled copy of the cover that the UI blows up
//! into a soft glow.
//!
//! The seed calls mirror `nikgob_md3e::color::seed_from_image` on purpose so the
//! UI crate is not a dependency of the importer.

use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat, RgbImage, RgbaImage};
use material_colors::color::Argb;
use material_colors::quantize::{Quantizer, QuantizerCelebi};
use material_colors::score::Score;

use crate::error::{ImportError, Result};

/// Longest side the image is sampled down to before quantizing.
const SEED_SAMPLE_SIDE: usize = 128;
/// Colours the quantizer clusters to, as Android's wallpaper colours do.
const SEED_QUANTIZE_COLORS: usize = 128;
const JPEG_QUALITY: u8 = 88;
/// Side of the ambient texture.
pub const AMBIENT_SIDE: u32 = 16;
/// A JPEG already within the limits is kept byte for byte up to this size.
const KEEP_ORIGINAL_BYTES: usize = 1_500_000;

/// A cover ready to be embedded and saved as `cover.jpg`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessedCover {
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Decodes `bytes` (JPEG or PNG), shrinks it to `max_edge` if larger, and
/// returns JPEG bytes. A JPEG that is small enough is passed through
/// untouched, so covers are not recompressed for nothing.
pub fn process_cover(bytes: &[u8], max_edge: u32) -> Result<ProcessedCover> {
    let format = image::guess_format(bytes).map_err(|e| ImportError::Image(e.to_string()))?;
    let img = image::load_from_memory_with_format(bytes, format)
        .map_err(|e| ImportError::Image(e.to_string()))?;
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return Err(ImportError::Image("empty image".into()));
    }
    let fits = w.max(h) <= max_edge;
    if fits && format == ImageFormat::Jpeg && bytes.len() <= KEEP_ORIGINAL_BYTES {
        return Ok(ProcessedCover {
            jpeg: bytes.to_vec(),
            width: w,
            height: h,
        });
    }
    let img = if fits {
        img
    } else {
        img.resize(max_edge, max_edge, FilterType::Lanczos3)
    };
    let rgb: RgbImage = img.to_rgb8();
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
        .encode_image(&rgb)
        .map_err(|e| ImportError::Image(e.to_string()))?;
    Ok(ProcessedCover {
        jpeg: out,
        width: rgb.width(),
        height: rgb.height(),
    })
}

/// Palette seed (`0xFFRRGGBB`) from straight-alpha RGBA pixels. `None` for
/// colourless art (greys, black and white) and for bad input, so the caller can
/// fall back to the seed from settings.
pub fn seed_from_rgba(rgba: &[u8], width: usize, height: usize) -> Option<u32> {
    if width == 0 || height == 0 || rgba.len() != width.checked_mul(height)?.checked_mul(4)? {
        return None;
    }
    let scale = (width.max(height).div_ceil(SEED_SAMPLE_SIDE)).max(1);
    let (out_w, out_h) = (width.div_ceil(scale), height.div_ceil(scale));
    let mut pixels = Vec::with_capacity(out_w * out_h);
    for oy in 0..out_h {
        let y = (oy * scale + scale / 2).min(height - 1);
        for ox in 0..out_w {
            let x = (ox * scale + scale / 2).min(width - 1);
            let i = (y * width + x) * 4;
            if rgba[i + 3] >= 128 {
                pixels.push(Argb::new(255, rgba[i], rgba[i + 1], rgba[i + 2]));
            }
        }
    }
    if pixels.is_empty() {
        return None;
    }
    let result = QuantizerCelebi::quantize(&pixels, SEED_QUANTIZE_COLORS);
    let none = Argb::new(0, 0, 0, 0);
    let ranked = Score::score(&result.color_to_count, Some(1), Some(none), Some(true));
    let best = *ranked.first()?;
    (best.alpha != 0).then(|| u32::from_be_bytes([0xFF, best.red, best.green, best.blue]))
}

/// Box-downscales to a 16x16 PNG (the "ambient" glow texture).
pub fn ambient_png(img: &DynamicImage) -> Result<Vec<u8>> {
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width(), rgb.height());
    let side = AMBIENT_SIDE;
    let mut out = RgbImage::new(side, side);
    for ty in 0..side {
        let (y0, y1) = (ty * h / side, ((ty + 1) * h / side).max(ty * h / side + 1));
        for tx in 0..side {
            let (x0, x1) = (tx * w / side, ((tx + 1) * w / side).max(tx * w / side + 1));
            let mut sum = [0u64; 3];
            let mut n = 0u64;
            for y in y0..y1.min(h) {
                for x in x0..x1.min(w) {
                    let p = rgb.get_pixel(x, y).0;
                    sum[0] += u64::from(p[0]);
                    sum[1] += u64::from(p[1]);
                    sum[2] += u64::from(p[2]);
                    n += 1;
                }
            }
            let n = n.max(1);
            out.put_pixel(
                tx,
                ty,
                image::Rgb([(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8]),
            );
        }
    }
    let mut png = Vec::new();
    out.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .map_err(|e| ImportError::Image(e.to_string()))?;
    Ok(png)
}

/// What the library stores per release next to the cover.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Visuals {
    /// `None` when the art has no usable colour (the caller applies its fallback).
    pub seed_argb: Option<u32>,
    /// 16x16 PNG.
    pub ambient_png: Vec<u8>,
}

/// Palette seed and ambient texture of a cover image.
pub fn derive_visuals(cover: &[u8]) -> Result<Visuals> {
    let img = image::load_from_memory(cover).map_err(|e| ImportError::Image(e.to_string()))?;
    // The seed needs few pixels; look at a reduced copy.
    let small = if img.width().max(img.height()) > 256 {
        img.thumbnail(256, 256)
    } else {
        img.clone()
    };
    let rgba: RgbaImage = small.to_rgba8();
    let seed_argb = seed_from_rgba(
        rgba.as_raw(),
        rgba.width() as usize,
        rgba.height() as usize,
    );
    Ok(Visuals {
        seed_argb,
        ambient_png: ambient_png(&img)?,
    })
}

#[cfg(test)]
pub(crate) mod testimg {
    use image::{Rgb, RgbImage};

    pub fn solid_png(rgb: [u8; 3], w: u32, h: u32) -> Vec<u8> {
        let img = RgbImage::from_pixel(w, h, Rgb(rgb));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    pub fn solid_jpeg(rgb: [u8; 3], w: u32, h: u32) -> Vec<u8> {
        let img = RgbImage::from_pixel(w, h, Rgb(rgb));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Jpeg)
            .unwrap();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::testimg::*;
    use super::*;

    #[test]
    fn large_covers_are_shrunk_to_the_limit() {
        let png = solid_png([30, 90, 200], 2400, 1800);
        let c = process_cover(&png, 1200).unwrap();
        assert_eq!((c.width, c.height), (1200, 900));
        let back = image::load_from_memory(&c.jpeg).unwrap();
        assert_eq!(back.width(), 1200);
        assert_eq!(image::guess_format(&c.jpeg).unwrap(), ImageFormat::Jpeg);
    }

    #[test]
    fn small_jpegs_pass_through_and_small_pngs_become_jpegs() {
        let jpg = solid_jpeg([200, 20, 20], 600, 600);
        assert_eq!(process_cover(&jpg, 1200).unwrap().jpeg, jpg);
        let png = solid_png([200, 20, 20], 600, 600);
        let c = process_cover(&png, 1200).unwrap();
        assert_eq!((c.width, c.height), (600, 600));
        assert_eq!(image::guess_format(&c.jpeg).unwrap(), ImageFormat::Jpeg);
        assert!(process_cover(b"not an image", 1200).is_err());
    }

    #[test]
    fn seed_is_the_cover_hue_and_none_for_grey() {
        let v = derive_visuals(&solid_png([200, 30, 40], 300, 200)).unwrap();
        let seed = v.seed_argb.expect("red seed");
        let (r, g, b) = ((seed >> 16) & 0xFF, (seed >> 8) & 0xFF, seed & 0xFF);
        assert!(r > g + 60 && r > b + 60, "{seed:08X}");
        assert_eq!(seed >> 24, 0xFF);
        let grey = derive_visuals(&solid_png([128, 128, 128], 200, 200)).unwrap();
        assert_eq!(grey.seed_argb, None);
    }

    #[test]
    fn ambient_is_a_16x16_downscale_of_the_cover() {
        // Left half red, right half blue.
        let mut img = RgbImage::new(320, 320);
        for (x, _, p) in img.enumerate_pixels_mut() {
            *p = if x < 160 {
                image::Rgb([255, 0, 0])
            } else {
                image::Rgb([0, 0, 255])
            };
        }
        let mut png = Vec::new();
        img.write_to(&mut Cursor::new(&mut png), ImageFormat::Png).unwrap();
        let v = derive_visuals(&png).unwrap();
        let amb = image::load_from_memory(&v.ambient_png).unwrap().to_rgb8();
        assert_eq!((amb.width(), amb.height()), (16, 16));
        assert_eq!(amb.get_pixel(2, 8).0, [255, 0, 0]);
        assert_eq!(amb.get_pixel(13, 8).0, [0, 0, 255]);
    }

    #[test]
    fn seed_rejects_bad_buffers() {
        assert_eq!(seed_from_rgba(&[], 0, 0), None);
        assert_eq!(seed_from_rgba(&[0; 10], 4, 4), None);
        assert_eq!(seed_from_rgba(&vec![0u8; 16 * 16 * 4], 16, 16), None);
    }
}
