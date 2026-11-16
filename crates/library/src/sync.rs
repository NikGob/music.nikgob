//! Syncing the library between the owner's devices: what changed, in
//! pages, and the rows of another device merged in.
//!
//! * **The clock.** [`Library::tick`] is a hybrid logical clock in unix
//!   milliseconds: never less than the wall clock, never back, and past
//!   every time seen in a row from another device ([`hlc_observe`], at most
//!   [`MAX_AHEAD_MS`] ahead of this device's own clock), so an edit made
//!   after seeing another device's always wins over it, and old rows still
//!   compare. The database keeps one too (`sync_clock.hlc`), for the
//!   change triggers.
//! * **The change log.** Every synced table ([`TABLES`]) has triggers that
//!   note the last change of each row in `change_log` with a number growing
//!   over the whole database (`seq`): another device asks for "everything
//!   of a table after N" ([`Library::sync_changes`]). Rows deleted leave
//!   their key there as gone (a tombstone), so a hard delete (`track_fx`,
//!   the catalog) syncs too.
//! * **Versions.** The user data with its own `updated_at`, `updated_by`
//!   and `deleted` columns (playlists, entries, likes, album choices, songs
//!   put in the library) keeps comparing by them; every other table by the
//!   change log's version (`hlc`, `node`, `gone`). Both with the rule of
//!   the userdata files ([`incoming_wins`]): the later clock wins, a gone
//!   row beats a live one on a tie, then the larger device id.
//! * **Merging** ([`Library::sync_apply`]) writes a page in one transaction,
//!   with the change triggers quiet (`sync_clock.applying`): the rows keep
//!   the version they came with and remember the device they came from,
//!   which is not offered them back. A row that breaks a constraint here
//!   (an artist whose MusicBrainz id another artist has, a stream source
//!   another one plays) is left out and counted; a catalog row is deleted
//!   only when nothing here still uses it (a track with a file of this
//!   device stays).
//! * **What does not travel**: the paths of local files (`source` rows of
//!   kind `local_file`; which device has which file travels as `node_file`
//!   rows, kept by triggers on `source`), what is computed per file by its
//!   source id (prints, the quality verdicts, the same-audio pairs), the
//!   services' sync state, downloads and wishes, masters and replicas, the
//!   guest's plays.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};

use nikgob_core::model::Uuid;
use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::Library;
use crate::error::{LibraryError, Result};
use crate::userdata::incoming_wins;
use crate::util::{meta_get, meta_set, now_ms};

/// Another device's clock is followed at most this far ahead of this
/// device's own (a clock far off is that device's problem: its rows still
/// count with their own times).
pub const MAX_AHEAD_MS: i64 = 5 * 60_000;
/// Rows a page holds at most…
pub const PAGE_ROWS: u32 = 500;
/// …and about this many bytes of values (blobs as base64).
pub const PAGE_BYTES: usize = 1 << 20;
/// The largest page a device asks for.
const MAX_PAGE_ROWS: u32 = 2_000;

/// What the triggers are made from: a change here (or in [`TABLES`])
/// makes them again.
const TRIGGERS_VERSION: &str = "1";

/// The next time of the clock `last` at wall time `now`.
pub(crate) fn hlc_next(last: &AtomicI64, now: i64) -> i64 {
    let mut prev = last.load(Ordering::Relaxed);
    loop {
        let next = now.max(prev + 1);
        match last.compare_exchange_weak(prev, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(actual) => prev = actual,
        }
    }
}

/// The clock `last` has seen time `seen` (another device's) at wall time
/// `now`: it never gives a time before it again, unless it is more than
/// [`MAX_AHEAD_MS`] ahead.
pub(crate) fn hlc_observe(last: &AtomicI64, seen: i64, now: i64) {
    last.fetch_max(seen.min(now + MAX_AHEAD_MS), Ordering::Relaxed);
}

/// What part of the library a synced table is (the order of a first sync).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncGroup {
    Catalog,
    UserData,
    History,
    Derived,
}

/// A synced table.
#[derive(Debug)]
pub struct SyncTable {
    pub name: &'static str,
    pub keys: &'static [&'static str],
    /// The synced columns besides the key (local ones, like a track's
    /// `has_local` or a row's `seq`, are not here).
    pub cols: &'static [&'static str],
    /// Versioned by its own `updated_at` / `updated_by` / `deleted`.
    pub own: bool,
    /// The rows that travel, `{r}` standing for the row.
    filter: Option<&'static str>,
    /// When a row gone elsewhere may go here too (`?1` the first key
    /// column); none: always.
    guard: Option<&'static str>,
    pub group: SyncGroup,
}

