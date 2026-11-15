//! SoundCloud's originals as a [`Source`]: a track whose uploader lets it
//! be downloaded has the file it was uploaded as (often a WAV, an AIFF or
//! a FLAC, sometimes an MP3), behind `/tracks/{id}/download`. api-v2 gives
//! that link only to a signed-in user (401 without, checked 2026-11-15),
//! so the source uses the sign-in the app keeps for SoundCloud
//! (crates/soundcloud) and offers nothing while signed out.
//!
//! A wish is searched with the service's own search (its rate limits and
//! pauses hold), the tracks that let their file go are judged by the same
//! rules as every catalogue ([`judge`]), and the best few are looked at:
//! the first bytes of the file say what it is ([`crate::sniff`]), so a WAV
//! is offered as lossless and an MP3 as the MP3 it is, whatever the title
//! says. The file is named by its bytes again once it is here. Tracks
//! only.

use std::path::Path;
use std::sync::Arc;

use nikgob_core::indexer::BoxFuture;
use nikgob_core::source::{Fetched, Format, Offer, OfferScope, Progress, Source, SourceError, SourceKey, Want, WantScope};
use nikgob_core::streaming::StreamError;
use nikgob_soundcloud::{Downloadable, Soundcloud};
use serde::{Deserialize, Serialize};

use crate::http::{client, download, get_range, safe_name};
use crate::matching::{judge, same_artist};
use crate::sniff::{named_by_bytes, sniff_remote, with_average};
use crate::text::catalogue_queries;

/// Matching tracks whose file is looked at for one wish.
const PROBED: usize = 2;

/// What the source needs of SoundCloud (the service, or a fake in tests).
pub trait Originals: Send + Sync {
    fn signed_in(&self) -> bool;
    /// Tracks found for `query` that let their original file go.
    fn find<'a>(&'a self, query: &'a str) -> BoxFuture<'a, Result<Vec<Downloadable>, StreamError>>;
    /// The link of a track's original file.
    fn link(&self, id: u64) -> BoxFuture<'_, Result<String, StreamError>>;
}

impl Originals for Soundcloud {
    fn signed_in(&self) -> bool {
        Soundcloud::signed_in(self)
    }

    fn find<'a>(&'a self, query: &'a str) -> BoxFuture<'a, Result<Vec<Downloadable>, StreamError>> {
        self.downloadable(query)
    }

    fn link(&self, id: u64) -> BoxFuture<'_, Result<String, StreamError>> {
        self.original_url(id)
    }
}

pub struct SoundcloudOriginals {
    sc: Arc<dyn Originals>,
    http: reqwest::Client,
}

/// What a fetch needs.
#[derive(Serialize, Deserialize)]
struct Handle {
    track: u64,
    stem: String,
    ext: String,
}

impl SoundcloudOriginals {
    pub fn new(sc: Arc<dyn Originals>) -> Self {
        Self { sc, http: client() }
    }

    /// What the original at `link` is and how big; `None` when it is not
    /// audio this knows.
    async fn probe(&self, link: &str, secs: Option<u32>) -> Result<Option<(Format, &'static str, Option<u64>)>, SourceError> {
        let http = &self.http;
        let Some((s, size)) = sniff_remote(move |from, len| get_range(http, link, from, len)).await? else { return Ok(None) };
        Ok(Some((with_average(&s, size, secs), s.ext, size)))
    }

    async fn search_track(&self, want: &Want) -> Result<Vec<Offer>, SourceError> {
        if want.scope != WantScope::Track || !self.sc.signed_in() {
            return Ok(Vec::new());
        }
        let mut found: Vec<Offer> = Vec::new();
        for q in catalogue_queries(want).iter().filter(|q| !q.is_empty()) {
            for d in self.sc.find(q).await.map_err(source_error)? {
                if !found.iter().any(|o| o.id == d.id.to_string()) {
                    found.push(offer_of(&d, want));
                }
            }
            if found.iter().any(|o| judge(o, want).is_some_and(|v| v.sure)) {
                break;
            }
        }
        let mut judged: Vec<(f64, Offer)> = found
            .into_iter()
            .filter_map(|o| judge(&o, want).map(|v| (v.score + if v.sure { 1.0 } else { 0.0 }, o)))
            .collect();
        judged.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut out = Vec::new();
        for (_, mut o) in judged.into_iter().take(PROBED) {
            let Ok(id) = o.id.parse::<u64>() else { continue };
            let link = match self.sc.link(id).await {
                Ok(l) => l,
                // Signed out meanwhile: nothing to offer.
                Err(StreamError::NotLoggedIn) => return Ok(Vec::new()),
                // This track's file is closed: another may not be.
                Err(StreamError::Http { status: 401 | 403 | 404 | 410 } | StreamError::Unavailable) => continue,
                Err(e) if out.is_empty() => return Err(source_error(e)),
                Err(_) => break,
            };
            let secs = o.duration_ms.map(|ms| ms / 1000);
            let (format, ext, size) = match self.probe(&link, secs).await {
                Ok(Some(found)) => found,
                Ok(None) | Err(SourceError::Parse(_) | SourceError::Http { status: 403 | 404 | 410 }) => {
                    log::info!("soundcloud: the original of track {id} is not audio this knows");
                    continue;
                }
                Err(e) if out.is_empty() => return Err(e),
                Err(_) => break,
            };
            o.format = Some(format);
            o.size_bytes = size;
            let stem = safe_name(o.track_title.as_deref().unwrap_or(&o.title));
            o.handle = serde_json::to_string(&Handle { track: id, stem, ext: ext.to_owned() }).unwrap_or_default();
            out.push(o);
        }
        Ok(out)
    }
}

