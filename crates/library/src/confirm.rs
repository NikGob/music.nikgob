//! «Подтверди»: files the importer parked for the user (table
//! `confirm_item`), kept across restarts. The payload is the importer's;
//! the library only keeps it. Not user data: not exported, not synced.

use nikgob_core::model::WishlistId;
use rusqlite::params;

use crate::db::Library;
use crate::error::Result;
use crate::types::ConfirmRow;
use crate::util::{get_opt_id, now_ms};

impl Library {
    /// Stores a parked file (a file parked again replaces its old row);
    /// returns its id.
    pub fn put_confirm_item(&self, path: &str, wishlist: Option<WishlistId>, payload: &str) -> Result<i64> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO confirm_item(path, wishlist_id, payload, created_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(path) DO UPDATE SET wishlist_id = excluded.wishlist_id,
                        payload = excluded.payload",
                params![path, wishlist.map(|w| w.to_string()), payload, now_ms()],
            )?;
            Ok(tx.query_row("SELECT id FROM confirm_item WHERE path = ?1", [path], |r| r.get(0))?)
        })
    }

    /// Every parked file, oldest first.
    pub fn confirm_items(&self) -> Result<Vec<ConfirmRow>> {
        self.read(|c| {
            let mut stmt = c.prepare(
                "SELECT id, path, wishlist_id, payload, created_at FROM confirm_item ORDER BY id",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(ConfirmRow {
                    id: r.get(0)?,
                    path: r.get(1)?,
                    wishlist: get_opt_id(r, 2)?,
                    payload: r.get(3)?,
                    created_at: r.get(4)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// The file was resolved (or is gone). Returns whether it was parked.
    pub fn delete_confirm_item(&self, id: i64) -> Result<bool> {
        self.write(|tx| Ok(tx.execute("DELETE FROM confirm_item WHERE id = ?1", [id])? > 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parked_files_survive_a_reopen_and_go_when_resolved() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("library.sqlite");
        let wish = WishlistId::new();
        let id = {
            let lib = Library::open(&db).unwrap();
            let id = lib.put_confirm_item("C:/dl/1.flac", Some(wish), "{}").unwrap();
            assert_eq!(lib.put_confirm_item("C:/dl/1.flac", Some(wish), "{\"a\":1}").unwrap(), id, "same file, same row");
            lib.put_confirm_item("C:/dl/2.flac", None, "{}").unwrap();
            id
        };
        let lib = Library::open(&db).unwrap();
        let rows = lib.confirm_items().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].id, rows[0].wishlist, rows[0].payload.as_str()), (id, Some(wish), "{\"a\":1}"));
        assert!(lib.delete_confirm_item(id).unwrap());
        assert!(!lib.delete_confirm_item(id).unwrap());
        assert_eq!(lib.confirm_items().unwrap().len(), 1);
    }
}
