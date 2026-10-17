//! Covers of streaming services (`https://` urls) for egui's image loaders.
//!
//! A bytes loader: the image loader egui_extras installs decodes what it
//! returns. Fetches run on a small tokio runtime of their own, never on
//! the UI thread; a failed url is remembered so it is not asked again
//! every frame (the page then draws its placeholder).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use egui::load::{Bytes, BytesLoadResult, BytesLoader, BytesPoll, LoadError};

/// Bytes kept at most; the oldest images are dropped past it (egui keeps
/// their decoded textures as long as they are drawn).
const BUDGET: usize = 48 << 20;
/// An image larger than this is not a cover.
const MAX_BYTES: usize = 8 << 20;

enum Slot {
    Pending,
    Ready { bytes: Arc<[u8]>, mime: Option<String> },
    Failed(String),
}

#[derive(Default)]
struct Cache {
    slots: HashMap<String, Slot>,
    /// Ready urls, oldest first.
    order: Vec<String>,
    size: usize,
}

pub struct WebImages {
    cache: Arc<Mutex<Cache>>,
    rt: tokio::runtime::Runtime,
    http: reqwest::Client,
}

fn lock(m: &Mutex<Cache>) -> MutexGuard<'_, Cache> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl WebImages {
    pub const ID: &'static str = egui::generate_loader_id!(WebImages);

    /// Registers the loader with `ctx`; without a runtime or a client
    /// covers simply stay placeholders.
    pub fn install(ctx: &egui::Context) {
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
        ctx.add_bytes_loader(Arc::new(Self { cache: Arc::default(), rt, http }));
    }

    fn fetch(&self, ctx: &egui::Context, uri: &str) {
        let (cache, http, ctx, uri) = (Arc::clone(&self.cache), self.http.clone(), ctx.clone(), uri.to_owned());
        self.rt.spawn(async move {
            let result = async {
                let resp = http
                    .get(&uri)
                    .header("Accept", "image/jpeg,image/png;q=0.9,*/*;q=0.1")
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;
                if !resp.status().is_success() {
                    return Err(format!("HTTP {}", resp.status().as_u16()));
                }
                let mime = resp
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
                if bytes.len() > MAX_BYTES {
                    return Err("too large".to_owned());
                }
                Ok((bytes, mime))
            }
            .await;
            let mut c = lock(&cache);
            match result {
                Ok((bytes, mime)) => {
                    c.size += bytes.len();
                    c.order.push(uri.clone());
                    c.slots.insert(uri, Slot::Ready { bytes: Arc::from(&bytes[..]), mime });
                    while c.size > BUDGET && c.order.len() > 1 {
                        let old = c.order.remove(0);
                        if let Some(Slot::Ready { bytes, .. }) = c.slots.remove(&old) {
                            c.size -= bytes.len();
                        }
                    }
                }
                Err(e) => {
                    log::debug!("web cover {uri}: {e}");
                    c.slots.insert(uri, Slot::Failed(e));
                }
            }
            drop(c);
            ctx.request_repaint();
        });
    }
}

impl BytesLoader for WebImages {
    fn id(&self) -> &str {
        Self::ID
    }

    fn load(&self, ctx: &egui::Context, uri: &str) -> BytesLoadResult {
        if !(uri.starts_with("https://") || uri.starts_with("http://")) {
            return Err(LoadError::NotSupported);
        }
        let mut c = lock(&self.cache);
        match c.slots.get(uri) {
            Some(Slot::Ready { bytes, mime }) => Ok(BytesPoll::Ready {
                size: None,
                bytes: Bytes::Shared(Arc::clone(bytes)),
                mime: mime.clone(),
            }),
            Some(Slot::Failed(e)) => Err(LoadError::Loading(e.clone())),
            Some(Slot::Pending) => Ok(BytesPoll::Pending { size: None }),
            None => {
                c.slots.insert(uri.to_owned(), Slot::Pending);
                drop(c);
                self.fetch(ctx, uri);
                Ok(BytesPoll::Pending { size: None })
            }
        }
    }

    fn forget(&self, uri: &str) {
        let mut c = lock(&self.cache);
        if let Some(Slot::Ready { bytes, .. }) = c.slots.remove(uri) {
            c.size -= bytes.len();
            c.order.retain(|u| u != uri);
        }
    }

    fn forget_all(&self) {
        *lock(&self.cache) = Cache::default();
    }

    fn byte_size(&self) -> usize {
        lock(&self.cache).size
    }

    fn has_pending(&self) -> bool {
        lock(&self.cache).slots.values().any(|s| matches!(s, Slot::Pending))
    }
}
