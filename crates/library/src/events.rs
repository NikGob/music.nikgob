//! Listening history: append-only events and per-track stats.

use nikgob_core::model::{DeviceId, Event, EventContext, EventId, EventMeta, EventType, SourceKind, TrackId};
use rusqlite::{Connection, Row, params};

use crate::db::Library;
use crate::error::Result;
use crate::types::TrackStats;
use crate::util::{get_enum, get_id, get_opt_enum, opt_u32};

pub(crate) const EVENT_COLS: &str =
    "id, ts, track_id, source_kind, type, position_ms, context, device, origin, context_id, end_reason, attention, volume";

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
    })
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

/// Inserts an event unless its id is already stored. `true` if it was new.
pub(crate) fn insert_event(conn: &Connection, event: &Event) -> Result<bool> {
    let n = conn.execute(
        "INSERT OR IGNORE INTO event(id, ts, track_id, source_kind, type, position_ms, context,
                                     device, origin, context_id, end_reason, attention, volume)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
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
            event.volume
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
        };
        self.record_event_raw(&event)?;
        Ok(event)
    }

    /// Stores a fully formed event. Idempotent on `id` (events are append-only,
    /// an existing id is left untouched). Returns `true` if it was new.
    pub fn record_event_raw(&self, event: &Event) -> Result<bool> {
        self.write(|tx| insert_event(tx, event))
    }

    /// Latest events, newest first.
    pub fn recent_events(&self, limit: u32) -> Result<Vec<Event>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {EVENT_COLS} FROM event ORDER BY ts DESC, id DESC LIMIT ?1"
            ))?;
            let rows = stmt.query_map([limit], event_from_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Latest events of one track, newest first.
    pub fn events_of_track(&self, track: TrackId, limit: u32) -> Result<Vec<Event>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {EVENT_COLS} FROM event WHERE track_id = ?1
                  ORDER BY ts DESC, id DESC LIMIT ?2"
            ))?;
            let rows = stmt.query_map(params![track.to_string(), limit], event_from_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn track_stats(&self, track: TrackId) -> Result<TrackStats> {
        self.read(|c| {
            Ok(c
                .prepare_cached(&format!("SELECT {STATS_COLS} FROM event WHERE track_id = ?1"))?
                .query_row([track.to_string()], |r| stats_from_row_at(r, 0))?)
        })
    }

    /// Stats of every track that has at least one event (input for recommendations).
    pub fn all_track_stats(&self) -> Result<Vec<(TrackId, TrackStats)>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT track_id, {STATS_COLS} FROM event GROUP BY track_id"
            ))?;
            let rows = stmt.query_map([], |r| Ok((get_id(r, 0)?, stats_from_row_at(r, 1)?)))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Total number of stored events.
    pub fn count_events(&self) -> Result<u64> {
        self.read(|c| {
            Ok(c.query_row("SELECT COUNT(*) FROM event", [], |r| r.get::<_, i64>(0))? as u64)
        })
    }
}
