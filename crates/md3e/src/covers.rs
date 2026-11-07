//! Album covers: a loader that decodes `file://` JPEG/PNG covers and web
//! covers on a few worker threads, downscaled to the size they are drawn at,
//! and an LRU that forgets the textures of covers that have scrolled away.
//!
//! Draw covers through [`texture`]. It picks a size tier (`#128` to `#2048`
//! after the URI, or the full file), marks the cover as used in this
//! pass and, while the wanted tier decodes, shows a tier of the same file that
//! is already loaded. At the end of every pass the covers that were not drawn
//! in this or the previous pass and are not [pinned](pin) are forgotten,
//! oldest first, until the textures fit the [budget](DEFAULT_BUDGET). The
//! loader hands every decoded image to egui's texture loader and keeps no
//! copy, and it reads files itself, so a cover costs only its texture.
//!
//! Web covers (`https://`) are fetched by the app's bytes loader; with a
//! [web sizer](set_web_sizer) the url is asked at the size it is drawn at.
//! Their bytes are decoded here like files, at the same tiers (a 640 px
//! Spotify cover drawn on a card becomes a 128 or 256 px texture), and are
//! dropped from the bytes loader once decoded, so a web cover too costs only
//! its texture; they are forgotten off screen like files. A web cover has no
//! palette seed from an import, so its seed is taken from the pixels it is
//! decoded to ([`web_seed`]).
//!
//! A cover inside black bars (a video's picture of a song uploaded with its
//! art) is decoded without them (`nikgob_core::cover_frame`).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use egui::load::{
    BytesPoll, ImageLoadResult, ImageLoader, ImagePoll, LoadError, SizeHint, SizedTexture, TexturePoll,
};
use egui::{ColorImage, Context, Id, Vec2};

/// Texture bytes all covers may take before off-screen ones are forgotten.
pub const DEFAULT_BUDGET: usize = 64 << 20;

/// Longest sides of the downscaled tiers, in pixels; larger draws get the
/// file (at most [`FULL`]). A big Now Playing cover drawn from a 3000 px
/// file kept 48 MB of texture for 600 px on screen; 1024 is enough for it.
const TIERS: [u32; 5] = [128, 256, 512, 1024, 2048];

/// The longest side a cover is decoded at, even drawn larger.
const FULL: u32 = 4096;

/// Decoding threads at most.
const WORKERS: usize = 3;

enum Slot {
    Pending,
    Ready(Arc<ColorImage>),
    Failed(String),
}

/// Where a cover's bytes come from.
enum Source {
    File(PathBuf),
    /// A web cover's url (its bytes loader's key) and the fetched bytes.
    Web(String, egui::load::Bytes),
}

struct Job {
    uri: String,
    source: Source,
    tier: Option<u32>,
}

#[derive(Default)]
struct Jobs {
    queue: Vec<Job>,
    workers: usize,
}

struct Use {
    /// The cover this is a size of: the file URI without its tier, or the
    /// web url before sizing.
    origin: String,
    /// The tier it is (`None`: the full file, or a web url as it is).
    tier: Option<u32>,
    /// Pass it was last drawn in.
    last: u64,
    /// Texture bytes; 0 while decoding.
    bytes: usize,
}

struct Lru {
    used: HashMap<String, Use>,
    /// Files (URIs without a tier) kept loaded off screen.
    pinned: HashSet<String>,
    budget: usize,
}

/// The url of a web cover for drawing at a tier (`None`: full size), or
/// `None` to draw the url as it is.
pub type WebSizer = fn(&str, Option<u32>) -> Option<String>;

