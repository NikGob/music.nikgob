//! Album covers: a loader that decodes `file://` JPEG/PNG covers and web
//! covers on a few worker threads, downscaled to the size they are drawn at,
//! and an LRU that forgets the textures of covers that have scrolled away.
//!
//! Draw covers through [`texture`]. It picks a size tier (`#128` to `#2048`
//! after the URI, or the full file), marks the cover as used in this
//! frame and, while the wanted tier decodes, shows a tier of the same file
//! that is already loaded. At the end of every frame (the last pass of it:
//! a pass egui discards ends nothing) the covers that were not drawn in this
//! or the previous frame and are not [pinned](pin) or [held](hold) are
//! forgotten, oldest first, until the textures fit the
//! [budget](DEFAULT_BUDGET); egui frees
//! their textures after that frame is painted, so a texture is never freed
//! while a frame still draws it. The loader hands every decoded image to
//! egui's texture loader and keeps no copy, and it reads files itself, so a
//! cover costs only its texture. Decoding runs on a few threads and within
//! [`DECODE_BUDGET`] bytes of pixels at once. [`memory`] says what covers
//! hold; [`trim_to_current`] lets a window hidden in the tray keep only the
//! playing track's cover.
//!
//! Whatever keeps a painted shape across frames instead of drawing the cover
//! again (a still tile's kept shapes) must hold its texture itself (a
//! [`egui::TextureHandle`] of the id): the loader frees what is not drawn.
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
//! art) is decoded without them (`nikgob_core::cover_frame`). A video's
//! thumbnail (`i.ytimg.com`) that is drawn is remembered for the app
//! ([`take_thumbnails`]) so it can look up the song's real art; once found
//! ([`set_real_covers`]) the art is drawn in the thumbnail's place.
//!
//! A cover file can be rewritten while its sizes are loaded (an import
//! replaces an album's `cover.jpg` with a larger one). Every size remembers
//! the modification time and length its file had when it was read; a size
//! read from the file as it is now forgets the sizes read from it before,
//! and [`refresh`] (after a library change, when the window comes back)
//! forgets every loaded size whose file changed. Both take effect at the end
//! of a pass, and a forgotten size that is still drawn is read again.
//! A file that could not be read (not there yet, or broken) is read again
//! after [`FAILED_RETRY`] while it stays drawn, not at every pass.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

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

/// [Held](hold) covers stay loaded off screen at the tiers up to this one
/// (the small pictures of a page's tiles, not a large copy drawn elsewhere).
pub const HELD_TIER: u32 = 256;

/// The longest side a cover is decoded at, even drawn larger.
const FULL: u32 = 4096;

/// Decoding threads at most.
const WORKERS: usize = 3;

/// Web cover seeds kept before the ones of covers no longer loaded go.
const MAX_SEEDS: usize = 512;

/// Bytes of pixels the decoding threads may hold at once (see
/// [`decode_cost`]); one decode runs whatever it costs.
pub const DECODE_BUDGET: usize = 64 << 20;

/// A cover file that failed to load is read again after this long while
/// it is drawn (an artist's photo downloaded again to the same path).
const FAILED_RETRY: Duration = Duration::from_secs(5);

/// What decoding a size at `tier` may hold at its peak: the decoded
/// picture, its RGBA copy and the image handed to egui.
fn decode_cost(tier: Option<u32>) -> usize {
    let side = tier.unwrap_or(FULL) as usize;
    side * side * 4 * 3
}

/// Texture bytes, loaded sizes and the budget at the end of the last frame,
/// and the pixel bytes being decoded now (for [`memory`]).
static TEXTURE_BYTES: AtomicUsize = AtomicUsize::new(0);
static SIZES: AtomicUsize = AtomicUsize::new(0);
static BUDGET: AtomicUsize = AtomicUsize::new(DEFAULT_BUDGET);
static DECODING: AtomicUsize = AtomicUsize::new(0);

/// What covers hold, for the app's memory line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Memory {
    /// Texture bytes of the loaded sizes (RGBA plus mipmaps, estimated).
    pub textures: usize,
    /// Sizes loaded or decoding.
    pub sizes: usize,
    /// The texture budget.
    pub budget: usize,
    /// Pixel bytes the decoding threads may hold right now.
    pub decoding: usize,
}

/// What covers hold now (as of the last frame). Costs a few atomic reads.
pub fn memory() -> Memory {
    Memory {
        textures: TEXTURE_BYTES.load(Ordering::Relaxed),
        sizes: SIZES.load(Ordering::Relaxed),
        budget: BUDGET.load(Ordering::Relaxed),
        decoding: DECODING.load(Ordering::Relaxed),
    }
}

enum Slot {
    Pending,
    Ready(Arc<ColorImage>),
    /// The error, and when it was met.
    Failed(String, Instant),
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
    /// What decoding it holds at its peak, once its header was read
    /// ([`source_cost`]); until then the tier's [`decode_cost`].
    cost: Option<usize>,
}

/// What a cover file was when a size of it was read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    modified: Option<std::time::SystemTime>,
    len: u64,
}

fn file_stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(Stamp { modified: meta.modified().ok(), len: meta.len() })
}

/// A [`refresh`] checking files, and whether another was asked meanwhile.
#[derive(Default)]
struct Check {
    running: bool,
    again: bool,
}

#[derive(Default)]
struct Jobs {
    queue: Vec<Job>,
    workers: usize,
    /// [`decode_cost`] of the decodes running now.
    in_flight: usize,
}

impl Jobs {
    /// Whether a decode costing `cost` may start now.
    fn fits(&self, cost: usize) -> bool {
        self.in_flight == 0 || self.in_flight + cost <= DECODE_BUDGET
    }
}

struct Use {
    /// The cover this is a size of: the file URI without its tier, or the
    /// web url before sizing.
    origin: String,
    /// The tier it is (`None`: the full file, or a web url as it is).
    tier: Option<u32>,
    /// Frame it was last drawn in ([`Shared::frame`]).
    last: u64,
    /// Texture bytes; 0 while decoding.
    bytes: usize,
}

