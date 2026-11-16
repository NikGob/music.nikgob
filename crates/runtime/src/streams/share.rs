//! "Поделиться": a page with every service for a track or an album, so the
//! link opens in whatever service the friend uses.
//!
//! The page is MusicLink's (`musiclink.one/song/…`, `/album/…`), asked
//! without a key and then through its API when the user put a key into the
//! settings; else album.link's / song.link's page. A library item without
//! stream ids is first looked up (Spotify by ISRC, YouTube Music by name
//! and length). An album known only on YouTube Music (album.link has no page
//! for those) is looked for on Deezer when Deezer is on, and only taken when
//! exactly one Deezer album has the same title and artist: a wrong page is
//! worse than the service link. No page: the service link, and the toast
//! says why; nothing at all: "Исполнитель — Название". The text goes to
//! [`nikgob_view::ViewState::clipboard`]; the app copies it.

use std::path::PathBuf;
use std::sync::Arc;

use nikgob_core::streaming::{EntityKind, RemoteId, Service, StreamService};
use nikgob_metadata::text::{album_key, artist_key};
use nikgob_metadata::{Deezer, DeezerAlbum, Http, MetadataConfig, MetadataError, MusicLink, Odesli};
use nikgob_view as v;

use super::Msg;
use super::resolve::{ResolveQuery, search_youtube};
use crate::Worker;

/// The clients a share asks, made on the first share.
#[derive(Clone)]
pub(crate) struct Clients {
    musiclink: MusicLink,
    odesli: Odesli,
    deezer: Deezer,
}

impl Clients {
    fn new(http: Http) -> Self {
        Self {
            musiclink: MusicLink::new(http.clone()),
            odesli: Odesli::new(http.clone()),
            deezer: Deezer::new(http),
        }
    }

    /// Every client asks `base` (tests).
    fn at(http: Http, base: &str) -> Option<Self> {
        Some(Self {
            musiclink: MusicLink::with_base_url(http.clone(), base).ok()?,
            odesli: Odesli::with_base_url(http.clone(), base).ok()?,
            deezer: Deezer::with_base_url(http, base).ok()?,
        })
    }
}

/// What a share came to.
pub(crate) struct Shared {
    /// The page to copy: one with every service, or the service's own.
    pub(crate) link: Option<String>,
    /// "Исполнитель — Название".
    pub(crate) text: String,
    /// What the toast says.
    pub(crate) toast: String,
}

/// A service page a share can start from.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Page {
    service: Service,
    url: String,
}

/// An album as the share knows it.
#[derive(Clone, Debug, Default)]
struct AlbumInfo {
    /// The credited artists, each on its own.
    artists: Vec<String>,
    title: String,
    tracks: Option<u32>,
}

impl AlbumInfo {
    fn text(&self) -> String {
        format!("{} — {}", self.artists.join(", "), self.title)
    }
}

/// How to find the thing in a service when the library has no id for it.
#[derive(Clone)]
enum Lookup {
    Track(ResolveQuery),
    Album(AlbumInfo),
}

/// Who did not answer, so the service link was copied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Failure {
    MusicLink,
    /// The user's MusicLink key was refused.
    MusicLinkKey,
    Deezer,
    SongLink,
    /// The album's own service did not give its title and artist.
    Album(Service),
}

/// What the providers came to.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Outcome {
    /// A page with every service; `key_refused`: the MusicLink key did not
    /// work on the way (worth saying even though a page was found).
    Everywhere { url: String, key_refused: bool },
    /// The service page: no provider has the item (`failed: None`), or one
    /// did not answer.
    Service { page: Page, failed: Option<Failure> },
    /// No page at all.
    Nothing,
}

impl Outcome {
    fn link(&self) -> Option<String> {
        match self {
            Self::Everywhere { url, .. } => Some(url.clone()),
            Self::Service { page, .. } => Some(page.url.clone()),
            Self::Nothing => None,
        }
    }

