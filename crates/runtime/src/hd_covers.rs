//! Larger covers for the stage of Главная's worlds: the
//! stage draws a cover of 568 points, up to 850 pixels at 150 %, and some
//! albums come with a cover of 300 px or less (Mushoku Tensei's are
//! 181–400 px). For an album whose own cover is smaller than [`HD_SIDE`],
//! the url of a larger one is looked up — a service's release linked to the
//! album (the same release, whatever kind the service calls it: a service
//! calls an album of three songs a single), the Cover Art Archive by the
//! album's MusicBrainz ids, else the album
//! of its artist found by its title on the services (Deezer, YouTube Music, Spotify) —
//! a single or an EP of its title never stands for an album of more songs
//! (the first single often bears the album's name) unless its songs are
//! counted as many — and kept in the library's cache (`cover_hd.v3.<album>`; a miss too, asked
//! again after [`RETRY_MS`]). Only the url is kept: the picture goes through
//! the web cover cache like any cover of a service, never into the music
//! folders. The stage shows the album's own cover at once and fades the
//! larger one in once it has loaded.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use nikgob_core::model::{ReleaseId, ReleaseType};
use nikgob_core::streaming::{EntityKind, RemoteId, Service, StreamAlbum, StreamService};
use nikgob_library::{Library, now_ms};
use nikgob_metadata::itunes::sized_artwork;
use nikgob_metadata::{CoverTarget, ItunesAlbum, Metadata};

use crate::Worker;
use crate::enrich::Event;
use crate::import::Internal;

/// An album's own cover smaller than this on its long side gets a larger one.
pub(crate) const HD_SIDE: u32 = 1000;
/// A lookup that found nothing is asked again after this long.
const RETRY_MS: i64 = 14 * 86_400_000;
/// Albums looked up per visit of Главная.
const BATCH: usize = 12;
/// The sides a CDN is asked for: Deezer's largest (it gives at most what
/// it has, larger asks are refused), Google's (likewise), the iTunes
/// Store's.
const DEEZER_SIDE: u32 = 1800;
const GOOGLE_SIDE: u32 = 3000;
const ITUNES_SIDE: u32 = 3000;
/// The size a Google or iTunes cover is kept at for the stage.
const STAGE_SIDE: u32 = 1200;
/// A Spotify cover id: its size, then the picture's hash.
const SPOTIFY_SIZES: &[&str] = &["ab67616d00004851", "ab67616d00001e02", "ab67616d0000b273", "ab67616d000082c1"];
const SPOTIFY_LARGE: &str = "ab67616d000082c1";
const SPOTIFY_MEDIUM: &str = "ab67616d0000b273";
/// The time one source has to answer within a lookup with a deadline.
const SOURCE_BUDGET: Duration = Duration::from_secs(20);
/// The linked releases asked, at most.
const MAX_LINKED: usize = 4;

pub(crate) fn key(id: ReleaseId) -> String {
    // `v2`: found with the album's artist (see [`Want::artists`]); `v3`:
    // never a single's art for an album of more songs (see [`Want::takes`]).
    format!("cover_hd.v3.{id}")
}

/// The larger cover found for `id`, if one was.
pub(crate) fn cached(lib: &Library, id: ReleaseId) -> Option<String> {
    lib.cache_get(&key(id)).ok().flatten().map(|(_, url)| url).filter(|u| !u.is_empty())
}

/// Whether `id` was not looked up yet, or found nothing long enough ago.
fn due(lib: &Library, id: ReleaseId, now: i64) -> bool {
    match lib.cache_get(&key(id)) {
        Ok(Some((at, url))) => url.is_empty() && now - at > RETRY_MS,
        Ok(None) => true,
        Err(_) => false,
    }
}

/// The long side of the picture at `path` (its header only).
pub(crate) fn side(path: &Path) -> Option<u32> {
    image::image_dimensions(path).ok().map(|(w, h)| w.max(h))
}

/// What a lookup knows of an album.
#[derive(Clone, Debug)]
pub(crate) struct Want {
    pub id: ReleaseId,
    pub mbid: Option<String>,
    pub group_mbid: Option<String>,
    /// The services' releases linked to it (Deezer's first, then
    /// Spotify's, then YouTube Music's): the same release.
    pub linked: Vec<RemoteId>,
    pub title: String,
    /// The album's artist names: an album found by its title is by one of
    /// them (titles alike are common: «There is a reason (RUS COVER)» is
    /// not «The Rush – Is the Reason»). Empty: the title alone.
    pub artists: Vec<String>,
    pub release_type: ReleaseType,
    /// Its songs: the larger of the count it says and the tracks it has.
    pub tracks: u32,
}

impl Want {
    fn wants_single(&self) -> bool {
        matches!(self.release_type, ReleaseType::Single | ReleaseType::Ep)
    }

    /// One song and no count: nothing tells it from a single.
    fn few(&self) -> bool {
        self.tracks <= 1
    }

    /// A single or an EP, or nothing tells it from one: a song of its
    /// found anywhere stands for it.
    pub(crate) fn song_stands(&self) -> bool {
        self.wants_single() || self.few()
    }

    /// Whether a service's release of `kind` with `n` songs may give its
    /// art. An album of several songs takes nothing of fewer songs than it
    /// has: no single or EP short of them or of songs untold (the library
    /// files many a single of three songs as an album), no album of one
    /// song. An album's count untold is no mismatch.
    pub(crate) fn takes(&self, kind: ReleaseType, n: Option<u32>) -> bool {
        if self.wants_single() || self.few() {
            return true;
        }
        match kind {
            // A single or an EP of songs untold may be just the single.
            ReleaseType::Single | ReleaseType::Ep => n.is_some_and(|n| n >= self.tracks),
            ReleaseType::Album | ReleaseType::Compilation | ReleaseType::Other => n.is_none_or(|n| n > 1),
        }
    }

    /// Whether a release linked to it with `n` songs is it: of whatever
    /// kind the service calls it (an album of three songs is a single on
    /// Deezer, Spotify, YouTube Music and in the iTunes Store), but no
    /// release of one song for an album of several (an import that took
    /// the album for its first single linked that).
    pub(crate) fn linked_fits(&self, n: Option<u32>) -> bool {
        self.song_stands() || n.is_none_or(|n| n > 1)
    }

    /// Like with like: an album's art for an album, a single's for a
    /// single, an EP's for an EP.
    fn prefers(&self, kind: ReleaseType) -> bool {
        let class = |t: ReleaseType| match t {
            ReleaseType::Single => 0,
            ReleaseType::Ep => 1,
            ReleaseType::Album | ReleaseType::Compilation | ReleaseType::Other => 2,
        };
        class(kind) == class(self.release_type)
    }
}

/// A MusicBrainz primary type as a release type (none: an album).
fn mb_kind(primary: Option<&str>) -> ReleaseType {
    match primary.map(str::to_ascii_lowercase).as_deref() {
        Some("single") => ReleaseType::Single,
        Some("ep") => ReleaseType::Ep,
        None | Some("album") => ReleaseType::Album,
        Some(_) => ReleaseType::Other,
    }
}

