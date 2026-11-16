//! Syncing the library between the owner's devices: what changed, in
//! pages, and the rows of another device merged in.
//!
//! * **The clock.** One hybrid logical clock per database, in process
//!   ([`Library`] keeps it; the change triggers read it through the SQL
//!   function `sync_hlc()`), persisted in `sync_clock.hlc`. A version is
//!   `unix ms << 16 | counter` ([`LOGICAL_BITS`]): never less than the wall
//!   clock, strictly growing (65 536 per millisecond before it runs ahead)
//!   and past every version merged from another device. A page with a
//!   version more than [`MAX_AHEAD_MS`] ahead of this device's wall clock
//!   is refused ([`page_ahead`]): a version that could not be superseded is
//!   never taken. `updated_at` columns stay wall-clock ms for display.
//! * **The change log.** Every synced table ([`TABLES`]) has triggers that
//!   note the last change of each row in `change_log`, each with its own
//!   number (`seq`, growing over the whole database) and version: another
//!   device asks for "everything of a table after N"
//!   ([`Library::sync_changes`]). Rows deleted leave their key there as
//!   gone (a tombstone), so a hard delete (`track_fx`, the catalog) syncs.
//! * **Versions.** Every table compares by the change log's
//!   `(version, gone, device)`: the larger version wins; a row noted before
//!   sync existed has version 0 (its own `updated_at` when it has one,
//!   [`SyncTable::stamp`]), and on an equal version 0 the row here stays (a
//!   migration is never an edit: an incoming row only fills what is
//!   missing); on another equal version a tombstone wins, then the larger
//!   device id. A table may prefer rows by a flag ([`SyncTable::prefer`]:
//!   a saved alignment beats an automatic one whatever the versions).
//! * **Merging** ([`Library::sync_apply`]) writes a page in one transaction,
//!   with the change triggers quiet (`sync_clock.applying`): the rows keep
//!   the version they came with and remember the device they came from,
//!   which is not offered them back. Ids of artists, albums and stream
//!   sources another device made for what this one has under its own id
//!   (the same MusicBrainz id, the same folded name, the same stream) become
//!   aliases (`sync_alias`), and every incoming reference goes through them.
//!   A row that cannot be written here, and a delete held back because the
//!   row is still used here, is kept in `sync_conflict` and tried again
//!   after every page and every turn of the loop; the count shows on the
//!   device's row. After stream sources change, their tracks' playing source
//!   is chosen again.
//! * **What does not travel**: the paths of local files (`source` rows of
//!   kind `local_file`; which device has which file travels as `node_file`
//!   rows, kept by triggers on `source`), what is computed per file by its
//!   source id (prints, the quality verdicts, the same-audio pairs), the
//!   services' sync state, downloads and wishes, masters and replicas, the
//!   guest's plays. Pictures (covers, artist photos) travel as files by
//!   their BLAKE3 hash (`picture`, `picture_file`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use nikgob_core::model::{TrackId, Uuid};
use nikgob_core::names::name_key;
use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::Library;
use crate::error::{LibraryError, Result};
use crate::util::{meta_get, meta_set, now_ms};

/// A page with a version further ahead of this device's wall clock is
/// refused.
pub const MAX_AHEAD_MS: i64 = 5 * 60_000;
/// The low bits of a version: a counter within one millisecond.
pub const LOGICAL_BITS: u32 = 16;
/// Rows a page holds at most…
pub const PAGE_ROWS: u32 = 500;
/// …and about this many bytes of values (blobs as base64).
pub const PAGE_BYTES: usize = 1 << 20;
/// The largest page a device asks for.
const MAX_PAGE_ROWS: u32 = 2_000;
/// Kept conflicts tried again after a page, at most.
const RETRY_BATCH: u32 = 300;
/// What the triggers are made from: a change here (or in [`TABLES`])
/// makes them again.
const TRIGGERS_VERSION: &str = "2";

// ------------------------------------------------------------------ clock

/// The version of wall-clock time `ms`.
pub const fn version_at(ms: i64) -> i64 {
    ms << LOGICAL_BITS
}

/// The wall-clock ms of version `v`.
pub const fn ms_of(v: i64) -> i64 {
    v >> LOGICAL_BITS
}

/// The next version of the clock `clock` at wall time `now` (ms).
pub(crate) fn hlc_next(clock: &AtomicI64, now: i64) -> i64 {
    let floor = version_at(now);
    let mut prev = clock.load(Ordering::Relaxed);
    loop {
        let next = floor.max(prev + 1);
        match clock.compare_exchange_weak(prev, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(actual) => prev = actual,
        }
    }
}

/// The clock has seen version `seen` (merged from another device): it never
/// gives a version up to it again. Only versions [`page_ahead`] let through
/// get here.
pub(crate) fn hlc_observe(clock: &AtomicI64, seen: i64) {
    clock.fetch_max(seen, Ordering::Relaxed);
}

/// Whether a page carries a version more than [`MAX_AHEAD_MS`] ahead of
/// wall time `now` (that device's clock runs fast: nothing of it is taken).
pub fn page_ahead(page: &SyncPage, now: i64) -> bool {
    let limit = version_at(now + MAX_AHEAD_MS);
    page.rows.iter().any(|r| r.v.0 > limit)
}

/// `sync_hlc()` for the change triggers, on connection `conn`, backed by
/// `clock`.
pub(crate) fn register_clock(conn: &Connection, clock: Arc<AtomicI64>) -> Result<()> {
    use rusqlite::functions::FunctionFlags;
    conn.create_scalar_function("sync_hlc", 0, FunctionFlags::SQLITE_UTF8, move |_| Ok(hlc_next(&clock, now_ms())))?;
    Ok(())
}

// ----------------------------------------------------------------- tables