const fn table(name: &'static str, keys: &'static [&'static str], cols: &'static [&'static str], group: SyncGroup) -> SyncTable {
    SyncTable { name, keys, cols, own: false, filter: None, guard: None, group }
}

const fn own(name: &'static str, keys: &'static [&'static str], cols: &'static [&'static str]) -> SyncTable {
    SyncTable { own: true, ..table(name, keys, cols, SyncGroup::UserData) }
}

use SyncGroup::{Catalog, Derived, History, UserData};

/// Every synced table, parents before the rows that point at them (the
/// order of a pull).
pub const TABLES: &[SyncTable] = &[
    SyncTable {
        guard: Some(
            "NOT EXISTS (SELECT 1 FROM track_artist WHERE artist_id = ?1)
             AND NOT EXISTS (SELECT 1 FROM release_artist WHERE artist_id = ?1)",
        ),
        ..table("artist", &["id"], &["mbid", "name", "sort_name", "sort_key", "images", "added_at"], Catalog)
    },
    SyncTable {
        guard: Some("NOT EXISTS (SELECT 1 FROM track WHERE release_id = ?1)"),
        ..table(
            "release",
            &["id"],
            &[
                "mbid", "release_group_mbid", "title", "title_key", "artist_credit", "artist_display", "artist_key", "year",
                "release_type", "cover_ref", "total_tracks", "added_at",
            ],
            Catalog,
        )
    },
    SyncTable {
        guard: Some("NOT EXISTS (SELECT 1 FROM source WHERE track_id = ?1)"),
        ..table(
            "track",
            &["id"],
            &[
                "recording_mbid", "isrc", "acoustid", "title", "title_key", "artist_credit", "artist_display", "artist_key",
                "release_id", "album_key", "disc_no", "track_no", "duration_ms", "added_at", "deleted_at", "in_library",
            ],
            Catalog,
        )
    },
    table("release_artist", &["release_id", "artist_id"], &["position"], Catalog),
    table("track_artist", &["track_id", "artist_id"], &["position"], Catalog),
    table("artist_alias", &["key", "artist_id"], &["name", "source"], Catalog),
    table("track_alias", &["key", "track_id"], &["title", "source"], Catalog),
    // The streams' sources (a local file's path is this device's own; which
    // device has which file is `node_file`). Which source plays is each
    // device's own choice (`is_active`).
    SyncTable {
        filter: Some("{r}.kind <> 'local_file'"),
        ..table(
            "source",
            &["id"],
            &[
                "track_id", "kind", "locator", "codec", "bitrate", "sample_rate", "bit_depth", "quality_score", "file_hash",
                "size_bytes", "origin_kind", "origin_info_hash", "origin_path", "available", "added_at", "trashed_at", "trash_path",
            ],
            Catalog,
        )
    },
    table(
        "node_file",
        &["node_id", "source_id"],
        &["track_id", "file_hash", "codec", "size_bytes", "quality_score", "bitrate", "sample_rate", "bit_depth"],
        Catalog,
    ),
    table("external_id", &["service", "kind", "remote_id"], &["local_id"], Catalog),
    own("playlist", &["id"], &["name", "cover", "created_at", "updated_at", "updated_by", "deleted"]),
    own("playlist_entry", &["id"], &["playlist_id", "track_id", "position", "added_at", "updated_at", "updated_by", "deleted"]),
    own("likes", &["target_kind", "target_id"], &["updated_at", "updated_by", "deleted"]),
    own("release_selection", &["release_id"], &["mode", "view", "versions", "updated_at", "updated_by", "deleted"]),
    own("release_track_selection", &["release_id", "track_id"], &["choice", "updated_at", "updated_by", "deleted"]),
    own("track_put", &["track_id"], &["updated_at", "updated_by", "deleted"]),
    table("rec_feedback", &["target_kind", "target_id"], &["dislike", "updated_at"], UserData),
    table("recording_link", &["track_id"], &["recording", "how", "at"], UserData),
    table("recording_apart", &["track_id", "other_track"], &["at"], UserData),
    table("track_fx", &["track_id"], &["variant", "updated_at"], UserData),
    table("lyrics_pick", &["track_id"], &["source"], UserData),
    table("lyrics_show", &["track_id"], &["sync"], UserData),
    table(
        "unwanted",
        &["release_id", "title_key"],
        &["title", "artist", "disc_no", "track_no", "duration_ms", "isrc", "recording_mbid", "track_id", "at"],
        UserData,
    ),
    table("artist_photo", &["artist_id"], &["path", "source", "fetched_at", "photo_locked"], UserData),
    SyncTable {
        filter: Some("coalesce({r}.actor, 'owner') <> 'guest'"),
        ..table(
            "event",
            &["id"],
            &[
                "ts", "track_id", "source_kind", "type", "position_ms", "context", "device", "origin", "context_id", "end_reason",
                "attention", "volume", "actor",
            ],
            History,
        )
    },
    table("lyrics", &["track_id"], &["synced", "plain", "instrumental", "source", "fetched_at"], Derived),
    table("lyrics_text", &["track_id", "source"], &["body", "fetched_at"], Derived),
    table(
        "lyrics_align",
        &["track_id", "source"],
        &["text_hash", "fit", "timing", "offset_ms", "covered", "body", "made_by", "made_at", "saved", "fits"],
        Derived,
    ),
    table("lyrics_align_done", &["track_id"], &["state", "detail", "made_by", "at"], Derived),
    table("analysis", &["track_id"], &["bliss_vector", "analyzed_with", "analyzed_at", "source"], Derived),
    table("sound_embedding", &["track_id"], &["vector", "embedded_with", "source", "embedded_at"], Derived),
    table("sound_sketch", &["track_id"], &["sketch", "sketched_with", "styled_with", "source", "sketched_at"], Derived),
    table("palette", &["release_id"], &["seed_argb", "scheme_blob", "ambient_blur", "computed_at"], Derived),
];

/// The synced table named `name`.
pub fn sync_table(name: &str) -> Option<&'static SyncTable> {
    TABLES.iter().find(|t| t.name == name)
}

// ------------------------------------------------------------------ wire

/// A page of a table's changes, oldest first.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SyncPage {
    pub table: String,
    /// The columns of [`SyncRow::d`].
    pub cols: Vec<String>,
    pub rows: Vec<SyncRow>,
    /// The last change number looked at: the next page starts after it.
    pub next: i64,
    /// Not the last page.
    pub more: bool,
}

/// One row as it travels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SyncRow {
    /// The key as the change log keeps it (`json_array` of the key columns).
    pub k: String,
    /// Its version: the clock, the device, gone.
    pub v: (i64, String, bool),
    /// The values of [`SyncPage::cols`]; none for a row gone. Blobs are
    /// `{"b": "<base64>"}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub d: Option<Vec<Value>>,
}

/// How far each table's changes go, and which database it is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncHeads {
    /// Made once per database: another one (rebuilt, restored from an old
    /// copy) is taken from the beginning.
    pub epoch: String,
    pub tables: BTreeMap<String, i64>,
}