/// Whether the MusicBrainz release (else group) `w` is linked to may give
/// its art: its kind and songs fit (see [`Want::takes`]). An import that
/// took an album for its first single linked the single. A single, an EP
/// or an album of one song needs no look.
async fn mb_fits(meta: &Metadata, w: &Want) -> Result<bool, String> {
    if w.wants_single() || w.few() {
        return Ok(true);
    }
    let release = if let Some(id) = &w.mbid {
        meta.musicbrainz.lookup_release(id).await.map(Some)
    } else if let Some(group) = &w.group_mbid {
        meta.musicbrainz.browse_release_group_releases(group, 1, 0).await.map(|p| p.items.into_iter().next())
    } else {
        return Ok(true);
    };
    match release {
        Ok(Some(r)) => {
            let kind = mb_kind(r.release_group.as_ref().and_then(|g| g.primary_type.as_deref()));
            let n: u32 = r.media.iter().map(|m| m.track_count).sum();
            let fits = w.takes(kind, (n > 0).then_some(n));
            if !fits {
                log::debug!("the MusicBrainz release linked to «{}» is a {kind:?} of {n} songs", w.title);
            }
            Ok(fits)
        }
        Ok(None) => Ok(false),
        Err(e) if crate::better_covers::meta_trouble(&e) => Err(e.to_string()),
        Err(_) => Ok(false),
    }
}

/// A Deezer `record_type` as a release type.
pub(crate) fn deezer_kind(record_type: Option<&str>) -> ReleaseType {
    match record_type {
        Some("single") => ReleaseType::Single,
        Some("ep") => ReleaseType::Ep,
        Some("compile") => ReleaseType::Compilation,
        _ => ReleaseType::Album,
    }
}

/// The cover of the first of `found` that is `w` ([`pick_hit`]).
fn pick(w: &Want, found: Vec<StreamAlbum>) -> Option<String> {
    pick_hit(w, found).and_then(|a| a.cover_url).filter(|u| !u.is_empty())
}

/// The first of `found` that is `w` (its title, by its artist, of a kind
/// that may stand for it, [`Want::takes`]): of its very title and its own
/// kind, else of its very title, else of its own kind, else the first.
fn pick_hit(w: &Want, found: Vec<StreamAlbum>) -> Option<StreamAlbum> {
    let fits: Vec<StreamAlbum> = found
        .into_iter()
        .filter(|a| {
            same_album(&w.title, &a.title) && by_them(w, a.artists.iter().map(|x| x.name.as_str())) && w.takes(a.release_type, a.total_tracks)
        })
        .collect();
    let exact = |a: &StreamAlbum| same_title(&w.title, &a.title);
    let at = fits
        .iter()
        .position(|a| exact(a) && w.prefers(a.release_type))
        .or_else(|| fits.iter().position(|a| exact(a)))
        .or_else(|| fits.iter().position(|a| w.prefers(a.release_type)))
        .unwrap_or(0);
    fits.into_iter().nth(at)
}

/// Words that tell one part of a series from another: they must be on
/// both titles or on neither («Season 2 Original Soundtrack» is not the
/// first season's).
const PARTS: &[&str] = &["season", "part", "movie", "film", "zero", "ii", "iii", "iv", "live", "concert", "piano", "orchestra"];

/// The words of an album title that tell it apart, lower case, without the
/// words every soundtrack has, and its numbers.
fn words(title: &str) -> (Vec<String>, Vec<String>) {
    const NOISE: &[&str] = &[
        "original", "soundtrack", "ost", "special", "edition", "the", "deluxe", "version", "music", "from", "motion",
        "picture", "tv", "anime", "game", "of", "and", "a", "collection",
    ];
    // «Season2» is «season 2».
    let mut split = String::new();
    let mut prev: Option<char> = None;
    for c in title.to_lowercase().chars() {
        if let Some(p) = prev
            && p.is_alphabetic()
            && c.is_ascii_digit()
        {
            split.push(' ');
        }
        split.push(c);
        prev = Some(c);
    }
    let all: Vec<String> = split
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && !NOISE.contains(w))
        .map(str::to_owned)
        .collect();
    let (mut numbers, names): (Vec<String>, Vec<String>) = all.into_iter().partition(|w| w.chars().all(|c| c.is_ascii_digit()));
    numbers.sort();
    numbers.dedup();
    (names, numbers)
}

/// Whether a service's album title is the same album as ours: the same
/// numbers («Original Soundtrack 2» is not «… 1»), the same part of the
/// series, and at least two of the words or half of the shorter title's.
pub(crate) fn same_album(ours: &str, theirs: &str) -> bool {
    let (a, an) = words(ours);
    let (b, bn) = words(theirs);
    if an != bn || a.is_empty() || b.is_empty() {
        return false;
    }
    if PARTS.iter().any(|p| a.iter().any(|w| w == p) != b.iter().any(|w| w == p)) {
        return false;
    }
    // One with words the other lacks while it has none of its own is a
    // part of it («Hollow Knight: Silksong» of «Hollow Knight»).
    let (only_a, only_b) = (a.iter().any(|w| !b.contains(w)), b.iter().any(|w| !a.contains(w)));
    if only_a != only_b {
        return false;
    }
    let shared = a.iter().filter(|w| b.contains(w)).count();
    shared >= 2 || shared * 2 >= a.len().min(b.len())
}

/// Where a cover found by the album's title comes from, in the order an
/// anchor is taken: Deezer, the iTunes Store, YouTube Music, the others.
pub(crate) fn source_order(service: Option<Service>) -> u8 {
    match service {
        Some(Service::Deezer) => 0,
        None => 1,
        Some(Service::Youtube) => 2,
        Some(_) => 3,
    }
}

/// A cover of an album found online.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Found {
    /// Of a release linked to the album whose songs fit it (a service's, or
    /// the MusicBrainz release that fits it), not of one found by its title.
    pub linked: bool,
    /// Its title is the album's very title, word for word ([`same_album`]
    /// lets «Deluxe» and the like go).
    pub exact: bool,
    /// [`source_order`] of where it was found.
    pub order: u8,
    /// Nothing tells its songs (a linked Spotify album by its public
    /// page): it only lends its size to a verified cover of the same
    /// picture, never stands for the album itself.
    pub lends: bool,
    /// The renditions to try, the largest first; the last is the url the
    /// source gave.
    pub urls: Vec<String>,
    /// What the stage takes: the source's url, or a rendition of about
    /// [`STAGE_SIDE`] made of it (kept only once it came).
    pub stage: String,
}

impl Found {
    fn new(linked: bool, exact: bool, order: u8, url: &str) -> Self {
        let (urls, stage) = renditions(url);
        Self { linked, exact, order, lends: false, urls, stage }
    }

    /// The url the source gave.
    pub(crate) fn original(&self) -> &str {
        self.urls.last().map_or("", String::as_str)
    }

    /// Its place among the covers found: the linked ones first (in the
    /// order they were asked), then those of the album's very title, then
    /// the others alike, each by [`source_order`]; those that only lend
    /// their size last.
    fn rank(&self) -> (u8, u8) {
        match (self.lends, self.linked, self.exact) {
            (true, _, _) => (3, self.order),
            (false, true, _) => (0, 0),
            (false, false, true) => (1, self.order),
            (false, false, false) => (2, self.order),
        }
    }
}

