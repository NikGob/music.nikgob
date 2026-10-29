//! What the cold-start imports and the sync of the services need from the
//! library (docs/stage3.md §3.6): listening history without duplicates,
//! "seen" tracks, keys that make a second import of the same file a no-op,
//! the tracks still waiting for their metadata, and a small cache.
//!
//! * History events are deduplicated by `(ts, track, type)` and get an id
//!   derived from the same triple ([`event_id`]), so importing a file twice,
//!   or on two devices, stores each play once.
//! * A history entry with an id in a service becomes a stream track (outside
//!   the library). One without (the old Spotify history has no uris) is
//!   matched by artist and title to a known track, else it becomes a "seen"
//!   track: a bare `track` row without any source, marked by an
//!   `external_id` row of the pseudo service [`SEEN_SERVICE`] whose remote id
//!   is the folded artist and title. The marker keeps it out of the library
//!   (see the module docs above) and finds it again on the next import.
//! * Import keys (`external_id` rows of the pseudo service `import`) remember
//!   what an export file created (a playlist by its name) or which track a
//!   video of it turned out to be (`youtube:<id>`), for the same reason.
//! * A song saved in a service's own library (YouTube Music's «music
//!   library songs») is no like: a row of the pseudo service
//!   [`SAVED_SERVICE`] marks its track, which teaches the recommendations
//!   and stays where it was (outside the library, if a stream). No pseudo
//!   service is ever returned by [`Library::external_ids`]: unknown
//!   services are skipped there.

use nikgob_core::model::{
    Event, EventId, EventType, LikeTarget, ReleaseId, Track, TrackId, UnixMs, Uuid,
};
use nikgob_core::streaming::{AlbumRef, ArtistRef, EntityKind, Service, StreamTrack};
use rusqlite::{OptionalExtension, params};

use super::{credit_for, linked, parse, refresh_in_library, release_for, upsert_stream_track_in};
use crate::catalog::upsert_track_in;
use crate::db::Library;
use crate::error::Result;
use crate::events::insert_event;
use crate::util::{meta_get, meta_set, sort_key};

/// Pseudo service of the marker of a "seen" track (no id in any service).
pub(crate) const SEEN_SERVICE: &str = "seen";
/// Pseudo service of the import keys.
const IMPORT_SERVICE: &str = "import";
/// Pseudo service of the marks of songs saved in a service's library.
const SAVED_SERVICE: &str = "saved";

/// A library track as an import compares the songs of an export with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchRow {
    pub id: TrackId,
    pub title: String,
    /// The credit line, then each credited artist and its aliases.
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub has_local: bool,
    pub duration_ms: Option<u32>,
}
/// Prefix of the cache keys in `meta`.
const CACHE_PREFIX: &str = "cache.";

/// FNV-1a over `bytes`, from `seed`.
fn fnv(seed: u64, bytes: &[u8]) -> u64 {
    let mut h = seed;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The id of an imported event: a UUIDv8 hashed from `(ts, track, type)`,
/// the same on every run and every device.
pub fn event_id(ts: UnixMs, track: TrackId, kind: EventType) -> EventId {
    let key = format!("{ts}|{track}|{}", kind.as_str());
    let hi = fnv(0xcbf2_9ce4_8422_2325, key.as_bytes());
    let lo = fnv(0x6c62_272e_07bb_0142, key.as_bytes());
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&hi.to_be_bytes());
    bytes[8..].copy_from_slice(&lo.to_be_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x80; // version 8
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // RFC 4122 variant
    EventId::from_uuid(Uuid::from_bytes(bytes))
}

/// Marker key of a seen track: service, folded artist and title.
fn seen_key(service: Service, artist: &str, title: &str) -> String {
    format!("{}:{}\u{1f}{}", service.as_str(), sort_key(artist), sort_key(title))
}

fn key_local(conn: &rusqlite::Connection, service: &str, kind: EntityKind, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT local_id FROM external_id WHERE service = ?1 AND kind = ?2 AND remote_id = ?3",
            params![service, kind.as_str(), key],
            |r| r.get(0),
        )
        .optional()?)
}

fn set_key(conn: &rusqlite::Connection, service: &str, kind: EntityKind, key: &str, local: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO external_id(service, kind, remote_id, local_id) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(service, kind, remote_id) DO UPDATE SET local_id = excluded.local_id",
        params![service, kind.as_str(), key, local],
    )?;
    Ok(())
}