    /// The toast after the copy.
    fn toast(&self) -> String {
        match self {
            Self::Everywhere { key_refused: false, .. } => "Ссылка скопирована".into(),
            Self::Everywhere { key_refused: true, .. } => "Ключ MusicLink не подошёл — скопирована ссылка song.link".into(),
            Self::Service { page, failed } => {
                let name = page.service.display_name();
                match failed {
                    None => format!("Скопирована ссылка на {name}"),
                    Some(Failure::MusicLink) => format!("MusicLink не ответил — скопирована ссылка на {name}"),
                    Some(Failure::MusicLinkKey) => format!("Ключ MusicLink не подошёл — скопирована ссылка на {name}"),
                    Some(Failure::Deezer) => format!("Deezer не ответил — скопирована ссылка на {name}"),
                    Some(Failure::SongLink) => format!("song.link не ответил — скопирована ссылка на {name}"),
                    Some(Failure::Album(s)) => format!("{} не ответил — скопирована ссылка на {name}", s.display_name()),
                }
            }
            Self::Nothing => "Нет в сервисах — скопировано название".into(),
        }
    }
}

/// The page of `remote` in its service.
fn page_of(services: &[Arc<dyn StreamService>], kind: EntityKind, remote: &RemoteId) -> Option<Page> {
    services.iter().find(|s| s.service() == remote.service).map(|s| Page {
        service: remote.service,
        url: s.web_url(kind, &remote.id),
    })
}

/// Pages in order, each once.
fn dedup(pages: Vec<Page>) -> Vec<Page> {
    let mut out: Vec<Page> = Vec::with_capacity(pages.len());
    for p in pages {
        if !out.iter().any(|o| o.url == p.url) {
            out.push(p);
        }
    }
    out
}

/// Whether album.link / song.link has a page for one of the pages (none
/// for YouTube Music albums).
fn has_songlink_page(pages: &[Page]) -> bool {
    pages.iter().any(|p| nikgob_metadata::odesli::page_url(&p.url).is_some())
}

/// A title as it reads, edition words included: lower case, letters and
/// digits, single spaces («Discovery (Live)» stays apart from «Discovery»).
fn exact_title(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars().flat_map(char::to_lowercase) {
        let c = if c == 'ё' { 'е' } else { c };
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with(' ') && !out.is_empty() {
            out.push(' ');
        }
    }
    out.trim_end().to_owned()
}

/// Whether a Deezer album is the one asked for: the same title as it reads
/// (edition included), Deezer's artist is one of the credited artists (by
/// name, not by part of a name), and the track count agrees when both know it.
fn same_album(want: &AlbumInfo, got: &DeezerAlbum) -> bool {
    let Some(artist) = got.artist.as_ref().map(|a| artist_key(&a.name)).filter(|a| !a.is_empty()) else {
        return false;
    };
    let title = exact_title(&got.title);
    !title.is_empty()
        && title == exact_title(&want.title)
        && want.artists.iter().any(|a| artist_key(a) == artist)
        && match (want.tracks, got.nb_tracks) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        }
}

/// The one Deezer album among `hits` that is `want`; none when no album or
/// more than one fits.
fn the_album(want: &AlbumInfo, hits: &[DeezerAlbum]) -> Option<u64> {
    let mut ids: Vec<u64> = hits.iter().filter(|a| same_album(want, a)).map(|a| a.id).collect();
    ids.sort_unstable();
    ids.dedup();
    match ids.as_slice() {
        [one] => Some(*one),
        _ => None,
    }
}

/// The Deezer page of an album, by its artist and title (Deezer's public
/// album search, no key).
async fn deezer_album(deezer: &Deezer, want: &AlbumInfo) -> Result<Option<Page>, MetadataError> {
    let artist = want.artists.first().map_or("", String::as_str);
    let hits = deezer.search_album(&format!("{artist} {}", want.title), 25).await?;
    Ok(the_album(want, &hits).map(|id| Page {
        service: Service::Deezer,
        url: format!("https://www.deezer.com/album/{id}"),
    }))
}

