//! Backup bookkeeping: lossless masters and their replicas, torrent downloads
//! and the wishlist (tables `master`, `replica`, `download`, `wishlist`).
//!
//! These rows carry no foreign keys and are not part of the userdata export:
//! masters/downloads are rebuilt from `.musiclib/masters` sidecars and the
//! torrent client, so they stay out of the CRDT-synced user data.

use nikgob_core::model::{
    Codec, DeviceId, Download, DownloadStatus, Master, Replica, SeedPolicy, SourceOrigin, TrackId,
    WishlistId, WishlistItem, WishlistStatus,
};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::catalog::origin_columns;
use crate::db::Library;
use crate::error::{LibraryError, Result};
use crate::util::{get_enum, get_id, get_json, get_opt_id, now_ms};

const MASTER_COLS: &str =
    "track_id, codec, file_hash, size_bytes, origin_kind, origin_info_hash, origin_path";

/// Reads a master row (without replicas).
fn master_from_row(row: &Row<'_>) -> rusqlite::Result<Master> {
    let origin = match row.get::<_, Option<String>>(4)?.as_deref() {
        Some("torrent") => Some(SourceOrigin::Torrent {
            info_hash: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
            path_in_torrent: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
        }),
        Some("stream") => Some(SourceOrigin::Stream),
        Some(_) => Some(SourceOrigin::Inbox),
        None => None,
    };
    Ok(Master {
        track_id: get_id(row, 0)?,
        codec: Codec::parse(&row.get::<_, String>(1)?),
        file_hash: row.get(2)?,
        size_bytes: row.get::<_, i64>(3)? as u64,
        origin,
        replicas: Vec::new(),
    })
}

