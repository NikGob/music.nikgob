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
//! by a TV channel).
//!
//! What is found is kept as a url in the library's cache
//! (`cover_better.<release>` / `cover_better.track.<track>`, a miss too,
//! asked again after [`RETRY_MS`], so art that appears later is picked up)
//! together with the cover it replaces: a file cover imported later wins
//! again. The picture goes through the web cover cache, never into the
//! music folders. Requests are spaced by the metadata clients' rate limits.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use nikgob_core::cover_frame;
use nikgob_core::model::{ReleaseId, TrackId};
use nikgob_core::streaming::{EntityKind, Service, StreamService, TrackKind};
use nikgob_library::{Library, TrackRow, is_web_ref, now_ms};
use nikgob_metadata::Metadata;
use nikgob_metadata::text::{artist_key, similarity, title_key};

use crate::Worker;
use crate::enrich::Event;
use crate::hd_covers;
use crate::import::Internal;

/// A lookup that found nothing is asked again after this long.
const RETRY_MS: i64 = 14 * 86_400_000;
/// A file cover smaller than this on its long side is poor.
pub(crate) const MIN_SIDE: u32 = 400;
/// Lookups per sweep of the library.
const BATCH: usize = 40;
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
            Self::Release(id) => format!("cover_better.{id}"),
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
}

fn folded(s: &str) -> String {
    artist_key(s).replace(' ', "")
}

/// Whether a hit of `artist`, `title` on `album` is the song (or the album)
/// `w` asks for.
pub(crate) fn accepts(w: &Want, artist: &str, title: &str, album: Option<&str>) -> bool {
    let a = artist_key(artist);
    if a.is_empty() {
        return false;
    }
    let named = w.artists.iter().any(|n| {
        let k = artist_key(n);
        !k.is_empty() && (similarity(&k, &a) >= 0.85 || nikgob_core::translit::same_translit(&k, &a))
    });
    let fa = folded(artist);
    let in_context = fa.chars().count() >= 2 && folded(&w.context).contains(&fa);
    if !(named || in_context) {
        return false;
    }
    let t = title_key(title).base;
    let song = !t.is_empty()
        && w.titles.iter().any(|x| {
            let k = title_key(x).base;
            k == t || similarity(&k, &t) >= 0.92
        });
    let same_album = match (&w.album, album) {
        (Some(want), Some(theirs)) => hd_covers::same_album(&want.title, theirs),
        _ => false,
    };
    song || same_album
}

/// The art for `w`, if a source has it.
pub(crate) async fn find(meta: Option<&Metadata>, services: &[Arc<dyn StreamService>], w: &Want) -> Result<Option<String>, String> {
    if let Some(album) = &w.album
        && let Some(url) = hd_covers::find(meta, services, album).await?
    {
        return Ok(Some(url));
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
                Err(e) => log::debug!("art of «{q}» on iTunes: {e}"),
            }
        }
    }
    // YouTube Music's songs carry their album's square art.
    for s in services {
        for q in queries.iter().take(2) {
            let Ok(found) = s.search(q).await else { continue };
            let hit = found.tracks.iter().find(|t| {
                t.kind == TrackKind::Song
                    && accepts(w, t.artists.first().map_or("", |a| a.name.as_str()), &t.title, t.album.as_ref().map(|a| a.title.as_str()))
            });
            if let Some(url) = hit.and_then(|t| t.cover_url.clone()).filter(|u| !u.contains("i.ytimg.com/")) {
                return Ok(Some(url));
            }
        }
    }
    Ok(None)
}

/// What to search for `w`, at most three: its first artist with its first
/// title, then the titles alone.
fn queries(w: &Want) -> Vec<String> {
    let main = w.artists.first().map(String::as_str).unwrap_or("");
    let mut out: Vec<String> = Vec::new();
    let first = w.titles.first().map(|t| format!("{main} {t}").trim().to_owned());
    for q in first.into_iter().chain(w.titles.iter().take(2).cloned()) {
        if !q.trim().is_empty() && !out.contains(&q) && out.len() < 3 {
            out.push(q);
        }
    }
    out
}

