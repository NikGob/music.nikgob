//! A real blur for the soft edges of the lyrics («Мягкие края»). egui has
//! no filters, so a line in the soft band is drawn from a texture of its
//! own: its glyphs copied out of the font atlas into a coverage bitmap,
//! blurred on the CPU (a Gaussian, as three box passes each way) and
//! uploaded once. The blur comes in fine fixed steps ([`STEP`] px of
//! sigma), too close to tell apart as a line moves through them; a bitmap
//! is kept by its line's text, size and step, and dropped a few seconds
//! after it was last drawn.
//!
//! The bitmaps are made on threads of their own: when the current line
//! changes, every line in the bands moves through new steps at once, and
//! making them in the frame (a few milliseconds each, six a frame) stalled
//! the very frames the lines start moving in. A frame uploads at most
//! [`UPLOADS`] finished ones; a line whose step is not made yet is drawn
//! meanwhile with the nearest step there is.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use egui::{Color32, ColorImage, Context, Galley, Id, Mesh, Painter, Pos2, Rect, TextureHandle, TextureOptions, pos2, vec2};

/// Sigma of one blur step, in physical pixels.
const STEP: f32 = 0.5;
/// The most blur there is: [`STEP`] × this.
const STEPS: u8 = 24;
/// Seconds a bitmap is kept after it was last drawn.
const KEEP_S: f64 = 4.0;
/// Finished bitmaps a frame uploads at most (each is a texture of its own).
const UPLOADS: usize = 4;
/// Bitmaps waiting to be made at most: past this the oldest asks are
/// dropped (a moving line has passed their steps by then). The newest ask
/// is made first for the same reason.
const QUEUE: usize = 24;
/// Threads making bitmaps.
const WORKERS: usize = 2;
/// With no thread to make them on, bitmaps a frame makes itself at most.
const PER_FRAME: usize = 2;

/// A line's galley (its text, size and look) and its blur step.
type Key = (u64, u8);

/// A blurred line: its texture and where it lies from the galley's origin
/// (points).
struct Blurred {
    tex: TextureHandle,
    rect: Rect,
    used: f64,
}

/// The font atlas, copied when it has been started anew since, or when a
/// glyph a line needs is not in the copy yet (new glyphs often land in the
/// atlas's current row, which leaves its fill ratio as it was). Glyphs
/// never move while the atlas only grows, so more fill alone keeps it.
struct Atlas {
    size: [usize; 2],
    fill: f32,
    image: Arc<ColorImage>,
    /// The pass it was copied in (at most one copy a pass).
    pass: u64,
}

/// One bitmap to make: the line's glyph quads ([`quads`]) out of `atlas`.
struct Job {
    key: Key,
    quads: Vec<(Rect, Rect)>,
    atlas: Arc<ColorImage>,
    ppp: f32,
    sigma: f32,
}

/// What the threads share with the frames: the asks (newest last) and the
/// bitmaps made (`None`: one that cannot be made).
#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    done: Vec<(Key, Option<(ColorImage, Rect)>)>,
    closed: bool,
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[derive(Default)]
enum Workers {
    #[default]
    NotStarted,
    Running(Arc<Shared>),
    /// No thread would start: bitmaps are made in the frame, a few at a
    /// time.
    Unavailable,
}

#[derive(Default)]
struct Cache {
    atlas: Option<Atlas>,
    items: HashMap<Key, Blurred>,
    /// Asked for and not made yet.
    pending: HashSet<Key>,
    /// Could not be made (too large): not asked for again until the next
    /// sweep.
    failed: HashSet<Key>,
    workers: Workers,
    pass: u64,
    /// Bitmaps uploaded (or made, without threads) this pass.
    made: usize,
    swept: f64,
}

impl Drop for Cache {
    fn drop(&mut self) {
        if let Workers::Running(shared) = &self.workers {
            shared.lock().closed = true;
            shared.ready.notify_all();
        }
    }
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
    let mut c = cache.lock().unwrap_or_else(PoisonError::into_inner);
    let (now, pass) = (ctx.input(|i| i.time), ctx.cumulative_pass_nr());
    if c.pass != pass {
        c.pass = pass;
        c.made = 0;
        if now - c.swept > 1.0 {
            c.swept = now;
            c.items.retain(|_, b| now - b.used < KEEP_S);
            c.failed.clear();
        }
    }
    c.collect(ctx, now);
    let wanted = (key, step);
    if !c.items.contains_key(&wanted) && !c.pending.contains(&wanted) && !c.failed.contains(&wanted) {
        c.ask(ctx, g, wanted, ppp, now);
    }
    // The step asked for, else the nearest one made, else the line as it is.
    let near = (0..=STEPS).flat_map(|d| [step.saturating_sub(d), step.saturating_add(d).min(STEPS)]).find(|s| *s > 0 && c.items.contains_key(&(key, *s)));
    if !c.pending.is_empty() {
        // Shown once it is made, even with nothing else moving.
        ctx.request_repaint_after(Duration::from_millis(4));
    }

