//! Copies of the library on the owner's other devices.
//!
//! A device with a storing profile works out what it should hold from the
//! synced catalog: every track's file the other devices have (their synced
//! `node_file` rows, [`Library::replica_offers`]) or, with «Избранное»,
//! «Любимые» and the pinned playlists up to a cap ([`plan_tracks`]); the
//! lossless masters another device lists ([`MasterItem`]). What it pulled
//! becomes its own ([`Library::put_pulled_file`],
//! [`Library::put_pulled_masters`]) with a `replica` row: this device holds
//! a copy of that file, verified then.
//!
//! On the computer that keeps the masters: the `replica` rows of the other
//! devices (what they said they hold, [`Library::set_node_replicas`]), the
//! masters enough trusted devices hold a verified copy of
//! ([`Library::masters_to_let_go`]), and the ones it let go of, in the
//! Trash (`master_trash`, migration 36).

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use nikgob_core::model::{PlaylistId, Source, TrackId};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use crate::db::Library;
use crate::error::Result;
use crate::util::{get_id, now_ms};

/// A file of a track another device has (its `node_file` row): what a
/// device keeping that track may pull.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub track_id: TrackId,
    /// The device that has it.
    pub node_id: String,
    pub file_hash: String,
    /// Bytes (0: not known).
    pub size: u64,
    pub codec: String,
    pub quality_score: i32,
    pub bitrate: Option<u32>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
}

/// What a device keeps of the library's tracks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Keep {
    /// Every track's file («Полная копия», «Библиотека»).
    All,
    /// These tracks in this order while they fit in `cap` bytes, what the
    /// device holds of them counted («Избранное»).
    Ordered { order: Vec<TrackId>, cap: u64 },
}

/// A track's file to pull: the best one the devices offer (`file_hash`,
/// `size` bytes, 0: not known) and the devices that have that very file
/// (one offer each); the pull asks for exactly it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Want {
    pub track_id: TrackId,
    pub file_hash: String,
    pub size: u64,
    pub offers: Vec<Offer>,
}

impl Want {
    /// The bytes of the file it would take (0: not known).
    pub fn size(&self) -> u64 {
        self.size
    }
}

/// A file of a track this device holds (a live local source): what the
/// planning counts as held once the caller has seen it on the disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldFile {
    pub track_id: TrackId,
    pub file_hash: Option<String>,
    pub size: u64,
    pub quality_score: i32,
    /// Where it is (as the source has it; a relative one is the library
    /// folder's).
    pub locator: String,
}

/// Offers best first: the better quality, then the larger file, then the
/// device's id.
fn rank(a: &Offer, b: &Offer) -> Ordering {
    b.quality_score.cmp(&a.quality_score).then(b.size.cmp(&a.size)).then_with(|| a.node_id.cmp(&b.node_id))
}

/// Each track's best file the devices offer.
pub fn best_offers(offers: &[Offer]) -> HashMap<TrackId, Offer> {
    let mut out: HashMap<TrackId, Offer> = HashMap::new();
    for o in offers {
        match out.get(&o.track_id) {
            Some(b) if rank(o, b) != Ordering::Less => {}
            _ => {
                out.insert(o.track_id, o.clone());
            }
        }
    }
    out
}

/// The best file of a track's offers and the devices that have that very
/// file, one offer each.
fn want_of(track_id: TrackId, mut list: Vec<Offer>) -> Option<Want> {
    list.sort_by(rank);
    let best = list.first()?.clone();
    let mut seen = HashSet::new();
    let offers: Vec<Offer> = list.into_iter().filter(|o| o.file_hash == best.file_hash && seen.insert(o.node_id.clone())).collect();
    let size = offers.iter().map(|o| o.size).max().unwrap_or(0);
    Some(Want { track_id, file_hash: best.file_hash, size, offers })
}

/// Whether what this device holds of a track (`held`) makes pulling `want`
/// pointless: that very file, or a better one than any offered.
fn satisfied(want: &Want, held: &[HeldFile]) -> bool {
    let best = want.offers.first().map_or(i32::MIN, |o| o.quality_score);
    held.iter().any(|h| h.file_hash.as_deref() == Some(want.file_hash.as_str()) || h.quality_score > best)
}

/// The files to pull, in order: for each track `keep` wants and some
/// device offers, its best file (by hash) unless this device holds that
/// very file or a better one (`held`: the files it has on the disk now).
/// With a cap, what is held of the wanted tracks is charged first; a file
/// that does not fit any more is passed over and a smaller one after it
/// may still fit; a file of a size not known is never taken as free.
pub fn plan_tracks(keep: &Keep, offers: Vec<Offer>, held: &HashMap<TrackId, Vec<HeldFile>>) -> Vec<Want> {
    let mut order: Vec<TrackId> = Vec::new();
    let mut by_track: HashMap<TrackId, Vec<Offer>> = HashMap::new();
    for o in offers {
        let list = by_track.entry(o.track_id).or_insert_with(|| {
            order.push(o.track_id);
            Vec::new()
        });
        list.push(o);
    }
    let none: Vec<HeldFile> = Vec::new();
    match keep {
        Keep::All => order
            .into_iter()
            .filter_map(|t| by_track.remove(&t).and_then(|list| want_of(t, list)))
            .filter(|w| !satisfied(w, held.get(&w.track_id).unwrap_or(&none)))
            .collect(),
        Keep::Ordered { order: wanted, cap } => {
            let mut used = 0u64;
            let mut seen = HashSet::new();
            let mut out = Vec::new();
            for t in wanted {
                if !seen.insert(*t) {
                    continue;
                }
                let here = held.get(t).unwrap_or(&none);
                used = used.saturating_add(here.iter().map(|h| h.size).sum::<u64>());
                let Some(w) = by_track.remove(t).and_then(|list| want_of(*t, list)) else { continue };
                if satisfied(&w, here) {
                    continue;
                }
                if w.size == 0 || used.saturating_add(w.size) > *cap {
                    continue;
                }
                used += w.size;
                out.push(w);
            }
            out
        }
    }
}

