//! Who an artist of the library is outside it: a readable Latin name for
//! a name in a script the user may not read («佐宗綾子 · Ayako Saso»), and
//! the photo sources after Deezer.
//!
//! * The identity ([`identify`]): the MusicBrainz artist proved to be this
//!   one — by the MusicBrainz id its files carry, else a search of its
//!   name (or an alias) whose recordings or release groups share a title
//!   with the library's — with its aliases, its type (a Character is a
//!   role) and its Wikidata item; the item must not name another
//!   MusicBrainz artist.
//! * The Latin name ([`choose_latin`]): MusicBrainz's alias, Wikidata's
//!   English label, MusicBrainz's sort name, the name a connected service
//!   gives the artist proved there, last a kana name's romaji. Kanji get
//!   no guess.
//! * Photos after Deezer ([`photo_sources`]): the connected services
//!   (YouTube Music, then Spotify) where the library links the artist or
//!   its albums there share a title with the library's, then the Wikimedia
//!   Commons picture of its Wikidata item. A role gets none.
//!
//! What was found is kept in `artist_latin` ([`nikgob_library::LatinFound`]),
//! a miss too: asked again after [`NAME_RETRY_MS`].

use std::collections::HashSet;
use std::sync::Arc;

use nikgob_core::model::ArtistId;
use nikgob_core::names::{foreign_script, is_latin, romaji_name};
use nikgob_core::streaming::{EntityKind, SearchKind, Service, StreamService};
use nikgob_library::{LatinFound, Library, ROMAJI, now_ms};
use nikgob_metadata::text::{artist_key, titles_overlap};
use nikgob_metadata::{ArtistInfo, Metadata, MetadataError};

use crate::enrich::{Found, MAX_FAILURES, Pace};

/// A lookup that found no name (or only a romaji reading) is repeated
/// after this long.
pub(crate) const NAME_RETRY_MS: i64 = 30 * 86_400_000;
/// Same-named artists of a service whose albums are compared.
const SERVICE_CANDIDATES: usize = 3;
/// Same-named MusicBrainz artists whose titles are compared.
const MB_CANDIDATES: usize = 3;
/// The library's titles of an artist handed to the proofs.
const TITLES: u32 = 60;
const BATCH: u32 = 25;

/// What [`identify`] proved of an artist.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Identity {
    pub(crate) mbid: Option<String>,
    pub(crate) wikidata: Option<String>,
    pub(crate) character: bool,
    /// The Latin name and where it came from.
    pub(crate) latin: Option<(String, &'static str)>,
    /// The Commons file of its picture.
    pub(crate) picture: Option<String>,
}

/// Where a photo after Deezer's comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PhotoSource {
    /// The artist's image in a connected service.
    Service(Service),
    /// The Wikimedia Commons picture of its Wikidata item.
    Wikimedia,
}

/// The sources after Deezer, in the order they are asked: each connected
/// service (YouTube Music first: Spotify's public quota is shared by every
/// user), then Wikimedia Commons.
pub(crate) fn photo_sources(connected: &[Service]) -> Vec<PhotoSource> {
    let mut out: Vec<PhotoSource> = [Service::Youtube, Service::Spotify]
        .into_iter()
        .filter(|s| connected.contains(s))
        .map(PhotoSource::Service)
        .collect();
    out.push(PhotoSource::Wikimedia);
    out
}