struct Lru {
    used: HashMap<String, Use>,
    /// Files (URIs without a tier) kept loaded off screen.
    pinned: HashSet<String>,
    /// The first of them: the playing track's ([`trim_to_current`]).
    current: Option<String>,
    /// Files (URIs without a tier) kept loaded off screen at the small
    /// tiers ([`hold`]).
    held: HashSet<String>,
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
    /// The stamp of each loaded size of a cover file: by the file's URI,
    /// then by the size's URI.
    stamps: Mutex<HashMap<String, HashMap<String, Stamp>>>,
    /// Sizes read from a file that changed since; forgotten at the end of
    /// the pass.
    stale: Mutex<Vec<String>>,
    check: Mutex<Check>,
    /// A video thumbnail's url → the real art found for it.
    real: Mutex<Arc<HashMap<String, String>>>,
    /// Video thumbnails drawn lately with the frame each was last drawn
    /// in, and those not handed to the app yet.
    thumbnails: Mutex<(HashMap<String, u64>, Vec<String>)>,
    /// The frame being drawn: counts frames, not egui's passes (a frame
    /// whose first pass is discarded runs two), and moves on only at the end
    /// of a frame's last pass.
    frame: AtomicU64,
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
            current: None,
            held: HashSet::new(),
            budget: DEFAULT_BUDGET,
        }),
        stamps: Mutex::default(),
        stale: Mutex::default(),
        check: Mutex::default(),
        real: Mutex::default(),
        thumbnails: Mutex::default(),
        frame: AtomicU64::new(0),
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
/// the next track, the current one first). Replaces the previous set.
pub fn pin<'a>(ctx: &Context, uris: impl IntoIterator<Item = &'a str>) {
    if let Some(s) = shared(ctx) {
        // A thumbnail's real art is what is drawn for it: kept too.
        let pinned: Vec<String> = uris
            .into_iter()
            .flat_map(|u| {
                let real = if is_web(u) && is_thumbnail(u) { s.real_of(u) } else { None };
                std::iter::once(base(u).to_owned()).chain(real.map(|r| base(&r).to_owned()))
            })
            .collect();
        let mut lru = lock(&s.lru);
        lru.current = pinned.first().cloned();
        lru.pinned = pinned.into_iter().collect();
    }
}

/// Keeps these covers loaded while off screen at the tiers up to
/// [`HELD_TIER`]: a few small pictures a page shows every time it is back
/// (Главная's quick access), so they are there at once instead of read
/// again after another page filled the budget. Replaces the previous set.
pub fn hold<'a>(ctx: &Context, uris: impl IntoIterator<Item = &'a str>) {
    if let Some(s) = shared(ctx) {
        let held: HashSet<String> = uris.into_iter().map(|u| base(u).to_owned()).collect();
        lock(&s.lru).held = held;
    }
}

/// Forgets every loaded size of every cover but the playing track's (the
/// first one [`pin`] was given), with the decodes still queued for them:
/// for a window hidden in the tray, which draws nothing until it is shown
/// and then reads its covers again. Call it before anything of the frame
/// is drawn (in the app's logic) or while the window draws nothing; a cover
/// drawn later in the same frame is read again, never drawn dead.
pub fn trim_to_current(ctx: &Context) {
    let Some(s) = shared(ctx) else { return };
    let forget: Vec<String> = {
        let mut lru = lock(&s.lru);
        let current = lru.current.clone();
        let mut gone = Vec::new();
        lru.used.retain(|uri, u| {
            let keep = current.as_deref() == Some(u.origin.as_str());
            if !keep {
                gone.push(uri.clone());
            }
            keep
        });
        TEXTURE_BYTES.store(lru.used.values().map(|u| u.bytes).sum(), Ordering::Relaxed);
        SIZES.store(lru.used.len(), Ordering::Relaxed);
        gone
    };
    lock(&s.thumbnails).0.clear();
    if forget.is_empty() {
        return;
    }
    {
        let gone: HashSet<&str> = forget.iter().map(String::as_str).collect();
        lock(&s.jobs).queue.retain(|j| !gone.contains(j.uri.as_str()));
    }
    let n = forget.len();
    s.forget_sizes(ctx, forget);
    log::debug!("covers: {n} sizes forgotten, the playing track's kept");
}

/// Looks at the files of every loaded cover again (on a thread) and forgets
/// the sizes of those that changed since they were read, so they are read
/// anew where they are drawn. Call it when cover files may have been
/// rewritten: after an import or a retag, when the window comes back.
pub fn refresh(ctx: &Context) {
    let Some(shared) = shared(ctx) else { return };
    if lock(&shared.stamps).is_empty() {
        return;
    }
    {
        let mut check = lock(&shared.check);
        if check.running {
            check.again = true;
            return;
        }
        check.running = true;
    }
    let (me, ctx) = (Arc::clone(&shared), ctx.clone());
    let spawned = std::thread::Builder::new()
        .name("md3e-covers-check".into())
        .spawn(move || me.check_files(&ctx));
    if spawned.is_err() {
        lock(&shared.check).running = false;
    }
}

/// Put before a web cover's url in a bytes loader's `forget`: what is kept
/// of it goes from the disk too, it is asked anew ([`refetch`]).
pub const REFETCH: &str = "refetch:";

/// The web cover at `url` is asked anew («Обновить обложку»): every size
/// of it held is forgotten at the end of the frame, and so is its seed; the
/// bytes loaders drop what they keep of each url it is asked at, on disk
/// too ([`REFETCH`]).
pub fn refetch(ctx: &Context, url: &str) {
    let Some(shared) = shared(ctx) else { return };
    let held: Vec<String> = lock(&shared.lru).used.iter().filter(|(_, u)| u.origin == url).map(|(k, _)| k.clone()).collect();
    lock(&shared.stale).extend(held);
    lock(&shared.web_seeds).remove(url);
    let mut asked: Vec<String> = TIERS
        .iter()
        .map(|t| Some(*t))
        .chain([None])
        .map(|t| base(&sized(&shared, url, t).0).to_owned())
        .collect();
    asked.sort_unstable();
    asked.dedup();
    for l in ctx.loaders().bytes.lock().iter() {
        for a in &asked {
            l.forget(&format!("{REFETCH}{a}"));
        }
    }
    ctx.request_repaint();
}