/// What a page did here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncApplied {
    /// Rows written (or deleted).
    pub taken: u32,
    /// Rows already as new here, or a delete held back.
    pub kept: u32,
    /// Rows that could not be written here.
    pub skipped: u32,
}

/// How far this device has taken another one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncCursors {
    pub off: bool,
    pub epoch: Option<String>,
    pub tables: BTreeMap<String, i64>,
    pub last_ok: Option<i64>,
}

// ---------------------------------------------------------------- values

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = (u32::from(chunk[0]) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        out.push(char::from(B64[(n >> 18) as usize & 63]));
        out.push(char::from(B64[(n >> 12) as usize & 63]));
        out.push(if chunk.len() > 1 { char::from(B64[(n >> 6) as usize & 63]) } else { '=' });
        out.push(if chunk.len() > 2 { char::from(B64[n as usize & 63]) } else { '=' });
    }
    out
}

pub(crate) fn b64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some(u32::from(c - b'A')),
            b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes = s.as_bytes();
    if bytes.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let last = bytes.len() / 4;
    for (k, chunk) in bytes.chunks(4).enumerate() {
        let pad = chunk.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && k + 1 != last) {
            return None;
        }
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            let v = if i >= 4 - pad { 0 } else { val(c)? };
            n = (n << 6) | v;
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

fn json_of(v: ValueRef<'_>) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::from(i),
        ValueRef::Real(f) => serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number),
        ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => serde_json::json!({ "b": b64_encode(b) }),
    }
}

