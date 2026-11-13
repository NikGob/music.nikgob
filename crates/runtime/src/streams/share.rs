//! "Поделиться": a page with every service for a track or an album, so the
//! link opens in whatever service the friend uses.
//!
//! The page is MusicLink's (`musiclink.one/song/…`, `/album/…`), asked
//! without a key; an album MusicLink cannot build on its server is asked
//! through its API when the user put a key into the settings, else it gets
//! album.link's page. A library item without stream ids is first looked up
//! (Spotify by ISRC, YouTube Music by name and length); a YouTube-only
//! album is found on Deezer, whose ids album.link understands. No page: the
//! service link, and the toast says why; nothing at all: "Исполнитель —
//! Название". The text goes to [`nikgob_view::ViewState::clipboard`]; the
//! app copies it.

use std::sync::Arc;

use nikgob_core::streaming::{EntityKind, RemoteId, Service, StreamService};
use nikgob_metadata::text::{album_key, artist_key};
use nikgob_metadata::{Deezer, Http, MetadataConfig, MetadataError, MusicLink, Odesli};
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

/// How to find the thing in a service when the library has no id for it.
#[derive(Clone)]
enum Lookup {
    Track(ResolveQuery),
    Album { artist: String, title: String },
}

/// Who did not answer, so the service link was copied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Failure {
    MusicLink,
    /// The user's MusicLink key was refused.
    MusicLinkKey,
    SongLink,
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
                    Some(Failure::SongLink) => format!("song.link не ответил — скопирована ссылка на {name}"),
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

/// Whether album.link can show one of the pages (it has no page for
/// YouTube Music albums).
fn has_songlink_page(pages: &[Page]) -> bool {
    pages.iter().any(|p| nikgob_metadata::odesli::page_url(&p.url).is_some())
}

/// Whether a Deezer album (its title, its artist) is the one asked for.
/// The credit asked may name more artists than Deezer's main one.
fn same_album(want_artist: &str, want_title: &str, artist: &str, title: &str) -> bool {
    let (want, got) = (artist_key(want_artist), artist_key(artist));
    album_key(title) == album_key(want_title) && !got.is_empty() && (want == got || want.contains(&got))
}

