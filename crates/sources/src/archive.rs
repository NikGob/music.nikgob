//! The Internet Archive (archive.org) as a [`Source`]: uploads of CD rips,
//! rare and old albums, concerts and net labels, often with the original
//! FLAC. Its open JSON needs no key: `advancedsearch.php` finds items,
//! `metadata/{id}` lists an item's files (format, length, size, tags) and
//! `download/{id}/{file}` gives one. An item is an album offer, one of its
//! files a track offer.
//!
//! Uploads are named freely, so an item is taken only as far as its files
//! say it is the wish: a track by its title (the tag, else the file's name)
//! and length, an album by its tracks' album tag and their count. A whole
//! side or album in one file is left out, and the original FLAC (or 24-bit
//! FLAC) is taken over the MP3 the Archive derives from it.
//!
//! Its limits are not published: one request a second, a short wait it
//! asks for is waited out once, at most two files at a time.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use nikgob_core::indexer::BoxFuture;
use nikgob_core::model::{Codec, quality_score};
use nikgob_core::source::{Fetched, Format, Offer, OfferScope, Progress, Source, SourceError, SourceKey, Want, WantScope};
use nikgob_metadata::text::{similarity, title_key};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::album::{Patience, fetch_each, left_out};
use crate::http::{Breaker, Pace, client, download, get_text, safe_name, track_file_name};
use crate::matching::other_version;
use crate::text::{album_core, album_keys, artist_names, franchise_of, is_soundtrack, title_forms, title_keys};

pub const DEFAULT_URL: &str = "https://archive.org";
const PROVIDER: &str = "Internet Archive";
/// Items a search asks for (those without audio are skipped).
const ROWS: usize = 20;
/// Items of a search looked into.
const ITEMS: usize = 4;
/// Offers of one search at most.
const OFFERS: usize = 12;
/// A track title this alike is the song.
const TITLE: f64 = 0.85;
/// An album tag this alike is the album.
const ALBUM: f64 = 0.85;
/// Files downloaded at once.
const SLOTS: usize = 2;
/// A wait the Archive asks for up to this long is waited out (once).
const LONGEST_WAIT: Duration = Duration::from_secs(60);
/// The wait when it did not say how long.
const FIRST_WAIT: Duration = Duration::from_secs(10);
/// Extensions of the files taken (what the import reads).
const AUDIO_EXT: &[&str] = &["flac", "mp3", "ogg", "m4a", "wav", "opus"];

pub struct Archive {
    base: String,
    http: reqwest::Client,
    pace: Pace,
    breaker: Breaker,
    slots: tokio::sync::Semaphore,
    first_wait: Duration,
}

impl Archive {
    /// `base` empty: [`DEFAULT_URL`].
    pub fn new(base: &str) -> Self {
        Self::with_pace(base, Duration::from_secs(1))
    }

    /// With another pace (tests: zero, and no wait before asking again).
    pub fn with_pace(base: &str, every: Duration) -> Self {
        let base = if base.trim().is_empty() { DEFAULT_URL } else { base.trim() };
        Self {
            base: base.trim_end_matches('/').to_owned(),
            http: client(),
            pace: Pace::new(every),
            breaker: Breaker::default(),
            slots: tokio::sync::Semaphore::new(SLOTS),
            first_wait: if every.is_zero() { Duration::ZERO } else { FIRST_WAIT },
        }
    }

    /// An API answer; a short wait asked for is waited out once.
    async fn page(&self, url: &str) -> Result<String, SourceError> {
        self.breaker.check()?;
        let mut waited = false;
        loop {
            self.pace.wait().await;
            let r = get_text(&self.http, url).await;
            if let Err(SourceError::RateLimited { retry_after }) = &r {
                let wait = retry_after.unwrap_or(self.first_wait);
                if !waited && wait <= LONGEST_WAIT {
                    log::info!("Internet Archive asks to wait, again in {}s", wait.as_secs());
                    waited = true;
                    tokio::time::sleep(wait).await;
                    continue;
                }
            }
            self.breaker.note(&r);
            return r;
        }
    }

    fn search_url(&self, q: &str) -> String {
        let enc = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
        format!(
            "{}/advancedsearch.php?q={}&fl%5B%5D=identifier&fl%5B%5D=title&fl%5B%5D=creator&fl%5B%5D=format&sort%5B%5D={}&rows={ROWS}&page=1&output=json",
            self.base,
            enc(q),
            enc("downloads desc")
        )
    }

