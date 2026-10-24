//! Album covers: a loader that decodes `file://` JPEG/PNG covers on a few
//! worker threads, downscaled to the size they are drawn at, and an LRU that
//! forgets the textures of covers that have scrolled away.
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
//! Web covers (`https://`) are loaded by the app's loader; with a
//! [web sizer](set_web_sizer) the url is asked at the size it is drawn at
//! (the same tiers), and they are forgotten off screen like files.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use egui::load::{
    ImageLoadResult, ImageLoader, ImagePoll, LoadError, SizeHint, SizedTexture, TexturePoll,
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

struct Job {
    uri: String,
    path: PathBuf,
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

/// The cover at `uri` for drawing into `size` points, magnified `zoom` times.
/// `None` until it loads (or if it cannot be).
pub fn texture(ctx: &Context, uri: &str, size: Vec2, zoom: f32) -> Option<SizedTexture> {
    let options = crate::components::PHOTO_TEXTURE;
    let load = |uri: &str| match ctx.try_load_texture(uri, options, SizeHint::default()) {
        Ok(TexturePoll::Ready { texture }) => Ok(Some(texture)),
        Ok(TexturePoll::Pending { .. }) => Ok(None),
        Err(e) => Err(e),
    };
    let web = is_web(uri);
    let shared = match shared(ctx) {
        Some(s) if web || parse(uri).is_some() => s,
        _ => return load(uri).ok().flatten(),
    };
    let px = size.max_elem() * ctx.pixels_per_point() * zoom.max(1.0);
    let tier = TIERS.iter().find(|t| px <= **t as f32).copied();
    let origin = if web { uri } else { base(uri) };
    let wanted = if web {
        let sizer = *lock(&shared.web_sizer);
        sizer.and_then(|f| f(uri, tier)).unwrap_or_else(|| uri.to_owned())
    } else {
        match tier {
            Some(t) => format!("{origin}#{t}"),
            None => origin.to_owned(),
        }
    };
    let pass = ctx.cumulative_pass_nr();
    match load(&wanted) {
        Ok(Some(texture)) => {
            shared.used(&wanted, origin, pass, texture_bytes(texture.size));
            Some(texture)
        }
        Ok(None) => {
            shared.used(&wanted, origin, pass, 0);
            // Another size of the cover meanwhile, so a growing cover does
            // not blink.
            let other = shared.loaded_tier(origin)?;
            let texture = load(&other).ok().flatten()?;
            shared.used(&other, origin, pass, texture_bytes(texture.size));
            Some(texture)
        }
        Err(_) => {
            // Kept while drawn so a broken file is not decoded every pass.
            shared.used(&wanted, origin, pass, 0);
            None
        }
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

/// File path and tier of a cover URI; `None` for URIs this loader leaves to
/// others.
fn parse(uri: &str) -> Option<(PathBuf, Option<u32>)> {
    let b = base(uri);
    let tier = if b.len() < uri.len() { uri[b.len() + 1..].parse().ok() } else { None };
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
    Some((path, tier))
}

fn decode(path: &Path, tier: Option<u32>) -> Result<ColorImage, String> {
    let img = image::ImageReader::open(path)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .decode()
        .map_err(|e| e.to_string())?;
    let side = tier.unwrap_or(FULL);
    let img = if img.width().max(img.height()) > side { img.thumbnail(side, side) } else { img };
    let rgba = img.to_rgba8();
    Ok(ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    ))
}

impl Shared {
    fn used(&self, uri: &str, origin: &str, pass: u64, bytes: usize) {
        let mut lru = lock(&self.lru);
        let u = lru.used.entry(uri.to_owned()).or_insert_with(|| Use { origin: origin.to_owned(), last: pass, bytes });
        u.last = pass;
        u.bytes = bytes;
    }

    /// The largest loaded size of the cover `origin`.
    fn loaded_tier(&self, origin: &str) -> Option<String> {
        let lru = lock(&self.lru);
        lru.used
            .iter()
            .filter(|(_, u)| u.bytes > 0 && u.origin == origin)
            .max_by_key(|(_, u)| u.bytes)
            .map(|(uri, _)| uri.clone())
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
            let slot = match decode(&job.path, job.tier) {
                Ok(img) => Slot::Ready(Arc::new(img)),
                Err(e) => Slot::Failed(e),
            };
            let mut slots = lock(&self.slots);
            if let Some(s @ Slot::Pending) = slots.get_mut(&job.uri) {
                *s = slot;
                drop(slots);
                ctx.request_repaint();
            }
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
        for uri in forget {
            ctx.forget_image(&uri);
        }
    }
}

impl ImageLoader for CoverLoader {
    fn id(&self) -> &str {
        Self::ID
    }

    fn load(&self, ctx: &Context, uri: &str, _: SizeHint) -> ImageLoadResult {
        let Some((path, tier)) = parse(uri) else {
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
                slots.insert(uri.to_owned(), Slot::Pending);
                drop(slots);
                self.0.enqueue(ctx, Job { uri: uri.to_owned(), path, tier });
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
        assert!(p.ends_with("x/a/cover.jpg"));
        assert_eq!(t, Some(256));
        assert_eq!(parse("file:///x/a/cover.PNG").unwrap().1, None);
        assert!(parse("file:///x/a/icon.svg").is_none());
        assert!(parse("bytes://cover.jpg").is_none());
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
    fn a_growing_cover_shows_a_loaded_tier_while_decoding() {
        let ctx = ctx();
        let uris = covers("grow", 1, 600);
        show(&ctx, &uris, 48.0);
        let first = pass(&ctx, &uris, 400.0);
        assert_eq!(first[0].map(|t| t.size), Some(vec2(128.0, 128.0)));
    }

    /// Web "covers" of the size their url says (`https://x/a=w226`), ready
    /// at once unless the url ends in `slow`; counts what was forgotten.
    #[derive(Default)]
    struct WebStub {
        forgotten: Mutex<Vec<String>>,
    }

    impl ImageLoader for WebStub {
        fn id(&self) -> &str {
            "web-stub"
        }
        fn load(&self, _: &Context, uri: &str, _: SizeHint) -> ImageLoadResult {
            if !is_web(uri) {
                return Err(LoadError::NotSupported);
            }
            if uri.ends_with("slow") {
                return Ok(ImagePoll::Pending { size: None });
            }
            let side: usize = uri.rsplit_once("=w").and_then(|(_, n)| n.parse().ok()).unwrap_or(640);
            Ok(ImagePoll::Ready { image: Arc::new(ColorImage::filled([side, side], egui::Color32::RED)) })
        }
        fn forget(&self, uri: &str) {
            lock(&self.forgotten).push(uri.to_owned());
        }
        fn forget_all(&self) {}
        fn byte_size(&self) -> usize {
            0
        }
    }

    #[test]
    fn web_covers_are_asked_at_the_drawn_size_and_forgotten_off_screen() {
        let ctx = ctx();
        let stub = Arc::new(WebStub::default());
        ctx.add_image_loader(stub.clone());
        // Resizable urls end in `=w…`; `big` stands for "full size".
        set_web_sizer(&ctx, |url, tier| {
            let (b, _) = url.rsplit_once("=w")?;
            Some(match tier {
                Some(t) if t <= 256 => format!("{b}=w226"),
                _ => format!("{b}=w1200slow"),
            })
        });
        let yt = vec!["https://g/a=w544".to_owned()];
        let got = pass(&ctx, &yt, 48.0);
        assert_eq!(got[0].map(|t| t.size), Some(vec2(226.0, 226.0)));
        // Bigger: the full size is pending, the small one stands in.
        let got = pass(&ctx, &yt, 700.0);
        assert_eq!(got[0].map(|t| t.size), Some(vec2(226.0, 226.0)));
        // A url the sizer leaves alone is drawn as it is.
        let sp = vec!["https://spotify/abc".to_owned()];
        assert_eq!(pass(&ctx, &sp, 48.0)[0].map(|t| t.size), Some(vec2(640.0, 640.0)));
        // The pending full size scrolled away: forgotten (it leaves the
        // download queue).
        pass(&ctx, &sp, 48.0);
        pass(&ctx, &sp, 48.0);
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