/// A source error of the service's.
fn source_error(e: StreamError) -> SourceError {
    match e {
        StreamError::NotConfigured | StreamError::NotLoggedIn => SourceError::NotConfigured,
        StreamError::RateLimited { retry_after } => SourceError::RateLimited { retry_after },
        StreamError::Http { status } => SourceError::Http { status },
        StreamError::Network(s) => SourceError::Unreachable(s),
        StreamError::Parse(s) => SourceError::Parse(s),
        StreamError::Unavailable
        | StreamError::RegionBlocked
        | StreamError::Drm
        | StreamError::AgeRestricted
        | StreamError::PremiumRequired
        | StreamError::Unsupported => SourceError::NothingToFetch,
    }
}

/// Words an uploader adds to a title about the download, not the song's:
/// «[Free Download]», «(FREE DOWNLOAD.wav)», «Buy = Free Download».
fn about_download(text: &str) -> bool {
    let t = text.to_lowercase();
    ["free download", "free dl", "freedl", "download", "buy =", "buy=", "out now"].iter().any(|w| t.contains(w))
        || [".wav", ".mp3", ".flac", ".aif", ".aiff"].iter().any(|e| t.trim_end().ends_with(e))
}

/// Where an ASCII `needle` starts in `hay`, any case, as a byte offset at
/// a character's start.
fn find_ascii_nocase(hay: &str, needle: &str) -> Option<usize> {
    let n = needle.as_bytes();
    hay.char_indices().map(|(i, _)| i).find(|&i| hay.as_bytes().get(i..i + n.len()).is_some_and(|w| w.eq_ignore_ascii_case(n)))
}

/// A title without what the uploader says about the download.
pub fn clean_title(raw: &str) -> String {
    let mut s = raw.trim().to_owned();
    // Brackets about the download.
    loop {
        let mut cut = None;
        for (open, close) in [('(', ')'), ('[', ']'), ('{', '}')] {
            let mut from = 0;
            while let Some(i) = s[from..].find(open).map(|i| from + i) {
                let Some(j) = s[i..].find(close).map(|j| i + j) else { break };
                if about_download(&s[i + 1..j]) {
                    cut = Some((i, j + close.len_utf8()));
                    break;
                }
                from = j;
            }
            if cut.is_some() {
                break;
            }
        }
        match cut {
            Some((i, j)) => s.replace_range(i..j, " "),
            None => break,
        }
    }
    // A tail about the download, after a separator or bare: found in the
    // title itself (a lowercase copy may be longer or shorter in bytes).
    if let Some(at) = ["| free", "// free", "- free download", "free download", "free dl", "buy ="]
        .iter()
        .filter_map(|w| find_ascii_nocase(&s, w))
        .filter(|at| *at > 0)
        .min()
    {
        s.truncate(at);
    }
    for ext in [".wav", ".mp3", ".flac", ".aiff", ".aif"] {
        let t = s.trim_end();
        if t.len() >= ext.len() && t.as_bytes()[t.len() - ext.len()..].eq_ignore_ascii_case(ext.as_bytes()) {
            // ASCII bytes end the title: the cut is between characters.
            let keep = t.len() - ext.len();
            s.truncate(keep);
        }
    }
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    s.trim_matches(|c: char| c.is_whitespace() || matches!(c, '-' | '|' | '/' | '–' | '—')).to_owned()
}

