//! "Нет в библиотеке" on a library artist's page: the artist's albums in the services that
//! the page does not show already.
//!
//! * The artist is found in each enabled service by the id the library
//!   knows (`external_id`), else by a search for the name: a hit whose name
//!   matches (folded like the importer folds names) is taken when one of
//!   our release titles is among its albums: then it is remembered as the
//!   artist's id in the service and its image is kept. The only hit of
//!   that name without a shared album is shown for this visit, never
//!   linked and its image not kept: a name does not make it the artist.
//!   An id known before gives its image only with a shared album too.
//! * Only albums credited to this artist are offered (one of its names,
//!   aliases or its Latin name among the album's artists; an album without
//!   credits too): a service may have merged two artists into the one the
//!   library links to.
//! * Albums the page has (a release of the artist of the same kind — a
//!   single, an EP and an album of one title are three releases — with the
//!   same folded title and a year within ±1, or linked to it) are left out,
//!   and so are the same albums of a later service (Spotify first).
//! * Which services: Spotify when it is signed in, else the Deezer
//!   catalogue (its discographies are good and need no account), then
//!   YouTube Music.
//! * Order: albums, then singles and EPs, then compilations, the rest last;
//!   newest first in each group.
//! * Each service is asked at most once a day per artist ([`TTL_MS`], kept
//!   in the library's cache; an hour when a service failed): Spotify's
//!   public quota is shared by every user.

use std::collections::HashSet;
use std::sync::Arc;

use nikgob_core::model::{ArtistId, ReleaseType, UnixMs};
use nikgob_core::streaming::{EntityKind, RemoteId, Service, StreamAlbum, StreamError, StreamService};
use nikgob_metadata::text::{album_key, artist_key};
use nikgob_view as v;

use super::{Msg, error_text};
use crate::Worker;

/// How long a looked-up discography is good for…
pub(crate) const TTL_MS: UnixMs = 24 * 3600 * 1000;
/// …and one that some service failed to give (not logged in yet, no
/// network).
const PARTIAL_TTL_MS: UnixMs = 3600 * 1000;
/// Hits of the right name whose albums are looked at.
const CANDIDATES: usize = 3;

pub(crate) struct DiscoMsg {
    artist: ArtistId,
    found: Vec<Found>,
}

/// What one service gave.
struct Found {
    service: Service,
    /// The artist in the service, and whether this lookup matched it.
    artist: Option<RemoteId>,
    matched_now: bool,
    /// Its image in the service.
    image: Option<String>,
    albums: Result<Vec<StreamAlbum>, StreamError>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Cached {
    albums: Vec<StreamAlbum>,
    error: Option<String>,
    /// Some service failed: looked up again sooner.
    #[serde(default)]
    partial: bool,
}

#[derive(Default)]
pub(crate) struct DiscoState {
    loading: HashSet<ArtistId>,
}

pub(crate) fn cache_key(artist: ArtistId) -> String {
    format!("discography.{artist}")
}

fn group_rank(t: ReleaseType) -> u8 {
    match t {
        ReleaseType::Album => 0,
        ReleaseType::Single | ReleaseType::Ep => 1,
        ReleaseType::Compilation => 2,
        ReleaseType::Other => 3,
    }
}

/// The kind a release is deduplicated within: a single, an EP, or an
/// album (a compilation and the rest count as one; the services file them
/// differently).
fn kind(t: ReleaseType) -> u8 {
    match t {
        ReleaseType::Single => 0,
        ReleaseType::Ep => 1,
        ReleaseType::Album | ReleaseType::Compilation | ReleaseType::Other => 2,
    }
}

/// Whether a release of kind `have` and year `year` is `a` of the same
/// title: of one kind, and their years within one (a year missing on
/// either side does not part them).
fn stands_for(have: ReleaseType, year: Option<i32>, a: &StreamAlbum) -> bool {
    kind(have) == kind(a.release_type)
        && match (year, a.year) {
            (Some(x), Some(y)) => (x - y).abs() <= 1,
            _ => true,
        }
}

/// A release on the page or found already: its folded title, year and
/// kind.
pub(crate) type Have = (String, Option<i32>, ReleaseType);

/// The albums not on the page (`ours`: each release shown there),
/// deduplicated, grouped and sorted.
pub(crate) fn missing<'a>(
    albums: &'a [StreamAlbum],
    ours: &[Have],
    linked: impl Fn(&RemoteId) -> bool,
) -> Vec<&'a StreamAlbum> {
    let mut seen: Vec<Have> = Vec::new();
    let mut out = Vec::new();
    for a in albums {
        let key = album_key(&a.title);
        let same = |(k, y, t): &Have| *k == key && stands_for(*t, *y, a);
        if linked(&a.id) || ours.iter().any(same) || seen.iter().any(same) {
            continue;
        }
        seen.push((key, a.year, a.release_type));
        out.push(a);
    }
    out.sort_by_key(|a| (group_rank(a.release_type), std::cmp::Reverse(a.year.unwrap_or(i32::MIN))));
    out
}