/// What part of the library a synced table is (the order of a first sync).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncGroup {
    Catalog,
    UserData,
    History,
    Derived,
}

/// What a column points at, for the aliases of ids another device made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Artist,
    Release,
    Source,
    /// By the value of this column: `artist`, `release`.
    ByKind(&'static str),
    /// A credit (JSON) naming artists by id.
    Credit,
}

/// How a row of another device is recognised as one this device has under
/// another id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Identity {
    None,
    /// The same MusicBrainz id, or the same folded name.
    Artist,
    /// The same MusicBrainz id, or the same folded title and artist line.
    Release,
    /// The same stream (kind and locator).
    Source,
}

/// A synced table.
#[derive(Debug)]
pub struct SyncTable {
    pub name: &'static str,
    pub keys: &'static [&'static str],
    /// The synced columns besides the key (local ones, like a track's
    /// `has_local` or a row's `seq`, are not here).
    pub cols: &'static [&'static str],
    /// A column with the row's last edit in wall-clock ms: the version of a
    /// row noted before sync existed (none: 0).
    pub stamp: Option<&'static str>,
    /// A flag column: between two live rows, the one with it set wins
    /// whatever the versions.
    pub prefer: Option<&'static str>,
    /// Columns pointing at artists, albums, sources (keys too).
    pub refs: &'static [(&'static str, Target)],
    pub identity: Identity,
    /// The rows that travel, `{r}` standing for the row.
    filter: Option<&'static str>,
    /// When a row gone elsewhere may go here too (`?1` the first key
    /// column); none: always.
    guard: Option<&'static str>,
    pub group: SyncGroup,
}

