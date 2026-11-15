//! Dogmazic (play.dogmazic.net) as a [`Source`]: a French archive of free
//! music under Creative Commons licences, run on Ampache. Mostly MP3, some
//! Ogg Vorbis, now and then a WAV (checked 2026-11-15).
//!
//! Without a login it is its public pages, as anyone sees them: the
//! Subsonic API refuses a request without one (error 10, «Required
//! parameter is missing»; a made-up guest gets 40), but the site lets
//! everyone search (`search.php`, a title rule and an artist rule) and
//! download (`stream.php?action=download&song_id=…`, a redirect to the
//! file as it was uploaded, `format/raw`). The pages say nothing of a
//! file's format, so the first bytes of the best matches are read
//! ([`crate::sniff`]) and offered for what they are: an MP3 is an MP3 with
//! its bitrate, never a lossless file. Tracks only: an album is wished for
//! from the sources that have albums.
//!
//! With a login (`[sources] dogmazic_user`, `dogmazic_password` in
//! settings.toml; registering is free) it is the Subsonic client
//! ([`crate::subsonic`]), albums too, with each song's codec and bitrate
//! as the server lists them.
//!
//! Its robots.txt asks a second between requests: one request every 1.5 s,
//! the hops of a redirect included (followed by hand, each at the pace),
//! one file at a time, and a pause of the whole source when it turns us
//! away ([`Breaker`]), looked at again once a request's turn has come.

use std::path::{Path, PathBuf};
use std::time::Duration;

use nikgob_core::indexer::BoxFuture;
use nikgob_core::settings::SourcesSettings;
use nikgob_core::source::{Fetched, Format, Offer, OfferScope, Progress, Source, SourceError, SourceKey, Want, WantScope};
use serde::{Deserialize, Serialize};

use crate::html::text_of;
use crate::http::{
    Breaker, Cookies, PAGE_TIMEOUT, Pace, client_without_redirects, cookie_header, page_of, range_header, read_range, safe_name, save,
    unreachable,
};
use crate::matching::judge;
use crate::sniff::{named_by_bytes, sniff_remote, with_average};
use crate::subsonic::{Server, Subsonic, base_of};
use crate::text::{main_artist, title_forms};

/// The archive's address.
pub const DEFAULT_URL: &str = "https://play.dogmazic.net";
const SERVER: Server = Server { key: SourceKey::Dogmazic, default_url: DEFAULT_URL, guest: None };
/// One request per this (its robots.txt: `Crawl-delay: 1`).
const PACE: Duration = Duration::from_millis(1500);
/// Matching songs whose file is looked at for one wish.
const PROBED: usize = 2;
/// Redirects followed for one request.
const REDIRECTS: usize = 5;

pub struct Dogmazic {
    /// The Subsonic API, with the user's login.
    api: Option<Subsonic>,
    base: String,
    /// Follows no redirect by itself: each hop waits for its turn.
    http: reqwest::Client,
    pace: Pace,
    breaker: Breaker,
    cookies: Cookies,
}

/// A song of a search page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub secs: Option<u32>,
}

/// What a fetch of a song of the pages needs.
#[derive(Serialize, Deserialize)]
struct Handle {
    /// The song's id on the site.
    web: String,
    /// The file's name without the extension.
    stem: String,
    /// The extension its first bytes gave.
    ext: String,
}

impl Dogmazic {
    /// From `[sources]` of settings.toml: the user's login, or none.
    pub fn from_settings(s: &SourcesSettings) -> Self {
        Self::with_pace(DEFAULT_URL, &s.dogmazic_user, &s.dogmazic_password, PACE)
    }

    /// At another address and pace (tests).
    pub fn with_pace(url: &str, user: &str, password: &str, every: Duration) -> Self {
        let api = (!user.trim().is_empty()).then(|| Subsonic::new(SERVER, url, user, password, every));
        Self {
            api,
            base: base_of(url, DEFAULT_URL),
            http: client_without_redirects(),
            pace: Pace::new(every),
            breaker: Breaker::default(),
            cookies: Cookies::default(),
        }
    }