/// Looks `what` up: Spotify by ISRC (tracks), YouTube Music by name.
async fn look_up(services: &[Arc<dyn StreamService>], what: &Lookup) -> Option<Page> {
    match what {
        Lookup::Track(q) => {
            if let Some(isrc) = &q.isrc
                && let Some(sp) = services.iter().find(|s| s.service() == Service::Spotify)
                && let Ok(Some(t)) = sp.find_isrc(isrc).await
            {
                return Some(Page {
                    service: Service::Spotify,
                    url: sp.web_url(EntityKind::Track, &t.id.id),
                });
            }
            let yt = services.iter().find(|s| s.service() == Service::Youtube)?;
            let hit = search_youtube(yt.as_ref(), q).await?;
            Some(Page {
                service: Service::Youtube,
                url: yt.web_url(EntityKind::Track, &hit.id.id),
            })
        }
        Lookup::Album(info) => {
            let artist = info.artists.first().map_or("", String::as_str);
            for s in services {
                let Ok(r) = s
                    .search_kind(&format!("{artist} {}", info.title), nikgob_core::streaming::SearchKind::Albums)
                    .await
                else {
                    continue;
                };
                if let Some(a) = r.albums.iter().find(|a| {
                    album_key(&a.title) == album_key(&info.title)
                        && a.artists.iter().any(|ar| artist_key(&ar.name) == artist_key(artist))
                }) {
                    return Some(Page {
                        service: s.service(),
                        url: s.web_url(EntityKind::Album, &a.id.id),
                    });
                }
            }
            None
        }
    }
}

/// How many service pages MusicLink is asked about, at most.
const MUSICLINK_TRIES: usize = 2;

/// Everything a share needs off the player loop.
struct Job {
    clients: Option<Clients>,
    services: Vec<Arc<dyn StreamService>>,
    settings_path: Option<PathBuf>,
    pages: Vec<Page>,
    lookup: Option<Lookup>,
    /// A service album whose title and artist are not known yet (shared
    /// from a card before its page was opened).
    album_to_fetch: Option<RemoteId>,
    /// Whether Deezer may be asked (it is switched on).
    deezer_on: bool,
    text: String,
}

/// Who failed on the way.
#[derive(Default)]
struct Trouble {
    musiclink: bool,
    key_refused: bool,
    deezer: bool,
    songlink: bool,
    album: Option<Service>,
}

impl Trouble {
    fn failure(&self) -> Option<Failure> {
        if self.key_refused {
            Some(Failure::MusicLinkKey)
        } else if self.musiclink {
            Some(Failure::MusicLink)
        } else if self.deezer {
            Some(Failure::Deezer)
        } else if self.songlink {
            Some(Failure::SongLink)
        } else {
            self.album.map(Failure::Album)
        }
    }
}

/// MusicLink without a key on up to [`MUSICLINK_TRIES`] of `pages`.
async fn musiclink_keyless(clients: &Clients, pages: &[Page], trouble: &mut Trouble) -> Option<String> {
    for p in pages.iter().take(MUSICLINK_TRIES) {
        match clients.musiclink.page(&p.url).await {
            Ok(Some(url)) => return Some(url),
            Ok(None) => {}
            Err(e) => {
                log::warn!("share: MusicLink: {e}");
                trouble.musiclink = true;
                return None;
            }
        }
    }
    None
}

