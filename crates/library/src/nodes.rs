//! The owner's paired devices (table `node`, migration 34): what the
//! devices list keeps between runs. The ring's certificates themselves
//! live in the mesh's files; a row here is the device as this instance last
//! knew it, with what was chosen for it here (a name, a storage profile).

use rusqlite::{Connection, Row, params};

use crate::db::Library;
use crate::error::Result;

/// One paired device.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NodeRow {
    /// Its node id (hex).
    pub id: String,
    /// Its own name (certificate, then what it says of itself).
    pub name: String,
    /// A name given here, shown over [`Self::name`].
    pub alias: Option<String>,
    /// `pc`, `pi`, `phone`, `tv`, `server`.
    pub kind: String,
    /// `owner` or `guest`.
    pub scope: String,
    /// `full`, `library`, `favorites:<bytes>`, `sound`.
    pub profile: String,
    pub last_seen: Option<i64>,
    /// `ip:port` it was last reached at.
    pub addr: Option<String>,
    pub version: Option<String>,
    /// Its ring certificate (JSON).
    pub cert: Option<String>,
    pub paired_at: Option<i64>,
}

/// A lossless original one device alone holds a verified copy of
/// ([`Library::flac_only_on`]), and where this computer keeps it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OnlyCopy {
    pub file_hash: String,
    /// The paths its masters are kept at by name.
    pub paths: Vec<String>,
    /// A master of it is kept as `{hash}.<ext>` right in the masters folder.
    pub flat: bool,
}

impl NodeRow {
    /// The name the list shows.
    pub fn shown_name(&self) -> &str {
        self.alias.as_deref().filter(|a| !a.trim().is_empty()).unwrap_or(&self.name)
    }
}

const COLS: &str = "id, name, alias, kind, scope, profile, last_seen, addr, version, cert, paired_at";

fn row_of(r: &Row<'_>) -> rusqlite::Result<NodeRow> {
    Ok(NodeRow {
        id: r.get(0)?,
        name: r.get(1)?,
        alias: r.get(2)?,
        kind: r.get(3)?,
        scope: r.get(4)?,
        profile: r.get(5)?,
        last_seen: r.get(6)?,
        addr: r.get(7)?,
        version: r.get(8)?,
        cert: r.get(9)?,
        paired_at: r.get(10)?,
    })
}

fn put_in(c: &Connection, n: &NodeRow) -> Result<()> {
    c.execute(
        "INSERT INTO node(id, name, alias, kind, scope, profile, last_seen, addr, version, cert, paired_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT(id) DO UPDATE SET
            name = excluded.name, alias = excluded.alias, kind = excluded.kind, scope = excluded.scope,
            profile = excluded.profile, last_seen = excluded.last_seen, addr = excluded.addr,
            version = excluded.version, cert = excluded.cert, paired_at = excluded.paired_at",
        params![n.id, n.name, n.alias, n.kind, n.scope, n.profile, n.last_seen, n.addr, n.version, n.cert, n.paired_at],
    )?;
    Ok(())
}

