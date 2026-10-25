//! Covers of streaming services (`https://` urls) for egui's image loaders.
//!
//! A bytes loader: md3e's cover loader decodes what it returns at the size
//! the cover is drawn at and then forgets it here, so the bytes stay in
//! memory only until then (the disk keeps them). A cover is looked up in memory, then on disk
//! ([`DiskCache`], `data/cache/covers`), then downloaded, at most
//! [`PARALLEL`] at a time, the most recently drawn first; covers that are no
//! longer drawn before their turn are dropped from the queue (md3e's cover
//! LRU forgets them). A failed download is asked again after [`RETRY`]
//! unless the server said the image is gone (404 / 410). Finished covers
//! ask for one repaint per batch, not one each.
//!
//! YouTube Music covers (Google image urls with `=w…-h…` size parameters)
//! are asked at the size they are drawn at ([`sized_url`], given to md3e as
//! its web sizer); Spotify's urls cannot be resized and are kept.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use egui::load::{Bytes, BytesLoadResult, BytesLoader, BytesPoll, LoadError};

use crate::cover_cache::DiskCache;

/// Bytes kept in memory at most; the oldest images are dropped past it.
/// Covers leave once decoded, so this only bounds the ones not decoded yet.
const BUDGET: usize = 8 << 20;
/// An image larger than this is not a cover.
const MAX_BYTES: usize = 8 << 20;
/// Downloads at once.
pub const PARALLEL: usize = 6;
/// A failed download is tried again after this long.
const RETRY: Duration = Duration::from_secs(60);
/// Finished covers within this window share one repaint.
const REPAINT_BATCH: Duration = Duration::from_millis(40);

enum Slot {
    /// Being read from the disk cache.
    Reading,
    /// Waiting for a download slot; `seen` is the last time it was drawn.
    Queued { seen: Instant },
    Fetching,
    Ready { bytes: Arc<[u8]> },
    Failed { error: String, at: Instant, gone: bool },
}

#[derive(Default)]
struct Counts {
    disk: u32,
    net: u32,
    failed: u32,
    logged: (u32, u32, u32),
}

#[derive(Default)]
struct State {
    slots: HashMap<String, Slot>,
    /// Ready urls, oldest first.
    order: VecDeque<String>,
    size: usize,
    /// Urls in `Slot::Queued`.
    queue: Vec<String>,
    running: usize,
    counts: Counts,
}

struct Inner {
    state: Mutex<State>,
    http: reqwest::Client,
    disk: Option<DiskCache>,
    ctx: egui::Context,
    /// A repaint is already asked for the covers that finished.
    repaint_asked: AtomicBool,
}

pub struct WebImages {
    inner: Arc<Inner>,
    rt: tokio::runtime::Runtime,
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl WebImages {
    pub const ID: &'static str = egui::generate_loader_id!(WebImages);

    /// Registers the loader with `ctx`, caching covers in `cache_dir`;
    /// without a runtime or a client covers simply stay placeholders.
    pub fn install(ctx: &egui::Context, cache_dir: PathBuf) {
        let rt = match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("web-images")
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                log::warn!("no runtime for web covers: {e}");
                return;
            }
        };
        let http = match reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(20))
            .user_agent(concat!("nikgob-music/", env!("CARGO_PKG_VERSION")))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                log::warn!("no http client for web covers: {e}");
                return;
            }
        };
        let disk = match DiskCache::open(&cache_dir, crate::cover_cache::DEFAULT_LIMIT) {
            Ok(d) => {
                log::info!("web covers: disk cache {} ({} KB)", d.dir().display(), d.size() >> 10);
                Some(d)
            }
            Err(e) => {
                log::warn!("web covers: no disk cache at {}: {e}", cache_dir.display());
                None
            }
        };
        let inner = Arc::new(Inner {
            state: Mutex::default(),
            http,
            disk,
            ctx: ctx.clone(),
            repaint_asked: AtomicBool::new(false),
        });
        let me = Arc::clone(&inner);
        ctx.on_begin_pass("web-images", Arc::new(move |_| me.repaint_asked.store(false, Ordering::Relaxed)));
        nikgob_md3e::covers::set_web_sizer(ctx, sized_url);
        ctx.add_bytes_loader(Arc::new(Self { inner, rt }));
    }
}

impl Inner {
    /// One repaint for every cover that finishes before the next pass.
    fn repaint(&self) {
        if !self.repaint_asked.swap(true, Ordering::Relaxed) {
            self.ctx.request_repaint_after(REPAINT_BATCH);
        }
    }

