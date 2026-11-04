//! A real blur for the soft edges of the lyrics («Мягкие края»). egui has
//! no filters, so a line in the soft band is drawn from a texture of its
//! own: its glyphs copied out of the font atlas into a coverage bitmap,
//! blurred on the CPU (a Gaussian, as three box passes each way) and
//! uploaded once. The blur comes in fine fixed steps ([`STEP`] px of
//! sigma), too close to tell apart as a line moves through them; a bitmap
//! is kept by its line's text, size and step, and dropped a few seconds
//! after it was last drawn. A frame makes at most [`PER_FRAME`] new ones;
//! a line whose step is not made yet is drawn meanwhile with the nearest
//! step there is.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use egui::{Color32, ColorImage, Context, Galley, Id, Mesh, Painter, Pos2, Rect, TextureHandle, TextureOptions, pos2, vec2};

/// Sigma of one blur step, in physical pixels.
const STEP: f32 = 0.5;
/// The most blur there is: [`STEP`] × this.
const STEPS: u8 = 24;
/// Seconds a bitmap is kept after it was last drawn.
const KEEP_S: f64 = 4.0;
/// New bitmaps a frame makes at most.
const PER_FRAME: usize = 6;

/// A blurred line: its texture and where it lies from the galley's origin
/// (points).
struct Blurred {
    tex: TextureHandle,
    rect: Rect,
    used: f64,
}

/// The font atlas's coverage, copied when it has grown since.
struct Atlas {
    size: [usize; 2],
    fill: f32,
    alpha: Vec<u8>,
}

#[derive(Default)]
struct Cache {
    atlas: Option<Atlas>,
    items: HashMap<(u64, u8), Blurred>,
    pass: u64,
    made: usize,
    swept: f64,
}

/// `g` at `pos` in `color`, blurred by `sigma` points (0: sharp).
pub(crate) fn galley(painter: &Painter, pos: Pos2, g: &Arc<Galley>, color: Color32, sigma: f32) {
    if color.a() == 0 {
        return;
    }
    let ctx = painter.ctx();
    let ppp = ctx.pixels_per_point();
    let step = ((sigma * ppp / STEP).round().max(0.0) as u32).min(u32::from(STEPS)) as u8;
    if step == 0 || g.rows.is_empty() {
        painter.galley_with_override_text_color(pos, g.clone(), color);
        return;
    }
    let key = {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        g.job.hash(&mut h);
        ppp.to_bits().hash(&mut h);
        g.rect.width().to_bits().hash(&mut h);
        g.rect.height().to_bits().hash(&mut h);
        h.finish()
    };
    let cache = ctx.data_mut(|d| d.get_temp_mut_or_default::<Arc<Mutex<Cache>>>(Id::new("md3e-lyrics-blur")).clone());
    let mut c = cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let (now, pass) = (ctx.input(|i| i.time), ctx.cumulative_pass_nr());
    if c.pass != pass {
        c.pass = pass;
        c.made = 0;
        if now - c.swept > 1.0 {
            c.swept = now;
            c.items.retain(|_, b| now - b.used < KEEP_S);
        }
    }
    // Past this frame's budget the step waits for the next frame (one
    // that cannot be made is not asked for again and again).
    let mut waiting = false;
    if !c.items.contains_key(&(key, step)) {
        if c.made < PER_FRAME {
            c.made += 1;
            if let Some((image, rect)) = make(ctx, &mut c, g, ppp, f32::from(step) * STEP) {
                let tex = ctx.load_texture("md3e-lyrics-blur", image, TextureOptions::LINEAR);
                c.items.insert((key, step), Blurred { tex, rect, used: now });
            }
        } else {
            waiting = true;
        }
    }
    // The step asked for, else the nearest one made, else the line as it is.
    let near = (0..=STEPS).flat_map(|d| [step.saturating_sub(d), step.saturating_add(d).min(STEPS)]).find(|s| *s > 0 && c.items.contains_key(&(key, *s)));
    if waiting {
        ctx.request_repaint();
    }

    match near.and_then(|s| c.items.get_mut(&(key, s))) {
        Some(b) => {
            b.used = now;
            let mut mesh = Mesh::with_texture(b.tex.id());
            mesh.add_rect_with_uv(b.rect.translate(pos.to_vec2()), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), color);
            painter.add(egui::Shape::mesh(mesh));
        }
        None => painter.galley_with_override_text_color(pos, g.clone(), color),
    }
}

