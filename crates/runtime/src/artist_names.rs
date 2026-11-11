//! Who an artist of the library is outside it: a readable Latin name for
//! a name in a script the user may not read («佐宗綾子 · Ayako Saso»),
//! whether it is a role, and the photo sources after Deezer.
//!
//! * The identity ([`identify`]): the MusicBrainz artist proved to be this
//!   one — by the MusicBrainz id its files carry, else a search of its
//!   name (or an alias) where exactly one candidate shares at least two
//!   distinct, not generic titles with the library
//!   ([`nikgob_metadata::text::titles_shared`]); one shared title, or two
//!   candidates that both qualify, prove nothing — with its aliases, its
//!   type (a Character is a role) and its Wikidata item, which must not
//!   name another MusicBrainz artist (checked on every read of it).
//! * The Latin name ([`choose_latin`]): MusicBrainz's alias, Wikidata's
//!   English label, MusicBrainz's sort name, the name a connected service
//!   gives the artist proved there (by the same rule, or by the id the
//!   library links), last a kana name's romaji. Kanji get no guess. It is
//!   for display and search only.
//! * Photos after Deezer ([`photo_sources`]): the connected services
//!   (YouTube Music, then Spotify), then the Wikimedia Commons picture of
//!   the Wikidata item. Before any photo lookup the artist's identity is
//!   settled ([`known`]): a role gets none, and it is checked again before
//!   a photo is saved.
//! * A source that cannot answer for now (rate limited, unreachable, a
//!   server error: [`Busy`]) is no miss: nothing it would have said is
//!   recorded, and after two such failures in a row it is left out until
//!   the run ends ([`Gate`]).
//!
//! What was found is kept in `artist_latin` ([`nikgob_library::LatinFound`]),
//! a complete miss too: asked again after [`NAME_RETRY_MS`].

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use nikgob_core::model::ArtistId;
use nikgob_core::names::{foreign_script, is_latin, romaji_name};
use nikgob_core::streaming::{EntityKind, SearchKind, Service, StreamArtist, StreamError, StreamService};
use nikgob_library::{LatinFound, Library, ROMAJI, now_ms};
use nikgob_metadata::text::{SHARED_TITLES_PROOF, artist_key, titles_shared};
use nikgob_metadata::{ArtistInfo, Metadata, MetadataError, WikidataArtist};

use crate::enrich::{Found, MAX_FAILURES, Pace};

/// A lookup that found no name (or only a romaji reading) is repeated
/// after this long, and so is the identity a photo lookup relies on.
pub(crate) const NAME_RETRY_MS: i64 = 30 * 86_400_000;
/// Same-named artists of a service whose albums are compared.
const SERVICE_CANDIDATES: usize = 3;
/// Same-named MusicBrainz artists whose titles are compared.
const MB_CANDIDATES: usize = 3;
/// The library's titles of an artist handed to the proofs.
const TITLES: u32 = 60;
const BATCH: u32 = 25;
/// Failures for now in a row after which a source sits out the run.
const GATE_FAILURES: u32 = 2;
const MUSICBRAINZ: &str = "musicbrainz";
const WIKIMEDIA: &str = "wikimedia";

/// A source that could not answer for now (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Busy(pub(crate) &'static str);

/// Rate limited, unreachable, timed out or a server error: for now.
pub(crate) fn busy_meta(e: &MetadataError) -> bool {
    match e {
        MetadataError::Network(_) | MetadataError::RateLimited(_) => true,
        MetadataError::Status { status, .. } => *status == 429 || *status >= 500,
        _ => false,
    }
}

/// [`busy_meta`] of a service (a lost login too).
pub(crate) fn busy_stream(e: &StreamError) -> bool {
    match e {
        StreamError::RateLimited { .. } | StreamError::Network(_) | StreamError::NotLoggedIn => true,
        StreamError::Http { status } => *status == 429 || *status >= 500,
        _ => false,
    }
}