    match near.and_then(|s| c.items.get_mut(&(key, s))) {
        Some(b) => {
            b.used = now;
            let mut mesh = Mesh::with_texture(b.tex.id());
            mesh.add_rect_with_uv(b.rect.translate(pos.to_vec2()), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), color);
            painter.add(egui::Shape::mesh(mesh));
        }
        // Not made yet: the sharp line, as faint as its blur would spread
        // it, so a blurred line never flashes sharp.
        None => {
            let faint = 1.0 - 0.7 * f32::from(step) / f32::from(STEPS);
            painter.galley_with_override_text_color(pos, g.clone(), color.gamma_multiply(faint));
        }
    }
}

impl Cache {
    /// Uploads the newest bitmaps the threads have made, up to this pass's
    /// budget (the rest wait for the next frames).
    fn collect(&mut self, ctx: &Context, now: f64) {
        let Workers::Running(shared) = &self.workers else { return };
        let room = UPLOADS.saturating_sub(self.made);
        if room == 0 {
            return;
        }
        let done = {
            let mut q = shared.lock();
            let n = q.done.len();
            q.done.split_off(n.saturating_sub(room))
        };
        for (key, made) in done {
            self.pending.remove(&key);
            self.insert(ctx, key, made, now);
        }
    }

    fn insert(&mut self, ctx: &Context, key: Key, made: Option<(ColorImage, Rect)>, now: f64) {
        match made {
            Some((image, rect)) => {
                self.made += 1;
                let tex = ctx.load_texture("md3e-lyrics-blur", image, TextureOptions::LINEAR);
                self.items.insert(key, Blurred { tex, rect, used: now });
            }
            None => {
                self.failed.insert(key);
            }
        }
    }

    /// Asks for the bitmap of `g` at `key`'s step: queued for the threads
    /// (made here without them, within the frame's budget).
    fn ask(&mut self, ctx: &Context, g: &Galley, key: Key, ppp: f32, now: f64) {
        if matches!(self.workers, Workers::NotStarted) {
            self.workers = start().map_or(Workers::Unavailable, Workers::Running);
        }
        if !matches!(self.workers, Workers::Running(_)) && self.made >= PER_FRAME {
            ctx.request_repaint();
            return;
        }
        let quads = quads(g);
        // A glyph still missing from this pass's copy: drawn sharp this
        // time, blurred next pass.
        let Some(atlas) = self.atlas_for(ctx, &quads) else {
            ctx.request_repaint();
            return;
        };
        let sigma = f32::from(key.1) * STEP;
        if let Workers::Running(shared) = &self.workers {
            let mut q = shared.lock();
            q.jobs.push_back(Job { key, quads, atlas, ppp, sigma });
            self.pending.insert(key);
            while q.jobs.len() > QUEUE {
                if let Some(old) = q.jobs.pop_front() {
                    self.pending.remove(&old.key);
                }
            }
            drop(q);
            shared.ready.notify_one();
        } else {
            let made = raster(&quads, ppp, &atlas, sigma);
            self.insert(ctx, key, made, now);
        }
    }

    /// The atlas copy that has every glyph of `quads`, copied anew when it
    /// is stale (at most once a pass); `None` while one is still missing.
    fn atlas_for(&mut self, ctx: &Context, quads: &[(Rect, Rect)]) -> Option<Arc<ColorImage>> {
        let (size, fill) = ctx.fonts(|f| (f.font_image_size(), f.font_atlas_fill_ratio()));
        let pass = self.pass;
        let stale = |a: &Atlas| a.size != size || fill < a.fill || quads.iter().any(|(_, uv)| !inked(a, uv));
        if self.atlas.as_ref().is_none_or(|a| a.pass != pass && stale(a)) {
            let image = ctx.fonts(|f| f.image());
            self.atlas = Some(Atlas { size: image.size, fill, image: Arc::new(image), pass });
        }
        let atlas = self.atlas.as_ref()?;
        if quads.iter().any(|(_, uv)| !inked(atlas, uv)) {
            return None;
        }
        Some(atlas.image.clone())
    }
}

