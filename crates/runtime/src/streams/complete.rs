//! The playing stream track completed from its service. YouTube Music's top
//! search result (the «Всё» answer, one mixed request) names a song's
//! artist and cover but no album and no length, so a song started from it
//! had no release (no cover on Now Playing, nothing for the better covers
//! to look up) and no length (0:00, an empty progress bar). When such a
//! track starts, its details are asked once a run: the album with its
//! cover and the length come with them, and Now Playing shows them. The
//! next two tracks of the queue are asked as soon as they are next
//! ([`Worker::complete_next`]), so the album cover is known (and drawn
//! ahead) before a skip; a clip is not asked (its song is looked for by
//! `clips`, and a song has its album).

use std::collections::HashSet;

use nikgob_core::model::TrackId;
use nikgob_core::streaming::{StreamError, StreamTrack};

use super::Msg;
use crate::Worker;

/// The tracks asked for this run (an answer without an album is not asked
/// again at every start).
#[derive(Default)]
pub(crate) struct Completing {
    asked: HashSet<TrackId>,
}

impl Worker {
    /// [`Self::complete_playing`] for the next two tracks of the queue.
    pub(crate) fn complete_next(&mut self) {
        let next: Vec<TrackId> = self.queue.ahead(2).iter().map(|e| e.track).collect();
        for t in next {
            self.complete_playing(t);
        }
    }

    /// Asks the service of a stream-only song that plays (or comes next)
    /// without its album or its length for its details (see the module
    /// docs).
    pub(crate) fn complete_playing(&mut self, track: TrackId) {
        if self.streaming.completing.asked.contains(&track) {
            return;
        }
        let Some(row) = self.lib.get_track_row(track).ok().flatten() else {
            return;
        };
        // A clip has no album: its song is looked for by the clip repair.
        if (row.release_id.is_some() && row.duration_ms.is_some()) || row.has_local || row.is_clip() {
            return;
        }
        let Some(src) = self.lib.active_source(track).ok().flatten() else {
            return;
        };
        let Some(svc) = src.kind.service().and_then(|s| self.service(s)) else {
            return;
        };
        if !self.streaming.completing.asked.insert(track) {
            return;
        }
        let id = src.locator;
        self.streaming.spawn(async move {
            let result = svc.track(&id).await;
            Msg::Completed { track, id, result }
        });
    }

    pub(crate) fn completed(&mut self, track: TrackId, id: &str, result: Result<StreamTrack, StreamError>) {
        let t = match result {
            Ok(t) => t,
            Err(e) => {
                log::debug!(target: "nikgob_runtime::streams", "{track}: no details of {id}: {e}");
                return;
            }
        };
        if t.id.id != id || (t.album.is_none() && t.duration_ms.is_none()) {
            return;
        }
        self.streaming.learn(std::slice::from_ref(&t));
        match self.lib.upsert_stream_track(&t) {
            Ok(got) if got == track => {
                log::debug!(
                    target: "nikgob_runtime::streams",
                    "{track}: completed from {id}: album {:?}, {:?} ms",
                    t.album.as_ref().map(|a| a.title.as_str()),
                    t.duration_ms
                );
            }
            Ok(_) => return,
            Err(e) => {
                log::warn!("{track}: storing the details of {id}: {e}");
                return;
            }
        }
        let shown = self.state.read().player.current.as_ref().map(|r| r.id);
        if shown == Some(track) {
            self.publish_now_playing();
            self.kick_better_covers(Some(track));
        } else if self.queue.upcoming().iter().take(2).any(|e| e.track == track) {
            // The queue's rows show its album cover, drawn ahead.
            self.publish_player();
        }
    }
}