    /// One file into `path`; two at a time across every fetch.
    async fn file(&self, item: &str, name: &str, path: &Path, progress: &Progress) -> Result<PathBuf, SourceError> {
        self.breaker.check()?;
        let _slot = self.slots.acquire().await.map_err(|_| SourceError::Io("the downloads closed".into()))?;
        self.pace.wait().await;
        let url = file_url(&self.base, item, name);
        let r = download(&self.http, &url, path, progress).await;
        // A 403 is one item's files kept back and a wait is waited for by
        // the album's fetch; only a file that came says about the site.
        if r.is_ok() {
            self.breaker.note(&r);
        }
        r
    }
}

/// `%XX` for every byte of `s` but the unreserved ones and `/`.
fn encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Where a file of an item downloads from.
pub fn file_url(base: &str, item: &str, name: &str) -> String {
    format!("{base}/download/{}/{}", encode_path(item).replace('/', "%2F"), encode_path(name))
}

fn details_url(base: &str, item: &str) -> String {
    format!("{base}/details/{}", encode_path(item).replace('/', "%2F"))
}

// ------------------------------------------------------------------ queries

/// The words of `s` for a query: letters and digits only (no query
/// syntax), lowercase (never an AND, OR or NOT), at most eight.
fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .take(8)
        .collect()
}

fn field(name: &str, ws: &[String]) -> String {
    format!("{name}:({})", ws.join(" AND "))
}

/// The searches for `want`, in turn until one finds an item with audio:
/// its album (or the song's forms) in the item's title and its artist
/// wherever an upload names one (creator, title, subject); for a
/// soundtrack, then the work alone (an OST's uploader seldom names the
/// composer).
pub fn queries(want: &Want) -> Vec<String> {
    let mut titles: Vec<Vec<String>> = Vec::new();
    let mut push = |ws: Vec<String>| {
        if !ws.is_empty() && !titles.contains(&ws) {
            titles.push(ws);
        }
    };
    let album = want.album.as_deref().or(if want.scope == WantScope::Album { want.title.as_deref() } else { None });
    if let Some(a) = album {
        push(words(&album_core(a)));
    }
    if want.scope == WantScope::Track
        && let Some(t) = &want.title
    {
        for f in title_forms(t).into_iter().take(2) {
            push(words(&f));
        }
    }
    if titles.is_empty() {
        return Vec::new();
    }
    let title_clause = titles.iter().map(|ws| field("title", ws)).collect::<Vec<_>>().join(" OR ");
    let names: Vec<Vec<String>> = artist_names(want).iter().map(|n| words(n)).filter(|w| !w.is_empty()).take(3).collect();
    if names.is_empty() {
        return vec![format!("({title_clause}) AND mediatype:(audio)")];
    }
    let artist_clause = names
        .iter()
        .flat_map(|ws| ["creator", "title", "subject"].map(|f| field(f, ws)))
        .collect::<Vec<_>>()
        .join(" OR ");
    let mut out = vec![format!("({title_clause}) AND ({artist_clause}) AND mediatype:(audio)")];
    let franchise = want.title.as_deref().and_then(franchise_of).or_else(|| want.album.as_deref().and_then(franchise_of));
    let ost = want.album.as_deref().is_some_and(is_soundtrack) || franchise.is_some();
    if ost {
        let work = franchise.or_else(|| want.album.as_deref().map(album_core)).unwrap_or_default();
        let ws = words(&work);
        if !ws.is_empty() {
            out.push(format!("{} AND mediatype:(audio)", field("title", &ws)));
        }
    }
    out
}

// ------------------------------------------------------------------ answers

/// A JSON value as text: a string, a number, the first of a list (the
/// Archive gives one or several values alike).
fn text(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) => Some(s.trim().to_owned()).filter(|s| !s.is_empty()),
        Value::Number(n) => Some(n.to_string()),
        Value::Array(a) => a.iter().find_map(|x| text(Some(x))),
        _ => None,
    }
}

/// Every value of a field that may be one or a list.
fn texts(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a.iter().filter_map(|x| text(Some(x))).collect(),
        other => text(other).into_iter().collect(),
    }
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// An item a search found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub id: String,
    pub title: String,
    pub creator: Option<String>,
    /// The formats of its files («Flac», «VBR MP3», «PNG»…).
    pub formats: Vec<String>,
}

impl Hit {
    fn has_audio(&self) -> bool {
        self.formats.iter().any(|f| codec_of(f).is_some())
    }

    fn has_lossless(&self) -> bool {
        self.formats.iter().any(|f| codec_of(f).is_some_and(|c| c.is_lossless()))
    }
}

/// The items of an `advancedsearch.php` answer, as listed.
pub fn search_results(json: &str) -> Result<Vec<Hit>, SourceError> {
    let v: Value = serde_json::from_str(json).map_err(|e| SourceError::Parse(e.to_string()))?;
    let docs = v
        .pointer("/response/docs")
        .and_then(Value::as_array)
        .ok_or_else(|| SourceError::Parse("a search answer without docs".into()))?;
    Ok(docs
        .iter()
        .filter_map(|d| {
            let id = text(d.get("identifier"))?;
            Some(Hit {
                title: text(d.get("title")).unwrap_or_else(|| id.clone()),
                creator: text(d.get("creator")),
                formats: texts(d.get("format")),
                id,
            })
        })
        .collect())
}