/// The sources that failed for now in one run: after [`GATE_FAILURES`] in
/// a row a source is left out until the run ends.
#[derive(Debug, Default)]
pub(crate) struct Gate {
    failures: Mutex<HashMap<&'static str, u32>>,
}

impl Gate {
    fn count(&self, source: &'static str) -> u32 {
        self.failures.lock().map(|f| f.get(source).copied().unwrap_or(0)).unwrap_or(0)
    }

    /// The source may be asked in this run.
    pub(crate) fn open(&self, source: &'static str) -> bool {
        self.count(source) < GATE_FAILURES
    }

    /// The source answered.
    pub(crate) fn ok(&self, source: &'static str) {
        if let Ok(mut f) = self.failures.lock() {
            f.remove(source);
        }
    }

    /// The source failed for now; returns that as a [`Busy`].
    pub(crate) fn failed(&self, source: &'static str) -> Busy {
        if let Ok(mut f) = self.failures.lock() {
            let n = f.entry(source).or_insert(0);
            *n += 1;
            if *n == GATE_FAILURES {
                log::info!("artist lookups: {source} fails for now, left out until the next run");
            }
        }
        Busy(source)
    }
}

/// What [`identify`] proved of an artist.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Identity {
    pub(crate) mbid: Option<String>,
    pub(crate) wikidata: Option<String>,
    pub(crate) character: bool,
    /// The Latin name and where it came from.
    pub(crate) latin: Option<(String, &'static str)>,
    /// The Commons file of its picture, its Wikidata item checked.
    pub(crate) picture: Option<String>,
}

/// An identity and whether every source of the name was asked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Lookup {
    pub(crate) identity: Identity,
    pub(crate) complete: bool,
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

/// The Wikidata item is `mbid`'s: it names no other MusicBrainz artist.
fn item_is(item: &WikidataArtist, mbid: &str) -> bool {
    item.musicbrainz.is_empty() || item.musicbrainz.iter().any(|m| m == mbid)
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
    let mut proved = Vec::new();
    for hit in hits.into_iter().take(MB_CANDIDATES) {
        if meta.musicbrainz.artist_shared_titles(&hit.id, titles).await? >= SHARED_TITLES_PROOF {
            proved.push(hit.id);
        }
    }
    match proved.as_slice() {
        [one] => meta.musicbrainz.artist_info(one).await.map(Some),
        [] => Ok(None),
        _ => {
            log::debug!("{name}: {} MusicBrainz artists share its titles, none taken", proved.len());
            Ok(None)
        }
    }
}

/// What a service says of the artist proved there.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ServiceHit {
    /// Its image (the search's square one first).
    image: Option<String>,
    /// Its name there.
    name: String,
}

/// How many of the library's titles `a` shares in its service: its
/// albums, then its top songs too when those are not enough.
async fn shared_in(svc: &dyn StreamService, a: &StreamArtist, titles: &[String]) -> Result<usize, StreamError> {
    let mut theirs: Vec<String> = a.albums.iter().map(|al| al.title.clone()).collect();
    let shared = titles_shared(titles, &theirs);
    if shared >= SHARED_TITLES_PROOF {
        return Ok(shared);
    }
    match svc.artist_top(&a.id.id).await {
        Ok(top) => theirs.extend(top.into_iter().map(|t| t.title)),
        Err(e) if busy_stream(&e) => return Err(e),
        Err(_) => {}
    }
    Ok(titles_shared(titles, &theirs))
}