/// The Deezer page of an album, by its artist and title (Deezer's public
/// search, no key).
async fn deezer_album(deezer: &Deezer, artist: &str, title: &str) -> Option<Page> {
    let clean = |s: &str| s.replace('"', " ");
    let q = format!("artist:\"{}\" album:\"{}\"", clean(artist), clean(title));
    let hits = deezer
        .search_raw(&q, 10)
        .await
        .map_err(|e| log::warn!("share: Deezer search: {e}"))
        .ok()?;
    hits.iter()
        .filter_map(|t| {
            let album = t.album.as_ref()?;
            let by = album.artist.as_ref().map_or_else(|| t.artist_name(), |a| a.name.as_str());
            same_album(artist, title, by, &album.title).then_some(album.id)
        })
        .next()
        .map(|id| Page {
            service: Service::Deezer,
            url: format!("https://www.deezer.com/album/{id}"),
        })
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
        Lookup::Album { artist, title } => {
            for s in services {
                let Ok(r) = s
                    .search_kind(&format!("{artist} {title}"), nikgob_core::streaming::SearchKind::Albums)
                    .await
                else {
                    continue;
                };
                if let Some(a) = r.albums.iter().find(|a| {
                    album_key(&a.title) == album_key(title)
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

/// Asks the providers about the pages, best first: MusicLink without a key,
/// MusicLink's API with the user's key, song.link.
async fn find_page(clients: &Clients, key: Option<&str>, pages: &[Page]) -> Outcome {
    let Some(first) = pages.first() else {
        return Outcome::Nothing;
    };
    let mut musiclink_failed = false;
    for p in pages.iter().take(MUSICLINK_TRIES) {
        match clients.musiclink.page(&p.url).await {
            Ok(Some(url)) => return Outcome::Everywhere { url, key_refused: false },
            Ok(None) => {}
            Err(e) => {
                log::warn!("share: MusicLink: {e}");
                musiclink_failed = true;
                break;
            }
        }
    }
    let mut key_refused = false;
    if let Some(key) = key {
        match clients.musiclink.api_page(key, &first.url).await {
            Ok(Some(url)) => return Outcome::Everywhere { url, key_refused: false },
            Ok(None) => {}
            Err(MetadataError::Status { status: 401 | 403, .. }) => {
                log::warn!("share: the MusicLink key was refused");
                key_refused = true;
            }
            Err(e) => {
                log::warn!("share: MusicLink API: {e}");
                musiclink_failed = true;
            }
        }
    }
    let mut songlink_failed = false;
    for page in pages.iter().filter_map(|p| nikgob_metadata::odesli::page_url(&p.url)).take(2) {
        match clients.odesli.has_page(&page).await {
            Ok(true) => return Outcome::Everywhere { url: page, key_refused },
            Ok(false) => {}
            Err(e) => {
                log::warn!("share: song.link: {e}");
                songlink_failed = true;
                break;
            }
        }
    }
    let failed = if key_refused {
        Some(Failure::MusicLinkKey)
    } else if musiclink_failed {
        Some(Failure::MusicLink)
    } else if songlink_failed {
        Some(Failure::SongLink)
    } else {
        None
    };
    Outcome::Service {
        page: first.clone(),
        failed,
    }
}

impl Worker {
    /// The share clients, made on first use.
    fn share_clients(&mut self, settings: Option<&nikgob_core::settings::Settings>) -> Option<Clients> {
        if self.streaming.share_clients.is_none() {
            let contact = settings.map(|s| s.metadata.contact.clone()).unwrap_or_default();
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
        let services: Vec<Arc<dyn StreamService>> = Service::ALL
            .iter()
            .filter_map(|&s| self.service(s))
            .collect();
        // What to copy when nothing is found, the pages known, and how to
        // look the thing up otherwise.
        let (text, pages, lookup) = match &target {
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
                let album = self.streaming.pages.album(r).map(|a| {
                    (nikgob_core::streaming::join_names(&a.artists), a.title.clone())
                });
                let text = album.as_ref().map(|(a, t)| format!("{a} — {t}")).unwrap_or_default();
                let lookup = album.map(|(artist, title)| Lookup::Album { artist, title });
                (text, page_of(&services, EntityKind::Album, r).into_iter().collect(), lookup)
            }
            v::ShareTarget::Track(id) => {
                let Some(t) = self.lib.get_track(*id).ok().flatten() else { return };
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
                };
                (text, pages, Some(Lookup::Track(q)))
            }
            v::ShareTarget::Release(id) => {
                let Some(r) = self.lib.get_release(*id).ok().flatten() else { return };
                let artist = r.artist_credit.display();
                let text = format!("{artist} — {}", r.title);
                let pages = self.pages_of_local(&services, EntityKind::Album, *id.as_uuid());
                (
                    text,
                    pages,
                    Some(Lookup::Album {
                        artist,
                        title: r.title.clone(),
                    }),
                )
            }
        };
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
        let settings = self
            .settings_path
            .as_deref()
            .and_then(|p| nikgob_core::settings::Settings::load(p).ok());
        let key = settings
            .as_ref()
            .and_then(|s| s.metadata.musiclink_key.clone())
            .map(|k| k.trim().to_owned())
            .filter(|k| !k.is_empty());
        let clients = self.share_clients(settings.as_ref());
        self.streaming.spawn(async move {
            let mut pages = pages;
            match &lookup {
                // album.link has no page for a YouTube Music album: find it
                // on Deezer too.
                Some(Lookup::Album { artist, title }) if !has_songlink_page(&pages) => {
                    if let Some(c) = &clients
                        && let Some(p) = deezer_album(&c.deezer, artist, title).await
                    {
                        pages.push(p);
                    }
                }
                _ => {}
            }
            if pages.is_empty()
                && let Some(l) = &lookup
                && let Some(p) = look_up(&services, l).await
            {
                pages.push(p);
            }
            let pages = dedup(pages);
            let outcome = match &clients {
                Some(c) => find_page(c, key.as_deref(), &pages).await,
                None => match pages.first() {
                    Some(p) => Outcome::Service {
                        page: p.clone(),
                        failed: Some(Failure::MusicLink),
                    },
                    None => Outcome::Nothing,
                },
            };
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

    fn page(service: Service, url: &str) -> Page {
        Page {
            service,
            url: url.into(),
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
        assert!(service(Some(Failure::SongLink)).toast().starts_with("song.link не ответил"));
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
    fn a_deezer_album_matches_by_title_and_artist() {
        assert!(same_album("mzlff", "Ничего плохого", "Mzlff", "Ничего плохого"));
        assert!(same_album("mzlff, pyrokinesis", "оставь себе", "Mzlff", "оставь себе"));
        assert!(!same_album("mzlff", "Ничего плохого", "Mzlff", "ADHD"));
        assert!(!same_album("mzlff", "Ничего плохого", "CMH", "Ничего плохого"));
        assert!(!same_album("mzlff", "Ничего плохого", "", "Ничего плохого"));
    }
}
