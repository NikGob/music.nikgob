//! khinsider (downloads.khinsider.com) as a [`Source`]: game soundtracks,
//! often in FLAC, as plain pages. Asked only for a soundtrack (the album
//! says so, or the song names the game it was made for): its search page
//! lists albums, an album page lists the songs with their CD, number,
//! length and sizes per format, and a song's page links the files.
//!
//! The site sits behind a bot check. Page requests carry a browser's
//! navigation headers; if the answer is still the check, and a browser was
//! given ([`Khinsider::set_browser`]), that browser reads the HTML once
//! (its own profile, no cookies copied into the HTTP client). The files
//! stay on [`crate::http::download`] from the CDN. Still a check: the
//! existing pause, and no second try. One request every two seconds.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nikgob_core::indexer::BoxFuture;
use nikgob_core::model::Codec;
use nikgob_core::source::{Fetched, Format, Offer, OfferScope, Progress, Source, SourceError, SourceKey, Want, WantScope};
use nikgob_metadata::text::{similarity, title_key};
use serde::{Deserialize, Serialize};

use crate::album::{Patience, fetch_each, left_out};
use crate::html::{Cell, links, table, text_of};
use crate::http::{Breaker, Cookies, Nav, Pace, client, download, get_page_nav, is_check, safe_name, BOT_CHECK};
use crate::matching::contains_words;
use crate::text::{album_core, album_keys, franchise_of, is_soundtrack, title_keys};

pub const DEFAULT_URL: &str = "https://downloads.khinsider.com";
/// Albums of a search looked into.
const ALBUMS: usize = 3;
/// A song title this alike is the track.
const TITLE: f64 = 0.85;

/// Reads one page's HTML after a check only a browser passes. The cookies
/// it earns stay in its profile; they are not given to the HTTP client.
/// [`SourceError::NotConfigured`] means no webview can be opened (Linux
/// without WebKit): the check stands, and nothing is tried again.
pub trait PageBrowser: Send + Sync {
    fn document<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<String, SourceError>>;
}

pub struct Khinsider {
    base: String,
    http: reqwest::Client,
    pace: Pace,
    breaker: Breaker,
    cookies: Cookies,
    browser: Mutex<Option<Arc<dyn PageBrowser>>>,
}

impl Khinsider {
    /// `base` empty: [`DEFAULT_URL`].
    pub fn new(base: &str) -> Self {
        Self::with_pace(base, Duration::from_secs(2))
    }

    /// With another pace (tests).
    pub fn with_pace(base: &str, every: Duration) -> Self {
        let base = if base.trim().is_empty() { DEFAULT_URL } else { base.trim() };
        Self {
            base: base.trim_end_matches('/').to_owned(),
            http: client(),
            pace: Pace::new(every),
            breaker: Breaker::default(),
            cookies: Cookies::default(),
            browser: Mutex::new(None),
        }
    }

    /// The hidden webview the app opens when a page is still a check.
    /// None (or a browser that says it is not there) leaves the HTTP answer.
    pub fn set_browser(&self, browser: Arc<dyn PageBrowser>) {
        *self.browser.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(browser);
    }

    fn url(&self, href: &str) -> String {
        if href.starts_with("http://") || href.starts_with("https://") {
            href.to_owned()
        } else {
            format!("{}/{}", self.base, href.trim_start_matches('/'))
        }
    }

    async fn page(&self, url: &str) -> Result<String, SourceError> {
        self.breaker.check()?;
        self.pace.wait().await;
        let http = get_page_nav(&self.http, url, Some(&self.cookies), nav_for(url, &self.base)).await;
        let browser = self.browser.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        let r = finish_page(http, browser.as_deref(), url).await;
        self.breaker.note(&r);
        r
    }
}

/// The path of `url`, without its query.
fn path_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let path = rest.find('/').map_or("/", |i| &rest[i..]);
    path.split(['?', '#']).next().unwrap_or(path)
}

