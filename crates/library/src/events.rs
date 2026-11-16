//! Listening history: append-only events and per-track stats, and the one
//! repair of the contexts older plays were recorded in
//! ([`Library::fix_event_contexts`]).

use nikgob_core::model::{DeviceId, Event, EventContext, EventId, EventMeta, EventType, SourceKind, TrackId};
use rusqlite::{Connection, Row, Transaction, params};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::db::Library;
use crate::error::Result;
use crate::types::TrackStats;
use crate::util::{get_enum, get_id, get_opt_enum, opt_u32};

/// Everything but a guest's plays: what the owner, the voice assistant and
/// the agents put on (their own `actor` and `device` say how much it counts).
pub(crate) const NOT_GUEST: &str = "coalesce(actor, 'owner') <> 'guest'";

pub(crate) const EVENT_COLS: &str =
    "id, ts, track_id, source_kind, type, position_ms, context, device, origin, context_id, end_reason, attention, volume, actor";

pub(crate) fn event_from_row(row: &Row<'_>) -> rusqlite::Result<Event> {
    Ok(Event {
        id: get_id(row, 0)?,
        ts: row.get(1)?,
        track_id: get_id(row, 2)?,
        source_kind: get_opt_enum(row, 3)?,
        event_type: get_enum(row, 4)?,
        position_ms: opt_u32(row, 5)?,
        context: get_enum(row, 6)?,
        device: DeviceId(row.get(7)?),
        origin: get_opt_enum(row, 8)?,
        context_id: row.get(9)?,
        end_reason: get_opt_enum(row, 10)?,
        attention: row.get(11)?,
        volume: row.get(12)?,
        actor: row.get::<_, Option<String>>(13)?.as_deref().and_then(nikgob_core::settings::ApiActor::parse),
    })
}

/// `meta` key of [`Library::fix_event_contexts`].
pub(crate) const CONTEXTS_KEY: &str = "event_contexts_fixed.v1";

/// The plays recorded in a wrong context before each play kept its own,
/// where the right one is certain:
/// * an album started from a service's page kept the id of the mix that
///   played before it (`album` with `mix:…`): it is the album of the
///   track, when the track is on one of the library's albums;
/// * what autoplay added after an album was recorded in the album
///   (`autoplay:…` ids of autoplay's picks): it plays in the radio, as
///   autoplay's picks are recorded now.
///
/// The rest stays as it is (a service's playlist after a mix: which one it
/// was is not known). Returns how many plays changed.
fn fix_contexts(tx: &Transaction<'_>) -> Result<(usize, usize)> {
    let albums = tx.execute(
        "UPDATE event SET context_id = 'release:' ||
                (SELECT t.release_id FROM track t JOIN release r ON r.id = t.release_id WHERE t.id = event.track_id)
          WHERE context = 'album' AND context_id LIKE 'mix:%'
            AND EXISTS (SELECT 1 FROM track t JOIN release r ON r.id = t.release_id WHERE t.id = event.track_id)",
        [],
    )?;
    let autoplay = tx.execute(
        "UPDATE event SET context = 'radio'
          WHERE origin = 'autoplay' AND context_id LIKE 'autoplay:%' AND context <> 'radio'",
        [],
    )?;
    if albums + autoplay > 0 {
        edited();
        note!("library: {albums} plays of an album took its id instead of a mix's, {autoplay} plays of autoplay are in the radio now");
    }
    Ok((albums + autoplay, albums + autoplay))
}

/// Aggregate columns over a group of events: plays, completes, skips, last played.
const STATS_COLS: &str = "\
coalesce(SUM(type = 'start'), 0), coalesce(SUM(type = 'complete'), 0), \
coalesce(SUM(type = 'skip'), 0), MAX(CASE WHEN type IN ('start', 'complete') THEN ts END)";

fn stats_from_row_at(row: &Row<'_>, base: usize) -> rusqlite::Result<TrackStats> {
    Ok(TrackStats {
        plays: row.get::<_, i64>(base)? as u32,
        completes: row.get::<_, i64>(base + 1)? as u32,
        skips: row.get::<_, i64>(base + 2)? as u32,
        last_played: row.get(base + 3)?,
    })
}

/// Edits of stored events (their count and newest time stay the same).
static EDITS: AtomicU64 = AtomicU64::new(0);

/// An event already stored was changed in place.
pub(crate) fn edited() {
    EDITS.fetch_add(1, Ordering::Relaxed);
}