/// A lossless master a device keeps, as it lists them for the others
/// (`GET /v1/replica/masters`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MasterItem {
    /// Its file's BLAKE3.
    pub hash: String,
    pub codec: String,
    pub size: u64,
    /// The tracks it is the master of (a disc image: several).
    pub tracks: Vec<TrackId>,
}

/// A page of [`MasterItem`]s.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MastersPage {
    pub masters: Vec<MasterItem>,
    /// Where the next page starts (`?after=`); none: this is the last.
    #[serde(default)]
    pub next: Option<String>,
}

/// A master as this device keeps it: where its file is by name, or flat
/// (`{hash}.<ext>` right in the masters folder).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MasterFile {
    pub item: MasterItem,
    pub paths: Vec<String>,
    /// A master row of it keeps no path: the file is kept flat.
    pub flat: bool,
}

/// `POST /v1/replica/check`: which of these files the device holds;
/// `rehash`: each one read and hashed again now.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckAsk {
    pub hashes: Vec<String>,
    #[serde(default)]
    pub rehash: bool,
}

/// A file a device holds and when it last made sure it is that file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Held {
    pub hash: String,
    pub verified_ms: i64,
}

/// The answer to a [`CheckAsk`]: the files held, and those not read this
/// time (`deferred`: the call's reading budget ran out; ask them again,
/// alone). Those in neither are not held.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckAnswer {
    pub held: Vec<Held>,
    #[serde(default)]
    pub deferred: Vec<String>,
}

/// A lossless master this computer may let go of: trusted devices hold
/// verified copies of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LetGo {
    pub file_hash: String,
    pub size: u64,
    pub tracks: Vec<TrackId>,
    /// Where it is kept by name here.
    pub paths: Vec<String>,
    /// A master of it is kept as `{hash}.<ext>` in the masters folder.
    pub flat: bool,
    /// The devices holding a verified copy (node ids).
    pub holders: Vec<String>,
}

/// A master file this computer moved to the Trash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MasterTrash {
    pub id: i64,
    pub file_hash: String,
    /// Where it was.
    pub path: String,
    /// Where it is in the Trash.
    pub trash_path: String,
    pub size: u64,
    pub trashed_at: i64,
}

fn opt_u32(v: Option<i64>) -> Option<u32> {
    v.and_then(|v| u32::try_from(v).ok())
}

fn offer_of(r: &Row<'_>) -> rusqlite::Result<Offer> {
    Ok(Offer {
        track_id: get_id(r, 0)?,
        node_id: r.get(1)?,
        file_hash: r.get(2)?,
        size: r.get::<_, i64>(3)?.max(0) as u64,
        codec: r.get(4)?,
        quality_score: r.get(5)?,
        bitrate: opt_u32(r.get(6)?),
        sample_rate: opt_u32(r.get(7)?),
        bit_depth: r.get::<_, Option<i64>>(8)?.and_then(|v| u8::try_from(v).ok()),
    })
}

fn trash_of(r: &Row<'_>) -> rusqlite::Result<MasterTrash> {
    Ok(MasterTrash {
        id: r.get(0)?,
        file_hash: r.get(1)?,
        path: r.get(2)?,
        trash_path: r.get(3)?,
        size: r.get::<_, i64>(4)?.max(0) as u64,
        trashed_at: r.get(5)?,
    })
}

const TRASH_COLS: &str = "id, file_hash, path, trash_path, size_bytes, trashed_at";

fn upsert_replica(c: &Connection, hash: &str, node: &str, at: i64) -> Result<()> {
    c.execute(
        "INSERT INTO replica(file_hash, node_id, verified_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(file_hash, node_id) DO UPDATE SET verified_at = excluded.verified_at",
        params![hash, node, at],
    )?;
    Ok(())
}

impl Library {
    // ------------------------------------------------ what a device keeps

