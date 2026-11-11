//! Genius: plain lyrics, often the only text of a new release. No account:
//! the site's own search (`/api/search/multi`, what its search box asks)
//! finds the song, its public page has the text.
//!
//! The track's artists are matched one by one against Genius's own («MellSher»
//! and «и ещё 1 исполнитель» against «MellSher & и ещё 1 исполнитель (i eshe
//! 1 ispolnitel')»), and the search is asked again with the title alone when
//! the main artist and the title find nothing that fits.
//!
//! A song can be on Genius twice (a re-recording, «космоцветок (ONF)» next
//! to «Космоцветок» of 2020): when the track's album is known, the version
//! on that album wins. Translation and romanisation pages (by «Genius …»
//! accounts) are never taken.
//!
//! Genius's bot check turns this client away with 403 at times (November
//! 2026: every request, for hours, while the system's curl got through):
//! then the system's curl asks for the rest of the session.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Deserialize;
use url::Url;

use crate::error::{MetadataError, Result};
use crate::http::{Http, Service, base_url, build_url, ok_body, parse_json};
use crate::text::{album_key, artist_key, search_artist, search_title, similarity, title_key};

const DEFAULT_BASE: &str = "https://genius.com/";
/// The pages answer a browser; the default client gets a challenge.
const BROWSER: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";
const MIN_TITLE_SIMILARITY: f64 = 0.85;
const MIN_ARTIST_SIMILARITY: f64 = 0.7;
/// Candidates whose album is looked at when there are several.
const ALBUM_CHECKS: usize = 3;
/// When the current matching came (2026-11-06 UTC): a miss stored before it
/// was found by an older one and is worth asking again.
pub const MATCHING_SINCE: i64 = 1_793_923_200_000;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Artist {
    name: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Song {
    id: u64,
    title: String,
    artist_names: String,
    primary_artist: Artist,
    url: String,
    lyrics_state: String,
    instrumental: bool,
}

#[derive(Clone, Debug)]
pub struct Genius {
    http: Http,
    base: Url,
    /// Genius turned the client away: curl asks (shared by the clones).
    turned_away: Arc<AtomicBool>,
}

impl Genius {
    pub fn new(http: Http) -> Self {
        Self { http, base: base_url(DEFAULT_BASE).expect("default base URL is valid"), turned_away: Arc::default() }
    }

    pub fn with_base_url(http: Http, base: &str) -> Result<Self> {
        Ok(Self { http, base: base_url(base)?, turned_away: Arc::default() })
    }

    async fn get(&self, url: Url) -> Result<Vec<u8>> {
        if !self.turned_away.load(Ordering::Relaxed) {
            let req = self.http.client().get(url.clone()).header(reqwest::header::USER_AGENT, BROWSER);
            let resp = self.http.send(Service::Genius, req).await?;
            if resp.status() != reqwest::StatusCode::FORBIDDEN {
                return ok_body(resp).await;
            }
            log::info!("genius: turned away (403), asking through curl from now on");
            self.turned_away.store(true, Ordering::Relaxed);
        }
        self.http.wait_turn(Service::Genius).await;
        let got = tokio::task::spawn_blocking(move || curl(&url))
            .await
            .map_err(|e| MetadataError::Network(format!("curl: {e}")))?;
        if let Err(MetadataError::NotConfigured(_)) = &got {
            // No curl here: the client is tried again next time.
            self.turned_away.store(false, Ordering::Relaxed);
        }
        got
    }

    async fn search(&self, query: &str) -> Result<Vec<Song>> {
        #[derive(Deserialize)]
        struct Hit {
            #[serde(rename = "type")]
            kind: String,
            result: serde_json::Value,
        }
        #[derive(Deserialize)]
        struct Section {
            hits: Vec<Hit>,
        }
        #[derive(Deserialize)]
        struct Resp {
            sections: Vec<Section>,
        }
        #[derive(Deserialize)]
        struct Envelope {
            response: Resp,
        }
        let url = build_url(&self.base, "api/search/multi", &[("q", query)])?;
        let env: Envelope = parse_json(&self.get(url).await?)?;
        let mut out: Vec<Song> = Vec::new();
        for s in env.response.sections {
            for h in s.hits {
                if h.kind != "song" {
                    continue;
                }
                if let Ok(song) = serde_json::from_value::<Song>(h.result)
                    && !out.iter().any(|o| o.id == song.id)
                {
                    out.push(song);
                }
            }
        }
        Ok(out)
    }

    async fn album(&self, id: u64) -> Result<Option<String>> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Album {
            name: String,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Detail {
            album: Option<Album>,
        }
        #[derive(Deserialize)]
        struct Resp {
            song: Detail,
        }
        #[derive(Deserialize)]
        struct Envelope {
            response: Resp,
        }
        let url = build_url(&self.base, &format!("api/songs/{id}"), &[])?;
        let env: Envelope = parse_json(&self.get(url).await?)?;
        Ok(env.response.song.album.map(|a| a.name).filter(|n| !n.trim().is_empty()))
    }

    /// The plain text of the song, `None` when Genius has none. `artists`:
    /// the track's credited artists, the main one first (one shown credit
    /// such as «A, B» or «A feat. B» is split here).
    pub async fn find(&self, artists: &[&str], title: &str, album: Option<&str>) -> Result<Option<String>> {
        let names = credited(artists);
        let mut found = Vec::new();
        for query in queries(&names, &search_title(title)) {
            found = self.search(&query).await?.into_iter().filter(|s| same_song(s, &names, title)).collect();
            if !found.is_empty() {
                break;
            }
        }
        if found.is_empty() {
            return Ok(None);
        }
        if let Some(album) = album.map(album_key).filter(|a| !a.is_empty())
            && found.len() > 1
        {
            let mut best: Option<(f64, usize)> = None;
            for (i, s) in found.iter().enumerate().take(ALBUM_CHECKS) {
                let Some(name) = self.album(s.id).await? else { continue };
                let sim = similarity(&album, &album_key(&name)).max(similarity(&album, &album_key(base_part(&name))));
                if best.is_none_or(|(b, _)| sim > b) {
                    best = Some((sim, i));
                }
            }
            if let Some((sim, i)) = best
                && sim >= MIN_TITLE_SIMILARITY
            {
                let s = found.remove(i);
                found.insert(0, s);
            }
        }
        let song = &found[0];
        if song.instrumental {
            return Ok(None);
        }
        let page = Url::parse(&song.url)
            .ok()
            .filter(|u| u.host_str().is_some_and(|h| h.ends_with("genius.com")))
            .map_or_else(|| build_url(&self.base, song.url.trim_start_matches('/'), &[]), Ok)?;
        // Tests serve the page from the mock server.
        let page = if self.base.host_str() == Some("genius.com") { page } else { build_url(&self.base, page.path().trim_start_matches('/'), &[])? };
        let html = String::from_utf8_lossy(&self.get(page).await?).into_owned();
        Ok(lyrics_of(&html).filter(|t| !t.trim().is_empty()))
    }
}

/// `url` through the system's curl (in Windows since 10 1803), as
/// [`ok_body`] would read it. No curl: [`MetadataError::NotConfigured`].
fn curl(url: &Url) -> Result<Vec<u8>> {
    let mut cmd = std::process::Command::new("curl");
    cmd.args(["--silent", "--show-error", "--location", "--connect-timeout", "10", "--max-time", "15"])
        .args(["--user-agent", BROWSER, "--write-out", "%{http_code}", url.as_str()])
        .stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window flashing up.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.output().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => MetadataError::NotConfigured("curl"),
        _ => MetadataError::Network(format!("curl: {e}")),
    })?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(MetadataError::Network(format!("curl: {}", err.trim())));
    }
    // The body, then the three digits of the status.
    let mut body = out.stdout;
    let at = body.len().checked_sub(3).ok_or_else(|| MetadataError::Parse("curl: no status".into()))?;
    let status: u16 = std::str::from_utf8(&body[at..])
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| MetadataError::Parse("curl: no status".into()))?;
    body.truncate(at);
    match status {
        200..=299 => Ok(body),
        404 => Err(MetadataError::NotFound),
        _ => Err(MetadataError::Status { status, body: String::from_utf8_lossy(&body).chars().take(200).collect() }),
    }
}