    fn ready(&self, s: &mut State, uri: String, bytes: Arc<[u8]>) {
        s.size += bytes.len();
        s.order.push_back(uri.clone());
        s.slots.insert(uri, Slot::Ready { bytes });
        while s.size > BUDGET && s.order.len() > 1 {
            let Some(old) = s.order.pop_front() else { break };
            if let Some(Slot::Ready { bytes }) = s.slots.remove(&old) {
                s.size -= bytes.len();
            }
        }
    }

    /// Starts downloads while there are free slots, the most recently drawn
    /// cover first.
    fn pump(self: &Arc<Self>, rt: &tokio::runtime::Handle) {
        let mut s = lock(&self.state);
        while s.running < PARALLEL && !s.queue.is_empty() {
            let seen = |s: &State, u: &str| match s.slots.get(u) {
                Some(Slot::Queued { seen }) => Some(*seen),
                _ => None,
            };
            let Some(i) = (0..s.queue.len()).max_by_key(|&i| seen(&s, &s.queue[i])) else { break };
            let uri = s.queue.swap_remove(i);
            if seen(&s, &uri).is_none() {
                continue;
            }
            s.slots.insert(uri.clone(), Slot::Fetching);
            s.running += 1;
            let (me, h) = (Arc::clone(self), rt.clone());
            rt.spawn(async move {
                let result = me.download(&uri).await;
                if let Ok(bytes) = &result
                    && me.disk.is_some()
                {
                    let (me2, uri2, bytes2) = (Arc::clone(&me), uri.clone(), Arc::clone(bytes));
                    // Off the async threads; the cover shows meanwhile.
                    h.spawn_blocking(move || {
                        if let Some(disk) = &me2.disk
                            && let Err(e) = disk.put(&uri2, &bytes2)
                        {
                            log::debug!("web cover {uri2}: not cached: {e}");
                        }
                    });
                }
                {
                    let mut s = lock(&me.state);
                    s.running -= 1;
                    match result {
                        Ok(bytes) => {
                            s.counts.net += 1;
                            me.ready(&mut s, uri, bytes);
                        }
                        Err((error, gone)) => {
                            log::debug!("web cover {uri}: {error}");
                            s.counts.failed += 1;
                            s.slots.insert(uri, Slot::Failed { error, at: Instant::now(), gone });
                        }
                    }
                    me.log_when_idle(&mut s);
                }
                me.repaint();
                me.pump(&h);
            });
        }
    }

    /// The image at `uri`; the error says whether it is gone for good.
    async fn download(&self, uri: &str) -> Result<Arc<[u8]>, (String, bool)> {
        let resp = self
            .http
            .get(uri)
            .header("Accept", "image/jpeg,image/png;q=0.9,*/*;q=0.1")
            .send()
            .await
            .map_err(|e| (e.to_string(), false))?;
        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            return Err((format!("HTTP {status}"), matches!(status, 404 | 410)));
        }
        let bytes = resp.bytes().await.map_err(|e| (e.to_string(), false))?;
        if bytes.len() > MAX_BYTES {
            return Err(("too large".to_owned(), true));
        }
        Ok(Arc::from(&bytes[..]))
    }

    /// A line in the log each time the covers asked so far are all in.
    fn log_when_idle(&self, s: &mut State) {
        let c = &s.counts;
        let now = (c.disk, c.net, c.failed);
        let busy = s.running > 0
            || s.slots.values().any(|x| matches!(x, Slot::Reading | Slot::Queued { .. } | Slot::Fetching));
        if !busy && now != c.logged {
            log::info!(
                "web covers: {} from disk, {} downloaded, {} failed so far (disk cache {} KB)",
                c.disk,
                c.net,
                c.failed,
                self.disk.as_ref().map_or(0, |d| d.size() >> 10)
            );
            s.counts.logged = now;
        }
    }
}

impl BytesLoader for WebImages {
    fn id(&self) -> &str {
        Self::ID
    }

