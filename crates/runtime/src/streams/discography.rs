//! "Нет в библиотеке" on a library artist's page (docs/stage3.md §0
//! "Дискография артиста", §3.4): the artist's albums in the services that
//! the page does not show already.
//!
//! * The artist is found in each enabled service by the id the library
//!   knows (`external_id`), else by a search for the name: a hit whose name
//!   matches (folded like the importer folds names) is taken when one of
//!   our release titles is among its albums, or when it is the only hit of
//!   that name. The match is remembered as the artist's id in the service.
//! * Albums the page has (a release of the artist with the same folded title
//!   and a year within ±1, or linked to it) are left out, and so are the
//!   same albums of a later service (Spotify first).
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

fn cache_key(artist: ArtistId) -> String {
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

/// Both years known and more than a year apart = different albums.
fn years_close(a: Option<i32>, b: Option<i32>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => (x - y).abs() <= 1,
        _ => true,
    }
}

/// The albums not on the page (`ours`: folded title and year of each
/// release shown there), deduplicated, grouped and sorted.
pub(crate) fn missing<'a>(
    albums: &'a [StreamAlbum],
    ours: &[(String, Option<i32>)],
    linked: impl Fn(&RemoteId) -> bool,
) -> Vec<&'a StreamAlbum> {
    let mut seen: Vec<(String, Option<i32>)> = Vec::new();
    let mut out = Vec::new();
    for a in albums {
        let key = album_key(&a.title);
        let same = |(k, y): &(String, Option<i32>)| *k == key && years_close(*y, a.year);
        if linked(&a.id) || ours.iter().any(same) || seen.iter().any(same) {
            continue;
        }
        seen.push((key, a.year));
        out.push(a);
    }
    out.sort_by_key(|a| (group_rank(a.release_type), std::cmp::Reverse(a.year.unwrap_or(i32::MIN))));
    out
}

/// Finds the artist in one service and reads its albums.
async fn lookup(
    svc: Arc<dyn StreamService>,
    name: String,
    ours: Vec<String>,
    known: Option<String>,
) -> Found {
    let service = svc.service();
    if let Some(id) = known {
        let full = svc.artist(&id).await;
        let image = full.as_ref().ok().and_then(|a| a.image_url.clone());
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
        let shared = full.albums.iter().any(|a| ours.contains(&album_key(&a.title)));
        if shared || only {
            return Found {
                service,
                artist: Some(full.id.clone()),
                matched_now: true,
                image: full.image_url.clone(),
                albums: Ok(full.albums),
            };
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
        let releases = self.lib.releases_of_artist(artist).unwrap_or_default();
        let ours: Vec<(String, Option<i32>)> = releases
            .iter()
            .map(|r| (album_key(&r.release.title), r.release.year))
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
        self.publish(|s, _| {
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
        let ours = vec![(album_key("Owned"), Some(2002))];
        let got: Vec<u32> = missing(&albums, &ours, |r| r.id == "a8")
            .iter()
            .map(|a| a.id.id[1..].parse().unwrap())
            .collect();
        assert_eq!(got, vec![3, 5, 2, 1, 6]);
    }
}
