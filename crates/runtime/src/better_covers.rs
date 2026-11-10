//! Real album art for tracks and albums that show a poor picture: a frame of
//! a music video (a clip without its song, a file saved from YouTube that
//! embedded the clip's thumbnail), a tiny cover (Mushoku Tensei's soundtracks
//! come with 300 px ones), or none at all.
//!
//! The art is looked up by what is known of the song: the Cover Art Archive
//! by the album's MusicBrainz ids, the Deezer album linked to it and the
//! services' album of the same title ([`crate::hd_covers::find`]); then the
//! song by its artist and title on Deezer, the iTunes Search API and the
//! services' songs (YouTube Music's square album art). A hit counts only
//! when its title is the song's (or its album the album) and its artist is
//! the track's, or is named in the clip's title («ヨルシカ「火星人」» uploaded
//! by a TV channel). An album takes only the art of an album of its title:
//! its song found on a single is the single's art, not the album's.
//!
//! Before the lookup the names are widened with what the library knows:
//! every spelling of the artist (its aliases, «ヨルシカ» read as
//! «Yorushika»), the tracks' other titles, and the library's songs of that
//! artist that carry the title beside another one («火星人 - Martian»): the
//! services often know a song only by its Latin title.
//!
//! A sweep goes through the whole library at the services' rate limits; a
//! miss is normal and the sweep goes on. A service that cannot answer (no
//! network, a refusal) stops it, and the next start takes it up again.
//!
//! What is found is kept as a url in the library's cache
//! (`cover_better.v2.<release>` / `cover_better.track.<track>`, a miss too,
//! asked again after [`RETRY_MS`], so art that appears later is picked up)
//! together with the cover it replaces: a file cover imported later wins
//! again. The picture goes through the web cover cache, never into the
//! music folders. Requests are spaced by the metadata clients' rate limits.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use nikgob_core::model::{ArtistId, ReleaseId, TrackId};
use nikgob_core::streaming::{EntityKind, Service, StreamError, StreamService, TrackKind};
use nikgob_core::{cover_frame, names};
use nikgob_library::{Library, TrackRow, is_web_ref, now_ms};
use nikgob_metadata::text::{artist_key, similarity, title_key};
use nikgob_metadata::{Metadata, MetadataError};

use crate::Worker;
use crate::enrich::Event;
use crate::hd_covers;
use crate::import::Internal;

/// A lookup that found nothing is asked again after this long.
const RETRY_MS: i64 = 14 * 86_400_000;
/// A file cover smaller than this on its long side is poor.
pub(crate) const MIN_SIDE: u32 = 400;
/// One lookup (every query of it, waiting for the rate limits) takes at
/// most this long; longer, and the network is in trouble.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(180);
/// Queries of one lookup, at most.
const MAX_QUERIES: usize = 6;
/// Covers on screen take what a sweep found at most this often.
const SHOW_EVERY: Duration = Duration::from_secs(30);
/// Before the first lookup of a sweep: playback and the first screens go
/// first.
const START: Duration = Duration::from_secs(30);
/// Separates the cover a lookup replaced from the url it found.
const SEP: char = '\u{1f}';

/// What a lookup is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Target {
    Release(ReleaseId),
    /// A clip without an album cover.
    Track(TrackId),
}

impl Target {
    fn key(self) -> String {
        match self {
            // `v2`: an album found by its title alone took the art of
            // another artist's («There is a reason (RUS COVER)» got «The
            // Rush – Is the Reason»): asked again with the artist.
            Self::Release(id) => format!("cover_better.v2.{id}"),
            Self::Track(id) => format!("cover_better.track.{id}"),
        }
    }
}

/// The url found for `target`, if the cover it replaced is still `current`
/// (the cover ref of the release; `None` for a clip's).
pub(crate) fn cached(lib: &Library, target: Target, current: Option<&str>) -> Option<String> {
    let (_, value) = lib.cache_get(&target.key()).ok().flatten()?;
    let (replaced, url) = value.split_once(SEP)?;
    (!url.is_empty() && replaced == current.unwrap_or("")).then(|| url.to_owned())
}

/// Whether `target` was not looked up for its `current` cover yet, or found
/// nothing long enough ago.
fn due(lib: &Library, target: Target, current: Option<&str>, now: i64) -> bool {
    match lib.cache_get(&target.key()) {
        Ok(Some((at, value))) => match value.split_once(SEP) {
            Some((replaced, url)) if replaced == current.unwrap_or("") => url.is_empty() && now - at > RETRY_MS,
            _ => true,
        },
        Ok(None) => true,
        Err(_) => false,
    }
}

/// Whether an album cover is poor: none, a video's thumbnail, a file that
/// is missing, a frame of a video or smaller than [`MIN_SIDE`]. `size` is
/// the file's, when it is one and could be read.
pub(crate) fn poor(cover_ref: Option<&str>, size: Option<(u32, u32)>) -> bool {
    match cover_ref {
        None => true,
        Some(r) if is_web_ref(r) => r.contains("i.ytimg.com/"),
        Some(_) => match size {
            None => true,
            Some((w, h)) => cover_frame::is_frame(w, h) || w.max(h) < MIN_SIDE,
        },
    }
}

/// What a lookup knows of the song or the album.
#[derive(Clone, Debug, Default)]
pub(crate) struct Want {
    pub target: Option<Target>,
    /// The cover it replaces (see [`cached`]).
    pub current: Option<String>,
    /// The album's own lookup ([`hd_covers::find`]), for a release.
    pub album: Option<hd_covers::Want>,
    /// The artist names a hit may have: the credit, the channel, names the
    /// clip's title credits.
    pub artists: Vec<String>,
    /// The song's titles (a clip's: what its title says without what clips
    /// add, each half of a dual title).
    pub titles: Vec<String>,
    /// The clip's or the file's own title: an artist named in it counts.
    pub context: String,
    /// A song's match counts: a clip, a single. An album takes only the
    /// art of an album of its title.
    pub song_ok: bool,
}