/// The renditions of the cover at `url`, the largest first and the url
/// itself last — Deezer's at [`DEEZER_SIDE`], Google's at [`GOOGLE_SIDE`],
/// the iTunes Store's at [`ITUNES_SIDE`] and [`STAGE_SIDE`], Spotify's
/// largest and its 640 px — and what the stage takes. A CDN gives at most
/// the picture it has: the bytes tell its real size.
pub(crate) fn renditions(url: &str) -> (Vec<String>, String) {
    let own = url.to_owned();
    let (mut urls, stage) = if let Some(larger) = deezer_sized(url, DEEZER_SIDE) {
        (vec![larger, own.clone()], own)
    } else if is_google(url) {
        (vec![google_sized(url, GOOGLE_SIDE), own], google_sized(url, STAGE_SIDE))
    } else if url.contains(".mzstatic.com/") {
        (vec![sized_artwork(url, ITUNES_SIDE), sized_artwork(url, STAGE_SIDE), own], sized_artwork(url, STAGE_SIDE))
    } else if let Some(hash) = spotify_hash(url) {
        let medium = format!("https://i.scdn.co/image/{SPOTIFY_MEDIUM}{hash}");
        (vec![format!("https://i.scdn.co/image/{SPOTIFY_LARGE}{hash}"), medium.clone(), own], medium)
    } else {
        (vec![own.clone()], own)
    };
    urls.dedup();
    (urls, stage)
}

/// A Deezer cover (`…/images/cover/<md5>/1000x1000-000000-80-0-0.jpg`) at
/// `side`, when it is asked smaller.
fn deezer_sized(url: &str, side: u32) -> Option<String> {
    if !url.contains(".dzcdn.net/images/") {
        return None;
    }
    let (base, file) = url.rsplit_once('/')?;
    let (size, rest) = file.split_once('-')?;
    let (w, h) = size.split_once('x')?;
    let n: u32 = w.parse().ok()?;
    (w == h && n < side).then(|| format!("{base}/{side}x{side}-{rest}"))
}

fn is_google(url: &str) -> bool {
    url.contains(".googleusercontent.com/") || url.contains(".ggpht.com/")
}

/// A Google-hosted square cover (`…=w544-h544-l90-rj`) at `side`, its other
/// options kept; one of another shape as it is.
fn google_sized(url: &str, side: u32) -> String {
    let (base, params) = url.rsplit_once('=').unwrap_or((url, ""));
    let parts: Vec<&str> = params.split('-').filter(|p| !p.is_empty()).collect();
    let sized = |p: &str| {
        let mut c = p.chars();
        matches!(c.next(), Some('w' | 'h' | 's')) && c.as_str().parse::<u32>().is_ok()
    };
    let num = |k: char| parts.iter().find_map(|p| p.strip_prefix(k).filter(|_| sized(*p)).and_then(|n| n.parse::<u32>().ok()));
    if let (Some(w), Some(h)) = (num('w'), num('h'))
        && w != h
    {
        return url.to_owned();
    }
    let mut out = format!("{base}=w{side}-h{side}");
    for p in parts.into_iter().filter(|p| !sized(*p)) {
        out.push('-');
        out.push_str(p);
    }
    out
}

/// The picture's hash in a Spotify cover url (`…/image/ab67616d0000b273<hash>`).
fn spotify_hash(url: &str) -> Option<&str> {
    if !(url.contains("i.scdn.co/image/") || url.contains(".spotifycdn.com/image/")) {
        return None;
    }
    let id = url.rsplit('/').next()?;
    SPOTIFY_SIZES
        .iter()
        .find_map(|p| id.strip_prefix(p))
        .filter(|h| !h.is_empty() && h.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Whether a release by `names` may be `w`'s: by one of its artists (any,
/// when it knows none).
fn by_them<'a>(w: &Want, mut names: impl Iterator<Item = &'a str>) -> bool {
    w.artists.is_empty() || names.any(|n| crate::better_covers::names_artist(&w.artists, n))
}

/// Whether two titles are the same words, word for word (lower case).
fn same_title(ours: &str, theirs: &str) -> bool {
    let words = |t: &str| -> Vec<String> { t.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_owned).collect() };
    let a = words(ours);
    !a.is_empty() && a == words(theirs)
}

/// The first of `found` that may be `w` but tells too little to be taken:
/// a single or an EP of its very title, by its artist, its songs untold
/// (YouTube Music counts none in a search). Asked for its songs, it is `w`
/// when they are as many ([`Want::takes`]).
fn untold<'a>(w: &Want, found: &'a [StreamAlbum]) -> Option<&'a StreamAlbum> {
    if w.song_stands() {
        return None;
    }
    found.iter().find(|a| {
        matches!(a.release_type, ReleaseType::Single | ReleaseType::Ep)
            && a.total_tracks.is_none()
            && same_title(&w.title, &a.title)
            && by_them(w, a.artists.iter().map(|x| x.name.as_str()))
    })
}

/// An iTunes Store album name as a title and a kind: the store names a
/// single «… - Single» and an EP «… - EP».
fn itunes_kind(name: &str) -> (&str, ReleaseType) {
    if let Some(t) = name.strip_suffix(" - Single") {
        (t, ReleaseType::Single)
    } else if let Some(t) = name.strip_suffix(" - EP") {
        (t, ReleaseType::Ep)
    } else {
        (name, ReleaseType::Album)
    }
}

/// The art of the first of the iTunes Store's `albums` that is `w` (as
/// [`pick`]: its title, by its artist, of a kind and songs that may stand
/// for it, its very title first, then its own kind), and whether its title
/// is the album's very title.
fn itunes_pick(w: &Want, albums: &[ItunesAlbum]) -> Option<(String, bool)> {
    let fits: Vec<(ReleaseType, bool, &ItunesAlbum)> = albums
        .iter()
        .filter_map(|a| {
            let (title, kind) = itunes_kind(&a.collection_name);
            (same_album(&w.title, title) && by_them(w, std::iter::once(a.artist_name.as_str())) && w.takes(kind, a.track_count))
                .then(|| (kind, same_title(&w.title, title), a))
        })
        .collect();
    let at = fits
        .iter()
        .position(|(k, exact, _)| *exact && w.prefers(*k))
        .or_else(|| fits.iter().position(|(_, exact, _)| *exact))
        .unwrap_or(0);
    let (_, exact, a) = fits.get(at)?;
    a.artwork_url100.clone().filter(|u| !u.is_empty()).map(|u| (u, *exact))
}

/// The cover of a Spotify album linked to `w` as its public page tells it:
/// of a title alike, never a linked release. Its songs untold, for an album
/// of several songs it only lends its size to a verified cover of the same
/// picture ([`Found::lends`]): it may be the album's first single.
fn oembed_found(w: &Want, art: &nikgob_metadata::SpotifyAlbumArt) -> Option<Found> {
    same_album(&w.title, &art.title).then(|| Found {
        lends: !w.song_stands(),
        ..Found::new(false, same_title(&w.title, &art.title), source_order(Some(Service::Spotify)), &art.thumbnail)
    })
}