/// A file of an item, as its metadata lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct File {
    /// Its path in the item («CD1/01 - Title.flac»).
    pub name: String,
    pub format: String,
    /// The file it was derived from (the Archive's MP3 of a FLAC).
    pub original: Option<String>,
    pub size: Option<u64>,
    pub length_ms: Option<u32>,
    pub bitrate: Option<u32>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub track: Option<String>,
    /// Kept back from the public.
    pub private: bool,
}

/// An item's metadata.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub creator: Option<String>,
    pub files: Vec<File>,
}

/// "187.52" (seconds), "03:08", "1:02:03".
fn length_of(s: &str) -> Option<u32> {
    let s = s.trim();
    if s.contains(':') {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() > 3 || parts.iter().any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit())) {
            return None;
        }
        return Some(parts.iter().fold(0u32, |acc, p| acc * 60 + p.parse::<u32>().unwrap_or(0)) * 1000);
    }
    let secs: f64 = s.parse().ok()?;
    (secs.is_finite() && secs > 0.0).then(|| (secs * 1000.0).round() as u32)
}

/// The item a `metadata/{id}` answer describes; `None` for one gone (an
/// empty answer) or dark.
pub fn item_of(json: &str) -> Option<Item> {
    let v: Value = serde_json::from_str(json).ok()?;
    if truthy(v.get("is_dark")) {
        return None;
    }
    let files = v.get("files")?.as_array()?;
    let meta = v.get("metadata");
    let m = |k: &str| text(meta.and_then(|m| m.get(k)));
    let files = files
        .iter()
        .filter_map(|f| {
            let s = |k: &str| text(f.get(k));
            Some(File {
                name: s("name")?,
                format: s("format").unwrap_or_default(),
                original: s("original"),
                size: s("size").and_then(|x| x.parse().ok()),
                length_ms: s("length").as_deref().and_then(length_of),
                bitrate: s("bitrate").and_then(|x| x.parse().ok()),
                title: s("title"),
                artist: s("artist").or_else(|| s("creator")),
                album: s("album"),
                track: s("track"),
                private: truthy(f.get("private")),
            })
        })
        .collect();
    Some(Item {
        id: m("identifier").unwrap_or_default(),
        title: m("title").unwrap_or_default(),
        creator: m("creator").or_else(|| m("artist")),
        files,
    })
}

// ------------------------------------------------------------------ tracks

/// The codec a format name of the Archive stands for (its audio ones).
fn codec_of(format: &str) -> Option<Codec> {
    let f = format.trim().to_lowercase();
    if f.contains("zip") {
        return None;
    }
    Some(match f.as_str() {
        "flac" | "24bit flac" => Codec::Flac,
        "apple lossless audio" => Codec::Alac,
        "wave" => Codec::Wav,
        "ogg vorbis" => Codec::Vorbis,
        "opus" => Codec::Opus,
        _ if f.contains("mp3") => Codec::Mp3,
        _ => return None,
    })
}

fn ext_of(name: &str) -> String {
    name.rsplit('/').next().unwrap_or(name).rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default()
}

/// The audio a file is, `None` for any other file (a cover, a
/// spectrogram, a zip of the whole item).
fn format_of(f: &File) -> Option<Format> {
    let codec = codec_of(&f.format)?;
    if !AUDIO_EXT.contains(&ext_of(&f.name).as_str()) {
        return None;
    }
    let lower = f.format.to_lowercase();
    let mut out = Format::new(codec);
    if lower.starts_with("24bit") {
        out.bit_depth = Some(24);
    }
    if !out.lossless() {
        // «VBR MP3» says its rate per file, «128Kbps MP3» in its name.
        out.bitrate_kbps = f
            .bitrate
            .or_else(|| lower.split(|c: char| !c.is_ascii_digit()).find(|s| !s.is_empty()).and_then(|s| s.parse().ok()));
    }
    Some(out)
}

fn quality(f: &Format) -> i32 {
    quality_score(&f.codec, f.bitrate_kbps, f.sample_rate, f.bit_depth, false)
}

/// A song of an item: its best file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    /// The file to download.
    pub file: String,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub disc: u32,
    pub number: u32,
    pub duration_ms: Option<u32>,
    pub size: Option<u64>,
    pub format: Format,
}

/// A file's name without its folders and extension.
fn stem(name: &str) -> &str {
    let base = name.rsplit('/').next().unwrap_or(name);
    base.rsplit_once('.').map_or(base, |(b, _)| b)
}