impl Library {
    /// Every paired device, by name.
    pub fn nodes(&self) -> Result<Vec<NodeRow>> {
        self.read(|c| {
            let mut stmt = c.prepare(&format!("SELECT {COLS} FROM node ORDER BY name, id"))?;
            let rows = stmt.query_map([], row_of)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Writes these rows as they are (inserted or replaced by id), in one
    /// transaction.
    pub fn put_nodes(&self, rows: &[NodeRow]) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        self.write(|tx| {
            for n in rows {
                put_in(tx, n)?;
            }
            Ok(())
        })
    }

    /// The lossless originals (FLAC masters) device `node` holds a verified
    /// copy of that no other trusted device has verified — an owner's device
    /// in the node table, not one of `leaving` (being forgotten too); the
    /// replica rows of forgotten devices stay (they say where a file was
    /// verified) and count for nothing here. Each file once, with where this
    /// computer keeps it; whether it still has it is the caller's to look
    /// at (on disk).
    pub fn flac_only_on(&self, node: &str, leaving: &[String]) -> Result<Vec<OnlyCopy>> {
        let leaving = serde_json::to_string(leaving).unwrap_or_else(|_| "[]".into());
        self.read(|c| {
            let mut stmt = c.prepare(
                "SELECT r.file_hash, m.file_path FROM replica r
                 JOIN master m ON m.file_hash = r.file_hash AND lower(m.codec) = 'flac'
                 WHERE r.node_id = ?1 AND r.verified_at IS NOT NULL
                   AND NOT EXISTS (
                       SELECT 1 FROM replica o JOIN node n ON n.id = o.node_id
                       WHERE o.file_hash = r.file_hash AND o.node_id <> ?1 AND o.verified_at IS NOT NULL
                         AND n.scope = 'owner'
                         AND o.node_id NOT IN (SELECT value FROM json_each(?2)))
                 ORDER BY r.file_hash, m.file_path",
            )?;
            let rows = stmt.query_map(params![node, leaving], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?;
            let mut out: Vec<OnlyCopy> = Vec::new();
            for row in rows {
                let (hash, path) = row?;
                if out.last().is_none_or(|o| o.file_hash != hash) {
                    out.push(OnlyCopy { file_hash: hash, paths: Vec::new(), flat: false });
                }
                let Some(o) = out.last_mut() else { continue };
                match path {
                    Some(p) if !o.paths.contains(&p) => o.paths.push(p),
                    Some(_) => {}
                    None => o.flat = true,
                }
            }
            Ok(out)
        })
    }

    /// Forgets these devices (their replicas' rows stay: they say where a
    /// file was verified).
    pub fn remove_nodes(&self, ids: &[String]) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        self.write(|tx| {
            let mut n = 0;
            for id in ids {
                n += tx.execute("DELETE FROM node WHERE id = ?1", [id])?;
            }
            Ok(n)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pi() -> NodeRow {
        NodeRow {
            id: "ab".repeat(32),
            name: "Raspberry Pi".into(),
            kind: "pi".into(),
            scope: "owner".into(),
            profile: "full".into(),
            addr: Some("192.168.0.157:47220".into()),
            version: Some("0.0.5".into()),
            cert: Some("{}".into()),
            paired_at: Some(1),
            ..NodeRow::default()
        }
    }

    #[test]
    fn a_device_is_kept_changed_and_forgotten() {
        let lib = Library::open_in_memory().unwrap();
        assert!(lib.nodes().unwrap().is_empty());
        let mut n = pi();
        lib.put_nodes(&[n.clone()]).unwrap();
        assert_eq!(lib.nodes().unwrap(), vec![n.clone()]);
        assert_eq!(n.shown_name(), "Raspberry Pi");
        n.alias = Some("Кухня".into());
        n.last_seen = Some(5);
        n.profile = "favorites:1024".into();
        lib.put_nodes(&[n.clone()]).unwrap();
        let back = lib.nodes().unwrap();
        assert_eq!(back, vec![n.clone()]);
        assert_eq!(back[0].shown_name(), "Кухня");
        assert_eq!(lib.remove_nodes(&[n.id.clone()]).unwrap(), 1);
        assert!(lib.nodes().unwrap().is_empty());
        assert_eq!(lib.remove_nodes(&[n.id]).unwrap(), 0);
    }

    /// Only distinct FLAC originals verified on the device and on no other
    /// trusted one count: not an MP3 master, not a file a forgotten device
    /// or a guest holds too (they count for nothing), one a device being
    /// forgotten too holds as well does count.
    #[test]
    fn only_copies_count_once_and_only_on_trusted_devices() {
        let lib = Library::open_in_memory().unwrap();
        assert!(lib.flac_only_on("pi", &[]).unwrap().is_empty());
        let node = |id: &str, scope: &str| NodeRow { id: id.into(), name: id.into(), kind: "pi".into(), scope: scope.into(), profile: "full".into(), ..NodeRow::default() };
        lib.put_nodes(&[node("pi", "owner"), node("lap", "owner"), node("tv", "guest")]).unwrap();
        lib.write(|tx| {
            // Two tracks of one FLAC file (an image), one kept by name.
            for (t, codec, h, path) in [
                ("t1", "flac", "h1", None),
                ("t2", "flac", "h2", None),
                ("t2b", "flac", "h2", Some("/m/Album/02.flac")),
                ("t3", "mp3", "h3", None),
                ("t4", "flac", "h4", None),
                ("t5", "flac", "h5", None),
                ("t6", "FLAC", "h6", Some("/m/Other/06.flac")),
            ] {
                tx.execute(
                    "INSERT INTO master(track_id, codec, file_hash, size_bytes, file_path) VALUES (?1, ?2, ?3, 10, ?4)",
                    rusqlite::params![t, codec, h, path],
                )?;
            }
            // h1, h2, h3 on the Pi alone; h4 on the Pi and on a forgotten
            // device; h5 on the Pi and the laptop; h6 on the Pi and the
            // guest; h7 (no master) on the laptop, not verified on the Pi.
            for (h, n, v) in [
                ("h1", "pi", Some(1)),
                ("h2", "pi", Some(1)),
                ("h3", "pi", Some(1)),
                ("h4", "pi", Some(1)),
                ("h4", "gone", Some(1)),
                ("h5", "pi", Some(1)),
                ("h5", "lap", Some(1)),
                ("h6", "pi", Some(1)),
                ("h6", "tv", Some(1)),
                ("h7", "pi", None),
                ("h7", "lap", Some(1)),
            ] {
                tx.execute("INSERT INTO replica(file_hash, node_id, verified_at) VALUES (?1, ?2, ?3)", rusqlite::params![h, n, v])?;
            }
            Ok(())
        })
        .unwrap();
        let only = lib.flac_only_on("pi", &[]).unwrap();
        let hashes: Vec<&str> = only.iter().map(|o| o.file_hash.as_str()).collect();
        assert_eq!(hashes, ["h1", "h2", "h4", "h6"]);
        assert_eq!(only[1], OnlyCopy { file_hash: "h2".into(), paths: vec!["/m/Album/02.flac".into()], flat: true }, "once, both ways it is kept");
        assert_eq!(only[3], OnlyCopy { file_hash: "h6".into(), paths: vec!["/m/Other/06.flac".into()], flat: false });
        let leaving = lib.flac_only_on("pi", &["lap".to_owned()]).unwrap();
        assert_eq!(leaving.iter().map(|o| o.file_hash.as_str()).collect::<Vec<_>>(), ["h1", "h2", "h4", "h5", "h6"], "the laptop goes too");
        assert!(lib.flac_only_on("lap", &[]).unwrap().is_empty(), "h5 is on the Pi too; h7 has no master");
    }

    /// A library of schema 33 keeps its (unused) node rows and gets the new
    /// columns with their defaults.
    #[test]
    fn the_node_columns_come_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        {
            let mut c = rusqlite::Connection::open(&path).unwrap();
            let tx = c.transaction().unwrap();
            for sql in &crate::migrations::MIGRATIONS[..33] {
                tx.execute_batch(sql).unwrap();
            }
            tx.pragma_update(None, "user_version", 33).unwrap();
            tx.execute("INSERT INTO node(id, name, kind, last_seen) VALUES ('n1', 'Old', 'pc', 7)", []).unwrap();
            tx.commit().unwrap();
        }
        let lib = Library::open(&path).unwrap();
        assert_eq!(lib.schema_version().unwrap(), crate::SCHEMA_VERSION);
        let rows = lib.nodes().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].scope.as_str(), rows[0].profile.as_str(), rows[0].last_seen), ("owner", "full", Some(7)));
        assert_eq!(rows[0].alias, None);
    }
}