/// The cover of the release `id` linked to `w`, when its songs fit
/// ([`Want::linked_fits`]): a Deezer album by the catalogue, else by the
/// service it is of. None for a release of a service that is off.
async fn linked_cover(meta: Option<&Metadata>, services: &[Arc<dyn StreamService>], w: &Want, id: &RemoteId) -> Result<Option<String>, String> {
    let some = |u: Option<String>| u.filter(|u| !u.is_empty());
    if id.service == Service::Deezer
        && let Some(meta) = meta
    {
        let Ok(n) = id.id.parse::<u64>() else { return Ok(None) };
        return match meta.deezer.album(n).await {
            Ok(a) if !w.linked_fits(a.nb_tracks) => {
                log::debug!("the Deezer album {n} linked to «{}» has {:?} songs", w.title, a.nb_tracks);
                Ok(None)
            }
            Ok(a) => Ok(some(a.cover_xl)),
            Err(e) if crate::better_covers::meta_trouble(&e) => Err(format!("Deezer: {e}")),
            Err(_) => Ok(None),
        };
    }
    let Some(s) = services.iter().find(|s| s.service() == id.service) else { return Ok(None) };
    match s.album(&id.id).await {
        Ok(a) => {
            let n = a.total_tracks.or_else(|| u32::try_from(a.tracks.len()).ok().filter(|n| *n > 0));
            if w.linked_fits(n) {
                Ok(some(a.cover_url))
            } else {
                log::debug!("the album {} linked to «{}» has {n:?} songs", id.id, w.title);
                Ok(None)
            }
        }
        Err(e) if crate::better_covers::stream_trouble(&e) => Err(format!("{}: {e}", id.service)),
        Err(_) => Ok(None),
    }
}

/// `fut` in the time one source has ([`SOURCE_BUDGET`]), never past
/// `deadline` (none: no limit). None: no answer in time.
async fn in_time<T>(deadline: Option<Instant>, fut: impl std::future::Future<Output = T>) -> Option<T> {
    match deadline {
        None => Some(fut.await),
        Some(d) => tokio::time::timeout_at(d.min(Instant::now() + SOURCE_BUDGET), fut).await.ok(),
    }
}

fn past(deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|d| Instant::now() >= d)
}

/// The covers of `w` found online, best first ([`Found::rank`]): of the
/// releases linked to it ([`linked_cover`]; at most [`MAX_LINKED`]), of the
/// Cover Art Archive by its MusicBrainz ids when MusicBrainz says they fit,
/// of the album of its title on each service ([`pick`]; else a single or an
/// EP of its very title and songs untold, asked for its songs: one request
/// a service); with `all` also of the iTunes Store and of a linked Spotify
/// album whose service is off (by its public page, as one alike by its
/// title). Without `all` it stops at the first step that found one. Each
/// source has its time and none goes past `deadline`: what was found by
/// then is the answer. A source that could not answer makes it no miss:
/// Err when nothing was found and one could not.
pub(crate) async fn find_all(
    meta: Option<&Metadata>,
    services: &[Arc<dyn StreamService>],
    w: &Want,
    all: bool,
    deadline: Option<Instant>,
) -> Result<Vec<Found>, String> {
    let mut out: Vec<Found> = Vec::new();
    let mut trouble: Option<String> = None;
    let late = |what: &dyn std::fmt::Display| format!("{what}: no answer in time");
    'look: {
        for id in w.linked.iter().take(MAX_LINKED) {
            if past(deadline) {
                break 'look;
            }
            let off = !services.iter().any(|s| s.service() == id.service);
            if id.service == Service::Spotify && off {
                if let (true, Some(meta)) = (all, meta) {
                    match in_time(deadline, meta.web_images.spotify_album_art(&id.id)).await {
                        Some(Ok(art)) => out.extend(art.and_then(|a| oembed_found(w, &a))),
                        Some(Err(e)) if crate::better_covers::meta_trouble(&e) => {
                            trouble.get_or_insert(format!("Spotify: {e}"));
                        }
                        Some(Err(_)) => {}
                        None => {
                            trouble.get_or_insert(late(&"Spotify"));
                        }
                    }
                }
                continue;
            }
            match in_time(deadline, linked_cover(meta, services, w, id)).await {
                Some(Ok(Some(url))) => out.push(Found::new(true, true, source_order(Some(id.service)), &url)),
                Some(Ok(None)) => {}
                Some(Err(e)) => {
                    log::debug!("the cover of «{}» linked on {}: {e}", w.title, id.service);
                    trouble.get_or_insert(e);
                }
                None => {
                    trouble.get_or_insert(late(&id.service));
                }
            }
            // The first linked cover is the answer of a lookup for the
            // first one.
            if !all && !out.is_empty() {
                break 'look;
            }
        }
        if let Some(meta) = meta {
            let fits = if w.mbid.is_some() || w.group_mbid.is_some() {
                match in_time(deadline, mb_fits(meta, w)).await {
                    Some(Ok(fits)) => fits,
                    Some(Err(e)) => {
                        trouble.get_or_insert(e);
                        false
                    }
                    None => {
                        trouble.get_or_insert(late(&"MusicBrainz"));
                        false
                    }
                }
            } else {
                false
            };
            for target in [w.mbid.clone().map(CoverTarget::Release), w.group_mbid.clone().map(CoverTarget::ReleaseGroup)]
                .into_iter()
                .flatten()
                .filter(|_| fits)
            {
                match in_time(deadline, meta.cover_art.front_cover(&target)).await {
                    Some(Ok(Some(c))) if c.thumbnails.contains_key("1200") => {
                        out.push(Found::new(true, true, 0, c.url_for(1200)));
                        break;
                    }
                    Some(Err(e)) if crate::better_covers::meta_trouble(&e) => {
                        trouble.get_or_insert(e.to_string());
                        break;
                    }
                    None => {
                        trouble.get_or_insert(late(&"Cover Art Archive"));
                        break;
                    }
                    Some(_) => {}
                }
            }
            if !all && !out.is_empty() {
                break 'look;
            }
        }
        // By its title on the services.
        let query = search_query(&w.title);
        for s in services {
            if past(deadline) {
                break 'look;
            }
            let order = source_order(Some(s.service()));
            let found = match in_time(deadline, s.search(&query)).await {
                Some(Ok(r)) => r.albums,
                Some(Err(e)) => {
                    log::debug!("a larger cover of «{}» on {}: {e}", w.title, s.service());
                    if crate::better_covers::stream_trouble(&e) {
                        trouble.get_or_insert(format!("{}: {e}", s.service()));
                    }
                    continue;
                }
                None => {
                    trouble.get_or_insert(late(&s.service()));
                    continue;
                }
            };
            // Its very title first: one of songs untold is asked for them
            // before one only alike is taken.
            let hit = pick_hit(w, found.clone());
            let exact = hit.as_ref().is_some_and(|h| same_title(&w.title, &h.title));
            let mut taken = None;
            if !exact && let Some(hit) = untold(w, &found) {
                match in_time(deadline, s.album(&hit.id.id)).await {
                    Some(Ok(a)) => {
                        let n = a.total_tracks.or_else(|| u32::try_from(a.tracks.len()).ok().filter(|n| *n > 0));
                        if w.takes(hit.release_type, n) {
                            taken = a.cover_url.or_else(|| hit.cover_url.clone()).filter(|u| !u.is_empty()).map(|u| (u, true));
                        } else {
                            log::debug!("the {:?} «{}» on {} has {n:?} songs", hit.release_type, hit.title, s.service());
                        }
                    }
                    Some(Err(e)) => {
                        log::debug!("the songs of «{}» on {}: {e}", hit.title, s.service());
                        if crate::better_covers::stream_trouble(&e) {
                            trouble.get_or_insert(format!("{}: {e}", s.service()));
                        }
                    }
                    None => {
                        trouble.get_or_insert(late(&s.service()));
                    }
                }
            }
            let taken = taken.or_else(|| hit.and_then(|h| h.cover_url).filter(|u| !u.is_empty()).map(|u| (u, exact)));
            if let Some((url, exact)) = taken {
                out.push(Found::new(false, exact, order, &url));
            }
            if !all && !out.is_empty() {
                break 'look;
            }
        }
        if all
            && !past(deadline)
            && let Some(meta) = meta
        {
            let term = w.artists.first().map_or_else(|| query.clone(), |a| format!("{a} {query}"));
            match in_time(deadline, meta.itunes.search_albums(&term, 10)).await {
                Some(Ok(albums)) => {
                    if let Some((url, exact)) = itunes_pick(w, &albums) {
                        out.push(Found::new(false, exact, source_order(None), &url));
                    }
                }
                Some(Err(e)) => {
                    log::debug!("a cover of «{}» in the iTunes Store: {e}", w.title);
                    if crate::better_covers::meta_trouble(&e) {
                        trouble.get_or_insert(format!("iTunes: {e}"));
                    }
                }
                None => {
                    trouble.get_or_insert(late(&"iTunes"));
                }
            }
        }
    }
    if past(deadline) {
        trouble.get_or_insert("the lookup ran out of time".to_owned());
    }
    out.sort_by_key(Found::rank);
    match trouble {
        Some(e) if out.is_empty() => Err(e),
        _ => Ok(out),
    }
}