/// «космоцветок (cosmoflower) (ONF)» → «космоцветок»: Genius puts the
/// translation of a title in brackets.
fn base_part(title: &str) -> &str {
    title.split(" (").next().unwrap_or(title).trim()
}

/// The names to match: each credited artist, or one shown credit split
/// («A, B», «A feat. B» → «A», «B»). Only clear separators split: «и ещё 1
/// исполнитель», «Florence and the Machine» stay whole.
fn credited(artists: &[&str]) -> Vec<String> {
    let parts: Vec<String> = match artists {
        [one] => {
            let main = search_artist(one);
            let feat = one.trim().strip_prefix(main.as_str()).unwrap_or("");
            let feat = feat.trim_start().split_once(' ').map_or("", |(_, rest)| rest);
            main.split([',', '&', ';']).chain(feat.split([',', '&', ';'])).map(str::to_owned).collect()
        }
        many => many.iter().map(|a| search_artist(a)).collect(),
    };
    let mut out: Vec<String> = Vec::new();
    for part in parts {
        let part = part.trim();
        if !part.is_empty() && !out.iter().any(|o| o == part) {
            out.push(part.to_owned());
        }
    }
    out
}

/// What the search is asked, in order until one finds the song: the main
/// artist with the title, every artist with it (when there are several),
/// the title alone.
fn queries(names: &[String], title: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(main) = names.first() {
        out.push(format!("{main} {title}"));
    }
    if names.len() > 1 {
        out.push(format!("{} {title}", names.join(" ")));
    }
    out.push(title.to_owned());
    out.dedup();
    out
}