/// The leading number of a text («01», «3/12», «07 - Title»), of up to
/// three digits (a year is not a track's number).
fn leading_number(s: &str) -> Option<u32> {
    let digits: String = s.trim().chars().take_while(char::is_ascii_digit).collect();
    (!digits.is_empty() && digits.len() <= 3).then(|| digits.parse().ok()).flatten()
}

/// The song's title a file's name says: «01 - Видели ночь.flac» →
/// «Видели ночь», «kino_-_gruppa_krovi.mp3» → «kino - gruppa krovi».
pub fn name_title(name: &str) -> String {
    let base = stem(name);
    let rest = base.trim_start_matches(|c: char| c.is_ascii_digit());
    let rest = if rest.len() < base.len() && leading_number(base).is_some() {
        rest.trim_start_matches(|c: char| matches!(c, ' ' | '.' | '-' | '_' | ')' | '–' | '—'))
    } else {
        base
    };
    let out = rest.replace('_', " ");
    let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if out.is_empty() { base.trim().to_owned() } else { out }
}

/// The disc a file's folder names («CD2», «Disc 2», «Диск 2»), else 1.
pub fn disc_of(name: &str) -> u32 {
    let parts: Vec<&str> = name.split('/').collect();
    for p in parts[..parts.len().saturating_sub(1)].iter().rev() {
        let l = p.to_lowercase();
        for w in ["disc", "disk", "cd", "диск"] {
            for (i, _) in l.match_indices(w) {
                if l[..i].chars().next_back().is_some_and(char::is_alphanumeric) {
                    continue;
                }
                let rest = l[i + w.len()..].trim_start_matches([' ', '_', '-', '.', '#']);
                if let Some(n) = leading_number(rest)
                    && (1..100).contains(&n)
                {
                    return n;
                }
            }
        }
    }
    1
}

/// The songs of an item, by disc and number: for each original audio file
/// its best version (the FLAC over the MP3 derived from it), one per song
/// when the uploader gave several formats; a whole side or album in one
/// file (as long as the rest together) is left out.
pub fn tracks(item: &Item) -> Vec<Track> {
    let audio: Vec<(&File, Format)> = item.files.iter().filter(|f| !f.private).filter_map(|f| format_of(f).map(|x| (f, x))).collect();
    let is_audio = |name: &str| audio.iter().any(|(f, _)| f.name == name);
    let mut out: Vec<Track> = Vec::new();
    for (unit, _) in audio.iter().filter(|(f, _)| f.original.as_deref().is_none_or(|o| !is_audio(o))) {
        let best = audio
            .iter()
            .filter(|(f, _)| f.name == unit.name || f.original.as_deref() == Some(unit.name.as_str()))
            .max_by_key(|(f, x)| (quality(x), f.name == unit.name));
        let Some((best, format)) = best else { continue };
        let or = |a: &Option<String>, b: &Option<String>| a.clone().or_else(|| b.clone());
        out.push(Track {
            file: best.name.clone(),
            title: or(&unit.title, &best.title).unwrap_or_else(|| name_title(&unit.name)),
            artist: or(&unit.artist, &best.artist),
            album: or(&unit.album, &best.album),
            disc: disc_of(&unit.name),
            number: or(&unit.track, &best.track)
                .as_deref()
                .and_then(leading_number)
                .or_else(|| leading_number(stem(&unit.name)))
                .unwrap_or(0),
            duration_ms: unit.length_ms.or(best.length_ms),
            size: best.size,
            format: format.clone(),
        });
    }
    // One song uploaded in two formats: the better one.
    let mut kept: Vec<Track> = Vec::new();
    for t in out {
        let key = (t.disc, t.number, title_key(&t.title).base);
        match kept.iter_mut().find(|k| (k.disc, k.number, title_key(&k.title).base) == key) {
            Some(k) if quality(&t.format) > quality(&k.format) => *k = t,
            Some(_) => {}
            None => kept.push(t),
        }
    }
    let mut out = kept;
    if out.len() >= 4 {
        let total: u64 = out.iter().filter_map(|t| t.duration_ms).map(u64::from).sum();
        out.retain(|t| t.duration_ms.is_none_or(|d| u64::from(d) * 4 < (total - u64::from(d)) * 3));
    }
    out.sort_by(|a, b| (a.disc, a.number, &a.file).cmp(&(b.disc, b.number, &b.file)));
    // No numbers anywhere: the order of the names.
    if out.iter().all(|t| t.number == 0) {
        let mut seen: Vec<(u32, u32)> = Vec::new();
        for t in &mut out {
            let n = match seen.iter_mut().find(|(d, _)| *d == t.disc) {
                Some((_, n)) => {
                    *n += 1;
                    *n
                }
                None => {
                    seen.push((t.disc, 1));
                    1
                }
            };
            t.number = n;
        }
    }
    out
}