    /// The search page's URL: songs whose title has `title` and whose
    /// artist has `artist` (when given).
    fn search_url(&self, title: &str, artist: &str) -> String {
        let mut u = format!("{}/search.php?type=song&action=search&rule_1=title&rule_1_operator=0&rule_1_input={}", self.base, enc(title));
        if !artist.trim().is_empty() {
            u.push_str(&format!("&rule_2=artist&rule_2_operator=0&rule_2_input={}&operator=and", enc(artist)));
        }
        u
    }

    fn download_url(&self, id: &str) -> String {
        format!("{}/stream.php?action=download&song_id={}", self.base, enc(id))
    }

    /// One request: at the pace, through the breaker (looked at again
    /// once its turn came: another request may have set the source aside
    /// meanwhile), a redirect followed by hand, each hop at the pace too.
    /// A page sends the site's cookies; a range asks for those bytes.
    async fn send(&self, url: &str, range: Option<(u64, u64)>, page: bool) -> Result<reqwest::Response, SourceError> {
        let mut url = url::Url::parse(url).map_err(|e| SourceError::Parse(e.to_string()))?;
        for _ in 0..=REDIRECTS {
            self.breaker.check()?;
            self.pace.wait().await;
            self.breaker.check()?;
            let mut req = self.http.get(url.clone());
            if page {
                req = req
                    .header(reqwest::header::ACCEPT, "text/html,application/xhtml+xml;q=0.9,*/*;q=0.8")
                    .timeout(PAGE_TIMEOUT);
                if let Some(c) = cookie_header(&self.cookies) {
                    req = req.header(reqwest::header::COOKIE, c);
                }
            }
            if let Some((from, len)) = range {
                req = req.header(reqwest::header::RANGE, range_header(from, len)).timeout(PAGE_TIMEOUT);
            }
            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    let e = unreachable(e);
                    self.breaker.note::<()>(&Err(e.clone()));
                    return Err(e);
                }
            };
            let next = if resp.status().is_redirection() {
                resp.headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|l| url.join(l).ok())
            } else {
                None
            };
            match next {
                Some(n) => url = n,
                None => return Ok(resp),
            }
        }
        Err(SourceError::Parse("too many redirects".into()))
    }

    /// A page. (The breaker hears of what the site answered, not of its
    /// own refusal: a request turned away at its turn adds no pause.)
    async fn page(&self, url: &str) -> Result<String, SourceError> {
        let resp = self.send(url, None, true).await?;
        let r = page_of(resp, Some(&self.cookies)).await;
        self.breaker.note(&r);
        r
    }

    /// Bytes of a song's file.
    async fn bytes(&self, id: &str, from: u64, len: u64) -> Result<(Vec<u8>, Option<u64>), SourceError> {
        let resp = self.send(&self.download_url(id), Some((from, len)), false).await?;
        let r = read_range(resp, from, len).await;
        self.breaker.note(&r);
        r
    }

    /// What a song's file is (its format with the bitrate, its extension)
    /// and how big; `None` when it is not audio this knows.
    async fn probe(&self, row: &Row) -> Result<Option<(Format, &'static str, Option<u64>)>, SourceError> {
        let id = row.id.as_str();
        let Some((s, size)) = sniff_remote(move |from, len| self.bytes(id, from, len)).await? else { return Ok(None) };
        Ok(Some((with_average(&s, size, row.secs), s.ext, size)))
    }

    fn offer(&self, row: &Row) -> Offer {
        Offer {
            source: SourceKey::Dogmazic,
            provider: SourceKey::Dogmazic.display_name().to_owned(),
            id: row.id.clone(),
            title: if row.artist.is_empty() { row.title.clone() } else { format!("{} — {}", row.artist, row.title) },
            artist: (!row.artist.is_empty()).then(|| row.artist.clone()),
            track_title: Some(row.title.clone()),
            album: row.album.clone(),
            duration_ms: row.secs.map(|s| s * 1000),
            isrc: None,
            scope: OfferScope::Track,
            format: None,
            size_bytes: None,
            seeders: None,
            info_url: Some(format!("{}/song.php?action=show_song&song_id={}", self.base, enc(&row.id))),
            handle: String::new(),
        }
    }

    /// Songs of the pages for a track wish: its title's forms with its
    /// artist, the best matches looked at.
    async fn search_pages(&self, want: &Want) -> Result<Vec<Offer>, SourceError> {
        let Some(title) = want.title.as_deref() else { return Ok(Vec::new()) };
        let artist = main_artist(want.artist());
        let mut rows: Vec<Row> = Vec::new();
        for form in title_forms(title).into_iter().take(2) {
            for r in rows_of(&self.page(&self.search_url(&form, &artist)).await?) {
                if !rows.iter().any(|x| x.id == r.id) {
                    rows.push(r);
                }
            }
            if rows.iter().any(|r| judge(&self.offer(r), want).is_some_and(|v| v.sure)) {
                break;
            }
        }
        let mut judged: Vec<(f64, &Row)> = rows
            .iter()
            .filter_map(|r| judge(&self.offer(r), want).map(|v| (v.score + if v.sure { 1.0 } else { 0.0 }, r)))
            .collect();
        judged.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut out = Vec::new();
        for (_, row) in judged.into_iter().take(PROBED) {
            let (format, ext, size) = match self.probe(row).await {
                Ok(Some(found)) => found,
                // A page, a file it cannot tell, a song gone: the next one.
                Ok(None) | Err(SourceError::Parse(_) | SourceError::Http { status: 404 | 410 }) => {
                    log::info!("dogmazic: song {} is not audio this knows", row.id);
                    continue;
                }
                Err(e) => return Err(e),
            };
            let mut o = self.offer(row);
            o.format = Some(format);
            o.size_bytes = size;
            let handle = Handle { web: row.id.clone(), stem: safe_name(&row.title), ext: ext.to_owned() };
            o.handle = serde_json::to_string(&handle).unwrap_or_default();
            out.push(o);
        }
        Ok(out)
    }

    /// A song of the pages into `dest`, named by what its bytes are.
    async fn fetch_page_song(&self, h: &Handle, dest: &Path, progress: &Progress) -> Result<PathBuf, SourceError> {
        let path = dest.join(format!("{}.{}", h.stem, h.ext));
        let resp = self.send(&self.download_url(&h.web), None, false).await?;
        let r = save(resp, &path, progress).await;
        self.breaker.note(&r);
        named_by_bytes(&r?).await
    }
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// The text of the cell of `class` in a row.
fn cell(row: &str, class: &str) -> Option<String> {
    let at = row.find(&format!("class=\"{class}\""))?;
    let open = at + row[at..].find('>')? + 1;
    let close = open + row[open..].find("</td")?;
    let text = text_of(&row[open..close]);
    (!text.is_empty()).then_some(text)
}