/// The Latin name to show beside `name`, best first (see the module docs).
/// `None` for a name in Latin or Cyrillic letters and for kanji nothing
/// names.
pub(crate) fn choose_latin(
    name: &str,
    mb: Option<&ArtistInfo>,
    wikidata_label: Option<&str>,
    service: Option<(String, &'static str)>,
) -> Option<(String, &'static str)> {
    if !foreign_script(name) {
        return None;
    }
    // Its aliases (or its own Latin name), the sort name not yet.
    if let Some(n) = mb.and_then(|a| nikgob_metadata::musicbrainz::latin_alias(&a.name, "", &a.aliases)) {
        return Some((n, "musicbrainz"));
    }
    if let Some(l) = wikidata_label.filter(|l| is_latin(l)) {
        return Some((l.trim().to_owned(), "wikidata"));
    }
    if let Some(n) = mb.and_then(ArtistInfo::latin_name) {
        return Some((n, "musicbrainz"));
    }
    if let Some((n, source)) = service.filter(|(n, _)| is_latin(n)) {
        return Some((n.trim().to_owned(), source));
    }
    romaji_name(name).map(|r| (r, ROMAJI))
}

/// The library's titles of `id`: its tracks' and their albums'.
fn titles(lib: &Library, id: ArtistId) -> Vec<String> {
    let tracks = lib.tracks_of_artist(id, 0, TITLES).unwrap_or_default();
    let mut out: Vec<String> = tracks.iter().map(|t| t.title.clone()).collect();
    for album in tracks.iter().filter_map(|t| t.album.clone()) {
        if !out.contains(&album) {
            out.push(album);
        }
    }
    out
}

/// The MusicBrainz artist proved to be `id` (see the module docs).
async fn musicbrainz_artist(
    meta: &Metadata,
    lib: &Library,
    id: ArtistId,
    name: &str,
    titles: &[String],
) -> Result<Option<ArtistInfo>, MetadataError> {
    let tagged = lib.get_artist(id).ok().flatten().and_then(|a| a.mbid).filter(|m| !m.trim().is_empty());
    if let Some(mbid) = tagged {
        return match meta.musicbrainz.artist_info(&mbid).await {
            Ok(a) => Ok(Some(a)),
            Err(MetadataError::NotFound | MetadataError::InvalidArgument(_)) => Ok(None),
            Err(e) => Err(e),
        };
    }
    if titles.is_empty() {
        return Ok(None);
    }
    let hits = meta.musicbrainz.search_artists(name, 10).await?;
    for hit in hits.into_iter().take(MB_CANDIDATES) {
        if meta.musicbrainz.artist_has_titles(&hit.id, titles).await? {
            return meta.musicbrainz.artist_info(&hit.id).await.map(Some);
        }
    }
    Ok(None)
}

/// The artist in `svc`: the one the library links it to, else a hit of
/// one of `names` whose albums there share a title with `titles` (linked
/// from now on). Its image (the search's square one first) and its name
/// there. Any failure is «not there».
async fn service_artist(
    svc: &dyn StreamService,
    lib: &Library,
    id: ArtistId,
    names: &[String],
    titles: &[String],
) -> Option<(Option<String>, String)> {
    let service = svc.service();
    let linked = lib
        .external_ids(EntityKind::Artist, *id.as_uuid())
        .unwrap_or_default()
        .into_iter()
        .find(|r| r.service == service);
    if let Some(remote) = linked {
        let a = svc.artist(&remote.id).await.ok()?;
        return Some((a.image_url, a.name));
    }
    let keys: Vec<String> = names.iter().map(|n| artist_key(n)).filter(|k| !k.is_empty()).collect();
    let mut asked = HashSet::new();
    for name in names {
        let hits = svc.search_kind(name, SearchKind::Artists).await.ok()?.artists;
        let same = hits.into_iter().filter(|h| keys.contains(&artist_key(&h.name)) && asked.insert(h.id.id.clone()));
        for hit in same.take(SERVICE_CANDIDATES) {
            let Ok(full) = svc.artist(&hit.id.id).await else { continue };
            let theirs: Vec<String> = full.albums.iter().map(|a| a.title.clone()).collect();
            if !titles_overlap(titles, &theirs) {
                continue;
            }
            let _ = lib.set_external_id(EntityKind::Artist, &hit.id, *id.as_uuid());
            return Some((hit.image_url.or(full.image_url), full.name));
        }
    }
    None
}

/// The connected services in [`photo_sources`]' order.
fn in_order(services: &[Arc<dyn StreamService>]) -> Vec<&Arc<dyn StreamService>> {
    let connected: Vec<Service> = services.iter().map(|s| s.service()).collect();
    photo_sources(&connected)
        .into_iter()
        .filter_map(|p| match p {
            PhotoSource::Service(kind) => services.iter().find(|s| s.service() == kind),
            PhotoSource::Wikimedia => None,
        })
        .collect()
}

/// Who `id` (called `name`) is (see the module docs). An error is the
/// network's: nothing is to be recorded.
pub(crate) async fn identify(
    meta: &Metadata,
    services: &[Arc<dyn StreamService>],
    lib: &Library,
    id: ArtistId,
    name: &str,
) -> Result<Identity, MetadataError> {
    let titles = titles(lib, id);
    let mut out = Identity::default();
    let mb = musicbrainz_artist(meta, lib, id, name, &titles).await?;
    let mut label = None;
    if let Some(a) = &mb {
        out.mbid = Some(a.id.clone());
        out.character = a.is_character();
        out.wikidata = a.wikidata();
        if let Some(q) = out.wikidata.clone() {
            match meta.wikidata.artist(&q).await {
                // An item that names other MusicBrainz artists is not this one's.
                Ok(w) if w.musicbrainz.is_empty() || w.musicbrainz.contains(&a.id) => {
                    label = w.label_en;
                    out.picture = w.image;
                }
                Ok(_) => out.wikidata = None,
                Err(MetadataError::NotFound | MetadataError::InvalidArgument(_)) => {}
                Err(e) => return Err(e),
            }
        }
    }
    let mut latin = choose_latin(name, mb.as_ref(), label.as_deref(), None);
    // No name of its own yet: what the services call it.
    if foreign_script(name) && latin.as_ref().is_none_or(|(_, s)| *s == ROMAJI) {
        let names = [name.to_owned()];
        for svc in in_order(services) {
            let Some((_, theirs)) = service_artist(&**svc, lib, id, &names, &titles).await else { continue };
            if let Some(found) = choose_latin(name, None, None, Some((theirs, svc.service().as_str())))
                && found.1 != ROMAJI
            {
                latin = Some(found);
                break;
            }
        }
    }
    out.latin = latin;
    Ok(out)
}

/// Records `who` for `id`; says what changed through `found`.
fn record(lib: &Library, id: ArtistId, who: &Identity, found: &impl Fn(Found)) {
    let row = LatinFound {
        name: who.latin.as_ref().map(|(n, _)| n.clone()),
        source: who.latin.as_ref().map_or_else(String::new, |(_, s)| (*s).to_owned()),
        character: who.character,
        mbid: who.mbid.clone(),
        wikidata: who.wikidata.clone(),
    };
    match lib.set_artist_latin(id, &row) {
        Ok(merged) => {
            if who.latin.is_some() || who.character {
                found(Found::Name(id));
            }
            if merged > 0 {
                found(Found::Reload);
            }
        }
        Err(e) => log::warn!("recording who artist {id} is: {e}"),
    }
}

/// The Commons file of `id`'s picture: from the Wikidata item of a recent
/// lookup, else from a new one (recorded). `None` for a role.
async fn picture_file(
    meta: &Metadata,
    services: &[Arc<dyn StreamService>],
    lib: &Library,
    id: ArtistId,
    name: &str,
    found: &impl Fn(Found),
) -> Option<String> {
    let known = lib.artist_latin(id).ok().flatten().filter(|r| r.fetched_at > now_ms() - NAME_RETRY_MS);
    if let Some(row) = known {
        if row.character {
            return None;
        }
        let item = meta.wikidata.artist(&row.wikidata?).await.ok()?;
        return item.image;
    }
    let who = nikgob_metadata::in_background(identify(meta, services, lib, id, name)).await.ok()?;
    record(lib, id, &who, found);
    if who.character { None } else { who.picture }
}

/// The photo sources after Deezer ([`photo_sources`]).
#[derive(Clone, Default)]
pub(crate) struct After {
    /// MusicBrainz, Wikidata and the download of a service's image;
    /// `None`: only Deezer is asked.
    pub(crate) meta: Option<Metadata>,
    /// The services the user has connected.
    pub(crate) services: Vec<Arc<dyn StreamService>>,
}

/// A photo of `id` from the sources after Deezer: its bytes (a JPEG or a
/// PNG) and the source (`youtube`, `spotify`, `wikimedia`).
pub(crate) async fn photo_after_deezer(
    after: &After,
    lib: &Library,
    id: ArtistId,
    name: &str,
    found: &impl Fn(Found),
) -> Option<(Vec<u8>, &'static str)> {
    let meta = after.meta.as_ref()?;
    let titles = titles(lib, id);
    let mut names = vec![name.to_owned()];
    if let Some(l) = lib.artist_latin(id).ok().flatten().and_then(|l| l.name).filter(|l| l != name) {
        names.push(l);
    }
    let connected: Vec<Service> = after.services.iter().map(|s| s.service()).collect();
    for source in photo_sources(&connected) {
        match source {
            PhotoSource::Service(kind) => {
                let Some(svc) = after.services.iter().find(|s| s.service() == kind) else { continue };
                let Some((Some(url), _)) = service_artist(&**svc, lib, id, &names, &titles).await else { continue };
                match meta.web_images.download(&url).await {
                    Ok(bytes) if crate::enrich::looks_like_image(&bytes) => return Some((bytes, kind.as_str())),
                    Ok(_) => {}
                    Err(e) => log::debug!("{kind} photo of {name}: {e}"),
                }
            }
            PhotoSource::Wikimedia => {
                let file = picture_file(meta, &after.services, lib, id, name, found).await?;
                let bytes = meta.wikidata.download_image(&file).await.ok()?;
                return crate::enrich::looks_like_image(&bytes).then_some((bytes, "wikimedia"));
            }
        }
    }
    None
}

/// What one names run did, for its log line.
#[derive(Default)]
struct NameTally {
    roles: usize,
    named: u32,
    romaji: u32,
    none: u32,
    failed: u32,
}

impl NameTally {
    fn log(&self) {
        if self.roles == 0 && self.named + self.romaji + self.none + self.failed == 0 {
            return;
        }
        log::info!(
            "artist names: {} found, {} read in romaji, {} not found, {} failed, {} roles marked",
            self.named,
            self.romaji,
            self.none,
            self.failed,
            self.roles
        );
    }
}

/// Looks up the Latin names of the artists that need one
/// ([`Library::artists_needing_latin`]), slowly and in the background
/// lanes; `roles` is how many roles this run marked (for its log line).
pub(crate) async fn run_names(
    meta: Metadata,
    services: Vec<Arc<dyn StreamService>>,
    lib: Library,
    pace: Pace,
    roles: usize,
    found: impl Fn(Found),
) {
    let mut tally = NameTally { roles, ..NameTally::default() };
    let mut seen = HashSet::new();
    let mut failures = 0;
    'runs: loop {
        let batch = match lib.artists_needing_latin(now_ms() - NAME_RETRY_MS, BATCH) {
            Ok(b) => b,
            Err(e) => {
                log::warn!("names queue: {e}");
                break;
            }
        };
        // Each artist once a run, also when its answer could not be kept.
        let batch: Vec<_> = batch.into_iter().filter(|(id, _)| seen.insert(*id)).collect();
        if batch.is_empty() {
            break;
        }
        for (id, name) in batch {
            match nikgob_metadata::in_background(identify(&meta, &services, &lib, id, &name)).await {
                Ok(who) => {
                    failures = 0;
                    match &who.latin {
                        Some((_, s)) if *s == ROMAJI => tally.romaji += 1,
                        Some(_) => tally.named += 1,
                        None => tally.none += 1,
                    }
                    record(&lib, id, &who, &found);
                }
                Err(e) => {
                    log::debug!("who {name} is: {e}");
                    failures += 1;
                    tally.failed += 1;
                    if failures >= MAX_FAILURES {
                        break 'runs;
                    }
                }
            }
            tokio::time::sleep(pace.between).await;
            // Unattended: waits while a game is in front.
            crate::quiet::free().await;
        }
    }
    tally.log();
}

