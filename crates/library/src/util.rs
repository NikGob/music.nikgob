//! Small helpers shared by the modules.

use std::error::Error as StdError;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use nikgob_core::model::{ArtistCredit, UnixMs};
use rusqlite::types::Type;
use rusqlite::{Connection, Row};

use crate::error::Result;

/// Current unix time in milliseconds.
pub fn now_ms() -> UnixMs {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Case-insensitive (Unicode aware) sort key; `ё` sorts as `е`.
pub fn sort_key(s: &str) -> String {
    fold(s.trim())
}

/// Lowercase and `ё -> е`. This is what search terms go through; the stored
/// side is folded by the FTS triggers (lowercasing is done by the tokenizer).
pub fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| if c == 'ё' { 'е' } else { c })
        .collect()
}

fn conv_err(idx: usize, ty: Type, e: impl StdError + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(idx, ty, Box::new(e))
}

pub(crate) fn get_id<T>(row: &Row<'_>, idx: usize) -> rusqlite::Result<T>
where
    T: FromStr,
    T::Err: StdError + Send + Sync + 'static,
{
    let s: String = row.get(idx)?;
    s.parse().map_err(|e| conv_err(idx, Type::Text, e))
}

pub(crate) fn get_opt_id<T>(row: &Row<'_>, idx: usize) -> rusqlite::Result<Option<T>>
where
    T: FromStr,
    T::Err: StdError + Send + Sync + 'static,
{
    let s: Option<String> = row.get(idx)?;
    s.map(|s| s.parse().map_err(|e| conv_err(idx, Type::Text, e)))
        .transpose()
}

pub(crate) fn get_enum<T>(row: &Row<'_>, idx: usize) -> rusqlite::Result<T>
where
    T: FromStr,
    T::Err: StdError + Send + Sync + 'static,
{
    get_id(row, idx)
}

pub(crate) fn get_opt_enum<T>(row: &Row<'_>, idx: usize) -> rusqlite::Result<Option<T>>
where
    T: FromStr,
    T::Err: StdError + Send + Sync + 'static,
{
    get_opt_id(row, idx)
}

pub(crate) fn get_json<T: serde::de::DeserializeOwned>(
    row: &Row<'_>,
    idx: usize,
) -> rusqlite::Result<T> {
    let s: String = row.get(idx)?;
    serde_json::from_str(&s).map_err(|e| conv_err(idx, Type::Text, e))
}

/// Stores `Option<u32>` etc. columns that SQLite hands back as `i64`.
pub(crate) fn opt_u32(row: &Row<'_>, idx: usize) -> rusqlite::Result<Option<u32>> {
    Ok(row.get::<_, Option<i64>>(idx)?.map(|v| v.max(0) as u32))
}

/// `(display, sort key)` of a credit.
pub(crate) fn credit_columns(credit: &ArtistCredit) -> Result<(String, String, String)> {
    let json = serde_json::to_string(credit)?;
    let display = credit.display();
    let key = sort_key(&display);
    Ok((json, display, key))
}

pub(crate) fn meta_get(conn: &Connection, key: &str) -> Result<Option<String>> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .optional()?)
}

pub(crate) fn meta_set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [key, value],
    )?;
    Ok(())
}