/// A Genius credit's names: the primary artist, every one in «A, B & C
/// (Ft. D)», each also without its transliteration in brackets.
fn genius_names(s: &Song) -> Vec<String> {
    let names = s.artist_names.replace(" (Ft. ", ", ").replace(" (Feat. ", ", ").replace(" (feat. ", ", ");
    let mut out = vec![artist_key(&s.primary_artist.name), artist_key(base_part(&s.primary_artist.name)), artist_key(&names)];
    for part in names.split([',', '&']) {
        let part = part.trim().trim_end_matches(')').trim();
        out.push(artist_key(part));
        out.push(artist_key(base_part(part)));
    }
    out.retain(|k| !k.is_empty());
    out
}

/// A title in two scripts, «Разгром (Mayhem)» or «Mayhem (Разгром)»: the
/// part before the bracket, which says the same in another script.
fn one_script(title: &str) -> Option<&str> {
    let (base, rest) = title.split_once(" (")?;
    let cyrillic = |t: &str| t.chars().any(|c| matches!(c, '\u{0400}'..='\u{04FF}'));
    let latin = |t: &str| t.chars().any(|c| c.is_ascii_alphabetic());
    let other = (cyrillic(base) && !cyrillic(rest) && latin(rest)) || (latin(base) && !latin(rest) && cyrillic(rest));
    other.then_some(base.trim())
}

fn same_song(s: &Song, names: &[String], title: &str) -> bool {
    if s.primary_artist.name.starts_with("Genius ") || s.lyrics_state.eq_ignore_ascii_case("unreleased") {
        return false;
    }
    // A translation or romanisation page under the artist's own name.
    let lower = s.title.to_lowercase();
    if ["translation)", "romanized)", "перевод)"].iter().any(|w| lower.contains(w)) {
        return false;
    }
    let theirs = [title_key(&s.title).base, title_key(base_part(&s.title)).base];
    let ours = [Some(title), one_script(title)];
    let t = ours
        .iter()
        .flatten()
        .map(|o| title_key(o).base)
        .flat_map(|o| theirs.iter().map(move |t| similarity(&o, t)))
        .fold(0.0, f64::max);
    if t < MIN_TITLE_SIMILARITY {
        return false;
    }
    let mut want: Vec<String> = names.iter().map(|n| artist_key(n)).filter(|k| !k.is_empty()).collect();
    if want.is_empty() {
        return true;
    }
    if names.len() > 1 {
        want.push(artist_key(&names.join(" & ")));
    }
    let theirs = genius_names(s);
    want.iter().any(|w| theirs.iter().any(|t| similarity(w, t) >= MIN_ARTIST_SIMILARITY))
}