/// What the events are now, cheaply: how many, the newest one's time, the
/// edits in place so far. Two equal stamps: the same events.
pub(crate) fn stamp(conn: &Connection) -> Result<(i64, i64, u64)> {
    let (count, newest) = conn.query_row("SELECT count(*), coalesce(max(ts), 0) FROM event", [], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok((count, newest, EDITS.load(Ordering::Relaxed)))
}

/// Inserts an event unless its id is already stored. `true` if it was new.
pub(crate) fn insert_event(conn: &Connection, event: &Event) -> Result<bool> {
    let n = conn.execute(
        "INSERT OR IGNORE INTO event(id, ts, track_id, source_kind, type, position_ms, context,
                                     device, origin, context_id, end_reason, attention, volume, actor)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            event.id.to_string(),
            event.ts,
            event.track_id.to_string(),
            event.source_kind.map(SourceKind::as_str),
            event.event_type.as_str(),
            event.position_ms,
            event.context.as_str(),
            event.device.as_str(),
            event.origin.map(|o| o.as_str()),
            event.context_id.as_deref(),
            event.end_reason.map(|r| r.as_str()),
            event.attention,
            event.volume,
            event.actor.map(|a| a.key())
        ],
    )?;
    Ok(n > 0)
}

impl Library {
    /// Records an event now, stamped with this device. Returns the stored event.
    pub fn record_event(
        &self,
        track: TrackId,
        event_type: EventType,
        source_kind: Option<SourceKind>,
        position_ms: Option<u32>,
        context: EventContext,
    ) -> Result<Event> {
        self.record_event_with(track, event_type, source_kind, position_ms, context, EventMeta::default())
    }

    /// [`Self::record_event`] with who chose the play, what played it, why
    /// it ended and how it was heard.
    pub fn record_event_with(
        &self,
        track: TrackId,
        event_type: EventType,
        source_kind: Option<SourceKind>,
        position_ms: Option<u32>,
        context: EventContext,
        meta: EventMeta,
    ) -> Result<Event> {
        let event = Event {
            id: EventId::new(),
            ts: self.tick(),
            track_id: track,
            source_kind,
            event_type,
            position_ms,
            context,
            device: self.device_id(),
            origin: meta.origin,
            context_id: meta.context_id,
            end_reason: meta.end_reason,
            attention: meta.attention,
            volume: meta.volume,
            actor: meta.actor,
        };
        self.record_event_raw(&event)?;
        Ok(event)
    }

    /// Stores a fully formed event. Idempotent on `id` (events are append-only,
    /// an existing id is left untouched). Returns `true` if it was new.
    pub fn record_event_raw(&self, event: &Event) -> Result<bool> {
        self.write(|tx| insert_event(tx, event))
    }

    /// Once per library (`event_contexts_fixed.v1`, a backup first): the
    /// contexts of older plays where the right one is certain (see
    /// [`fix_contexts`]). Returns how many plays changed.
    pub fn fix_event_contexts(&self) -> Result<usize> {
        Ok(self.repair(CONTEXTS_KEY, fix_contexts)?.unwrap_or(0))
    }

    /// Latest events, newest first.
    pub fn recent_events(&self, limit: u32) -> Result<Vec<Event>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {EVENT_COLS} FROM event WHERE {NOT_GUEST} ORDER BY ts DESC, id DESC LIMIT ?1"
            ))?;
            let rows = stmt.query_map([limit], event_from_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Latest events of one track, newest first.
    pub fn events_of_track(&self, track: TrackId, limit: u32) -> Result<Vec<Event>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {EVENT_COLS} FROM event WHERE track_id = ?1 AND {NOT_GUEST}
                  ORDER BY ts DESC, id DESC LIMIT ?2"
            ))?;
            let rows = stmt.query_map(params![track.to_string(), limit], event_from_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// The plays of `track`, and of the same song on other albums (one
    /// recording, see `recordings`): they count for the song once.
    pub fn track_stats(&self, track: TrackId) -> Result<TrackStats> {
        self.read(|c| {
            Ok(c
                .prepare_cached(&format!(
                    "SELECT {STATS_COLS} FROM event
                      WHERE track_id IN (SELECT ?1
                                         UNION SELECT b.track_id FROM recording_link a
                                                 JOIN recording_link b ON b.recording = a.recording
                                                WHERE a.track_id = ?1)
                        AND {NOT_GUEST}"
                ))?
                .query_row([track.to_string()], |r| stats_from_row_at(r, 0))?)
        })
    }

    /// Stats of every track that has at least one event (input for
    /// recommendations); the plays of a song on two albums count for the
    /// copy standing for it (`recording_canon`), one row.
    pub fn all_track_stats(&self) -> Result<Vec<(TrackId, TrackStats)>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT coalesce(s.canon, e.track_id) AS song, {STATS_COLS}
                   FROM event e LEFT JOIN recording_spare s ON s.track_id = e.track_id
                  WHERE coalesce(e.actor, 'owner') <> 'guest'
                  GROUP BY song"
            ))?;
            let rows = stmt.query_map([], |r| Ok((get_id(r, 0)?, stats_from_row_at(r, 1)?)))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Total number of stored events.
    pub fn count_events(&self) -> Result<u64> {
        self.read(|c| {
            Ok(c.query_row(&format!("SELECT COUNT(*) FROM event WHERE {NOT_GUEST}"), [], |r| r.get::<_, i64>(0))? as u64)
        })
    }
}