/// Navigation headers for one khinsider page. The first visit (the site,
/// a search) is `none`; an album or a song is `same-origin` with the site
/// as its referer.
pub fn nav_for(url: &str, base: &str) -> Nav {
    let path = path_of(url);
    if path == "/" || path.is_empty() || path.starts_with("/search") {
        Nav { site: "none", referer: None }
    } else {
        Nav { site: "same-origin", referer: Some(format!("{}/", base.trim_end_matches('/'))) }
    }
}

/// A Cloudflare check page, whatever status it came with.
pub fn is_challenge_html(html: &str) -> bool {
    html.contains("Just a moment") || html.contains("cf-chl") || html.contains("challenge-platform") || html.contains("Attention Required")
}

fn as_check(http: Result<String, SourceError>) -> Result<String, SourceError> {
    match http {
        Err(e) if is_check(&e) => Err(e),
        _ => Err(SourceError::Blocked(BOT_CHECK.into())),
    }
}

/// What a page answer becomes. A check, and a browser: ask it once. A
/// check and no browser, or a browser that is not there: the check stands.
/// A page that is not a check is used as it is, and the browser is not asked.
pub async fn finish_page(http: Result<String, SourceError>, browser: Option<&dyn PageBrowser>, url: &str) -> Result<String, SourceError> {
    let check = match &http {
        Err(e) if is_check(e) => true,
        Ok(html) if is_challenge_html(html) => true,
        _ => false,
    };
    if !check {
        return http;
    }
    let Some(browser) = browser else { return as_check(http) };
    match browser.document(url).await {
        Err(SourceError::NotConfigured) => as_check(http),
        other => after_browser(other),
    }
}

/// The browser's one answer. A page that is still a check is the pause;
/// there is no second ask.
pub fn after_browser(got: Result<String, SourceError>) -> Result<String, SourceError> {
    match got {
        Ok(html) if !is_challenge_html(&html) => Ok(html),
        Ok(_) => Err(SourceError::Blocked(BOT_CHECK.into())),
        Err(e) if is_check(&e) => Err(e),
        Err(_) => Err(SourceError::Blocked(BOT_CHECK.into())),
    }
}

/// What a fetch needs.
#[derive(Serialize, Deserialize)]
struct Handle {
    /// A song's page (a track offer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    song: Option<String>,
    /// The album's page (an album offer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    album: Option<String>,
}

/// An album of a search page: its page and name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlbumLink {
    pub href: String,
    pub name: String,
}

/// The albums of a search page, as listed.
pub fn search_results(html: &str) -> Vec<AlbumLink> {
    let mut out: Vec<AlbumLink> = Vec::new();
    for row in table(html, "albumList") {
        // The album's name is the cell whose link goes to it with text.
        for c in &row {
            if let Some(h) = &c.href
                && h.contains("/game-soundtracks/album/")
                && !c.text.is_empty()
                && !out.iter().any(|a| &a.href == h)
            {
                out.push(AlbumLink { href: h.clone(), name: c.text.clone() });
                break;
            }
        }
    }
    if out.is_empty() {
        // A search with one hit may land on the album's own page; other
        // layouts: any album link with a name.
        for (h, t) in links(html) {
            if h.contains("/game-soundtracks/album/") && !t.is_empty() && !out.iter().any(|a| a.href == h) {
                out.push(AlbumLink { href: h, name: t });
            }
        }
    }
    out
}

/// A song of an album page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Song {
    /// The song's page.
    pub href: String,
    pub name: String,
    pub disc: u32,
    pub number: u32,
    pub duration_ms: Option<u32>,
    /// Size of the FLAC, when the album has one.
    pub flac_bytes: Option<u64>,
    /// Size of the lossy file and its codec (MP3, else OGG or M4A).
    pub lossy: Option<(Codec, u64)>,
}

/// The album page: its name and songs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AlbumPage {
    pub name: String,
    pub songs: Vec<Song>,
}