/// A live track with this title whose credit is `artist` or starts with it.
fn by_name(conn: &rusqlite::Connection, artist: &str, title: &str) -> Result<Option<String>> {
    let artist = sort_key(artist);
    Ok(conn
        .query_row(
            "SELECT id FROM track
              WHERE title_key = ?1 AND deleted_at IS NULL
                AND (artist_key = ?2 OR substr(artist_key, 1, length(?2) + 2) = ?2 || ', ')
              ORDER BY has_local DESC, in_library DESC, seq LIMIT 1",
            params![sort_key(title), artist],
            |r| r.get(0),
        )
        .optional()?)
}

impl Library {
    /// The library tracks of stream tracks, created (outside the library)
    /// only when missing. Unlike [`Self::upsert_stream_tracks`], a track the
    /// library knows by its remote id keeps its metadata: an export file has
    /// less of it than the service gave earlier.
    pub fn ensure_stream_tracks(&self, tracks: &[StreamTrack]) -> Result<Vec<TrackId>> {
        self.write(|tx| {
            tracks
                .iter()
                .map(|t| match linked(tx, "track", t.id.service, EntityKind::Track, &t.id.id)? {
                    Some(id) => parse(&id),
                    None => upsert_stream_track_in(tx, t, None),
                })
                .collect()
        })
    }

    /// A live track called `title` by `artist` (or a credit that starts with
    /// it), local files first.
    pub fn track_by_name(&self, artist: &str, title: &str) -> Result<Option<TrackId>> {
        self.read(|c| by_name(c, artist, title)?.map(|s| parse(&s)).transpose())
    }

    /// The track of a history entry without a remote id: a known track of
    /// that name, else the seen track made for it earlier, else a new seen
    /// track (no source, outside the library; see the module docs).
    pub fn seen_track_by_name(
        &self,
        service: Service,
        artist: &str,
        title: &str,
        album: Option<&str>,
    ) -> Result<TrackId> {
        self.write(|tx| {
            let key = seen_key(service, artist, title);
            if let Some(id) = key_local(tx, SEEN_SERVICE, EntityKind::Track, &key)?
                && super::exists(tx, "track", &id)?
            {
                return parse(&id);
            }
            if let Some(id) = by_name(tx, artist, title)? {
                return parse(&id);
            }
            let artists = [ArtistRef::new(None, artist)];
            let credit = credit_for(tx, &artists)?;
            let release: Option<ReleaseId> = album
                .filter(|a| !a.trim().is_empty())
                .map(|a| release_for(tx, &AlbumRef::new(None, a), &artists))
                .transpose()?;
            let mut track = Track::new(title.to_owned(), credit);
            track.release_id = release;
            let id = upsert_track_in(tx, &track)?.id;
            let tid = id.to_string();
            set_key(tx, SEEN_SERVICE, EntityKind::Track, &key, &tid)?;
            tx.execute("UPDATE track SET in_library = 0 WHERE id = ?1", [&tid])?;
            refresh_in_library(tx, &tid)?;
            Ok(id)
        })
    }