/// Runs a share: the page to copy and the text.
async fn run(job: Job) -> (Outcome, String) {
    let Job {
        clients,
        services,
        settings_path,
        mut pages,
        mut lookup,
        album_to_fetch,
        deezer_on,
        mut text,
    } = job;
    let mut trouble = Trouble::default();
    if pages.is_empty()
        && let Some(l) = &lookup
        && let Some(p) = look_up(&services, l).await
    {
        pages.push(p);
    }
    let pages_known = dedup(pages);
    let Some(first) = pages_known.first().cloned() else {
        return (Outcome::Nothing, text);
    };
    let Some(clients) = clients else {
        let failed = Some(Failure::MusicLink);
        return (Outcome::Service { page: first, failed }, text);
    };
    // 1. MusicLink, keyless, then with the user's key.
    if let Some(url) = musiclink_keyless(&clients, &pages_known, &mut trouble).await {
        return (Outcome::Everywhere { url, key_refused: false }, text);
    }
    let key = settings_path
        .as_deref()
        .and_then(|p| nikgob_core::settings::Settings::load(p).ok())
        .and_then(|s| s.metadata.musiclink_key)
        .map(|k| k.trim().to_owned())
        .filter(|k| !k.is_empty());
    if let Some(key) = &key {
        match clients.musiclink.api_page(key, &first.url).await {
            Ok(Some(url)) => return (Outcome::Everywhere { url, key_refused: false }, text),
            Ok(None) => {}
            Err(MetadataError::Status { status: 401 | 403, .. }) => {
                log::warn!("share: the MusicLink key was refused");
                trouble.key_refused = true;
            }
            Err(e) => {
                log::warn!("share: MusicLink API: {e}");
                trouble.musiclink = true;
            }
        }
    }
    // 2. An album album.link cannot show (YouTube Music): the same album
    // on Deezer, when Deezer is on and exactly one album fits.
    let mut pages = pages_known.clone();
    let is_album = album_to_fetch.is_some() || matches!(lookup, Some(Lookup::Album(_)));
    if is_album && deezer_on && !has_songlink_page(&pages) {
        if !matches!(lookup, Some(Lookup::Album(_)))
            && let Some(r) = &album_to_fetch
            && let Some(s) = services.iter().find(|s| s.service() == r.service)
        {
            match s.album(&r.id).await {
                Ok(a) => {
                    let info = AlbumInfo {
                        artists: a.artists.iter().map(|ar| ar.name.clone()).collect(),
                        title: a.title.clone(),
                        tracks: a.total_tracks,
                    };
                    if text.is_empty() {
                        text = info.text();
                    }
                    lookup = Some(Lookup::Album(info));
                }
                Err(e) => {
                    log::warn!("share: the album from {}: {e}", r.service.as_str());
                    trouble.album = Some(r.service);
                }
            }
        }
        if let Some(Lookup::Album(info)) = &lookup {
            match deezer_album(&clients.deezer, info).await {
                Ok(Some(p)) => {
                    if let Some(url) = musiclink_keyless(&clients, std::slice::from_ref(&p), &mut trouble).await {
                        return (Outcome::Everywhere { url, key_refused: trouble.key_refused }, text);
                    }
                    pages.push(p);
                }
                Ok(None) => {}
                Err(e) => {
                    log::warn!("share: Deezer: {e}");
                    trouble.deezer = true;
                }
            }
        }
    }
    // 3. song.link / album.link.
    for page in pages.iter().filter_map(|p| nikgob_metadata::odesli::page_url(&p.url)).take(2) {
        match clients.odesli.has_page(&page).await {
            Ok(true) => return (Outcome::Everywhere { url: page, key_refused: trouble.key_refused }, text),
            Ok(false) => {}
            Err(e) => {
                log::warn!("share: song.link: {e}");
                trouble.songlink = true;
                break;
            }
        }
    }
    let failed = trouble.failure();
    (Outcome::Service { page: first, failed }, text)
}

impl Worker {
    /// The share clients, made on first use.
    fn share_clients(&mut self) -> Option<Clients> {
        if self.streaming.share_clients.is_none() {
            let contact = self.state.read().prefs.metadata_contact.clone();
            self.streaming.share_clients = Http::new(&MetadataConfig::new(contact))
                .map(Clients::new)
                .map_err(|e| log::warn!("share: {e}"))
                .ok();
        }
        self.streaming.share_clients.clone()
    }