/// "22.1 MB", "980 KB", "1.2 GB".
fn size_of(s: &str) -> Option<u64> {
    let mut it = s.split_whitespace();
    let n: f64 = it.next()?.replace(',', "").parse().ok()?;
    let mult = match it.next()?.to_ascii_uppercase().as_str() {
        "KB" => 1024.0,
        "MB" => 1024.0 * 1024.0,
        "GB" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((n * mult) as u64)
}

/// "3:05", "1:02:03".
fn duration_of(s: &str) -> Option<u32> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    if parts.len() < 2 || parts.len() > 3 || parts.iter().any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    let secs = parts.iter().fold(0u32, |acc, p| acc * 60 + p.parse::<u32>().unwrap_or(0));
    Some(secs * 1000)
}

/// The column each cell of a row starts at (cells span columns).
fn columns(row: &[Cell]) -> Vec<usize> {
    let mut at = 0;
    row.iter()
        .map(|c| {
            let start = at;
            at += c.span;
            start
        })
        .collect()
}

/// The album page's name and songs. Columns are found by their headers,
/// spans counted («Song Name» covers two columns: the name and the
/// length); the CD column is there only for albums of several discs, the
/// formats are whatever the album has (MP3, FLAC, OGG, M4A).
pub fn album_page(html: &str) -> AlbumPage {
    let name = html
        .find("<h2>")
        .and_then(|a| html[a + 4..].find("</h2>").map(|b| text_of(&html[a + 4..a + 4 + b])))
        .unwrap_or_default();
    let rows = table(html, "songlist");
    let header: Vec<(usize, String)> = rows
        .iter()
        .find(|r| r.iter().any(|c| c.head))
        .map(|r| columns(r).into_iter().zip(r.iter().map(|c| c.text.to_lowercase())).collect())
        .unwrap_or_default();
    let col = |name: &str| header.iter().find(|(_, h)| h == name).map(|(at, _)| *at);
    let (cd, num, title, flac) = (col("cd"), col("#"), col("song name"), col("flac"));
    let lossy: Vec<(Codec, usize)> = [("mp3", Codec::Mp3), ("ogg", Codec::Vorbis), ("m4a", Codec::Aac), ("aac", Codec::Aac)]
        .into_iter()
        .filter_map(|(h, c)| col(h).map(|at| (c, at)))
        .collect();
    let mut songs = Vec::new();
    for row in rows.iter().filter(|r| !r.iter().any(|c| c.head)) {
        let starts = columns(row);
        let cell = |i: Option<usize>| -> Option<&Cell> {
            let i = i?;
            starts.iter().position(|s| *s == i).and_then(|k| row.get(k))
        };
        // The song's name: its column, else the longest linked text that
        // is not a number, a length or a size.
        let name_cell = cell(title).filter(|c| !c.text.is_empty()).or_else(|| {
            row.iter()
                .filter(|c| c.href.is_some() && duration_of(&c.text).is_none() && size_of(&c.text).is_none())
                .filter(|c| !c.text.trim_end_matches('.').chars().all(|ch| ch.is_ascii_digit()))
                .max_by_key(|c| c.text.len())
        });
        let Some(name_cell) = name_cell else { continue };
        let Some(href) = name_cell.href.clone().or_else(|| row.iter().find_map(|c| c.href.clone())) else { continue };
        if name_cell.text.is_empty() || !href.contains("/game-soundtracks/album/") {
            continue;
        }
        let number = |c: Option<&Cell>| c.and_then(|c| c.text.trim_end_matches('.').trim().parse::<u32>().ok());
        let lossy_file = lossy
            .iter()
            .find_map(|(codec, at)| cell(Some(*at)).and_then(|c| size_of(&c.text)).map(|b| (codec.clone(), b)));
        songs.push(Song {
            href,
            name: name_cell.text.clone(),
            disc: number(cell(cd)).unwrap_or(1),
            number: number(cell(num)).unwrap_or(0),
            duration_ms: row.iter().find_map(|c| duration_of(&c.text)),
            flac_bytes: cell(flac).and_then(|c| size_of(&c.text)),
            lossy: lossy_file,
        });
    }
    AlbumPage { name, songs }
}

