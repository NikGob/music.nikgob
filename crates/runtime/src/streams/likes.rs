//! Likes both ways (docs/stage3.md §0 "Лайки обратно").
//!
//! * Push: a like / unlike here (a track, or an album = its tracks) goes to
//!   every logged-in service the track has an id in, when
//!   [`nikgob_view::Prefs::push_likes`] is on. Pushes wait in a queue per
//!   service for [`StreamSchedule::like_delay`] and go out together (Spotify:
//!   40 per request; YouTube: one at a time); a like and its unlike within
//!   that time cancel out and send nothing.
//! * Pull: each logged-in service's liked list is compared with the one the
//!   last pull saw (`remote_like`): as soon as its login is up, then every
//!   [`StreamSchedule::likes_every`] (± jitter), and on window focus or a
//!   playback start when the last check is older than
//!   [`StreamSchedule::likes_fresh`]. A check reads only the first page
//!   and the list's length ([`StreamService::liked_head`]): when every
//!   track of it is known and the counts add up, nothing else is asked
//!   (one request); new likes at the top come from that page; anything else
//!   (an unlike, more than a page of news) and once a day read the whole
//!   list. A new remote like becomes a like here (origin `sync`, so it
//!   downloads like a new like); a remote unlike takes the like back only
//!   if that like came from this service. Pulled likes are never pushed
//!   back, so nothing echoes.
//! * The very first pull of a service behaves like an import: its likes land
//!   in the library with wishlist items that wait (`import`, "ждёт") for the
//!   user's "скачать" instead of starting hundreds of downloads. The API
//!   import ([`super::cold`]) goes through the same code, so whichever runs
//!   first takes the likes and the other finds them known.
//!
//! [`StreamSchedule::like_delay`]: super::StreamSchedule::like_delay
//! [`StreamSchedule::likes_every`]: super::StreamSchedule::likes_every
//! [`StreamSchedule::likes_fresh`]: super::StreamSchedule::likes_fresh
//! [`StreamService::liked_head`]: nikgob_core::streaming::StreamService::liked_head

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use nikgob_core::model::{TrackId, WishlistOrigin};
use nikgob_core::streaming::{LikedHead, RemoteId, Service, StreamError, StreamTrack};
use nikgob_library::RemoteLike;

use super::Msg;
use crate::Worker;

/// Library meta key: when the whole liked list of a service was last read.
fn full_key(service: Service) -> String {
    format!("likes_full.{service}")
}

/// Likes and unlikes waiting to be sent, per service.
#[derive(Default)]
pub(crate) struct LikeQueue {
    pending: HashMap<Service, Vec<(RemoteId, bool)>>,
    /// A flush is on its way.
    armed: HashSet<Service>,
}

impl LikeQueue {
    /// Queues `remote` → `on`; a pending opposite cancels out (nothing is
    /// sent). Returns whether the queue was empty (a flush must be armed).
    pub(crate) fn push(&mut self, remote: RemoteId, on: bool) -> bool {
        let q = self.pending.entry(remote.service).or_default();
        if let Some(i) = q.iter().position(|(r, _)| *r == remote) {
            if q[i].1 != on {
                q.remove(i);
            }
            return false;
        }
        q.push((remote.clone(), on));
        self.armed.insert(remote.service)
    }

    fn take(&mut self, service: Service) -> Vec<(RemoteId, bool)> {
        self.armed.remove(&service);
        self.pending.remove(&service).unwrap_or_default()
    }
}

/// What a head check says.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum HeadVerdict {
    /// Nothing new, nothing gone.
    Same,
    /// Only these (from the head, newest first) are new.
    New(Vec<StreamTrack>),
    /// Something else changed: read the whole list.
    Full,
}

/// Compares the head of a liked list with what the last pull saw.
pub(crate) fn judge_head(head: &LikedHead, known: &HashSet<String>) -> HeadVerdict {
    let new: Vec<StreamTrack> = head
        .tracks
        .iter()
        .filter(|t| !known.contains(&t.id.id))
        .cloned()
        .collect();
    let Some(total) = head.total else {
        return HeadVerdict::Full;
    };
    // The newest likes are on top: news below a known track, or a last
    // track that is news too (more news on the next page), is not a clean
    // addition.
    let first_known = head.tracks.iter().position(|t| known.contains(&t.id.id));
    let clean_top = match first_known {
        Some(i) => head.tracks[i..].iter().all(|t| known.contains(&t.id.id)),
        None => head.complete,
    };
    if total as usize != known.len() + new.len() || !clean_top {
        return HeadVerdict::Full;
    }
    if new.is_empty() {
        HeadVerdict::Same
    } else {
        HeadVerdict::New(new)
    }
}

impl Worker {
    /// From the tick: each logged-in service whose check is due (as soon as
    /// its login is up: the Spotify login is restored in the background).
    pub(crate) fn likes_tick(&mut self) {
        let now = Instant::now();
        for &s in Service::ALL {
            let due = self
                .streaming
                .likes
                .get(&s)
                .is_none_or(|l| !l.pulling && l.next_at.is_none_or(|at| now >= at));
            if due {
                self.pull_likes(s);
            }
        }
    }