/// Whether `album` is credited to the artist named by `keys`
/// ([`crate::artist_names::credit_keys`]); an album without credits is.
fn credited(album: &StreamAlbum, keys: &[String]) -> bool {
    keys.is_empty() || album.artists.is_empty() || album.artists.iter().any(|a| keys.contains(&artist_key(&a.name)))
}

/// Finds the artist in one service and reads its albums.
async fn lookup(
    svc: Arc<dyn StreamService>,
    name: String,
    ours: Vec<String>,
    known: Option<String>,
) -> Found {
    let service = svc.service();
    let shared = |albums: &[StreamAlbum]| albums.iter().any(|a| ours.contains(&album_key(&a.title)));
    if let Some(id) = known {
        let full = svc.artist(&id).await;
        let image = full.as_ref().ok().filter(|a| shared(a.albums.as_slice())).and_then(|a| a.image_url.clone());
        return Found {
            service,
            artist: Some(RemoteId::new(service, id)),
            matched_now: false,
            image,
            albums: full.map(|a| a.albums),
        };
    }
    let none = |albums| Found {
        service,
        artist: None,
        matched_now: false,
        image: None,
        albums,
    };
    // Artists only: one request in the service's background lane.
    let hits = match svc.search_kind(&name, nikgob_core::streaming::SearchKind::Artists).await {
        Ok(s) => s.artists,
        Err(e) => return none(Err(e)),
    };
    let key = artist_key(&name);
    let exact: Vec<_> = hits
        .into_iter()
        .filter(|a| artist_key(&a.name) == key)
        .take(CANDIDATES)
        .collect();
    let only = exact.len() == 1;
    for c in exact {
        let Ok(full) = svc.artist(&c.id.id).await else { continue };
        if shared(full.albums.as_slice()) {
            return Found {
                service,
                artist: Some(full.id.clone()),
                matched_now: true,
                image: full.image_url.clone(),
                albums: Ok(full.albums),
            };
        }
        if only {
            return none(Ok(full.albums));
        }
    }
    none(Ok(Vec::new()))
}