/// The files a song's page links: (url, is FLAC), FLAC first, then MP3,
/// then the other lossy ones.
pub fn song_files(html: &str) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool, u8)> = Vec::new();
    for (h, _) in links(html) {
        let path = h.split('?').next().unwrap_or(&h).to_lowercase();
        let rank = if path.ends_with(".flac") {
            0
        } else if path.ends_with(".mp3") {
            1
        } else if path.ends_with(".m4a") || path.ends_with(".ogg") {
            2
        } else {
            continue;
        };
        if !h.contains("/game-soundtracks/album/") && !out.iter().any(|(u, ..)| *u == h) {
            out.push((h, rank == 0, rank));
        }
    }
    out.sort_by_key(|(.., r)| *r);
    out.into_iter().map(|(u, f, _)| (u, f)).collect()
}

/// A file name from a file's URL ("01.%20Title.flac" → "01. Title.flac").
fn file_name(url: &str) -> String {
    let last = url.split('?').next().unwrap_or(url).rsplit('/').next().unwrap_or("track");
    safe_name(&crate::http::percent_decode(last))
}

/// What the wish is a soundtrack of, as a search: the album's own name,
/// or the game the song was made for.
fn query_of(want: &Want) -> Option<String> {
    let franchise = want.title.as_deref().and_then(franchise_of).or_else(|| want.album.as_deref().and_then(franchise_of));
    franchise.or_else(|| want.album.as_deref().filter(|a| is_soundtrack(a)).map(album_core))
}

fn names_like(name: &str, keys: &[String]) -> bool {
    let got = title_key(&album_core(name)).base;
    let full = title_key(name).base;
    keys.iter().any(|k| similarity(&got, k) >= 0.9 || contains_words(&full, k))
}

impl Source for Khinsider {
    fn key(&self) -> SourceKey {
        SourceKey::Khinsider
    }

    fn name(&self) -> &str {
        "khinsider"
    }

    fn search<'a>(&'a self, want: &'a Want) -> BoxFuture<'a, Result<Vec<Offer>, SourceError>> {
        Box::pin(async move {
            let Some(q) = query_of(want) else { return Ok(Vec::new()) };
            let mut keys: Vec<String> = want.album.as_deref().map(album_keys).unwrap_or_default();
            keys.push(title_key(&q).base);
            let url = format!("{}/search?search={}", self.base, url::form_urlencoded::byte_serialize(q.as_bytes()).collect::<String>());
            let html = self.page(&url).await?;
            let albums: Vec<AlbumLink> = search_results(&html).into_iter().filter(|a| names_like(&a.name, &keys)).take(ALBUMS).collect();
            let mut out = Vec::new();
            for a in albums {
                let page_url = self.url(&a.href);
                let page = album_page(&self.page(&page_url).await?);
                let name = if page.name.is_empty() { a.name.clone() } else { page.name.clone() };
                let has_flac = page.songs.iter().any(|s| s.flac_bytes.is_some());
                let lossy_codec = page.songs.iter().find_map(|s| s.lossy.as_ref().map(|(c, _)| c.clone())).unwrap_or(Codec::Mp3);
                let format = |flac: bool| Some(Format::new(if flac { Codec::Flac } else { lossy_codec.clone() }));
                match want.scope {
                    WantScope::Album => {
                        let size: u64 = page
                            .songs
                            .iter()
                            .map(|s| if has_flac { s.flac_bytes } else { s.lossy.as_ref().map(|(_, b)| *b) }.unwrap_or(0))
                            .sum();
                        out.push(Offer {
                            source: SourceKey::Khinsider,
                            provider: "khinsider".into(),
                            id: a.href.clone(),
                            title: name.clone(),
                            artist: None,
                            track_title: None,
                            album: Some(name.clone()),
                            duration_ms: None,
                            isrc: None,
                            scope: OfferScope::Album,
                            format: format(has_flac),
                            size_bytes: (size > 0).then_some(size),
                            seeders: None,
                            info_url: Some(page_url.clone()),
                            handle: serde_json::to_string(&Handle { song: None, album: Some(page_url.clone()) }).unwrap_or_default(),
                        });
                    }
                    WantScope::Track => {
                        let Some(title) = want.title.as_deref() else { continue };
                        let wanted = title_keys(title);
                        for s in &page.songs {
                            let sim = title_keys(&s.name)
                                .iter()
                                .flat_map(|g| wanted.iter().map(move |w| similarity(g, w)))
                                .fold(0.0, f64::max);
                            let numbered = want.track_no > 0 && s.number == want.track_no && s.disc == want.disc_no.max(1);
                            if sim < TITLE && !(numbered && sim >= 0.5) {
                                continue;
                            }
                            let song_url = self.url(&s.href);
                            out.push(Offer {
                                source: SourceKey::Khinsider,
                                provider: "khinsider".into(),
                                id: s.href.clone(),
                                title: format!("{name} — {}", s.name),
                                artist: None,
                                track_title: Some(s.name.clone()),
                                album: Some(name.clone()),
                                duration_ms: s.duration_ms,
                                isrc: None,
                                scope: OfferScope::Track,
                                format: format(s.flac_bytes.is_some()),
                                size_bytes: s.flac_bytes.or(s.lossy.as_ref().map(|(_, b)| *b)),
                                seeders: None,
                                info_url: Some(song_url.clone()),
                                handle: serde_json::to_string(&Handle { song: Some(song_url), album: None }).unwrap_or_default(),
                            });
                        }
                    }
                }
            }
            Ok(out)
        })
    }