#[cfg(test)]
mod tests {
    use nikgob_core::model::{Artist, ArtistCredit, EventOrigin, Release, Track};

    use super::*;

    /// Only the plays whose right context is certain change, once.
    #[test]
    fn the_repair_fixes_only_the_contexts_it_is_sure_of() {
        let lib = Library::open_in_memory().unwrap();
        let a = lib.upsert_artist(&Artist::new("Кино")).unwrap();
        let credit = ArtistCredit::single("Кино", Some(a.id));
        let album = lib.upsert_release(&Release::new("Группа крови", credit.clone())).unwrap();
        let mut on_album = Track::new("Война", credit.clone());
        on_album.release_id = Some(album.id);
        let on_album = lib.upsert_track(&on_album).unwrap();
        let single = lib.upsert_track(&Track::new("Кукушка", credit)).unwrap();
        let play = |track: TrackId, ctx: EventContext, origin: EventOrigin, id: &str| {
            let meta = EventMeta { origin: Some(origin), context_id: Some(id.to_owned()), ..EventMeta::default() };
            lib.record_event_with(track, EventType::Start, None, Some(0), ctx, meta).unwrap().id
        };
        let release_id = format!("release:{}", album.id);
        // A service's album after a mix: the track's album is known…
        let stale = play(on_album.id, EventContext::Album, EventOrigin::Manual, "mix:Спокойное");
        // …or not.
        let unknown = play(single.id, EventContext::Album, EventOrigin::Queue, "mix:Спокойное");
        // A service's playlist after a mix: which one is not known.
        let playlist = play(on_album.id, EventContext::Playlist, EventOrigin::Manual, "mix:Спокойное");
        // Right already: an album's own id, a mix's song.
        let right = play(on_album.id, EventContext::Album, EventOrigin::Queue, &release_id);
        let mix = play(on_album.id, EventContext::HomeSection, EventOrigin::Mix, "mix:Спокойное");
        // Autoplay after an album was in the album; in the radio already.
        let autoplay = play(single.id, EventContext::Album, EventOrigin::Autoplay, "autoplay:track:x");
        let radio = play(single.id, EventContext::Radio, EventOrigin::Autoplay, "autoplay:track:x");
        // A play of the queue keeps its album even with an odd id.
        let queue = play(single.id, EventContext::Album, EventOrigin::Queue, "autoplay:track:x");
        let before = lib.recent_events(100).unwrap();

        assert_eq!(lib.fix_event_contexts().unwrap(), 2);
        let after = lib.recent_events(100).unwrap();
        let of = |id: EventId| after.iter().find(|e| e.id == id).unwrap().clone();
        assert_eq!(of(stale).context_id.as_deref(), Some(release_id.as_str()));
        assert_eq!(of(stale).context, EventContext::Album);
        assert_eq!(of(autoplay).context, EventContext::Radio);
        assert_eq!(of(autoplay).context_id.as_deref(), Some("autoplay:track:x"));
        for id in [unknown, playlist, right, mix, radio, queue] {
            let was = before.iter().find(|e| e.id == id).unwrap();
            let now = of(id);
            assert_eq!((now.context, now.context_id.as_deref()), (was.context, was.context_id.as_deref()), "{id:?}");
        }
        // Once per library: a play recorded wrong later is left.
        play(on_album.id, EventContext::Album, EventOrigin::Manual, "mix:Другой");
        assert_eq!(lib.fix_event_contexts().unwrap(), 0);
        assert!(lib.meta_value(CONTEXTS_KEY).unwrap().is_some());
    }
}
