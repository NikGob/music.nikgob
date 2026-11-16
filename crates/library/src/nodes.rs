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