fn sql_of(v: &Value) -> Result<SqlValue> {
    Ok(match v {
        Value::Null => SqlValue::Null,
        Value::Bool(b) => SqlValue::Integer(i64::from(*b)),
        Value::Number(n) => match n.as_i64() {
            Some(i) => SqlValue::Integer(i),
            None => SqlValue::Real(n.as_f64().unwrap_or(0.0)),
        },
        Value::String(s) => SqlValue::Text(s.clone()),
        Value::Object(o) => match o.get("b").and_then(Value::as_str).and_then(b64_decode) {
            Some(b) => SqlValue::Blob(b),
            None => return Err(LibraryError::Invalid("a synced value that is not a blob".into())),
        },
        Value::Array(_) => return Err(LibraryError::Invalid("a synced value that is a list".into())),
    })
}

/// About how many bytes a value takes on the wire.
fn weight(v: &Value) -> usize {
    match v {
        Value::String(s) => s.len() + 2,
        Value::Object(o) => o.get("b").and_then(Value::as_str).map_or(8, |b| b.len() + 8),
        _ => 8,
    }
}

/// The values of a key as the change log keeps it.
fn key_values(key: &str) -> Result<Vec<SqlValue>> {
    let parts: Vec<Value> = serde_json::from_str(key)?;
    parts.iter().map(sql_of).collect()
}

fn is_constraint(e: &rusqlite::Error) -> bool {
    e.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation)
}

// -------------------------------------------------------------- triggers

/// `UPDATE` of the database's clock for a change made here.
const BUMP: &str = "UPDATE sync_clock SET seq = seq + 1, hlc = max(hlc + 1, CAST(unixepoch('subsec') * 1000 AS INTEGER)) WHERE id = 1;";
/// The triggers stay quiet while another device's rows are written.
const QUIET: &str = "(SELECT applying FROM sync_clock WHERE id = 1) = 0";

fn key_expr(t: &SyncTable, r: &str) -> String {
    let parts: Vec<String> = t.keys.iter().map(|k| format!("{r}.{k}")).collect();
    format!("json_array({})", parts.join(", "))
}

fn filter_of(t: &SyncTable, r: &str) -> String {
    t.filter.map_or_else(|| "1".to_owned(), |f| f.replace("{r}", r))
}

/// The statements noting `key`'s change (`gone` an SQL expression), when
/// `when` holds. A delete and an insert, never `OR REPLACE`: the statement
/// that fired the trigger would impose its own conflict rule on it.
fn note(t: &SyncTable, key: &str, gone: &str, when: &str) -> String {
    let n = t.name;
    format!(
        "DELETE FROM change_log WHERE tbl = '{n}' AND key = {key} AND ({when}); \
         INSERT INTO change_log(tbl, key, seq, hlc, node, gone, came_from) \
         SELECT '{n}', {key}, seq, hlc, node, {gone}, NULL FROM sync_clock WHERE id = 1 AND ({when});"
    )
}

fn triggers(t: &SyncTable) -> [String; 3] {
    let n = t.name;
    let (knew, kold) = (key_expr(t, "new"), key_expr(t, "old"));
    let (fnew, fold) = (filter_of(t, "new"), filter_of(t, "old"));
    let changed: Vec<String> = t.keys.iter().chain(t.cols.iter()).map(|c| format!("old.{c} IS NOT new.{c}")).collect();
    let changed = changed.join(" OR ");
    [
        format!("CREATE TRIGGER sync_{n}_ai AFTER INSERT ON {n} WHEN {QUIET} AND ({fnew}) BEGIN {BUMP} {} END", note(t, &knew, "0", "1")),
        format!(
            "CREATE TRIGGER sync_{n}_au AFTER UPDATE ON {n} WHEN {QUIET} AND (({fnew}) OR ({fold})) AND ({changed}) BEGIN {BUMP} {} {} END",
            note(t, &kold, "1", &format!("{kold} IS NOT {knew}")),
            note(t, &knew, &format!("CASE WHEN ({fnew}) THEN 0 ELSE 1 END"), "1"),
        ),
        format!("CREATE TRIGGER sync_{n}_ad AFTER DELETE ON {n} WHEN {QUIET} AND ({fold}) BEGIN {BUMP} {} END", note(t, &kold, "1", "1")),
    ]
}

/// What the triggers are made of now.
fn fingerprint() -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for t in TABLES {
        let parts = std::iter::once(t.name).chain(t.keys.iter().copied()).chain(t.cols.iter().copied()).chain(t.filter).chain(t.guard);
        for part in parts {
            for b in part.bytes().chain(std::iter::once(0)) {
                h ^= u64::from(b);
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        }
    }
    format!("{TRIGGERS_VERSION}:{h:016x}")
}

