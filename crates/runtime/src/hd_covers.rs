//! Larger covers for the stage of Главная's worlds (home-v2 §15.3): the
//! stage draws a cover of 568 points, up to 850 pixels at 150 %, and some
//! albums come with a cover of 300 px or less (Mushoku Tensei's are
//! 181–400 px). For an album whose own cover is smaller than [`HD_SIDE`],
//! the url of a larger one is looked up — the Cover Art Archive by the
//! album's MusicBrainz ids, the Deezer album linked to it, else the album
//! of its artist found by its title on the services (Deezer, YouTube Music, Spotify) —
//! and kept in the library's cache (`cover_hd.v2.<album>`; a miss too, asked
//! again after [`RETRY_MS`]). Only the url is kept: the picture goes through
//! the web cover cache like any cover of a service, never into the music
//! folders. The stage shows the album's own cover at once and fades the
//! larger one in once it has loaded.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use nikgob_core::model::ReleaseId;
use nikgob_core::streaming::{EntityKind, Service, StreamService};
use nikgob_library::{Library, now_ms};
use nikgob_metadata::{CoverTarget, Metadata};

use crate::Worker;
use crate::enrich::Event;
use crate::import::Internal;

/// An album's own cover smaller than this on its long side gets a larger one.
pub(crate) const HD_SIDE: u32 = 1000;
/// A lookup that found nothing is asked again after this long.
const RETRY_MS: i64 = 14 * 86_400_000;
/// Albums looked up per visit of Главная.
const BATCH: usize = 12;

fn key(id: ReleaseId) -> String {
    // `v2`: found with the album's artist (see [`Want::artists`]).
    format!("cover_hd.v2.{id}")
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
    pub deezer: Option<u64>,
    pub title: String,
    /// The album's artist names: an album found by its title is by one of
    /// them (titles alike are common: «There is a reason (RUS COVER)» is
    /// not «The Rush – Is the Reason»). Empty: the title alone.
    pub artists: Vec<String>,
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

/// The url of a larger cover of `w`, if a source has one.
pub(crate) async fn find(meta: Option<&Metadata>, services: &[Arc<dyn StreamService>], w: &Want) -> Result<Option<String>, String> {
    if let Some(meta) = meta {
        for target in [w.mbid.clone().map(CoverTarget::Release), w.group_mbid.clone().map(CoverTarget::ReleaseGroup)]
            .into_iter()
            .flatten()
        {
            match meta.cover_art.front_cover(&target).await {
                Ok(Some(c)) if c.thumbnails.contains_key("1200") => return Ok(Some(c.url_for(1200).to_owned())),
                Err(e) if crate::better_covers::meta_trouble(&e) => return Err(e.to_string()),
                Ok(_) | Err(_) => {}
            }
        }
        if let Some(id) = w.deezer {
            match meta.deezer.album(id).await {
                Ok(a) => {
                    if let Some(url) = a.cover_xl.filter(|u| !u.is_empty()) {
                        return Ok(Some(url));
                    }
                }
                Err(e) if crate::better_covers::meta_trouble(&e) => return Err(e.to_string()),
                Err(_) => {}
            }
        }
    }
    // By its title on the services, the first that has it. A service that
    // could not answer makes it no miss: it is asked again.
    let query = search_query(&w.title);
    let mut trouble = None;
    for s in services {
        let found = match s.search(&query).await {
            Ok(r) => r.albums,
            Err(e) => {
                log::debug!("a larger cover of «{}» on {}: {e}", w.title, s.service());
                if crate::better_covers::stream_trouble(&e) {
                    trouble.get_or_insert(format!("{}: {e}", s.service()));
                }
                continue;
            }
        };
        let by_them = |a: &nikgob_core::streaming::StreamAlbum| {
            w.artists.is_empty() || a.artists.iter().any(|x| crate::better_covers::names_artist(&w.artists, &x.name))
        };
        if let Some(url) = found
            .into_iter()
            .find(|a| same_album(&w.title, &a.title) && by_them(a))
            .and_then(|a| a.cover_url)
            .filter(|u| !u.is_empty())
        {
            return Ok(Some(url));
        }
    }
    trouble.map_or(Ok(None), Err)
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
            // Only a file cover (a service's is as large as it comes).
            let Some(cover) = r.cover_ref.as_deref().filter(|c| !nikgob_library::is_web_ref(c)) else { continue };
            if side(&self.res.path(cover)).is_some_and(|s| s >= HD_SIDE) {
                continue;
            }
            let deezer = self
                .lib
                .external_ids(EntityKind::Album, *id.as_uuid())
                .unwrap_or_default()
                .into_iter()
                .find(|x| x.service == Service::Deezer)
                .and_then(|x| x.id.parse().ok());
            wants.push(Want {
                id,
                mbid: r.mbid.clone(),
                group_mbid: r.release_group_mbid.clone(),
                deezer,
                title: r.title.clone(),
                artists: vec![r.artist_credit.display()],
            });
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
                match tokio::time::timeout(Duration::from_secs(45), find(meta.as_ref(), &services, &w)).await {
                    Ok(Ok(found)) => {
                        failures = 0;
                        let url = found.unwrap_or_default();
                        let _ = lib.cache_put(&key(w.id), now_ms(), &url);
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
        self.publish(|s| {
            for w in &mut s.home.worlds {
                if w.release == Some(id) {
                    w.cover_hd = Some(cover.clone());
                }
            }
            if let Some(w) = s.world.as_mut().filter(|w| w.world.release == Some(id)) {
                w.world.cover_hd = Some(cover.clone());
            }
        });
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
}