/// The colour-scheme seed of the web cover at `url`, once a size of it has
/// been decoded; `None` before, or for a cover too grey for one.
pub fn web_seed(ctx: &Context, url: &str) -> Option<u32> {
    let shared = shared(ctx)?;
    let url = shared.real_of(url).unwrap_or_else(|| url.to_owned());
    lock(&shared.web_seeds).get(&url).copied().flatten()
}

/// The real art found for video thumbnails (thumbnail url → art url): drawn
/// in their place from now on.
pub fn set_real_covers(ctx: &Context, real: &Arc<HashMap<String, String>>) {
    if let Some(s) = shared(ctx) {
        let mut held = lock(&s.real);
        if !Arc::ptr_eq(&held, real) {
            *held = Arc::clone(real);
        }
    }
}

/// The video thumbnails that came on screen since the last call (once
/// while they stay drawn): the app looks up their real art.
pub fn take_thumbnails(ctx: &Context) -> Vec<String> {
    shared(ctx).map(|s| std::mem::take(&mut lock(&s.thumbnails).1)).unwrap_or_default()
}

/// Whether `url` is a video's thumbnail rather than album art.
fn is_thumbnail(url: &str) -> bool {
    url.contains("i.ytimg.com/")
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
    // A video's thumbnail: its real art once found (the thumbnail while
    // the art loads, so the swap does not blink), else it is asked for.
    if is_web(uri) && is_thumbnail(uri) {
        match shared.real_of(uri) {
            Some(real) => return sized_texture(ctx, &shared, &real, size, zoom).or_else(|| sized_texture(ctx, &shared, uri, size, zoom)),
            None => shared.thumbnail_drawn(uri, shared.frame()),
        }
    }
    sized_texture(ctx, &shared, uri, size, zoom)
}

/// [`texture`] of a cover the loader handles.
fn sized_texture(ctx: &Context, shared: &Shared, uri: &str, size: Vec2, zoom: f32) -> Option<SizedTexture> {
    let px = size.max_elem() * ctx.pixels_per_point() * zoom.max(1.0);
    let tier = TIERS.iter().find(|t| px <= **t as f32).copied();
    let (wanted, origin) = sized(shared, uri, tier);
    let pass = shared.frame();
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
/// A video thumbnail counts as what [`texture`] draws for it: its real art
/// once found (the thumbnail too, drawn while the art decodes).
pub fn prefetch_like(ctx: &Context, current: &str, next: &str) {
    let Some(shared) = shared(ctx) else { return };
    let real = |u: &str| if is_web(u) && is_thumbnail(u) { shared.real_of(u) } else { None };
    let current = real(current).unwrap_or_else(|| current.to_owned());
    let next_real = real(next);
    let drawn = next_real.as_deref().unwrap_or(next);
    if base(&current) == base(drawn) || parse(drawn).is_none() {
        return;
    }
    let mut nexts = vec![drawn];
    if next_real.is_some() && parse(next).is_some() {
        nexts.push(next);
    }
    let pass = shared.frame();
    let (_, origin) = sized(&shared, &current, None);
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
        for next in &nexts {
            let (wanted, origin) = sized(&shared, next, tier);
            let bytes = match load(ctx, &wanted) {
                Ok(Some(texture)) => texture_bytes(texture.size),
                _ => 0,
            };
            shared.used(&wanted, &origin, tier, pass, bytes);
        }
    }
}