impl Worker {
    /// Fills [`v::ArtistDetail::discography`] for the open artist: from the
    /// cache when it is fresh (and, when `retry`, not a failure), else from
    /// the services.
    pub(crate) fn load_discography(&mut self, artist: ArtistId, retry: bool) {
        let spotify = self
            .service(Service::Spotify)
            .filter(|s| s.account().is_logged_in());
        let first = spotify.or_else(|| self.service(Service::Deezer));
        let services: Vec<Arc<dyn StreamService>> =
            first.into_iter().chain(self.service(Service::Youtube)).collect();
        if services.is_empty() {
            self.publish_discography(artist, v::DiscographyView::default());
            return;
        }
        let now = nikgob_library::now_ms();
        let cached = self
            .lib
            .cache_get(&cache_key(artist))
            .ok()
            .flatten()
            .and_then(|(at, json)| Some((at, serde_json::from_str::<Cached>(&json).ok()?)))
            .filter(|(at, c)| now - at < if c.partial { PARTIAL_TTL_MS } else { TTL_MS })
            .map(|(_, c)| c);
        if let Some(c) = cached
            && !(retry && c.error.is_some())
        {
            let view = self.discography_view(artist, &c.albums, c.error);
            self.publish_discography(artist, view);
            return;
        }
        if !self.streaming.disco.loading.insert(artist) {
            self.publish_discography(artist, v::DiscographyView { loading: true, ..Default::default() });
            return;
        }
        let Some(a) = self.lib.get_artist(artist).ok().flatten() else {
            self.streaming.disco.loading.remove(&artist);
            return;
        };
        self.publish_discography(artist, v::DiscographyView { loading: true, ..Default::default() });
        let ours: Vec<String> = self
            .lib
            .releases_of_artist(artist)
            .unwrap_or_default()
            .iter()
            .map(|r| album_key(&r.release.title))
            .collect();
        let known = self
            .lib
            .external_ids(EntityKind::Artist, *artist.as_uuid())
            .unwrap_or_default();
        let name = a.name;
        self.streaming.spawn(async move {
            let mut found = Vec::new();
            for svc in services {
                let mine = known.iter().find(|r| r.service == svc.service()).map(|r| r.id.clone());
                found.push(lookup(svc, name.clone(), ours.clone(), mine).await);
            }
            Msg::Disco(DiscoMsg { artist, found })
        });
    }

    pub(crate) fn disco_msg(&mut self, m: DiscoMsg) {
        let artist = m.artist;
        self.streaming.disco.loading.remove(&artist);
        let mut albums = Vec::new();
        let mut errors = Vec::new();
        let mut ok = false;
        for f in m.found {
            if f.matched_now
                && let Some(remote) = &f.artist
            {
                log::info!("artist {artist} is {remote}");
                let _ = self.lib.set_external_id(EntityKind::Artist, remote, *artist.as_uuid());
            }
            if let Some(url) = &f.image {
                let _ = self.lib.set_stream_artist_image(artist, url);
            }
            match f.albums {
                Ok(a) => {
                    ok = true;
                    albums.extend(a);
                }
                Err(e) => {
                    log::warn!("{}: discography of {artist}: {e}", f.service);
                    errors.push(format!("{}: {}", f.service.display_name(), error_text(&e)));
                }
            }
        }
        let error = (!ok && !errors.is_empty()).then(|| errors.join("; "));
        let partial = !errors.is_empty();
        let cached = Cached { albums, error, partial };
        if let Ok(json) = serde_json::to_string(&cached) {
            // A failure is kept too (no hammering), but `retry` skips it.
            let _ = self.lib.cache_put(&cache_key(artist), nikgob_library::now_ms(), &json);
        }
        let view = self.discography_view(artist, &cached.albums, cached.error);
        self.publish_discography(artist, view);
    }

    fn discography_view(&self, artist: ArtistId, albums: &[StreamAlbum], error: Option<String>) -> v::DiscographyView {
        let name = self.lib.get_artist(artist).ok().flatten().map(|a| a.name).unwrap_or_default();
        let keys = crate::artist_names::credit_keys(&self.lib, artist, &name);
        let albums: Vec<StreamAlbum> = albums.iter().filter(|a| credited(a, &keys)).cloned().collect();
        let albums = albums.as_slice();
        let releases = self.lib.releases_of_artist(artist).unwrap_or_default();
        let ours: Vec<Have> = releases
            .iter()
            .map(|r| (album_key(&r.release.title), r.release.year, r.release.release_type))
            .collect();
        let ids: HashSet<_> = releases.iter().map(|r| r.release.id).collect();
        let linked = |remote: &RemoteId| {
            self.lib
                .release_by_remote(remote)
                .ok()
                .flatten()
                .is_some_and(|r| ids.contains(&r))
        };
        let missing = missing(albums, &ours, linked);
        let cards = missing.into_iter().map(|a| self.album_card(a)).collect();
        v::DiscographyView {
            loading: false,
            error,
            albums: cards,
        }
    }