/// Notes every row of the synced tables not in the change log yet (a
/// library from before sync, a table newly synced), each with its own
/// change number.
fn backfill(tx: &Connection) -> Result<()> {
    let now = now_ms();
    tx.execute("UPDATE sync_clock SET hlc = max(hlc, ?1) WHERE id = 1", [now])?;
    for t in TABLES {
        let key = key_expr(t, "x");
        let n = tx.execute(
            &format!(
                "INSERT INTO change_log(tbl, key, seq, hlc, node, gone, came_from)
                 SELECT '{name}', {key}, c.seq + row_number() OVER (), c.hlc, c.node, 0, NULL
                   FROM {name} x, sync_clock c
                  WHERE c.id = 1 AND ({filter})
                    AND NOT EXISTS (SELECT 1 FROM change_log l WHERE l.tbl = '{name}' AND l.key = {key})",
                name = t.name,
                filter = filter_of(t, "x"),
            ),
            [],
        )?;
        if n > 0 {
            tx.execute("UPDATE sync_clock SET seq = seq + ?1 WHERE id = 1", [n as i64])?;
        }
    }
    Ok(())
}

/// Readies sync on an open, migrated database: this device's id for the
/// change log, the triggers (made again when [`TABLES`] changed, and then
/// the rows not noted yet), the database's epoch. The clock's time.
pub(crate) fn install(conn: &mut Connection, device: &str) -> Result<i64> {
    let tx = conn.transaction()?;
    tx.execute("UPDATE sync_clock SET node = ?1, applying = 0 WHERE id = 1", [device])?;
    let want = fingerprint();
    if meta_get(&tx, "sync.triggers")?.as_deref() != Some(want.as_str()) {
        let old: Vec<String> = {
            let mut stmt = tx.prepare(r"SELECT name FROM sqlite_master WHERE type = 'trigger' AND name LIKE 'sync\_%' ESCAPE '\'")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for name in old {
            tx.execute_batch(&format!("DROP TRIGGER IF EXISTS \"{name}\""))?;
        }
        for t in TABLES {
            for sql in triggers(t) {
                tx.execute_batch(&sql)?;
            }
        }
        backfill(&tx)?;
        meta_set(&tx, "sync.triggers", &want)?;
    }
    if meta_get(&tx, "sync.epoch")?.is_none() {
        meta_set(&tx, "sync.epoch", &Uuid::now_v7().simple().to_string())?;
    }
    let hlc: i64 = tx.query_row("SELECT hlc FROM sync_clock WHERE id = 1", [], |r| r.get(0))?;
    tx.commit()?;
    Ok(hlc)
}

// ----------------------------------------------------------------- reads

fn where_keys(t: &SyncTable) -> String {
    let parts: Vec<String> = t.keys.iter().enumerate().map(|(i, k)| format!("x.{k} = ?{}", i + 1)).collect();
    parts.join(" AND ")
}

fn select_sql(t: &SyncTable) -> String {
    let cols: Vec<String> = t.cols.iter().map(|c| format!("x.{c}")).collect();
    format!("SELECT {} FROM {} x WHERE {} AND ({})", cols.join(", "), t.name, where_keys(t), filter_of(t, "x"))
}

/// The row of `keys`, as it travels (none: not here, or not one that
/// travels).
fn fetch_row(stmt: &mut rusqlite::Statement<'_>, keys: &[SqlValue], n: usize) -> Result<Option<Vec<Value>>> {
    let mut rows = stmt.query(params_from_iter(keys.iter()))?;
    let out = match rows.next()? {
        Some(r) => {
            let mut d = Vec::with_capacity(n);
            for i in 0..n {
                d.push(json_of(r.get_ref(i)?));
            }
            Some(d)
        }
        None => None,
    };
    Ok(out)
}

/// The version of a user-data row from its own columns.
fn own_version(t: &SyncTable, d: &[Value]) -> Option<(i64, String, bool)> {
    let at = |name: &str| t.cols.iter().position(|c| *c == name).and_then(|i| d.get(i));
    let ts = at("updated_at")?.as_i64()?;
    let by = at("updated_by")?.as_str()?.to_owned();
    let deleted = at("deleted")?.as_i64()? != 0;
    Some((ts, by, deleted))
}

fn changes_in(c: &Connection, t: &SyncTable, since: i64, limit: u32, peer: Option<&str>) -> Result<SyncPage> {
    type Entry = (String, i64, i64, String, bool, Option<String>);
    let entries: Vec<Entry> = {
        let mut stmt = c.prepare_cached(
            "SELECT key, seq, hlc, node, gone, came_from FROM change_log WHERE tbl = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![t.name, since, limit], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get::<_, i64>(4)? != 0, r.get(5)?))
        })?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut page = SyncPage {
        table: t.name.to_owned(),
        cols: t.cols.iter().map(|c| (*c).to_owned()).collect(),
        rows: Vec::new(),
        next: since,
        more: entries.len() as u32 >= limit,
    };
    let mut fetch = c.prepare_cached(&select_sql(t))?;
    let mut bytes = 0usize;
    for (key, seq, hlc, node, gone, came) in entries {
        page.next = seq;
        if peer.is_some() && came.as_deref() == peer {
            continue;
        }
        let data = if gone { None } else { fetch_row(&mut fetch, &key_values(&key)?, t.cols.len())? };
        let v = match &data {
            Some(d) if t.own => own_version(t, d).unwrap_or((hlc, node, false)),
            Some(_) => (hlc, node, false),
            None => (hlc, node, true),
        };
        bytes += key.len() + 32 + data.as_ref().map_or(0, |d| d.iter().map(weight).sum());
        page.rows.push(SyncRow { k: key, v, d: data });
        if bytes >= PAGE_BYTES {
            page.more = true;
            break;
        }
    }
    Ok(page)
}

// ----------------------------------------------------------------- merge

/// The version of the row of `key` here, if it has one: a user-data row's
/// own, else the change log's (a row here the log does not know loses to
/// any other).
fn local_version(tx: &Connection, t: &SyncTable, key: &str, keys: &[SqlValue]) -> Result<Option<(i64, bool, String)>> {
    if t.own {
        let sql = format!("SELECT x.updated_at, x.deleted, x.updated_by FROM {} x WHERE {}", t.name, where_keys(t));
        let row: Option<(i64, bool, String)> = tx
            .prepare_cached(&sql)?
            .query_row(params_from_iter(keys.iter()), |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0, r.get(2)?)))
            .optional()?;
        if row.is_some() {
            return Ok(row);
        }
    }
    let logged: Option<(i64, bool, String)> = tx
        .prepare_cached("SELECT hlc, gone, node FROM change_log WHERE tbl = ?1 AND key = ?2")?
        .query_row(params![t.name, key], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0, r.get(2)?)))
        .optional()?;
    if logged.is_some() || t.own {
        return Ok(logged);
    }
    Ok(row_exists(tx, t, keys)?.then(|| (0, false, String::new())))
}