// ------------------------------------------------------------------ offers

/// What a fetch needs: the item and its files with the names they get.
#[derive(Serialize, Deserialize)]
struct Handle {
    item: String,
    files: Vec<HandleFile>,
}

#[derive(Serialize, Deserialize)]
struct HandleFile {
    name: String,
    out: String,
    title: String,
}

/// An artist line worth taking: not a link or an address (an uploader's
/// channel stands there often).
fn usable(name: Option<&str>) -> Option<String> {
    name.map(str::trim)
        .filter(|n| !n.is_empty() && !n.contains("http") && !n.contains("www.") && !n.contains('@'))
        .map(str::to_owned)
}

/// The value most of `n` tracks share, when at least half do.
fn common(values: impl Iterator<Item = Option<String>>, n: usize) -> Option<String> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for v in values.flatten() {
        match counts.iter_mut().find(|(k, _)| *k == v) {
            Some((_, c)) => *c += 1,
            None => counts.push((v, 1)),
        }
    }
    counts.into_iter().max_by_key(|(_, c)| *c).filter(|(_, c)| c * 2 >= n).map(|(k, _)| k)
}

/// The names the tracks' files get, one each.
fn out_names(tracks: &[Track]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in tracks {
        let stem = if t.number > 0 { track_file_name(t.disc, t.number, &t.title) } else { safe_name(&t.title) };
        let ext = ext_of(&t.file);
        let mut name = format!("{stem}.{ext}");
        let mut n = 2;
        while out.iter().any(|o| o.eq_ignore_ascii_case(&name)) {
            name = format!("{stem} ({n}).{ext}");
            n += 1;
        }
        out.push(name);
    }
    out
}

fn handle(item: &str, tracks: &[Track]) -> String {
    let files = tracks
        .iter()
        .zip(out_names(tracks))
        .map(|(t, out)| HandleFile { name: t.file.clone(), out, title: t.title.clone() })
        .collect();
    serde_json::to_string(&Handle { item: item.to_owned(), files }).unwrap_or_default()
}

/// How alike a track's title is to one of `wanted` (a title's keys): its
/// own title, or what follows "Someone - " in it.
fn title_sim(t: &Track, wanted: &[String]) -> f64 {
    let mut forms = vec![t.title.clone()];
    if let Some((_, tail)) = t.title.split_once(" - ") {
        forms.push(tail.to_owned());
    }
    forms
        .iter()
        .flat_map(|f| title_keys(f))
        .flat_map(|g| wanted.iter().map(move |w| similarity(&g, w)))
        .fold(0.0, f64::max)
}

/// The offers of an item for `want`: the album (its tracks of the album
/// asked for, as many as it has), or each file that is the song.
pub fn offers(base: &str, item: &Item, want: &Want) -> Vec<Offer> {
    let want_text = format!("{} {}", want.title.as_deref().unwrap_or_default(), want.album.as_deref().unwrap_or_default());
    if other_version(&item.title, &want_text).is_some() {
        return Vec::new();
    }
    let mut tracks = tracks(item);
    if tracks.is_empty() {
        return Vec::new();
    }
    let creator = usable(item.creator.as_deref());
    let info = details_url(base, &item.id);
    match want.scope {
        WantScope::Album => {
            // An item of several albums: the tracks tagged with this one.
            if let Some(wanted) = want.album.as_deref().or(want.title.as_deref()) {
                let keys = album_keys(wanted);
                let named = |t: &Track| {
                    t.album.as_deref().is_some_and(|a| album_keys(a).iter().any(|g| keys.iter().any(|k| similarity(g, k) >= ALBUM)))
                };
                if tracks.iter().any(|t| named(t)) && !tracks.iter().all(|t| named(t)) {
                    tracks.retain(|t| named(t));
                }
            }
            // As many tracks as the album has (a song alone is not it).
            let least = if want.album_tracks == 1 { 1 } else { (want.album_tracks as usize).max(2) };
            if tracks.len() < least {
                return Vec::new();
            }
            let n = tracks.len();
            let album = common(tracks.iter().map(|t| t.album.clone()), n);
            let artist = common(tracks.iter().map(|t| usable(t.artist.as_deref())), n).or_else(|| creator.clone());
            // Said apart only with both tags; else the item's own title
            // (and its creator) is what names the release.
            let (title, artist, album) = match (artist, album) {
                (Some(a), Some(al)) => (format!("{a} — {al}"), Some(a), Some(al)),
                _ => match &creator {
                    Some(c) if !title_key(&item.title).base.contains(&title_key(c).base) => (format!("{c} - {}", item.title), None, None),
                    _ => (item.title.clone(), None, None),
                },
            };
            let format = tracks.iter().map(|t| t.format.clone()).min_by_key(quality);
            let size: u64 = tracks.iter().filter_map(|t| t.size).sum();
            vec![Offer {
                source: SourceKey::Archive,
                provider: PROVIDER.into(),
                id: item.id.clone(),
                title,
                artist,
                track_title: None,
                album,
                duration_ms: None,
                isrc: None,
                scope: OfferScope::Album,
                format,
                size_bytes: (size > 0).then_some(size),
                seeders: None,
                info_url: Some(info),
                handle: handle(&item.id, &tracks),
            }]
        }
        WantScope::Track => {
            let Some(title) = want.title.as_deref() else { return Vec::new() };
            let wanted = title_keys(title);
            tracks
                .iter()
                .filter(|t| title_sim(t, &wanted) >= TITLE)
                .map(|t| {
                    let artist = usable(t.artist.as_deref()).or_else(|| creator.clone());
                    let shown = match &artist {
                        Some(a) => format!("{a} — {}", t.title),
                        None => format!("{} — {}", item.title, t.title),
                    };
                    Offer {
                        source: SourceKey::Archive,
                        provider: PROVIDER.into(),
                        id: format!("{}/{}", item.id, t.file),
                        title: shown,
                        artist,
                        track_title: Some(t.title.clone()),
                        album: t.album.clone(),
                        duration_ms: t.duration_ms,
                        isrc: None,
                        scope: OfferScope::Track,
                        format: Some(t.format.clone()),
                        size_bytes: t.size,
                        seeders: None,
                        info_url: Some(info.clone()),
                        handle: handle(&item.id, std::slice::from_ref(t)),
                    }
                })
                .collect()
        }
    }
}