fn folded(s: &str) -> String {
    artist_key(s).replace(' ', "")
}

/// Whether a hit of `artist`, `title` on `album` is the song (or the album)
/// `w` asks for.
pub(crate) fn accepts(w: &Want, artist: &str, title: &str, album: Option<&str>) -> bool {
    if !(names_artist(&w.artists, artist) || named_in(&w.context, artist)) {
        return false;
    }
    let t = title_key(title).base;
    let song = !t.is_empty()
        && w.titles.iter().any(|x| {
            let k = title_key(x).base;
            k == t || similarity(&k, &t) >= 0.92
        });
    let same_album = match (&w.album, album) {
        (Some(want), Some(theirs)) => album_titles(&want.title).iter().any(|t| hd_covers::same_album(t, theirs)),
        _ => false,
    };
    same_album || (song && (w.song_ok || w.album.is_none()))
}

/// Whether one of `names` is the hit's `artist` (spelled alike, in another
/// script, or a little apart).
pub(crate) fn names_artist(names: &[String], artist: &str) -> bool {
    let a = artist_key(artist);
    !a.is_empty()
        && names.iter().any(|n| {
            let k = artist_key(n);
            !k.is_empty() && (similarity(&k, &a) >= 0.85 || nikgob_core::translit::same_translit(&k, &a) || names::same_name(n, artist))
        })
}