    /// Stores imported events, skipping every one whose `(ts, track, type)`
    /// is stored already (whatever its id). Returns how many were new.
    pub fn import_events(&self, events: &[Event]) -> Result<usize> {
        self.write(|tx| {
            let mut new = 0;
            for e in events {
                let known: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM event WHERE track_id = ?1 AND ts = ?2 AND type = ?3)",
                    params![e.track_id.to_string(), e.ts, e.event_type.as_str()],
                    |r| r.get(0),
                )?;
                if !known && insert_event(tx, e)? {
                    new += 1;
                }
            }
            Ok(new)
        })
    }

    /// `Some(true)` liked, `Some(false)` liked once and unliked since (the
    /// user's choice: an import does not like it again), `None` never.
    pub fn like_state(&self, target: impl Into<LikeTarget>) -> Result<Option<bool>> {
        let target = target.into();
        self.read(|c| {
            Ok(c.query_row(
                "SELECT deleted FROM likes WHERE target_kind = ?1 AND target_id = ?2",
                params![target.kind_str(), target.id_string()],
                |r| r.get::<_, bool>(0),
            )
            .optional()?
            .map(|deleted| !deleted))
        })
    }

    /// The local row an import created for `key` (a playlist of an export
    /// file, by service and name), if it was recorded.
    pub fn import_key(&self, kind: EntityKind, key: &str) -> Result<Option<Uuid>> {
        self.read(|c| key_local(c, IMPORT_SERVICE, kind, key)?.map(|s| parse(&s)).transpose())
    }

    pub fn set_import_key(&self, kind: EntityKind, key: &str, local: Uuid) -> Result<()> {
        self.write(|tx| set_key(tx, IMPORT_SERVICE, kind, key, &local.to_string()))
    }

    /// [`Self::set_import_key`] for many keys in one transaction.
    pub fn set_import_keys(&self, kind: EntityKind, keys: &[(String, Uuid)]) -> Result<()> {
        self.write(|tx| {
            for (key, local) in keys {
                set_key(tx, IMPORT_SERVICE, kind, key, &local.to_string())?;
            }
            Ok(())
        })
    }

    /// Every live track of the library (the oldest first), for an import to
    /// find the songs of an export among them.
    pub fn match_rows(&self) -> Result<Vec<MatchRow>> {
        self.read(|c| {
            let mut stmt = c.prepare(
                "SELECT t.id, t.title, t.artist_display, r.title, t.has_local, t.duration_ms,
                        (SELECT group_concat(a.name, char(31)) FROM track_artist ta
                           JOIN artist a ON a.id = ta.artist_id WHERE ta.track_id = t.id),
                        (SELECT group_concat(al.name, char(31)) FROM track_artist ta
                           JOIN artist_alias al ON al.artist_id = ta.artist_id WHERE ta.track_id = t.id)
                   FROM track t LEFT JOIN release r ON r.id = t.release_id
                  WHERE t.deleted_at IS NULL AND t.in_library = 1
                  ORDER BY t.seq",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, bool>(4)?,
                    r.get::<_, Option<i64>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, Option<String>>(7)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (id, title, credit, album, has_local, duration, names, aliases) = row?;
                let mut artists = vec![credit];
                for n in names.iter().chain(&aliases).flat_map(|s| s.split('\u{1f}')) {
                    if !n.is_empty() && !artists.iter().any(|a| a == n) {
                        artists.push(n.to_owned());
                    }
                }
                out.push(MatchRow {
                    id: parse(&id)?,
                    title,
                    artists,
                    album,
                    has_local,
                    duration_ms: duration.and_then(|d| u32::try_from(d).ok()),
                });
            }
            Ok(out)
        })
    }

    /// Marks each track as the song (its id in `service`) saved in that
    /// service's library (see the module docs).
    pub fn mark_saved(&self, service: Service, songs: &[(String, TrackId)]) -> Result<()> {
        self.write(|tx| {
            for (remote, track) in songs {
                let key = format!("{}:{remote}", service.as_str());
                set_key(tx, SAVED_SERVICE, EntityKind::Track, &key, &track.to_string())?;
            }
            Ok(())
        })
    }

    /// The live tracks saved in a service's library, each once.
    pub fn saved_tracks(&self) -> Result<Vec<TrackId>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT DISTINCT x.local_id FROM external_id x JOIN track t ON t.id = x.local_id
                  WHERE x.service = ?1 AND x.kind = 'track' AND t.deleted_at IS NULL
                  ORDER BY x.local_id",
            )?;
            let rows = stmt.query_map([SAVED_SERVICE], |r| r.get::<_, String>(0))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(parse(&row?)?);
            }
            Ok(out)
        })
    }

    /// The albums in the library only for the playlists an import brought
    /// (each playlist an import created or synced in, by its import key):
    /// every track of it in the library is a stream without a file and
    /// without a like, in no other playlist, and the album is not liked.
    /// They are in the library and say «стрим», but nobody added them:
    /// «Недавно добавлено» leaves them out.
    pub fn releases_of_imported_playlists(&self) -> Result<std::collections::HashSet<ReleaseId>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "WITH imported(id) AS (
                     SELECT local_id FROM external_id WHERE service = ?1 AND kind = 'playlist'),
                 brought(id, release_id) AS (
                     SELECT t.id, t.release_id FROM track t
                      WHERE t.in_library = 1 AND t.deleted_at IS NULL AND t.release_id IS NOT NULL
                        AND NOT EXISTS (SELECT 1 FROM source s WHERE s.track_id = t.id AND s.kind = 'local_file')
                        AND NOT EXISTS (SELECT 1 FROM likes l WHERE l.target_kind = 'track' AND l.target_id = t.id
                                           AND l.deleted = 0)
                        AND EXISTS (SELECT 1 FROM playlist_entry e JOIN playlist p ON p.id = e.playlist_id
                                     WHERE e.track_id = t.id AND e.deleted = 0 AND p.deleted = 0
                                       AND p.id IN imported)
                        AND NOT EXISTS (SELECT 1 FROM playlist_entry e JOIN playlist p ON p.id = e.playlist_id
                                         WHERE e.track_id = t.id AND e.deleted = 0 AND p.deleted = 0
                                           AND p.id NOT IN imported))
                 SELECT DISTINCT b.release_id FROM brought b
                  WHERE NOT EXISTS (SELECT 1 FROM track o WHERE o.release_id = b.release_id AND o.in_library = 1
                                       AND o.deleted_at IS NULL AND o.id NOT IN (SELECT id FROM brought))
                    AND NOT EXISTS (SELECT 1 FROM likes l WHERE l.target_kind = 'release'
                                       AND l.target_id = b.release_id AND l.deleted = 0)",
            )?;
            let rows = stmt.query_map([IMPORT_SERVICE], |r| r.get::<_, String>(0))?;
            let mut out = std::collections::HashSet::new();
            for row in rows {
                out.insert(parse(&row?)?);
            }
            Ok(out)
        })
    }

    /// Stream-only tracks of `service` that still lack what the service
    /// knows (an import made them from an export file: no length), as
    /// `(track, remote id)`, oldest first. Unavailable sources are left out.
    pub fn stream_tracks_to_enrich(
        &self,
        service: Service,
        limit: u32,
    ) -> Result<Vec<(TrackId, String)>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT t.id, s.locator FROM track t JOIN source s ON s.track_id = t.id
                  WHERE t.duration_ms IS NULL AND t.deleted_at IS NULL AND t.has_local = 0
                    AND s.kind = ?1 AND s.trashed_at IS NULL AND s.available = 1
                  ORDER BY t.seq LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![service.as_str(), limit], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (id, locator) = row?;
                out.push((parse(&id)?, locator));
            }
            Ok(out)
        })
    }

    /// A cached value (discography lookups and the like), with the time it
    /// was stored. Not user data: never exported.
    pub fn cache_get(&self, key: &str) -> Result<Option<(UnixMs, String)>> {
        self.read(|c| {
            let Some(v) = meta_get(c, &format!("{CACHE_PREFIX}{key}"))? else {
                return Ok(None);
            };
            Ok(v.split_once('|')
                .and_then(|(at, value)| Some((at.parse().ok()?, value.to_owned()))))
        })
    }

    pub fn cache_put(&self, key: &str, at: UnixMs, value: &str) -> Result<()> {
        self.write(|tx| meta_set(tx, &format!("{CACHE_PREFIX}{key}"), &format!("{at}|{value}")))
    }

    /// Forgets the cached answers whose key starts with `prefix` and whose
    /// value is `value`; returns how many went.
    pub fn cache_forget(&self, prefix: &str, value: &str) -> Result<usize> {
        let prefix = format!("{CACHE_PREFIX}{prefix}");
        self.write(|tx| {
            Ok(tx.execute(
                "DELETE FROM meta WHERE substr(key, 1, length(?1)) = ?1
                    AND substr(value, instr(value, '|') + 1) = ?2",
                rusqlite::params![prefix, value],
            )?)
        })
    }
}