struct Shared {
    slots: Mutex<HashMap<String, Slot>>,
    /// The colour-scheme seed of each web cover decoded so far, by its url
    /// (`None`: too grey for one).
    web_seeds: Mutex<HashMap<String, Option<u32>>>,
    web_sizer: Mutex<Option<WebSizer>>,
    jobs: Mutex<Jobs>,
    lru: Mutex<Lru>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The egui image loader for covers; [`install`] registers it.
pub struct CoverLoader(Arc<Shared>);

impl CoverLoader {
    pub const ID: &'static str = egui::generate_loader_id!(CoverLoader);
}

/// Handle kept in the context's temp data so painters find the LRU.
#[derive(Clone)]
struct Covers(Arc<Shared>);

fn data_id() -> Id {
    Id::new("md3e-covers")
}

/// Registers the cover loader (ahead of egui_extras' image loader) and the
/// end-of-pass eviction. Calling it again is harmless.
pub fn install(ctx: &Context) {
    if ctx.is_loader_installed(CoverLoader::ID) {
        return;
    }
    let shared = Arc::new(Shared {
        slots: Mutex::default(),
        web_seeds: Mutex::default(),
        web_sizer: Mutex::default(),
        jobs: Mutex::default(),
        lru: Mutex::new(Lru {
            used: HashMap::new(),
            pinned: HashSet::new(),
            budget: DEFAULT_BUDGET,
        }),
    });
    ctx.add_image_loader(Arc::new(CoverLoader(Arc::clone(&shared))));
    ctx.data_mut(|d| d.insert_temp(data_id(), Covers(Arc::clone(&shared))));
    ctx.on_end_pass("md3e-covers", Arc::new(move |ui| shared.evict(ui.ctx())));
}

fn shared(ctx: &Context) -> Option<Arc<Shared>> {
    ctx.data(|d| d.get_temp::<Covers>(data_id())).map(|c| c.0)
}

/// Sets how many texture bytes covers may take (see [`DEFAULT_BUDGET`]).
pub fn set_budget(ctx: &Context, bytes: usize) {
    if let Some(s) = shared(ctx) {
        lock(&s.lru).budget = bytes;
    }
}

/// How web cover urls are sized for the tier they are drawn at (the app
/// knows which services' urls can be resized).
pub fn set_web_sizer(ctx: &Context, sizer: WebSizer) {
    install(ctx);
    if let Some(s) = shared(ctx) {
        *lock(&s.web_sizer) = Some(sizer);
    }
}

/// Keeps every tier of these covers loaded while off screen (the current and
/// the next track). Replaces the previous set.
pub fn pin<'a>(ctx: &Context, uris: impl IntoIterator<Item = &'a str>) {
    if let Some(s) = shared(ctx) {
        lock(&s.lru).pinned = uris.into_iter().map(|u| base(u).to_owned()).collect();
    }
}

/// The colour-scheme seed of the web cover at `url`, once a size of it has
/// been decoded; `None` before, or for a cover too grey for one.
pub fn web_seed(ctx: &Context, url: &str) -> Option<u32> {
    let shared = shared(ctx)?;
    lock(&shared.web_seeds).get(url).copied().flatten()
}