/// The url of a larger cover of `w` for the stage, if a source has one:
/// the best [`find_all`] finds, as its source gave it (never a rendition
/// nobody asked for yet).
pub(crate) async fn find(meta: Option<&Metadata>, services: &[Arc<dyn StreamService>], w: &Want) -> Result<Option<String>, String> {
    let found = find_all(meta, services, w, false, None).await?;
    Ok(found.first().map(|f| f.original().to_owned()).filter(|u| !u.is_empty()))
}

/// An album title as a search query: without a «~subtitle~» and «Special
/// Edition», which the services spell otherwise.
fn search_query(title: &str) -> String {
    let mut s = title.to_owned();
    for (open, close) in [('~', '~'), ('〜', '〜')] {
        if let Some(a) = s.find(open)
            && let Some(b) = s[a + open.len_utf8()..].find(close)
        {
            s.replace_range(a..a + open.len_utf8() + b + close.len_utf8(), " ");
        }
    }
    s.replace("Special Edition", "").split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Worker {
    /// Looks up larger covers for these albums (the stages of the worlds)
    /// in the background, those whose own cover is small and that were not
    /// asked lately.
    pub(crate) fn kick_hd_covers(&mut self, albums: impl IntoIterator<Item = ReleaseId>) {
        let now = now_ms();
        let mut wants = Vec::new();
        for id in albums {
            if wants.len() == BATCH {
                break;
            }
            if self.hd_asked.contains(&id) || !due(&self.lib, id, now) {
                continue;
            }
            let Ok(Some(r)) = self.lib.get_release(id) else { continue };
            // Its cover was refreshed by hand.
            if crate::cover_refresh::decided(&self.lib, id, r.cover_ref.as_deref()) {
                continue;
            }
            // Only a file cover (a service's is as large as it comes).
            let Some(cover) = r.cover_ref.as_deref().filter(|c| !nikgob_library::is_web_ref(c)) else { continue };
            if side(&self.res.path(cover)).is_some_and(|s| s >= HD_SIDE) {
                continue;
            }
            wants.push(want_of(&self.lib, &r, vec![r.artist_credit.display()]));
        }
        if wants.is_empty() {
            return;
        }
        let meta = self.import.as_ref().filter(|s| s.config().fetch_cover_online).and_then(|s| s.metadata());
        let services: Vec<Arc<dyn StreamService>> = [Service::Deezer, Service::Youtube, Service::Spotify].into_iter().filter_map(|k| self.service(k)).collect();
        if meta.is_none() && services.is_empty() {
            return;
        }
        self.hd_asked.extend(wants.iter().map(|w| w.id));
        let lib = self.lib.clone();
        self.streaming.spawn_reporting(move |tx| async move {
            let mut failures = 0;
            for w in wants {
                crate::quiet::free().await;
                match tokio::time::timeout(Duration::from_secs(45), find(meta.as_ref(), &services, &w)).await {
                    Ok(Ok(found)) => {
                        failures = 0;
                        let url = found.unwrap_or_default();
                        // A refresh by hand since this began has the last word.
                        let decision = crate::cover_refresh::decision_key(w.id);
                        if !lib.cache_put_unless(&key(w.id), now_ms(), &url, &decision, now).unwrap_or(false) {
                            continue;
                        }
                        if url.is_empty() {
                            log::debug!("no larger cover of «{}»", w.title);
                        } else {
                            log::info!("a larger cover of «{}»: {url}", w.title);
                            let _ = tx.send(Internal::Enrich(Event::HdCover(w.id)));
                        }
                    }
                    Ok(Err(e)) => {
                        log::debug!("a larger cover of «{}»: {e}", w.title);
                        failures += 1;
                    }
                    Err(_) => failures += 1,
                }
                if failures >= 3 {
                    return;
                }
            }
        });
    }

    /// A larger cover was found for `id`: the worlds on screen take it.
    pub(crate) fn hd_cover_found(&mut self, id: ReleaseId) {
        let Some(url) = cached(&self.lib, id) else { return };
        let cover = nikgob_view::Cover::web(url);
        // Kept for a home page on its way too.
        let on_home = cover.clone();
        self.patch_home(move |home| {
            for w in &mut home.worlds {
                if w.release == Some(id) {
                    w.cover_hd = Some(on_home.clone());
                }
            }
        });
        self.publish(|s| {
            if let Some(w) = s.world.as_mut().filter(|w| w.world.release == Some(id)) {
                w.world.cover_hd = Some(cover.clone());
            }
        });
    }
}

/// The lookup of release `r` by `artists` (empty: the title alone).
pub(crate) fn want_of(lib: &Library, r: &nikgob_core::model::Release, artists: Vec<String>) -> Want {
    let mut linked = lib.external_ids(EntityKind::Album, *r.id.as_uuid()).unwrap_or_default();
    // Deezer's first (one request, its cover large), YouTube Music's last.
    linked.sort_by_key(|x| match x.service {
        Service::Deezer => 0,
        Service::Spotify => 1,
        Service::Youtube => 2,
        _ => 3,
    });
    let have = lib.tracks_of_release(r.id).map_or(0, |t| u32::try_from(t.len()).unwrap_or(u32::MAX));
    Want {
        id: r.id,
        mbid: r.mbid.clone(),
        group_mbid: r.release_group_mbid.clone(),
        linked,
        title: r.title.clone(),
        artists,
        release_type: r.release_type,
        tracks: r.total_tracks.unwrap_or(0).max(have),
    }
}

/// The albums already asked in this run.
pub(crate) type Asked = HashSet<ReleaseId>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_album_of_a_service_is_ours_by_its_words_numbers_and_part() {
        let ours = "Mushoku Tensei ~Isekai Ittara Honki Dasu~ Original Soundtrack 2 Special Edition";
        assert!(same_album(ours, "Mushoku Tensei: Jobless Reincarnation (Original Soundtrack 2)"));
        assert!(!same_album(ours, "Mushoku Tensei: Jobless Reincarnation (Original Soundtrack 1)"));
        // Deezer has the second season's, not the first's.
        assert!(!same_album(ours, "Mushoku Tensei Jobless Reincarnation Season2 Original Soundtrack 2"));
        assert!(same_album(
            "Mushoku Tensei ~Isekai Ittara Honki Dasu~ Season 2 Original Soundtrack",
            "Mushoku Tensei: Jobless Reincarnation Season 2 (Original Soundtrack)"
        ));
        assert!(same_album("Hollow Knight: Gods & Nightmares", "Hollow Knight (Gods & Nightmares)"));
        assert!(!same_album("Hollow Knight: Original Soundtrack", "Hollow Knight: Silksong (Original Soundtrack)"));
        assert!(!same_album("Hollow Knight: Original Soundtrack", "Hollow Knight Soundtrack Piano Collections"));
        assert!(same_album("MADE IN ABYSS ORIGINAL SOUNDTRACK 2", "Made in Abyss Original Soundtrack 2"));
        assert_eq!(
            search_query("Mushoku Tensei ~Isekai Ittara Honki Dasu~ Original Soundtrack 1 Special Edition"),
            "Mushoku Tensei Original Soundtrack 1"
        );
    }

    fn found(title: &str, kind: ReleaseType, n: Option<u32>, cover: &str) -> StreamAlbum {
        StreamAlbum {
            id: nikgob_core::streaming::RemoteId::new(Service::Deezer, cover),
            title: title.into(),
            artists: vec![nikgob_core::streaming::ArtistRef::new(None, "Artist")],
            year: Some(2024),
            release_date: None,
            release_type: kind,
            cover_url: Some(format!("https://x/{cover}.jpg")),
            total_tracks: n,
            tracks: Vec::new(),
        }
    }

    fn want(kind: ReleaseType, tracks: u32) -> Want {
        Want {
            id: ReleaseId::new(),
            mbid: None,
            group_mbid: None,
            linked: Vec::new(),
            title: "Рисунки".into(),
            artists: vec!["Artist".into()],
            release_type: kind,
            tracks,
        }
    }

    /// The album's first single bears its name and comes first in the
    /// search: the album of eleven songs takes the album's art, the single
    /// takes its own.
    #[test]
    fn an_album_never_takes_the_art_of_its_single_of_the_same_title() {
        let hits = || {
            vec![
                found("Рисунки", ReleaseType::Single, Some(1), "single"),
                found("Рисунки", ReleaseType::Ep, Some(3), "ep"),
                found("Рисунки", ReleaseType::Album, Some(11), "album"),
            ]
        };
        assert_eq!(pick(&want(ReleaseType::Album, 11), hits()).as_deref(), Some("https://x/album.jpg"));
        assert_eq!(pick(&want(ReleaseType::Single, 1), hits()).as_deref(), Some("https://x/single.jpg"));
        // The album listed first: a single still takes the single's.
        let mut reversed = hits();
        reversed.reverse();
        assert_eq!(pick(&want(ReleaseType::Single, 1), reversed).as_deref(), Some("https://x/single.jpg"));
        // An album of which one song came in, no count: an album's art.
        assert_eq!(pick(&want(ReleaseType::Album, 1), hits()).as_deref(), Some("https://x/album.jpg"));
        // An EP the library files as an album: the EP of as many songs.
        assert_eq!(pick(&want(ReleaseType::Album, 3), hits()[..2].to_vec()).as_deref(), Some("https://x/ep.jpg"));
        assert_eq!(pick(&want(ReleaseType::Album, 11), hits()[..2].to_vec()), None);
        // A YouTube single tells no count: it may be the single, not taken.
        let untold = vec![found("Рисунки", ReleaseType::Single, None, "yt-single"), found("Рисунки", ReleaseType::Album, None, "yt-album")];
        assert_eq!(pick(&want(ReleaseType::Album, 5), untold[..1].to_vec()), None);
        assert_eq!(pick(&want(ReleaseType::Album, 5), untold.clone()).as_deref(), Some("https://x/yt-album.jpg"));
        // A single takes the single's art over the album's.
        assert_eq!(pick(&want(ReleaseType::Single, 1), untold).as_deref(), Some("https://x/yt-single.jpg"));
    }

    /// A single or an EP found for an album of several songs is taken only
    /// when its songs are counted as many; what MusicBrainz says of a linked
    /// release likewise.
    #[test]
    fn a_found_single_gives_no_art_to_an_album() {
        let album = want(ReleaseType::Album, 9);
        assert!(!album.takes(deezer_kind(Some("single")), Some(1)));
        assert!(!album.takes(deezer_kind(Some("album")), Some(1)), "an album of one song");
        assert!(!album.takes(deezer_kind(Some("ep")), Some(4)));
        assert!(!album.takes(deezer_kind(Some("single")), None), "a single of songs untold");
        assert!(album.takes(deezer_kind(Some("album")), None), "an album's count untold");
        assert!(album.takes(deezer_kind(Some("album")), Some(9)));
        // A single of three songs the library files as an album of three.
        assert!(want(ReleaseType::Album, 3).takes(deezer_kind(Some("single")), Some(3)));
        assert!(album.takes(deezer_kind(Some("compile")), None));
        assert!(album.takes(deezer_kind(None), None));
        assert!(want(ReleaseType::Single, 1).takes(deezer_kind(Some("single")), Some(1)));
        assert!(want(ReleaseType::Ep, 4).takes(deezer_kind(Some("single")), Some(2)));
        // What MusicBrainz says of a linked release.
        assert!(!album.takes(mb_kind(Some("Single")), Some(1)));
        assert!(!album.takes(mb_kind(Some("EP")), Some(4)));
        assert!(album.takes(mb_kind(Some("Album")), Some(9)));
        assert!(album.takes(mb_kind(None), None));
    }

    /// A release linked to the album is the album whatever kind the
    /// service calls it; only one of one song for an album of several (the
    /// album's first single, linked by a wrong match) is not.
    #[test]
    fn a_linked_release_fits_but_a_linked_single_song() {
        let album = want(ReleaseType::Album, 3);
        assert!(album.linked_fits(Some(3)) && album.linked_fits(Some(2)) && album.linked_fits(None));
        assert!(!album.linked_fits(Some(1)));
        assert!(want(ReleaseType::Single, 1).linked_fits(Some(1)));
        assert!(want(ReleaseType::Album, 1).linked_fits(Some(1)), "one song of it came in");
    }

    /// Each CDN is asked for its largest picture first and the url the
    /// source gave last; the stage takes that url or a rendition of 1200.
    #[test]
    fn a_cover_is_asked_large_on_its_cdn() {
        let dz = "https://cdn-images.dzcdn.net/images/cover/d9e1d3a69e838614ad4a50ecabaa0b46/1000x1000-000000-80-0-0.jpg";
        assert_eq!(
            renditions(dz),
            (
                vec![
                    "https://cdn-images.dzcdn.net/images/cover/d9e1d3a69e838614ad4a50ecabaa0b46/1800x1800-000000-80-0-0.jpg".to_owned(),
                    dz.to_owned()
                ],
                dz.to_owned()
            )
        );
        assert_eq!(renditions("https://cdn-images.dzcdn.net/images/cover/aa/1800x1800-000000-80-0-0.jpg").0.len(), 1);
        let yt = "https://yt3.googleusercontent.com/7MXi=w544-h544-l90-rj";
        assert_eq!(
            renditions(yt),
            (
                vec!["https://yt3.googleusercontent.com/7MXi=w3000-h3000-l90-rj".to_owned(), yt.to_owned()],
                "https://yt3.googleusercontent.com/7MXi=w1200-h1200-l90-rj".to_owned()
            )
        );
        let banner = "https://lh3.googleusercontent.com/b=w2880-h1200-p-l90-rj";
        assert_eq!(renditions(banner), (vec![banner.to_owned()], banner.to_owned()));
        let it = "https://is1-ssl.mzstatic.com/image/thumb/Music211/v4/eb/cover.jpg/100x100bb.jpg";
        let (urls, stage) = renditions(it);
        assert_eq!(
            urls,
            [
                "https://is1-ssl.mzstatic.com/image/thumb/Music211/v4/eb/cover.jpg/3000x3000bb.jpg",
                "https://is1-ssl.mzstatic.com/image/thumb/Music211/v4/eb/cover.jpg/1200x1200bb.jpg",
                it
            ]
        );
        assert_eq!(stage, "https://is1-ssl.mzstatic.com/image/thumb/Music211/v4/eb/cover.jpg/1200x1200bb.jpg");
        let sp = "https://image-cdn-fa.spotifycdn.com/image/ab67616d00001e022c5b24ecfa39523a75c993c4";
        let (urls, stage) = renditions(sp);
        assert_eq!(
            urls,
            [
                "https://i.scdn.co/image/ab67616d000082c12c5b24ecfa39523a75c993c4",
                "https://i.scdn.co/image/ab67616d0000b2732c5b24ecfa39523a75c993c4",
                sp
            ]
        );
        assert_eq!(stage, "https://i.scdn.co/image/ab67616d0000b2732c5b24ecfa39523a75c993c4");
        let caa = "https://coverartarchive.org/release/x/1-1200.jpg";
        assert_eq!(renditions(caa), (vec![caa.to_owned()], caa.to_owned()));
    }

    fn risunki(tracks: u32) -> Want {
        Want { title: "рисунки".into(), artists: vec!["nyan.mp3".into()], ..want(ReleaseType::Album, tracks) }
    }

    /// «рисунки» (an album of three songs in the library) is linked to its
    /// YouTube Music release, which YouTube Music calls a single: its cover
    /// comes first, asked large; the stage keeps the url YouTube Music gave.
    #[tokio::test]
    async fn the_linked_release_is_taken_first_whatever_the_service_calls_it() {
        use nikgob_core::streaming::FakeService;
        let cover = "https://yt3.googleusercontent.com/risunki=w544-h544-l90-rj";
        let yt = Arc::new(FakeService::youtube().with_release("risunki", "рисунки", "nyan.mp3", ReleaseType::Single, 3, false, cover));
        let id = yt.albums().into_iter().find(|a| a.title == "рисунки").unwrap().id;
        let services: Vec<Arc<dyn StreamService>> = vec![yt.clone()];
        let w = Want { linked: vec![id], ..risunki(3) };
        let all = find_all(None, &services, &w, true, None).await.unwrap();
        let first = all.first().unwrap();
        assert!(first.linked);
        assert_eq!(first.urls, ["https://yt3.googleusercontent.com/risunki=w3000-h3000-l90-rj", cover]);
        assert_eq!(find(None, &services, &w).await.unwrap().as_deref(), Some(cover), "never a rendition nobody asked for");
        // Linked by a wrong match: a release of one song is not an album
        // of three.
        let one = Arc::new(FakeService::youtube().with_release("one", "другое", "nyan.mp3", ReleaseType::Single, 1, true, cover));
        let id = one.albums().into_iter().find(|a| a.title == "другое").unwrap().id;
        let services: Vec<Arc<dyn StreamService>> = vec![one];
        assert_eq!(find_all(None, &services, &Want { linked: vec![id], ..risunki(3) }, true, None).await, Ok(Vec::new()));
    }

    /// A linked Spotify album is linked only through its service, which
    /// counts its songs: a single of one song is no album of three. With
    /// the service off its public page tells no songs: what it gives is
    /// only alike by its title, never linked.
    #[tokio::test]
    async fn a_linked_spotify_album_counts_only_with_its_songs() {
        use nikgob_core::streaming::FakeService;
        let cover = "https://i.scdn.co/image/ab67616d0000b2732c5b24ecfa39523a75c993c4";
        let sp = Arc::new(FakeService::spotify().with_release("lead", "рисунки", "nyan.mp3", ReleaseType::Single, 1, true, cover));
        let id = sp.albums().into_iter().find(|a| a.title == "рисунки").unwrap().id;
        let w = Want { linked: vec![id.clone()], ..risunki(3) };
        let services: Vec<Arc<dyn StreamService>> = vec![sp.clone()];
        assert!(find_all(None, &services, &w, true, None).await.unwrap().iter().all(|f| !f.linked));
        // Off: nothing asked without the page's reader, nothing linked.
        assert_eq!(find_all(None, &[], &w, true, None).await, Ok(Vec::new()));
        let art = |title: &str| nikgob_metadata::SpotifyAlbumArt { title: title.into(), thumbnail: cover.into() };
        // Of an album of three: it only lends its size, ranked last.
        let alike = oembed_found(&w, &art("рисунки")).unwrap();
        assert!(!alike.linked && alike.exact && alike.lends);
        assert_eq!(alike.rank(), (3, source_order(Some(Service::Spotify))));
        assert_eq!(oembed_found(&w, &art("Совсем другое")), None);
        // Of a single: nothing to tell it from, it stands for it.
        let single = Want { title: "рисунки".into(), ..want(ReleaseType::Single, 1) };
        assert!(!oembed_found(&single, &art("рисунки")).unwrap().lends);
    }

    /// A lookup for the first cover answers with the first linked one: the
    /// next linked release is not asked.
    #[tokio::test]
    async fn the_first_linked_cover_answers_a_lookup_for_one() {
        use nikgob_core::streaming::FakeService;
        let dz = Arc::new(FakeService::deezer().with_release("r", "рисунки", "nyan.mp3", ReleaseType::Single, 3, true, "https://x/dz.jpg"));
        let yt = Arc::new(FakeService::youtube().with_release("r", "рисунки", "nyan.mp3", ReleaseType::Single, 3, false, "https://x/yt.jpg"));
        let ids = vec![dz.albums().into_iter().find(|a| a.title == "рисунки").unwrap().id, yt.albums().into_iter().find(|a| a.title == "рисунки").unwrap().id];
        let services: Vec<Arc<dyn StreamService>> = vec![dz.clone(), yt.clone()];
        let w = Want { linked: ids, ..risunki(3) };
        assert_eq!(find(None, &services, &w).await.unwrap().as_deref(), Some("https://x/dz.jpg"));
        assert_eq!((dz.calls("album"), yt.calls("album"), dz.calls("search")), (1, 0, 0));
        // A refresh asks them all.
        assert_eq!(find_all(None, &services, &w, true, None).await.unwrap().iter().filter(|f| f.linked).count(), 2);
    }

    /// The album's very title of songs untold is asked for its songs before
    /// a counted one only alike («Deluxe Edition») is taken.
    #[tokio::test]
    async fn the_very_title_of_songs_untold_comes_before_one_alike() {
        use nikgob_core::streaming::FakeService;
        let yt = Arc::new(
            FakeService::youtube()
                .with_release("dlx", "рисунки (Deluxe Edition)", "nyan.mp3", ReleaseType::Album, 3, true, "https://x/deluxe.jpg")
                .with_release("r", "рисунки", "nyan.mp3", ReleaseType::Single, 3, false, "https://x/yt.jpg"),
        );
        let services: Vec<Arc<dyn StreamService>> = vec![yt.clone()];
        let found = find_all(None, &services, &risunki(3), true, None).await.unwrap();
        assert_eq!(found.iter().map(Found::original).collect::<Vec<_>>(), ["https://x/yt.jpg"]);
        assert!(found[0].exact);
        assert_eq!(yt.calls("album"), 1);
        // Its songs fewer: the one alike stands.
        let short = Arc::new(
            FakeService::youtube()
                .with_release("dlx", "рисунки (Deluxe Edition)", "nyan.mp3", ReleaseType::Album, 3, true, "https://x/deluxe.jpg")
                .with_release("r", "рисунки", "nyan.mp3", ReleaseType::Single, 1, false, "https://x/yt.jpg"),
        );
        let services: Vec<Arc<dyn StreamService>> = vec![short];
        let found = find_all(None, &services, &risunki(3), true, None).await.unwrap();
        assert_eq!(found.iter().map(|f| (f.original(), f.exact)).collect::<Vec<_>>(), [("https://x/deluxe.jpg", false)]);
    }

    /// A single of the album's very title whose search hit counts no songs
    /// is asked for its songs (one request): as many as the album's, it is
    /// the album; fewer, it is not.
    #[tokio::test]
    async fn a_single_of_songs_untold_is_asked_for_its_songs() {
        use nikgob_core::streaming::FakeService;
        let cover = "https://yt3.googleusercontent.com/risunki=w544-h544-l90-rj";
        let three = Arc::new(FakeService::youtube().with_release("risunki", "рисунки", "nyan.mp3", ReleaseType::Single, 3, false, cover));
        let services: Vec<Arc<dyn StreamService>> = vec![three.clone()];
        let found = find_all(None, &services, &risunki(3), true, None).await.unwrap();
        assert_eq!(found.len(), 1);
        assert!(!found[0].linked && found[0].exact);
        assert_eq!(found[0].original(), cover);
        assert_eq!(three.calls("album"), 1);
        let one = Arc::new(FakeService::youtube().with_release("lead", "рисунки", "nyan.mp3", ReleaseType::Single, 1, false, cover));
        let services: Vec<Arc<dyn StreamService>> = vec![one.clone()];
        assert_eq!(find(None, &services, &risunki(3)).await, Ok(None));
        assert_eq!(one.calls("album"), 1);
        // Another title or another artist: not asked at all.
        let other = Arc::new(FakeService::youtube().with_release("x", "рисунки 2", "nyan.mp3", ReleaseType::Single, 3, false, cover));
        let services: Vec<Arc<dyn StreamService>> = vec![other.clone()];
        assert_eq!(find(None, &services, &risunki(3)).await, Ok(None));
        assert_eq!(other.calls("album"), 0);
    }

    /// The covers found come best first: the linked ones, then those of
    /// the album's very title, then those only alike («Deluxe Edition»),
    /// each by the source order (Deezer, the iTunes Store, YouTube Music,
    /// the others).
    #[tokio::test]
    async fn the_covers_found_come_linked_then_of_the_very_title_then_alike() {
        use nikgob_core::streaming::FakeService;
        let dz = Arc::new(FakeService::deezer().with_release("dlx", "рисунки (Deluxe Edition)", "nyan.mp3", ReleaseType::Album, 3, true, "https://x/dz.jpg"));
        let yt = Arc::new(FakeService::youtube().with_release("r", "рисунки", "nyan.mp3", ReleaseType::Album, 3, true, "https://x/yt.jpg"));
        let services: Vec<Arc<dyn StreamService>> = vec![dz, yt];
        let found = find_all(None, &services, &risunki(3), true, None).await.unwrap();
        let urls: Vec<&str> = found.iter().map(Found::original).collect();
        assert_eq!(urls, ["https://x/yt.jpg", "https://x/dz.jpg"]);
        assert_eq!((found[0].exact, found[1].exact), (true, false));
        assert!(same_title("Рисунки", "рисунки") && !same_title("рисунки", "рисунки (Deluxe Edition)"));
    }

    /// The iTunes Store names a single «… - Single» and counts its songs.
    #[test]
    fn an_itunes_album_is_the_album_by_its_title_artist_and_songs() {
        let album = |name: &str, artist: &str, n: u32| ItunesAlbum {
            artist_name: artist.into(),
            collection_name: name.into(),
            track_count: Some(n),
            artwork_url100: Some(format!("https://is1-ssl.mzstatic.com/image/thumb/{n}/cover.jpg/100x100bb.jpg")),
        };
        let w = risunki(3);
        assert_eq!(
            itunes_pick(&w, &[album("рисунки - Single", "nyan.mp3", 3)]),
            Some(("https://is1-ssl.mzstatic.com/image/thumb/3/cover.jpg/100x100bb.jpg".to_owned(), true))
        );
        assert_eq!(itunes_pick(&w, &[album("рисунки - Single", "nyan.mp3", 1)]), None, "the lead single");
        assert_eq!(itunes_pick(&w, &[album("рисунки", "Кто-то ещё", 3)]), None);
        // The very title before one only alike.
        let both = [album("рисунки (Deluxe)", "nyan.mp3", 4), album("рисунки", "nyan.mp3", 3)];
        assert_eq!(itunes_pick(&w, &both), Some(("https://is1-ssl.mzstatic.com/image/thumb/3/cover.jpg/100x100bb.jpg".to_owned(), true)));
        assert_eq!(itunes_kind("X - EP"), ("X", ReleaseType::Ep));
    }
}