/// The next track's cover as if drawn: a video thumbnail without its real
/// art yet is handed to the app ([`take_thumbnails`], once while asked
/// for), so the art is looked up while the track waits, not after it
/// starts.
pub fn want_real_cover(ctx: &Context, uri: &str) {
    let Some(shared) = shared(ctx) else { return };
    if is_web(uri) && is_thumbnail(uri) && shared.real_of(uri).is_none() {
        shared.thumbnail_drawn(uri, shared.frame());
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

/// Pixels a cover may have at most (8192 × 8192): a larger one is refused
/// before it is decoded.
const MAX_SOURCE_PIXELS: u64 = 1 << 26;
/// A cover file larger than this is not read.
const MAX_SOURCE_BYTES: u64 = 64 << 20;

/// What decoding `source` for `tier` holds at its peak, from its header
/// alone: the encoded bytes, the picture as decoded (a JPEG the DCT can
/// scale at 1/2, 1/4 or 1/8, see [`scaled_jpeg`]; anything else at its full
/// size) and the tier's own copies ([`decode_cost`]). An error for a source
/// too large to decode. Only the header is read: a JPEG's segments before
/// its frame header (an embedded thumbnail, a colour profile) are skipped,
/// not read.
fn source_cost(source: &Source, tier: Option<u32>) -> Result<usize, String> {
    let (encoded, header) = match source {
        Source::File(path) => {
            let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
            let len = f.metadata().map_err(|e| e.to_string())?.len();
            (len, header_of(&mut f))
        }
        Source::Web(_, bytes) => (bytes.len() as u64, header_of(&mut std::io::Cursor::new(&bytes[..]))),
    };
    if encoded > MAX_SOURCE_BYTES {
        return Err(format!("cover file too large ({encoded} bytes)"));
    }
    // Not readable here: the decode says why.
    let Some(Header { width: w, height: h, scalable }) = header else {
        return Ok(usize::try_from(encoded).unwrap_or(usize::MAX).saturating_add(decode_cost(tier)));
    };
    let pixels = u64::from(w) * u64::from(h);
    if pixels > MAX_SOURCE_PIXELS {
        return Err(format!("cover too large ({w}×{h})"));
    }
    // As `scaled_jpeg` decides: at least twice the side wanted, and a pixel
    // format it scales.
    let side = u64::from(tier.unwrap_or(FULL));
    let long = u64::from(w.max(h));
    let scale = if scalable { [8, 4, 2].into_iter().find(|s| long / s >= side).unwrap_or(1) } else { 1 };
    let decoded = pixels / (scale * scale) * 4;
    Ok(usize::try_from(encoded + decoded).unwrap_or(usize::MAX).saturating_add(decode_cost(tier)))
}

/// What a picture's header says.
#[derive(Debug, PartialEq, Eq)]
struct Header {
    width: u32,
    height: u32,
    /// A JPEG [`scaled_jpeg`] can decode scaled (8-bit grey or colour).
    scalable: bool,
}

/// The header of the picture `r` reads from its start.
fn header_of(r: &mut (impl std::io::Read + std::io::Seek)) -> Option<Header> {
    use std::io::Read as _;
    let mut magic = [0u8; 3];
    r.read_exact(&mut magic).ok()?;
    if magic == [0xFF, 0xD8, 0xFF] {
        return jpeg_header(r);
    }
    r.seek(std::io::SeekFrom::Start(0)).ok()?;
    let mut head = Vec::new();
    r.take(64 << 10).read_to_end(&mut head).ok()?;
    let (width, height) =
        image::ImageReader::new(std::io::Cursor::new(&head)).with_guessed_format().ok()?.into_dimensions().ok()?;
    Some(Header { width, height, scalable: false })
}

/// A JPEG's frame header, `r` just past `FF D8 FF`: segments are skipped
/// by their lengths until a start-of-frame marker.
fn jpeg_header(r: &mut (impl std::io::Read + std::io::Seek)) -> Option<Header> {
    let byte = |r: &mut dyn std::io::Read| {
        let mut b = [0u8; 1];
        r.read_exact(&mut b).ok().map(|()| b[0])
    };
    let mut marker = byte(r)?;
    // At most a few hundred segments before the frame.
    for _ in 0..1024 {
        while marker == 0xFF {
            marker = byte(r)?;
        }
        let mut len = [0u8; 2];
        r.read_exact(&mut len).ok()?;
        let len = u16::from_be_bytes(len);
        if len < 2 {
            return None;
        }
        // SOF0..SOF15 but DHT (C4), JPG (C8) and DAC (CC).
        if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let mut f = [0u8; 6];
            r.read_exact(&mut f).ok()?;
            let (precision, height, width, components) =
                (f[0], u16::from_be_bytes([f[1], f[2]]), u16::from_be_bytes([f[3], f[4]]), f[5]);
            // Sequential and progressive Huffman, 8-bit, grey or colour:
            // what jpeg-decoder gives as L8 / RGB24 and scales.
            let scalable = matches!(marker, 0xC0..=0xC2) && precision == 8 && matches!(components, 1 | 3);
            return Some(Header { width: u32::from(width), height: u32::from(height), scalable });
        }
        if marker == 0xD9 || marker == 0xDA {
            return None;
        }
        r.seek(std::io::SeekFrom::Current(i64::from(len) - 2)).ok()?;
        marker = match (byte(r)?, byte(r)?) {
            (0xFF, m) => m,
            _ => return None,
        };
    }
    None
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
    /// The frame being drawn.
    fn frame(&self) -> u64 {
        self.frame.load(Ordering::Relaxed)
    }

    /// The real art found for the web cover `url`, if it is a thumbnail.
    fn real_of(&self, url: &str) -> Option<String> {
        lock(&self.real).get(url).cloned()
    }

    /// Remembers the thumbnail `url` drawn in frame `pass`: handed to the
    /// app when it was not drawn in the frame before (it came on screen).
    fn thumbnail_drawn(&self, url: &str, pass: u64) {
        let mut t = lock(&self.thumbnails);
        let before = t.0.insert(url.to_owned(), pass);
        if before.is_none_or(|p| p + 1 < pass) {
            t.1.push(url.to_owned());
        }
    }

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
        let cost = decode_cost(job.tier);
        jobs.queue.push(job);
        // Over the decode budget a new thread would only leave again: the
        // ones decoding take the job when they are done.
        if jobs.workers < WORKERS && jobs.fits(cost) {
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
    /// until the queue is empty. A decode that would take the pixels held
    /// at once past [`DECODE_BUDGET`] is left to the thread decoding now
    /// (there is one: something is in flight), and this one ends.
    fn work(&self, ctx: &Context) {
        loop {
            let (mut job, cost) = {
                let mut jobs = lock(&self.jobs);
                let Some(job) = jobs.queue.pop() else {
                    jobs.workers -= 1;
                    return;
                };
                let cost = job.cost.unwrap_or_else(|| decode_cost(job.tier));
                if !jobs.fits(cost) {
                    jobs.queue.push(job);
                    jobs.workers -= 1;
                    return;
                }
                jobs.in_flight += cost;
                DECODING.store(jobs.in_flight, Ordering::Relaxed);
                (job, cost)
            };
            let release = || {
                let mut jobs = lock(&self.jobs);
                jobs.in_flight -= cost;
                DECODING.store(jobs.in_flight, Ordering::Relaxed);
            };
            // Forgotten (scrolled away) before its turn.
            if !matches!(lock(&self.slots).get(&job.uri), Some(Slot::Pending)) {
                release();
                continue;
            }
            // Before reading: a file rewritten in between is seen as changed
            // later and read again, never the other way round.
            let stamp = match &job.source {
                Source::File(path) => file_stamp(path),
                Source::Web(..) => None,
            };
            // What the source really takes to decode, from its header: a
            // large picture waits for room in the budget like any other.
            let checked = match job.cost {
                Some(_) => Ok(()),
                None => source_cost(&job.source, job.tier).map(|real| {
                    job.cost = Some(real);
                }),
            };
            if checked.is_ok() && job.cost.is_some_and(|real| real > cost) {
                release();
                lock(&self.jobs).queue.push(job);
                continue;
            }
            let slot = match checked.and_then(|()| decode(&job.source, job.tier)) {
                Ok(img) => Slot::Ready(Arc::new(img)),
                Err(e) => Slot::Failed(e, Instant::now()),
            };
            release();
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
                // Stamped before it can be drawn, so a refresh from then on
                // sees it.
                if let (Source::File(path), Some(stamp)) = (&job.source, stamp) {
                    self.stamped(&job.uri, path, stamp);
                }
                *s = slot;
                drop(slots);
                ctx.request_repaint();
            }
        }
    }

    /// Records the stamp a size of a file was read with. If another size was
    /// read from a different file, the file changed in between: the sizes
    /// not read from it as it is now become stale.
    fn stamped(&self, uri: &str, path: &Path, stamp: Stamp) {
        let origin = base(uri);
        let mut stamps = lock(&self.stamps);
        let sizes = stamps.entry(origin.to_owned()).or_default();
        sizes.insert(uri.to_owned(), stamp);
        if sizes.values().all(|s| *s == stamp) {
            return;
        }
        let now = file_stamp(path);
        let stale: Vec<String> = sizes.iter().filter(|(_, s)| Some(**s) != now).map(|(u, _)| u.clone()).collect();
        for u in &stale {
            sizes.remove(u);
        }
        if sizes.is_empty() {
            stamps.remove(origin);
        }
        drop(stamps);
        lock(&self.stale).extend(stale);
    }

    /// [`refresh`]'s thread: stats every loaded file once a round, again
    /// while more refreshes were asked meanwhile.
    fn check_files(&self, ctx: &Context) {
        loop {
            let files: Vec<(String, Vec<(String, Stamp)>)> = lock(&self.stamps)
                .iter()
                .map(|(origin, sizes)| (origin.clone(), sizes.iter().map(|(u, s)| (u.clone(), *s)).collect()))
                .collect();
            let mut changed = Vec::new();
            for (origin, sizes) in files {
                let Some((Parsed::File(path), _)) = parse(&origin) else { continue };
                let now = file_stamp(&path);
                changed.extend(sizes.into_iter().filter(|(_, s)| Some(*s) != now).map(|(u, s)| (origin.clone(), u, s)));
            }
            let mut stale = Vec::new();
            if !changed.is_empty() {
                let mut stamps = lock(&self.stamps);
                for (origin, uri, stamp) in changed {
                    let Some(sizes) = stamps.get_mut(&origin) else { continue };
                    // A size read again meanwhile is already the new file.
                    if sizes.get(&uri) == Some(&stamp) {
                        sizes.remove(&uri);
                        stale.push(uri);
                    }
                    if sizes.is_empty() {
                        stamps.remove(&origin);
                    }
                }
            }
            if !stale.is_empty() {
                lock(&self.stale).extend(stale);
                ctx.request_repaint();
            }
            let mut check = lock(&self.check);
            if !check.again {
                check.running = false;
                return;
            }
            check.again = false;
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

    /// At the end of a frame: cancels covers no longer drawn that are still
    /// decoding, then forgets the least recently drawn ones off screen until
    /// the budget fits. A pass egui discards (another follows in the same
    /// frame) forgets nothing: only a frame that is painted ends.
    fn evict(&self, ctx: &Context) {
        if ctx.will_discard() {
            return;
        }
        let pass = self.frame.fetch_add(1, Ordering::Relaxed);
        let shown = |u: &Use| u.last + 1 >= pass;
        // Read from a file that changed: forgotten even on screen, so the
        // next pass reads it again.
        let mut forget = std::mem::take(&mut *lock(&self.stale));
        {
            let mut lru = lock(&self.lru);
            for uri in &forget {
                lru.used.remove(uri);
            }
            let Lru { used, pinned, held, budget, .. } = &mut *lru;
            let kept = |u: &Use| pinned.contains(&u.origin) || (u.tier.is_some_and(|t| t <= HELD_TIER) && held.contains(&u.origin));
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
                    .filter(|(_, u)| !shown(u) && !kept(u))
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
            TEXTURE_BYTES.store(total, Ordering::Relaxed);
            SIZES.store(used.len(), Ordering::Relaxed);
            BUDGET.store(*budget, Ordering::Relaxed);
            // Seeds of web covers none of whose sizes is loaded any more
            // (and not the playing track's): a cover drawn again is seeded
            // again when it decodes.
            let mut seeds = lock(&self.web_seeds);
            if seeds.len() > MAX_SEEDS {
                let live: HashSet<&str> = used.values().map(|u| u.origin.as_str()).collect();
                seeds.retain(|url, _| live.contains(url.as_str()) || pinned.contains(url));
            }
        }
        // Thumbnails not drawn lately: forgetting them changes nothing (one
        // drawn again is new on screen either way), and the map stays small.
        {
            let mut t = lock(&self.thumbnails);
            if t.0.len() > 256 {
                t.0.retain(|_, p| *p + 1 >= pass);
            }
        }
        self.forget_sizes(ctx, forget);
    }

    /// Forgets these sizes in egui's loaders (their textures are freed
    /// after the frame is painted); a web cover none of whose sizes is kept
    /// any more also leaves the bytes loader (and its download queue if it
    /// still waits there).
    fn forget_sizes(&self, ctx: &Context, forget: Vec<String>) {
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
        let file = matches!(parsed, Parsed::File(_));
        let mut slots = lock(&self.0.slots);
        // A file that failed a while ago is read again (a web cover's
        // bytes stay what they were: its failure stands).
        if file && matches!(slots.get(uri), Some(Slot::Failed(_, at)) if at.elapsed() >= FAILED_RETRY) {
            slots.remove(uri);
        }
        match slots.remove(uri) {
            // The texture loader keeps the texture; no copy stays here.
            Some(Slot::Ready(image)) => Ok(ImagePoll::Ready { image }),
            Some(slot) => {
                let r = match &slot {
                    Slot::Failed(e, at) => {
                        // Drawn now: a frame when it may be read again,
                        // so an idle window does not keep it missing.
                        if file {
                            ctx.request_repaint_after(FAILED_RETRY.saturating_sub(at.elapsed()));
                        }
                        Err(LoadError::Loading(e.clone()))
                    }
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
                self.0.enqueue(ctx, Job { uri: uri.to_owned(), source, tier, cost: None });
                Ok(ImagePoll::Pending { size: None })
            }
        }
    }

    fn forget(&self, uri: &str) {
        lock(&self.0.slots).remove(uri);
        let origin = base(uri);
        let mut stamps = lock(&self.0.stamps);
        if let Some(sizes) = stamps.get_mut(origin) {
            sizes.remove(uri);
            if sizes.is_empty() {
                stamps.remove(origin);
            }
        }
    }

    fn forget_all(&self) {
        lock(&self.0.slots).clear();
        lock(&self.0.stamps).clear();
    }

    fn byte_size(&self) -> usize {
        lock(&self.0.slots)
            .values()
            .map(|s| match s {
                Slot::Ready(img) => img.pixels.len() * 4,
                Slot::Failed(e, _) => e.len(),
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

    /// A thumbnail is handed to the app when it comes on screen, not at
    /// every pass it stays drawn; the art found is drawn in its place.
    #[test]
    fn a_thumbnail_on_screen_is_handed_over_and_its_real_art_taken() {
        let ctx = ctx();
        let s = shared(&ctx).unwrap();
        let thumb = "https://i.ytimg.com/vi/a/hqdefault.jpg";
        s.thumbnail_drawn(thumb, 1);
        s.thumbnail_drawn(thumb, 1);
        s.thumbnail_drawn(thumb, 2);
        assert_eq!(take_thumbnails(&ctx), vec![thumb.to_owned()]);
        assert!(take_thumbnails(&ctx).is_empty());
        // Scrolled away and back.
        s.thumbnail_drawn(thumb, 9);
        assert_eq!(take_thumbnails(&ctx), vec![thumb.to_owned()]);
        assert_eq!(s.real_of(thumb), None);
        let real = Arc::new(HashMap::from([(thumb.to_owned(), "https://x.invalid/art.jpg".to_owned())]));
        set_real_covers(&ctx, &real);
        assert_eq!(s.real_of(thumb).as_deref(), Some("https://x.invalid/art.jpg"));
        assert!(is_thumbnail(thumb) && !is_thumbnail("https://x.invalid/art.jpg"));
    }

    #[test]
    fn a_web_cover_is_seeded_by_its_pixels() {
        let ctx = ctx();
        let red = ColorImage::new([64, 64], vec![egui::Color32::from_rgb(200, 30, 30); 64 * 64]);
        shared(&ctx).expect("installed").seed_web("https://x.invalid/c.jpg#128", &red);
        assert!(web_seed(&ctx, "https://x.invalid/c.jpg").is_some());
        assert_eq!(web_seed(&ctx, "https://x.invalid/other.jpg"), None);
    }

    /// Seeds of covers browsed past do not pile up for the window's life;
    /// the playing track's stays.
    #[test]
    fn web_seeds_of_covers_no_longer_loaded_go() {
        let ctx = ctx();
        let s = shared(&ctx).unwrap();
        {
            let mut seeds = lock(&s.web_seeds);
            for i in 0..MAX_SEEDS + 100 {
                seeds.insert(format!("https://x.invalid/{i}.jpg"), Some(i as u32));
            }
        }
        pin(&ctx, ["https://x.invalid/7.jpg"]);
        pass(&ctx, &[], 64.0);
        assert_eq!(lock(&s.web_seeds).len(), 1);
        assert_eq!(web_seed(&ctx, "https://x.invalid/7.jpg"), Some(7));
    }

    fn web(bytes: Vec<u8>) -> Source {
        Source::Web("https://x.invalid/a".into(), egui::load::Bytes::Shared(Arc::from(bytes)))
    }

    fn encoded(img: &image::DynamicImage, format: image::ImageFormat) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, format).unwrap();
        out.into_inner()
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }

    /// A small PNG whose header claims `w` × `h`.
    fn png_claiming(w: u32, h: u32) -> Vec<u8> {
        let mut png = encoded(&image::DynamicImage::new_rgb8(1, 1), image::ImageFormat::Png);
        // Signature (8), IHDR length (4), "IHDR" (4), width, height.
        png[16..20].copy_from_slice(&w.to_be_bytes());
        png[20..24].copy_from_slice(&h.to_be_bytes());
        let crc = crc32(&png[12..29]);
        png[29..33].copy_from_slice(&crc.to_be_bytes());
        png
    }

    /// A small tile of a large PNG costs the decoded picture, not the
    /// tile; a picture beyond the cap is refused before decoding.
    #[test]
    fn the_decode_cost_counts_the_source_picture() {
        let big = image::DynamicImage::new_rgb8(2048, 2048);
        let png = web(encoded(&big, image::ImageFormat::Png));
        let cost = source_cost(&png, Some(128)).unwrap();
        assert!(cost >= 2048 * 2048 * 4, "{cost}");
        assert!(cost > decode_cost(Some(128)) * 20);
        // A JPEG decodes scaled in the DCT: an eighth of the side for a
        // 128 px tile.
        let jpeg = web(encoded(&big, image::ImageFormat::Jpeg));
        let scaled = source_cost(&jpeg, Some(128)).unwrap();
        assert!(scaled < 2048 * 2048 * 4 / 16, "{scaled}");
        let err = source_cost(&web(png_claiming(20_000, 20_000)), Some(128)).unwrap_err();
        assert!(err.contains("too large"), "{err}");
        assert!(source_cost(&web(png_claiming(64, 64)), Some(128)).is_ok());
        // Not an image: left to the decode, which says why.
        assert_eq!(source_cost(&web(b"<html>".to_vec()), Some(128)).unwrap(), 6 + decode_cost(Some(128)));
    }

    /// A JPEG header: `app` bytes of metadata segments, then a frame of
    /// `components` at 2048 × 2048.
    fn jpeg_head(app: usize, components: u8) -> Vec<u8> {
        let mut j = vec![0xFF, 0xD8];
        let mut left = app;
        while left > 0 {
            let n = left.min(60_000);
            j.extend_from_slice(&[0xFF, 0xE1]);
            j.extend_from_slice(&(n as u16 + 2).to_be_bytes());
            j.extend(std::iter::repeat_n(0u8, n));
            left -= n;
        }
        let len = 8 + 3 * u16::from(components);
        j.extend_from_slice(&[0xFF, 0xC0]);
        j.extend_from_slice(&len.to_be_bytes());
        j.extend_from_slice(&[8, 0x08, 0x00, 0x08, 0x00, components]);
        for c in 0..components {
            j.extend_from_slice(&[c + 1, 0x11, 0]);
        }
        j.extend_from_slice(&[0xFF, 0xD9]);
        j
    }

    /// A CMYK JPEG is decoded whole (the DCT scaling does not take it), so
    /// it is charged whole; metadata past the first 64 KB is skipped, not
    /// read, to find the frame.
    #[test]
    fn a_jpeg_the_dct_cannot_scale_costs_its_full_picture() {
        let full = 2048 * 2048 * 4;
        let cmyk = source_cost(&web(jpeg_head(150_000, 4)), Some(128)).unwrap();
        assert!(cmyk >= full, "{cmyk}");
        let rgb = source_cost(&web(jpeg_head(150_000, 3)), Some(128)).unwrap();
        assert!(rgb < full / 16 + 150_100 + decode_cost(Some(128)), "{rgb}");
        assert_eq!(
            header_of(&mut std::io::Cursor::new(jpeg_head(150_000, 1))),
            Some(Header { width: 2048, height: 2048, scalable: true })
        );
        // From a file too.
        let dir = std::env::temp_dir().join(format!("md3e-cmyk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("cmyk.jpg");
        std::fs::write(&p, jpeg_head(70_000, 4)).unwrap();
        assert!(source_cost(&Source::File(p), Some(128)).unwrap() >= full);
        let _ = std::fs::remove_dir_all(&dir);
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
    fn held_covers_stay_loaded_while_another_page_fills_the_budget() {
        let ctx = ctx();
        let home = covers("held", 2, 300);
        let other = covers("held-other", 6, 300);
        let small = texture_bytes(vec2(128.0, 128.0));
        set_budget(&ctx, 3 * small);
        hold(&ctx, home.iter().map(String::as_str));
        show(&ctx, &home, 72.0);
        // Another page, more than the budget on its own.
        show(&ctx, &other, 72.0);
        pass(&ctx, &other, 72.0);
        pass(&ctx, &other, 72.0);
        let has = |uris: &[String]| {
            let now = loaded(&ctx);
            uris.iter().all(|u| now.iter().any(|l| base(l) == u))
        };
        assert!(has(&home), "the held covers were forgotten");
        assert!(has(&other), "the visible ones stay too");
        // A large copy of a held cover is not held.
        show(&ctx, &home[..1], 400.0);
        pass(&ctx, &other, 72.0);
        pass(&ctx, &other, 72.0);
        let now = loaded(&ctx);
        assert!(!now.iter().any(|l| l.ends_with("#512")), "{now:?}");
        assert!(has(&home));
        // Held no longer: they go like any cover off screen.
        hold(&ctx, std::iter::empty());
        pass(&ctx, &other, 72.0);
        pass(&ctx, &other, 72.0);
        let now = loaded(&ctx);
        assert!(home.iter().all(|u| !now.iter().any(|l| base(l) == u)), "{now:?}");
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

    /// A stream track's cover is a video thumbnail whose real art the app
    /// found: the next one's art is loaded at the sizes the current one's
    /// art is drawn at.
    #[test]
    fn the_next_real_art_of_a_thumbnail_loads_ahead() {
        let ctx = ctx();
        let art = covers("next-real", 2, 600);
        let (current, next) = ("https://i.ytimg.com/vi/cur/hqdefault.jpg", "https://i.ytimg.com/vi/nxt/hqdefault.jpg");
        let real = Arc::new(HashMap::from([(current.to_owned(), art[0].clone()), (next.to_owned(), art[1].clone())]));
        set_real_covers(&ctx, &real);
        for _ in 0..5000 {
            let mut done = false;
            ctx.run_ui(egui::RawInput::default(), |ui| {
                let small = texture(ui.ctx(), current, vec2(48.0, 48.0), 1.0);
                let big = texture(ui.ctx(), current, vec2(400.0, 400.0), 1.0);
                prefetch_like(ui.ctx(), current, next);
                let l = loaded(ui.ctx());
                done = small.is_some() && big.is_some() && l.contains(&format!("{}#128", art[1])) && l.contains(&format!("{}#512", art[1]));
            })
            .drop_without_applying_deltas();
            if done {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("the next real art was not loaded ahead");
    }

    /// The next cover's thumbnail without art is handed over before it is
    /// drawn; one with art is not.
    #[test]
    fn the_next_thumbnail_is_handed_over_ahead() {
        let ctx = ctx();
        let (bare, found) = ("https://i.ytimg.com/vi/a/hqdefault.jpg", "https://i.ytimg.com/vi/b/hqdefault.jpg");
        set_real_covers(&ctx, &Arc::new(HashMap::from([(found.to_owned(), "https://x.invalid/art.jpg".to_owned())])));
        want_real_cover(&ctx, bare);
        want_real_cover(&ctx, found);
        want_real_cover(&ctx, "https://x.invalid/album.jpg");
        assert_eq!(take_thumbnails(&ctx), vec![bare.to_owned()]);
    }

    /// The path of a cover URI made by [`covers`].
    fn path_of(uri: &str) -> PathBuf {
        match parse(uri) {
            Some((Parsed::File(p), _)) => p,
            _ => panic!("not a file cover: {uri}"),
        }
    }

    /// Passes drawing `uri` into `size` points until it is drawn at `want`.
    fn settles_at(ctx: &Context, uri: &str, size: f32, want: Vec2) -> bool {
        let uris = [uri.to_owned()];
        for _ in 0..5000 {
            if pass(ctx, &uris, size)[0].is_some_and(|t| t.size == want) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        false
    }

    #[test]
    fn a_size_read_before_the_file_changed_is_read_again() {
        let ctx = ctx();
        let uris = covers("changed", 1, 600);
        let uri = &uris[0];
        pin(&ctx, [uri.as_str()]);
        assert!(settles_at(&ctx, uri, 48.0, vec2(128.0, 128.0)));
        // An import puts a larger, wider picture in its place.
        image::RgbImage::from_pixel(1200, 600, image::Rgb([0, 200, 0])).save(path_of(uri)).unwrap();
        // The big size is read from the new file...
        assert!(settles_at(&ctx, uri, 400.0, vec2(512.0, 256.0)));
        // ...and the small one, pinned and loaded before, follows it.
        assert!(settles_at(&ctx, uri, 48.0, vec2(128.0, 64.0)), "the small size kept the old picture");
    }

    /// A cover file that was not there when it was first drawn (a photo
    /// downloaded again to its old path) is read again a while later while
    /// it stays on screen; before that its failure stands.
    #[test]
    fn a_cover_file_that_failed_is_read_again_after_a_while() {
        let ctx = ctx();
        let uris = covers("failed", 1, 300);
        let path = path_of(&uris[0]);
        std::fs::remove_file(&path).unwrap();
        let s = shared(&ctx).unwrap();
        let failed = || lock(&s.slots).values().any(|slot| matches!(slot, Slot::Failed(..)));
        for _ in 0..5000 {
            pass(&ctx, &uris, 72.0);
            if failed() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(failed(), "the missing file did not fail");
        image::RgbImage::from_pixel(300, 300, image::Rgb([0, 200, 0])).save(&path).unwrap();
        for _ in 0..20 {
            assert!(pass(&ctx, &uris, 72.0)[0].is_none(), "read again at once");
        }
        assert!(failed());
        for slot in lock(&s.slots).values_mut() {
            if let Slot::Failed(_, at) = slot {
                *at = Instant::now().checked_sub(FAILED_RETRY).unwrap_or(*at);
            }
        }
        assert!(settles_at(&ctx, &uris[0], 72.0, vec2(128.0, 128.0)), "the file was not read again");
    }

    #[test]
    fn refresh_reads_again_the_covers_whose_files_changed() {
        let ctx = ctx();
        let uris = covers("refresh", 2, 600);
        pin(&ctx, uris.iter().map(String::as_str));
        assert_eq!(show(&ctx, &uris, 48.0).iter().map(|t| t.size).collect::<Vec<_>>(), vec![vec2(128.0, 128.0); 2]);
        image::RgbImage::from_pixel(600, 300, image::Rgb([0, 200, 0])).save(path_of(&uris[0])).unwrap();
        refresh(&ctx);
        assert!(settles_at(&ctx, &uris[0], 48.0, vec2(128.0, 64.0)), "the changed cover was not read again");
        // The other file did not change: its size stays as it was.
        assert!(loaded(&ctx).contains(&format!("{}#128", uris[1])));
        assert!(lock(&shared(&ctx).unwrap().stamps).contains_key(&uris[1]));
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

    /// Hidden in the tray, only the playing track's cover stays; shown
    /// again, the others are read again.
    #[test]
    fn trimming_keeps_only_the_playing_cover() {
        let ctx = ctx();
        let uris = covers("trim", 3, 300);
        pin(&ctx, [uris[0].as_str(), uris[1].as_str()]);
        show(&ctx, &uris, 64.0);
        assert_eq!(loaded(&ctx).len(), 3);
        trim_to_current(&ctx);
        assert_eq!(loaded(&ctx), vec![format!("{}#128", uris[0])]);
        assert_eq!(show(&ctx, &uris, 64.0).len(), 3);
    }

    /// A frame egui runs twice (its first pass discarded) is one frame: a
    /// cover drawn in the frame before is not forgotten in it, however
    /// small the budget, and is drawn with the same texture after it.
    #[test]
    fn a_discarded_pass_forgets_nothing() {
        let ctx = ctx();
        set_budget(&ctx, 0);
        let uris = covers("discard", 1, 300);
        let first = show(&ctx, &uris, 64.0)[0].id;
        let mut passes = 0;
        ctx.run_ui(egui::RawInput::default(), |ui| {
            if passes == 0 {
                ui.ctx().request_discard("test");
            }
            passes += 1;
        })
        .drop_without_applying_deltas();
        assert_eq!(passes, 2);
        assert!(loaded(&ctx).contains(&format!("{}#128", uris[0])), "forgotten in a discarded pass");
        assert_eq!(pass(&ctx, &uris, 64.0)[0].map(|t| t.id), Some(first));
        // Two frames without it: gone.
        pass(&ctx, &[], 64.0);
        pass(&ctx, &[], 64.0);
        assert!(loaded(&ctx).is_empty());
    }

    #[test]
    fn decodes_stay_within_their_budget_and_give_it_back() {
        assert!(decode_cost(Some(1024)) * 3 <= DECODE_BUDGET, "three middle sizes at once");
        assert!(decode_cost(None) > DECODE_BUDGET, "a full size decodes alone");
        let busy = Jobs { in_flight: decode_cost(Some(2048)), ..Jobs::default() };
        assert!(busy.fits(decode_cost(Some(512))));
        assert!(!busy.fits(decode_cost(None)));
        assert!(Jobs::default().fits(decode_cost(None)), "one decode runs whatever it costs");
        let ctx = ctx();
        let uris = covers("budget", 6, 300);
        show(&ctx, &uris, 64.0);
        let s = shared(&ctx).unwrap();
        for _ in 0..5000 {
            if lock(&s.jobs).workers == 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let jobs = lock(&s.jobs);
        assert_eq!((jobs.workers, jobs.in_flight), (0, 0));
    }
}