fn row_exists(tx: &Connection, t: &SyncTable, keys: &[SqlValue]) -> Result<bool> {
    let sql = format!("SELECT EXISTS (SELECT 1 FROM {} x WHERE {})", t.name, where_keys(t));
    Ok(tx.prepare_cached(&sql)?.query_row(params_from_iter(keys.iter()), |r| r.get(0))?)
}

fn upsert_sql(t: &SyncTable, cols: &[&str]) -> String {
    let all: Vec<&str> = t.keys.iter().copied().chain(cols.iter().copied()).collect();
    let marks: Vec<String> = (1..=all.len()).map(|i| format!("?{i}")).collect();
    let action = if cols.is_empty() {
        "NOTHING".to_owned()
    } else {
        let set: Vec<String> = cols.iter().map(|c| format!("{c} = excluded.{c}")).collect();
        format!("UPDATE SET {}", set.join(", "))
    };
    format!(
        "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT({}) DO {action}",
        t.name,
        all.join(", "),
        marks.join(", "),
        t.keys.join(", ")
    )
}

fn delete_sql(t: &SyncTable) -> String {
    let keys: Vec<String> = t.keys.iter().enumerate().map(|(i, k)| format!("{k} = ?{}", i + 1)).collect();
    match t.guard {
        Some(g) => format!("DELETE FROM {} WHERE {} AND ({g})", t.name, keys.join(" AND ")),
        None => format!("DELETE FROM {} WHERE {}", t.name, keys.join(" AND ")),
    }
}