    fn load(&self, _ctx: &egui::Context, uri: &str) -> BytesLoadResult {
        if !(uri.starts_with("https://") || uri.starts_with("http://")) {
            return Err(LoadError::NotSupported);
        }
        let inner = &self.inner;
        let mut s = lock(&inner.state);
        match s.slots.get_mut(uri) {
            Some(Slot::Ready { bytes }) => {
                return Ok(BytesPoll::Ready { size: None, bytes: Bytes::Shared(Arc::clone(bytes)), mime: None });
            }
            Some(Slot::Failed { error, at, gone }) if *gone || at.elapsed() < RETRY => {
                return Err(LoadError::Loading(error.clone()));
            }
            Some(Slot::Queued { seen }) => {
                *seen = Instant::now();
                return Ok(BytesPoll::Pending { size: None });
            }
            Some(Slot::Reading | Slot::Fetching) => return Ok(BytesPoll::Pending { size: None }),
            // Never asked, or failed long enough ago to try again.
            Some(Slot::Failed { .. }) | None => {}
        }
        let uri = uri.to_owned();
        // The demo's fake catalog: a picture made up from the url.
        if uri.starts_with("https://fake.invalid/") || uri.starts_with("https://i.ytimg.com/vi/yt-") {
            let bytes = fake_cover(&uri);
            inner.ready(&mut s, uri, bytes.clone());
            return Ok(BytesPoll::Ready { size: None, bytes: Bytes::Shared(bytes), mime: None });
        }
        if inner.disk.is_some() {
            s.slots.insert(uri.clone(), Slot::Reading);
            drop(s);
            let (me, h) = (Arc::clone(inner), self.rt.handle().clone());
            self.rt.spawn_blocking(move || {
                let hit = me.disk.as_ref().and_then(|d| d.get(&uri));
                let mut s = lock(&me.state);
                // Forgotten meanwhile: nothing to do.
                if !matches!(s.slots.get(&uri), Some(Slot::Reading)) {
                    return;
                }
                match hit {
                    Some(bytes) => {
                        s.counts.disk += 1;
                        me.ready(&mut s, uri, Arc::from(bytes));
                        me.log_when_idle(&mut s);
                        drop(s);
                        me.repaint();
                    }
                    None => {
                        s.slots.insert(uri.clone(), Slot::Queued { seen: Instant::now() });
                        s.queue.push(uri);
                        drop(s);
                        me.pump(&h);
                    }
                }
            });
        } else {
            s.slots.insert(uri.clone(), Slot::Queued { seen: Instant::now() });
            s.queue.push(uri);
            drop(s);
            inner.pump(self.rt.handle());
        }
        Ok(BytesPoll::Pending { size: None })
    }

    /// A cover no longer drawn: its bytes leave memory (the disk keeps
    /// them), and if it still waits for a download it leaves the queue.
    fn forget(&self, uri: &str) {
        let mut s = lock(&self.inner.state);
        match s.slots.get(uri) {
            Some(Slot::Ready { .. }) => {
                if let Some(Slot::Ready { bytes }) = s.slots.remove(uri) {
                    s.size -= bytes.len();
                    s.order.retain(|u| u != uri);
                }
            }
            Some(Slot::Queued { .. } | Slot::Reading) => {
                s.slots.remove(uri);
                s.queue.retain(|u| u != uri);
            }
            _ => {}
        }
    }

    fn forget_all(&self) {
        let mut s = lock(&self.inner.state);
        s.slots.retain(|_, x| matches!(x, Slot::Fetching | Slot::Failed { gone: true, .. }));
        s.order.clear();
        s.queue.clear();
        s.size = 0;
    }

    fn byte_size(&self) -> usize {
        lock(&self.inner.state).size
    }

    fn has_pending(&self) -> bool {
        lock(&self.inner.state)
            .slots
            .values()
            .any(|s| matches!(s, Slot::Reading | Slot::Queued { .. } | Slot::Fetching))
    }
}

/// A cover for a url of the demo's fake catalog (`https://fake.invalid/…`):
/// a diagonal gradient between two colours taken from the url's hash.
fn fake_cover(url: &str) -> Arc<[u8]> {
    let h = blake3::hash(url.as_bytes());
    let b = h.as_bytes();
    let (a, c) = (image::Rgb([b[0], b[1], b[2]]), image::Rgb([b[3] / 2, b[4] / 2, b[5] / 2]));
    let img = image::RgbImage::from_fn(300, 300, |x, y| {
        let t = (x + y) as f32 / 598.0;
        image::Rgb(std::array::from_fn(|i| (f32::from(a.0[i]) * (1.0 - t) + f32::from(c.0[i]) * t) as u8))
    });
    let mut out = std::io::Cursor::new(Vec::new());
    let _ = img.write_to(&mut out, image::ImageFormat::Png);
    Arc::from(out.into_inner())
}