#[cfg(test)]
mod tests {
    use nikgob_core::model::{DeviceId, EventContext, SourceKind};
    use nikgob_core::streaming::{FakeService, RemoteId};

    use super::*;
    use crate::TrackFilter;

    fn lib() -> Library {
        Library::open_in_memory().unwrap()
    }

    fn event(track: TrackId, ts: UnixMs, kind: EventType) -> Event {
        Event {
            id: event_id(ts, track, kind),
            ts,
            track_id: track,
            source_kind: Some(SourceKind::Spotify),
            event_type: kind,
            position_ms: Some(1000),
            context: EventContext::Other,
            device: DeviceId::from("import"),
            origin: None,
            context_id: None,
            end_reason: None,
            attention: None,
            volume: None,
        }
    }

    #[test]
    fn event_ids_are_stable_and_events_are_deduplicated() {
        let lib = lib();
        let t = lib
            .upsert_stream_track(&FakeService::spotify().tracks()[0])
            .unwrap();
        assert_eq!(event_id(5, t, EventType::Start), event_id(5, t, EventType::Start));
        assert_ne!(event_id(5, t, EventType::Start), event_id(5, t, EventType::Skip));
        let batch = [event(t, 5, EventType::Start), event(t, 9, EventType::Complete)];
        assert_eq!(lib.import_events(&batch).unwrap(), 2);
        assert_eq!(lib.import_events(&batch).unwrap(), 0, "the same file again");
        // Same play under another id (an older import): still a duplicate.
        let mut other = event(t, 5, EventType::Start);
        other.id = EventId::new();
        assert_eq!(lib.import_events(&[other]).unwrap(), 0);
        assert_eq!(lib.count_events().unwrap(), 2);
    }

