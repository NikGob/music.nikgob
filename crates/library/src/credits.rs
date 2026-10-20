//! One-time repair of credits imported before "A, B" was split into its
//! artists: such a track (or album) was linked to one artist called "A, B",
//! so neither A's nor B's page had it and the track could only lead to the
//! combined name.

use std::collections::HashMap;

use nikgob_core::credit::{split_credit, split_pair};
use nikgob_core::model::{Artist, ArtistCredit, ArtistCreditPart, ArtistId};
use rusqlite::{Connection, OptionalExtension, params};

use crate::catalog::{upsert_artist_in, write_credit_links};
use crate::db::Library;
use crate::error::Result;
use crate::util::{credit_columns, fold, meta_get, meta_set};

/// `meta` key: the repair of [`Library::split_combined_credits`] ran.
const SPLIT_KEY: &str = "combined_credits_split.v1";

/// Artists by folded name, and which of them stand for themselves (have a
/// MusicBrainz id: "Simon & Garfunkel" from MusicBrainz is one act).
struct Names {
    by_name: HashMap<String, ArtistId>,
    with_mbid: std::collections::HashSet<ArtistId>,
}

impl Names {
    fn load(conn: &Connection) -> Result<Self> {
        let mut stmt = conn.prepare("SELECT id, name, mbid FROM artist ORDER BY seq")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?))
        })?;
        let mut names = Self { by_name: HashMap::new(), with_mbid: Default::default() };
        for row in rows {
            let (id, name, mbid) = row?;
            let Ok(id) = id.parse::<ArtistId>() else { continue };
            names.by_name.entry(fold(name.trim())).or_insert(id);
            if mbid.is_some_and(|m| !m.is_empty()) {
                names.with_mbid.insert(id);
            }
        }
        Ok(names)
    }

    fn known(&self, name: &str) -> bool {
        self.by_name.contains_key(&fold(name.trim()))
    }

    /// The artist called `name`, created when the library has none.
    fn ensure(&mut self, tx: &Connection, name: &str) -> Result<ArtistId> {
        let key = fold(name.trim());
        if let Some(id) = self.by_name.get(&key) {
            return Ok(*id);
        }
        let stored = upsert_artist_in(tx, &Artist::new(name.trim()))?;
        self.by_name.insert(key, stored.id);
        Ok(stored.id)
    }
}

/// The parts one credited name stands for, when it is several artists.
fn pieces(names: &Names, part: &ArtistCreditPart) -> Option<Vec<(String, String)>> {
    if part.artist_id.is_some_and(|id| names.with_mbid.contains(&id)) {
        return None;
    }
    let split = split_credit(&part.name);
    if split.len() > 1 {
        return Some(split);
    }
    // A lone "A & B": two artists when both are known and the pair is
    // known only as this credit's own row.
    let [a, b] = split_pair(&part.name)?;
    (names.known(&a.1) && names.known(&b.1)).then(|| vec![a, b])
}

/// The credit with every combined name split; `None` when nothing changes.
fn split(names: &mut Names, tx: &Connection, credit: &ArtistCredit) -> Result<Option<ArtistCredit>> {
    let mut changed = false;
    let mut out = ArtistCredit::default();
    for part in &credit.0 {
        match pieces(names, part) {
            Some(list) => {
                changed = true;
                let n = list.len();
                for (i, (join, name)) in list.into_iter().enumerate() {
                    let id = names.ensure(tx, &name)?;
                    if i > 0
                        && let Some(last) = out.0.last_mut()
                    {
                        last.join_phrase = join;
                    }
                    out.0.push(ArtistCreditPart {
                        artist_id: Some(id),
                        name,
                        join_phrase: if i + 1 == n { part.join_phrase.clone() } else { String::new() },
                    });
                }
            }
            None => out.0.push(part.clone()),
        }
    }
    Ok(changed.then_some(out))
}