/// The url of a web cover for drawing at `tier` pixels (`None`: as large as
/// it gets): Google-hosted covers (YouTube Music: `lh3.googleusercontent.com`,
/// `yt3.ggpht.com`, `…=w544-h544-l90-rj`) at 226 px for rows and cards, 544
/// for a medium cover, 1200 for Now Playing and page headers; Deezer's
/// (`…dzcdn.net/…/1000x1000-…`) at 250, 500 and 1000. Other urls (Spotify,
/// video thumbnails) cannot be resized: `None`, drawn as they are.
pub fn sized_url(url: &str, tier: Option<u32>) -> Option<String> {
    if let Some(sized) = deezer_sized(url, tier) {
        return (sized != url).then_some(sized);
    }
    let px = match tier {
        Some(t) if t <= 256 => 226,
        Some(t) if t <= 544 => 544,
        _ => 1200,
    };
    let rest = url.strip_prefix("https://")?;
    let host = rest.split('/').next()?;
    if !(host.ends_with(".googleusercontent.com") || host.ends_with(".ggpht.com")) {
        return None;
    }
    let (base, params) = url.rsplit_once('=')?;
    if base.len() <= "https://".len() + host.len() || params.contains('/') {
        return None;
    }
    let parts: Vec<&str> = params.split('-').collect();
    let size = |p: &str, c: char| p.strip_prefix(c).is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    let (has_w, has_h) = (parts.iter().any(|p| size(p, 'w')), parts.iter().any(|p| size(p, 'h')));
    let has_s = parts.iter().any(|p| size(p, 's'));
    if !(has_w && has_h || has_s) {
        return None;
    }
    let mut out = format!("{base}=w{px}-h{px}");
    for p in parts.iter().filter(|p| !(size(p, 'w') || size(p, 'h') || size(p, 's'))) {
        out.push('-');
        out.push_str(p);
    }
    (out != url).then_some(out)
}

/// A Deezer cover (`https://cdn-images.dzcdn.net/images/cover/{md5}/1000x1000-000000-80-0-0.jpg`,
/// also `e-cdns-images.dzcdn.net`, artists and playlists alike) at the size
/// it is drawn at: 250 px for rows and cards, 500 medium, 1000 for Now
/// Playing and page headers. `None`: not a Deezer image url.
fn deezer_sized(url: &str, tier: Option<u32>) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let host = rest.split('/').next()?;
    if !host.ends_with(".dzcdn.net") {
        return None;
    }
    let (base, file) = url.rsplit_once('/')?;
    let (size, tail) = file.split_once('-')?;
    let (w, h) = size.split_once('x')?;
    if w.is_empty() || w != h || !w.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let px = match tier {
        Some(t) if t <= 256 => 250,
        Some(t) if t <= 544 => 500,
        _ => 1000,
    };
    Some(format!("{base}/{px}x{px}-{tail}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deezer_covers_are_asked_at_the_size_they_are_drawn_at() {
        let u = "https://cdn-images.dzcdn.net/images/cover/2e018122cb56986277102d2041a592c8/1000x1000-000000-80-0-0.jpg";
        assert_eq!(
            sized_url(u, Some(128)).as_deref(),
            Some("https://cdn-images.dzcdn.net/images/cover/2e018122cb56986277102d2041a592c8/250x250-000000-80-0-0.jpg")
        );
        assert_eq!(
            sized_url(u, Some(400)).as_deref(),
            Some("https://cdn-images.dzcdn.net/images/cover/2e018122cb56986277102d2041a592c8/500x500-000000-80-0-0.jpg")
        );
        assert_eq!(sized_url(u, None), None, "already 1000");
        let artist = "https://e-cdns-images.dzcdn.net/images/artist/f2bc007e9133c946ac3c3907ddc5d2ea/500x500-000000-80-0-0.jpg";
        assert!(sized_url(artist, Some(200)).is_some_and(|s| s.contains("/250x250-000000-80-0-0.jpg")));
        assert_eq!(sized_url("https://cdn-images.dzcdn.net/images/misc/logo.png", Some(100)), None);
    }

    #[test]
    fn youtube_covers_are_asked_at_the_size_they_are_drawn_at() {
        let u = "https://lh3.googleusercontent.com/abc=w544-h544-l90-rj";
        assert_eq!(sized_url(u, Some(128)).as_deref(), Some("https://lh3.googleusercontent.com/abc=w226-h226-l90-rj"));
        assert_eq!(sized_url(u, Some(256)).as_deref(), Some("https://lh3.googleusercontent.com/abc=w226-h226-l90-rj"));
        assert_eq!(sized_url(u, Some(512)), None, "already 544");
        assert_eq!(sized_url(u, None).as_deref(), Some("https://lh3.googleusercontent.com/abc=w1200-h1200-l90-rj"));
        let a = "https://yt3.ggpht.com/xyz=s120-c-k-c0x00ffffff-no-rj";
        assert_eq!(sized_url(a, Some(256)).as_deref(), Some("https://yt3.ggpht.com/xyz=w226-h226-c-k-c0x00ffffff-no-rj"));
    }

    #[test]
    fn other_covers_are_kept() {
        for u in [
            "https://i.scdn.co/image/ab67616d0000b273abcdef",
            "https://i.ytimg.com/vi/x/sddefault.jpg",
            "https://lh3.googleusercontent.com/abc",
            "https://lh3.googleusercontent.com/abc=l90-rj",
            "https://evil.example/googleusercontent.com/a=w60-h60",
            "http://lh3.googleusercontent.com/abc=w60-h60",
        ] {
            assert_eq!(sized_url(u, Some(256)), None, "{u}");
        }
    }
}