    fn fetch<'a>(&'a self, offer: &'a Offer, want: &'a Want, dest: &'a Path, progress: Progress) -> BoxFuture<'a, Result<Fetched, SourceError>> {
        Box::pin(async move {
            let h: Handle = serde_json::from_str(&offer.handle).map_err(|e| SourceError::Parse(e.to_string()))?;
            let songs: Vec<(String, String)> = match (h.song, h.album) {
                (Some(s), _) => vec![(s, offer.track_title.clone().unwrap_or_else(|| offer.title.clone()))],
                (None, Some(a)) => {
                    let page = album_page(&self.page(&a).await?);
                    let all = page.songs.iter().map(|s| (self.url(&s.href), s.name.clone(), s.name.clone())).collect();
                    left_out(all, &want.got)
                }
                (None, None) => return Err(SourceError::NothingToFetch),
            };
            let lossless = offer.lossless();
            let before = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
            fetch_each(songs, Patience::default(), |song, _| {
                let (before, progress) = (before.clone(), progress.clone());
                async move {
                    use std::sync::atomic::Ordering;
                    let html = self.page(&song).await?;
                    let found = song_files(&html);
                    let Some((url, _)) = found.iter().find(|(_, flac)| *flac == lossless).or(found.first()).cloned() else {
                        return Err(SourceError::Parse("a song page without a file".into()));
                    };
                    let url = self.url(&url);
                    self.pace.wait().await;
                    let base = before.load(Ordering::Relaxed);
                    let step: Progress = std::sync::Arc::new(move |done, _| progress(base + done, None));
                    let path = download(&self.http, &url, &dest.join(file_name(&url)), &step).await?;
                    before.fetch_add(std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0), Ordering::Relaxed);
                    Ok(path)
                }
            })
            .await
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use nikgob_core::indexer::BoxFuture;
    use nikgob_core::source::SourceError;

    use super::*;
    use crate::http::{is_check, BOT_CHECK};
    #[test]
    fn sizes_lengths_and_names() {
        assert_eq!(size_of("22.1 MB"), Some((22.1 * 1024.0 * 1024.0) as u64));
        assert_eq!(size_of("980 KB"), Some(980 * 1024));
        assert_eq!(size_of("FLAC"), None);
        assert_eq!(duration_of("3:05"), Some(185_000));
        assert_eq!(duration_of("1:02:03"), Some(3_723_000));
        assert_eq!(duration_of("2019"), None);
        assert_eq!(file_name("https://x/soundtracks/a/b/01.%20Big%20Walk%20Theme%20%26%20Co.flac"), "01. Big Walk Theme & Co.flac");
    }

    #[test]
    fn only_soundtracks_are_asked() {
        let w = |album: &str, title: &str| Want {
            artists: vec!["x".into()],
            album: Some(album.into()),
            title: Some(title.into()),
            ..Want::default()
        };
        assert_eq!(query_of(&w("Secrets of the Silent Witch Original Soundtrack", "a")).as_deref(), Some("Secrets of the Silent Witch"));
        assert_eq!(
            query_of(&w("Das Nennt Ihr Himmel", "Das Nennt Ihr Himmel (Supraland Six Inches Under Credits Song)")).as_deref(),
            Some("Supraland Six Inches Under")
        );
        assert_eq!(query_of(&w("ТУЗИК", "Каблуки")), None);
    }

    struct Fake {
        html: Mutex<Result<String, SourceError>>,
        calls: std::sync::atomic::AtomicU32,
    }

    impl PageBrowser for Fake {
        fn document<'a>(&'a self, _url: &'a str) -> BoxFuture<'a, Result<String, SourceError>> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let r = self.html.lock().unwrap().clone();
            Box::pin(async move { r })
        }
    }

    fn calls(fake: &Fake) -> u32 {
        fake.calls.load(std::sync::atomic::Ordering::Relaxed)
    }

    #[test]
    fn a_check_page_and_a_later_page_are_told_apart() {
        assert!(is_challenge_html("<title>Just a moment...</title>"));
        assert!(is_challenge_html("cf-chl-bypass"));
        assert!(!is_challenge_html("Search - KHInsider Video Game Music"));
        let search = nav_for("https://downloads.khinsider.com/search?search=supraland", DEFAULT_URL);
        assert_eq!(search.site, "none");
        assert!(search.referer.is_none());
        let album = nav_for("https://downloads.khinsider.com/game-soundtracks/album/x", DEFAULT_URL);
        assert_eq!(album.site, "same-origin");
        assert_eq!(album.referer.as_deref(), Some("https://downloads.khinsider.com/"));
    }

    #[tokio::test]
    async fn a_check_asks_the_browser_once_and_uses_the_cleared_page() {
        let fake = Fake { html: Mutex::new(Ok("<html>Search - KHInsider</html>".into())), calls: std::sync::atomic::AtomicU32::new(0) };
        let got = finish_page(Err(SourceError::Blocked(BOT_CHECK.into())), Some(&fake), "https://downloads.khinsider.com/search?search=a")
            .await
            .unwrap();
        assert!(got.contains("KHInsider"));
        assert_eq!(calls(&fake), 1);
        // A 200 that is still the check goes to the browser too.
        let still = finish_page(Ok("<title>Just a moment...</title>".into()), Some(&fake), "https://downloads.khinsider.com/").await;
        assert!(still.unwrap().contains("KHInsider"), "the browser cleared it");
        assert_eq!(calls(&fake), 2, "that page asked the browser once too");
    }

    #[tokio::test]
    async fn a_check_the_browser_does_not_clear_is_a_block_without_a_second_ask() {
        let fake = Fake {
            html: Mutex::new(Ok("<title>Just a moment...</title>".into())),
            calls: std::sync::atomic::AtomicU32::new(0),
        };
        let got = finish_page(Err(SourceError::Blocked(BOT_CHECK.into())), Some(&fake), "https://downloads.khinsider.com/").await;
        assert!(is_check(&got.unwrap_err()));
        assert_eq!(calls(&fake), 1, "the browser is asked once, not in a loop");
    }

    #[tokio::test]
    async fn no_webview_keeps_the_check_and_a_real_page_skips_the_browser() {
        let none = finish_page(Err(SourceError::Blocked(BOT_CHECK.into())), None, "https://downloads.khinsider.com/").await;
        assert!(is_check(&none.unwrap_err()));
        let fake = Fake { html: Mutex::new(Err(SourceError::NotConfigured)), calls: std::sync::atomic::AtomicU32::new(0) };
        let skipped = finish_page(Err(SourceError::Blocked(BOT_CHECK.into())), Some(&fake), "u").await;
        assert!(is_check(&skipped.unwrap_err()), "a webview that is not there leaves the check");
        assert_eq!(calls(&fake), 1);
        let real = finish_page(Ok("albums".into()), Some(&fake), "u").await.unwrap();
        assert_eq!(real, "albums");
        assert_eq!(calls(&fake), 1, "a page that is not a check does not open the browser");
    }
}
