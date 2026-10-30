//! khinsider (downloads.khinsider.com) as a [`Source`]: game soundtracks,
//! often in FLAC, as plain pages. Asked only for a soundtrack (the album
//! says so, or the song names the game it was made for): its search page
//! lists albums, an album page lists the songs with their CD, number,
//! length and sizes per format, and a song's page links the files.
//!
//! The site sits behind a bot check that turns away clients asking too
//! fast or coming back without the cookie it set: one request every two
//! seconds with the site's cookies, and a pause when it does turn us away
//! ([`crate::http::Breaker`]).

use std::path::Path;
use std::time::Duration;

use nikgob_core::indexer::BoxFuture;
use nikgob_core::model::Codec;
use nikgob_core::source::{Fetched, Format, Offer, OfferScope, Progress, Source, SourceError, SourceKey, Want, WantScope};
use nikgob_metadata::text::{similarity, title_key};
use serde::{Deserialize, Serialize};

use crate::html::{Cell, links, table, text_of};
use crate::http::{Breaker, Cookies, Pace, client, download, get_page, safe_name};
use crate::matching::contains_words;
use crate::text::{album_core, album_keys, franchise_of, is_soundtrack, title_keys};

pub const DEFAULT_URL: &str = "https://downloads.khinsider.com";
/// Albums of a search looked into.
const ALBUMS: usize = 3;
/// A song title this alike is the track.
const TITLE: f64 = 0.85;

pub struct Khinsider {
    base: String,
    http: reqwest::Client,
    pace: Pace,
    breaker: Breaker,
    cookies: Cookies,
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
        }
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
        let r = get_page(&self.http, url, Some(&self.cookies)).await;
        self.breaker.note(&r);
        r
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
    pub mp3_bytes: Option<u64>,
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

/// The album page's name and songs. Columns are found by their headers
/// (the CD column is there only for albums of several discs).
pub fn album_page(html: &str) -> AlbumPage {
    let name = html
        .find("<h2>")
        .and_then(|a| html[a + 4..].find("</h2>").map(|b| text_of(&html[a + 4..a + 4 + b])))
        .unwrap_or_default();
    let rows = table(html, "songlist");
    let header: Vec<String> = rows
        .iter()
        .find(|r| r.iter().any(|c| c.head))
        .map(|r| r.iter().map(|c| c.text.to_lowercase()).collect())
        .unwrap_or_default();
    let col = |name: &str| header.iter().position(|h| h == name);
    let (cd, num, title, mp3, flac) = (col("cd"), col("#"), col("song name"), col("mp3"), col("flac"));
    let mut songs = Vec::new();
    for row in rows.iter().filter(|r| !r.iter().any(|c| c.head)) {
        let cell = |i: Option<usize>| -> Option<&Cell> { i.and_then(|i| row.get(i)) };
        // The song's name: its column, else the longest linked text that
        // is not a number, a length or a size.
        let name_cell = cell(title).or_else(|| {
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
        songs.push(Song {
            href,
            name: name_cell.text.clone(),
            disc: number(cell(cd)).unwrap_or(1),
            number: number(cell(num)).unwrap_or(0),
            duration_ms: row.iter().find_map(|c| duration_of(&c.text)),
            flac_bytes: cell(flac).and_then(|c| size_of(&c.text)),
            mp3_bytes: cell(mp3).and_then(|c| size_of(&c.text)),
        });
    }
    AlbumPage { name, songs }
}

/// The files a song's page links: (url, is FLAC), FLAC first.
pub fn song_files(html: &str) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    for (h, _) in links(html) {
        let path = h.split('?').next().unwrap_or(&h).to_lowercase();
        let flac = path.ends_with(".flac");
        if (flac || path.ends_with(".mp3") || path.ends_with(".m4a") || path.ends_with(".ogg"))
            && !h.contains("/game-soundtracks/album/")
            && !out.iter().any(|(u, _)| *u == h)
        {
            out.push((h, flac));
        }
    }
    out.sort_by_key(|(_, flac)| !*flac);
    out
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
                let format = |flac: bool| Some(Format::new(if flac { Codec::Flac } else { Codec::Mp3 }));
                match want.scope {
                    WantScope::Album => {
                        let size: u64 = page.songs.iter().map(|s| if has_flac { s.flac_bytes } else { s.mp3_bytes }.unwrap_or(0)).sum();
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
                                size_bytes: s.flac_bytes.or(s.mp3_bytes),
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

    fn fetch<'a>(&'a self, offer: &'a Offer, _: &'a Want, dest: &'a Path, progress: Progress) -> BoxFuture<'a, Result<Fetched, SourceError>> {
        Box::pin(async move {
            let h: Handle = serde_json::from_str(&offer.handle).map_err(|e| SourceError::Parse(e.to_string()))?;
            let songs: Vec<String> = match (h.song, h.album) {
                (Some(s), _) => vec![s],
                (None, Some(a)) => {
                    let page = album_page(&self.page(&a).await?);
                    page.songs.iter().map(|s| self.url(&s.href)).collect()
                }
                (None, None) => return Err(SourceError::NothingToFetch),
            };
            let lossless = offer.lossless();
            let mut files = Vec::new();
            let mut before = 0u64;
            for song in songs {
                let html = self.page(&song).await?;
                let found = song_files(&html);
                let Some((url, _)) = found.iter().find(|(_, flac)| *flac == lossless).or(found.first()).cloned() else {
                    return Err(SourceError::Parse("a song page without a file".into()));
                };
                let url = self.url(&url);
                self.pace.wait().await;
                let base = before;
                let step: Progress = {
                    let progress = progress.clone();
                    std::sync::Arc::new(move |done, _| progress(base + done, None))
                };
                let path = download(&self.http, &url, &dest.join(file_name(&url)), &step).await?;
                before += std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                files.push(path);
            }
            Ok(Fetched::Files(files))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