fn load(ctx: &Context, uri: &str) -> Result<Option<SizedTexture>, LoadError> {
    match ctx.try_load_texture(uri, crate::components::PHOTO_TEXTURE, SizeHint::default()) {
        Ok(TexturePoll::Ready { texture }) => Ok(Some(texture)),
        Ok(TexturePoll::Pending { .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

/// The URI of `uri` at `tier`, and the cover it is a size of. A web cover's
/// is its (sized) url with the tier after it, like a file's.
fn sized(shared: &Shared, uri: &str, tier: Option<u32>) -> (String, String) {
    if is_web(uri) {
        let sizer = *lock(&shared.web_sizer);
        let url = sizer.and_then(|f| f(uri, tier)).unwrap_or_else(|| uri.to_owned());
        let wanted = match tier {
            Some(t) => format!("{url}#{t}"),
            None => url,
        };
        (wanted, uri.to_owned())
    } else {
        let origin = base(uri);
        let wanted = match tier {
            Some(t) => format!("{origin}#{t}"),
            None => origin.to_owned(),
        };
        (wanted, origin.to_owned())
    }
}

/// The cover at `uri` for drawing into `size` points, magnified `zoom` times.
/// `None` until it loads (or if it cannot be).
pub fn texture(ctx: &Context, uri: &str, size: Vec2, zoom: f32) -> Option<SizedTexture> {
    let shared = match shared(ctx) {
        Some(s) if parse(uri).is_some() => s,
        _ => return load(ctx, uri).ok().flatten(),
    };
    let px = size.max_elem() * ctx.pixels_per_point() * zoom.max(1.0);
    let tier = TIERS.iter().find(|t| px <= **t as f32).copied();
    let (wanted, origin) = sized(&shared, uri, tier);
    let pass = ctx.cumulative_pass_nr();
    match load(ctx, &wanted) {
        Ok(Some(texture)) => {
            shared.used(&wanted, &origin, tier, pass, texture_bytes(texture.size));
            Some(texture)
        }
        Ok(None) => {
            shared.used(&wanted, &origin, tier, pass, 0);
            // Another size of the cover meanwhile, so a growing cover does
            // not blink.
            let (other, other_tier) = shared.loaded_tier(&origin)?;
            let texture = load(ctx, &other).ok().flatten()?;
            shared.used(&other, &origin, other_tier, pass, texture_bytes(texture.size));
            Some(texture)
        }
        Err(_) => {
            // Kept while drawn so a broken file is not decoded every pass.
            shared.used(&wanted, &origin, tier, pass, 0);
            None
        }
    }
}

/// Loads `next` at every size `current` is drawn at now (the next track's
/// cover in the player bar and Now Playing), so it is decoded and on the
/// GPU before its track starts, not in the frame the cover morph begins.
pub fn prefetch_like(ctx: &Context, current: &str, next: &str) {
    let Some(shared) = shared(ctx) else { return };
    if base(current) == base(next) || parse(next).is_none() {
        return;
    }
    let pass = ctx.cumulative_pass_nr();
    let (_, origin) = sized(&shared, current, None);
    let tiers: Vec<Option<u32>> = {
        let lru = lock(&shared.lru);
        let mut tiers: Vec<Option<u32>> = lru
            .used
            .values()
            .filter(|u| u.origin == origin && u.last + 1 >= pass)
            .map(|u| u.tier)
            .collect();
        tiers.sort_unstable();
        tiers.dedup();
        tiers
    };
    for tier in tiers {
        let (wanted, origin) = sized(&shared, next, tier);
        let bytes = match load(ctx, &wanted) {
            Ok(Some(texture)) => texture_bytes(texture.size),
            _ => 0,
        };
        shared.used(&wanted, &origin, tier, pass, bytes);
    }
}

fn is_web(uri: &str) -> bool {
    uri.starts_with("https://") || uri.starts_with("http://")
}

/// The URI without its tier suffix.
fn base(uri: &str) -> &str {
    match uri.rsplit_once('#') {
        Some((b, t)) if !t.is_empty() && t.bytes().all(|c| c.is_ascii_digit()) => b,
        _ => uri,
    }
}

/// RGBA plus a third for mipmaps.
fn texture_bytes(size: Vec2) -> usize {
    (size.x.max(1.0) * size.y.max(1.0) * 4.0 * 4.0 / 3.0) as usize
}

/// What a cover URI is: a file or a web url.
enum Parsed {
    File(PathBuf),
    Web(String),
}

/// What a cover URI is and its tier; `None` for URIs this loader leaves to
/// others.
fn parse(uri: &str) -> Option<(Parsed, Option<u32>)> {
    let b = base(uri);
    let tier = if b.len() < uri.len() { uri[b.len() + 1..].parse().ok() } else { None };
    if is_web(b) {
        return Some((Parsed::Web(b.to_owned()), tier));
    }
    let path = b.strip_prefix("file://")?;
    let ext = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    if !matches!(ext.as_str(), "jpg" | "jpeg" | "png") {
        return None;
    }
    // Like egui_extras' file loader: `file:///C:/x` on Windows, else UNC.
    let path = if cfg!(windows) {
        match path.strip_prefix('/') {
            Some(p) => PathBuf::from(p),
            None => PathBuf::from(format!("\\\\{path}")),
        }
    } else {
        PathBuf::from(path)
    };
    Some((Parsed::File(path), tier))
}

fn decode(source: &Source, tier: Option<u32>) -> Result<ColorImage, String> {
    let file;
    let bytes: &[u8] = match source {
        Source::File(path) => {
            file = std::fs::read(path).map_err(|e| e.to_string())?;
            &file
        }
        Source::Web(_, bytes) => bytes,
    };
    let side = tier.unwrap_or(FULL);
    let img = match scaled_jpeg(bytes, side) {
        Some(img) => img,
        None => image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|e| e.to_string())?
            .decode()
            .map_err(|e| e.to_string())?,
    };
    let img = if img.width().max(img.height()) > side { img.thumbnail(side, side) } else { img };
    let mut rgba = img.to_rgba8();
    // The art inside a video's black bars (a song uploaded with its cover,
    // a letterboxed thumbnail): the bars are not the cover.
    if let Some((x, y, w, h)) = nikgob_core::cover_frame::content(rgba.width(), rgba.height(), rgba.as_raw()) {
        rgba = image::imageops::crop_imm(&rgba, x, y, w, h).to_image();
    }
    Ok(ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    ))
}

/// A JPEG at least twice as large as `side` decoded at 1/2, 1/4 or 1/8 of its
/// size, the smallest that is still at least `side` on its long side (the
/// scaling happens in the DCT, so a 3000 px cover for a 128 px tile never
/// takes 36 MB of pixels). `None`: not such a JPEG, or one this decoder
/// cannot scale (CMYK, 12-bit, anything it rejects); it is decoded whole.
fn scaled_jpeg(bytes: &[u8], side: u32) -> Option<image::DynamicImage> {
    if !bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return None;
    }
    let mut d = jpeg_decoder::Decoder::new(bytes);
    d.read_info().ok()?;
    let info = d.info()?;
    let long = u32::from(info.width.max(info.height));
    if long < side * 2 || !matches!(info.pixel_format, jpeg_decoder::PixelFormat::RGB24 | jpeg_decoder::PixelFormat::L8) {
        return None;
    }
    let req = u16::try_from(side).ok()?;
    let (w, h) = d.scale(req, req).ok()?;
    let pixels = d.decode().ok()?;
    let (w, h) = (u32::from(w), u32::from(h));
    match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => image::RgbImage::from_raw(w, h, pixels).map(Into::into),
        _ => image::GrayImage::from_raw(w, h, pixels).map(Into::into),
    }
}