    /// Window focus, a playback start: a check older than
    /// [`super::StreamSchedule::likes_fresh`] runs now.
    pub(crate) fn likes_if_stale(&mut self) {
        let fresh = self.streaming.schedule.likes_fresh;
        for &s in Service::ALL {
            let stale = self
                .streaming
                .likes
                .get(&s)
                .is_some_and(|l| !l.pulling && l.last.is_some_and(|t| t.elapsed() >= fresh));
            if stale {
                self.pull_likes(s);
            }
        }
    }

    /// Checks a service's likes now: its head, or the whole list when that
    /// is due (first pull, once a day).
    pub(crate) fn pull_likes(&mut self, service: Service) {
        let Some(svc) = self.service(service) else { return };
        if !svc.account().is_logged_in() {
            return;
        }
        let every = self.streaming.schedule.jittered(self.streaming.schedule.likes_every);
        let full_every = self.streaming.schedule.likes_full_every;
        let l = self.streaming.likes.entry(service).or_default();
        if l.pulling {
            return;
        }
        l.pulling = true;
        l.last = Some(Instant::now());
        l.next_at = Some(Instant::now() + every);
        let first = !self.lib.likes_pulled(service).unwrap_or(true);
        let now = nikgob_library::now_ms();
        let full_due = self
            .lib
            .cache_get(&full_key(service))
            .ok()
            .flatten()
            .is_none_or(|(at, _)| now - at >= full_every.as_millis() as i64);
        if first || full_due {
            log::debug!("{service}: reading the whole liked list");
            self.streaming
                .spawn(async move { Msg::LikesPulled(service, svc.liked().await) });
        } else {
            self.streaming
                .spawn(async move { Msg::LikesHead(service, svc.liked_head().await) });
        }
    }

    pub(crate) fn likes_head(&mut self, service: Service, r: Result<LikedHead, StreamError>) {
        let head = match r {
            Ok(h) => h,
            Err(e) => {
                log::warn!("{service}: checking likes: {e}");
                self.streaming.likes.entry(service).or_default().pulling = false;
                return;
            }
        };
        if head.complete {
            self.likes_pulled(service, Ok(head.tracks));
            return;
        }
        let known: HashSet<String> = self
            .lib
            .remote_likes(service)
            .unwrap_or_default()
            .into_iter()
            .map(|k| k.remote.id)
            .collect();
        match judge_head(&head, &known) {
            HeadVerdict::Same => {
                log::debug!("{service}: likes unchanged ({} known)", known.len());
                self.streaming.likes.entry(service).or_default().pulling = false;
            }
            HeadVerdict::New(new) => {
                self.streaming.likes.entry(service).or_default().pulling = false;
                self.apply_remote_likes(service, &new, false, true);
            }
            HeadVerdict::Full => {
                log::debug!("{service}: likes changed beyond their first page: reading them all");
                let Some(svc) = self.service(service) else {
                    self.streaming.likes.entry(service).or_default().pulling = false;
                    return;
                };
                self.streaming
                    .spawn(async move { Msg::LikesPulled(service, svc.liked().await) });
            }
        }
    }

    pub(crate) fn likes_pulled(&mut self, service: Service, r: Result<Vec<StreamTrack>, StreamError>) {
        self.streaming.likes.entry(service).or_default().pulling = false;
        let liked = match r {
            Ok(l) => l,
            Err(e) => {
                log::warn!("{service}: pulling likes: {e}");
                return;
            }
        };
        self.apply_remote_likes(service, &liked, false, false);
        let now = nikgob_library::now_ms();
        let _ = self.lib.cache_put(&full_key(service), now, "");
    }