    /// Points the share lookups at another server (tests).
    pub(crate) fn set_share_base(&mut self, base: &str) {
        let http = Http::new(&MetadataConfig::new("tests"))
            .map(|h| h.with_rate_limits(nikgob_metadata::RateLimits::unlimited()));
        self.streaming.share_clients = http.ok().and_then(|h| Clients::at(h, base));
    }

    pub(crate) fn share(&mut self, target: v::ShareTarget) {
        self.streaming.share_seq += 1;
        let seq = self.streaming.share_seq;
        let Some(job) = self.share_job(&target) else { return };
        let text = job.text.clone();
        self.publish(|s| {
            s.share = Some(v::ShareView {
                target: target.clone(),
                busy: true,
                link: None,
                text: text.clone(),
            });
        });
        // The lookup can take a few seconds: say it started (the result
        // replaces this toast).
        self.drop_share_toast();
        self.toast("Ищу ссылку…", None);
        self.streaming.share_toast = Some(self.toast_id);
        self.streaming.spawn(async move {
            let (outcome, text) = run(job).await;
            Msg::Shared {
                seq,
                target,
                result: Shared {
                    link: outcome.link(),
                    text,
                    toast: outcome.toast(),
                },
            }
        });
    }

    /// The share of the local API: the same lookup, its page and text
    /// answered to `reply` as `{"link", "text", "everywhere"}`, with no
    /// toast and nothing copied.
    pub(crate) fn share_for_api(&mut self, target: v::ShareTarget, reply: v::ApiReply) {
        let Some(job) = self.share_job(&target) else {
            reply.send(Err("Ничего не нашлось".to_owned()));
            return;
        };
        self.streaming.spawn_reporting(move |_| async move {
            let (outcome, text) = run(job).await;
            let everywhere = matches!(outcome, Outcome::Everywhere { .. });
            reply.send(Ok(serde_json::json!({ "link": outcome.link(), "text": text, "everywhere": everywhere })));
        });
    }

    /// What a share of `target` looks up; `None` for a library item that is
    /// gone.
    fn share_job(&mut self, target: &v::ShareTarget) -> Option<Job> {
        let services: Vec<Arc<dyn StreamService>> = Service::ALL
            .iter()
            .filter_map(|&s| self.service(s))
            .collect();
        // What to copy when nothing is found, the pages known, and how to
        // look the thing up otherwise.
        let mut album_to_fetch = None;
        let (text, pages, lookup) = match target {
            v::ShareTarget::StreamTrack(r) => {
                let text = self
                    .streaming
                    .known
                    .get(r)
                    .map(|t| format!("{} — {}", t.artist_line(), t.title))
                    .unwrap_or_default();
                (text, page_of(&services, EntityKind::Track, r).into_iter().collect(), None)
            }
            v::ShareTarget::StreamAlbum(r) => {
                let info = self.streaming.pages.album(r).map(|a| AlbumInfo {
                    artists: a.artists.iter().map(|ar| ar.name.clone()).collect(),
                    title: a.title.clone(),
                    tracks: a.total_tracks,
                });
                if info.is_none() {
                    album_to_fetch = Some(r.clone());
                }
                let text = info.as_ref().map(AlbumInfo::text).unwrap_or_default();
                (text, page_of(&services, EntityKind::Album, r).into_iter().collect(), info.map(Lookup::Album))
            }
            v::ShareTarget::Track(id) => {
                let t = self.lib.get_track(*id).ok().flatten()?;
                let artist = t.artist_credit.display();
                let text = format!("{artist} — {}", t.title);
                let pages = self.pages_of_local(&services, EntityKind::Track, *id.as_uuid());
                let q = ResolveQuery {
                    title: t.title.clone(),
                    artist,
                    duration_ms: t.duration_ms.map(u64::from),
                    isrc: t.isrc.clone(),
                    exclude: Vec::new(),
                    aliases: Vec::new(),
                    album: None,
                };
                (text, pages, Some(Lookup::Track(q)))
            }
            v::ShareTarget::Release(id) => {
                let r = self.lib.get_release(*id).ok().flatten()?;
                let text = format!("{} — {}", r.artist_credit.display(), r.title);
                let pages = self.pages_of_local(&services, EntityKind::Album, *id.as_uuid());
                let info = AlbumInfo {
                    artists: r.artist_credit.0.iter().map(|p| p.name.clone()).collect(),
                    title: r.title.clone(),
                    tracks: None,
                };
                (text, pages, Some(Lookup::Album(info)))
            }
        };
        Some(Job {
            clients: self.share_clients(),
            services,
            // Read in the task: the key may change while the player runs.
            settings_path: self.settings_path.clone(),
            pages,
            lookup,
            album_to_fetch,
            deezer_on: self.state.read().prefs.service_enabled(Service::Deezer),
            text,
        })
    }