impl Shared {
    fn used(&self, uri: &str, origin: &str, tier: Option<u32>, pass: u64, bytes: usize) {
        let mut lru = lock(&self.lru);
        let u = lru
            .used
            .entry(uri.to_owned())
            .or_insert_with(|| Use { origin: origin.to_owned(), tier, last: pass, bytes });
        u.last = pass;
        u.bytes = bytes;
    }

    /// The largest loaded size of the cover `origin`, and its tier.
    fn loaded_tier(&self, origin: &str) -> Option<(String, Option<u32>)> {
        let lru = lock(&self.lru);
        lru.used
            .iter()
            .filter(|(_, u)| u.bytes > 0 && u.origin == origin)
            .max_by_key(|(_, u)| u.bytes)
            .map(|(uri, u)| (uri.clone(), u.tier))
    }

    fn enqueue(self: &Arc<Self>, ctx: &Context, job: Job) {
        let mut jobs = lock(&self.jobs);
        jobs.queue.push(job);
        if jobs.workers < WORKERS {
            jobs.workers += 1;
            let (me, ctx) = (Arc::clone(self), ctx.clone());
            let spawned = std::thread::Builder::new()
                .name("md3e-covers".into())
                .spawn(move || me.work(&ctx));
            if spawned.is_err() {
                jobs.workers -= 1;
            }
        }
    }

    /// Decodes queued covers, newest first (what was just scrolled to),
    /// until the queue is empty.
    fn work(&self, ctx: &Context) {
        loop {
            let job = {
                let mut jobs = lock(&self.jobs);
                match jobs.queue.pop() {
                    Some(j) => j,
                    None => {
                        jobs.workers -= 1;
                        return;
                    }
                }
            };
            // Forgotten (scrolled away) before its turn.
            if !matches!(lock(&self.slots).get(&job.uri), Some(Slot::Pending)) {
                continue;
            }
            let slot = match decode(&job.source, job.tier) {
                Ok(img) => Slot::Ready(Arc::new(img)),
                Err(e) => Slot::Failed(e),
            };
            if let (Source::Web(..), Slot::Ready(img)) = (&job.source, &slot) {
                self.seed_web(&job.uri, img);
            }
            // The texture is all a web cover needs from now on; another
            // tier reads it again from the disk cache.
            if let Source::Web(url, _) = &job.source {
                for l in ctx.loaders().bytes.lock().iter() {
                    l.forget(url);
                }
            }
            let mut slots = lock(&self.slots);
            if let Some(s @ Slot::Pending) = slots.get_mut(&job.uri) {
                *s = slot;
                drop(slots);
                ctx.request_repaint();
            }
        }
    }

    /// Keeps the seed of the web cover a decoded size `uri` is of: its first
    /// size decoded is enough, the seed is taken from 128 px anyway.
    fn seed_web(&self, uri: &str, img: &ColorImage) {
        let origin = lock(&self.lru).used.get(uri).map_or_else(|| base(uri).to_owned(), |u| u.origin.clone());
        let mut seeds = lock(&self.web_seeds);
        if !seeds.contains_key(&origin) {
            let seed = crate::color::seed_from_image(img.as_raw(), img.width(), img.height());
            seeds.insert(origin, seed);
        }
    }