/// An offer of a downloadable track. A title "A - B" names the performer
/// when one side is the wish's artist (an upload by a label, a fan, or the
/// artist naming the song first).
fn offer_of(d: &Downloadable, want: &Want) -> Offer {
    let title = clean_title(&d.title);
    let credited = if d.artists.is_empty() { d.uploader.clone() } else { d.artists.join(", ") };
    let (artist, song) = match title.split_once(" - ") {
        Some((a, b)) if same_artist(a.trim(), want) => (a.trim().to_owned(), b.trim().to_owned()),
        Some((a, b)) if same_artist(b.trim(), want) => (b.trim().to_owned(), a.trim().to_owned()),
        _ => (credited, title.clone()),
    };
    Offer {
        source: SourceKey::Soundcloud,
        provider: SourceKey::Soundcloud.display_name().to_owned(),
        id: d.id.to_string(),
        title: if artist.is_empty() { song.clone() } else { format!("{artist} — {song}") },
        artist: (!artist.is_empty()).then_some(artist),
        track_title: Some(song),
        album: d.album.clone(),
        duration_ms: d.duration_ms.and_then(|ms| u32::try_from(ms).ok()),
        isrc: d.isrc.clone(),
        scope: OfferScope::Track,
        format: None,
        size_bytes: None,
        seeders: None,
        info_url: d.url.clone(),
        handle: String::new(),
    }
}

impl Source for SoundcloudOriginals {
    fn key(&self) -> SourceKey {
        SourceKey::Soundcloud
    }

    fn name(&self) -> &str {
        SourceKey::Soundcloud.display_name()
    }

    fn search<'a>(&'a self, want: &'a Want) -> BoxFuture<'a, Result<Vec<Offer>, SourceError>> {
        Box::pin(self.search_track(want))
    }

    fn fetch<'a>(&'a self, offer: &'a Offer, _want: &'a Want, dest: &'a Path, progress: Progress) -> BoxFuture<'a, Result<Fetched, SourceError>> {
        Box::pin(async move {
            let h: Handle = serde_json::from_str(&offer.handle).map_err(|e| SourceError::Parse(e.to_string()))?;
            // The link is short-lived: asked again right before.
            let link = self.sc.link(h.track).await.map_err(source_error)?;
            let path = download(&self.http, &link, &dest.join(format!("{}.{}", h.stem, h.ext)), &progress).await?;
            Ok(Fetched::Files(vec![named_by_bytes(&path).await?]))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_lose_what_the_uploader_says_about_the_download() {
        assert_eq!(clean_title("LA PAZ - NIXON (FREE DOWNLOAD.wav)"), "LA PAZ - NIXON");
        assert_eq!(clean_title("Ell Kingston, Enzo Draper - Critizice Edit [Free Download].wav"), "Ell Kingston, Enzo Draper - Critizice Edit");
        assert_eq!(clean_title("1234 (Deka Edit)  Buy = Free Download"), "1234 (Deka Edit)");
        assert_eq!(clean_title("UN_WAV - SOCCA [FREE DOWNLOAD]"), "UN_WAV - SOCCA");
        assert_eq!(clean_title("Night Drive (Extended Mix)"), "Night Drive (Extended Mix)");
        assert_eq!(clean_title("Song | FREE DL"), "Song");
        // Letters whose lowercase is shorter or longer in bytes: no cut
        // inside a character.
        assert_eq!(clean_title("ẞẞé free download"), "ẞẞé");
        assert_eq!(clean_title("İstanbul İİ Buy = Free Download"), "İstanbul İİ");
        assert_eq!(clean_title("Ⱥⱥ Song.WAV"), "Ⱥⱥ Song");
        assert_eq!(find_ascii_nocase("ẞẞé free download", "free download"), Some(9));
    }

    #[test]
    fn the_performer_is_read_out_of_the_title() {
        let want = Want { artists: vec!["NIXON".into()], title: Some("La Paz".into()), ..Want::default() };
        let d = Downloadable {
            id: 2437472571,
            title: "LA PAZ - NIXON (FREE DOWNLOAD.wav)".into(),
            artists: vec!["NIXON".into()],
            uploader: "NIXON".into(),
            duration_ms: Some(136_615),
            ..Downloadable::default()
        };
        let o = offer_of(&d, &want);
        assert_eq!((o.artist.as_deref(), o.track_title.as_deref()), (Some("NIXON"), Some("LA PAZ")));
        assert!(judge(&o, &want).is_some_and(|v| v.score > 0.5));
        let label = Downloadable { title: "NIXON - La Paz".into(), artists: vec!["Some Records".into()], uploader: "Some Records".into(), ..d.clone() };
        let o = offer_of(&label, &want);
        assert_eq!((o.artist.as_deref(), o.track_title.as_deref()), (Some("NIXON"), Some("La Paz")));
        assert!(judge(&o, &want).is_some());
        assert_eq!(source_error(StreamError::NotLoggedIn), SourceError::NotConfigured);
        assert_eq!(source_error(StreamError::Unavailable), SourceError::NothingToFetch);
    }
}