    /// The service pages of a library row with known remote ids, the
    /// preferred service first.
    fn pages_of_local(
        &self,
        services: &[Arc<dyn StreamService>],
        kind: EntityKind,
        local: nikgob_core::model::Uuid,
    ) -> Vec<Page> {
        let mut remotes = self.lib.external_ids(kind, local).unwrap_or_default();
        remotes.sort_by_key(|r| r.service.source_kind().priority());
        remotes.iter().filter_map(|r| page_of(services, kind, r)).collect()
    }

    /// Takes away the «Ищу ссылку…» toast, if it is still there.
    fn drop_share_toast(&mut self) {
        if let Some(id) = self.streaming.share_toast.take() {
            self.publish(|s| s.toasts.retain(|t| t.id != id));
        }
    }

    pub(crate) fn shared(&mut self, seq: u64, target: v::ShareTarget, result: Shared) {
        if seq != self.streaming.share_seq {
            return;
        }
        self.drop_share_toast();
        let copied = result.link.clone().unwrap_or_else(|| result.text.clone());
        if copied.is_empty() {
            self.publish(|s| s.share = None);
            self.toast("Нечем поделиться", None);
            return;
        }
        self.streaming.clipboard_id += 1;
        let request = v::ClipboardRequest {
            id: self.streaming.clipboard_id,
            text: copied.clone(),
        };
        self.publish(|s| {
            s.share = Some(v::ShareView {
                target,
                busy: false,
                link: result.link,
                text: copied,
            });
            s.clipboard = Some(request);
        });
        self.toast(result.toast, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_core::streaming::FakeService;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn page(service: Service, url: &str) -> Page {
        Page {
            service,
            url: url.into(),
        }
    }

    fn info(artists: &[&str], title: &str, tracks: Option<u32>) -> AlbumInfo {
        AlbumInfo {
            artists: artists.iter().map(|a| (*a).to_owned()).collect(),
            title: title.into(),
            tracks,
        }
    }

    fn deezer_album(id: u64, artist: &str, title: &str, tracks: Option<u32>) -> DeezerAlbum {
        DeezerAlbum {
            id,
            title: title.into(),
            nb_tracks: tracks,
            artist: Some(nikgob_metadata::deezer::DeezerArtist {
                name: artist.into(),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn toasts_say_what_was_copied() {
        let yt = page(Service::Youtube, "https://music.youtube.com/watch?v=x");
        let everywhere = |key_refused| Outcome::Everywhere {
            url: "https://musiclink.one/song/a/b".into(),
            key_refused,
        };
        assert_eq!(everywhere(false).toast(), "Ссылка скопирована");
        assert_eq!(everywhere(false).link().as_deref(), Some("https://musiclink.one/song/a/b"));
        assert!(everywhere(true).toast().starts_with("Ключ MusicLink не подошёл"));
        let service = |failed| Outcome::Service { page: yt.clone(), failed };
        assert_eq!(service(None).toast(), "Скопирована ссылка на YouTube Music");
        assert_eq!(service(None).link().as_deref(), Some("https://music.youtube.com/watch?v=x"));
        assert_eq!(
            service(Some(Failure::MusicLink)).toast(),
            "MusicLink не ответил — скопирована ссылка на YouTube Music"
        );
        assert!(service(Some(Failure::MusicLinkKey)).toast().starts_with("Ключ MusicLink не подошёл"));
        assert!(service(Some(Failure::Deezer)).toast().starts_with("Deezer не ответил"));
        assert!(service(Some(Failure::SongLink)).toast().starts_with("song.link не ответил"));
        assert!(service(Some(Failure::Album(Service::Youtube))).toast().starts_with("YouTube Music не ответил"));
        assert_eq!(Outcome::Nothing.link(), None);
        assert_eq!(Outcome::Nothing.toast(), "Нет в сервисах — скопировано название");
    }

    #[test]
    fn pages_come_once_and_youtube_albums_need_another_service() {
        let yt = page(Service::Youtube, "https://music.youtube.com/browse/MPREb_x");
        let dz = page(Service::Deezer, "https://www.deezer.com/album/1");
        assert_eq!(dedup(vec![yt.clone(), dz.clone(), yt.clone()]), vec![yt.clone(), dz.clone()]);
        assert!(!has_songlink_page(&[yt.clone()]));
        assert!(has_songlink_page(&[yt, dz]));
        assert!(has_songlink_page(&[page(Service::Youtube, "https://music.youtube.com/watch?v=x")]));
    }

    #[test]
    fn only_the_same_album_of_the_same_artist_is_taken() {
        let want = info(&["Air Supply"], "Lost in Love", Some(10));
        // «Air» is part of «Air Supply», not the artist.
        assert!(!same_album(&want, &deezer_album(1, "Air", "Lost in Love", Some(10))));
        assert!(same_album(&want, &deezer_album(2, "Air Supply", "Lost in Love", Some(10))));
        assert!(same_album(&want, &deezer_album(2, "air supply", "LOST IN LOVE", None)));
        // Another edition, another length.
        assert!(!same_album(&want, &deezer_album(3, "Air Supply", "Lost in Love (Live)", Some(10))));
        assert!(!same_album(&want, &deezer_album(4, "Air Supply", "Lost in Love (Deluxe Edition)", Some(10))));
        assert!(!same_album(&want, &deezer_album(5, "Air Supply", "Lost in Love", Some(14))));
        // One of several credited artists.
        let duo = info(&["mzlff", "pyrokinesis"], "Ничего плохого", None);
        assert!(same_album(&duo, &deezer_album(6, "Mzlff", "Ничего плохого", Some(9))));
        assert!(!same_album(&duo, &deezer_album(7, "", "Ничего плохого", Some(9))));
        // Exactly one fits, or none is taken.
        let hits = [deezer_album(1, "Air", "Lost in Love", Some(10)), deezer_album(2, "Air Supply", "Lost in Love", Some(10))];
        assert_eq!(the_album(&want, &hits), Some(2));
        let twice = [deezer_album(2, "Air Supply", "Lost in Love", None), deezer_album(8, "Air Supply", "Lost in Love", None)];
        assert_eq!(the_album(&want, &twice), None);
        assert_eq!(the_album(&want, &[]), None);
        assert_eq!(exact_title("  Discovery  (Live) "), "discovery live");
        assert_eq!(exact_title("Ёлка"), exact_title("елка"));
    }

    /// The share of a YouTube Music album: `server` stands for MusicLink,
    /// song.link and Deezer.
    async fn share_youtube_album(server: &MockServer, deezer_on: bool, known: bool) -> (Outcome, String) {
        let http = Http::new(&MetadataConfig::new("tests"))
            .unwrap()
            .with_rate_limits(nikgob_metadata::RateLimits::unlimited())
            .with_retry_policy(nikgob_metadata::RetryPolicy {
                max_attempts: 1,
                ..Default::default()
            });
        let yt: Arc<dyn StreamService> = Arc::new(FakeService::youtube());
        let r = RemoteId::new(Service::Youtube, "yt-alb-gruppa");
        let lookup = known.then(|| Lookup::Album(info(&["Кино"], "Группа крови", Some(4))));
        let job = Job {
            clients: Clients::at(http, &server.uri()),
            services: vec![yt.clone()],
            settings_path: None,
            pages: vec![page_of(&[yt], EntityKind::Album, &r).unwrap()],
            album_to_fetch: (!known).then(|| r.clone()),
            lookup,
            deezer_on,
            text: String::new(),
        };
        run(job).await
    }

    fn deezer_answer(body: &'static str) -> Mock {
        Mock::given(method("GET"))
            .and(path("/search/album"))
            .and(query_param("q", "Кино Группа крови"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body.as_bytes().to_vec(), "application/json"))
    }

    const DEEZER_HITS: &str = r#"{"data":[
        {"id":77,"title":"Группа крови","nb_tracks":4,"artist":{"id":1,"name":"Кино"}},
        {"id":78,"title":"Группа крови (Live)","nb_tracks":4,"artist":{"id":1,"name":"Кино"}},
        {"id":79,"title":"Группа крови","nb_tracks":4,"artist":{"id":2,"name":"Кинофильм"}}]}"#;

    #[tokio::test]
    async fn an_unopened_youtube_album_reaches_album_link_through_deezer() {
        let server = MockServer::start().await;
        deezer_answer(DEEZER_HITS).expect(1).mount(&server).await;
        Mock::given(method("GET"))
            .and(path("/d/77"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"<meta property="og:title" content="Группа крови by Кино"/>"#))
            .mount(&server)
            .await;
        // MusicLink knows neither the YouTube nor the Deezer album (404 from
        // the mock = not built on its server).
        let (outcome, text) = share_youtube_album(&server, true, false).await;
        assert_eq!(
            outcome,
            Outcome::Everywhere {
                url: "https://album.link/d/77".into(),
                key_refused: false
            }
        );
        assert_eq!(text, "Кино — Группа крови", "the card's album is fetched for the text too");
    }

    #[tokio::test]
    async fn musiclink_goes_first_and_deezer_is_not_asked() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/resolve"))
            .and(query_param("q", "https://music.youtube.com/browse/yt-alb-gruppa"))
            .respond_with(ResponseTemplate::new(307).insert_header("location", "/album/kino/gruppa-krovi"))
            .mount(&server)
            .await;
        deezer_answer(DEEZER_HITS).expect(0).mount(&server).await;
        let (outcome, _) = share_youtube_album(&server, true, true).await;
        assert_eq!(outcome.link().as_deref(), Some("https://musiclink.one/album/kino/gruppa-krovi"));
    }

    #[tokio::test]
    async fn a_switched_off_deezer_is_not_asked() {
        let server = MockServer::start().await;
        deezer_answer(DEEZER_HITS).expect(0).mount(&server).await;
        let (outcome, _) = share_youtube_album(&server, false, true).await;
        assert_eq!(
            outcome,
            Outcome::Service {
                page: page(Service::Youtube, "https://music.youtube.com/browse/yt-alb-gruppa"),
                failed: None
            }
        );
    }

    #[tokio::test]
    async fn a_deezer_failure_is_said() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search/album"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let (outcome, _) = share_youtube_album(&server, true, true).await;
        assert!(matches!(outcome, Outcome::Service { failed: Some(Failure::Deezer), .. }), "{outcome:?}");
    }

    #[tokio::test]
    async fn an_ambiguous_deezer_answer_gives_the_service_link() {
        let server = MockServer::start().await;
        deezer_answer(
            r#"{"data":[{"id":77,"title":"Группа крови","artist":{"name":"Кино"}},
                {"id":80,"title":"Группа крови","artist":{"name":"Кино"}}]}"#,
        )
        .mount(&server)
        .await;
        let (outcome, _) = share_youtube_album(&server, true, true).await;
        assert!(matches!(outcome, Outcome::Service { failed: None, .. }), "{outcome:?}");
    }
}