    /// Cancels covers no longer drawn that are still decoding, then forgets
    /// the least recently drawn ones off screen until the budget fits.
    fn evict(&self, ctx: &Context) {
        let pass = ctx.cumulative_pass_nr();
        let shown = |u: &Use| u.last + 1 >= pass;
        let mut forget = Vec::new();
        {
            let mut lru = lock(&self.lru);
            let Lru { used, pinned, budget } = &mut *lru;
            used.retain(|uri, u| {
                let keep = u.bytes > 0 || shown(u);
                if !keep {
                    forget.push(uri.clone());
                }
                keep
            });
            let mut total: usize = used.values().map(|u| u.bytes).sum();
            if total > *budget {
                let mut old: Vec<(u64, String, usize)> = used
                    .iter()
                    .filter(|(_, u)| !shown(u) && !pinned.contains(&u.origin))
                    .map(|(uri, u)| (u.last, uri.clone(), u.bytes))
                    .collect();
                old.sort_unstable();
                for (_, uri, bytes) in old {
                    if total <= *budget {
                        break;
                    }
                    total -= bytes;
                    used.remove(&uri);
                    forget.push(uri);
                }
            }
        }
        // A web cover none of whose tiers is kept any more also leaves the
        // bytes loader (and its download queue if it still waits there).
        let mut urls: Vec<&str> =
            forget.iter().filter(|u| is_web(u) && base(u) != u.as_str()).map(|u| base(u)).collect();
        if !urls.is_empty() {
            let lru = lock(&self.lru);
            urls.retain(|url| !lru.used.keys().any(|k| base(k) == *url));
            urls.sort_unstable();
            urls.dedup();
        }
        for uri in &forget {
            ctx.forget_image(uri);
        }
        if !urls.is_empty() {
            for l in ctx.loaders().bytes.lock().iter() {
                for url in &urls {
                    l.forget(url);
                }
            }
        }
    }
}

impl ImageLoader for CoverLoader {
    fn id(&self) -> &str {
        Self::ID
    }

    fn load(&self, ctx: &Context, uri: &str, _: SizeHint) -> ImageLoadResult {
        let Some((parsed, tier)) = parse(uri) else {
            return Err(LoadError::NotSupported);
        };
        let mut slots = lock(&self.0.slots);
        match slots.remove(uri) {
            // The texture loader keeps the texture; no copy stays here.
            Some(Slot::Ready(image)) => Ok(ImagePoll::Ready { image }),
            Some(slot) => {
                let r = match &slot {
                    Slot::Failed(e) => Err(LoadError::Loading(e.clone())),
                    _ => Ok(ImagePoll::Pending { size: None }),
                };
                slots.insert(uri.to_owned(), slot);
                r
            }
            None => {
                let source = match parsed {
                    Parsed::File(path) => Source::File(path),
                    // Polled until the bytes are in; nothing is kept here
                    // meanwhile (the bytes loader remembers a failure).
                    Parsed::Web(url) => {
                        drop(slots);
                        let bytes = match ctx.try_load_bytes(&url)? {
                            BytesPoll::Ready { bytes, .. } => bytes,
                            BytesPoll::Pending { size } => return Ok(ImagePoll::Pending { size }),
                        };
                        slots = lock(&self.0.slots);
                        if slots.contains_key(uri) {
                            return Ok(ImagePoll::Pending { size: None });
                        }
                        Source::Web(url, bytes)
                    }
                };
                slots.insert(uri.to_owned(), Slot::Pending);
                drop(slots);
                self.0.enqueue(ctx, Job { uri: uri.to_owned(), source, tier });
                Ok(ImagePoll::Pending { size: None })
            }
        }
    }

    fn forget(&self, uri: &str) {
        lock(&self.0.slots).remove(uri);
    }

    fn forget_all(&self) {
        lock(&self.0.slots).clear();
    }

    fn byte_size(&self) -> usize {
        lock(&self.0.slots)
            .values()
            .map(|s| match s {
                Slot::Ready(img) => img.pixels.len() * 4,
                Slot::Failed(e) => e.len(),
                Slot::Pending => 0,
            })
            .sum()
    }