/// The text of a song page: every `data-lyrics-container` block, line
/// breaks kept, tags and the blocks Genius excludes from selection (its
/// headers) dropped, section labels and zero-width spaces left out. What
/// is sung stays, a sung tag of the artist in the intro too («Мяу,
/// nyan.mp3» under «[Интро]»).
pub fn lyrics_of(html: &str) -> Option<String> {
    const MARK: &str = "data-lyrics-container=\"true\"";
    let mut out = String::new();
    let mut rest = html;
    let mut any = false;
    while let Some(at) = rest.find(MARK) {
        any = true;
        // Back to the `<div` that carries the mark, then past its `>`.
        let open = rest[..at].rfind('<').unwrap_or(at);
        let after = &rest[open..];
        let Some(gt) = after.find('>') else { break };
        let (inner, end) = container(&after[gt + 1..]);
        out.push_str(&text_of(inner));
        out.push('\n');
        rest = &after[gt + 1 + end..];
    }
    if !any {
        return None;
    }
    // Genius' editors leave zero-width spaces before some lines.
    let out = out.replace(['\u{200b}', '\u{feff}'], "");
    let lines: Vec<&str> = out
        .lines()
        .map(str::trim)
        .filter(|l| !(l.starts_with('[') && l.ends_with(']')))
        .collect();
    // Collapse runs of empty lines (between sections) into one.
    let mut text = String::new();
    let mut blank = true;
    for l in lines {
        if l.is_empty() {
            if !blank {
                text.push('\n');
            }
            blank = true;
        } else {
            text.push_str(l);
            text.push('\n');
            blank = false;
        }
    }
    Some(text.trim().to_owned()).filter(|t| !t.is_empty())
}

/// The inside of a `<div>` whose opening tag was just read: up to its
/// closing tag (nested divs counted), and how far that is.
fn container(s: &str) -> (&str, usize) {
    let mut depth = 1;
    let mut i = 0;
    while i < s.len() {
        let r = &s[i..];
        if r.starts_with("<div") {
            depth += 1;
        } else if r.starts_with("</div") {
            depth -= 1;
            if depth == 0 {
                let end = r.find('>').map_or(s.len(), |g| i + g + 1);
                return (&s[..i], end);
            }
        }
        i += r.chars().next().map_or(1, char::len_utf8);
    }
    (s, s.len())
}