fn rows(tx: &Connection, table: &str) -> Result<Vec<(String, String)>> {
    let mut stmt = tx.prepare(&format!("SELECT id, artist_credit FROM {table}"))?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Drops an artist that the split left without anything: no track, album,
/// like or id in a service points at it.
fn drop_if_orphan(tx: &Connection, id: ArtistId) -> Result<()> {
    let id = id.to_string();
    let used: Option<i64> = tx
        .query_row(
            "SELECT 1 WHERE EXISTS (SELECT 1 FROM track_artist WHERE artist_id = ?1)
                OR EXISTS (SELECT 1 FROM release_artist WHERE artist_id = ?1)
                OR EXISTS (SELECT 1 FROM likes WHERE target_kind = 'artist' AND target_id = ?1
                              AND deleted = 0)
                OR EXISTS (SELECT 1 FROM external_id WHERE kind = 'artist' AND local_id = ?1)",
            [&id],
            |r| r.get(0),
        )
        .optional()?;
    if used.is_none() {
        tx.execute("DELETE FROM artist_photo WHERE artist_id = ?1", [&id])?;
        tx.execute("DELETE FROM artist WHERE id = ?1", [&id])?;
    }
    Ok(())
}

impl Library {
    /// Splits credits stored as one combined artist ("A, B", "A feat. B &
    /// C") into their artists ([`split_credit`]), on tracks and albums: each
    /// gets linked to every artist it names (created when missing, so each
    /// has a page), the displayed line stays the same, and a combined artist
    /// left with nothing is removed. An artist with a MusicBrainz id is a
    /// real act and stays whole. Runs once per library (a `meta` mark);
    /// returns how many tracks and albums changed.
    pub fn split_combined_credits(&self) -> Result<usize> {
        if self.read(|c| meta_get(c, SPLIT_KEY))?.is_some() {
            return Ok(0);
        }
        self.write(|tx| {
            let mut names = Names::load(tx)?;
            let mut changed = 0;
            let mut replaced: Vec<ArtistId> = Vec::new();
            for (table, links, owner) in [("track", "track_artist", "track_id"), ("release", "release_artist", "release_id")] {
                for (id, json) in rows(tx, table)? {
                    let Ok(credit) = serde_json::from_str::<ArtistCredit>(&json) else { continue };
                    let Some(new) = split(&mut names, tx, &credit)? else { continue };
                    let (json, display, key) = credit_columns(&new)?;
                    tx.execute(
                        &format!(
                            "UPDATE {table} SET artist_credit = ?2, artist_display = ?3, artist_key = ?4
                              WHERE id = ?1"
                        ),
                        params![id, json, display, key],
                    )?;
                    write_credit_links(tx, links, owner, &id, &new)?;
                    let kept: Vec<ArtistId> = new.artist_ids().collect();
                    replaced.extend(credit.artist_ids().filter(|a| !kept.contains(a)));
                    changed += 1;
                }
            }
            replaced.sort_by_key(ToString::to_string);
            replaced.dedup();
            for a in replaced {
                drop_if_orphan(tx, a)?;
            }
            meta_set(tx, SPLIT_KEY, "1")?;
            if changed > 0 {
                log::info!("library: {changed} combined artist credits split");
            }
            Ok(changed)
        })
    }
}

#[cfg(test)]
mod tests {
    use nikgob_core::model::{Artist, ArtistCredit, Release, Track};

    use crate::Library;

    #[test]
    fn combined_credits_split_into_their_artists() {
        let lib = Library::open_in_memory().unwrap();
        let combined = lib.upsert_artist(&Artist::new("Кино, Земфира")).unwrap();
        let credit = ArtistCredit::single("Кино, Земфира", Some(combined.id));
        let mut release = Release::new("Live", credit.clone());
        release = lib.upsert_release(&release).unwrap();
        let mut track = Track::new("Song", credit);
        track.release_id = Some(release.id);
        let track = lib.upsert_track(&track).unwrap();
        // A real act from MusicBrainz stays whole.
        let mut band = Artist::new("Crosby, Nash");
        band.mbid = Some("mbid".into());
        let band = lib.upsert_artist(&band).unwrap();
        let other = lib
            .upsert_track(&Track::new("Other", ArtistCredit::single(&band.name, Some(band.id))))
            .unwrap();

        assert_eq!(lib.split_combined_credits().unwrap(), 2, "the track and the album");
        assert_eq!(lib.split_combined_credits().unwrap(), 0, "once");

        let got = lib.get_track(track.id).unwrap().unwrap();
        let names: Vec<_> = got.artist_credit.0.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Кино", "Земфира"]);
        assert_eq!(got.artist_credit.display(), "Кино, Земфира");
        for a in got.artist_credit.artist_ids() {
            assert_eq!(lib.tracks_of_artist(a, 0, 10).unwrap().len(), 1, "each artist has the track");
            assert!(lib.get_artist(a).unwrap().is_some(), "and a page");
        }
        let row = lib.get_track_row(track.id).unwrap().unwrap();
        assert_eq!(row.artist, "Кино, Земфира");
        assert_eq!(row.credit.0.len(), 2);
        let album = lib.get_release(release.id).unwrap().unwrap();
        assert_eq!(album.artist_credit.0.len(), 2);
        assert!(lib.get_artist(combined.id).unwrap().is_none(), "the combined artist is gone");
        let kept = lib.get_track(other.id).unwrap().unwrap();
        assert_eq!(kept.artist_credit.0.len(), 1);
    }
}