    #[test]
    fn seen_tracks_stay_outside_and_are_found_again() {
        let lib = lib();
        let a = lib
            .seen_track_by_name(Service::Spotify, "Кино", "Звезда по имени Солнце", Some("Звезда"))
            .unwrap();
        let b = lib
            .seen_track_by_name(Service::Spotify, "кино", "звезда по имени солнце", None)
            .unwrap();
        assert_eq!(a, b);
        assert_eq!(lib.track_in_library(a).unwrap(), Some(false));
        assert_eq!(lib.count_tracks(TrackFilter::All).unwrap(), 0);
        assert!(lib.external_ids(EntityKind::Track, *a.as_uuid()).unwrap().is_empty());
        // A repair does not bring it in; a like does.
        assert_eq!(lib.recompute_in_library_all().unwrap(), 0);
        lib.like(a).unwrap();
        assert_eq!(lib.track_in_library(a).unwrap(), Some(true));
        // A known track of that name is taken instead of a new seen one.
        let known = lib.upsert_stream_track(&FakeService::spotify().tracks()[0]).unwrap();
        let by_name = lib
            .seen_track_by_name(Service::Spotify, "Кино", "Группа крови", None)
            .unwrap();
        assert_eq!(by_name, known);
        assert_eq!(lib.track_by_name("кино", "группа крови").unwrap(), Some(known));
    }

    #[test]
    fn ensure_keeps_metadata_and_finds_tracks_to_enrich() {
        let lib = lib();
        let full = FakeService::spotify().tracks()[0].clone();
        let id = lib.upsert_stream_track(&full).unwrap();
        let mut thin = StreamTrack::new(full.id.clone(), "группа крови");
        thin.artists = vec![ArtistRef::new(None, "Кино")];
        assert_eq!(lib.ensure_stream_tracks(&[thin]).unwrap(), vec![id]);
        let t = lib.get_track(id).unwrap().unwrap();
        assert_eq!(t.title, "Группа крови", "metadata kept");
        assert!(lib.stream_tracks_to_enrich(Service::Spotify, 10).unwrap().is_empty());

        let mut bare = StreamTrack::new(RemoteId::new(Service::Spotify, "from-file"), "Song");
        bare.artists = vec![ArtistRef::new(None, "Artist")];
        let bare_id = lib.ensure_stream_tracks(&[bare]).unwrap()[0];
        assert_eq!(
            lib.stream_tracks_to_enrich(Service::Spotify, 10).unwrap(),
            vec![(bare_id, "from-file".to_owned())]
        );
        assert!(lib.stream_tracks_to_enrich(Service::Youtube, 10).unwrap().is_empty());
    }