const fn table(name: &'static str, keys: &'static [&'static str], cols: &'static [&'static str], group: SyncGroup) -> SyncTable {
    SyncTable { name, keys, cols, stamp: None, prefer: None, refs: &[], identity: Identity::None, filter: None, guard: None, group }
}

/// User data versioned by its own `updated_at`.
const fn own(name: &'static str, keys: &'static [&'static str], cols: &'static [&'static str]) -> SyncTable {
    SyncTable { stamp: Some("updated_at"), ..table(name, keys, cols, SyncGroup::UserData) }
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
        refs: &[("id", Target::Artist)],
        identity: Identity::Artist,
        ..table("artist", &["id"], &["mbid", "name", "sort_name", "sort_key", "images", "added_at"], Catalog)
    },
    SyncTable {
        guard: Some("NOT EXISTS (SELECT 1 FROM track WHERE release_id = ?1)"),
        refs: &[("id", Target::Release), ("artist_credit", Target::Credit)],
        identity: Identity::Release,
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
        refs: &[("release_id", Target::Release), ("artist_credit", Target::Credit)],
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
    SyncTable {
        refs: &[("release_id", Target::Release), ("artist_id", Target::Artist)],
        ..table("release_artist", &["release_id", "artist_id"], &["position"], Catalog)
    },
    SyncTable { refs: &[("artist_id", Target::Artist)], ..table("track_artist", &["track_id", "artist_id"], &["position"], Catalog) },
    SyncTable { refs: &[("artist_id", Target::Artist)], ..table("artist_alias", &["key", "artist_id"], &["name", "source"], Catalog) },
    table("track_alias", &["key", "track_id"], &["title", "source"], Catalog),
    // The streams' sources (a local file's path is this device's own; which
    // device has which file is `node_file`). Which source plays is each
    // device's own choice (`is_active`, chosen again after a merge).
    SyncTable {
        filter: Some("{r}.kind <> 'local_file'"),
        refs: &[("id", Target::Source)],
        identity: Identity::Source,
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
    SyncTable {
        refs: &[("local_id", Target::ByKind("kind"))],
        ..table("external_id", &["service", "kind", "remote_id"], &["local_id"], Catalog)
    },
    own("playlist", &["id"], &["name", "cover", "created_at", "updated_at", "updated_by", "deleted"]),
    own("playlist_entry", &["id"], &["playlist_id", "track_id", "position", "added_at", "updated_at", "updated_by", "deleted"]),
    SyncTable {
        refs: &[("target_id", Target::ByKind("target_kind"))],
        ..own("likes", &["target_kind", "target_id"], &["updated_at", "updated_by", "deleted"])
    },
    SyncTable {
        refs: &[("release_id", Target::Release)],
        ..own("release_selection", &["release_id"], &["mode", "view", "versions", "updated_at", "updated_by", "deleted"])
    },
    SyncTable {
        refs: &[("release_id", Target::Release)],
        ..own("release_track_selection", &["release_id", "track_id"], &["choice", "updated_at", "updated_by", "deleted"])
    },
    own("track_put", &["track_id"], &["updated_at", "updated_by", "deleted"]),
    SyncTable {
        stamp: Some("updated_at"),
        refs: &[("target_id", Target::ByKind("target_kind"))],
        ..table("rec_feedback", &["target_kind", "target_id"], &["dislike", "updated_at"], UserData)
    },
    table("recording_link", &["track_id"], &["recording", "how", "at"], UserData),
    table("recording_apart", &["track_id", "other_track"], &["at"], UserData),
    SyncTable { stamp: Some("updated_at"), ..table("track_fx", &["track_id"], &["variant", "updated_at"], UserData) },
    table("lyrics_pick", &["track_id"], &["source"], UserData),
    table("lyrics_show", &["track_id"], &["sync"], UserData),
    SyncTable {
        refs: &[("release_id", Target::Release)],
        ..table(
            "unwanted",
            &["release_id", "title_key"],
            &["title", "artist", "disc_no", "track_no", "duration_ms", "isrc", "recording_mbid", "track_id", "at"],
            UserData,
        )
    },
    SyncTable {
        refs: &[("artist_id", Target::Artist)],
        ..table("artist_photo", &["artist_id"], &["path", "source", "fetched_at", "photo_locked"], UserData)
    },
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
    SyncTable {
        prefer: Some("saved"),
        ..table(
            "lyrics_align",
            &["track_id", "source"],
            &["text_hash", "fit", "timing", "offset_ms", "covered", "body", "made_by", "made_at", "saved", "fits"],
            Derived,
        )
    },
    table("lyrics_align_done", &["track_id"], &["state", "detail", "made_by", "at"], Derived),
    table("analysis", &["track_id"], &["bliss_vector", "analyzed_with", "analyzed_at", "source"], Derived),
    table("sound_embedding", &["track_id"], &["vector", "embedded_with", "source", "embedded_at"], Derived),
    table("sound_sketch", &["track_id"], &["sketch", "sketched_with", "styled_with", "source", "sketched_at"], Derived),
    SyncTable {
        refs: &[("release_id", Target::Release)],
        ..table("palette", &["release_id"], &["seed_argb", "scheme_blob", "ambient_blur", "computed_at"], Derived)
    },
    SyncTable { refs: &[("id", Target::ByKind("kind"))], ..table("picture", &["kind", "id"], &["hash"], Derived) },
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
    /// Its version: the version, the device, gone.
    pub v: (i64, String, bool),
    /// The values of [`SyncPage::cols`]; none for a row gone. Blobs are
    /// `{"b": "<base64>"}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub d: Option<Vec<Value>>,
}

/// How far each table's changes go, and which database it is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncHeads {
    /// Made once per database, and again when the database was put back
    /// from a copy: another one is taken from the beginning.
    pub epoch: String,
    pub tables: BTreeMap<String, i64>,
}

/// What a page did here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncApplied {
    /// Rows written (or deleted).
    pub taken: u32,
    /// Rows already as new here.
    pub kept: u32,
    /// Deletes held back (the row is still used here), kept to try again.
    pub held: u32,
    /// Rows that could not be written here, kept to try again.
    pub failed: u32,
    /// Kept conflicts that went through this time.
    pub resolved: u32,
}

/// How far this device has taken another one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncCursors {
    pub off: bool,
    pub epoch: Option<String>,
    pub tables: BTreeMap<String, i64>,
    pub last_ok: Option<i64>,
}

/// The rows of a device kept to try again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncConflicts {
    /// Rows that could not be written.
    pub failed: u32,
    /// Deletes held back (still used here).
    pub held: u32,
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

/// The change log's text of a key (as the triggers write it).
fn key_text(tx: &Connection, keys: &[SqlValue]) -> Result<String> {
    let marks: Vec<String> = (1..=keys.len()).map(|i| format!("?{i}")).collect();
    let sql = format!("SELECT json_array({})", marks.join(", "));
    Ok(tx.prepare_cached(&sql)?.query_row(params_from_iter(keys.iter()), |r| r.get(0))?)
}

fn text(v: Option<&SqlValue>) -> Option<&str> {
    match v {
        Some(SqlValue::Text(s)) => Some(s.as_str()),
        _ => None,
    }
}

fn is_constraint(e: &rusqlite::Error) -> bool {
    e.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation)
}

// -------------------------------------------------------------- triggers

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
/// `when` holds: a change number and a version of its own, then a delete
/// and an insert (never `OR REPLACE`: the statement that fired the trigger
/// would impose its own conflict rule on it).
fn note(t: &SyncTable, key: &str, gone: &str, when: &str) -> String {
    let n = t.name;
    format!(
        "UPDATE sync_clock SET seq = seq + 1, hlc = sync_hlc() WHERE id = 1 AND ({when}); \
         DELETE FROM change_log WHERE tbl = '{n}' AND key = {key} AND ({when}); \
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
        format!("CREATE TRIGGER sync_{n}_ai AFTER INSERT ON {n} WHEN {QUIET} AND ({fnew}) BEGIN {} END", note(t, &knew, "0", "1")),
        format!(
            "CREATE TRIGGER sync_{n}_au AFTER UPDATE ON {n} WHEN {QUIET} AND (({fnew}) OR ({fold})) AND ({changed}) BEGIN {} {} END",
            note(t, &kold, "1", &format!("{kold} IS NOT {knew}")),
            note(t, &knew, &format!("CASE WHEN ({fnew}) THEN 0 ELSE 1 END"), "1"),
        ),
        format!("CREATE TRIGGER sync_{n}_ad AFTER DELETE ON {n} WHEN {QUIET} AND ({fold}) BEGIN {} END", note(t, &kold, "1", "1")),
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
/// change number and the version it can be trusted with: its own last edit
/// ([`SyncTable::stamp`]), else 0. Never an edit.
fn backfill(tx: &Connection) -> Result<()> {
    for t in TABLES {
        let key = key_expr(t, "x");
        let version = t.stamp.map_or_else(|| "0".to_owned(), |s| format!("coalesce(x.{s}, 0) * {}", 1i64 << LOGICAL_BITS));
        let n = tx.execute(
            &format!(
                "INSERT INTO change_log(tbl, key, seq, hlc, node, gone, came_from)
                 SELECT '{name}', {key}, c.seq + row_number() OVER (), {version}, c.node, 0, NULL
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

/// The epoch kept beside the database file (`<db>.sync-epoch`).
pub(crate) fn epoch_file_of(db: &Path) -> PathBuf {
    let mut name = db.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".sync-epoch");
    db.with_file_name(name)
}

/// Readies sync on an open, migrated database: this device's id for the
/// change log, the triggers (made again when [`TABLES`] changed, and then
/// the rows not noted yet), the database's epoch — a new one when the copy
/// beside the database (`epoch_file`) says another (the database was put
/// back from an older copy: the other devices take it from the start). The
/// clock's last version.
pub(crate) fn install(conn: &mut Connection, device: &str, epoch_file: Option<&Path>) -> Result<i64> {
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
    let mut epoch = meta_get(&tx, "sync.epoch")?.unwrap_or_default();
    let kept = epoch_file.and_then(|f| std::fs::read_to_string(f).ok()).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    if epoch.is_empty() || kept.as_ref().is_some_and(|k| *k != epoch) {
        if !epoch.is_empty() {
            log::warn!("library: the database is not the one last opened here (put back from a copy?): a new sync epoch");
        }
        epoch = Uuid::now_v7().simple().to_string();
        meta_set(&tx, "sync.epoch", &epoch)?;
    }
    let hlc: i64 = tx.query_row("SELECT hlc FROM sync_clock WHERE id = 1", [], |r| r.get(0))?;
    tx.commit()?;
    if let Some(f) = epoch_file
        && kept.as_deref() != Some(epoch.as_str())
    {
        let tmp = f.with_extension("sync-epoch.tmp");
        if let Err(e) = std::fs::write(&tmp, &epoch).and_then(|()| std::fs::rename(&tmp, f)) {
            log::warn!("library: the sync epoch not kept beside the database: {e}");
        }
    }
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
        let v = (hlc, node, data.is_none());
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

/// Does an incoming row of version `inc` replace the row here of version
/// `here` (each `(version, gone, device)`)? The larger version; on an
/// equal version 0 (both noted before sync) the row here stays; on another
/// equal version a tombstone, then the larger device id.
pub(crate) fn wins(inc: (i64, bool, &str), here: (i64, bool, &str)) -> bool {
    if inc.0 != here.0 {
        return inc.0 > here.0;
    }
    if inc.0 == 0 {
        return false;
    }
    if inc.1 != here.1 {
        return inc.1;
    }
    inc.2 > here.2
}

/// What became of one incoming row.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Outcome {
    Taken,
    Kept,
    /// A delete held back: the row is still used here.
    Held(String),
    /// It could not be written here.
    Failed(String),
}

/// What a merge keeps while it runs.
struct Ctx {
    /// The device the rows come from.
    from: String,
    seq: i64,
    seen: i64,
    /// Tracks whose stream sources changed.
    tracks: HashSet<String>,
    /// This device's own artists (not taken from `from`) by folded name:
    /// (id, MusicBrainz id).
    artists: Option<HashMap<String, (String, Option<String>)>>,
    /// …and albums by folded title and artist line.
    releases: Option<HashMap<(String, String), (String, Option<String>)>>,
}

impl Ctx {
    fn new(tx: &Connection, from: &str) -> Result<Self> {
        let seq: i64 = tx.query_row("SELECT seq FROM sync_clock WHERE id = 1", [], |r| r.get(0))?;
        Ok(Self { from: from.to_owned(), seq, seen: 0, tracks: HashSet::new(), artists: None, releases: None })
    }

    fn artists(&mut self, tx: &Connection) -> Result<&HashMap<String, (String, Option<String>)>> {
        if self.artists.is_none() {
            let mut stmt = tx.prepare_cached(
                "SELECT a.id, a.name, a.mbid FROM artist a
                   LEFT JOIN change_log l ON l.tbl = 'artist' AND l.key = json_array(a.id)
                  WHERE l.came_from IS NULL OR l.came_from <> ?1",
            )?;
            let rows = stmt.query_map([&self.from], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)))?;
            let mut map = HashMap::new();
            for row in rows {
                let (id, name, mbid) = row?;
                let k = name_key(&name);
                if !k.is_empty() {
                    map.entry(k).or_insert((id, mbid));
                }
            }
            self.artists = Some(map);
        }
        Ok(self.artists.as_ref().expect("filled"))
    }

    fn releases(&mut self, tx: &Connection) -> Result<&HashMap<(String, String), (String, Option<String>)>> {
        if self.releases.is_none() {
            let mut stmt = tx.prepare_cached(
                "SELECT r.id, r.title, r.artist_display, r.mbid FROM release r
                   LEFT JOIN change_log l ON l.tbl = 'release' AND l.key = json_array(r.id)
                  WHERE l.came_from IS NULL OR l.came_from <> ?1",
            )?;
            let rows = stmt.query_map([&self.from], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?))
            })?;
            let mut map = HashMap::new();
            for row in rows {
                let (id, title, artist, mbid) = row?;
                let k = (name_key(&title), name_key(&artist));
                if !k.0.is_empty() {
                    map.entry(k).or_insert((id, mbid));
                }
            }
            self.releases = Some(map);
        }
        Ok(self.releases.as_ref().expect("filled"))
    }
}

/// Where column `name` of `t` is: a key (index) or a synced column.
enum Place {
    Key(usize),
    Col(usize),
}

fn place(t: &SyncTable, name: &str) -> Option<Place> {
    if let Some(i) = t.keys.iter().position(|k| *k == name) {
        return Some(Place::Key(i));
    }
    t.cols.iter().position(|c| *c == name).map(Place::Col)
}

fn value<'v>(t: &SyncTable, keys: &'v [SqlValue], data: &'v [Option<SqlValue>], name: &str) -> Option<&'v SqlValue> {
    match place(t, name)? {
        Place::Key(i) => keys.get(i),
        Place::Col(i) => data.get(i).and_then(Option::as_ref),
    }
}

fn alias_of(tx: &Connection, table: &str, id: &str) -> Result<Option<String>> {
    Ok(tx
        .prepare_cached("SELECT local_id FROM sync_alias WHERE tbl = ?1 AND remote_id = ?2")?
        .query_row(params![table, id], |r| r.get(0))
        .optional()?)
}

/// Every reference of an incoming row through the aliases of ids another
/// device made.
fn remap(tx: &Connection, t: &SyncTable, keys: &mut [SqlValue], data: &mut [Option<SqlValue>]) -> Result<()> {
    for (col, target) in t.refs {
        let Some(id) = text(value(t, keys, data, col)).map(str::to_owned) else { continue };
        let table = match target {
            Target::Artist => "artist",
            Target::Release => "release",
            Target::Source => "source",
            Target::ByKind(kind) => match text(value(t, keys, data, kind)) {
                Some("artist") => "artist",
                Some("release") => "release",
                _ => continue,
            },
            Target::Credit => {
                let pairs: Vec<(String, String)> = {
                    let mut stmt =
                        tx.prepare_cached("SELECT remote_id, local_id FROM sync_alias WHERE tbl = 'artist' AND instr(?1, remote_id) > 0")?;
                    let rows = stmt.query_map([&id], |r| Ok((r.get(0)?, r.get(1)?)))?;
                    rows.collect::<rusqlite::Result<_>>()?
                };
                if let (false, Some(Place::Col(i))) = (pairs.is_empty(), place(t, col)) {
                    let credit = pairs.iter().fold(id, |c, (remote, local)| c.replace(remote.as_str(), local));
                    data[i] = Some(SqlValue::Text(credit));
                }
                continue;
            }
        };
        let Some(local) = alias_of(tx, table, &id)? else { continue };
        match place(t, col) {
            Some(Place::Key(i)) => keys[i] = SqlValue::Text(local),
            Some(Place::Col(i)) => data[i] = Some(SqlValue::Text(local)),
            None => {}
        }
    }
    Ok(())
}

/// A row here of another id that is what the incoming row of `t` is.
fn same_here(tx: &Connection, t: &SyncTable, keys: &[SqlValue], data: &[Option<SqlValue>], ctx: &mut Ctx) -> Result<Option<String>> {
    let Some(id) = text(keys.first()).map(str::to_owned) else { return Ok(None) };
    let col = |name: &str| text(value(t, keys, data, name)).map(str::to_owned);
    match t.identity {
        Identity::None => Ok(None),
        Identity::Artist | Identity::Release => {
            let table = t.name;
            let mbid = col("mbid").filter(|m| !m.is_empty());
            if let Some(m) = &mbid {
                let found: Option<String> = tx
                    .prepare_cached(&format!("SELECT id FROM {table} WHERE mbid = ?1 AND id <> ?2"))?
                    .query_row(params![m, id], |r| r.get(0))
                    .optional()?;
                if found.is_some() {
                    return Ok(found);
                }
            }
            let hit = if t.identity == Identity::Artist {
                let k = name_key(&col("name").unwrap_or_default());
                if k.is_empty() {
                    return Ok(None);
                }
                ctx.artists(tx)?.get(&k).cloned()
            } else {
                let k = (name_key(&col("title").unwrap_or_default()), name_key(&col("artist_display").unwrap_or_default()));
                if k.0.is_empty() {
                    return Ok(None);
                }
                ctx.releases(tx)?.get(&k).cloned()
            };
            // Two MusicBrainz ids that differ are two things.
            Ok(hit.filter(|(local, local_mbid)| *local != id && (mbid.is_none() || local_mbid.is_none() || *local_mbid == mbid)).map(|h| h.0))
        }
        Identity::Source => {
            if value(t, keys, data, "trashed_at").is_some_and(|v| *v != SqlValue::Null) {
                return Ok(None);
            }
            let (Some(kind), Some(locator)) = (col("kind"), col("locator")) else { return Ok(None) };
            Ok(tx
                .prepare_cached("SELECT id FROM source WHERE kind = ?1 AND locator = ?2 AND trashed_at IS NULL AND id <> ?3")?
                .query_row(params![kind, locator, id], |r| r.get(0))
                .optional()?)
        }
    }
}

/// The version of the row of `key` here, if it has one: the change log's
/// (a row here the log does not know has version 0).
fn local_version(tx: &Connection, t: &SyncTable, key: &str, keys: &[SqlValue]) -> Result<Option<(i64, bool, String)>> {
    let logged: Option<(i64, bool, String)> = tx
        .prepare_cached("SELECT hlc, gone, node FROM change_log WHERE tbl = ?1 AND key = ?2")?
        .query_row(params![t.name, key], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0, r.get(2)?)))
        .optional()?;
    if logged.is_some() {
        return Ok(logged);
    }
    Ok(row_exists(tx, t, keys)?.then(|| (0, false, String::new())))
}