/// A clip title's song titles, the most specific first: a quoted title
/// («"ReawakeR" by LiSA»), the title before « by », then the halves of the
/// title from the last (the quoted song of «ヨルシカ「火星人」») to the whole.
fn clip_titles(title: &str, channel: &str) -> Vec<String> {
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
fn release_want(lib: &Library, root: &Path, id: ReleaseId, row: &TrackRow, now: i64) -> Option<Want> {
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
            album: Some(hd_covers::Want {
                id,
                mbid: r.mbid.clone(),
                group_mbid: r.release_group_mbid.clone(),
                deezer,
                title: r.title.clone(),
            }),
            ..Want::default()
        }
    };
    want.target = Some(target);
    want.current = current;
    Some(want)
}

/// The lookups `rows` (tracks of the library, or the one that started)
/// need, at most [`BATCH`], skipping what `asked` looked at already.
fn wants(lib: &Library, root: &Path, rows: &[TrackRow], asked: &Asked) -> Vec<Want> {
    let now = now_ms();
    let mut out = Vec::new();
    for row in rows {
        if out.len() == BATCH {
            break;
        }
        match row.release_id {
            Some(id) => {
                if asked.first(Target::Release(id))
                    && let Some(w) = release_want(lib, root, id, row, now)
                {
                    out.push(w);
                }
            }
            None if row.is_clip() => {
                let target = Target::Track(row.id);
                if asked.first(target) && due(lib, target, None, now) {
                    out.push(Want { target: Some(target), ..clip_want(row) });
                }
            }
            None => {}
        }
    }
    out
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
        let sweep = current.is_none();
        let services: Vec<Arc<dyn StreamService>> =
            [Service::Deezer, Service::Youtube].into_iter().filter_map(|k| self.service(k)).collect();
        let lib = self.lib.clone();
        let root = self.res.root().to_owned();
        self.streaming.spawn_reporting(move |tx| async move {
            if sweep {
                tokio::time::sleep(START).await;
            }
            let wants = {
                let (lib, root, asked) = (lib.clone(), root.clone(), asked.clone());
                tokio::task::spawn_blocking(move || {
                    let rows = if sweep {
                        lib.tracks(0, 50_000, nikgob_library::Sort::asc(nikgob_library::TrackSortKey::Album)).unwrap_or_default()
                    } else {
                        rows
                    };
                    wants(&lib, &root, &rows, &asked)
                })
                .await
                .unwrap_or_default()
            };
            let (mut found, mut failures) = (0, 0);
            for w in wants {
                let Some(target) = w.target else { continue };
                match tokio::time::timeout(Duration::from_secs(60), find(Some(&meta), &services, &w)).await {
                    Ok(Ok(url)) => {
                        failures = 0;
                        let url = url.unwrap_or_default();
                        let value = format!("{}{SEP}{url}", w.current.as_deref().unwrap_or(""));
                        let _ = lib.cache_put(&target.key(), now_ms(), &value);
                        if url.is_empty() {
                            log::debug!("no art for {target:?} ({:?})", w.titles.first());
                        } else {
                            log::info!("art for {target:?} ({:?}): {url}", w.titles.first());
                            found += 1;
                        }
                    }
                    Ok(Err(e)) => {
                        log::debug!("art for {target:?}: {e}");
                        failures += 1;
                    }
                    Err(_) => failures += 1,
                }
                if failures >= 3 {
                    break;
                }
            }
            if sweep {
                asked.lock().1 = false;
            }
            if found > 0 {
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
        assert!(queries(&w).len() <= 3);
        assert!(accepts(&w, "LiSA feat. Felix of Stray Kids", "ReawakeR (feat. Felix of Stray Kids)", Some("ReawakeR - Single")));
        assert!(!accepts(&w, "Dima Lancaster", "ReawakeR (from \"Solo Leveling\")", None), "a cover of the song");
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
}