impl Source for Archive {
    fn key(&self) -> SourceKey {
        SourceKey::Archive
    }

    fn name(&self) -> &str {
        PROVIDER
    }

    fn search<'a>(&'a self, want: &'a Want) -> BoxFuture<'a, Result<Vec<Offer>, SourceError>> {
        Box::pin(async move {
            let want_text = format!("{} {}", want.title.as_deref().unwrap_or_default(), want.album.as_deref().unwrap_or_default());
            let mut hits: Vec<Hit> = Vec::new();
            for q in queries(want) {
                for h in search_results(&self.page(&self.search_url(&q)).await?)? {
                    if h.has_audio() && other_version(&h.title, &want_text).is_none() && !hits.iter().any(|x| x.id == h.id) {
                        hits.push(h);
                    }
                }
                if !hits.is_empty() {
                    break;
                }
            }
            // The items with FLAC first, else as the search ranked them.
            hits.sort_by_key(|h| !h.has_lossless());
            let mut out: Vec<Offer> = Vec::new();
            for h in hits.into_iter().take(ITEMS) {
                let url = format!("{}/metadata/{}", self.base, encode_path(&h.id).replace('/', "%2F"));
                let json = match self.page(&url).await {
                    Ok(j) => j,
                    // Turned away: what was found stays, the rest later.
                    Err(e) if e.is_refusal() => {
                        if out.is_empty() {
                            return Err(e);
                        }
                        break;
                    }
                    Err(e) => {
                        log::info!("Internet Archive: {}: {e}", h.id);
                        continue;
                    }
                };
                let Some(mut item) = item_of(&json) else { continue };
                item.id = h.id.clone();
                if item.title.is_empty() {
                    item.title = h.title.clone();
                }
                if item.creator.is_none() {
                    item.creator = h.creator.clone();
                }
                out.extend(offers(&self.base, &item, want));
                if out.len() >= OFFERS {
                    out.truncate(OFFERS);
                    break;
                }
            }
            Ok(out)
        })
    }