fn row_exists(tx: &Connection, t: &SyncTable, keys: &[SqlValue]) -> Result<bool> {
    let sql = format!("SELECT EXISTS (SELECT 1 FROM {} x WHERE {})", t.name, where_keys(t));
    Ok(tx.prepare_cached(&sql)?.query_row(params_from_iter(keys.iter()), |r| r.get(0))?)
}

/// The flag of [`SyncTable::prefer`] of the live row here (none: no row).
fn preferred_here(tx: &Connection, t: &SyncTable, col: &str, keys: &[SqlValue]) -> Result<Option<bool>> {
    let sql = format!("SELECT x.{col} FROM {} x WHERE {}", t.name, where_keys(t));
    let v: Option<i64> = tx.prepare_cached(&sql)?.query_row(params_from_iter(keys.iter()), |r| r.get(0)).optional()?;
    Ok(v.map(|v| v != 0))
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
    format!("DELETE FROM {} WHERE {}", t.name, keys.join(" AND "))
}

/// Merges one row of `ctx.from` (`cols` naming its values).
fn apply_row(tx: &Connection, t: &SyncTable, cols: &[String], row: &SyncRow, ctx: &mut Ctx) -> Result<Outcome> {
    let mut keys = match key_values(&row.k) {
        Ok(k) if k.len() == t.keys.len() => k,
        _ => return Ok(Outcome::Failed("a key that is not this table's".into())),
    };
    let mut data: Vec<Option<SqlValue>> = vec![None; t.cols.len()];
    if let Some(d) = &row.d {
        for (i, c) in t.cols.iter().enumerate() {
            if let Some(p) = cols.iter().position(|x| x == c) {
                match sql_of(d.get(p).unwrap_or(&Value::Null)) {
                    Ok(v) => data[i] = Some(v),
                    Err(e) => return Ok(Outcome::Failed(e.to_string())),
                }
            }
        }
    }
    let gone = row.d.is_none() || row.v.2;
    let (version, node) = (row.v.0, row.v.1.as_str());
    remap(tx, t, &mut keys, &mut data)?;
    if !gone
        && t.identity != Identity::None
        && !row_exists(tx, t, &keys)?
        && let Some(local) = same_here(tx, t, &keys, &data, ctx)?
    {
        let remote = text(keys.first()).unwrap_or_default().to_owned();
        tx.execute(
            "INSERT INTO sync_alias(tbl, remote_id, local_id, peer, at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(tbl, remote_id) DO UPDATE SET local_id = excluded.local_id, peer = excluded.peer, at = excluded.at",
            params![t.name, remote, local, ctx.from, now_ms()],
        )?;
        log::info!("sync: {} {remote} of {} is {local} here", t.name, ctx.from);
        // The row here stays as it is (its own ids, names, MusicBrainz id);
        // what points at the other id comes here through the alias.
        return Ok(Outcome::Kept);
    }
    let key = key_text(tx, &keys)?;
    let here = local_version(tx, t, &key, &keys)?;
    let take = match &here {
        None => true,
        Some((at, here_gone, by)) => {
            let flags = match t.prefer.and_then(|p| place(t, p).map(|pl| (p, pl))) {
                Some((col, Place::Col(i))) if !gone && !*here_gone => {
                    let incoming = matches!(data[i], Some(SqlValue::Integer(v)) if v != 0);
                    preferred_here(tx, t, col, &keys)?.map(|h| (incoming, h))
                }
                _ => None,
            };
            match flags {
                Some((incoming, here_flag)) if incoming != here_flag => incoming,
                _ => wins((version, gone, node), (*at, *here_gone, by.as_str())),
            }
        }
    };
    if !take {
        return Ok(Outcome::Kept);
    }
    // The tracks whose sources change (their playing source is chosen
    // again): the one the source was on here, the one it is on now.
    let mut source_tracks: Vec<String> = Vec::new();
    if t.name == "source" {
        let was: Option<String> =
            tx.prepare_cached("SELECT track_id FROM source WHERE id = ?1")?.query_row([&keys[0]], |r| r.get(0)).optional()?;
        let now = if gone { None } else { text(value(t, &keys, &data, "track_id")).map(str::to_owned) };
        if was.is_some() && was != now {
            // Moved to another track: it plays there only once chosen there.
            tx.prepare_cached("UPDATE source SET is_active = 0 WHERE id = ?1")?.execute([&keys[0]])?;
        }
        source_tracks.extend(was);
        source_tracks.extend(now);
    }
    // Chosen again whatever comes of the write below.
    ctx.tracks.extend(source_tracks);
    let written = if gone {
        if row_exists(tx, t, &keys)? {
            if let Some(g) = t.guard {
                let may: bool = tx.prepare_cached(&format!("SELECT ({g})"))?.query_row([&keys[0]], |r| r.get(0))?;
                if !may {
                    return Ok(Outcome::Held("still used here".into()));
                }
            }
            tx.prepare_cached(&delete_sql(t))?.execute(params_from_iter(keys.iter())).map(|_| ())
        } else {
            Ok(())
        }
    } else {
        let present: Vec<usize> = (0..t.cols.len()).filter(|i| data[*i].is_some()).collect();
        let names: Vec<&str> = present.iter().map(|i| t.cols[*i]).collect();
        let mut vals = keys.clone();
        for i in &present {
            vals.push(data[*i].clone().unwrap_or(SqlValue::Null));
        }
        tx.prepare_cached(&upsert_sql(t, &names))?.execute(params_from_iter(vals.iter())).map(|_| ())
    };
    match written {
        Ok(()) => {}
        Err(e) if is_constraint(&e) => return Ok(Outcome::Failed(e.to_string())),
        Err(e) => return Err(e.into()),
    }
    ctx.seq += 1;
    tx.execute("DELETE FROM change_log WHERE tbl = ?1 AND key = ?2", params![t.name, key])?;
    tx.execute(
        "INSERT INTO change_log(tbl, key, seq, hlc, node, gone, came_from) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![t.name, key, ctx.seq, version, node, gone, ctx.from],
    )?;
    ctx.seen = ctx.seen.max(version);
    Ok(Outcome::Taken)
}

