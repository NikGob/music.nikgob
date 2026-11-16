//! What the recommendations read in one go: every
//! track with its artists and release, the listening events, the playlist
//! entries with their times, and the ids the services know the library by.
//! Plain rows; the runtime turns them into the engine's inputs.

use std::collections::{HashMap, HashSet};

use nikgob_core::model::{ArtistId, Event, ReleaseId, TrackId, UnixMs};
use nikgob_core::streaming::{RemoteId, Service};

use crate::db::Library;
use crate::error::Result;
use crate::events::{EVENT_COLS, NOT_GUEST, event_from_row};
use crate::util::{get_id, get_opt_id, meta_get};

/// One track as the recommendations see it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecTrack {
    pub id: TrackId,
    pub title: String,
    /// The credit as shown ("A feat. B").
    pub artist: String,
    /// Credited artists, main first.
    pub artists: Vec<ArtistId>,
    pub release: Option<ReleaseId>,
    /// In the library (a file, a like or a live playlist entry); the rest
    /// are "seen" stream tracks, kept for their history.
    pub in_library: bool,
    pub has_local: bool,
    /// Which song it is: the recording it is linked into (the same song on
    /// two albums, see `recordings`), else its recording's MusicBrainz id,
    /// else its ISRC, else its folded title with its main artist. One song
    /// on two albums (an OST and its special edition) is one key: a mix
    /// plays it once.
    pub song: String,
}