/// An album's titles: the whole and each part of a title in two scripts
/// («第二回演奏会 メイドインアビス / THE SECOND CONCERT MADE IN ABYSS»).
fn album_titles(title: &str) -> Vec<String> {
    let mut out = vec![title.to_owned()];
    for p in title_parts(title) {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// The parts of a title split by « - », « / » and alike, without empty
/// ones; a title of one part gives none.
fn title_parts(title: &str) -> Vec<String> {
    let mut parts = vec![title.to_owned()];
    for sep in [" - ", " – ", " — ", " / ", "／", " | "] {
        parts = parts.into_iter().flat_map(|p| p.split(sep).map(|x| x.trim().to_owned()).collect::<Vec<_>>()).collect();
    }
    parts.retain(|p| p.chars().any(char::is_alphanumeric));
    if parts.len() < 2 { Vec::new() } else { parts }
}

/// The runs of kana in `s` («ヨルシカ» of «TVアニメ…｜ヨルシカ「火星人」»).
fn kana_runs(s: &str) -> Vec<String> {
    let kana = |c: char| matches!(c as u32, 0x3041..=0x3096 | 0x30A1..=0x30FC);
    let mut out = Vec::new();
    let mut run = String::new();
    for c in s.chars() {
        if kana(c) {
            run.push(c);
        } else if !run.is_empty() {
            out.push(std::mem::take(&mut run));
        }
    }
    if !run.is_empty() {
        out.push(run);
    }
    out
}

/// Whether `context` (a clip's title) names the artist `name`: as written
/// (a whole word, for a name in Latin letters), or in kana that read as it
/// («ヨルシカ» names «Yorushika»).
fn named_in(context: &str, name: &str) -> bool {
    let key = artist_key(name);
    if key.chars().count() < 2 || context.is_empty() {
        return false;
    }
    if !names::is_latin(name) {
        return folded(context).contains(&folded(name));
    }
    (key.chars().count() >= 3 && format!(" {} ", artist_key(context)).contains(&format!(" {key} ")))
        || kana_runs(context).iter().any(|k| names::same_name(k, name))
}

/// A metadata service that could not answer (as opposed to having
/// nothing): no network, a refusal, a rate limit that did not pass.
pub(crate) fn meta_trouble(e: &MetadataError) -> bool {
    match e {
        MetadataError::Network(_) | MetadataError::RateLimited(_) | MetadataError::Service(_) => true,
        MetadataError::Status { status, .. } => matches!(status, 401 | 403) || *status >= 500,
        _ => false,
    }
}

/// [`meta_trouble`] of a streaming service.
pub(crate) fn stream_trouble(e: &StreamError) -> bool {
    match e {
        StreamError::Network(_) | StreamError::RateLimited { .. } => true,
        StreamError::Http { status } => matches!(status, 401 | 403) || *status >= 500,
        _ => false,
    }
}

/// The art for `w`, if a source has it. Err when nothing was found and a
/// service could not answer: that is no miss, it is asked again.
pub(crate) async fn find(meta: Option<&Metadata>, services: &[Arc<dyn StreamService>], w: &Want) -> Result<Option<String>, String> {
    let mut trouble: Option<String> = None;
    if let Some(album) = &w.album {
        // By every spelling of the artist the lookup knows.
        let album = hd_covers::Want { artists: w.artists.clone(), ..album.clone() };
        match hd_covers::find(meta, services, &album).await {
            Ok(Some(url)) => return Ok(Some(url)),
            Ok(None) => {}
            Err(e) => trouble = Some(e),
        }
    }
    let queries = queries(w);
    if let Some(meta) = meta {
        for q in &queries {
            match meta.deezer.search_raw(q, 10).await {
                Ok(hits) => {
                    let found = hits.iter().find(|h| {
                        accepts(w, h.artist_name(), &h.title, h.album.as_ref().map(|a| a.title.as_str()))
                    });
                    if let Some(url) = found.and_then(|h| h.album.as_ref()).and_then(|a| a.best_cover()) {
                        return Ok(Some(url.to_owned()));
                    }
                }
                Err(e) if meta_trouble(&e) => {
                    trouble.get_or_insert(format!("Deezer: {e}"));
                    break;
                }
                Err(e) => log::debug!("art of «{q}» on Deezer: {e}"),
            }
        }
        for q in &queries {
            match meta.itunes.search_songs(q, 10).await {
                Ok(hits) => {
                    if let Some(url) = hits
                        .iter()
                        .find(|h| accepts(w, &h.artist_name, &h.track_name, h.collection_name.as_deref()))
                        .and_then(|h| h.artwork())
                    {
                        return Ok(Some(url));
                    }
                }
                Err(e) if meta_trouble(&e) => {
                    trouble.get_or_insert(format!("iTunes: {e}"));
                    break;
                }
                Err(e) => log::debug!("art of «{q}» on iTunes: {e}"),
            }
        }
    }
    // YouTube Music's songs carry their album's square art.
    for s in services {
        for q in queries.iter().take(2) {
            let found = match s.search(q).await {
                Ok(f) => f,
                Err(e) if stream_trouble(&e) => {
                    trouble.get_or_insert(format!("{}: {e}", s.service()));
                    break;
                }
                Err(_) => continue,
            };
            let hit = found.tracks.iter().find(|t| {
                t.kind == TrackKind::Song
                    && accepts(w, t.artists.first().map_or("", |a| a.name.as_str()), &t.title, t.album.as_ref().map(|a| a.title.as_str()))
            });
            if let Some(url) = hit.and_then(|t| t.cover_url.clone()).filter(|u| !u.contains("i.ytimg.com/")) {
                return Ok(Some(url));
            }
        }
    }
    match trouble {
        Some(e) => Err(e),
        None => Ok(None),
    }
}

/// What to search for `w`, at most [`MAX_QUERIES`]: its artists with its
/// titles, a Latin spelling with a Latin title first (what the services
/// know best), then the titles alone.
fn queries(w: &Want) -> Vec<String> {
    let latin = |s: &str| names::is_latin(s);
    let mut artists: Vec<&str> = w.artists.first().map(String::as_str).into_iter().collect();
    if let Some(a) = w.artists.iter().find(|a| latin(a) && !artists.contains(&a.as_str())) {
        artists.push(a);
    }
    let mut titles: Vec<&str> = w.titles.iter().take(3).map(String::as_str).collect();
    for t in w.titles.iter().filter(|t| latin(t)).take(2) {
        if !titles.contains(&t.as_str()) {
            titles.push(t);
        }
    }
    let mut pairs: Vec<(bool, String)> = Vec::new();
    for t in &titles {
        for a in &artists {
            pairs.push((latin(a) && latin(t), format!("{a} {t}").trim().to_owned()));
        }
    }
    // Stable: each kind keeps its order.
    pairs.sort_by_key(|(both, _)| !*both);
    let mut out: Vec<String> = Vec::new();
    for q in pairs.into_iter().map(|(_, q)| q).chain(titles.iter().take(2).map(|t| (*t).to_owned())) {
        if !q.trim().is_empty() && !out.contains(&q) && out.len() < MAX_QUERIES {
            out.push(q);
        }
    }
    out
}

/// Every artist's spellings the library knows: its name and its aliases.
pub(crate) struct Spellings(Vec<Vec<String>>);

impl Spellings {
    pub(crate) fn of(lib: &Library) -> Self {
        let mut by: HashMap<ArtistId, Vec<String>> = HashMap::new();
        for (id, name) in lib.artist_names().unwrap_or_default() {
            by.entry(id).or_default().push(name);
        }
        for (id, name) in lib.artist_aliases().unwrap_or_default() {
            by.entry(id).or_default().push(name);
        }
        Self(by.into_values().collect())
    }
}

fn push_new(list: &mut Vec<String>, s: &str) {
    let s = s.trim();
    if !s.is_empty() && !list.iter().any(|x| x == s) {
        list.push(s.to_owned());
    }
}

/// Widens `w` with what the library knows (see the module docs): every
/// spelling of the artists it names or its title names, the other titles
/// of its own tracks (`own`), and the titles of the library's songs by
/// those artists that carry one of its titles beside another. A kana name
/// with no Latin spelling known is read in romaji.
pub(crate) fn widen(lib: &Library, spellings: &Spellings, w: &mut Want, own: &[TrackId]) {
    let mut artists = w.artists.clone();
    for group in &spellings.0 {
        let hit = group.iter().any(|g| w.artists.iter().any(|a| names::same_name(a, g)) || named_in(&w.context, g));
        if hit {
            for g in group {
                push_new(&mut artists, g);
            }
        }
    }
    let mut titles = w.titles.clone();
    for id in own {
        for t in lib.track_aliases(*id).unwrap_or_default() {
            push_new(&mut titles, &t);
        }
    }
    let mut found_titles = Vec::new();
    let mut found_artists = Vec::new();
    for t in w.titles.iter().take(3) {
        let key = title_key(t).base;
        if key.is_empty() {
            continue;
        }
        for row in lib.search(t, 20).map(|r| r.tracks).unwrap_or_default() {
            let by_them = artists.iter().any(|a| names::same_name(a, &row.artist) || artist_key(a) == artist_key(&row.artist));
            if own.contains(&row.id) || !by_them {
                continue;
            }
            let mut parts = title_parts(&row.title);
            parts.extend(lib.track_aliases(row.id).unwrap_or_default());
            if parts.iter().any(|p| title_key(p).base == key) {
                found_titles.extend(parts);
                found_artists.push(row.artist.clone());
            }
        }
    }
    for t in &found_titles {
        push_new(&mut titles, t);
    }
    for a in &found_artists {
        push_new(&mut artists, a);
    }
    for a in artists.clone() {
        let known = artists.iter().any(|x| names::is_latin(x) && names::same_name(x, &a));
        if let Some(r) = names::kana_romaji(&a).filter(|_| !known) {
            let mut c = r.chars();
            let r: String = c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default();
            push_new(&mut artists, &r);
        }
    }
    w.artists = artists;
    w.titles = titles;
}

/// A clip title's song titles, the most specific first: a quoted title
/// («"ReawakeR" by LiSA»), the title before « by », then the halves of the
/// title from the last (the quoted song of «ヨルシカ「火星人」») to the whole.
pub(crate) fn clip_titles(title: &str, channel: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |t: &str| {
        let t = t.trim().trim_matches(|c: char| "\"'«»“”".contains(c)).trim();
        if !t.is_empty() && !out.iter().any(|o| o == t) {
            out.push(t.to_owned());
        }
    };
    for (open, close) in [('"', '"'), ('«', '»'), ('“', '”')] {
        if let Some(a) = title.find(open)
            && let Some(b) = title[a + open.len_utf8()..].find(close)
        {
            push(&title[a + open.len_utf8()..a + open.len_utf8() + b]);
        }
    }
    let halves = nikgob_core::clip_title::song_titles(title, channel);
    for h in halves.iter().rev() {
        let h = h.split(" by ").next().unwrap_or(h);
        push(h);
    }
    out
}

/// What a clip's (or a file saved from a clip's) row says of its song.
pub(crate) fn clip_want(row: &TrackRow) -> Want {
    let channel = row.artist.as_str();
    // A name the title credits («by LiSA») searches better than a channel.
    let mut artists = nikgob_core::clip_title::credited_names(&row.title, channel);
    if !artists.iter().any(|a| a == channel) {
        artists.push(channel.to_owned());
    }
    Want {
        titles: clip_titles(&row.title, channel),
        artists,
        context: format!("{} {}", row.title, row.album.as_deref().unwrap_or("")),
        song_ok: true,
        ..Want::default()
    }
}

/// The size of the picture at `path` (its header only).
fn size(path: &Path) -> Option<(u32, u32)> {
    image::image_dimensions(path).ok()
}

/// The albums and clips looked at in this run (asked, or their cover is
/// fine), shared with the sweep, and whether a sweep runs.
#[derive(Clone, Default)]
pub(crate) struct Asked {
    inner: Arc<Mutex<(HashSet<Target>, bool)>>,
}

impl Asked {
    fn lock(&self) -> MutexGuard<'_, (HashSet<Target>, bool)> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Marks `t` looked at; whether it was not yet.
    fn first(&self, t: Target) -> bool {
        self.lock().0.insert(t)
    }
}

/// The lookup of release `id` (of `row`, one of its tracks; cover files
/// under `root`) when its cover is poor and it is due.
fn release_want(lib: &Library, root: &Path, id: ReleaseId, row: &TrackRow, now: i64, spellings: &mut Option<Spellings>) -> Option<Want> {
    let target = Target::Release(id);
    let r = lib.get_release(id).ok().flatten()?;
    let current = r.cover_ref.clone();
    if !due(lib, target, current.as_deref(), now) {
        return None;
    }
    let dims = current.as_deref().filter(|c| !is_web_ref(c)).and_then(|c| {
        let p = Path::new(c);
        size(&if p.is_absolute() { p.to_owned() } else { root.join(p) })
    });
    if !poor(current.as_deref(), dims) {
        return None;
    }
    let frame = dims.is_some_and(|(w, h)| cover_frame::is_frame(w, h));
    let tracks = lib.tracks_of_release(id).unwrap_or_default();
    let deezer = lib
        .external_ids(EntityKind::Album, *id.as_uuid())
        .unwrap_or_default()
        .into_iter()
        .find(|x| x.service == Service::Deezer)
        .and_then(|x| x.id.parse().ok());
    // A file saved from a clip, or a clip: its song as the clip's title
    // says; else the album's artist, its songs and the album itself.
    let clip = frame || row.is_clip();
    let mut want = if clip {
        clip_want(tracks.first().unwrap_or(row))
    } else {
        Want {
            artists: vec![r.artist_credit.display()],
            titles: tracks.iter().take(2).map(|t| t.title.clone()).collect(),
            context: r.title.clone(),
            // A single's song is the single.
            song_ok: tracks.len() <= 1,
            album: Some(hd_covers::Want {
                id,
                mbid: r.mbid.clone(),
                group_mbid: r.release_group_mbid.clone(),
                deezer,
                title: r.title.clone(),
                artists: Vec::new(),
            }),
            ..Want::default()
        }
    };
    want.target = Some(target);
    want.current = current;
    let mut own = vec![row.id];
    own.extend(tracks.iter().take(2).map(|t| t.id).filter(|t| *t != row.id));
    widen(lib, spellings.get_or_insert_with(|| Spellings::of(lib)), &mut want, &own);
    Some(want)
}

/// The lookups `rows` (tracks of the library, or the one that started)
/// need, skipping what `asked` looked at already.
fn wants(lib: &Library, root: &Path, rows: &[TrackRow], asked: &Asked) -> Vec<Want> {
    let now = now_ms();
    let mut spellings = None;
    let mut out = Vec::new();
    for row in rows {
        match row.release_id {
            Some(id) => {
                if asked.first(Target::Release(id))
                    && let Some(w) = release_want(lib, root, id, row, now, &mut spellings)
                {
                    out.push(w);
                }
            }
            None if row.is_clip() => {
                let target = Target::Track(row.id);
                if asked.first(target) && due(lib, target, None, now) {
                    let mut w = Want { target: Some(target), ..clip_want(row) };
                    widen(lib, spellings.get_or_insert_with(|| Spellings::of(lib)), &mut w, &[row.id]);
                    out.push(w);
                }
            }
            None => {}
        }
    }
    out
}

/// How a run of lookups ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Swept {
    pub asked: usize,
    pub found: usize,
    /// Why it stopped before the end: a service could not answer.
    pub stopped: Option<String>,
}

/// Looks `wants` up one after another with `look` (each within
/// [`LOOKUP_TIMEOUT`]) and keeps what each found, a miss too. A lookup a
/// service could not answer stops the run: it and the rest are not kept,
/// so the next run asks them. `shown` is told of each art found. `quiet`:
/// an unattended run, which waits while a game is in front; the playing
/// song's lookup never does.
pub(crate) async fn sweep<F, Fut>(
    lib: &Library,
    wants: Vec<Want>,
    quiet: bool,
    look: F,
    mut shown: impl FnMut(),
) -> Swept
where
    F: Fn(Want) -> Fut,
    Fut: Future<Output = Result<Option<String>, String>>,
{
    let mut done = Swept { asked: 0, found: 0, stopped: None };
    for w in wants {
        let Some(target) = w.target else { continue };
        let (current, title) = (w.current.clone(), w.titles.first().cloned());
        // Waits while a game is in front (outside the lookup's timeout).
        if quiet {
            crate::quiet::free().await;
        }
        let url = match tokio::time::timeout(LOOKUP_TIMEOUT, look(w)).await {
            Ok(Ok(url)) => url.unwrap_or_default(),
            Ok(Err(e)) => {
                done.stopped = Some(e);
                break;
            }
            Err(_) => {
                done.stopped = Some("a lookup took too long".into());
                break;
            }
        };
        done.asked += 1;
        let _ = lib.cache_put(&target.key(), now_ms(), &format!("{}{SEP}{url}", current.as_deref().unwrap_or("")));
        if url.is_empty() {
            log::debug!("no art for {target:?} ({title:?})");
        } else {
            log::info!("art for {target:?} ({title:?}): {url}");
            done.found += 1;
            shown();
        }
    }
    done
}

/// A sweep of this many lookups under way: dropped before it ends (the app
/// closes), it says so in the log.
struct Unfinished(usize);

impl Drop for Unfinished {
    fn drop(&mut self) {
        log::info!("covers: the sweep of {} lookups ended with the app; the rest is asked at the next start", self.0);
    }
}

impl Worker {
    /// Looks up real art in the background for the library's albums and
    /// clips whose picture is poor (a sweep, one at a time), or for
    /// `current` (the track that started); what was looked at in this run
    /// is not looked at again.
    pub(crate) fn kick_better_covers(&mut self, current: Option<TrackId>) {
        let Some(meta) = self.import.as_ref().filter(|s| s.config().fetch_cover_online).and_then(|s| s.metadata()) else {
            return;
        };
        let asked = self.better_asked.clone();
        let rows: Vec<TrackRow> = match current {
            Some(t) => self.lib.get_track_row(t).ok().flatten().into_iter().collect(),
            None => {
                let mut s = asked.lock();
                if s.1 {
                    return;
                }
                s.1 = true;
                Vec::new()
            }
        };
        let sweep_run = current.is_none();
        let services: Vec<Arc<dyn StreamService>> =
            [Service::Deezer, Service::Youtube].into_iter().filter_map(|k| self.service(k)).collect();
        let lib = self.lib.clone();
        let root = self.res.root().to_owned();
        self.streaming.spawn_reporting(move |tx| async move {
            if sweep_run {
                tokio::time::sleep(START).await;
                crate::quiet::free().await;
            }
            let wants = {
                let (lib, root, asked) = (lib.clone(), root.clone(), asked.clone());
                tokio::task::spawn_blocking(move || {
                    let rows = if sweep_run {
                        lib.tracks(0, 50_000, nikgob_library::Sort::asc(nikgob_library::TrackSortKey::Album)).unwrap_or_default()
                    } else {
                        rows
                    };
                    wants(&lib, &root, &rows, &asked)
                })
                .await
                .unwrap_or_default()
            };
            let n = wants.len();
            // A sweep cut short by the app closing says so too.
            let mut cut = (sweep_run && n > 0).then(|| Unfinished(n));
            // Covers on screen take the art as it comes, not at every hit.
            let mut last: Option<Instant> = None;
            let mut pending = false;
            let look = |w: Want| {
                let (meta, services) = (meta.clone(), services.clone());
                async move { find(Some(&meta), &services, &w).await }
            };
            let done = sweep(&lib, wants, sweep_run, look, || {
                pending = true;
                if last.is_none_or(|t| t.elapsed() >= SHOW_EVERY) {
                    last = Some(Instant::now());
                    pending = false;
                    let _ = tx.send(Internal::Enrich(Event::BetterCovers));
                }
            })
            .await;
            if let Some(c) = cut.take() {
                std::mem::forget(c);
            }
            match &done.stopped {
                Some(why) => log::info!("covers: stopped after {} of {n} lookups ({why}); the rest is asked at the next start", done.asked),
                None if sweep_run && n > 0 => log::info!("covers: {} lookups, {} found", done.asked, done.found),
                None => {}
            }
            if sweep_run {
                asked.lock().1 = false;
            }
            if pending {
                let _ = tx.send(Internal::Enrich(Event::BetterCovers));
            }
        });
    }

    /// Art was found: covers on screen take it.
    pub(crate) fn better_covers_found(&mut self) {
        self.reload_after_import();
        self.publish_now_playing();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn want(artists: &[&str], titles: &[&str], context: &str) -> Want {
        Want {
            artists: artists.iter().map(|s| (*s).to_owned()).collect(),
            titles: titles.iter().map(|s| (*s).to_owned()).collect(),
            context: context.to_owned(),
            ..Want::default()
        }
    }

    #[test]
    fn poor_covers_are_frames_tiny_missing_or_thumbnails() {
        assert!(poor(None, None));
        assert!(poor(Some("https://i.ytimg.com/vi/x/hqdefault.jpg"), None));
        assert!(!poor(Some("https://cdn-images.dzcdn.net/images/cover/x/1000x1000.jpg"), None));
        assert!(poor(Some("A/Singles/X/cover.jpg"), Some((853, 480))), "a clip's frame");
        assert!(poor(Some("A/2021 - X/cover.jpg"), Some((300, 297))), "a tiny cover");
        assert!(poor(Some("A/2021 - X/cover.jpg"), None), "a missing file");
        assert!(!poor(Some("A/2021 - X/cover.jpg"), Some((1200, 1200))));
        assert!(!poor(Some("A/2014 - X/cover.jpg"), Some((604, 800))), "a booklet's scan");
    }

    #[test]
    fn a_hit_is_the_song_by_its_title_and_artist() {
        let w = want(&["Кино"], &["Группа крови"], "Группа крови");
        assert!(accepts(&w, "KINO", "Группа крови", None), "the artist in Latin letters");
        assert!(accepts(&w, "Кино", "Группа крови (Remastered)", None));
        assert!(accepts(&w, "Кино", "Группа крови", Some("Группа крови")));
        assert!(!accepts(&w, "Кино", "Звезда по имени Солнце", None), "another song");
        assert!(!accepts(&w, "Ария", "Группа крови", None), "a cover by another band");
        // Either script.
        let w = want(&["Borislav Slavov"], &["Down by the River"], "");
        assert!(accepts(&w, "Борислав Славов", "Down by the River", None));
    }

    #[test]
    fn a_clip_of_a_channel_takes_the_song_of_the_artist_its_title_names() {
        let row = TrackRow {
            id: TrackId::new(),
            title: "TVアニメ『小市民シリーズ』第2期オープニング映像｜ヨルシカ「火星人」".into(),
            artist: "tv asahi animation YouTubeチャンネル".into(),
            album: None,
            release_id: None,
            cover_ref: None,
            disc_no: 1,
            track_no: 1,
            duration_ms: None,
            quality: None,
            codec: None,
            active_kind: None,
            has_local: false,
            liked: false,
            credit: Default::default(),
            youtube_id: None,
            check: None,
        };
        let w = clip_want(&row);
        assert_eq!(w.titles.first().map(String::as_str), Some("火星人"), "{:?}", w.titles);
        assert!(accepts(&w, "ヨルシカ", "火星人", Some("火星人")));
        assert!(!accepts(&w, "Someone Else", "火星人", None));
        assert!(!accepts(&w, "ヨルシカ", "ただ君に晴れ", None));
    }

    #[test]
    fn a_clip_crediting_its_artist_searches_by_the_artist_and_the_quoted_title() {
        let titles = clip_titles("Solo Leveling Season 2 -Arise from the Shadow- Opening | \"ReawakeR\" by LiSA feat Felix of Stray Kids", "Crunchyroll");
        assert_eq!(titles.first().map(String::as_str), Some("ReawakeR"));
        let w = Want {
            artists: vec!["LiSA".into(), "Crunchyroll".into()],
            titles,
            context: "Solo Leveling Season 2 -Arise from the Shadow- Opening | \"ReawakeR\" by LiSA feat Felix of Stray Kids".into(),
            ..Want::default()
        };
        assert_eq!(queries(&w).first().map(String::as_str), Some("LiSA ReawakeR"));
        assert!(queries(&w).len() <= MAX_QUERIES);
        assert!(accepts(&w, "LiSA feat. Felix of Stray Kids", "ReawakeR (feat. Felix of Stray Kids)", Some("ReawakeR - Single")));
        assert!(!accepts(&w, "Dima Lancaster", "ReawakeR (from \"Solo Leveling\")", None), "a cover of the song");
    }

    #[test]
    fn an_album_found_by_its_title_is_by_its_artist() {
        assert!(!names_artist(&["j.am".into()], "The Rush"), "a cover's single is not the original's");
        assert!(names_artist(&["Kevin Penkin".into()], "Kevin Penkin"));
        assert!(names_artist(&["ヨルシカ".into(), "Yorushika".into()], "Yorushika"));
        assert!(!names_artist(&[], "Kevin Penkin"));
    }

    #[test]
    fn an_album_matches_by_its_title() {
        let mut w = want(&["藤澤慶昌"], &["Opening"], "");
        w.album = Some(hd_covers::Want {
            id: ReleaseId::new(),
            mbid: None,
            group_mbid: None,
            deezer: None,
            title: "Mushoku Tensei ~Isekai Ittara Honki Dasu~ Original Soundtrack 2 Special Edition".into(),
            artists: Vec::new(),
        });
        assert!(
            !accepts(&w, "Yoshiaki Fujisawa", "Something", Some("Mushoku Tensei: Jobless Reincarnation (Original Soundtrack 2)")),
            "an artist the want does not name"
        );
        assert!(accepts(&w, "藤澤慶昌", "Something", Some("Mushoku Tensei: Jobless Reincarnation (Original Soundtrack 2)")));
        assert!(!accepts(&w, "藤澤慶昌", "Something", Some("Mushoku Tensei: Jobless Reincarnation (Original Soundtrack 1)")));
    }

    /// A sweep asks for the album saved with a clip's frame and the tiny
    /// cover, not for the good one, each once in a run; the art found then
    /// stands for the frame wherever the album shows.
    #[test]
    fn a_sweep_asks_for_poor_covers_once_and_their_art_shows() {
        use nikgob_core::model::{ArtistCredit, Release, Track};
        let lib = Library::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut rows = Vec::new();
        let mut ids = Vec::new();
        for (album, w, h) in [("Frame", 853, 480), ("Tiny", 300, 297), ("Good", 1000, 1000)] {
            std::fs::create_dir_all(dir.path().join(album)).unwrap();
            image::RgbImage::from_pixel(w, h, image::Rgb([90, 60, 30])).save(dir.path().join(album).join("cover.png")).unwrap();
            let mut r = Release::new(album, ArtistCredit::single("X", None));
            r.cover_ref = Some(format!("{album}/cover.png"));
            let r = lib.upsert_release(&r).unwrap();
            let mut t = Track::new(format!("{album} song"), ArtistCredit::single("X", None));
            t.release_id = Some(r.id);
            let t = lib.upsert_track(&t).unwrap();
            rows.push(lib.get_track_row(t.id).unwrap().unwrap());
            ids.push(r.id);
        }
        let asked = Asked::default();
        let got = wants(&lib, dir.path(), &rows, &asked);
        let targets: Vec<_> = got.iter().filter_map(|w| w.target).collect();
        assert_eq!(targets, vec![Target::Release(ids[0]), Target::Release(ids[1])]);
        assert!(got[0].album.is_none(), "a frame is looked up as a clip's song");
        assert_eq!(got[1].album.as_ref().map(|a| a.title.as_str()), Some("Tiny"));
        assert!(wants(&lib, dir.path(), &rows, &asked).is_empty(), "once in a run");

        lib.cache_put(&Target::Release(ids[0]).key(), now_ms(), &format!("Frame/cover.png{SEP}https://x/art.jpg")).unwrap();
        let mut res = crate::convert::Resolver::new(lib.clone(), dir.path().to_owned());
        let r = lib.get_release(ids[0]).unwrap().unwrap();
        assert_eq!(res.release_card(&r, 1, Some(1), false).cover.and_then(|c| c.web_url().map(str::to_owned)).as_deref(), Some("https://x/art.jpg"));
        let good = lib.get_release(ids[2]).unwrap().unwrap();
        assert!(res.release_card(&good, 1, Some(1), false).cover.unwrap().file().is_some());
    }

    #[test]
    fn the_cache_keeps_the_cover_it_replaced() {
        let lib = Library::open_in_memory().unwrap();
        let id = ReleaseId::new();
        let t = Target::Release(id);
        assert!(due(&lib, t, Some("a/cover.jpg"), now_ms()));
        lib.cache_put(&t.key(), now_ms(), &format!("a/cover.jpg{SEP}https://x/art.jpg")).unwrap();
        assert_eq!(cached(&lib, t, Some("a/cover.jpg")).as_deref(), Some("https://x/art.jpg"));
        assert!(!due(&lib, t, Some("a/cover.jpg"), now_ms()));
        // A new cover imported since: its own, and asked again.
        assert_eq!(cached(&lib, t, Some("a/new-cover.jpg")), None);
        assert!(due(&lib, t, Some("a/new-cover.jpg"), now_ms()));
        // A miss is asked again after a while.
        let c = Target::Track(TrackId::new());
        lib.cache_put(&c.key(), now_ms() - RETRY_MS - 1, &format!("{SEP}")).unwrap();
        assert_eq!(cached(&lib, c, None), None);
        assert!(due(&lib, c, None, now_ms()));
    }

    fn album_want(artist: &str, tracks: &[&str], title: &str, song_ok: bool) -> Want {
        let mut w = want(&[artist], tracks, title);
        w.song_ok = song_ok;
        w.album = Some(hd_covers::Want {
            id: ReleaseId::new(),
            mbid: None,
            group_mbid: None,
            deezer: None,
            title: title.into(),
            artists: vec![artist.into()],
        });
        w
    }

    /// The Theme Song Collection's song found on its single gave the album
    /// the single's art: an album takes only an album of its title.
    #[test]
    fn an_album_never_takes_the_art_of_another_release_with_its_song() {
        let w = album_want("大原ゆい子", &["旅人の唄"], "TV Anime \"Mushoku Tensei ~Isekai Ittara Honki Dasu~\" Theme Song Collection", false);
        assert!(!accepts(&w, "大原ゆい子", "旅人の唄", Some("旅人の唄")), "the single");
        assert!(!accepts(&w, "大原ゆい子", "旅人の唄", None), "a song without its album");
        assert!(accepts(&w, "大原ゆい子", "旅人の唄", Some("Mushoku Tensei: Jobless Reincarnation Theme Song Collection")));
        // A single's song is the single; a clip's song is the clip's.
        let single = album_want("大原ゆい子", &["旅人の唄"], "旅人の唄", true);
        assert!(accepts(&single, "大原ゆい子", "旅人の唄", Some("旅人の唄 - Single")));
        let mut clip = want(&["LiSA"], &["ReawakeR"], "");
        clip.song_ok = true;
        assert!(accepts(&clip, "LiSA", "ReawakeR", Some("ReawakeR - Single")));
        // A title in two scripts matches by either part.
        let concert = album_want("Ensemble G.A.P. TOKYO", &["x"], "第二回演奏会 メイドインアビス / THE SECOND CONCERT MADE IN ABYSS", false);
        assert!(accepts(&concert, "Ensemble G.A.P. TOKYO", "y", Some("The Second Concert: Made in Abyss")));
    }

    fn row_of(lib: &Library, title: &str, artist: &str) -> TrackRow {
        use nikgob_core::model::{Artist, ArtistCredit, Track};
        let known = lib.artist_names().unwrap().into_iter().find(|a| a.1 == artist).map(|a| a.0);
        let id = known.unwrap_or_else(|| lib.upsert_artist(&Artist::new(artist)).unwrap().id);
        let t = lib.upsert_track(&Track::new(title, ArtistCredit::single(artist, Some(id)))).unwrap();
        lib.get_track_row(t.id).unwrap().unwrap()
    }

    /// 火星人 is known to the services only as Yorushika's «Martian»: the
    /// library's own «火星人 - Martian» by Yorushika, whose name the clip's
    /// title writes in kana, gives the Latin title and spelling to search.
    #[test]
    fn a_native_title_searches_with_the_spellings_the_library_knows() {
        let lib = Library::open_in_memory().unwrap();
        row_of(&lib, "火星人 - Martian", "Yorushika");
        row_of(&lib, "晴る - Sunny", "Yorushika");
        row_of(&lib, "火星人", "Someone Else");
        let clip = row_of(&lib, "TVアニメ『小市民シリーズ』第2期オープニング映像｜ヨルシカ「火星人」", "tv asahi animation YouTubeチャンネル");
        let mut w = clip_want(&clip);
        widen(&lib, &Spellings::of(&lib), &mut w, &[clip.id]);
        assert!(w.titles.iter().any(|t| t == "Martian"), "{:?}", w.titles);
        assert!(!w.titles.iter().any(|t| t == "Sunny"), "another song: {:?}", w.titles);
        assert!(w.artists.iter().any(|a| a == "Yorushika"), "{:?}", w.artists);
        assert!(!w.artists.iter().any(|a| a == "Someone Else"), "{:?}", w.artists);
        assert_eq!(queries(&w).first().map(String::as_str), Some("Yorushika Martian"), "{:?}", queries(&w));
        assert!(queries(&w).len() <= MAX_QUERIES);
        assert!(accepts(&w, "Yorushika", "Martian", Some("Martian")));
        assert!(!accepts(&w, "Karaoke Manekineko", "Martian (Karaoke) [Yorushika]", None), "a karaoke version");
    }

    /// An alias of the artist (a merged double) and a track's other title
    /// are searched too; a kana name nobody spelled in Latin is read in
    /// romaji.
    #[test]
    fn aliases_and_other_titles_widen_a_lookup() {
        let lib = Library::open_in_memory().unwrap();
        let native = row_of(&lib, "旅人の唄", "大原ゆい子");
        row_of(&lib, "Tabibito no Uta", "Ohara Yuiko");
        let id = |name: &str| lib.artist_names().unwrap().into_iter().find(|a| a.1 == name).unwrap().0;
        lib.merge_artists(id("Ohara Yuiko"), id("大原ゆい子")).unwrap();
        lib.add_track_alias(native.id, "Tabibito no Uta", "merged").unwrap();
        let mut w = want(&["大原ゆい子"], &["旅人の唄"], "");
        widen(&lib, &Spellings::of(&lib), &mut w, &[native.id]);
        assert!(w.artists.iter().any(|a| a == "Ohara Yuiko"), "{:?}", w.artists);
        assert!(w.titles.iter().any(|t| t == "Tabibito no Uta"), "{:?}", w.titles);
        assert_eq!(queries(&w).first().map(String::as_str), Some("Ohara Yuiko Tabibito no Uta"));
        assert!(accepts(&w, "Yuiko Ohara", "Tabibito no Uta", None), "the name in the other order");
        let mut kana = want(&["ナナホシ"], &["x"], "");
        widen(&lib, &Spellings::of(&lib), &mut kana, &[]);
        assert!(kana.artists.iter().any(|a| a == "Nanahoshi"), "{:?}", kana.artists);
    }

    fn targets(n: usize) -> Vec<Want> {
        (0..n).map(|_| Want { target: Some(Target::Track(TrackId::new())), song_ok: true, ..Want::default() }).collect()
    }

    /// Misses are normal: a sweep goes through all of them. A lookup a
    /// service could not answer stops it, and what it did not ask stays
    /// due for the next start.
    #[tokio::test]
    async fn a_sweep_goes_past_misses_and_stops_only_when_a_service_cannot_answer() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let lib = Library::open_in_memory().unwrap();
        let wants = targets(8);
        let ids: Vec<Target> = wants.iter().filter_map(|w| w.target).collect();
        let done = sweep(&lib, wants, false, |_| async { Ok(None) }, || {}).await;
        assert_eq!(done, Swept { asked: 8, found: 0, stopped: None });
        assert!(ids.iter().all(|t| !due(&lib, *t, None, now_ms())), "every miss kept for the re-ask");

        let wants = targets(7);
        let ids: Vec<Target> = wants.iter().filter_map(|w| w.target).collect();
        let n = AtomicUsize::new(0);
        let mut shown = 0;
        let look = |_| {
            let i = n.fetch_add(1, Ordering::SeqCst);
            async move {
                match i {
                    4 => Ok(Some("https://x/art.jpg".to_owned())),
                    5 => Err("Deezer: network error".to_owned()),
                    _ => Ok(None),
                }
            }
        };
        let done = sweep(&lib, wants, false, look, || shown += 1).await;
        assert_eq!(done, Swept { asked: 5, found: 1, stopped: Some("Deezer: network error".into()) });
        assert_eq!(shown, 1);
        assert_eq!(n.load(Ordering::SeqCst), 6, "nothing asked after the refusal");
        assert_eq!(cached(&lib, ids[4], None).as_deref(), Some("https://x/art.jpg"));
        assert!(ids[..5].iter().all(|t| !due(&lib, *t, None, now_ms())));
        assert!(ids[5..].iter().all(|t| due(&lib, *t, None, now_ms())), "asked again at the next start");
    }

    #[tokio::test]
    async fn a_service_that_cannot_answer_is_no_miss() {
        use nikgob_core::streaming::FakeService;
        let mut w = want(&["Yorushika"], &["Martian"], "");
        w.song_ok = true;
        let yt = Arc::new(FakeService::youtube().with_song("m", "Martian", "Yorushika", "Martian", 235));
        let services: Vec<Arc<dyn StreamService>> = vec![yt.clone()];
        let found = find(None, &services, &w).await.unwrap();
        assert!(found.is_some_and(|u| u.ends_with("/m.jpg")));
        let mut other = want(&["Yorushika"], &["Sunny"], "");
        other.song_ok = true;
        assert_eq!(find(None, &services, &other).await, Ok(None), "a miss");
        yt.rate_limit_next("search", 5, None);
        assert!(find(None, &services, &w).await.is_err(), "a refusal");
    }
}