/// Keeps a row that did not go through to try again, or forgets the one
/// kept when it went through (or turned out older than what is here).
fn record(tx: &Connection, t: &SyncTable, cols: &[String], row: &SyncRow, from: &str, outcome: &Outcome) -> Result<()> {
    match outcome {
        Outcome::Taken | Outcome::Kept => {
            tx.prepare_cached("DELETE FROM sync_conflict WHERE tbl = ?1 AND key = ?2 AND peer = ?3")?.execute(params![t.name, row.k, from])?;
        }
        Outcome::Held(why) | Outcome::Failed(why) => {
            let now = now_ms();
            tx.prepare_cached(
                "INSERT INTO sync_conflict(tbl, key, peer, cols, row, held, reason, first_seen, last_try, tries)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, 1)
                 ON CONFLICT(tbl, key, peer) DO UPDATE SET cols = excluded.cols, row = excluded.row, held = excluded.held,
                    reason = excluded.reason, last_try = excluded.last_try, tries = sync_conflict.tries + 1",
            )?
            .execute(params![
                t.name,
                row.k,
                from,
                serde_json::to_string(cols)?,
                serde_json::to_string(row)?,
                matches!(outcome, Outcome::Held(_)),
                why,
                now
            ])?;
        }
    }
    Ok(())
}