/// Starts the threads that make bitmaps; `None` when none would start.
fn start() -> Option<Arc<Shared>> {
    let shared = Arc::new(Shared::default());
    let started = (0..WORKERS)
        .filter(|i| {
            let s = shared.clone();
            std::thread::Builder::new().name(format!("lyrics-blur-{i}")).spawn(move || work(&s)).is_ok()
        })
        .count();
    (started > 0).then_some(shared)
}

/// One thread: makes the newest ask, until the cache is gone.
fn work(shared: &Shared) {
    loop {
        let job = {
            let mut q = shared.lock();
            loop {
                if q.closed {
                    return;
                }
                if let Some(job) = q.jobs.pop_back() {
                    break job;
                }
                q = shared.ready.wait(q).unwrap_or_else(PoisonError::into_inner);
            }
        };
        let made = raster(&job.quads, job.ppp, &job.atlas, job.sigma);
        let mut q = shared.lock();
        if q.closed {
            return;
        }
        q.done.push((job.key, made));
    }
}

/// Whether the atlas copy has any ink inside `uv` (texels): a glyph that
/// is there always has some.
fn inked(a: &Atlas, uv: &Rect) -> bool {
    let w = a.size[0];
    let (x0, y0) = (uv.left().max(0.0) as usize, uv.top().max(0.0) as usize);
    let (x1, y1) = ((uv.right().ceil() as usize).min(w), (uv.bottom().ceil() as usize).min(a.size[1]));
    (y0..y1).any(|y| a.image.pixels[y * w + x0.min(x1)..y * w + x1].iter().any(|p| p.a() > 0))
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

/// Draws the glyph `quads` at `ppp` pixels a point from the atlas's
/// coverage (its alpha), blurs them by `sigma` px and gives the bitmap with
/// where it lies (points). A wide blur is made at a lower resolution (the
/// texture is drawn smoothed), which keeps it cheap.
fn raster(quads: &[(Rect, Rect)], ppp: f32, atlas: &ColorImage, sigma: f32) -> Option<(ColorImage, Rect)> {
    let size = atlas.size;
    let bounds = quads.iter().map(|q| q.0).reduce(|a, b| a.union(b))?;
    let down = ((sigma / 2.0).floor() as usize).clamp(1, 6);
    let pad = ((3.0 * sigma).ceil() as usize + 1).next_multiple_of(down);
    let w = ((bounds.width() * ppp).ceil() as usize + 2 * pad).next_multiple_of(down);
    let h = ((bounds.height() * ppp).ceil() as usize + 2 * pad).next_multiple_of(down);
    if w * h > 4096 * 512 {
        return None;
    }
    let mut cover = vec![0.0f32; w * h];
    for (at, uv) in quads {
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
                let a = f32::from(atlas.pixels[ty * size[0] + tx].a()) / 255.0;
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

    /// Bitmaps asked of the threads come back made.
    #[test]
    fn asks_come_back_made_by_the_threads() {
        let shared = start().expect("a thread starts");
        let mut atlas = ColorImage::new([8, 8], vec![Color32::TRANSPARENT; 64]);
        for y in 2..6 {
            for x in 2..6 {
                atlas.pixels[y * 8 + x] = Color32::WHITE;
            }
        }
        let atlas = Arc::new(atlas);
        let quad = (Rect::from_min_size(pos2(0.0, 0.0), vec2(4.0, 4.0)), Rect::from_min_max(pos2(2.0, 2.0), pos2(6.0, 6.0)));
        {
            let mut q = shared.lock();
            for step in 1..=3u8 {
                q.jobs.push_back(Job { key: (7, step), quads: vec![quad], atlas: atlas.clone(), ppp: 1.0, sigma: f32::from(step) * STEP });
            }
        }
        shared.ready.notify_all();
        let started = std::time::Instant::now();
        while shared.lock().done.len() < 3 {
            assert!(started.elapsed() < Duration::from_secs(10), "made in time");
            std::thread::sleep(Duration::from_millis(1));
        }
        let done = std::mem::take(&mut shared.lock().done);
        assert!(done.iter().all(|(_, made)| made.as_ref().is_some_and(|(image, _)| image.pixels.iter().any(|p| p.a() > 0))));
        shared.lock().closed = true;
        shared.ready.notify_all();
    }

    #[test]
    fn shrinking_averages_blocks() {
        let v = vec![1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
        let (s, w, h) = shrink(&v, 4, 2, 2);
        assert_eq!((w, h), (2, 1));
        // Blocks {1, 0, 1, 1} and {0, 0, 1, 1}.
        assert_eq!(s, vec![0.75, 0.5]);
    }
}
