//! Genius: plain lyrics, often the only text of a new release. No account:
//! the site's own search (`/api/search/multi`, what its search box asks)
//! finds the song, its public page has the text.
//!
//! A song can be on Genius twice (a re-recording, «космоцветок (ONF)» next
//! to «Космоцветок» of 2020): when the track's album is known, the version
//! on that album wins. Translation and romanisation pages (by «Genius …»
//! accounts) are never taken.

use serde::Deserialize;
use url::Url;

use crate::error::Result;
use crate::http::{Http, Service, base_url, build_url, ok_body, parse_json};
use crate::text::{album_key, artist_key, search_artist, search_title, similarity, title_key};

const DEFAULT_BASE: &str = "https://genius.com/";
/// The pages answer a browser; the default client gets a challenge.
const BROWSER: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";
const MIN_TITLE_SIMILARITY: f64 = 0.85;
const MIN_ARTIST_SIMILARITY: f64 = 0.7;
/// Candidates whose album is looked at when there are several.
const ALBUM_CHECKS: usize = 3;

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
}

impl Genius {
    pub fn new(http: Http) -> Self {
        Self { http, base: base_url(DEFAULT_BASE).expect("default base URL is valid") }
    }

    pub fn with_base_url(http: Http, base: &str) -> Result<Self> {
        Ok(Self { http, base: base_url(base)? })
    }

    async fn get(&self, url: Url) -> Result<Vec<u8>> {
        let req = self.http.client().get(url).header(reqwest::header::USER_AGENT, BROWSER);
        ok_body(self.http.send(Service::Genius, req).await?).await
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

    /// The plain text of the song, `None` when Genius has none.
    pub async fn find(&self, artist: &str, title: &str, album: Option<&str>) -> Result<Option<String>> {
        let (artist, title) = (search_artist(artist), search_title(title));
        let hits = self.search(&format!("{artist} {title}")).await?;
        let mut found: Vec<Song> = hits.into_iter().filter(|s| same_song(s, &artist, &title)).collect();
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

/// «космоцветок (cosmoflower) (ONF)» → «космоцветок»: Genius puts the
/// translation of a title in brackets.
fn base_part(title: &str) -> &str {
    title.split(" (").next().unwrap_or(title).trim()
}

fn same_song(s: &Song, artist: &str, title: &str) -> bool {
    if s.primary_artist.name.starts_with("Genius ") || s.lyrics_state.eq_ignore_ascii_case("unreleased") {
        return false;
    }
    let want = title_key(title).base;
    let t = similarity(&want, &title_key(&s.title).base).max(similarity(&want, &title_key(base_part(&s.title)).base));
    if t < MIN_TITLE_SIMILARITY {
        return false;
    }
    let want = artist_key(artist);
    want.is_empty()
        || similarity(&want, &artist_key(&s.primary_artist.name)) >= MIN_ARTIST_SIMILARITY
        || s.artist_names.split([',', '&']).any(|a| similarity(&want, &artist_key(a.trim())) >= MIN_ARTIST_SIMILARITY)
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
            && let Some(semi) = r[..r.len().min(12)].find(';')
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
        assert_eq!(g.find("mzlff", "космоцветок", Some("не забытое старое")).await.unwrap().as_deref(), Some("новый"));
        // Without an album, the first hit.
        assert_eq!(g.find("mzlff", "космоцветок", None).await.unwrap().as_deref(), Some("старый"));
        // Another song: nothing.
        Mock::given(path("/api/search/multi")).and(query_param("q", "mzlff другое")).respond_with(ResponseTemplate::new(200).set_body_raw(hits, "application/json")).mount(&server).await;
        assert_eq!(g.find("mzlff", "другое", None).await.unwrap(), None);
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
        let text = g.find("mzlff", "космоцветок", Some("не забытое старое")).await.unwrap().unwrap();
        println!("{text}");
        assert!(text.lines().count() > 10);
    }
}