fn count(out: &mut SyncApplied, o: &Outcome) {
    match o {
        Outcome::Taken => out.taken += 1,
        Outcome::Kept => out.kept += 1,
        Outcome::Held(_) => out.held += 1,
        Outcome::Failed(_) => out.failed += 1,
    }
}

/// Tries the kept rows (of any device) last tried at `before` or earlier
/// again, at most `limit`. How many went through (or turned out older).
fn retry_in(tx: &Connection, ctx: &mut Ctx, before: i64, limit: u32) -> Result<u32> {
    type Kept = (String, String, String, String, String);
    let kept: Vec<Kept> = {
        let mut stmt = tx.prepare_cached(
            "SELECT tbl, key, peer, cols, row FROM sync_conflict WHERE last_try <= ?1 ORDER BY last_try, tbl, key LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![before, limit], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mine = ctx.from.clone();
    let mut resolved = 0;
    for (tbl, key, peer, cols, row) in kept {
        let parsed = sync_table(&tbl).and_then(|t| Some((t, serde_json::from_str::<Vec<String>>(&cols).ok()?, serde_json::from_str::<SyncRow>(&row).ok()?)));
        let Some((t, cols, row)) = parsed else {
            tx.execute("DELETE FROM sync_conflict WHERE tbl = ?1 AND key = ?2 AND peer = ?3", params![tbl, key, peer])?;
            continue;
        };
        if ctx.from != peer {
            ctx.from = peer.clone();
            ctx.artists = None;
            ctx.releases = None;
        }
        let o = apply_row(tx, t, &cols, &row, ctx)?;
        if matches!(o, Outcome::Taken | Outcome::Kept) {
            resolved += 1;
        }
        record(tx, t, &cols, &row, &peer, &o)?;
    }
    if ctx.from != mine {
        ctx.from = mine;
        ctx.artists = None;
        ctx.releases = None;
    }
    Ok(resolved)
}

/// Ends a merge: the changed tracks' playing source chosen again, the
/// clock kept.
fn finish(tx: &Connection, ctx: &mut Ctx) -> Result<()> {
    for track in ctx.tracks.drain() {
        if let Ok(id) = track.parse::<TrackId>() {
            crate::catalog::refresh_active(tx, id)?;
        }
    }
    tx.execute("UPDATE sync_clock SET seq = ?1, hlc = max(hlc, ?2), applying = 0 WHERE id = 1", params![ctx.seq, ctx.seen])?;
    Ok(())
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

    /// Merges a page of device `from`'s changes in, in one transaction,
    /// then tries the rows kept from before again; with `cursor`, notes in
    /// it that `from`'s table is taken up to the page's end (a row that did
    /// not go through is kept, so the cursor may pass it). Idempotent. A
    /// page of a clock running ahead ([`page_ahead`]) is refused whole.
    pub fn sync_apply(&self, page: &SyncPage, from: &str, cursor: bool) -> Result<SyncApplied> {
        let t = sync_table(&page.table).ok_or_else(|| LibraryError::Invalid(format!("no synced table {}", page.table)))?;
        let now = now_ms();
        if page_ahead(page, now) {
            return Err(LibraryError::Invalid(format!("the clock of {from} runs ahead")));
        }
        let (out, seen) = self.write(|tx| {
            tx.execute("UPDATE sync_clock SET applying = 1 WHERE id = 1", [])?;
            let mut ctx = Ctx::new(tx, from)?;
            let mut out = SyncApplied::default();
            for row in &page.rows {
                let o = apply_row(tx, t, &page.cols, row, &mut ctx)?;
                record(tx, t, &page.cols, row, from, &o)?;
                count(&mut out, &o);
            }
            out.resolved = retry_in(tx, &mut ctx, now, RETRY_BATCH)?;
            if cursor {
                tx.execute(
                    "INSERT INTO sync_peer(node, tbl, seq) VALUES (?1, ?2, ?3)
                     ON CONFLICT(node, tbl) DO UPDATE SET seq = excluded.seq",
                    params![from, t.name, page.next],
                )?;
            }
            finish(tx, &mut ctx)?;
            Ok((out, ctx.seen))
        })?;
        if seen > 0 {
            self.observe_clock(seen);
        }
        Ok(out)
    }

    /// Tries every kept row again (the loop's turn: what blocked one may
    /// have changed here). How many went through.
    pub fn sync_retry(&self) -> Result<u32> {
        let any: bool = self.read(|c| Ok(c.query_row("SELECT EXISTS (SELECT 1 FROM sync_conflict)", [], |r| r.get(0))?))?;
        if !any {
            return Ok(0);
        }
        let now = now_ms();
        let (resolved, seen) = self.write(|tx| {
            tx.execute("UPDATE sync_clock SET applying = 1 WHERE id = 1", [])?;
            let mut ctx = Ctx::new(tx, "")?;
            let resolved = retry_in(tx, &mut ctx, now, RETRY_BATCH)?;
            finish(tx, &mut ctx)?;
            Ok((resolved, ctx.seen))
        })?;
        if seen > 0 {
            self.observe_clock(seen);
        }
        Ok(resolved)
    }

    /// The rows of device `node` kept to try again.
    pub fn sync_conflicts(&self, node: &str) -> Result<SyncConflicts> {
        self.read(|c| {
            let (failed, held): (i64, i64) = c.query_row(
                "SELECT coalesce(sum(held = 0), 0), coalesce(sum(held = 1), 0) FROM sync_conflict WHERE peer = ?1",
                [node],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            Ok(SyncConflicts { failed: failed as u32, held: held as u32 })
        })
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

    // ------------------------------------------------------------ pictures

    /// Covers and artist photos with a file here not hashed yet (or whose
    /// reference changed): (`release` | `artist`, id, the reference as the
    /// row has it), at most `limit`.
    pub fn pictures_to_index(&self, limit: u32) -> Result<Vec<(String, String, String)>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT 'release', r.id, r.cover_ref FROM release r
                   LEFT JOIN picture_src s ON s.kind = 'release' AND s.id = r.id
                  WHERE r.cover_ref IS NOT NULL AND r.cover_ref NOT LIKE 'http%' AND s.ref IS NOT r.cover_ref
                 UNION ALL
                 SELECT 'artist', p.artist_id, p.path FROM artist_photo p
                   LEFT JOIN picture_src s ON s.kind = 'artist' AND s.id = p.artist_id
                  WHERE p.path IS NOT NULL AND p.path NOT LIKE 'http%' AND s.ref IS NOT p.path
                 LIMIT ?1",
            )?;
            let rows = stmt.query_map([limit], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// What the picture of `kind`/`id` (reference `reference`) is: its hash
    /// and the file here (`None`: no file here — the picture a device sent
    /// stays as it is).
    pub fn put_picture(&self, kind: &str, id: &str, reference: &str, found: Option<(&str, &Path)>) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO picture_src(kind, id, ref) VALUES (?1, ?2, ?3) ON CONFLICT(kind, id) DO UPDATE SET ref = excluded.ref",
                params![kind, id, reference],
            )?;
            if let Some((hash, path)) = found {
                tx.execute(
                    "INSERT INTO picture(kind, id, hash) VALUES (?1, ?2, ?3) ON CONFLICT(kind, id) DO UPDATE SET hash = excluded.hash",
                    params![kind, id, hash],
                )?;
                tx.execute(
                    "INSERT INTO picture_file(hash, path) VALUES (?1, ?2) ON CONFLICT(hash) DO UPDATE SET path = excluded.path",
                    params![hash, path.to_string_lossy()],
                )?;
            }
            Ok(())
        })
    }

    /// Pictures the synced rows name that have no file here: their hashes,
    /// at most `limit`.
    pub fn pictures_missing(&self, limit: u32) -> Result<Vec<String>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT DISTINCT p.hash FROM picture p LEFT JOIN picture_file f ON f.hash = p.hash WHERE f.hash IS NULL LIMIT ?1",
            )?;
            let rows = stmt.query_map([limit], |r| r.get(0))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// The file of picture `hash` is here now, at `path`.
    pub fn put_picture_file(&self, hash: &str, path: &Path) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO picture_file(hash, path) VALUES (?1, ?2) ON CONFLICT(hash) DO UPDATE SET path = excluded.path",
                params![hash, path.to_string_lossy()],
            )?;
            Ok(())
        })
    }

    /// Where the file of picture `hash` is here.
    pub fn picture_file(&self, hash: &str) -> Result<Option<PathBuf>> {
        self.read(|c| {
            let p: Option<String> =
                c.prepare_cached("SELECT path FROM picture_file WHERE hash = ?1")?.query_row([hash], |r| r.get(0)).optional()?;
            Ok(p.map(PathBuf::from))
        })
    }

    /// The files here of the pictures of `kind` (`release` | `artist`), by
    /// id.
    pub fn local_pictures(&self, kind: &str) -> Result<HashMap<String, PathBuf>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached("SELECT p.id, f.path FROM picture p JOIN picture_file f ON f.hash = p.hash WHERE p.kind = ?1")?;
            let rows = stmt.query_map([kind], |r| Ok((r.get::<_, String>(0)?, PathBuf::from(r.get::<_, String>(1)?))))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// The local copy of the picture of `kind` (`release` | `artist`) `id`,
    /// for a cover or photo whose own file is not here.
    pub fn local_picture(&self, kind: &str, id: &str) -> Result<Option<PathBuf>> {
        self.read(|c| {
            let p: Option<String> = c
                .prepare_cached("SELECT f.path FROM picture p JOIN picture_file f ON f.hash = p.hash WHERE p.kind = ?1 AND p.id = ?2")?
                .query_row(params![kind, id], |r| r.get(0))
                .optional()?;
            Ok(p.map(PathBuf::from))
        })
    }
}