#[cfg(test)]
mod tests {
    use nikgob_metadata::musicbrainz::Alias;

    use super::*;

    fn mb(name: &str, sort: &str, aliases: Vec<Alias>) -> ArtistInfo {
        ArtistInfo { id: "x".into(), name: name.into(), sort_name: sort.into(), aliases, ..ArtistInfo::default() }
    }

    fn alias(name: &str, locale: &str) -> Alias {
        Alias { name: name.into(), locale: Some(locale.into()), kind: Some("Artist name".into()), primary: true, ..Alias::default() }
    }

    #[test]
    fn the_name_comes_from_musicbrainz_then_wikidata_then_the_sort_name_then_a_service_then_romaji() {
        let saso = mb("佐宗綾子", "Saso, Ayako", vec![alias("Ayako Saso", "en")]);
        let bare = mb("佐宗綾子", "Saso, Ayako", Vec::new());
        let service = || Some(("Sasou Ayako".to_owned(), "youtube"));
        let pick = |a: Option<&ArtistInfo>, label: Option<&str>, svc| choose_latin("佐宗綾子", a, label, svc);
        assert_eq!(pick(Some(&saso), Some("Ayako Sasō"), service()), Some(("Ayako Saso".into(), "musicbrainz")));
        assert_eq!(pick(Some(&bare), Some("Ayako Sasō"), service()), Some(("Ayako Sasō".into(), "wikidata")));
        assert_eq!(pick(Some(&bare), None, service()), Some(("Ayako Saso".into(), "musicbrainz")));
        assert_eq!(pick(Some(&mb("佐宗綾子", "佐宗綾子", Vec::new())), Some("佐宗綾子"), service()), Some(("Sasou Ayako".into(), "youtube")));
        assert_eq!(pick(None, None, Some(("佐宗綾子".into(), "spotify"))), None, "kanji get no guess");
        assert_eq!(pick(None, None, None), None);
    }

    #[test]
    fn romaji_only_for_kana_and_nothing_for_names_one_can_read() {
        assert_eq!(choose_latin("リコ", None, None, None), Some(("Riko".into(), ROMAJI)));
        assert_eq!(choose_latin("ナナホシ", None, None, None), Some(("Nanahoshi".into(), ROMAJI)));
        assert_eq!(choose_latin("大原ゆい子", None, None, None), None);
        assert_eq!(choose_latin("Кино", None, None, Some(("Kino".into(), "spotify"))), None);
        assert_eq!(choose_latin("Aminé", Some(&mb("Aminé", "Aminé", Vec::new())), None, None), None);
    }

    #[test]
    fn photos_after_deezer_come_from_the_services_then_wikimedia() {
        use PhotoSource::{Service as S, Wikimedia};
        assert_eq!(photo_sources(&[]), vec![Wikimedia]);
        assert_eq!(
            photo_sources(&[Service::Spotify, Service::Deezer, Service::Youtube]),
            vec![S(Service::Youtube), S(Service::Spotify), Wikimedia]
        );
        assert_eq!(photo_sources(&[Service::Spotify]), vec![S(Service::Spotify), Wikimedia]);
    }
}