/// "3:33", "1:02:03" → seconds.
fn secs_of(s: &str) -> Option<u32> {
    let mut total = 0u32;
    for part in s.trim().split(':') {
        total = total.checked_mul(60)?.checked_add(part.trim().parse().ok()?)?;
    }
    (total > 0).then_some(total)
}

/// An artist as the site writes it, without the decoration some put
/// around their name (":: Kali-Frogz ::").
fn artist_of(s: &str) -> String {
    s.trim_matches(|c: char| c.is_whitespace() || matches!(c, ':' | '|' | '*' | '~' | '=')).to_owned()
}

/// The songs of a search page.
pub fn rows_of(html: &str) -> Vec<Row> {
    let mut out = Vec::new();
    for chunk in html.split("<tr id=\"song_").skip(1) {
        let id: String = chunk.chars().take_while(char::is_ascii_digit).collect();
        let row = chunk.split("</tr>").next().unwrap_or(chunk);
        let Some(title) = cell(row, "cel_song") else { continue };
        if id.is_empty() {
            continue;
        }
        out.push(Row {
            id,
            title,
            artist: cell(row, "cel_artist").map(|a| artist_of(&a)).unwrap_or_default(),
            album: cell(row, "cel_album"),
            secs: cell(row, "cel_time").and_then(|t| secs_of(&t)),
        });
    }
    out
}