    fn has_pending(&self) -> bool {
        lock(&self.0.slots).values().any(|s| matches!(s, Slot::Pending))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use egui::vec2;

    use super::*;

    fn ctx() -> Context {
        let ctx = Context::default();
        install(&ctx);
        ctx
    }

    #[test]
    fn a_web_cover_is_seeded_by_its_pixels() {
        let ctx = ctx();
        let red = ColorImage::new([64, 64], vec![egui::Color32::from_rgb(200, 30, 30); 64 * 64]);
        shared(&ctx).expect("installed").seed_web("https://x.invalid/c.jpg#128", &red);
        assert!(web_seed(&ctx, "https://x.invalid/c.jpg").is_some());
        assert_eq!(web_seed(&ctx, "https://x.invalid/other.jpg"), None);
    }

    /// `n` solid PNG covers of `side` pixels in a fresh temp folder.
    fn covers(name: &str, n: usize, side: u32) -> Vec<String> {
        let dir = std::env::temp_dir().join(format!("md3e-covers-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (0..n)
            .map(|i| {
                let p = dir.join(format!("{i}.png"));
                image::RgbImage::from_pixel(side, side, image::Rgb([i as u8, 0, 0])).save(&p).unwrap();
                nikgob_view::file_uri(&p)
            })
            .collect()
    }

    /// One pass drawing `uris` into squares of `size` points.
    fn pass(ctx: &Context, uris: &[String], size: f32) -> Vec<Option<SizedTexture>> {
        let mut got = Vec::new();
        ctx.run_ui(egui::RawInput::default(), |ui| {
            got = uris.iter().map(|u| texture(ui.ctx(), u, vec2(size, size), 1.0)).collect();
        })
        .drop_without_applying_deltas();
        got
    }

    /// Passes until every cover in `uris` is drawn.
    fn show(ctx: &Context, uris: &[String], size: f32) -> Vec<SizedTexture> {
        for _ in 0..5000 {
            let got = pass(ctx, uris, size);
            if got.iter().all(Option::is_some) {
                return got.into_iter().flatten().collect();
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("covers did not load");
    }

    fn textures(ctx: &Context) -> usize {
        ctx.loaders().texture.lock().iter().map(|l| l.byte_size()).sum()
    }

    fn decoded(ctx: &Context) -> usize {
        let l = ctx.loaders();
        let images: usize = l.image.lock().iter().map(|l| l.byte_size()).sum();
        let bytes: usize = l.bytes.lock().iter().map(|l| l.byte_size()).sum();
        images + bytes
    }

    fn loaded(ctx: &Context) -> Vec<String> {
        let s = shared(ctx).unwrap();
        let lru = lock(&s.lru);
        lru.used.iter().filter(|(_, u)| u.bytes > 0).map(|(k, _)| k.clone()).collect()
    }

    #[test]
    fn uris_parse_with_and_without_a_tier() {
        let (p, t) = parse("file:///x/a/cover.jpg#256").unwrap();
        assert!(matches!(p, Parsed::File(p) if p.ends_with("x/a/cover.jpg")));
        assert_eq!(t, Some(256));
        assert_eq!(parse("file:///x/a/cover.PNG").unwrap().1, None);
        assert!(parse("file:///x/a/icon.svg").is_none());
        assert!(parse("bytes://cover.jpg").is_none());
        let (w, t) = parse("https://i.scdn.co/image/ab67#128").unwrap();
        assert!(matches!(w, Parsed::Web(u) if u == "https://i.scdn.co/image/ab67"));
        assert_eq!(t, Some(128));
        assert_eq!(base("file:///x/a#b/cover.jpg"), "file:///x/a#b/cover.jpg");
        assert_eq!(base("file:///x/cover.jpg#512"), "file:///x/cover.jpg");
    }

    #[test]
    fn covers_decode_at_the_drawn_size() {
        let ctx = ctx();
        let uris = covers("size", 1, 600);
        // Smaller tiers stand in while the next one decodes: wait until
        // nothing is left in the loader.
        let settled = |size: f32| {
            for _ in 0..5000 {
                let t = pass(&ctx, &uris, size)[0];
                if let Some(t) = t
                    && lock(&shared(&ctx).unwrap().slots).is_empty()
                {
                    return t.size;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            panic!("cover did not settle");
        };
        assert_eq!(settled(48.0), vec2(128.0, 128.0));
        assert_eq!(settled(200.0), vec2(256.0, 256.0));
        assert_eq!(settled(400.0), vec2(512.0, 512.0));
        assert_eq!(settled(560.0), vec2(600.0, 600.0));
        // Only textures remain: no file bytes, no decoded copies.
        assert_eq!(decoded(&ctx), 0);
    }

    #[test]
    fn the_art_in_a_pillarboxed_frame_decodes_without_its_bars() {
        let dir = std::env::temp_dir().join(format!("md3e-covers-bars-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("frame.png");
        let img = image::RgbImage::from_fn(640, 360, |x, _| if (140..500).contains(&x) { image::Rgb([200, 40, 40]) } else { image::Rgb([0, 0, 0]) });
        img.save(&path).unwrap();
        let decoded = decode(&Source::File(path), Some(512)).unwrap();
        assert_eq!(decoded.size, [288, 288], "640 × 360 drawn at 512 is 512 × 288 with its bars");
        assert!(decoded.pixels.iter().all(|p| p.r() > 150));
    }

    #[test]
    fn large_jpegs_decode_scaled_to_the_same_picture() {
        let dir = std::env::temp_dir().join(format!("md3e-covers-jpeg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("big.jpg");
        let img = image::RgbImage::from_fn(3000, 2400, |x, y| {
            image::Rgb([(x * 255 / 3000) as u8, (y * 255 / 2400) as u8, ((x + y) % 256) as u8 / 4 + 100])
        });
        img.save(&path).unwrap();
        let source = Source::File(path.clone());
        for side in [128, 256, 512, 1024] {
            let got = decode(&source, Some(side)).unwrap();
            let full = image::open(&path).unwrap().thumbnail(side, side).to_rgba8();
            assert_eq!(got.size, [full.width() as usize, full.height() as usize]);
            let diff: u64 = got
                .pixels
                .iter()
                .zip(full.pixels())
                .map(|(a, b)| (0..3).map(|i| u64::from(a.to_array()[i].abs_diff(b.0[i]))).sum::<u64>())
                .sum();
            let mean = diff as f64 / (got.pixels.len() * 3) as f64;
            assert!(mean < 2.0, "{side}: mean difference {mean}");
        }
        // Not twice the tier: decoded whole as before.
        assert!(scaled_jpeg(&std::fs::read(&path).unwrap(), 2048).is_none());
    }

    #[test]
    fn the_next_cover_loads_at_the_sizes_the_current_one_is_drawn_at() {
        let ctx = ctx();
        let uris = covers("next", 2, 600);
        let (current, next) = (&uris[0], &uris[1]);
        // The current cover at two sizes (the bar and Now Playing).
        for _ in 0..5000 {
            let mut done = false;
            ctx.run_ui(egui::RawInput::default(), |ui| {
                let small = texture(ui.ctx(), current, vec2(48.0, 48.0), 1.0);
                let big = texture(ui.ctx(), current, vec2(400.0, 400.0), 1.0);
                prefetch_like(ui.ctx(), current, next);
                let l = loaded(ui.ctx());
                done = small.is_some_and(|t| t.size == vec2(128.0, 128.0))
                    && big.is_some_and(|t| t.size == vec2(512.0, 512.0))
                    && l.contains(&format!("{next}#128"))
                    && l.contains(&format!("{next}#512"));
            })
            .drop_without_applying_deltas();
            if done {
                // Drawn at once when its track starts, at either size.
                let got = pass(&ctx, &[next.clone()], 400.0);
                assert_eq!(got[0].map(|t| t.size), Some(vec2(512.0, 512.0)));
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("the next cover was not loaded ahead");
    }

    #[test]
    fn a_growing_cover_shows_a_loaded_tier_while_decoding() {
        let ctx = ctx();
        let uris = covers("grow", 1, 600);
        show(&ctx, &uris, 48.0);
        let first = pass(&ctx, &uris, 400.0);
        assert_eq!(first[0].map(|t| t.size), Some(vec2(128.0, 128.0)));
    }

    /// The bytes of web "covers": PNGs of the size their url says
    /// (`https://x/a=w226`, 640 px without one), ready at once unless the url
    /// ends in `slow`. Keeps what it handed out until it is forgotten, and
    /// counts what was forgotten.
    #[derive(Default)]
    struct WebStub {
        held: Mutex<HashMap<String, usize>>,
        forgotten: Mutex<Vec<String>>,
    }

    impl egui::load::BytesLoader for WebStub {
        fn id(&self) -> &str {
            "web-stub"
        }
        fn load(&self, _: &Context, uri: &str) -> egui::load::BytesLoadResult {
            if !is_web(uri) {
                return Err(LoadError::NotSupported);
            }
            if uri.ends_with("slow") {
                return Ok(BytesPoll::Pending { size: None });
            }
            let side: u32 = uri.rsplit_once("=w").and_then(|(_, n)| n.parse().ok()).unwrap_or(640);
            let mut png = std::io::Cursor::new(Vec::new());
            image::RgbImage::from_pixel(side, side, image::Rgb([200, 0, 0]))
                .write_to(&mut png, image::ImageFormat::Png)
                .unwrap();
            let png = png.into_inner();
            lock(&self.held).insert(uri.to_owned(), png.len());
            Ok(BytesPoll::Ready { size: None, bytes: egui::load::Bytes::Shared(png.into()), mime: None })
        }
        fn forget(&self, uri: &str) {
            lock(&self.held).remove(uri);
            lock(&self.forgotten).push(uri.to_owned());
        }
        fn forget_all(&self) {}
        fn byte_size(&self) -> usize {
            lock(&self.held).values().sum()
        }
    }

    #[test]
    fn web_covers_decode_at_the_drawn_size_and_are_forgotten_off_screen() {
        let ctx = ctx();
        let stub = Arc::new(WebStub::default());
        ctx.add_bytes_loader(stub.clone());
        // Resizable urls end in `=w…`; `slow` ones never arrive.
        set_web_sizer(&ctx, |url, tier| {
            let (b, _) = url.rsplit_once("=w")?;
            Some(match tier {
                Some(t) if t <= 256 => format!("{b}=w226"),
                _ => format!("{b}=w1200slow"),
            })
        });
        let yt = vec!["https://g/a=w544".to_owned()];
        // Asked at 226 px, decoded down to the 128 px tier.
        assert_eq!(show(&ctx, &yt, 48.0)[0].size, vec2(128.0, 128.0));
        // Bigger: the full size is pending, the small one stands in.
        let got = pass(&ctx, &yt, 700.0);
        assert_eq!(got[0].map(|t| t.size), Some(vec2(128.0, 128.0)));
        // A url the sizer leaves alone is asked as it is and decoded at the
        // tier too: a 640 px cover on a card is not a 640 px texture.
        let sp = vec!["https://spotify/abc".to_owned()];
        assert_eq!(show(&ctx, &sp, 48.0)[0].size, vec2(128.0, 128.0));
        let mut big = None;
        for _ in 0..5000 {
            big = pass(&ctx, &sp, 200.0)[0].map(|t| t.size);
            if big == Some(vec2(256.0, 256.0)) && lock(&shared(&ctx).unwrap().slots).is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(big, Some(vec2(256.0, 256.0)));
        // Once decoded, no copy of a cover is left: no bytes, no image.
        assert_eq!(decoded(&ctx), 0);
        assert!(lock(&stub.forgotten).iter().any(|u| u == "https://spotify/abc"));
        // The pending full size scrolled away meanwhile: its url is
        // forgotten too (it leaves the download queue).
        assert!(lock(&stub.forgotten).iter().any(|u| u == "https://g/a=w1200slow"));
    }

    #[test]
    fn scrolling_a_long_library_keeps_textures_bounded() {
        let ctx = ctx();
        let one = texture_bytes(vec2(128.0, 128.0));
        set_budget(&ctx, 10 * one);
        let uris = covers("scroll", 80, 300);
        pin(&ctx, [uris[0].as_str()]);
        show(&ctx, &uris[..1], 64.0);
        for start in 1..uris.len() - 4 {
            show(&ctx, &uris[start..start + 4], 64.0);
            assert!(loaded(&ctx).len() <= 10, "{} covers loaded", loaded(&ctx).len());
            assert!(textures(&ctx) <= 10 * 128 * 128 * 4);
        }
        let kept = loaded(&ctx);
        // The pinned cover and the visible ones stay.
        for u in std::iter::once(&uris[0]).chain(&uris[uris.len() - 5..uris.len() - 1]) {
            assert!(kept.contains(&format!("{u}#128")), "{u} was forgotten");
        }
        assert_eq!(decoded(&ctx), 0);
    }
}