/// The blurred bitmap of `g` (white, its alpha the coverage) and where it
/// lies from the galley's origin.
fn make(ctx: &Context, c: &mut Cache, g: &Galley, ppp: f32, sigma: f32) -> Option<(ColorImage, Rect)> {
    let (size, fill) = ctx.fonts(|f| (f.font_image_size(), f.font_atlas_fill_ratio()));
    if c.atlas.as_ref().is_none_or(|a| a.size != size || a.fill != fill) {
        let image = ctx.fonts(|f| f.image());
        c.atlas = Some(Atlas { size: image.size, fill, alpha: image.pixels.iter().map(|p| p.a()).collect() });
    }
    let atlas = c.atlas.as_ref()?;
    raster(g, ppp, atlas.size, &atlas.alpha, sigma)
}

/// The glyph quads of `g`: where each lies (points, from the galley's
/// origin) and its texels in the atlas.
fn quads(g: &Galley) -> Vec<(Rect, Rect)> {
    let mut out = Vec::new();
    for row in &g.rows {
        let v = &row.row.visuals;
        let Some(verts) = v.mesh.vertices.get(v.glyph_vertex_range.clone()) else { continue };
        for q in verts.chunks_exact(4) {
            let at = Rect::from_points(&q.iter().map(|v| v.pos).collect::<Vec<_>>()).translate(row.pos.to_vec2());
            let uv = Rect::from_points(&q.iter().map(|v| v.uv).collect::<Vec<_>>());
            if at.is_positive() && uv.is_positive() {
                out.push((at, uv));
            }
        }
    }
    out
}

/// Draws the glyphs of `g` at `ppp` pixels a point from the atlas's
/// coverage (`alpha`, `size`), blurs them by `sigma` px and gives the
/// bitmap with where it lies (points). A wide blur is made at a lower
/// resolution (the texture is drawn smoothed), which keeps it cheap.
fn raster(g: &Galley, ppp: f32, size: [usize; 2], alpha: &[u8], sigma: f32) -> Option<(ColorImage, Rect)> {
    let quads = quads(g);
    let bounds = quads.iter().map(|q| q.0).reduce(|a, b| a.union(b))?;
    let down = ((sigma / 2.0).floor() as usize).clamp(1, 6);
    let pad = ((3.0 * sigma).ceil() as usize + 1).next_multiple_of(down);
    let w = ((bounds.width() * ppp).ceil() as usize + 2 * pad).next_multiple_of(down);
    let h = ((bounds.height() * ppp).ceil() as usize + 2 * pad).next_multiple_of(down);
    if w * h > 4096 * 512 {
        return None;
    }
    let mut cover = vec![0.0f32; w * h];
    for (at, uv) in &quads {
        let x0 = ((at.left() - bounds.left()) * ppp).round() as usize + pad;
        let y0 = ((at.top() - bounds.top()) * ppp).round() as usize + pad;
        let x1 = (((at.right() - bounds.left()) * ppp).round() as usize + pad).min(w);
        let y1 = (((at.bottom() - bounds.top()) * ppp).round() as usize + pad).min(h);
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        let (sx, sy) = (uv.width() / (x1 - x0) as f32, uv.height() / (y1 - y0) as f32);
        for y in y0..y1 {
            let ty = ((uv.top() + (y - y0) as f32 * sy + sy * 0.5) as usize).min(size[1] - 1);
            for x in x0..x1 {
                let tx = ((uv.left() + (x - x0) as f32 * sx + sx * 0.5) as usize).min(size[0] - 1);
                let a = f32::from(alpha[ty * size[0] + tx]) / 255.0;
                let px = &mut cover[y * w + x];
                *px = px.max(a);
            }
        }
    }
    let (cover, w, h) = shrink(&cover, w, h, down);
    let blurred = gaussian(cover, w, h, sigma / down as f32);
    let pixels = blurred.iter().map(|a| Color32::from_white_alpha((a.clamp(0.0, 1.0) * 255.0).round() as u8)).collect();
    let k = down as f32 / ppp;
    let rect = Rect::from_min_size(bounds.min - vec2(pad as f32, pad as f32) / ppp, vec2(w as f32, h as f32) * k);
    Some((ColorImage::new([w, h], pixels), rect))
}