    fn fetch<'a>(&'a self, offer: &'a Offer, want: &'a Want, dest: &'a Path, progress: Progress) -> BoxFuture<'a, Result<Fetched, SourceError>> {
        Box::pin(async move {
            let h: Handle = serde_json::from_str(&offer.handle).map_err(|e| SourceError::Parse(e.to_string()))?;
            let files: Vec<(String, String)> = if offer.scope == OfferScope::Track {
                h.files.into_iter().map(|f| (f.name, f.out)).collect()
            } else {
                left_out(h.files.into_iter().map(|f| (f.name, f.out, f.title)).collect(), &want.got)
            };
            let item = h.item.as_str();
            let before = Arc::new(AtomicU64::new(0));
            fetch_each(files, Patience::default(), |name, out| {
                let (before, progress) = (before.clone(), progress.clone());
                async move {
                    let base = before.load(Ordering::Relaxed);
                    let step: Progress = Arc::new(move |done, _| progress(base + done, None));
                    let path = self.file(item, &name, &dest.join(&out), &step).await?;
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
    use super::*;
    use crate::select::{pick, rank};

    const SEARCH: &str = include_str!("../tests/pages/archive_search.json");
    const ITEM: &str = include_str!("../tests/pages/archive_item.json");

    fn noch() -> Want {
        Want {
            artists: vec!["Кино".into()],
            album: Some("Ночь".into()),
            scope: WantScope::Album,
            album_tracks: 4,
            ..Want::default()
        }
    }

    fn filmy() -> Want {
        Want {
            artists: vec!["Кино".into()],
            title: Some("Фильмы".into()),
            album: Some("Ночь".into()),
            duration_ms: Some(294_000),
            ..Want::default()
        }
    }

    #[test]
    fn lengths_numbers_discs_and_names() {
        assert_eq!(length_of("187.52"), Some(187_520));
        assert_eq!(length_of("03:08"), Some(188_000));
        assert_eq!(length_of("1:02:03"), Some(3_723_000));
        assert_eq!(length_of("abc"), None);
        assert_eq!(leading_number("3/12"), Some(3));
        assert_eq!(leading_number("1985 ● Ночь"), None, "a year is not a number");
        assert_eq!(disc_of("1985 ● Ночь (2021, 3CD)/CD2 ● Демо/01 - Х.flac"), 2);
        assert_eq!(disc_of("Disc 3/01.flac"), 3);
        assert_eq!(disc_of("Диск 2/01.flac"), 2);
        assert_eq!(disc_of("Abcd 5/01.flac"), 1);
        assert_eq!(disc_of("01 - Title.flac"), 1);
        assert_eq!(name_title("CD1/01 - Видели ночь.flac"), "Видели ночь");
        assert_eq!(name_title("kino_-_gruppa_krovi.mp3"), "kino - gruppa krovi");
        assert_eq!(name_title("03. Big Walk Theme.flac"), "Big Walk Theme");
        assert_eq!(name_title("2019 Song.mp3"), "2019 Song");
        assert_eq!(
            file_url("https://archive.org", "kino-noch", "CD1 ● Альбом/01 - Видели ночь.flac"),
            "https://archive.org/download/kino-noch/CD1%20%E2%97%8F%20%D0%90%D0%BB%D1%8C%D0%B1%D0%BE%D0%BC/01%20-%20%D0%92%D0%B8%D0%B4%D0%B5%D0%BB%D0%B8%20%D0%BD%D0%BE%D1%87%D1%8C.flac"
        );
    }

    #[test]
    fn formats_of_the_archive() {
        let f = |format: &str, name: &str, bitrate: Option<u32>| {
            format_of(&File { name: name.into(), format: format.into(), bitrate, ..File::default() })
        };
        assert_eq!(f("Flac", "a.flac", None), Some(Format::new(Codec::Flac)));
        assert_eq!(f("24bit Flac", "a.flac", None).and_then(|x| x.bit_depth), Some(24));
        assert_eq!(f("VBR MP3", "a.mp3", Some(198)).and_then(|x| x.bitrate_kbps), Some(198));
        assert_eq!(f("128Kbps MP3", "a.mp3", None).and_then(|x| x.bitrate_kbps), Some(128));
        assert_eq!(f("Ogg Vorbis", "a.ogg", None).map(|x| x.codec), Some(Codec::Vorbis));
        assert_eq!(f("64Kbps MP3 ZIP", "a_64kb_mp3.zip", None), None);
        assert_eq!(f("VBR M3U", "a_vbr.m3u", None), None);
        assert_eq!(f("Flac FingerPrint", "a.ffp", None), None);
        assert_eq!(f("PNG", "a.png", None), None);
        assert_eq!(f("Spectrogram", "a_spectrogram.png", None), None);
    }

    #[test]
    fn a_search_answer_is_read_and_its_items_told_apart() {
        let hits = search_results(SEARCH).unwrap();
        let ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, ["kino-zip-only", "kino-noch-live-1986", "kino-noch-1985", "kino-noch-mp3"]);
        assert!(!hits[0].has_audio(), "a zip only");
        assert!(hits[2].has_lossless() && hits[2].creator.is_none());
        assert_eq!(hits[3].creator.as_deref(), Some("Кино"), "a list of one");
        assert!(hits[3].has_audio() && !hits[3].has_lossless());
        assert!(search_results("{\"error\":\"x\"}").is_err());
    }

    #[test]
    fn queries_name_the_album_and_the_artist_without_query_syntax() {
        let q = queries(&noch());
        assert_eq!(q, ["(title:(ночь)) AND (creator:(кино) OR title:(кино) OR subject:(кино)) AND mediatype:(audio)"]);
        let q = queries(&filmy());
        assert_eq!(q.len(), 1);
        assert!(q[0].starts_with("(title:(ночь) OR title:(фильмы)) AND"), "{}", q[0]);
        // A soundtrack is asked for once more without its composer.
        let w = Want {
            artists: vec!["Yoshihisa Kato".into()],
            title: Some("Chinmoku no Majo".into()),
            album: Some("Secrets of the Silent Witch: Original Soundtrack".into()),
            ..Want::default()
        };
        let q = queries(&w);
        assert_eq!(q.len(), 2);
        assert_eq!(q[1], "title:(secrets AND of AND the AND silent AND witch) AND mediatype:(audio)");
        assert!(q[0].starts_with("(title:(secrets AND of AND the AND silent AND witch) OR title:(chinmoku AND no AND majo))"), "{}", q[0]);
        // Words like AND/OR in a name stay words.
        let w = Want { artists: vec!["AND OR NOT".into()], album: Some("x".into()), scope: WantScope::Album, ..Want::default() };
        assert!(queries(&w)[0].contains("creator:(and AND or AND not)"));
        assert!(queries(&Want { artists: vec!["Кино".into()], ..Want::default() }).is_empty(), "nothing to search by");
    }

    #[test]
    fn an_items_tracks_are_its_best_files_without_the_whole_album() {
        let item = item_of(ITEM).unwrap();
        assert_eq!(item.id, "kino-noch-1985");
        let t = tracks(&item);
        let got: Vec<(u32, u32, &str, String)> = t.iter().map(|t| (t.disc, t.number, t.title.as_str(), t.format.label())).collect();
        assert_eq!(
            got,
            [
                (1, 1, "Видели ночь", "FLAC".to_owned()),
                (1, 2, "Фильмы", "FLAC".to_owned()),
                (1, 3, "Твой номер", "FLAC".to_owned()),
                (1, 4, "Танец", "FLAC".to_owned()),
                (2, 1, "Последний герой (демо)", "MP3 192".to_owned()),
            ],
            "the full-album file and the derived MP3s left out"
        );
        assert_eq!(t[1].duration_ms, Some(293_960));
        assert!(t[0].file.ends_with("01 - Видели ночь.flac"));
        assert_eq!(item_of("{}"), None, "an item gone");
        assert_eq!(item_of("{\"is_dark\":true,\"files\":[]}"), None);
    }

    #[test]
    fn the_album_is_its_tracks_of_that_album() {
        let item = item_of(ITEM).unwrap();
        let o = offers("https://archive.org", &item, &noch());
        assert_eq!(o.len(), 1);
        let o = &o[0];
        assert_eq!((o.artist.as_deref(), o.album.as_deref()), (Some("Кино"), Some("Ночь")));
        assert!(o.lossless());
        assert_eq!(o.size_bytes, Some(19_853_929 + 28_858_117 + 21_828_364 + 28_235_877), "the demo of another album out");
        let h: Handle = serde_json::from_str(&o.handle).unwrap();
        let outs: Vec<&str> = h.files.iter().map(|f| f.out.as_str()).collect();
        assert_eq!(outs, ["01 - Видели ночь.flac", "02 - Фильмы.flac", "03 - Твой номер.flac", "04 - Танец.flac"]);
        let ranked = rank(vec![o.clone()], &noch(), &[SourceKey::Archive]);
        assert_eq!(pick(&ranked, None), Some(0), "the artist's album, its name, FLAC: sure");
        // More tracks than the item has: not this album.
        assert!(offers("https://archive.org", &item, &Want { album_tracks: 9, ..noch() }).is_empty());
        // Another artist's wish: offered, but never matched.
        let other = Want { artists: vec!["Аквариум".into()], ..noch() };
        let o = offers("https://archive.org", &item, &other);
        assert!(rank(o, &other, &[]).is_empty());
    }

    #[test]
    fn a_track_is_the_file_with_its_title_and_length() {
        let item = item_of(ITEM).unwrap();
        let o = offers("https://archive.org", &item, &filmy());
        assert_eq!(o.len(), 1, "{o:?}");
        assert_eq!(o[0].track_title.as_deref(), Some("Фильмы"));
        assert_eq!(o[0].duration_ms, Some(293_960));
        let ranked = rank(o.clone(), &filmy(), &[]);
        assert!(ranked[0].verdict.sure);
        // Another recording of the song (a minute longer): not it.
        let long = Want { duration_ms: Some(360_000), ..filmy() };
        assert!(rank(o, &long, &[]).is_empty());
        // A live item is not looked into for a studio song.
        let live = Item { title: "Кино - Ночь (Live 1986)".into(), ..item.clone() };
        assert!(offers("https://archive.org", &live, &filmy()).is_empty());
    }
}