/// Tags out, `<br>` as new lines, entities decoded; an element marked
/// `data-exclude-from-selection` is skipped whole.
fn text_of(html: &str) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < html.len() {
        let r = &html[i..];
        if r.starts_with('<') {
            let end = r.find('>').map_or(r.len(), |g| g + 1);
            let tag = &r[..end];
            if tag.starts_with("<br") {
                out.push('\n');
            } else if tag.contains("data-exclude-from-selection=\"true\"") {
                // Skip the element with its content.
                let name: String = tag[1..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
                let (_, len) = element(&r[end..], &name);
                i += end + len;
                continue;
            }
            i += end;
            continue;
        }
        if r.starts_with('&')
            && let Some(semi) = r.as_bytes()[..r.len().min(12)].iter().position(|&b| b == b';')
            && let Some(c) = entity(&r[1..semi])
        {
            out.push(c);
            i += semi + 1;
            continue;
        }
        let c = r.chars().next().unwrap_or(' ');
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// Like [`container`] for any element name.
fn element<'a>(s: &'a str, name: &str) -> (&'a str, usize) {
    let (open, close) = (format!("<{name}"), format!("</{name}"));
    let mut depth = 1;
    let mut i = 0;
    while i < s.len() {
        let r = &s[i..];
        if r.starts_with(&open) {
            depth += 1;
        } else if r.starts_with(&close) {
            depth -= 1;
            if depth == 0 {
                let end = r.find('>').map_or(s.len(), |g| i + g + 1);
                return (&s[..i], end);
            }
        }
        i += r.chars().next().map_or(1, char::len_utf8);
    }
    (s, s.len())
}

fn entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        n if n.starts_with("#x") || n.starts_with("#X") => u32::from_str_radix(&n[2..], 16).ok().and_then(char::from_u32),
        n if n.starts_with('#') => n[1..].parse().ok().and_then(char::from_u32),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const PAGE: &str = r#"<html><body><div class="x">
        <div data-lyrics-container="true" class="Lyrics__Container"><div data-exclude-from-selection="true"><span>3 Contributors</span><div>Translations</div></div>[Куплет 1]<br/>Я <a href="/1"><span>смотрю</span></a> в небо,<br/>там&#x27;s космоцветок &amp; звёзды<br/><br/>[Припев]<br/>Лети</div>
        <div>Embed</div>
        <div data-lyrics-container="true">Второй <i>блок</i></div></body></html>"#;

    #[test]
    fn an_ampersand_before_cyrillic_is_text() {
        // A bare «&» with Cyrillic within its first 12 bytes: no entity, no panic.
        assert_eq!(text_of("Рок &Слово и Сердце"), "Рок &Слово и Сердце");
        assert_eq!(text_of("&amp;Сердце"), "&Сердце");
    }

    #[test]
    fn page_text_without_headers_and_labels() {
        let text = lyrics_of(PAGE).unwrap();
        assert_eq!(text, "Я смотрю в небо,\nтам's космоцветок & звёзды\n\nЛети\nВторой блок");
        assert!(lyrics_of("<html></html>").is_none());
    }

    /// The page of «Замок» by nyan.mp3 as Genius serves it: the header
    /// (contributors, «… Lyrics») is not text; the sung intro tag is,
    /// without the zero-width space before «nyan.pop».
    #[test]
    fn a_sung_intro_stays_and_the_header_goes() {
        let page = concat!(
            r#"<div data-lyrics-container="true" class="Lyrics__Container"><div data-exclude-from-selection="true" class="LyricsHeader__Container">"#,
            r#"<button><span><svg viewBox="0 0 22 16.47"><path d="M12.55"></path></svg><span>6 Contributors</span></span></button>"#,
            r#"<div><div><div><svg role="img"><path d="M4 16"></path></svg></div><h2>Замок (Castle) Lyrics</h2></div></div></div>"#,
            "[Текст песни «Замок»]<br/><br/>[Интро]<br/><i>Мяу, nyan.mp3</i><br/><i>\u{200b}nyan.pop</i><br/><br/>",
            "[Куплет 1]<br/>Я невменяемый (Ай)<br/>Грусть и тоска, не хватает внимания (Компании)<br/></div>",
        );
        let text = lyrics_of(page).unwrap();
        assert_eq!(text, "Мяу, nyan.mp3\nnyan.pop\n\nЯ невменяемый (Ай)\nГрусть и тоска, не хватает внимания (Компании)");
    }

    fn http() -> Http {
        Http::new(&crate::MetadataConfig::new("tests@example.invalid")).unwrap()
    }

    #[tokio::test]
    async fn the_version_on_the_tracks_album_wins_and_translations_are_skipped() {
        let server = MockServer::start().await;
        let hits = r#"{"response":{"sections":[{"type":"top_hit","hits":[{"type":"song","result":{"id":1,"title":"Космоцветок (Cosmoflower)","artist_names":"mzlff","primary_artist":{"name":"mzlff"},"url":"https://genius.com/old-lyrics","lyrics_state":"complete"}}]},
            {"type":"song","hits":[{"type":"song","result":{"id":1,"title":"Космоцветок (Cosmoflower)","artist_names":"mzlff","primary_artist":{"name":"mzlff"},"url":"https://genius.com/old-lyrics","lyrics_state":"complete"}},
            {"type":"song","result":{"id":2,"title":"космоцветок (cosmoflower) (ONF)","artist_names":"mzlff","primary_artist":{"name":"mzlff"},"url":"https://genius.com/new-lyrics","lyrics_state":"complete"}},
            {"type":"song","result":{"id":3,"title":"mzlff - космоцветок (cosmoflower) (ONF) (English Translation)","artist_names":"Genius English Translations","primary_artist":{"name":"Genius English Translations"},"url":"https://genius.com/tr-lyrics","lyrics_state":"complete"}}]}]}}"#;
        Mock::given(path("/api/search/multi")).and(query_param("q", "mzlff космоцветок")).respond_with(ResponseTemplate::new(200).set_body_raw(hits, "application/json")).mount(&server).await;
        let album = |name: &str| ResponseTemplate::new(200).set_body_raw(format!(r#"{{"response":{{"song":{{"album":{{"name":"{name}"}}}}}}}}"#), "application/json");
        Mock::given(path("/api/songs/1")).respond_with(album("Космоцветок (Cosmoflower)")).mount(&server).await;
        Mock::given(path("/api/songs/2")).respond_with(album("не забытое старое (old not forgotten)")).mount(&server).await;
        let page = |t: &str| ResponseTemplate::new(200).set_body_raw(format!(r#"<div data-lyrics-container="true">{t}</div>"#), "text/html");
        Mock::given(path("/old-lyrics")).respond_with(page("старый")).mount(&server).await;
        Mock::given(path("/new-lyrics")).respond_with(page("новый")).mount(&server).await;
        let g = Genius::with_base_url(http(), &server.uri()).unwrap();
        assert_eq!(g.find(&["mzlff"], "космоцветок", Some("не забытое старое")).await.unwrap().as_deref(), Some("новый"));
        // Without an album, the first hit.
        assert_eq!(g.find(&["mzlff"], "космоцветок", None).await.unwrap().as_deref(), Some("старый"));
        // Another song: nothing.
        Mock::given(path("/api/search/multi")).and(query_param("q", "mzlff другое")).respond_with(ResponseTemplate::new(200).set_body_raw(hits, "application/json")).mount(&server).await;
        Mock::given(path("/api/search/multi")).and(query_param("q", "другое")).respond_with(ResponseTemplate::new(200).set_body_raw(hits, "application/json")).mount(&server).await;
        assert_eq!(g.find(&["mzlff"], "другое", None).await.unwrap(), None);
    }

    fn song(id: u64, title: &str, artist_names: &str, primary: &str) -> String {
        format!(r#"{{"type":"song","result":{{"id":{id},"title":"{title}","artist_names":"{artist_names}","primary_artist":{{"name":"{primary}"}},"url":"https://genius.com/song-{id}","lyrics_state":"complete"}}}}"#)
    }

    fn hits(songs: &[String]) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_raw(format!(r#"{{"response":{{"sections":[{{"type":"song","hits":[{}]}}]}}}}"#, songs.join(",")), "application/json")
    }

    /// A server that answers every other search with nothing and serves
    /// `song-<id>` pages with «text <id>».
    async fn server() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(path("/api/search/multi")).respond_with(hits(&[])).with_priority(10).mount(&server).await;
        for id in 1..=9 {
            Mock::given(path(format!("/song-{id}")))
                .respond_with(ResponseTemplate::new(200).set_body_raw(format!(r#"<div data-lyrics-container="true">text {id}</div>"#), "text/html"))
                .mount(&server)
                .await;
        }
        server
    }

    /// «Разгром» by MellSher and «и ещё 1 исполнитель»: on Genius as
    /// «Разгром (Mayhem)» by «MellSher & и ещё 1 исполнитель (i eshe 1
    /// ispolnitel')». Each artist is matched on its own, and the shown
    /// credit as one string is split the same way.
    #[tokio::test]
    async fn a_song_of_several_artists_is_found_by_each_of_them() {
        let server = server().await;
        let razgrom = song(1, "Разгром (Mayhem)", "MellSher & и ещё 1 исполнитель (i eshe 1 ispolnitel')", "MellSher");
        Mock::given(path("/api/search/multi")).and(query_param("q", "MellSher Разгром")).respond_with(hits(&[razgrom.clone()])).mount(&server).await;
        let g = Genius::with_base_url(http(), &server.uri()).unwrap();
        let found = |artists: &'static [&'static str]| {
            let g = g.clone();
            async move { g.find(artists, "Разгром", Some("Разгром")).await.unwrap() }
        };
        assert_eq!(found(&["MellSher", "и ещё 1 исполнитель"]).await.as_deref(), Some("text 1"));
        assert_eq!(found(&["MellSher, и ещё 1 исполнитель"]).await.as_deref(), Some("text 1"));
        // Only the second artist known: found by the title alone.
        Mock::given(path("/api/search/multi")).and(query_param("q", "Разгром")).respond_with(hits(&[razgrom])).mount(&server).await;
        assert_eq!(found(&["и ещё 1 исполнитель"]).await.as_deref(), Some("text 1"));
    }

    /// The main artist with the title finds other songs: the title alone
    /// finds this one, the one by another artist of the same title is not
    /// taken, nor a translation page under the artist's own name.
    #[tokio::test]
    async fn the_title_alone_is_asked_when_the_artist_and_title_find_nothing() {
        let server = server().await;
        Mock::given(path("/api/search/multi")).and(query_param("q", "Кино Звезда"))
            .respond_with(hits(&[song(1, "Звезда по имени Солнце", "КИНО (KINO)", "КИНО (KINO)")])).mount(&server).await;
        Mock::given(path("/api/search/multi")).and(query_param("q", "Звезда")).respond_with(hits(&[
            song(2, "Звезда (Star)", "Другой", "Другой"),
            song(3, "Звезда (English Translation)", "Кино", "Кино"),
            song(4, "Звезда (Zvezda)", "КИНО (KINO)", "КИНО (KINO)"),
        ])).mount(&server).await;
        let g = Genius::with_base_url(http(), &server.uri()).unwrap();
        assert_eq!(g.find(&["Кино"], "Звезда", None).await.unwrap().as_deref(), Some("text 4"));
        // Another artist with a song of that title only: nothing.
        assert_eq!(g.find(&["Ария"], "Звезда", None).await.unwrap(), None);
    }

    /// A title in two scripts on our side matches Genius's one-script one.
    #[tokio::test]
    async fn a_title_with_its_translation_in_brackets_matches() {
        let server = server().await;
        Mock::given(path("/api/search/multi")).and(query_param("q", "mzlff Космоцветок Cosmoflower"))
            .respond_with(hits(&[song(5, "Космоцветок", "mzlff", "mzlff")])).mount(&server).await;
        let g = Genius::with_base_url(http(), &server.uri()).unwrap();
        assert_eq!(g.find(&["mzlff"], "Космоцветок (Cosmoflower)", None).await.unwrap().as_deref(), Some("text 5"));
    }

    /// Genius turns the client away: curl asks, for that request and the
    /// ones after it.
    #[tokio::test]
    async fn a_client_genius_turns_away_asks_through_curl() {
        if std::process::Command::new("curl").arg("--version").output().is_err() {
            return;
        }
        let server = server().await;
        let razgrom = song(1, "Разгром (Mayhem)", "MellSher & и ещё 1 исполнитель (i eshe 1 ispolnitel')", "MellSher");
        let challenge = ResponseTemplate::new(403).set_body_raw("<!DOCTYPE html><title>Genius</title>", "text/html");
        Mock::given(path("/api/search/multi")).respond_with(challenge).up_to_n_times(1).with_priority(1).mount(&server).await;
        Mock::given(path("/api/search/multi")).and(query_param("q", "MellSher Разгром")).respond_with(hits(&[razgrom])).mount(&server).await;
        let g = Genius::with_base_url(http(), &server.uri()).unwrap();
        assert_eq!(g.find(&["MellSher"], "Разгром", None).await.unwrap().as_deref(), Some("text 1"));
        assert!(g.turned_away.load(Ordering::Relaxed));
        // Answers other than 200 read as the client's would.
        assert!(matches!(curl(&build_url(&g.base, "nowhere", &[]).unwrap()), Err(MetadataError::NotFound)));
    }

    #[test]
    fn credits_split_only_on_clear_separators() {
        assert_eq!(credited(&["MellSher, и ещё 1 исполнитель"]), ["MellSher", "и ещё 1 исполнитель"]);
        assert_eq!(credited(&["A feat. B & C"]), ["A", "B", "C"]);
        assert_eq!(credited(&["Florence and the Machine"]), ["Florence and the Machine"]);
        assert_eq!(credited(&["MellSher", "и ещё 1 исполнитель"]), ["MellSher", "и ещё 1 исполнитель"]);
        assert_eq!(queries(&credited(&["A", "B"]), "T"), ["A T", "A B T", "T"]);
        assert_eq!(queries(&[], "T"), ["T"]);
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// `cargo test -p nikgob-metadata genius_live -- --ignored`
    #[tokio::test]
    #[ignore = "asks genius.com"]
    async fn genius_live() {
        let g = Genius::new(Http::new(&crate::MetadataConfig::new("tests@example.invalid")).unwrap());
        let text = g.find(&["mzlff"], "космоцветок", Some("не забытое старое")).await.unwrap().unwrap();
        println!("{text}");
        assert!(text.lines().count() > 10);
        let text = g.find(&["MellSher", "и ещё 1 исполнитель"], "Разгром", Some("Разгром")).await.unwrap().unwrap();
        println!("{text}");
        assert!(text.lines().count() > 10);
    }
}