/// The artist in `svc`: the one the library links it to, else the only
/// hit of one of `names` that shares two titles with `titles` there
/// (linked from now on). Ambiguous: none, and no link.
async fn service_artist(
    svc: &dyn StreamService,
    lib: &Library,
    id: ArtistId,
    names: &[String],
    titles: &[String],
) -> Result<Option<ServiceHit>, Busy> {
    let service = svc.service();
    let busy = Busy(service.as_str());
    let linked = lib
        .external_ids(EntityKind::Artist, *id.as_uuid())
        .unwrap_or_default()
        .into_iter()
        .find(|r| r.service == service);
    if let Some(remote) = linked {
        return match svc.artist(&remote.id).await {
            Ok(a) => Ok(Some(ServiceHit { image: a.image_url, name: a.name })),
            Err(e) if busy_stream(&e) => Err(busy),
            Err(_) => Ok(None),
        };
    }
    if titles.is_empty() {
        return Ok(None);
    }
    let keys: Vec<String> = names.iter().map(|n| artist_key(n)).filter(|k| !k.is_empty()).collect();
    let mut asked = HashSet::new();
    for name in names {
        let hits = match svc.search_kind(name, SearchKind::Artists).await {
            Ok(s) => s.artists,
            Err(e) if busy_stream(&e) => return Err(busy),
            Err(_) => continue,
        };
        let same: Vec<StreamArtist> = hits
            .into_iter()
            .filter(|h| keys.contains(&artist_key(&h.name)) && asked.insert(h.id.id.clone()))
            .take(SERVICE_CANDIDATES)
            .collect();
        let mut proved = Vec::new();
        for hit in same {
            let full = match svc.artist(&hit.id.id).await {
                Ok(f) => f,
                Err(e) if busy_stream(&e) => return Err(busy),
                Err(_) => continue,
            };
            match shared_in(svc, &full, titles).await {
                Ok(n) if n >= SHARED_TITLES_PROOF => proved.push((hit, full)),
                Ok(_) => {}
                Err(_) => return Err(busy),
            }
        }
        if proved.len() > 1 {
            log::debug!("{name}: {} artists of {service} share its titles, none taken", proved.len());
            return Ok(None);
        }
        if let Some((hit, full)) = proved.pop() {
            let _ = lib.set_external_id(EntityKind::Artist, &hit.id, *id.as_uuid());
            return Ok(Some(ServiceHit { image: hit.image_url.or(full.image_url), name: full.name }));
        }
    }
    Ok(None)
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

/// Who `id` (called `name`) is (see the module docs). The services are
/// asked for a name only when `ask_services` (the names run; a photo
/// lookup needs the identity only); without them a lookup that found no
/// name of its own is not complete. `Err`: MusicBrainz or Wikidata could
/// not answer for now.
pub(crate) async fn identify(
    meta: &Metadata,
    services: &[Arc<dyn StreamService>],
    gate: &Gate,
    lib: &Library,
    id: ArtistId,
    name: &str,
    ask_services: bool,
) -> Result<Lookup, Busy> {
    if !gate.open(MUSICBRAINZ) {
        return Err(Busy(MUSICBRAINZ));
    }
    let titles = titles(lib, id);
    let mut out = Identity::default();
    let mb = match musicbrainz_artist(meta, lib, id, name, &titles).await {
        Ok(a) => {
            gate.ok(MUSICBRAINZ);
            a
        }
        Err(e) if busy_meta(&e) => return Err(gate.failed(MUSICBRAINZ)),
        Err(e) => {
            log::debug!("{name} on MusicBrainz: {e}");
            None
        }
    };
    let mut label = None;
    if let Some(a) = &mb {
        out.mbid = Some(a.id.clone());
        out.character = a.is_character();
        out.wikidata = a.wikidata();
        if let Some(q) = out.wikidata.clone() {
            if !gate.open(WIKIMEDIA) {
                return Err(Busy(WIKIMEDIA));
            }
            match meta.wikidata.artist(&q).await {
                Ok(w) if item_is(&w, &a.id) => {
                    gate.ok(WIKIMEDIA);
                    label = w.label_en;
                    out.picture = w.image;
                }
                Ok(_) => {
                    gate.ok(WIKIMEDIA);
                    out.wikidata = None;
                }
                Err(e) if busy_meta(&e) => return Err(gate.failed(WIKIMEDIA)),
                Err(_) => {}
            }
        }
    }
    let mut latin = choose_latin(name, mb.as_ref(), label.as_deref(), None);
    let mut complete = true;
    // No name of its own yet: what the services call it.
    if foreign_script(name) && latin.as_ref().is_none_or(|(_, s)| *s == ROMAJI) && !services.is_empty() {
        complete = ask_services;
        if ask_services {
            let names = [name.to_owned()];
            for svc in in_order(services) {
                let key = svc.service().as_str();
                if !gate.open(key) {
                    complete = false;
                    continue;
                }
                match service_artist(&**svc, lib, id, &names, &titles).await {
                    Ok(Some(hit)) => {
                        gate.ok(key);
                        if let Some(found) = choose_latin(name, None, None, Some((hit.name, key)))
                            && found.1 != ROMAJI
                        {
                            latin = Some(found);
                            break;
                        }
                    }
                    Ok(None) => gate.ok(key),
                    Err(Busy(source)) => {
                        gate.failed(source);
                        complete = false;
                    }
                }
            }
        }
    }
    if latin.as_ref().is_some_and(|(_, s)| *s != ROMAJI) {
        complete = true;
    }
    out.latin = latin;
    Ok(Lookup { identity: out, complete })
}

/// Records `look` for `id`; says what changed through `found`.
fn record(lib: &Library, id: ArtistId, look: &Lookup, found: &impl Fn(Found)) {
    let who = &look.identity;
    let row = LatinFound {
        name: who.latin.as_ref().map(|(n, _)| n.clone()),
        source: who.latin.as_ref().map_or_else(String::new, |(_, s)| (*s).to_owned()),
        character: who.character,
        mbid: who.mbid.clone(),
        wikidata: who.wikidata.clone(),
        complete: look.complete,
    };
    match lib.set_artist_latin(id, &row) {
        Ok(_) => {
            if who.latin.is_some() || who.character {
                found(Found::Name(id));
            }
        }
        Err(e) => log::warn!("recording who artist {id} is: {e}"),
    }
}

/// What a photo lookup knows of the artist before it asks for a photo.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Known {
    pub(crate) character: bool,
    mbid: Option<String>,
    wikidata: Option<String>,
    /// The Commons file, when the item was just read and checked.
    picture: Option<String>,
}