    fn publish_discography(&mut self, artist: ArtistId, view: v::DiscographyView) {
        let loading = self.streaming.disco.loading.contains(&artist);
        self.publish(|s| {
            if let Some(a) = s.artist.as_mut().filter(|a| a.card.id == artist) {
                a.discography = v::DiscographyView { loading: loading || view.loading, ..view };
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(title: &str, year: i32, t: ReleaseType, n: u32) -> StreamAlbum {
        StreamAlbum {
            id: RemoteId::new(Service::Spotify, format!("a{n}")),
            title: title.into(),
            artists: Vec::new(),
            year: Some(year),
            release_date: None,
            release_type: t,
            cover_url: None,
            total_tracks: None,
            tracks: Vec::new(),
        }
    }

    #[test]
    fn missing_albums_are_deduplicated_grouped_and_newest_first() {
        let albums = vec![
            album("Single A", 2020, ReleaseType::Single, 1),
            album("Old", 1990, ReleaseType::Album, 2),
            album("New", 2015, ReleaseType::Album, 3),
            album("Owned", 2001, ReleaseType::Album, 4),
            album("Owned", 2010, ReleaseType::Album, 5), // a later re-release
            album("Best of", 2005, ReleaseType::Compilation, 6),
            album("new", 2016, ReleaseType::Album, 7), // the same on another service
            album("Linked", 1999, ReleaseType::Album, 8),
        ];
        let ours = vec![(album_key("Owned"), Some(2002), ReleaseType::Album)];
        let got: Vec<u32> = missing(&albums, &ours, |r| r.id == "a8")
            .iter()
            .map(|a| a.id.id[1..].parse().unwrap())
            .collect();
        assert_eq!(got, vec![3, 5, 2, 1, 6]);
    }

    /// A channel that merged two artists: only the albums credited to this
    /// one are offered. Its name in another script still matches, and so
    /// does an album with a guest beside it, or one without credits.
    #[test]
    fn only_albums_credited_to_the_artist_are_offered() {
        let credit = |title: &str, n: u32, names: &[&str]| StreamAlbum {
            artists: names.iter().map(|x| nikgob_core::streaming::ArtistRef::new(None, (*x).to_owned())).collect(),
            ..album(title, 2020, ReleaseType::Single, n)
        };
        let keys = vec![artist_key("gotlibgotlibgotlib")];
        assert!(credited(&credit("Own", 1, &["gotlibgotlibgotlib"]), &keys));
        assert!(!credited(&credit("Pop", 2, &["Готлиб"]), &keys));
        assert!(credited(&credit("Feat", 3, &["gotlibgotlibgotlib", "Guest"]), &keys));
        assert!(credited(&credit("Bare", 4, &[]), &keys));
        let kino = vec![artist_key("Кино")];
        assert!(credited(&credit("Группа крови", 5, &["Kino"]), &kino));
        assert!(!credited(&credit("Other", 6, &["Аквариум"]), &kino));

        // The names come from the library: its own, its Latin name.
        let lib = nikgob_library::Library::open_in_memory().unwrap();
        let a = lib.upsert_artist(&nikgob_core::model::Artist::new("藤澤慶昌")).unwrap();
        let latin = nikgob_library::LatinFound {
            name: Some("Yoshiaki Fujisawa".into()),
            source: "musicbrainz".into(),
            complete: true,
            ..nikgob_library::LatinFound::default()
        };
        lib.set_artist_latin(a.id, &latin).unwrap();
        let keys = crate::artist_names::credit_keys(&lib, a.id, &a.name);
        assert!(credited(&credit("OST", 7, &["Yoshiaki Fujisawa"]), &keys));
        assert!(credited(&credit("OST 2", 8, &["藤澤慶昌"]), &keys));
        assert!(!credited(&credit("Other", 9, &["Someone Else"]), &keys));
    }

    /// An artist of the name with one of our releases among its albums is
    /// linked with its image; the only one of the name without one is shown,
    /// not linked, no image. A known id gives no image without one either.
    #[tokio::test]
    async fn an_artist_image_needs_a_shared_album() {
        let svc: Arc<dyn StreamService> = Arc::new(nikgob_core::streaming::FakeService::spotify());
        let ours = vec![album_key("Группа крови")];
        let f = lookup(svc.clone(), "Кино".into(), ours.clone(), None).await;
        assert!(f.matched_now && f.image.is_some());
        let id = f.artist.expect("linked").id;
        let f = lookup(svc.clone(), "Кино".into(), ours, Some(id.clone())).await;
        assert!(f.image.is_some());

        let none = vec![album_key("Совсем другой альбом")];
        let f = lookup(svc.clone(), "Кино".into(), none.clone(), None).await;
        assert!(!f.matched_now && f.artist.is_none(), "not linked on the name alone");
        assert_eq!(f.image, None);
        assert!(f.albums.as_ref().is_ok_and(|a| !a.is_empty()), "its albums are shown");
        let f = lookup(svc, "Кино".into(), none, Some(id)).await;
        assert_eq!(f.image, None, "a link proves nothing by itself");
        assert!(f.albums.is_ok_and(|a| !a.is_empty()));
    }

    fn ids(got: Vec<&StreamAlbum>) -> Vec<u32> {
        got.iter().map(|a| a.id.id[1..].parse().unwrap()).collect()
    }

    /// The library has the single «Рисунки» that came out before the album
    /// «Рисунки»: the album is still missing from the page, and the single
    /// of a service is the library's. A single, an EP and an album of one
    /// title are three releases.
    #[test]
    fn a_single_in_the_library_does_not_hide_the_album_of_its_title() {
        let albums = vec![
            album("Рисунки", 2023, ReleaseType::Single, 1),
            album("Рисунки", 2023, ReleaseType::Album, 2),
            album("Рисунки", 2023, ReleaseType::Ep, 3),
        ];
        let single = vec![(album_key("Рисунки"), Some(2023), ReleaseType::Single)];
        assert_eq!(ids(missing(&albums, &single, |_| false)), vec![2, 3]);
        let whole = vec![(album_key("Рисунки"), Some(2023), ReleaseType::Album)];
        assert_eq!(ids(missing(&albums, &whole, |_| false)), vec![1, 3]);
        assert_eq!(ids(missing(&albums, &[], |_| false)), vec![2, 1, 3]);
        // The same single on another service is one.
        let twice = vec![album("Рисунки", 2023, ReleaseType::Single, 1), album("рисунки", 2023, ReleaseType::Single, 4)];
        assert_eq!(ids(missing(&twice, &[], |_| false)), vec![1]);
    }

    /// A year missing makes two releases of one title the same only when
    /// they are of one kind.
    #[test]
    fn a_missing_year_joins_only_releases_of_one_kind() {
        let mut undated = album("Рисунки", 2023, ReleaseType::Single, 1);
        undated.year = None;
        let albums = vec![undated, album("Рисунки", 2024, ReleaseType::Album, 2), album("Other", 2010, ReleaseType::Album, 3)];
        let ours = vec![(album_key("Other"), None, ReleaseType::Album), (album_key("Рисунки"), None, ReleaseType::Album)];
        // The undated album of the library is the service's album, not
        // its undated single.
        assert_eq!(ids(missing(&albums, &ours, |_| false)), vec![1]);
    }
}