/// Takes a test database back before the mesh's and sync's migrations
/// (34, 35): their triggers, tables and columns go, so older migrations
/// can run again on it.
#[cfg(test)]
pub(crate) fn rewind_before_mesh(conn: &Connection) {
    let triggers: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'trigger' AND (name LIKE 'sync!_%' ESCAPE '!' OR name LIKE 'node!_file!_%' ESCAPE '!')")
            .unwrap();
        stmt.query_map([], |r| r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap()
    };
    for t in triggers {
        conn.execute_batch(&format!("DROP TRIGGER \"{t}\"")).unwrap();
    }
    conn.execute_batch(
        "DROP TABLE change_log; DROP TABLE sync_clock; DROP TABLE sync_peer; DROP TABLE sync_node; DROP TABLE sync_conflict;
         DROP TABLE sync_alias; DROP TABLE picture; DROP TABLE picture_file; DROP TABLE picture_src; DROP TABLE node_file;
         ALTER TABLE event DROP COLUMN actor; ALTER TABLE lyrics_align DROP COLUMN fits;
         ALTER TABLE node DROP COLUMN alias; ALTER TABLE node DROP COLUMN scope;
         ALTER TABLE node DROP COLUMN profile; ALTER TABLE node DROP COLUMN addr;
         ALTER TABLE node DROP COLUMN version; ALTER TABLE node DROP COLUMN cert;
         ALTER TABLE node DROP COLUMN paired_at;
         DELETE FROM meta WHERE key IN ('sync.triggers', 'sync.epoch')",
    )
    .unwrap();
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