/// The photo sources after Deezer ([`photo_sources`]) and what they need.
#[derive(Clone, Default)]
pub(crate) struct After {
    /// MusicBrainz, Wikidata and the download of a service's image;
    /// `None`: only Deezer is asked, and no identity is settled.
    pub(crate) meta: Option<Metadata>,
    /// The services the user has connected.
    pub(crate) services: Vec<Arc<dyn StreamService>>,
    /// The sources that failed for now in this run.
    pub(crate) gate: Arc<Gate>,
}

/// Who `id` is, settled before any photo lookup: a role recorded before,
/// a recent identity, else a new one (recorded). `Err`: it cannot be
/// settled now, so no photo is looked up.
pub(crate) async fn known(after: &After, lib: &Library, id: ArtistId, name: &str, found: &impl Fn(Found)) -> Result<Known, Busy> {
    let Some(meta) = &after.meta else { return Ok(Known::default()) };
    let row = lib.artist_latin(id).ok().flatten();
    if row.as_ref().is_some_and(|r| r.character) {
        return Ok(Known { character: true, ..Known::default() });
    }
    if let Some(r) = row.filter(|r| r.identified_at > now_ms() - NAME_RETRY_MS) {
        return Ok(Known { character: false, mbid: r.mbid, wikidata: r.wikidata, picture: None });
    }
    let look =
        nikgob_metadata::in_background(identify(meta, &after.services, &after.gate, lib, id, name, false)).await?;
    record(lib, id, &look, found);
    let who = look.identity;
    Ok(Known { character: who.character, mbid: who.mbid, wikidata: who.wikidata, picture: who.picture })
}

