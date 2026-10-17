//! Liked YouTube music videos and their songs (docs/stage3.md §7, «Клипы
//! в лайках»).
//!
//! A clip liked on YouTube plays its video: an intro, an outro, another
//! mix than the record, no album and no cover. When YouTube Music has the
//! song, [`Library::clip_to_song`] makes the library track the song (its
//! title, album, cover, length and the song's video id as the source) and
//! keeps the clip's video id linked to it as the identity of the remote
//! like (`remote_like` and `external_id` rows), so an unlike here unlikes
//! what was liked there and the next like check knows it. A clip without
//! a song stays one: its source is marked a clip ([`Library::mark_clip`],
//! `quality_score` [`score::STREAM_YOUTUBE_CLIP`]), which the rows show.
//!
//! [`score::STREAM_YOUTUBE_CLIP`]: nikgob_core::model::score::STREAM_YOUTUBE_CLIP

use nikgob_core::model::{ArtistCredit, TrackId, score};
use nikgob_core::streaming::{EntityKind, RemoteId, Service, StreamTrack, TrackKind};
use rusqlite::{OptionalExtension, params};

use super::{
    artist_refs, exists, has_local_source, linked, normal_isrc, parse, refresh_in_library,
    same, set_external, upsert_stream_track_in, write_stream_meta,
};
use crate::catalog::{merge_track_in, refresh_active};
use crate::db::Library;
use crate::error::{LibraryError, Result};

/// A YouTube like whose track still plays the liked video.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LikedClip {
    pub track: TrackId,
    /// The liked video as the library knows it (kind [`TrackKind::Video`]).
    pub video: StreamTrack,
}

impl Library {
    /// The YouTube likes whose track has no file and plays the liked video
    /// itself, and looks like a clip (no album, or marked a clip): what
    /// [`Self::clip_to_song`] may turn into songs. Newest likes first.
    pub fn liked_youtube_clips(&self) -> Result<Vec<LikedClip>> {
        self.read(|c| {
            let mut stmt = c.prepare(
                "SELECT rl.track_id, rl.remote_id, t.title, t.artist_credit, t.duration_ms
                   FROM remote_like rl
                   JOIN track t ON t.id = rl.track_id
                   JOIN source s ON s.track_id = t.id AND s.kind = 'youtube'
                                AND s.locator = rl.remote_id AND s.trashed_at IS NULL
                  WHERE rl.service = 'youtube' AND t.deleted_at IS NULL
                    AND (t.release_id IS NULL OR s.quality_score = ?1)
                    AND NOT EXISTS (SELECT 1 FROM source l WHERE l.track_id = t.id
                                       AND l.kind = 'local_file' AND l.trashed_at IS NULL)
                  ORDER BY rl.seen_at DESC, rl.remote_id",
            )?;
            let rows = stmt.query_map([score::STREAM_YOUTUBE_CLIP], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (track, video, title, credit, duration) = row?;
                let credit: ArtistCredit = serde_json::from_str(&credit)?;
                let mut t = StreamTrack::new(RemoteId::new(Service::Youtube, video), title);
                t.artists = artist_refs(c, &credit, Service::Youtube)?;
                t.duration_ms = duration.and_then(|d| u64::try_from(d).ok());
                t.kind = TrackKind::Video;
                out.push(LikedClip {
                    track: parse(&track)?,
                    video: t,
                });
            }
            Ok(out)
        })
    }

    /// The liked clip `video` (playing on the track `clip`) is the song
    /// `song` (see the module docs). The song's own track, when the
    /// library has one (linked, by ISRC or as the same song), takes the
    /// clip's track in ([`Self::merge_track_into`]); otherwise the clip's
    /// track becomes the song in place (its likes, playlists and history
    /// stay; the clip's lyrics and analysis go). Either way the clip loses
    /// its source and keeps its id linked. Returns the song's track.
    pub fn clip_to_song(&self, clip: TrackId, video: &RemoteId, song: &StreamTrack) -> Result<TrackId> {
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| {
            let cid = clip.to_string();
            if !exists(tx, "track", &cid)? {
                return Err(LibraryError::not_found("track", cid));
            }
            let mut song = song.clone();
            song.kind = TrackKind::Song;
            // The song's own track, if the library has one.
            let mut own = linked(tx, "track", song.id.service, EntityKind::Track, &song.id.id)?;
            if own.is_none()
                && let Some(isrc) = normal_isrc(song.isrc.as_deref())
            {
                own = tx
                    .query_row(
                        "SELECT id FROM track WHERE isrc = ?1 AND deleted_at IS NULL
                          ORDER BY has_local DESC, seq LIMIT 1",
                        [isrc],
                        |r| r.get(0),
                    )
                    .optional()?;
            }
            if own.is_none() {
                own = same::same_song_track(tx, &song)?;
            }
            let target = match own.filter(|o| *o != cid) {
                Some(o) => {
                    let o = parse(&o)?;
                    merge_track_in(tx, clip, o, device.as_str(), now)?;
                    o
                }
                None => {
                    if !has_local_source(tx, &cid)? {
                        write_stream_meta(tx, &song, Some(clip), false)?;
                        for table in ["lyrics", "lyrics_text", "lyrics_pick", "analysis"] {
                            tx.execute(&format!("DELETE FROM {table} WHERE track_id = ?1"), [&cid])?;
                        }
                    }
                    clip
                }
            };
            let tid = target.to_string();
            upsert_stream_track_in(tx, &song, Some(target))?;
            tx.execute(
                "DELETE FROM source WHERE track_id = ?1 AND kind = ?2 AND locator = ?3",
                params![tid, video.service.source_kind().as_str(), video.id],
            )?;
            set_external(tx, video.service, EntityKind::Track, &video.id, &tid)?;
            tx.execute(
                "UPDATE remote_like SET track_id = ?3 WHERE service = ?1 AND remote_id = ?2",
                params![video.service.as_str(), video.id, tid],
            )?;
            refresh_active(tx, target)?;
            refresh_in_library(tx, &tid)?;
            Ok(target)
        })
    }

    /// The YouTube video `video` has no song: its sources rank and show as
    /// a clip. Returns whether anything changed.
    pub fn mark_clip(&self, video: &RemoteId) -> Result<bool> {
        self.write(|tx| {
            let tracks: Vec<String> = {
                let mut stmt = tx.prepare(
                    "SELECT DISTINCT track_id FROM source
                      WHERE kind = ?1 AND locator = ?2 AND quality_score != ?3",
                )?;
                let rows = stmt.query_map(
                    params![video.service.source_kind().as_str(), video.id, score::STREAM_YOUTUBE_CLIP],
                    |r| r.get(0),
                )?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            tx.execute(
                "UPDATE source SET quality_score = ?3 WHERE kind = ?1 AND locator = ?2",
                params![video.service.source_kind().as_str(), video.id, score::STREAM_YOUTUBE_CLIP],
            )?;
            for t in &tracks {
                refresh_active(tx, parse(t)?)?;
            }
            Ok(!tracks.is_empty())
        })
    }
}