fn replicas_of(conn: &Connection, file_hash: &str) -> Result<Vec<Replica>> {
    let mut stmt = conn.prepare(
        "SELECT file_hash, node_id, verified_at FROM replica WHERE file_hash = ?1 ORDER BY node_id",
    )?;
    let rows = stmt.query_map([file_hash], |r| {
        Ok(Replica {
            file_hash: r.get(0)?,
            node_id: DeviceId(r.get(1)?),
            verified_at: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Loads replicas for every master in the list.
fn with_replicas(conn: &Connection, mut masters: Vec<Master>) -> Result<Vec<Master>> {
    for m in &mut masters {
        m.replicas = replicas_of(conn, &m.file_hash)?;
    }
    Ok(masters)
}

fn upsert_replica(conn: &Connection, r: &Replica) -> Result<()> {
    conn.execute(
        "INSERT INTO replica(file_hash, node_id, verified_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(file_hash, node_id) DO UPDATE SET verified_at = excluded.verified_at",
        params![r.file_hash, r.node_id.as_str(), r.verified_at],
    )?;
    Ok(())
}

const DOWNLOAD_COLS: &str = "info_hash, magnet, torrent_file, selected_files, status, \
    seed_policy, bytes_up, bytes_down, wishlist_id, created_at, updated_at";

fn download_from_row(row: &Row<'_>) -> rusqlite::Result<Download> {
    Ok(Download {
        info_hash: row.get(0)?,
        magnet: row.get(1)?,
        torrent_file: row.get(2)?,
        selected_files: get_json(row, 3)?,
        status: get_enum::<DownloadStatus>(row, 4)?,
        seed_policy: get_enum::<SeedPolicy>(row, 5)?,
        bytes_up: row.get::<_, i64>(6)? as u64,
        bytes_down: row.get::<_, i64>(7)? as u64,
        wishlist_id: get_opt_id::<WishlistId>(row, 8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

const WISHLIST_COLS: &str = "id, title, artist, recording_mbid, isrc, spotify_id, youtube_id, \
    status, candidates_json, created_at, updated_at";

fn wishlist_from_row(row: &Row<'_>) -> rusqlite::Result<WishlistItem> {
    Ok(WishlistItem {
        id: get_id(row, 0)?,
        title: row.get(1)?,
        artist: row.get(2)?,
        recording_mbid: row.get(3)?,
        isrc: row.get(4)?,
        spotify_id: row.get(5)?,
        youtube_id: row.get(6)?,
        status: get_enum::<WishlistStatus>(row, 7)?,
        candidates_json: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

impl Library {
    // ------------------------------------------------------------ masters

    /// Inserts or replaces the master of `master.track_id` and upserts its
    /// replicas (replicas not listed are left alone).
    pub fn put_master(&self, master: &Master) -> Result<()> {
        self.write(|tx| {
            let (kind, info_hash, path) = match master.origin.as_ref().map(origin_columns) {
                Some((k, h, p)) => (Some(k), h, p),
                None => (None, None, None),
            };
            tx.execute(
                "INSERT INTO master(track_id, codec, file_hash, size_bytes,
                                    origin_kind, origin_info_hash, origin_path)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(track_id) DO UPDATE SET
                    codec = excluded.codec, file_hash = excluded.file_hash,
                    size_bytes = excluded.size_bytes, origin_kind = excluded.origin_kind,
                    origin_info_hash = excluded.origin_info_hash,
                    origin_path = excluded.origin_path",
                params![
                    master.track_id.to_string(),
                    master.codec.as_str(),
                    master.file_hash,
                    master.size_bytes as i64,
                    kind,
                    info_hash,
                    path
                ],
            )?;
            for r in &master.replicas {
                upsert_replica(tx, r)?;
            }
            Ok(())
        })
    }

    /// The master of a track, with its replicas.
    pub fn get_master(&self, track: TrackId) -> Result<Option<Master>> {
        self.read(|c| {
            let m = c
                .query_row(
                    &format!("SELECT {MASTER_COLS} FROM master WHERE track_id = ?1"),
                    [track.to_string()],
                    master_from_row,
                )
                .optional()?;
            Ok(with_replicas(c, m.into_iter().collect())?.pop())
        })
    }

    /// All masters that came out of the torrent `info_hash`.
    pub fn masters_of_torrent(&self, info_hash: &str) -> Result<Vec<Master>> {
        self.read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {MASTER_COLS} FROM master
                  WHERE origin_kind = 'torrent' AND origin_info_hash = ?1
                  ORDER BY origin_path, track_id"
            ))?;
            let masters = stmt
                .query_map([info_hash], master_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            with_replicas(c, masters)
        })
    }

    /// Masters no node has a replica of yet (waiting for backup), up to `limit`.
    pub fn masters_without_replicas(&self, limit: usize) -> Result<Vec<Master>> {
        self.read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {MASTER_COLS} FROM master m
                  WHERE NOT EXISTS (SELECT 1 FROM replica r WHERE r.file_hash = m.file_hash)
                  ORDER BY track_id LIMIT ?1"
            ))?;
            let masters = stmt
                .query_map([limit as i64], master_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(masters)
        })
    }

    /// Updates the in-torrent path of a torrent-origin master (the file was
    /// moved out of the torrent folder; the origin stays the torrent).
    pub fn set_master_origin_path(&self, track: TrackId, path: &str) -> Result<()> {
        self.write(|tx| {
            let kind: Option<Option<String>> = tx
                .query_row(
                    "SELECT origin_kind FROM master WHERE track_id = ?1",
                    [track.to_string()],
                    |r| r.get(0),
                )
                .optional()?;
            match kind {
                None => Err(LibraryError::not_found("master", track)),
                Some(Some(k)) if k == "torrent" => {
                    tx.execute(
                        "UPDATE master SET origin_path = ?2 WHERE track_id = ?1",
                        params![track.to_string(), path],
                    )?;
                    Ok(())
                }
                Some(_) => Err(LibraryError::Invalid(format!(
                    "master {track} does not come from a torrent"
                ))),
            }
        })
    }

    /// Rewrites `origin_path` of masters that lived inside `old_dir` to point
    /// into `new_dir`. Returns the number of rows updated.
    pub fn relocate_master_paths(
        &self,
        old_dir: &std::path::Path,
        new_dir: &std::path::Path,
    ) -> Result<usize> {
        self.write(|tx| {
            let rows: Vec<(String, String)> = {
                let mut stmt = tx.prepare(
                    "SELECT track_id, origin_path FROM master WHERE origin_path IS NOT NULL",
                )?;
                let mapped = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
                mapped.collect::<rusqlite::Result<_>>()?
            };
            let mut updated = 0usize;
            for (track_id, path_str) in rows {
                let p = std::path::Path::new(&path_str);
                if let Ok(rel) = p.strip_prefix(old_dir) {
                    let target = new_dir.join(rel).to_string_lossy().into_owned();
                    tx.execute(
                        "UPDATE master SET origin_path = ?2 WHERE track_id = ?1",
                        params![track_id, target],
                    )?;
                    updated += 1;
                }
            }
            Ok(updated)
        })
    }

    /// Inserts or updates one replica.
    pub fn put_replica(&self, replica: &Replica) -> Result<()> {
        self.write(|tx| upsert_replica(tx, replica))
    }

    /// Removes the master and, when no other master shares its file, the
    /// replicas. Returns whether a master existed.
    pub fn delete_master(&self, track: TrackId) -> Result<bool> {
        self.write(|tx| {
            let hash: Option<String> = tx
                .query_row(
                    "SELECT file_hash FROM master WHERE track_id = ?1",
                    [track.to_string()],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(hash) = hash else {
                return Ok(false);
            };
            tx.execute(
                "DELETE FROM master WHERE track_id = ?1",
                [track.to_string()],
            )?;
            tx.execute(
                "DELETE FROM replica WHERE file_hash = ?1
                   AND NOT EXISTS (SELECT 1 FROM master WHERE file_hash = ?1)",
                [hash],
            )?;
            Ok(true)
        })
    }

    // ---------------------------------------------------------- downloads

    /// Inserts or updates a download by `info_hash` (`created_at` of an
    /// existing row is kept).
    pub fn put_download(&self, d: &Download) -> Result<()> {
        let selected = serde_json::to_string(&d.selected_files)?;
        self.write(|tx| {
            tx.execute(
                "INSERT INTO download(info_hash, magnet, torrent_file, selected_files, status,
                                      seed_policy, bytes_up, bytes_down, wishlist_id,
                                      created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(info_hash) DO UPDATE SET
                    magnet = excluded.magnet, torrent_file = excluded.torrent_file,
                    selected_files = excluded.selected_files, status = excluded.status,
                    seed_policy = excluded.seed_policy, bytes_up = excluded.bytes_up,
                    bytes_down = excluded.bytes_down, wishlist_id = excluded.wishlist_id,
                    updated_at = excluded.updated_at",
                params![
                    d.info_hash,
                    d.magnet,
                    d.torrent_file,
                    selected,
                    d.status.as_str(),
                    d.seed_policy.as_str(),
                    d.bytes_up as i64,
                    d.bytes_down as i64,
                    d.wishlist_id.map(|w| w.to_string()),
                    d.created_at,
                    d.updated_at
                ],
            )?;
            Ok(())
        })
    }

    pub fn get_download(&self, info_hash: &str) -> Result<Option<Download>> {
        self.read(|c| {
            Ok(c.query_row(
                &format!("SELECT {DOWNLOAD_COLS} FROM download WHERE info_hash = ?1"),
                [info_hash],
                download_from_row,
            )
            .optional()?)
        })
    }

    /// All downloads, newest first.
    pub fn list_downloads(&self) -> Result<Vec<Download>> {
        self.read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {DOWNLOAD_COLS} FROM download ORDER BY created_at DESC, info_hash"
            ))?;
            let rows = stmt
                .query_map([], download_from_row)?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })
    }

    /// Stores the torrent client's progress and bumps `updated_at`.
    pub fn update_download_progress(
        &self,
        info_hash: &str,
        status: DownloadStatus,
        bytes_up: u64,
        bytes_down: u64,
    ) -> Result<()> {
        self.write(|tx| {
            let n = tx.execute(
                "UPDATE download SET status = ?2, bytes_up = ?3, bytes_down = ?4, updated_at = ?5
                  WHERE info_hash = ?1",
                params![
                    info_hash,
                    status.as_str(),
                    bytes_up as i64,
                    bytes_down as i64,
                    now_ms()
                ],
            )?;
            if n == 0 {
                return Err(LibraryError::not_found("download", info_hash));
            }
            Ok(())
        })
    }

    /// Returns whether the download existed.
    pub fn delete_download(&self, info_hash: &str) -> Result<bool> {
        self.write(|tx| {
            Ok(tx.execute("DELETE FROM download WHERE info_hash = ?1", [info_hash])? > 0)
        })
    }

    // ----------------------------------------------------------- wishlist

    /// Inserts or updates a wishlist item (`created_at` of an existing row is kept).
    pub fn put_wishlist_item(&self, w: &WishlistItem) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO wishlist(id, title, artist, recording_mbid, isrc, spotify_id,
                                      youtube_id, status, candidates_json, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(id) DO UPDATE SET
                    title = excluded.title, artist = excluded.artist,
                    recording_mbid = excluded.recording_mbid, isrc = excluded.isrc,
                    spotify_id = excluded.spotify_id, youtube_id = excluded.youtube_id,
                    status = excluded.status, candidates_json = excluded.candidates_json,
                    updated_at = excluded.updated_at",
                params![
                    w.id.to_string(),
                    w.title,
                    w.artist,
                    w.recording_mbid,
                    w.isrc,
                    w.spotify_id,
                    w.youtube_id,
                    w.status.as_str(),
                    w.candidates_json,
                    w.created_at,
                    w.updated_at
                ],
            )?;
            Ok(())
        })
    }

    pub fn get_wishlist_item(&self, id: WishlistId) -> Result<Option<WishlistItem>> {
        self.read(|c| {
            Ok(c.query_row(
                &format!("SELECT {WISHLIST_COLS} FROM wishlist WHERE id = ?1"),
                [id.to_string()],
                wishlist_from_row,
            )
            .optional()?)
        })
    }

    /// Wishlist items in one status, oldest first.
    pub fn list_wishlist_by_status(&self, status: WishlistStatus) -> Result<Vec<WishlistItem>> {
        self.read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {WISHLIST_COLS} FROM wishlist WHERE status = ?1 ORDER BY created_at, id"
            ))?;
            let rows = stmt
                .query_map([status.as_str()], wishlist_from_row)?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })
    }

    /// Returns whether the item existed.
    pub fn delete_wishlist_item(&self, id: WishlistId) -> Result<bool> {
        self.write(|tx| Ok(tx.execute("DELETE FROM wishlist WHERE id = ?1", [id.to_string()])? > 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib() -> Library {
        Library::open_in_memory().unwrap()
    }

    fn master(hash: &str, origin: Option<SourceOrigin>) -> Master {
        Master {
            track_id: TrackId::new(),
            codec: Codec::Flac,
            file_hash: hash.to_owned(),
            size_bytes: 12_345_678,
            origin,
            replicas: Vec::new(),
        }
    }

    fn torrent(hash: &str, path: &str) -> Option<SourceOrigin> {
        Some(SourceOrigin::Torrent {
            info_hash: hash.to_owned(),
            path_in_torrent: path.to_owned(),
        })
    }

    fn replica(hash: &str, node: &str, at: Option<i64>) -> Replica {
        Replica {
            file_hash: hash.to_owned(),
            node_id: node.into(),
            verified_at: at,
        }
    }

    #[test]
    fn master_roundtrip_and_upsert() {
        let lib = lib();
        let mut m = master("h1", torrent("ih", "A/01.flac"));
        m.replicas = vec![replica("h1", "pc", Some(5)), replica("h1", "pi", None)];
        lib.put_master(&m).unwrap();
        let got = lib.get_master(m.track_id).unwrap().unwrap();
        assert_eq!(got, m);
        assert!(lib.get_master(TrackId::new()).unwrap().is_none());

        // Upsert: change the file, re-verify one replica.
        m.size_bytes = 99;
        m.origin = None;
        m.replicas = vec![replica("h1", "pc", Some(9))];
        lib.put_master(&m).unwrap();
        let got = lib.get_master(m.track_id).unwrap().unwrap();
        assert_eq!(got.size_bytes, 99);
        assert_eq!(got.origin, None);
        assert_eq!(got.replicas.len(), 2);
        assert_eq!(got.replicas[0].verified_at, Some(9));

        for origin in [SourceOrigin::Inbox, SourceOrigin::Stream] {
            let m = master("hx", Some(origin));
            lib.put_master(&m).unwrap();
            assert_eq!(lib.get_master(m.track_id).unwrap().unwrap(), m);
        }
    }

    #[test]
    fn masters_of_torrent_and_origin_path() {
        let lib = lib();
        let a = master("a", torrent("ih1", "d/02.flac"));
        let b = master("b", torrent("ih1", "d/01.flac"));
        let c = master("c", torrent("ih2", "x.flac"));
        let d = master("d", Some(SourceOrigin::Inbox));
        for m in [&a, &b, &c, &d] {
            lib.put_master(m).unwrap();
        }
        let of1 = lib.masters_of_torrent("ih1").unwrap();
        assert_eq!(of1, vec![b.clone(), a.clone()]);
        assert!(lib.masters_of_torrent("nope").unwrap().is_empty());

        lib.set_master_origin_path(a.track_id, "masters/a.flac")
            .unwrap();
        let got = lib.get_master(a.track_id).unwrap().unwrap();
        assert_eq!(got.origin, torrent("ih1", "masters/a.flac"));
        assert!(matches!(
            lib.set_master_origin_path(d.track_id, "p"),
            Err(LibraryError::Invalid(_))
        ));
        assert!(matches!(
            lib.set_master_origin_path(TrackId::new(), "p"),
            Err(LibraryError::NotFound { .. })
        ));
    }

    #[test]
    fn replicas_and_masters_without_replicas() {
        let lib = lib();
        let a = master("a", None);
        let b = master("b", None);
        lib.put_master(&a).unwrap();
        lib.put_master(&b).unwrap();
        assert_eq!(lib.masters_without_replicas(10).unwrap().len(), 2);
        assert_eq!(lib.masters_without_replicas(1).unwrap().len(), 1);

        lib.put_replica(&replica("a", "server", Some(1))).unwrap();
        let left = lib.masters_without_replicas(10).unwrap();
        assert_eq!(left, vec![b.clone()]);
        let got = lib.get_master(a.track_id).unwrap().unwrap();
        assert_eq!(got.replicas, vec![replica("a", "server", Some(1))]);

        lib.put_replica(&replica("a", "server", Some(2))).unwrap();
        let got = lib.get_master(a.track_id).unwrap().unwrap();
        assert_eq!(got.replicas, vec![replica("a", "server", Some(2))]);
    }

    #[test]
    fn delete_master_removes_replicas() {
        let lib = lib();
        let mut a = master("a", None);
        a.replicas = vec![replica("a", "pc", None)];
        lib.put_master(&a).unwrap();
        assert!(lib.delete_master(a.track_id).unwrap());
        assert!(lib.get_master(a.track_id).unwrap().is_none());
        assert!(!lib.delete_master(a.track_id).unwrap());
        let n: i64 = lib
            .read(|c| Ok(c.query_row("SELECT COUNT(*) FROM replica", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(n, 0);
    }

    fn download(hash: &str, created: i64) -> Download {
        Download {
            info_hash: hash.to_owned(),
            magnet: Some(format!("magnet:?xt=urn:btih:{hash}")),
            torrent_file: None,
            selected_files: vec!["a.flac".into(), "b/c.flac".into()],
            status: DownloadStatus::Queued,
            seed_policy: SeedPolicy::Keep,
            bytes_up: 0,
            bytes_down: 0,
            wishlist_id: Some(WishlistId::new()),
            created_at: created,
            updated_at: created,
        }
    }

    #[test]
    fn download_crud() {
        let lib = lib();
        let a = download("aa", 100);
        let b = download("bb", 200);
        lib.put_download(&a).unwrap();
        lib.put_download(&b).unwrap();
        assert_eq!(lib.get_download("aa").unwrap().unwrap(), a);
        assert!(lib.get_download("zz").unwrap().is_none());
        assert_eq!(lib.list_downloads().unwrap(), vec![b.clone(), a.clone()]);

        // Upsert keeps created_at.
        let mut a2 = a.clone();
        a2.created_at = 999;
        a2.status = DownloadStatus::Paused;
        a2.selected_files.clear();
        a2.wishlist_id = None;
        a2.updated_at = 300;
        lib.put_download(&a2).unwrap();
        let got = lib.get_download("aa").unwrap().unwrap();
        assert_eq!(got.created_at, 100);
        assert_eq!(got.status, DownloadStatus::Paused);
        assert!(got.selected_files.is_empty());
        assert_eq!(got.wishlist_id, None);

        lib.update_download_progress("bb", DownloadStatus::Seeding, 7, 1 << 40)
            .unwrap();
        let got = lib.get_download("bb").unwrap().unwrap();
        assert_eq!(got.status, DownloadStatus::Seeding);
        assert_eq!((got.bytes_up, got.bytes_down), (7, 1 << 40));
        assert!(got.updated_at > 200);
        assert!(matches!(
            lib.update_download_progress("zz", DownloadStatus::Done, 0, 0),
            Err(LibraryError::NotFound { .. })
        ));

        assert!(lib.delete_download("aa").unwrap());
        assert!(!lib.delete_download("aa").unwrap());
        assert_eq!(lib.list_downloads().unwrap().len(), 1);
    }

    #[test]
    fn download_defaults_match_schema() {
        let lib = lib();
        lib.write(|tx| {
            tx.execute(
                "INSERT INTO download(info_hash, created_at, updated_at) VALUES ('d', 1, 1)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let d = lib.get_download("d").unwrap().unwrap();
        assert_eq!(d.status, DownloadStatus::Queued);
        assert_eq!(d.seed_policy, SeedPolicy::Standard);
        assert!(d.selected_files.is_empty());
    }

    #[test]
    fn wishlist_crud() {
        let lib = lib();
        let mk = |title: &str, status, created| WishlistItem {
            id: WishlistId::new(),
            title: Some(title.to_owned()),
            artist: Some("Artist".into()),
            recording_mbid: None,
            isrc: Some("USRC17607839".into()),
            spotify_id: None,
            youtube_id: Some("yt".into()),
            status,
            candidates_json: Some("[]".into()),
            created_at: created,
            updated_at: created,
        };
        let a = mk("A", WishlistStatus::New, 1);
        let b = mk("B", WishlistStatus::New, 2);
        let c = mk("C", WishlistStatus::Done, 3);
        for w in [&a, &b, &c] {
            lib.put_wishlist_item(w).unwrap();
        }
        assert_eq!(lib.get_wishlist_item(a.id).unwrap().unwrap(), a);
        assert!(lib.get_wishlist_item(WishlistId::new()).unwrap().is_none());
        assert_eq!(
            lib.list_wishlist_by_status(WishlistStatus::New).unwrap(),
            vec![a.clone(), b.clone()]
        );

        let mut a2 = a.clone();
        a2.status = WishlistStatus::Done;
        a2.created_at = 50;
        a2.updated_at = 60;
        lib.put_wishlist_item(&a2).unwrap();
        let got = lib.get_wishlist_item(a.id).unwrap().unwrap();
        assert_eq!(
            (got.status, got.created_at, got.updated_at),
            (WishlistStatus::Done, 1, 60)
        );
        assert_eq!(
            lib.list_wishlist_by_status(WishlistStatus::Done)
                .unwrap()
                .len(),
            2
        );

        assert!(lib.delete_wishlist_item(b.id).unwrap());
        assert!(!lib.delete_wishlist_item(b.id).unwrap());
        assert_eq!(
            lib.list_wishlist_by_status(WishlistStatus::New)
                .unwrap()
                .len(),
            0
        );
    }
}