    /// The files of live tracks the devices `nodes` have, by their synced
    /// `node_file` rows (only those whose hash is known), track by track.
    pub fn replica_offers(&self, nodes: &[String]) -> Result<Vec<Offer>> {
        if nodes.is_empty() {
            return Ok(Vec::new());
        }
        let nodes = serde_json::to_string(nodes).unwrap_or_else(|_| "[]".into());
        self.read(|c| {
            let mut stmt = c.prepare(
                "SELECT nf.track_id, nf.node_id, nf.file_hash, coalesce(nf.size_bytes, 0), nf.codec, nf.quality_score,
                        nf.bitrate, nf.sample_rate, nf.bit_depth
                   FROM node_file nf JOIN track t ON t.id = nf.track_id
                  WHERE t.deleted_at IS NULL AND nf.file_hash IS NOT NULL
                    AND nf.node_id IN (SELECT value FROM json_each(?1))
                  ORDER BY nf.track_id, nf.node_id",
            )?;
            let rows = stmt.query_map([nodes], offer_of)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// The file `hash` as a device has it (its `node_file` row), for what
    /// a copy of it is (its format and quality).
    pub fn offer_of_hash(&self, hash: &str) -> Result<Option<Offer>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT track_id, node_id, file_hash, coalesce(size_bytes, 0), codec, quality_score, bitrate, sample_rate, bit_depth
                   FROM node_file WHERE file_hash = ?1 ORDER BY quality_score DESC LIMIT 1",
                [hash],
                offer_of,
            )
            .optional()?)
        })
    }

    /// The files this device's live local sources say it has (whether each
    /// is on the disk now is the caller's to look at).
    pub fn replica_held_files(&self) -> Result<Vec<HeldFile>> {
        self.read(|c| {
            let mut stmt = c.prepare(
                "SELECT track_id, file_hash, coalesce(size_bytes, 0), quality_score, locator FROM source
                  WHERE kind = 'local_file' AND trashed_at IS NULL AND available = 1
                  ORDER BY track_id, id",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(HeldFile {
                    track_id: get_id::<TrackId>(r, 0)?,
                    file_hash: r.get(1)?,
                    size: r.get::<_, i64>(2)?.max(0) as u64,
                    quality_score: r.get(3)?,
                    locator: r.get(4)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// «Избранное»'s order: «Любимые» newest liked first (`liked`), then
    /// each of `playlists` in its own order; each track once.
    pub fn replica_order(&self, liked: bool, playlists: &[PlaylistId]) -> Result<Vec<TrackId>> {
        self.read(|c| {
            let mut out: Vec<TrackId> = Vec::new();
            let mut seen: HashSet<TrackId> = HashSet::new();
            if liked {
                let mut stmt = c.prepare_cached(
                    "SELECT target_id FROM likes WHERE target_kind = 'track' AND deleted = 0
                      ORDER BY updated_at DESC, target_id",
                )?;
                for id in stmt.query_map([], |r| get_id::<TrackId>(r, 0))? {
                    let id = id?;
                    if seen.insert(id) {
                        out.push(id);
                    }
                }
            }
            for p in playlists {
                let mut stmt = c.prepare_cached(
                    "SELECT e.track_id FROM playlist_entry e JOIN playlist p ON p.id = e.playlist_id
                      WHERE e.playlist_id = ?1 AND e.deleted = 0 AND p.deleted = 0
                      ORDER BY e.position, e.id",
                )?;
                for id in stmt.query_map([p.to_string()], |r| get_id::<TrackId>(r, 0))? {
                    let id = id?;
                    if seen.insert(id) {
                        out.push(id);
                    }
                }
            }
            Ok(out)
        })
    }

    /// A file pulled from another device kept as this device's own: its
    /// source, under the library's own rules (the track's playing file
    /// chosen again; `node_file` tells the other devices), and this
    /// device's (`me`) verified copy of it.
    pub fn put_pulled_file(&self, source: &Source, me: &str) -> Result<Source> {
        let hash = source.file_hash.clone();
        self.write(|tx| {
            let stored = crate::catalog::add_source_in(tx, source)?;
            if let Some(h) = hash.as_deref() {
                upsert_replica(tx, h, me, now_ms())?;
            }
            Ok(stored)
        })
    }

    // ------------------------------------------------------------ masters

    /// The masters by hash after `after`, `limit` hashes at most, each with
    /// its tracks and where it is kept.
    pub fn masters_after(&self, after: &str, limit: u32) -> Result<Vec<MasterFile>> {
        self.read(|c| {
            let hashes: Vec<String> = {
                let mut stmt = c.prepare_cached("SELECT DISTINCT file_hash FROM master WHERE file_hash > ?1 ORDER BY file_hash LIMIT ?2")?;
                let rows = stmt.query_map(params![after, limit], |r| r.get(0))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            let mut out = Vec::with_capacity(hashes.len());
            let mut stmt = c.prepare_cached("SELECT track_id, codec, size_bytes, file_path FROM master WHERE file_hash = ?1 ORDER BY track_id")?;
            for h in hashes {
                let mut f = MasterFile { item: MasterItem { hash: h.clone(), ..MasterItem::default() }, paths: Vec::new(), flat: false };
                let rows = stmt.query_map([&h], |r| {
                    Ok((get_id::<TrackId>(r, 0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, Option<String>>(3)?))
                })?;
                for row in rows {
                    let (t, codec, size, path) = row?;
                    f.item.tracks.push(t);
                    if f.item.codec.is_empty() {
                        f.item.codec = codec;
                    }
                    f.item.size = f.item.size.max(size.max(0) as u64);
                    match path {
                        Some(p) if !f.paths.contains(&p) => f.paths.push(p),
                        Some(_) => {}
                        None => f.flat = true,
                    }
                }
                out.push(f);
            }
            Ok(out)
        })
    }

    /// The distinct hashes of the masters after `after`, `limit` at most.
    pub fn master_hashes_after(&self, after: &str, limit: u32) -> Result<Vec<String>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached("SELECT DISTINCT file_hash FROM master WHERE file_hash > ?1 ORDER BY file_hash LIMIT ?2")?;
            let rows = stmt.query_map(params![after, limit], |r| r.get(0))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Every master row's file: its hash and where it is kept by name
    /// (None: flat, `{hash}.<ext>` in the masters folder). Whether each is
    /// on the disk now is the caller's to look at.
    pub fn master_paths(&self) -> Result<Vec<(String, Option<String>)>> {
        self.read(|c| {
            let mut stmt = c.prepare("SELECT DISTINCT file_hash, file_path FROM master ORDER BY file_hash")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// A master pulled from another device, kept here: a row for each of
    /// its tracks the catalog has (never over a track's master of another
    /// file), `file_path` where it was put by name (none: kept flat), and
    /// this device's (`me`) verified copy. How many rows were written.
    pub fn put_pulled_masters(&self, item: &MasterItem, file_path: Option<&str>, me: &str) -> Result<usize> {
        let now = now_ms();
        self.write(|tx| {
            let mut n = 0;
            for t in &item.tracks {
                let id = t.to_string();
                let known: bool = tx.query_row("SELECT EXISTS (SELECT 1 FROM track WHERE id = ?1)", [&id], |r| r.get(0))?;
                if !known {
                    continue;
                }
                let other: Option<String> = tx.query_row("SELECT file_hash FROM master WHERE track_id = ?1", [&id], |r| r.get(0)).optional()?;
                if other.as_deref().is_some_and(|h| h != item.hash) {
                    continue;
                }
                tx.execute(
                    "INSERT INTO master(track_id, codec, file_hash, size_bytes, origin_kind, origin_info_hash, origin_path, file_path)
                     VALUES (?1, ?2, ?3, ?4, NULL, NULL, NULL, ?5)
                     ON CONFLICT(track_id) DO UPDATE SET codec = excluded.codec, size_bytes = excluded.size_bytes,
                            file_path = excluded.file_path",
                    params![id, item.codec, item.hash, item.size as i64, file_path],
                )?;
                n += 1;
            }
            upsert_replica(tx, &item.hash, me, now)?;
            Ok(n)
        })
    }

    // ------------------------------------------------- copies of a file

    /// Where this device keeps the file `hash`: its live local sources'
    /// locators and its masters' paths by name; and whether a master of it
    /// is kept flat (`{hash}.<ext>` in the masters folder).
    pub fn copies_of_hash(&self, hash: &str) -> Result<(Vec<String>, bool)> {
        self.read(|c| {
            let mut paths: Vec<String> = Vec::new();
            {
                let mut stmt = c.prepare_cached(
                    "SELECT locator FROM source WHERE file_hash = ?1 AND kind = 'local_file' AND trashed_at IS NULL ORDER BY added_at, id",
                )?;
                for p in stmt.query_map([hash], |r| r.get::<_, String>(0))? {
                    let p = p?;
                    if !paths.contains(&p) {
                        paths.push(p);
                    }
                }
            }
            let mut flat = false;
            let mut stmt = c.prepare_cached("SELECT file_path FROM master WHERE file_hash = ?1 ORDER BY track_id")?;
            for p in stmt.query_map([hash], |r| r.get::<_, Option<String>>(0))? {
                match p? {
                    Some(p) if !paths.contains(&p) => paths.push(p),
                    Some(_) => {}
                    None => flat = true,
                }
            }
            Ok((paths, flat))
        })
    }

    /// When device `node` was last known to hold a verified copy of `hash`.
    pub fn replica_verified(&self, hash: &str, node: &str) -> Result<Option<i64>> {
        self.read(|c| {
            Ok(c.query_row("SELECT verified_at FROM replica WHERE file_hash = ?1 AND node_id = ?2", params![hash, node], |r| {
                r.get::<_, Option<i64>>(0)
            })
            .optional()?
            .flatten())
        })
    }

    /// Device `node`'s copy of `hash` verified at `at`, or (None) gone.
    pub fn note_replica(&self, hash: &str, node: &str, at: Option<i64>) -> Result<()> {
        self.write(|tx| {
            match at {
                Some(at) => upsert_replica(tx, hash, node, at)?,
                None => {
                    tx.execute("DELETE FROM replica WHERE file_hash = ?1 AND node_id = ?2", params![hash, node])?;
                }
            }
            Ok(())
        })
    }

    /// What device `node` said of the files `asked`: those it holds
    /// (`held`, verified when) noted, its rows of the others dropped.
    /// Whether the devices holding a verified copy of any file changed
    /// (a new time alone is no change).
    pub fn set_node_replicas(&self, node: &str, asked: &[String], held: &[Held]) -> Result<bool> {
        let held: HashMap<&str, i64> = held.iter().map(|h| (h.hash.as_str(), h.verified_ms)).collect();
        self.write(|tx| {
            let mut changed = false;
            for h in asked {
                let before: Option<Option<i64>> = tx
                    .query_row("SELECT verified_at FROM replica WHERE file_hash = ?1 AND node_id = ?2", params![h, node], |r| r.get(0))
                    .optional()?;
                let was = before.flatten().is_some();
                match held.get(h.as_str()) {
                    Some(at) => {
                        if before != Some(Some(*at)) {
                            upsert_replica(tx, h, node, *at)?;
                        }
                        changed |= !was;
                    }
                    None => {
                        if before.is_some() {
                            tx.execute("DELETE FROM replica WHERE file_hash = ?1 AND node_id = ?2", params![h, node])?;
                        }
                        changed |= was;
                    }
                }
            }
            Ok(changed)
        })
    }

    /// The trusted devices holding a verified copy of `hash` now: owner
    /// devices of the node table, none of `leaving`. Read right before a
    /// master goes.
    pub fn verified_holders(&self, hash: &str, leaving: &[String]) -> Result<Vec<String>> {
        let leaving = serde_json::to_string(leaving).unwrap_or_else(|_| "[]".into());
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT r.node_id FROM replica r JOIN node n ON n.id = r.node_id
                  WHERE r.file_hash = ?1 AND r.verified_at IS NOT NULL AND n.scope = 'owner'
                    AND r.node_id NOT IN (SELECT value FROM json_each(?2))
                  ORDER BY r.node_id",
            )?;
            let rows = stmt.query_map(params![hash, leaving], |r| r.get::<_, String>(0))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    // ------------------------------------------------ what a file hashed to

    /// What the file at `path` hashed to when it had `size` bytes and was
    /// changed at `mtime_ms`: (size, mtime, hash).
    pub fn file_stamp(&self, path: &str) -> Result<Option<(u64, i64, String)>> {
        self.read(|c| {
            Ok(c.query_row("SELECT size_bytes, mtime_ms, file_hash FROM file_stamp WHERE path = ?1", [path], |r| {
                Ok((r.get::<_, i64>(0)?.max(0) as u64, r.get::<_, i64>(1)?, r.get::<_, String>(2)?))
            })
            .optional()?)
        })
    }

    /// Notes what the file at `path` hashed to just now.
    pub fn put_file_stamp(&self, path: &str, size: u64, mtime_ms: i64, hash: &str) -> Result<()> {
        let now = now_ms();
        self.write(|tx| {
            tx.execute(
                "INSERT INTO file_stamp(path, size_bytes, mtime_ms, file_hash, at) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(path) DO UPDATE SET size_bytes = excluded.size_bytes, mtime_ms = excluded.mtime_ms,
                        file_hash = excluded.file_hash, at = excluded.at",
                params![path, size as i64, mtime_ms, hash, now],
            )?;
            Ok(())
        })
    }

    /// The lossless (FLAC) masters that at least `need` trusted devices
    /// hold a verified copy of: owner devices of the node table, none of
    /// `leaving` (being forgotten). The rows of a forgotten device count
    /// for nothing, as in [`Self::flac_only_on`]. `need` 0 lists nothing.
    /// Whether this computer still has each file is the caller's to look
    /// at (on disk).
    pub fn masters_to_let_go(&self, need: u32, leaving: &[String]) -> Result<Vec<LetGo>> {
        if need == 0 {
            return Ok(Vec::new());
        }
        let leaving = serde_json::to_string(leaving).unwrap_or_else(|_| "[]".into());
        self.read(|c| {
            let mut holders: HashMap<String, Vec<String>> = HashMap::new();
            {
                let mut stmt = c.prepare(
                    "SELECT r.file_hash, r.node_id FROM replica r JOIN node n ON n.id = r.node_id
                      WHERE r.verified_at IS NOT NULL AND n.scope = 'owner'
                        AND r.node_id NOT IN (SELECT value FROM json_each(?1))
                        AND EXISTS (SELECT 1 FROM master m WHERE m.file_hash = r.file_hash AND lower(m.codec) = 'flac')
                      ORDER BY r.file_hash, r.node_id",
                )?;
                let rows = stmt.query_map([&leaving], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
                for row in rows {
                    let (h, n) = row?;
                    holders.entry(h).or_default().push(n);
                }
            }
            holders.retain(|_, v| v.len() >= need as usize);
            if holders.is_empty() {
                return Ok(Vec::new());
            }
            let mut out: Vec<LetGo> = Vec::new();
            let mut stmt =
                c.prepare("SELECT file_hash, track_id, size_bytes, file_path FROM master WHERE lower(codec) = 'flac' ORDER BY file_hash, track_id")?;
            let rows = stmt.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, get_id::<TrackId>(r, 1)?, r.get::<_, i64>(2)?, r.get::<_, Option<String>>(3)?))
            })?;
            for row in rows {
                let (h, t, size, path) = row?;
                let Some(nodes) = holders.get(&h) else { continue };
                if out.last().is_none_or(|g| g.file_hash != h) {
                    out.push(LetGo { file_hash: h.clone(), size: 0, tracks: Vec::new(), paths: Vec::new(), flat: false, holders: nodes.clone() });
                }
                let Some(g) = out.last_mut() else { continue };
                g.size = g.size.max(size.max(0) as u64);
                g.tracks.push(t);
                match path {
                    Some(p) if !g.paths.contains(&p) => g.paths.push(p),
                    Some(_) => {}
                    None => g.flat = true,
                }
            }
            Ok(out)
        })
    }

    // ------------------------------------------- masters in the Trash

    /// Notes a master file moved to the Trash (on the disk when it
    /// returns); its id.
    pub fn put_master_trash(&self, file_hash: &str, path: &str, trash_path: &str, size: u64) -> Result<i64> {
        let now = now_ms();
        self.write_durable(|tx| {
            tx.execute(
                "INSERT INTO master_trash(file_hash, path, trash_path, size_bytes, trashed_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![file_hash, path, trash_path, size as i64, now],
            )?;
            Ok(tx.last_insert_rowid())
        })
    }

    pub fn master_trash_row(&self, id: i64) -> Result<Option<MasterTrash>> {
        self.read(|c| Ok(c.query_row(&format!("SELECT {TRASH_COLS} FROM master_trash WHERE id = ?1"), [id], trash_of).optional()?))
    }

    /// The master files moved to the Trash before `cutoff` (unix ms).
    pub fn master_trash_before(&self, cutoff: i64) -> Result<Vec<MasterTrash>> {
        self.read(|c| {
            let mut stmt = c.prepare(&format!("SELECT {TRASH_COLS} FROM master_trash WHERE trashed_at < ?1 ORDER BY id"))?;
            let rows = stmt.query_map([cutoff], trash_of)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// The note of a master file in the Trash goes (it came back, or it
    /// went for good). Whether there was one.
    pub fn remove_master_trash(&self, id: i64) -> Result<bool> {
        self.write(|tx| Ok(tx.execute("DELETE FROM master_trash WHERE id = ?1", [id])? > 0))
    }
}

#[cfg(test)]
mod tests {
    use nikgob_core::model::{Artist, ArtistCredit, Codec, DeviceId, Release, SourceKind, Track};

    use super::*;
    use crate::NodeRow;

    fn offer(t: TrackId, node: &str, hash: &str, size: u64, quality: i32) -> Offer {
        Offer {
            track_id: t,
            node_id: node.into(),
            file_hash: hash.into(),
            size,
            codec: "opus".into(),
            quality_score: quality,
            bitrate: Some(160),
            sample_rate: Some(48_000),
            bit_depth: None,
        }
    }

    fn held_file(t: TrackId, hash: &str, size: u64, quality: i32) -> HeldFile {
        HeldFile { track_id: t, file_hash: Some(hash.into()), size, quality_score: quality, locator: format!("{hash}.opus") }
    }

    /// The whole library: each track's best file by hash, asked of the
    /// devices that have that very file; held only when this device has
    /// that file (or a better one), not any file of the track.
    #[test]
    fn a_full_copy_takes_each_tracks_best_file_it_lacks() {
        let t: Vec<TrackId> = (0..4).map(|_| TrackId::new()).collect();
        let offers = vec![
            offer(t[0], "pc", "mp3", 10, 50),
            offer(t[0], "lap", "flac", 12, 80),
            offer(t[0], "pc", "flac", 12, 80),
            offer(t[1], "pc", "a1", 10, 50),
            offer(t[2], "pc", "a2", 10, 50),
            offer(t[3], "pc", "a3", 10, 50),
        ];
        let held: HashMap<TrackId, Vec<HeldFile>> = [
            (t[0], vec![held_file(t[0], "mp3", 10, 50)]),
            (t[1], vec![held_file(t[1], "a1", 10, 50)]),
            (t[3], vec![held_file(t[3], "flac3", 30, 90)]),
        ]
        .into();
        let plan = plan_tracks(&Keep::All, offers, &held);
        assert_eq!(plan.iter().map(|w| w.track_id).collect::<Vec<_>>(), vec![t[0], t[2]], "a held MP3 does not stand for the FLAC; a better file held does");
        assert_eq!(plan[0].file_hash, "flac");
        assert_eq!(plan[0].offers.iter().map(|o| o.node_id.as_str()).collect::<Vec<_>>(), ["lap", "pc"], "both have that very file");
        assert_eq!(plan[0].size(), 12);
        assert_eq!(best_offers(&[offer(t[0], "pc", "mp3", 10, 50), offer(t[0], "pc", "flac", 12, 80)])[&t[0]].file_hash, "flac");
    }

    /// «Избранное»: in its order while it fits the cap, what is held
    /// charged; a file too big is passed over, a smaller one after it still
    /// taken; tracks no device has, repeats and files of a size not known
    /// are skipped.
    #[test]
    fn favorites_take_their_order_up_to_the_cap() {
        let t: Vec<TrackId> = (0..7).map(|_| TrackId::new()).collect();
        let offers = vec![
            offer(t[0], "pc", "h0", 40, 50),
            offer(t[1], "pc", "h1", 50, 50),
            offer(t[2], "pc", "h2", 30, 50),
            offer(t[4], "pc", "h4", 20, 50),
            offer(t[6], "pc", "h6", 0, 50),
        ];
        let held: HashMap<TrackId, Vec<HeldFile>> = [(t[3], vec![held_file(t[3], "h3", 25, 50)])].into();
        let keep = Keep::Ordered { order: vec![t[3], t[6], t[0], t[0], t[1], t[5], t[2], t[4]], cap: 100 };
        let plan = plan_tracks(&keep, offers.clone(), &held);
        // 25 held; size unknown: not free; + 40 = 65; 50 would be 115: passed over; 30 → 95; 20 → 115: no.
        assert_eq!(plan.iter().map(|w| w.track_id).collect::<Vec<_>>(), vec![t[0], t[2]]);
        let roomy = Keep::Ordered { order: vec![t[0], t[1]], cap: 1000 };
        assert_eq!(plan_tracks(&roomy, offers.clone(), &HashMap::new()).len(), 2);
        let none = Keep::Ordered { order: Vec::new(), cap: 1000 };
        assert!(plan_tracks(&none, offers, &HashMap::new()).is_empty(), "nothing pinned, nothing kept");
    }

    fn track(lib: &Library, title: &str, n: u32) -> Track {
        let artist = lib.upsert_artist(&Artist::new("Кино")).unwrap();
        let credit = ArtistCredit::single(artist.name.clone(), Some(artist.id));
        let release = lib.upsert_release(&Release::new("Группа крови", credit.clone())).unwrap();
        let mut t = Track::new(title, credit);
        t.release_id = Some(release.id);
        t.track_no = n;
        lib.upsert_track(&t).unwrap()
    }

    fn node_file(lib: &Library, node: &str, t: TrackId, hash: &str, size: i64) {
        lib.write(|tx| {
            tx.execute(
                "INSERT INTO node_file(node_id, source_id, track_id, file_hash, codec, size_bytes, quality_score)
                 VALUES (?1, ?2, ?3, ?4, 'opus', ?5, 50)",
                params![node, format!("s-{hash}"), t.to_string(), hash, size],
            )?;
            Ok(())
        })
        .unwrap();
    }

    /// The pins' order: «Любимые» newest first, then each playlist in its
    /// order, every track once.
    #[test]
    fn pinned_songs_come_in_their_order() {
        let lib = Library::open_in_memory().unwrap();
        let a = track(&lib, "A", 1);
        let b = track(&lib, "B", 2);
        let c = track(&lib, "C", 3);
        lib.like(a.id).unwrap();
        lib.like(b.id).unwrap();
        lib.write(|tx| {
            tx.execute("UPDATE likes SET updated_at = 100 WHERE target_id = ?1", [a.id.to_string()])?;
            tx.execute("UPDATE likes SET updated_at = 200 WHERE target_id = ?1", [b.id.to_string()])?;
            Ok(())
        })
        .unwrap();
        let p = lib.create_playlist("Вечер").unwrap();
        lib.playlist_add(p.id, c.id).unwrap();
        lib.playlist_add(p.id, a.id).unwrap();
        assert_eq!(lib.replica_order(true, &[p.id]).unwrap(), vec![b.id, a.id, c.id]);
        assert_eq!(lib.replica_order(false, &[p.id]).unwrap(), vec![c.id, a.id]);
        assert!(lib.replica_order(false, &[]).unwrap().is_empty());
    }

    /// The other devices' files of live tracks are offered; a pulled file
    /// becomes this device's own source (held, with its verified copy) and
    /// tells the others through `node_file`.
    #[test]
    fn a_pulled_file_is_registered_as_this_devices_own() {
        let lib = Library::open_in_memory().unwrap();
        let me = "me".to_owned();
        lib.set_device_id(DeviceId(me.clone())).unwrap();
        let a = track(&lib, "A", 1);
        let gone = track(&lib, "Gone", 2);
        node_file(&lib, "pc", a.id, "ha", 1000);
        node_file(&lib, "pc", gone.id, "hg", 1000);
        node_file(&lib, "tv", a.id, "hx", 1000);
        lib.mark_track_deleted(gone.id, 1).unwrap();
        let offers = lib.replica_offers(&["pc".to_owned()]).unwrap();
        assert_eq!(offers.len(), 1, "a deleted track, another device's file: not offered");
        assert_eq!((offers[0].track_id, offers[0].file_hash.as_str(), offers[0].size), (a.id, "ha", 1000));
        assert!(lib.replica_held_files().unwrap().is_empty());

        let mut s = Source::new(a.id, SourceKind::LocalFile, "/srv/Music/Кино/01 - A.opus", Codec::Opus);
        s.file_hash = Some("ha".into());
        s.size_bytes = Some(1000);
        let stored = lib.put_pulled_file(&s, &me).unwrap();
        assert!(stored.is_active, "it plays");
        let files = lib.replica_held_files().unwrap();
        assert_eq!((files[0].track_id, files[0].file_hash.as_deref(), files[0].size), (a.id, Some("ha"), 1000));
        assert!(lib.replica_verified("ha", &me).unwrap().is_some());
        let mine = lib.replica_offers(&[me.clone()]).unwrap();
        assert_eq!(mine.len(), 1, "the others see it as this device's");
        assert_eq!(lib.offer_of_hash("hx").unwrap().map(|o| o.node_id), Some("tv".to_owned()));
        assert_eq!(lib.copies_of_hash("ha").unwrap(), (vec!["/srv/Music/Кино/01 - A.opus".to_owned()], false));
    }

    fn master(lib: &Library, t: TrackId, hash: &str, codec: &str, path: Option<&str>) {
        lib.write(|tx| {
            tx.execute(
                "INSERT INTO master(track_id, codec, file_hash, size_bytes, file_path) VALUES (?1, ?2, ?3, 100, ?4)",
                params![t.to_string(), codec, hash, path],
            )?;
            Ok(())
        })
        .unwrap();
    }

    fn node(id: &str, scope: &str) -> NodeRow {
        NodeRow { id: id.into(), name: id.into(), kind: "pi".into(), scope: scope.into(), profile: "full".into(), ..NodeRow::default() }
    }

    fn held(hash: &str, at: i64) -> Held {
        Held { hash: hash.into(), verified_ms: at }
    }

    /// A master is offered to let go only with enough verified copies on
    /// trusted devices: not a copy never verified, not one on a forgotten
    /// device (no row in the node table) or a guest's, not one on a device
    /// being forgotten; an MP3 master never.
    #[test]
    fn a_master_with_too_few_verified_copies_is_never_let_go() {
        let lib = Library::open_in_memory().unwrap();
        let t: Vec<Track> = (0..4).map(|i| track(&lib, &format!("T{i}"), i)).collect();
        master(&lib, t[0].id, "h1", "flac", Some("/m/A/01.flac"));
        master(&lib, t[1].id, "h2", "flac", None);
        master(&lib, t[2].id, "h3", "mp3", None);
        master(&lib, t[3].id, "h4", "flac", None);
        lib.put_nodes(&[node("pi", "owner"), node("lap", "owner"), node("tv", "guest")]).unwrap();
        lib.set_node_replicas("pi", &["h1".into(), "h2".into(), "h3".into(), "h4".into()], &[held("h1", 5), held("h2", 5), held("h3", 5)]).unwrap();
        lib.set_node_replicas("lap", &["h1".into()], &[held("h1", 6)]).unwrap();
        lib.set_node_replicas("tv", &["h2".into()], &[held("h2", 6)]).unwrap();
        lib.set_node_replicas("gone", &["h2".into()], &[held("h2", 6)]).unwrap();
        lib.write(|tx| {
            tx.execute("INSERT INTO replica(file_hash, node_id, verified_at) VALUES ('h4', 'lap', NULL)", [])?;
            Ok(())
        })
        .unwrap();

        let one = lib.masters_to_let_go(1, &[]).unwrap();
        assert_eq!(one.iter().map(|g| g.file_hash.as_str()).collect::<Vec<_>>(), ["h1", "h2"], "no MP3, no unverified copy");
        assert_eq!(one[0].holders, ["lap", "pi"]);
        assert_eq!(one[0].paths, ["/m/A/01.flac"]);
        assert!(one[1].flat);
        let two = lib.masters_to_let_go(2, &[]).unwrap();
        assert_eq!(two.iter().map(|g| g.file_hash.as_str()).collect::<Vec<_>>(), ["h1"], "a guest's and a forgotten device's copies count for nothing");
        assert!(lib.masters_to_let_go(2, &["lap".into()]).unwrap().is_empty(), "a device being forgotten counts for nothing");
        assert_eq!(lib.verified_holders("h1", &[]).unwrap(), ["lap", "pi"]);
        assert_eq!(lib.verified_holders("h1", &["pi".into()]).unwrap(), ["lap"]);
        assert!(lib.verified_holders("h4", &[]).unwrap().is_empty(), "never verified");
        assert!(lib.masters_to_let_go(0, &[]).unwrap().is_empty());
    }

    /// What a device says it holds replaces what was known of the files
    /// asked; only a change of who holds what counts as one.
    #[test]
    fn a_devices_answer_sets_its_copies() {
        let lib = Library::open_in_memory().unwrap();
        let asked = vec!["h1".to_owned(), "h2".to_owned()];
        assert!(lib.set_node_replicas("pi", &asked, &[held("h1", 5), held("x", 5)]).unwrap());
        assert_eq!(lib.replica_verified("h1", "pi").unwrap(), Some(5));
        assert_eq!(lib.replica_verified("x", "pi").unwrap(), None, "only what was asked");
        assert!(!lib.set_node_replicas("pi", &asked, &[held("h1", 9)]).unwrap(), "a new time alone");
        assert_eq!(lib.replica_verified("h1", "pi").unwrap(), Some(9));
        assert!(lib.set_node_replicas("pi", &asked, &[]).unwrap(), "gone from the device");
        assert_eq!(lib.replica_verified("h1", "pi").unwrap(), None);
    }

    /// A pulled master gets a row for each track the catalog has, never
    /// over another file's master, and this device's verified copy.
    #[test]
    fn a_pulled_master_is_kept_for_its_known_tracks() {
        let lib = Library::open_in_memory().unwrap();
        let me = lib.device_id().0;
        let a = track(&lib, "A", 1);
        let b = track(&lib, "B", 2);
        master(&lib, b.id, "other", "flac", None);
        let item = MasterItem { hash: "hm".into(), codec: "flac".into(), size: 300, tracks: vec![a.id, b.id, TrackId::new()] };
        assert_eq!(lib.put_pulled_masters(&item, Some("/srv/Music/.musiclib/masters/Кино/01 - A.flac"), &me).unwrap(), 1);
        let m = lib.get_master(a.id).unwrap().unwrap();
        assert_eq!((m.file_hash.as_str(), m.size_bytes, m.file_path.as_deref()), ("hm", 300, Some("/srv/Music/.musiclib/masters/Кино/01 - A.flac")));
        assert_eq!(lib.get_master(b.id).unwrap().unwrap().file_hash, "other");
        assert!(lib.master_paths().unwrap().contains(&("hm".to_owned(), Some("/srv/Music/.musiclib/masters/Кино/01 - A.flac".to_owned()))));
        let page = lib.masters_after("", 10).unwrap();
        assert_eq!(page.iter().map(|f| f.item.hash.as_str()).collect::<Vec<_>>(), ["hm", "other"]);
        assert_eq!(page[0].item.tracks, vec![a.id]);
        assert_eq!(lib.master_hashes_after("hm", 10).unwrap(), ["other"]);
    }

    #[test]
    fn a_files_hash_is_noted_with_its_size_and_time() {
        let lib = Library::open_in_memory().unwrap();
        assert!(lib.file_stamp("/m/a.flac").unwrap().is_none());
        lib.put_file_stamp("/m/a.flac", 10, 5, "h1").unwrap();
        lib.put_file_stamp("/m/a.flac", 12, 6, "h2").unwrap();
        assert_eq!(lib.file_stamp("/m/a.flac").unwrap(), Some((12, 6, "h2".to_owned())));
    }

    #[test]
    fn masters_in_the_trash_are_noted() {
        let lib = Library::open_in_memory().unwrap();
        let id = lib.put_master_trash("h1", "/m/A/01.flac", "/t/2026-11-16 01.flac", 42).unwrap();
        let row = lib.master_trash_row(id).unwrap().unwrap();
        assert_eq!((row.file_hash.as_str(), row.path.as_str(), row.size), ("h1", "/m/A/01.flac", 42));
        assert_eq!(lib.master_trash_before(row.trashed_at + 1).unwrap().len(), 1);
        assert!(lib.master_trash_before(row.trashed_at).unwrap().is_empty());
        assert!(lib.remove_master_trash(id).unwrap());
        assert!(lib.master_trash_row(id).unwrap().is_none());
    }
}
