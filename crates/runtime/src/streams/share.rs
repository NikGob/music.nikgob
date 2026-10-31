//! "Поделиться": a song.link / album.link page (Odesli) for a track or an
//! album, so the link opens in whatever service the friend uses. A library
//! track without stream ids is first looked up (Spotify by ISRC, YouTube
//! Music by name and length); nothing found → "Исполнитель — Название".
//! The text goes to [`nikgob_view::ViewState::clipboard`]; the app copies it.

use std::sync::Arc;

use nikgob_core::streaming::{EntityKind, RemoteId, Service, StreamService};
use nikgob_metadata::text::{album_key, artist_key};
use nikgob_metadata::{Http, MetadataConfig, Odesli};
use nikgob_view as v;

use super::Msg;
use super::resolve::{ResolveQuery, search_youtube};
use crate::Worker;

/// What a share came to.
pub(crate) struct Shared {
    /// The aggregator page, or the service page when Odesli did not answer.
    pub(crate) link: Option<String>,
    /// "Исполнитель — Название".
    pub(crate) text: String,
}

/// How to find the thing in a service when the library has no id for it.
#[derive(Clone)]
enum Lookup {
    Track(ResolveQuery),
    Album { artist: String, title: String },
}

/// The page of `remote` in its service.
fn page_of(services: &[Arc<dyn StreamService>], kind: EntityKind, remote: &RemoteId) -> Option<String> {
    services
        .iter()
        .find(|s| s.service() == remote.service)
        .map(|s| s.web_url(kind, &remote.id))
}

/// Looks `what` up: Spotify by ISRC (tracks), YouTube Music by name.
async fn look_up(services: &[Arc<dyn StreamService>], what: &Lookup) -> Option<String> {
    match what {
        Lookup::Track(q) => {
            if let Some(isrc) = &q.isrc
                && let Some(sp) = services.iter().find(|s| s.service() == Service::Spotify)
                && let Ok(Some(t)) = sp.find_isrc(isrc).await
            {
                return Some(sp.web_url(EntityKind::Track, &t.id.id));
            }
            let yt = services.iter().find(|s| s.service() == Service::Youtube)?;
            let hit = search_youtube(yt.as_ref(), q).await?;
            Some(yt.web_url(EntityKind::Track, &hit.id.id))
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
                    return Some(s.web_url(EntityKind::Album, &a.id.id));
                }
            }
            None
        }
    }
}

impl Worker {
    fn odesli(&mut self) -> Option<Odesli> {
        if self.streaming.odesli.is_none() {
            let contact = self
                .settings_path
                .as_deref()
                .and_then(|p| nikgob_core::settings::Settings::load(p).ok())
                .map(|s| s.metadata.contact)
                .unwrap_or_default();
            self.streaming.odesli = Http::new(&MetadataConfig::new(contact))
                .map(Odesli::new)
                .map_err(|e| log::warn!("Odesli: {e}"))
                .ok();
        }
        self.streaming.odesli.clone()
    }

    /// Points the share lookups at another Odesli (tests).
    pub(crate) fn set_odesli_base(&mut self, base: &str) {
        let http = Http::new(&MetadataConfig::new("tests"))
            .map(|h| h.with_rate_limits(nikgob_metadata::RateLimits::unlimited()));
        self.streaming.odesli = http.ok().and_then(|h| Odesli::with_base_url(h, base).ok());
    }

    pub(crate) fn share(&mut self, target: v::ShareTarget) {
        self.streaming.share_seq += 1;
        let seq = self.streaming.share_seq;
        let services: Vec<Arc<dyn StreamService>> = Service::ALL
            .iter()
            .filter_map(|&s| self.service(s))
            .collect();
        // What to copy when nothing is found, the page if an id is known,
        // and how to look it up otherwise.
        let (text, page, lookup) = match &target {
            v::ShareTarget::StreamTrack(r) => {
                let text = self
                    .streaming
                    .known
                    .get(r)
                    .map(|t| format!("{} — {}", t.artist_line(), t.title))
                    .unwrap_or_default();
                (text, page_of(&services, EntityKind::Track, r), None)
            }
            v::ShareTarget::StreamAlbum(r) => {
                let text = self
                    .streaming
                    .pages
                    .album(r)
                    .map(|a| format!("{} — {}", nikgob_core::streaming::join_names(&a.artists), a.title))
                    .unwrap_or_default();
                (text, page_of(&services, EntityKind::Album, r), None)
            }
            v::ShareTarget::Track(id) => {
                let Some(t) = self.lib.get_track(*id).ok().flatten() else { return };
                let artist = t.artist_credit.display();
                let text = format!("{artist} — {}", t.title);
                let page = self.page_of_local(&services, EntityKind::Track, *id.as_uuid());
                let q = ResolveQuery {
                    title: t.title.clone(),
                    artist,
                    duration_ms: t.duration_ms.map(u64::from),
                    isrc: t.isrc.clone(),
                    exclude: Vec::new(),
                    aliases: Vec::new(),
                };
                (text, page, Some(Lookup::Track(q)))
            }
            v::ShareTarget::Release(id) => {
                let Some(r) = self.lib.get_release(*id).ok().flatten() else { return };
                let artist = r.artist_credit.display();
                let text = format!("{artist} — {}", r.title);
                let page = self.page_of_local(&services, EntityKind::Album, *id.as_uuid());
                (
                    text,
                    page,
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
        let odesli = self.odesli();
        self.streaming.spawn(async move {
            let page = match (page, lookup) {
                (Some(p), _) => Some(p),
                (None, Some(l)) => look_up(&services, &l).await,
                (None, None) => None,
            };
            let link = match (&page, odesli) {
                (Some(p), Some(o)) => match o.page(p).await {
                    Ok(Some(found)) => Some(found.page_url),
                    Ok(None) => Some(p.clone()),
                    Err(e) => {
                        log::warn!("Odesli: {e}");
                        Some(p.clone())
                    }
                },
                (p, _) => p.clone(),
            };
            Msg::Shared {
                seq,
                target,
                result: Shared { link, text },
            }
        });
    }

    /// The service page of a library row with a known remote id.
    fn page_of_local(
        &self,
        services: &[Arc<dyn StreamService>],
        kind: EntityKind,
        local: nikgob_core::model::Uuid,
    ) -> Option<String> {
        let mut remotes = self.lib.external_ids(kind, local).unwrap_or_default();
        remotes.sort_by_key(|r| r.service.source_kind().priority());
        remotes.iter().find_map(|r| page_of(services, kind, r))
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
        let found = result.link.is_some();
        self.publish(|s| {
            s.share = Some(v::ShareView {
                target,
                busy: false,
                link: result.link,
                text: copied,
            });
            s.clipboard = Some(request);
        });
        self.toast(
            if found {
                "Ссылка скопирована"
            } else {
                "Нет в сервисах — скопировано название"
            },
            None,
        );
    }
}