    /// Brings the service's liked list in (see the module docs). With
    /// `import` (the API import of part B), new likes wait in the wishlist
    /// like on the first pull. With `partial`, `liked` is only the news
    /// (nothing is gone). Returns how many likes are new here.
    pub(crate) fn apply_remote_likes(
        &mut self,
        service: Service,
        liked: &[StreamTrack],
        import: bool,
        partial: bool,
    ) -> usize {
        let first = !self.lib.likes_pulled(service).unwrap_or(true);
        let known = self.lib.remote_likes(service).unwrap_or_default();
        let known_ids: HashSet<&str> = known.iter().map(|k| k.remote.id.as_str()).collect();
        let now = nikgob_library::now_ms();
        let origin = if first || import {
            WishlistOrigin::Import
        } else {
            WishlistOrigin::Sync
        };
        // Oldest first, so "Любимые" keeps the service's order (newest on top).
        let mut seen = HashSet::new();
        let adds: Vec<StreamTrack> = liked
            .iter()
            .rev()
            .filter(|t| !known_ids.contains(t.id.id.as_str()) && seen.insert(t.id.id.as_str()))
            .cloned()
            .collect();
        // Gone from the service: the like goes only if it came from there.
        let gone: Vec<RemoteLike> = if partial {
            Vec::new()
        } else {
            let there: HashSet<&str> = liked.iter().map(|t| t.id.id.as_str()).collect();
            known
                .iter()
                .filter(|k| !there.contains(k.remote.id.as_str()))
                .cloned()
                .collect()
        };
        let (new, unliked) = if adds.is_empty() && gone.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            self.log_err("лайки", self.lib.take_remote_likes(&adds, &gone, now))
                .unwrap_or_default()
        };
        for &t in &new {
            self.wish_for_like(t, origin);
        }
        for &t in &unliked {
            self.wish_drop_unstarted(t);
        }
        if first {
            let _ = self.lib.set_likes_pulled(service, now);
        }
        let changed = new.len() + unliked.len();
        log::info!(
            "{service}: {} liked{}, {changed} changes here",
            liked.len(),
            if partial { " (new ones)" } else { "" }
        );
        if changed > 0 {
            self.after_like_change();
            self.publish_wishlist();
            self.wish_tick();
        }
        new.len()
    }

    /// Queues a like / unlike of a library track for its services (sent
    /// together a moment later, see the module docs).
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
            let service = remote.service;
            if self.streaming.like_queue.push(remote, on) {
                let wait = self.streaming.schedule.like_wait();
                self.streaming.spawn(async move {
                    tokio::time::sleep(wait).await;
                    Msg::LikeFlush(service)
                });
            }
        }
    }

    /// Sends what waits in the queue of `service`: one call per direction.
    pub(crate) fn flush_likes(&mut self, service: Service) {
        let queued = self.streaming.like_queue.take(service);
        if queued.is_empty() {
            return;
        }
        let Some(svc) = self.service(service) else { return };
        for on in [true, false] {
            let remotes: Vec<RemoteId> = queued
                .iter()
                .filter(|(_, o)| *o == on)
                .map(|(r, _)| r.clone())
                .collect();
            if remotes.is_empty() {
                continue;
            }
            let ids: Vec<String> = remotes.iter().map(|r| r.id.clone()).collect();
            let svc = svc.clone();
            log::debug!("{service}: sending {} {}", ids.len(), if on { "likes" } else { "unlikes" });
            self.streaming.spawn(async move {
                let result = svc.set_liked_many(&ids, on).await;
                Msg::LikesPushed {
                    service,
                    remotes,
                    on,
                    result,
                }
            });
        }
    }

    pub(crate) fn likes_pushed(
        &mut self,
        service: Service,
        remotes: &[RemoteId],
        on: bool,
        r: Result<(), StreamError>,
    ) {
        if let Err(e) = r {
            log::warn!("{service}: set_liked_many({on}, {}): {e}", remotes.len());
            if on {
                for remote in remotes {
                    let _ = self.lib.delete_remote_like(remote);
                }
            }
            self.toast(
                format!(
                    "Не получилось {} лайк в {}",
                    if on { "поставить" } else { "снять" },
                    service.display_name()
                ),
                None,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_core::streaming::StreamTrack;

    fn t(id: &str) -> StreamTrack {
        StreamTrack::new(RemoteId::new(Service::Spotify, id), id)
    }

    fn known(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| (*s).to_owned()).collect()
    }

    fn head(ids: &[&str], total: u32) -> LikedHead {
        LikedHead {
            tracks: ids.iter().map(|i| t(i)).collect(),
            total: Some(total),
            complete: false,
        }
    }

    #[test]
    fn a_head_tells_same_new_or_read_everything() {
        let k = known(&["a", "b", "c", "d"]);
        assert_eq!(judge_head(&head(&["a", "b"], 4), &k), HeadVerdict::Same);
        let HeadVerdict::New(n) = judge_head(&head(&["x", "a", "b"], 5), &k) else { panic!() };
        assert_eq!(n.len(), 1);
        // An unlike: the count is short.
        assert_eq!(judge_head(&head(&["a", "b"], 3), &k), HeadVerdict::Full);
        // One like and one unlike: the count matches the known list, but
        // there is news: everything is read.
        assert_eq!(judge_head(&head(&["x", "a"], 4), &k), HeadVerdict::Full);
        // News below a known track (re-liked older song): everything.
        assert_eq!(judge_head(&head(&["a", "x", "b"], 5), &k), HeadVerdict::Full);
        // A whole page of news: more may follow.
        assert_eq!(judge_head(&head(&["x", "y"], 6), &k), HeadVerdict::Full);
    }

    #[test]
    fn a_like_and_its_unlike_cancel_out() {
        let mut q = LikeQueue::default();
        let r = RemoteId::new(Service::Spotify, "a");
        assert!(q.push(r.clone(), true), "first in the queue: arm a flush");
        assert!(!q.push(RemoteId::new(Service::Spotify, "b"), true));
        assert!(!q.push(r.clone(), false), "cancelled");
        assert_eq!(q.take(Service::Spotify), vec![(RemoteId::new(Service::Spotify, "b"), true)]);
        assert!(q.push(r, true), "armed again after a flush");
    }
}
