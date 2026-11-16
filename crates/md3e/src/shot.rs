//! A screenshot run's pictures, shared by the desktop's and the phone's
//! (`NIKGOB_SHOT`): the frame `ViewportCommand::Screenshot` answered with,
//! saved as PNG, or as JPEG for a `.jpg` path.

use std::path::Path;
use std::sync::Arc;

/// The screenshot that came in this frame's input, if one did.
pub fn taken(ctx: &egui::Context) -> Option<Arc<egui::ColorImage>> {
    ctx.input(|i| {
        i.raw.events.iter().find_map(|e| match e {
            egui::Event::Screenshot { image, .. } => Some(image.clone()),
            _ => None,
        })
    })
}

/// Saves `img` at `path`: PNG, or JPEG (without alpha) at `quality` for a
/// `.jpg` / `.jpeg` path.
pub fn save(img: &egui::ColorImage, path: &Path, quality: u8) -> image::ImageResult<()> {
    let [w, h] = img.size;
    let raw: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
    let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, raw) else {
        return Err(image::ImageError::Parameter(image::error::ParameterError::from_kind(
            image::error::ParameterErrorKind::DimensionMismatch,
        )));
    };
    let jpeg = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"));
    if !jpeg {
        return buf.save(path);
    }
    let rgb = image::DynamicImage::ImageRgba8(buf).to_rgb8();
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    image::codecs::jpeg::JpegEncoder::new_with_quality(file, quality).encode_image(&rgb)
}