    #[test]
    fn like_state_import_keys_and_cache() {
        let lib = lib();
        let t = lib.upsert_stream_track(&FakeService::spotify().tracks()[1]).unwrap();
        assert_eq!(lib.like_state(t).unwrap(), None);
        lib.like(t).unwrap();
        assert_eq!(lib.like_state(t).unwrap(), Some(true));
        lib.unlike(t).unwrap();
        assert_eq!(lib.like_state(t).unwrap(), Some(false));

        lib.set_import_keys(EntityKind::Track, &[("youtube:upload00001".to_owned(), *t.as_uuid())]).unwrap();
        assert_eq!(lib.import_key(EntityKind::Track, "youtube:upload00001").unwrap(), Some(*t.as_uuid()));
        assert!(lib.external_ids(EntityKind::Track, *t.as_uuid()).unwrap().iter().all(|r| r.id != "upload00001"));
        let p = lib.create_playlist("Mix").unwrap();
        assert_eq!(lib.import_key(EntityKind::Playlist, "spotify:mix").unwrap(), None);
        lib.set_import_key(EntityKind::Playlist, "spotify:mix", *p.id.as_uuid()).unwrap();
        assert_eq!(
            lib.import_key(EntityKind::Playlist, "spotify:mix").unwrap(),
            Some(*p.id.as_uuid())
        );
        assert!(lib.external_ids(EntityKind::Playlist, *p.id.as_uuid()).unwrap().is_empty());

        assert_eq!(lib.cache_get("disco.x").unwrap(), None);
        lib.cache_put("disco.x", 42, "[1|2]").unwrap();
        assert_eq!(lib.cache_get("disco.x").unwrap(), Some((42, "[1|2]".to_owned())));
    }

    /// The albums only an import's playlists brought; one the user liked,
    /// put a song of in a playlist of their own, or has a file of is the
    /// user's.
    #[test]
    fn albums_brought_by_imported_playlists_are_told_apart() {
        let lib = lib();
        let song = |id: &str, title: &str, album: &str| {
            let mut t = StreamTrack::new(RemoteId::new(Service::Youtube, id), title);
            t.artists = vec![ArtistRef::new(None, "Artist")];
            t.album = Some(AlbumRef::new(None, album));
            lib.ensure_stream_tracks(&[t]).unwrap()[0]
        };
        let release = |t: TrackId| lib.get_track(t).unwrap().unwrap().release_id.unwrap();
        let (a, b, c, d, e) = (
            song("song0000001", "One", "Brought"),
            song("song0000002", "Two", "Liked Song"),
            song("song0000003", "Three", "Own List"),
            song("song0000004", "Four", "Liked Album"),
            song("song0000005", "Five", "Mine"),
        );
        let imported = lib.create_playlist("Save").unwrap();
        lib.set_import_key(EntityKind::Playlist, "youtube:save", *imported.id.as_uuid()).unwrap();
        lib.playlist_add_many(imported.id, &[a, b, c, d]).unwrap();
        lib.like(b).unwrap();
        let own = lib.create_playlist("Мой").unwrap();
        lib.playlist_add(own.id, c).unwrap();
        lib.like(LikeTarget::Release(release(d))).unwrap();
        lib.playlist_add(own.id, e).unwrap();
        assert_eq!(lib.releases_of_imported_playlists().unwrap(), [release(a)].into_iter().collect());
        // A playlist of no import is the user's own.
        let plain = lib.create_playlist("Ещё").unwrap();
        let f = song("song0000006", "Six", "Other");
        lib.playlist_add(plain.id, f).unwrap();
        assert!(!lib.releases_of_imported_playlists().unwrap().contains(&release(f)));
        // The playlist gone: its songs leave the library, nothing to tell.
        lib.delete_playlist(imported.id).unwrap();
        assert!(lib.releases_of_imported_playlists().unwrap().is_empty());
    }

    #[test]
    fn saved_songs_are_marked_apart_from_likes_and_the_library() {
        let lib = lib();
        let mut song = StreamTrack::new(RemoteId::new(Service::Youtube, "savedsong01"), "Song");
        song.artists = vec![ArtistRef::new(None, "Artist")];
        let t = lib.ensure_stream_tracks(&[song]).unwrap()[0];
        let songs = [("savedsong01".to_owned(), t)];
        lib.mark_saved(Service::Youtube, &songs).unwrap();
        lib.mark_saved(Service::Youtube, &songs).unwrap();
        assert_eq!(lib.saved_tracks().unwrap(), vec![t]);
        assert_eq!(lib.track_in_library(t).unwrap(), Some(false), "a saved song is not the library");
        assert_eq!(lib.like_state(t).unwrap(), None);
        assert_eq!(
            lib.external_ids(EntityKind::Track, *t.as_uuid()).unwrap(),
            vec![RemoteId::new(Service::Youtube, "savedsong01")],
            "the mark is no id in a service"
        );
        assert!(lib.match_rows().unwrap().is_empty(), "only the library's tracks are matched");
        lib.like(t).unwrap();
        let rows = lib.match_rows().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].id, rows[0].title.as_str()), (t, "Song"));
        assert_eq!(rows[0].artists, ["Artist"]);
    }
}