impl Source for Dogmazic {
    fn key(&self) -> SourceKey {
        SourceKey::Dogmazic
    }

    fn name(&self) -> &str {
        SourceKey::Dogmazic.display_name()
    }

    fn search<'a>(&'a self, want: &'a Want) -> BoxFuture<'a, Result<Vec<Offer>, SourceError>> {
        Box::pin(async move {
            match (&self.api, want.scope) {
                (Some(api), _) => api.search(want).await,
                (None, WantScope::Track) => self.search_pages(want).await,
                (None, WantScope::Album) => Ok(Vec::new()),
            }
        })
    }

    fn fetch<'a>(&'a self, offer: &'a Offer, want: &'a Want, dest: &'a Path, progress: Progress) -> BoxFuture<'a, Result<Fetched, SourceError>> {
        Box::pin(async move {
            if let Ok(h) = serde_json::from_str::<Handle>(&offer.handle) {
                return Ok(Fetched::Files(vec![self.fetch_page_song(&h, dest, &progress).await?]));
            }
            match &self.api {
                Some(api) => api.fetch(offer, want, dest, progress).await,
                None => Err(SourceError::NothingToFetch),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_come_from_the_search_page() {
        let html = r#"<tr id="song_1430">
            <td class="cel_song"><a href="x" title=":: Kali-Frogz :: - Time Rated Morning"> Time Rated Morning</a></td>
            <td class="cel_artist"><a href="y" title=":: Kali-Frogz ::">:: Kali-Frogz ::</a></td>
            <td class="cel_album"><a href="z"> Flatbone-EP</a></td>
            <td data-label="Année" class="cel_year">2005</td>
            <td class="cel_time">3:33</td>
        </tr><tr id="song_9"><td class="cel_song"><a>L&#039;été</a></td><td class="cel_time"></td></tr>"#;
        let rows = rows_of(html);
        assert_eq!(
            rows[0],
            Row { id: "1430".into(), title: "Time Rated Morning".into(), artist: "Kali-Frogz".into(), album: Some("Flatbone-EP".into()), secs: Some(213) }
        );
        assert_eq!((rows[1].title.as_str(), rows[1].artist.as_str(), rows[1].secs), ("L'été", "", None));
        assert_eq!(secs_of("1:02:03"), Some(3723));
        assert_eq!(secs_of("x"), None);
    }

    #[test]
    fn a_login_is_the_api_and_none_is_the_pages() {
        let anon = Dogmazic::with_pace("", "", "", Duration::ZERO);
        assert!(anon.api.is_none());
        assert_eq!(anon.base, DEFAULT_URL);
        assert_eq!(
            anon.search_url("Time Rated Morning", "Kali-Frogz"),
            "https://play.dogmazic.net/search.php?type=song&action=search&rule_1=title&rule_1_operator=0&rule_1_input=Time+Rated+Morning&rule_2=artist&rule_2_operator=0&rule_2_input=Kali-Frogz&operator=and"
        );
        assert_eq!(anon.download_url("1430"), "https://play.dogmazic.net/stream.php?action=download&song_id=1430");
        let user = Dogmazic::with_pace("", "me", "pw", Duration::ZERO);
        assert_eq!(user.api.as_ref().map(|a| a.key()), Some(SourceKey::Dogmazic));
    }
}