impl Library {
    /// Every track not deleted, with its artists in credit order.
    pub fn rec_tracks(&self) -> Result<Vec<RecTrack>> {
        self.read(|c| {
            let mut artists: HashMap<TrackId, Vec<ArtistId>> = HashMap::new();
            let mut stmt = c.prepare_cached("SELECT track_id, artist_id FROM track_artist ORDER BY track_id, position")?;
            let rows = stmt.query_map([], |r| Ok((get_id::<TrackId>(r, 0)?, get_id::<ArtistId>(r, 1)?)))?;
            for row in rows {
                let (t, a) = row?;
                artists.entry(t).or_default().push(a);
            }
            let mut stmt = c.prepare_cached(
                "SELECT id, title, artist_display, release_id, in_library, has_local, recording_mbid, isrc,
                        (SELECT l.recording FROM recording_link l WHERE l.track_id = track.id)
                   FROM track
                  WHERE deleted_at IS NULL ORDER BY seq",
            )?;
            let rows = stmt.query_map([], |r| {
                let ids: (Option<String>, Option<String>, Option<String>) = (r.get(6)?, r.get(7)?, r.get(8)?);
                Ok((
                    RecTrack {
                        id: get_id(r, 0)?,
                        title: r.get(1)?,
                        artist: r.get(2)?,
                        artists: Vec::new(),
                        release: get_opt_id(r, 3)?,
                        in_library: r.get(4)?,
                        has_local: r.get(5)?,
                        song: String::new(),
                    },
                    ids,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (mut t, (recording, isrc, linked)) = row?;
                t.artists = artists.remove(&t.id).unwrap_or_default();
                t.song = match (linked, recording.filter(|m| !m.is_empty()), crate::streams::normal_isrc(isrc.as_deref())) {
                    (Some(g), _, _) => format!("rec:{g}"),
                    (None, Some(m), _) => format!("mb:{m}"),
                    (None, None, Some(i)) => format!("isrc:{i}"),
                    (None, None, None) => {
                        let by = t.artists.first().map_or_else(|| crate::streams::credit_keys(&t.artist).join(","), ToString::to_string);
                        format!("song:{}|{by}", crate::streams::song_key(&t.title))
                    }
                };
                out.push(t);
            }
            Ok(out)
        })
    }

    /// Every start, complete and skip, oldest first (the taste model's
    /// history; likes and the rest are read as state). A guest's plays never
    /// count; what the voice assistant and the agents put on comes with its
    /// `actor` and `device`, and the taste weighs it by them.
    ///
    /// A long history is read again and again (every build of the
    /// recommendations, every home), so the last answer is kept while the
    /// events are what they were ([`crate::events::stamp`]).
    pub fn listening_events(&self) -> Result<Vec<Event>> {
        self.read(|c| {
            let stamp = crate::events::stamp(c)?;
            if let Some((at, kept)) = self.listening_kept()
                && at == stamp
            {
                return Ok(kept.as_ref().clone());
            }
            let mut stmt = c.prepare_cached(
                &format!(
                    "SELECT {EVENT_COLS} FROM event WHERE type IN ('start', 'complete', 'skip') AND {NOT_GUEST} ORDER BY ts, id"
                ),
            )?;
            let rows = stmt.query_map([], event_from_row)?;
            let events = rows.collect::<rusqlite::Result<Vec<_>>>()?;
            self.keep_listening(stamp, &events);
            Ok(events)
        })
    }

    /// Every seek, oldest first (a touch of the play it fell in: how the
    /// user listens).
    pub fn seek_events(&self) -> Result<Vec<Event>> {
        self.read(|c| {
            let mut stmt =
                c.prepare_cached(&format!("SELECT {EVENT_COLS} FROM event WHERE type = 'seek' AND {NOT_GUEST} ORDER BY ts, id"))?;
            let rows = stmt.query_map([], event_from_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// The ids this device has stamped its events with: its own, and the
    /// ones it had before it took its node's (`device_ids_before`).
    pub fn own_device_ids(&self) -> Result<HashSet<String>> {
        let mut ids: HashSet<String> = HashSet::from([self.device_id().as_str().to_owned()]);
        let before = self.read(|c| meta_get(c, "device_ids_before"))?.unwrap_or_default();
        ids.extend(before.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned));
        Ok(ids)
    }

    /// Every liked track and when it was liked, newest first.
    pub fn liked_track_times(&self) -> Result<Vec<(TrackId, UnixMs)>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT target_id, updated_at FROM likes WHERE target_kind = 'track' AND deleted = 0
                  ORDER BY updated_at DESC",
            )?;
            let rows = stmt.query_map([], |r| Ok((get_id(r, 0)?, r.get(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Every live entry of a live playlist: its track and when it was added.
    pub fn playlist_entry_times(&self) -> Result<Vec<(TrackId, UnixMs)>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT e.track_id, e.added_at FROM playlist_entry e
                   JOIN playlist p ON p.id = e.playlist_id
                  WHERE e.deleted = 0 AND p.deleted = 0",
            )?;
            let rows = stmt.query_map([], |r| Ok((get_id(r, 0)?, r.get(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// The ids the services know the library's tracks by (tracks in the
    /// library only): live stream sources and the tracks' external ids.
    pub fn track_remote_ids(&self) -> Result<Vec<RemoteId>> {
        self.read(|c| {
            let mut out = Vec::new();
            let mut stmt = c.prepare_cached(
                "SELECT s.kind, s.locator FROM source s JOIN track t ON t.id = s.track_id
                  WHERE s.kind != 'local_file' AND s.trashed_at IS NULL
                    AND t.in_library = 1 AND t.deleted_at IS NULL
                 UNION
                 SELECT e.service, e.remote_id FROM external_id e JOIN track t ON t.id = e.local_id
                  WHERE e.kind = 'track' AND t.in_library = 1 AND t.deleted_at IS NULL",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                let (service, id) = row?;
                if let Some(service) = Service::parse(&service) {
                    out.push(RemoteId::new(service, id));
                }
            }
            Ok(out)
        })
    }

    /// Title and credit of every release that has a track in the library.
    pub fn release_names(&self) -> Result<Vec<(String, String)>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT r.title, r.artist_display FROM release r
                  WHERE EXISTS (SELECT 1 FROM track t WHERE t.release_id = r.id
                                   AND t.deleted_at IS NULL AND t.in_library = 1)",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Every artist with a track in the library: id and name.
    pub fn artist_names(&self) -> Result<Vec<(ArtistId, String)>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT a.id, a.name FROM artist a
                  WHERE EXISTS (SELECT 1 FROM track_artist ta JOIN track t ON t.id = ta.track_id
                                 WHERE ta.artist_id = a.id AND t.deleted_at IS NULL AND t.in_library = 1)",
            )?;
            let rows = stmt.query_map([], |r| Ok((get_id(r, 0)?, r.get(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
}

#[cfg(test)]
mod tests {
    use nikgob_core::model::{Artist, ArtistCredit, EventContext, EventType, Track};

    use super::*;

    #[test]
    fn rec_rows_carry_artists_events_and_entries() {
        let lib = Library::open_in_memory().unwrap();
        let a = lib.upsert_artist(&Artist::new("A")).unwrap();
        let mut credit = ArtistCredit::single("A", Some(a.id));
        let b = lib.upsert_artist(&Artist::new("B")).unwrap();
        credit.push(" feat. ", "B", Some(b.id));
        let t = lib.upsert_track(&Track::new("Song", credit)).unwrap();
        lib.record_event(t.id, EventType::Start, None, Some(0), EventContext::Library).unwrap();
        lib.record_event(t.id, EventType::Seek, None, Some(10), EventContext::Library).unwrap();
        lib.record_event(t.id, EventType::Complete, None, None, EventContext::Library).unwrap();
        let p = lib.create_playlist("P").unwrap();
        lib.playlist_add(p.id, t.id).unwrap();

        let rows = lib.rec_tracks().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].artists, vec![a.id, b.id]);
        assert_eq!(rows[0].artist, "A feat. B");
        let kinds: Vec<EventType> = lib.listening_events().unwrap().iter().map(|e| e.event_type).collect();
        assert_eq!(kinds, vec![EventType::Start, EventType::Complete]);
        assert_eq!(lib.playlist_entry_times().unwrap().len(), 1);
        assert_eq!(lib.artist_names().unwrap().len(), 2);
    }
}
