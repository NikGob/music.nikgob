//! Likes both ways (docs/stage3.md §0 "Лайки обратно").
//!
//! * Push: a like / unlike here (a track, or an album = its tracks) goes to
//!   every logged-in service the track has an id in, when
//!   [`nikgob_view::Prefs::push_likes`] is on.
//! * Pull: at start-up and every 15 minutes each logged-in service's liked
//!   list is compared with the one the last pull saw (`remote_like`). A new
//!   remote like becomes a like here (origin `sync`, so it downloads like a
//!   new like); a remote unlike takes the like back only if that like came
//!   from this service. Pulled likes are never pushed back, so nothing
//!   echoes.
//! * The very first pull of a service behaves like an import: its likes land
//!   in the library with wishlist items that wait (`import`, "ждёт") for the
//!   user's "скачать" instead of starting hundreds of downloads. Part B's
//!   API import can run later; it finds these likes known and adds nothing.

use nikgob_core::model::{TrackId, WishlistOrigin};
use nikgob_core::streaming::{RemoteId, Service, StreamError, StreamTrack};
use nikgob_library::RemoteLike;

use super::Msg;
use crate::Worker;

impl Worker {
    pub(crate) fn pull_all_likes(&mut self) {
        self.streaming.last_pull = Some(std::time::Instant::now());
        for &s in Service::ALL {
            self.pull_likes(s);
        }
    }

    pub(crate) fn pull_likes(&mut self, service: Service) {
        let Some(svc) = self.service(service) else { return };
        if !svc.account().is_logged_in() || !self.streaming.pulling.insert(service) {
            return;
        }
        self.streaming
            .spawn(async move { Msg::LikesPulled(service, svc.liked().await) });
    }

    pub(crate) fn likes_pulled(&mut self, service: Service, r: Result<Vec<StreamTrack>, StreamError>) {
        self.streaming.pulling.remove(&service);
        let liked = match r {
            Ok(l) => l,
            Err(e) => {
                log::warn!("{service}: pulling likes: {e}");
                return;
            }
        };
        let first = !self.lib.likes_pulled(service).unwrap_or(true);
        let known = self.lib.remote_likes(service).unwrap_or_default();
        let now = nikgob_library::now_ms();
        let origin = if first {
            WishlistOrigin::Import
        } else {
            WishlistOrigin::Sync
        };
        let mut changed = 0usize;
        // Oldest first, so "Любимые" keeps the service's order (newest on top).
        for t in liked.iter().rev() {
            if known.iter().any(|k| k.remote == t.id) {
                continue;
            }
            let Some(track) = self.lib.upsert_stream_track(t).ok() else {
                continue;
            };
            let liked_here = self.lib.is_liked(track).unwrap_or(false);
            if !liked_here {
                if self.lib.like(track).is_err() {
                    continue;
                }
                changed += 1;
                self.wish_for_like(track, origin);
            }
            let _ = self.lib.put_remote_like(&RemoteLike {
                remote: t.id.clone(),
                track_id: track,
                pulled: !liked_here,
                seen_at: now,
            });
        }
        // Gone from the service: the like goes only if it came from there.
        for k in &known {
            if liked.iter().any(|t| t.id == k.remote) {
                continue;
            }
            let _ = self.lib.delete_remote_like(&k.remote);
            if k.pulled && self.lib.is_liked(k.track_id).unwrap_or(false) {
                let _ = self.lib.unlike(k.track_id);
                self.wish_drop_unstarted(k.track_id);
                changed += 1;
            }
        }
        if first {
            let _ = self.lib.set_likes_pulled(service, now);
        }
        log::info!("{service}: {} liked, {changed} changes here", liked.len());
        if changed > 0 {
            self.after_like_change();
            self.publish_wishlist();
            self.wish_tick();
        }
    }

    /// Pushes a like / unlike of a library track to its services.
    pub(crate) fn push_like(&mut self, track: TrackId, on: bool) {
        if !self.state.read().prefs.push_likes {
            return;
        }
        let remotes = self
            .lib
            .external_ids(nikgob_core::streaming::EntityKind::Track, *track.as_uuid())
            .unwrap_or_default();
        let now = nikgob_library::now_ms();
        let mut pushed = Vec::new();
        for remote in remotes {
            // One id per service (a track may have collected several).
            if pushed.contains(&remote.service) {
                continue;
            }
            let Some(svc) = self.service(remote.service) else { continue };
            if !svc.account().is_logged_in() {
                continue;
            }
            pushed.push(remote.service);
            // Recorded now, so the next pull sees it as known, not as news.
            if on {
                let _ = self.lib.put_remote_like(&RemoteLike {
                    remote: remote.clone(),
                    track_id: track,
                    pulled: false,
                    seen_at: now,
                });
            } else {
                let _ = self.lib.delete_remote_like(&remote);
            }
            let id = remote.id.clone();
            self.streaming.spawn(async move {
                let r = svc.set_liked(&id, on).await;
                Msg::LikePushed(remote, on, r)
            });
        }
    }

    pub(crate) fn like_pushed(&mut self, remote: &RemoteId, on: bool, r: Result<(), StreamError>) {
        if let Err(e) = r {
            log::warn!("{remote}: set_liked({on}): {e}");
            if on {
                let _ = self.lib.delete_remote_like(remote);
            }
            self.toast(
                format!(
                    "Не получилось {} лайк в {}",
                    if on { "поставить" } else { "снять" },
                    remote.service.display_name()
                ),
                None,
            );
        }
    }
}