/// The Commons file of the artist's picture: the one just checked, else
/// the Wikidata item read again and checked against the MusicBrainz id.
async fn commons_file(meta: &Metadata, gate: &Gate, known: &Known) -> Result<Option<String>, Busy> {
    if known.picture.is_some() {
        return Ok(known.picture.clone());
    }
    let (Some(q), Some(mbid)) = (&known.wikidata, &known.mbid) else { return Ok(None) };
    if !gate.open(WIKIMEDIA) {
        return Err(Busy(WIKIMEDIA));
    }
    match meta.wikidata.artist(q).await {
        Ok(item) => {
            gate.ok(WIKIMEDIA);
            Ok(if item_is(&item, mbid) { item.image } else { None })
        }
        Err(e) if busy_meta(&e) => Err(gate.failed(WIKIMEDIA)),
        Err(_) => Ok(None),
    }
}

/// A photo of `id` from the sources after Deezer: its bytes (a JPEG or a
/// PNG) and the source (`youtube`, `spotify`, `wikimedia`). `Err` when
/// none had one but some source could not answer for now: no miss.
pub(crate) async fn photo_after_deezer(
    after: &After,
    lib: &Library,
    id: ArtistId,
    name: &str,
    known: &Known,
) -> Result<Option<(Vec<u8>, &'static str)>, Busy> {
    let Some(meta) = after.meta.as_ref() else { return Ok(None) };
    if known.character {
        return Ok(None);
    }
    let gate = &after.gate;
    let titles = titles(lib, id);
    let mut names = vec![name.to_owned()];
    if let Some(l) = lib.artist_latin(id).ok().flatten().and_then(|l| l.name).filter(|l| l != name) {
        names.push(l);
    }
    let mut busy = None;
    let connected: Vec<Service> = after.services.iter().map(|s| s.service()).collect();
    for source in photo_sources(&connected) {
        match source {
            PhotoSource::Service(kind) => {
                let key = kind.as_str();
                let Some(svc) = after.services.iter().find(|s| s.service() == kind) else { continue };
                if !gate.open(key) {
                    busy = Some(Busy(key));
                    continue;
                }
                let url = match service_artist(&**svc, lib, id, &names, &titles).await {
                    Ok(hit) => {
                        gate.ok(key);
                        hit.and_then(|h| h.image)
                    }
                    Err(Busy(s)) => {
                        busy = Some(gate.failed(s));
                        None
                    }
                };
                let Some(url) = url else { continue };
                match meta.web_images.download(&url).await {
                    Ok(bytes) if crate::enrich::looks_like_image(&bytes) => return Ok(Some((bytes, key))),
                    Ok(_) => {}
                    Err(e) if busy_meta(&e) => busy = Some(Busy(key)),
                    Err(e) => log::debug!("{kind} photo of {name}: {e}"),
                }
            }
            PhotoSource::Wikimedia => {
                let file = match commons_file(meta, gate, known).await {
                    Ok(f) => f,
                    Err(b) => {
                        busy = Some(b);
                        None
                    }
                };
                let Some(file) = file else { continue };
                match meta.wikidata.download_image(&file).await {
                    Ok(bytes) if crate::enrich::looks_like_image(&bytes) => return Ok(Some((bytes, WIKIMEDIA))),
                    Ok(_) => {}
                    Err(e) if busy_meta(&e) => busy = Some(gate.failed(WIKIMEDIA)),
                    Err(e) => log::debug!("Commons photo of {name}: {e}"),
                }
            }
        }
    }
    busy.map_or(Ok(None), Err)
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
            "artist names: {} found, {} read in romaji, {} not found, {} failed for now, {} roles marked",
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
/// Stops after [`MAX_FAILURES`] artists in a row could not be looked up.
pub(crate) async fn run_names(
    meta: Metadata,
    services: Vec<Arc<dyn StreamService>>,
    lib: Library,
    pace: Pace,
    roles: usize,
    found: impl Fn(Found),
) {
    let gate = Gate::default();
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
            match nikgob_metadata::in_background(identify(&meta, &services, &gate, &lib, id, &name, true)).await {
                Ok(look) => {
                    failures = 0;
                    match &look.identity.latin {
                        Some((_, s)) if *s == ROMAJI => tally.romaji += 1,
                        Some(_) => tally.named += 1,
                        None => tally.none += 1,
                    }
                    record(&lib, id, &look, &found);
                }
                Err(Busy(source)) => {
                    log::debug!("who {name} is: {source} fails for now");
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
    use std::time::Duration;

    use nikgob_core::model::{Artist, ArtistCredit, Codec, Source, SourceKind, Track};
    use nikgob_metadata::musicbrainz::Alias;
    use nikgob_metadata::{Http, MetadataConfig, RateLimits, RetryPolicy};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const MBID: &str = "0b2f0b5a-0000-4000-8000-000000000001";
    const OTHER: &str = "0b2f0b5a-0000-4000-8000-000000000002";
    const NO_WAIT: Pace = Pace { start: Duration::ZERO, between: Duration::ZERO };

    fn mb(name: &str, sort: &str, aliases: Vec<Alias>) -> ArtistInfo {
        ArtistInfo { id: "x".into(), name: name.into(), sort_name: sort.into(), aliases, ..ArtistInfo::default() }
    }

    fn alias(name: &str, locale: &str) -> Alias {
        Alias { name: name.into(), locale: Some(locale.into()), kind: Some("Artist name".into()), primary: true, ..Alias::default() }
    }

    fn http() -> Http {
        Http::new(&MetadataConfig::new("tests@example.invalid"))
            .unwrap()
            .with_rate_limits(RateLimits::unlimited())
            .with_retry_policy(RetryPolicy { max_attempts: 1, base_delay: Duration::from_millis(1), max_delay: Duration::from_millis(1) })
    }

    fn meta(server: &MockServer) -> Metadata {
        let mut meta = Metadata::new(&MetadataConfig::new("tests@example.invalid")).unwrap();
        meta.musicbrainz = nikgob_metadata::MusicBrainz::with_base_url(http(), &server.uri()).unwrap();
        meta.wikidata =
            nikgob_metadata::Wikidata::with_base_urls(http(), &format!("{}/w/", server.uri()), &format!("{}/wiki/", server.uri()))
                .unwrap();
        meta
    }

    fn json(body: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_raw(body.as_bytes().to_vec(), "application/json")
    }

    /// An artist of the library with a file of each title.
    fn artist_with(lib: &Library, name: &str, titles: &[&str]) -> ArtistId {
        let a = lib.upsert_artist(&Artist::new(name)).unwrap();
        for t in titles {
            let t = lib.upsert_track(&Track::new(*t, ArtistCredit::single(name, Some(a.id)))).unwrap();
            lib.add_source(&Source::new(t.id, SourceKind::LocalFile, format!("/nonexistent/{}.flac", t.id), Codec::Flac)).unwrap();
        }
        a.id
    }

    fn hit(id: &str) -> String {
        format!(
            r#"{{"id": "{id}", "name": "佐宗綾子", "sort-name": "Saso, Ayako", "score": 100, "type": "Person",
                "aliases": [{{"name": "Ayako Saso", "locale": "en", "type": "Artist name", "primary": true}}]}}"#
        )
    }

    /// MusicBrainz knows the artists `ids` by name; each has the recordings `recordings`.
    async fn musicbrainz(ids: &[&str], recordings: &[&str]) -> MockServer {
        let server = MockServer::start().await;
        let hits: Vec<String> = ids.iter().map(|id| hit(id)).collect();
        Mock::given(method("GET"))
            .and(path("/artist"))
            .respond_with(json(&format!(r#"{{"artists": [{}]}}"#, hits.join(","))))
            .mount(&server)
            .await;
        for id in ids {
            Mock::given(method("GET")).and(path(format!("/artist/{id}"))).respond_with(json(&hit(id))).mount(&server).await;
        }
        let recs: Vec<String> = recordings.iter().enumerate().map(|(i, t)| format!(r#"{{"id": "r{i}", "title": "{t}"}}"#)).collect();
        Mock::given(method("GET"))
            .and(path("/recording"))
            .respond_with(json(&format!(r#"{{"recordings": [{}]}}"#, recs.join(","))))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/release-group"))
            .respond_with(json(r#"{"release-groups": [], "release-group-count": 0, "release-group-offset": 0}"#))
            .mount(&server)
            .await;
        server
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

    #[test]
    fn a_source_that_fails_for_now_sits_out_the_run() {
        let gate = Gate::default();
        assert!(gate.open("spotify"));
        assert_eq!(gate.failed("spotify"), Busy("spotify"));
        gate.ok("spotify");
        gate.failed("spotify");
        assert!(gate.open("spotify"), "one failure in a row");
        gate.failed("spotify");
        assert!(!gate.open("spotify"), "two in a row: out for the run");
        assert!(gate.open("musicbrainz"), "the others stay");
        assert!(busy_meta(&MetadataError::RateLimited(None)) && busy_meta(&MetadataError::Network("timeout".into())));
        assert!(busy_meta(&MetadataError::Status { status: 503, body: String::new() }));
        assert!(!busy_meta(&MetadataError::NotFound) && !busy_meta(&MetadataError::Status { status: 404, body: String::new() }));
        assert!(busy_stream(&StreamError::RateLimited { retry_after: None }) && busy_stream(&StreamError::Http { status: 502 }));
        assert!(!busy_stream(&StreamError::Unavailable));
    }

    /// A name search proves an artist only by two distinct titles of one
    /// candidate: one shared title, or two candidates that both have them,
    /// give no name and no MusicBrainz id.
    #[tokio::test]
    async fn one_shared_title_or_two_candidates_prove_nothing() {
        let lib = Library::open_in_memory().unwrap();
        let id = artist_with(&lib, "佐宗綾子", &["Hoshi no Uta", "Kaze Machi"]);
        let ask = |server: MockServer| {
            let lib = lib.clone();
            async move {
                let meta = meta(&server);
                identify(&meta, &[], &Gate::default(), &lib, id, "佐宗綾子", true).await.unwrap()
            }
        };
        let one = ask(musicbrainz(&[MBID], &["Hoshi no Uta"]).await).await;
        assert_eq!(one.identity, Identity::default(), "one title");
        assert!(one.complete);
        let two = ask(musicbrainz(&[MBID], &["Hoshi no Uta", "Kaze Machi"]).await).await;
        assert_eq!(two.identity.mbid.as_deref(), Some(MBID));
        assert_eq!(two.identity.latin, Some(("Ayako Saso".to_owned(), "musicbrainz")));
        let both = ask(musicbrainz(&[MBID, OTHER], &["Hoshi no Uta", "Kaze Machi"]).await).await;
        assert_eq!(both.identity, Identity::default(), "two candidates");
    }

    /// MusicBrainz answering 503: nothing is recorded (no miss), the artists
    /// stay due, and after two failures it is not asked again this run.
    #[tokio::test]
    async fn a_source_failing_for_now_records_no_miss() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(503)).mount(&server).await;
        let lib = Library::open_in_memory().unwrap();
        let ids = [
            artist_with(&lib, "佐宗綾子", &["Hoshi no Uta"]),
            artist_with(&lib, "井澤詩織", &["Kaze Machi"]),
            artist_with(&lib, "伊瀬茉莉也", &["Tsuki Akari"]),
        ];
        run_names(meta(&server), Vec::new(), lib.clone(), NO_WAIT, 0, |_| {}).await;
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
        for id in ids {
            assert_eq!(lib.artist_latin(id).unwrap(), None);
        }
        assert_eq!(lib.artists_needing_latin(now_ms(), 10).unwrap().len(), 3);
    }
}