/// Merges `page` (rows of device `from`) in; the change triggers are quiet
/// meanwhile. What it did, and the latest clock among its rows.
fn apply_in(tx: &Connection, t: &SyncTable, page: &SyncPage, from: &str) -> Result<(SyncApplied, i64)> {
    // The columns both sides know (another version may have more or less).
    let present: Vec<(usize, &'static str)> =
        t.cols.iter().filter_map(|c| page.cols.iter().position(|p| p == c).map(|i| (i, *c))).collect();
    if t.own && !["updated_at", "updated_by", "deleted"].iter().all(|c| present.iter().any(|(_, n)| n == c)) {
        return Err(LibraryError::Invalid(format!("{}: a page without its versions", t.name)));
    }
    let cols: Vec<&str> = present.iter().map(|(_, c)| *c).collect();
    let up = upsert_sql(t, &cols);
    let del = delete_sql(t);
    let mut seq: i64 = tx.query_row("SELECT seq FROM sync_clock WHERE id = 1", [], |r| r.get(0))?;
    let mut seen = 0i64;
    let mut out = SyncApplied::default();
    for row in &page.rows {
        let keys = match key_values(&row.k) {
            Ok(k) if k.len() == t.keys.len() => k,
            _ => {
                out.skipped += 1;
                continue;
            }
        };
        let (hlc, node) = (row.v.0, row.v.1.as_str());
        // A user-data row deleted is a row with `deleted` set (written as
        // it is); any other row gone is deleted here too.
        let tomb = row.v.2 || row.d.is_none();
        let gone = row.d.is_none() || (!t.own && row.v.2);
        seen = seen.max(hlc);
        if let Some((at, deleted, by)) = local_version(tx, t, &row.k, &keys)?
            && !incoming_wins((hlc, tomb, node), (at, deleted, by.as_str()))
        {
            out.kept += 1;
            continue;
        }
        let written = if gone {
            tx.prepare_cached(&del)?.execute(params_from_iter(keys.iter()))
        } else {
            let d = row.d.as_deref().unwrap_or_default();
            let mut vals = keys.clone();
            for (i, _) in &present {
                vals.push(sql_of(d.get(*i).unwrap_or(&Value::Null))?);
            }
            tx.prepare_cached(&up)?.execute(params_from_iter(vals.iter()))
        };
        match written {
            Ok(_) => {}
            Err(e) if is_constraint(&e) => {
                log::debug!("sync: a row of {} from {from} left out: {e}", t.name);
                out.skipped += 1;
                continue;
            }
            Err(e) => return Err(e.into()),
        }
        if gone && row_exists(tx, t, &keys)? {
            // Still used here: it stays (with its own version).
            out.kept += 1;
            continue;
        }
        seq += 1;
        tx.execute("DELETE FROM change_log WHERE tbl = ?1 AND key = ?2", params![t.name, row.k])?;
        tx.execute(
            "INSERT INTO change_log(tbl, key, seq, hlc, node, gone, came_from) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![t.name, row.k, seq, hlc, node, gone, from],
        )?;
        out.taken += 1;
    }
    let seen_capped = seen.min(now_ms() + MAX_AHEAD_MS);
    tx.execute("UPDATE sync_clock SET seq = ?1, hlc = max(hlc, ?2) WHERE id = 1", params![seq, seen_capped])?;
    Ok((out, seen))
}

impl Library {
    /// This database's epoch and how far each synced table's changes go.
    pub fn sync_heads(&self) -> Result<SyncHeads> {
        self.read(|c| {
            let epoch = meta_get(c, "sync.epoch")?.unwrap_or_default();
            let mut stmt = c.prepare_cached("SELECT max(seq) FROM change_log WHERE tbl = ?1")?;
            let mut tables = BTreeMap::new();
            for t in TABLES {
                let head: Option<i64> = stmt.query_row([t.name], |r| r.get(0))?;
                tables.insert(t.name.to_owned(), head.unwrap_or(0));
            }
            Ok(SyncHeads { epoch, tables })
        })
    }

    /// The changes of `table` after change `since`, at most `limit` rows
    /// (and about [`PAGE_BYTES`]); none of those taken from device `peer`
    /// (the one asking).
    pub fn sync_changes(&self, table: &str, since: i64, limit: u32, peer: Option<&str>) -> Result<SyncPage> {
        let t = sync_table(table).ok_or_else(|| LibraryError::Invalid(format!("no synced table {table}")))?;
        let limit = limit.clamp(1, MAX_PAGE_ROWS);
        self.read(|c| changes_in(c, t, since, limit, peer))
    }

    /// Merges a page of device `from`'s changes in, in one transaction;
    /// with `cursor`, notes in it that `from`'s table is taken up to the
    /// page's end. Idempotent: a page merged twice changes nothing more.
    pub fn sync_apply(&self, page: &SyncPage, from: &str, cursor: bool) -> Result<SyncApplied> {
        let t = sync_table(&page.table).ok_or_else(|| LibraryError::Invalid(format!("no synced table {}", page.table)))?;
        let (out, seen) = self.write(|tx| {
            tx.execute("UPDATE sync_clock SET applying = 1 WHERE id = 1", [])?;
            let r = apply_in(tx, t, page, from)?;
            if cursor {
                tx.execute(
                    "INSERT INTO sync_peer(node, tbl, seq) VALUES (?1, ?2, ?3)
                     ON CONFLICT(node, tbl) DO UPDATE SET seq = excluded.seq",
                    params![from, t.name, page.next],
                )?;
            }
            tx.execute("UPDATE sync_clock SET applying = 0 WHERE id = 1", [])?;
            Ok(r)
        })?;
        if seen > 0 {
            self.observe_clock(seen);
        }
        Ok(out)
    }

    /// How far device `node` is taken, and whether syncing with it is off.
    pub fn sync_cursors(&self, node: &str) -> Result<SyncCursors> {
        self.read(|c| {
            let (off, epoch, last_ok) = c
                .query_row("SELECT off, epoch, last_ok FROM sync_node WHERE node = ?1", [node], |r| {
                    Ok((r.get::<_, i64>(0)? != 0, r.get(1)?, r.get(2)?))
                })
                .optional()?
                .unwrap_or((false, None, None));
            let mut stmt = c.prepare_cached("SELECT tbl, seq FROM sync_peer WHERE node = ?1")?;
            let rows = stmt.query_map([node], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            let tables = rows.collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
            Ok(SyncCursors { off, epoch, tables, last_ok })
        })
    }

    /// Device `node`'s database is another one now (`epoch`), or one of its
    /// tables went back: those of `tables` (all when empty) are taken from
    /// the beginning again (merging what is here, never wiping it).
    pub fn sync_start_over(&self, node: &str, epoch: &str, tables: &[&str]) -> Result<()> {
        self.write(|tx| {
            if tables.is_empty() {
                tx.execute("DELETE FROM sync_peer WHERE node = ?1", [node])?;
            } else {
                for t in tables {
                    tx.execute("DELETE FROM sync_peer WHERE node = ?1 AND tbl = ?2", params![node, t])?;
                }
            }
            tx.execute(
                "INSERT INTO sync_node(node, epoch) VALUES (?1, ?2) ON CONFLICT(node) DO UPDATE SET epoch = excluded.epoch",
                params![node, epoch],
            )?;
            Ok(())
        })
    }

    /// A pull from `node` went through at `at` (unix ms).
    pub fn sync_mark_ok(&self, node: &str, at: i64) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO sync_node(node, last_ok) VALUES (?1, ?2) ON CONFLICT(node) DO UPDATE SET last_ok = excluded.last_ok",
                params![node, at],
            )?;
            Ok(())
        })
    }

    /// Turns syncing with `node` on or off (its cursors stay: on again, it
    /// goes on from where it was).
    pub fn set_sync_off(&self, node: &str, off: bool) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO sync_node(node, off) VALUES (?1, ?2) ON CONFLICT(node) DO UPDATE SET off = excluded.off",
                params![node, off],
            )?;
            Ok(())
        })
    }

    /// The devices syncing is turned off with.
    pub fn sync_off_nodes(&self) -> Result<Vec<String>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached("SELECT node FROM sync_node WHERE off = 1 ORDER BY node")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// This device's id becomes `node` (the mesh's node id): the old one is
    /// kept in `meta` (`device_ids_before`, one per line; rows stamped with
    /// it stay as they are), this device's files are noted under the new
    /// one. Whether it changed.
    pub fn adopt_node_id(&self, node: &str) -> Result<bool> {
        let old = self.device_id();
        if node.is_empty() || old.as_str() == node {
            return Ok(false);
        }
        self.write(|tx| {
            let before = meta_get(tx, "device_ids_before")?.unwrap_or_default();
            let list = if before.is_empty() { old.as_str().to_owned() } else { format!("{before}\n{}", old.as_str()) };
            meta_set(tx, "device_ids_before", &list)?;
            meta_set(tx, "device_id", node)?;
            tx.execute("UPDATE sync_clock SET node = ?1 WHERE id = 1", [node])?;
            tx.execute("UPDATE OR REPLACE node_file SET node_id = ?1 WHERE node_id = ?2", params![node, old.as_str()])?;
            Ok(())
        })?;
        self.set_device_in_memory(nikgob_core::model::DeviceId(node.to_owned()));
        log::info!("library: this device's id is now its node id {node}");
        Ok(true)
    }
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