/// `v` (`w` × `h`) averaged over `d` × `d` blocks (`w` and `h` multiples of `d`).
fn shrink(v: &[f32], w: usize, h: usize, d: usize) -> (Vec<f32>, usize, usize) {
    if d <= 1 {
        return (v.to_vec(), w, h);
    }
    let (sw, sh) = (w / d, h / d);
    let mut out = vec![0.0f32; sw * sh];
    for y in 0..h {
        for x in 0..w {
            out[(y / d) * sw + x / d] += v[y * w + x];
        }
    }
    let n = (d * d) as f32;
    out.iter_mut().for_each(|p| *p /= n);
    (out, sw, sh)
}

/// The three box widths whose passes together blur like a Gaussian of
/// `sigma` (Kovesi's approximation).
fn boxes(sigma: f32) -> [usize; 3] {
    let n = 3.0f32;
    let ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let mut wl = ideal.floor() as i32;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wl = wl.max(1);
    let wu = wl + 2;
    let m = ((12.0 * sigma * sigma - n * (wl * wl) as f32 - 4.0 * n * wl as f32 - 3.0 * n) / (-4.0 * wl as f32 - 4.0)).round() as i32;
    std::array::from_fn(|i| if (i as i32) < m { wl as usize } else { wu as usize })
}

/// `v` (`w` × `h`) blurred by a Gaussian of `sigma` px: three box passes
/// along the rows, then three along the columns; outside is empty.
fn gaussian(mut v: Vec<f32>, w: usize, h: usize, sigma: f32) -> Vec<f32> {
    if sigma <= 0.0 {
        return v;
    }
    let mut tmp = vec![0.0f32; v.len()];
    for b in boxes(sigma) {
        let r = b / 2;
        if r == 0 {
            continue;
        }
        box_pass(&v, &mut tmp, w, h, r, true);
        box_pass(&tmp, &mut v, w, h, r, false);
    }
    v
}

/// One box blur of radius `r` along rows (`horizontal`) or columns.
fn box_pass(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize, horizontal: bool) {
    let (len, lines) = if horizontal { (w, h) } else { (h, w) };
    let at = |line: usize, i: usize| if horizontal { line * w + i } else { i * w + line };
    let k = 1.0 / (2 * r + 1) as f32;
    for line in 0..lines {
        let mut sum = 0.0f32;
        for i in 0..=r.min(len - 1) {
            sum += src[at(line, i)];
        }
        for i in 0..len {
            dst[at(line, i)] = sum * k;
            if i + r + 1 < len {
                sum += src[at(line, i + r + 1)];
            }
            if i >= r {
                sum -= src[at(line, i - r)];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The box widths blur as much as the Gaussian asked for.
    #[test]
    fn three_boxes_blur_like_the_gaussian() {
        for sigma in [1.0f32, 2.5, 6.0, 12.0] {
            let var: f32 = boxes(sigma).iter().map(|b| ((b * b) as f32 - 1.0) / 12.0).sum();
            assert!((var.sqrt() - sigma).abs() < 0.5, "{sigma}: {}", var.sqrt());
        }
    }

    /// A dot spreads into a smooth, symmetric hill and keeps its weight.
    #[test]
    fn a_blurred_dot_is_a_smooth_hill() {
        let (w, h) = (41, 41);
        let mut v = vec![0.0f32; w * h];
        v[20 * w + 20] = 1.0;
        let b = gaussian(v, w, h, 4.0);
        let total: f32 = b.iter().sum();
        assert!((total - 1.0).abs() < 1e-3, "{total}");
        let row: Vec<f32> = (0..w).map(|x| b[20 * w + x]).collect();
        for x in 1..=20 {
            assert!(row[20 - x] <= row[20 - x + 1] + 1e-6, "falls off from the middle");
            assert!((row[20 - x] - row[20 + x]).abs() < 1e-5, "symmetric");
        }
        // No second ring: a ghost would show as a bump away from the middle.
        assert!(row[20] > row[16] && row[16] > row[12]);
    }

    #[test]
    fn shrinking_averages_blocks() {
        let v = vec![1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
        let (s, w, h) = shrink(&v, 4, 2, 2);
        assert_eq!((w, h), (2, 1));
        assert_eq!(s, vec![0.5, 0.5]);
    }
}
