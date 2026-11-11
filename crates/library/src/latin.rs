//! A readable name for an artist named in a script the user may not read
//! (table `artist_latin`): «佐宗綾子» is «Ayako Saso», the role «リコ» is
//! «Riko». The name is shown beside the artist's own, never instead of it.
//!
//! * The runtime looks the name up (MusicBrainz, Wikidata, the services)
//!   and records it with [`Library::set_artist_latin`]; a lookup that could
//!   not ask every source keeps the artist due, a complete one that found
//!   nothing is asked again later ([`Library::artists_needing_latin`]).
//! * The name is for display and search only: it is an alias of the kind
//!   `reading`, which search finds the artist by but which never decides
//!   which artist another credit is (an unrelated «Riko» stays someone
//!   else). Nothing is merged on it: two artist rows that turn out to be
//!   one MusicBrainz artist are only reported in the log.
//! * A role (a voice credit `リコ(CV:富田美憂)`, or a MusicBrainz artist of
//!   type Character) is marked: it gets no photo lookups and shows no
//!   automatic photo ([`Library::mark_voice_roles`]).

use std::collections::{BTreeSet, HashMap};

use nikgob_core::model::{ArtistCredit, ArtistId, UnixMs};
use nikgob_core::names::romaji_name;
use rusqlite::{Connection, OptionalExtension, params};

use crate::aliases::{READING, add_alias, refresh_track_search};
use crate::db::Library;
use crate::error::Result;
use crate::util::{get_id, now_ms};

/// `artist_latin.source` of a kana name read in romaji.
pub const ROMAJI: &str = "romaji";

/// The stored readable name of an artist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtistLatin {
    /// `None`: none found yet.
    pub name: Option<String>,
    /// Where the name came from (`musicbrainz`, `wikidata`, `youtube`,
    /// `spotify`, `romaji`).
    pub source: String,
    /// A role, not a person.
    pub character: bool,
    /// The MusicBrainz artist it was proved to be.
    pub mbid: Option<String>,
    /// That artist's Wikidata item.
    pub wikidata: Option<String>,
    /// The last MusicBrainz lookup; 0: never.
    pub identified_at: UnixMs,
    /// The last complete name lookup; 0: never.
    pub fetched_at: UnixMs,
}

/// What a lookup proved of an artist ([`Library::set_artist_latin`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LatinFound {
    /// The Latin name; `None` keeps the one recorded before (a miss).
    pub name: Option<String>,
    /// Where `name` came from.
    pub source: String,
    pub character: bool,
    pub mbid: Option<String>,
    pub wikidata: Option<String>,
    /// Every source of the name was asked (none failed for now): the
    /// lookup is not repeated before its retry time. Else the artist stays
    /// due for the names run.
    pub complete: bool,
}

/// The tracks crediting `artist` search by its aliases again.
fn refresh_tracks_of(conn: &Connection, artist: &str) -> Result<()> {
    let tracks: Vec<String> = {
        let mut stmt = conn.prepare_cached("SELECT track_id FROM track_artist WHERE artist_id = ?1")?;
        let rows = stmt.query_map([artist], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for t in &tracks {
        refresh_track_search(conn, t)?;
    }
    Ok(())
}

/// Records `name` as an alias of `artist` (see the module docs); the
/// search rows of its tracks follow when it is new.
fn add_name(conn: &Connection, artist: &str, name: &str, source: &str) -> Result<()> {
    let count = |c: &Connection| -> Result<i64> {
        Ok(c.query_row("SELECT count(*) FROM artist_alias WHERE artist_id = ?1", [artist], |r| r.get(0))?)
    };
    let before = count(conn)?;
    add_alias(conn, artist, name, source)?;
    if count(conn)? > before {
        refresh_tracks_of(conn, artist)?;
    }
    Ok(())
}

impl Library {
    /// The stored readable name of an artist, if it was looked up or marked.
    pub fn artist_latin(&self, artist: ArtistId) -> Result<Option<ArtistLatin>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT name, source, character, mbid, wikidata, identified_at, fetched_at
                   FROM artist_latin WHERE artist_id = ?1",
                [artist.to_string()],
                |r| {
                    Ok(ArtistLatin {
                        name: r.get(0)?,
                        source: r.get(1)?,
                        character: r.get(2)?,
                        mbid: r.get(3)?,
                        wikidata: r.get(4)?,
                        identified_at: r.get(5)?,
                        fetched_at: r.get(6)?,
                    })
                },
            )
            .optional()?)
        })
    }

    /// Every artist with a readable name or marked as a role: `(name, role)`.
    pub fn artist_latin_names(&self) -> Result<HashMap<ArtistId, (Option<String>, bool)>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT artist_id, name, character FROM artist_latin WHERE name IS NOT NULL OR character = 1",
            )?;
            let rows = stmt.query_map([], |r| Ok((get_id::<ArtistId>(r, 0)?, (r.get(1)?, r.get(2)?))))?;
            Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
        })
    }

    /// Artists of the library (a file, or a live library track) named in a
    /// foreign script whose Latin name is to be looked up: never looked up,
    /// or a miss or a romaji reading looked up before `retry_before` (unix
    /// ms). Never looked up first, then the oldest, most tracks first. A
    /// compilation's «Various Artists» is nobody.
    pub fn artists_needing_latin(&self, retry_before: UnixMs, limit: u32) -> Result<Vec<(ArtistId, String)>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT a.id, a.name FROM artist a LEFT JOIN artist_latin l ON l.artist_id = a.id
                  WHERE EXISTS (SELECT 1 FROM track_artist ta JOIN track t ON t.id = ta.track_id
                                 WHERE ta.artist_id = a.id
                                   AND (t.has_local = 1 OR (t.in_library = 1 AND t.deleted_at IS NULL)))
                    AND foreign_script(a.name) AND NOT various_artists(a.name)
                    AND (l.artist_id IS NULL
                         OR ((l.name IS NULL OR l.source = ?3) AND l.fetched_at < ?1))
                  ORDER BY coalesce(l.fetched_at, -1),
                           (SELECT COUNT(*) FROM track_artist ta WHERE ta.artist_id = a.id) DESC,
                           a.seq
                  LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![retry_before, limit, ROMAJI], |r| {
                Ok((get_id::<ArtistId>(r, 0)?, r.get::<_, String>(1)?))
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Records what a lookup proved of `artist` (see the module docs): its
    /// Latin name (a miss keeps the name recorded before), whether it is a
    /// role, its MusicBrainz id and Wikidata item. Never merges, never
    /// touches the artist row: another artist row that is the same
    /// MusicBrainz artist is reported in the log. Returns those rows' names.
    pub fn set_artist_latin(&self, artist: ArtistId, found: &LatinFound) -> Result<Vec<String>> {
        let id = artist.to_string();
        self.write(|tx| {
            let own: Option<String> =
                tx.query_row("SELECT name FROM artist WHERE id = ?1", [&id], |r| r.get(0)).optional()?;
            let Some(own) = own else { return Ok(Vec::new()) };
            let now = now_ms();
            tx.execute(
                "INSERT INTO artist_latin(artist_id, name, source, character, mbid, wikidata, identified_at, fetched_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, CASE WHEN ?8 THEN ?7 ELSE 0 END)
                 ON CONFLICT(artist_id) DO UPDATE SET
                    name = coalesce(excluded.name, artist_latin.name),
                    source = CASE WHEN excluded.name IS NULL THEN artist_latin.source ELSE excluded.source END,
                    character = max(artist_latin.character, excluded.character),
                    mbid = coalesce(excluded.mbid, artist_latin.mbid),
                    wikidata = coalesce(excluded.wikidata, artist_latin.wikidata),
                    identified_at = excluded.identified_at,
                    fetched_at = CASE WHEN ?8 THEN excluded.identified_at ELSE artist_latin.fetched_at END",
                params![
                    id,
                    found.name.as_deref().map(str::trim).filter(|n| !n.is_empty()),
                    found.source,
                    found.character,
                    found.mbid,
                    found.wikidata,
                    now,
                    found.complete
                ],
            )?;
            if let Some(name) = found.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                add_name(tx, &id, name, READING)?;
            }
            // The same MusicBrainz artist under another row: told, not merged.
            let mut same = Vec::new();
            if let Some(mbid) = &found.mbid {
                let mut stmt = tx.prepare_cached(
                    "SELECT name FROM artist WHERE id <> ?1
                        AND (mbid = ?2 OR id IN (SELECT artist_id FROM artist_latin WHERE mbid = ?2))",
                )?;
                let rows = stmt.query_map(params![id, mbid], |r| r.get::<_, String>(0))?;
                same = rows.collect::<rusqlite::Result<Vec<_>>>()?;
                for other in &same {
                    log::info!("library: artists «{own}» and «{other}» are one MusicBrainz artist {mbid}; not merged");
                }
            }
            Ok(same)
        })
    }

    /// Marks the roles of the voice credits (`リコ(CV:富田美憂)`: the part
    /// before `(CV:`) and gives a kana one its romaji. Returns how many
    /// artists were newly marked.
    pub fn mark_voice_roles(&self) -> Result<usize> {
        self.write(|tx| {
            let roles: BTreeSet<String> = {
                let mut stmt = tx.prepare_cached(
                    "SELECT artist_credit FROM track WHERE deleted_at IS NULL AND artist_credit LIKE '%CV%'",
                )?;
                let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
                let mut roles = BTreeSet::new();
                for json in rows {
                    let Ok(credit) = serde_json::from_str::<ArtistCredit>(&json?) else { continue };
                    let last = credit.0.len().saturating_sub(1);
                    for (i, p) in credit.0.iter().enumerate() {
                        if i < last
                            && nikgob_core::credit::is_voice_join(&p.join_phrase)
                            && let Some(a) = p.artist_id
                        {
                            roles.insert(a.to_string());
                        }
                    }
                }
                roles
            };
            let mut marked = 0;
            for id in roles {
                let name: Option<String> =
                    tx.query_row("SELECT name FROM artist WHERE id = ?1", [&id], |r| r.get(0)).optional()?;
                let Some(name) = name else { continue };
                let reading = romaji_name(&name);
                let changed = tx.execute(
                    "INSERT INTO artist_latin(artist_id, name, source, character, fetched_at) VALUES (?1, ?2, ?3, 1, 0)
                     ON CONFLICT(artist_id) DO UPDATE SET character = 1,
                        name = coalesce(artist_latin.name, excluded.name),
                        source = CASE WHEN artist_latin.name IS NULL THEN excluded.source ELSE artist_latin.source END
                     WHERE artist_latin.character = 0",
                    params![id, reading, if reading.is_some() { ROMAJI } else { "" }],
                )?;
                if changed == 0 {
                    continue;
                }
                marked += 1;
                let shown: Option<String> =
                    tx.query_row("SELECT name FROM artist_latin WHERE artist_id = ?1", [&id], |r| r.get(0))?;
                if let Some(shown) = shown {
                    add_name(tx, &id, &shown, READING)?;
                }
            }
            Ok(marked)
        })
    }
}

#[cfg(test)]
mod tests {
    use nikgob_core::model::{Artist, ArtistCredit, Codec, Source, SourceKind, Track};

    use super::*;

    fn artist(lib: &Library, name: &str) -> Artist {
        lib.upsert_artist(&Artist::new(name)).unwrap()
    }

    fn song(lib: &Library, title: &str, credit: ArtistCredit) -> nikgob_core::model::TrackId {
        let t = lib.upsert_track(&Track::new(title, credit)).unwrap();
        lib.add_source(&Source::new(t.id, SourceKind::LocalFile, format!("Music/{}.opus", t.id), Codec::Opus)).unwrap();
        t.id
    }

    #[test]
    fn foreign_names_are_queued_and_a_found_name_finds_the_artist() {
        let lib = Library::open_in_memory().unwrap();
        let saso = artist(&lib, "佐宗綾子");
        let kino = artist(&lib, "Кино");
        let yoasobi = artist(&lib, "YOASOBI");
        song(&lib, "Song", ArtistCredit::single("佐宗綾子", Some(saso.id)));
        song(&lib, "Группа крови", ArtistCredit::single("Кино", Some(kino.id)));
        song(&lib, "Idol", ArtistCredit::single("YOASOBI", Some(yoasobi.id)));
        let day = 86_400_000;
        let due = |at| lib.artists_needing_latin(at, 10).unwrap().into_iter().map(|(id, _)| id).collect::<Vec<_>>();
        assert_eq!(due(now_ms() - day), vec![saso.id], "Cyrillic and Latin names are read as they are");

        let found = LatinFound { name: Some("Ayako Saso".into()), source: "musicbrainz".into(), complete: true, ..LatinFound::default() };
        assert!(lib.set_artist_latin(saso.id, &found).unwrap().is_empty());
        assert!(due(now_ms() + day).is_empty(), "a found name is not asked again");
        assert_eq!(lib.artist_latin(saso.id).unwrap().unwrap().name.as_deref(), Some("Ayako Saso"));
        assert_eq!(lib.artist_latin_names().unwrap()[&saso.id], (Some("Ayako Saso".to_owned()), false));
        // Search finds the artist and its songs by the Latin name.
        let hits = lib.search("Saso", 10).unwrap();
        assert_eq!(hits.artists.iter().map(|a| a.artist.id).collect::<Vec<_>>(), vec![saso.id]);
        assert_eq!(hits.tracks.len(), 1);

        // A complete miss is asked again after a while; one that could not
        // ask every source (a service busy) stays due.
        let other = artist(&lib, "井澤詩織");
        song(&lib, "Other", ArtistCredit::single("井澤詩織", Some(other.id)));
        lib.set_artist_latin(other.id, &LatinFound { complete: true, ..LatinFound::default() }).unwrap();
        assert!(due(now_ms() - day).is_empty());
        assert_eq!(due(now_ms() + day), vec![other.id]);
        let third = artist(&lib, "伊瀬茉莉也");
        song(&lib, "Third", ArtistCredit::single("伊瀬茉莉也", Some(third.id)));
        lib.set_artist_latin(third.id, &LatinFound { mbid: Some("m".into()), ..LatinFound::default() }).unwrap();
        assert_eq!(due(now_ms() - day), vec![third.id]);
        assert!(lib.artist_latin(third.id).unwrap().unwrap().identified_at > 0);
    }

    /// A Latin name is display and search only: it never takes another
    /// credit of that spelling, and nothing is merged on it or on the
    /// MusicBrainz id (a row holding the same id is only reported).
    #[test]
    fn a_latin_name_neither_resolves_credits_nor_merges_artists() {
        let lib = Library::open_in_memory().unwrap();
        let saso = artist(&lib, "佐宗綾子");
        let latin = artist(&lib, "Saso Ayako");
        let swapped = artist(&lib, "Ayako Saso");
        let mut held = Artist::new("さそうあやこ");
        held.mbid = Some("0b2f0b5a-0000-4000-8000-000000000001".into());
        let held = lib.upsert_artist(&held).unwrap();
        song(&lib, "A", ArtistCredit::single("佐宗綾子", Some(saso.id)));
        let b = song(&lib, "B", ArtistCredit::single("Saso Ayako", Some(latin.id)));
        song(&lib, "C", ArtistCredit::single("さそうあやこ", Some(held.id)));

        let found = LatinFound {
            name: Some("Riko".into()),
            source: "musicbrainz".into(),
            mbid: held.mbid.clone(),
            complete: true,
            ..LatinFound::default()
        };
        assert_eq!(lib.set_artist_latin(saso.id, &found).unwrap(), vec!["さそうあやこ".to_owned()]);
        for a in [saso.id, latin.id, swapped.id, held.id] {
            assert!(lib.get_artist(a).unwrap().is_some(), "every row stays");
        }
        assert_eq!(lib.get_artist(saso.id).unwrap().unwrap().mbid, None, "the artist row is not touched");
        assert_eq!(lib.get_track_row(b).unwrap().unwrap().artist, "Saso Ayako");
        // Another «Riko» credit is not this artist.
        assert_eq!(lib.artist_by_alias("Riko").unwrap(), None);
        assert!(lib.artist_aliases().unwrap().is_empty());
        assert_eq!(lib.search("Riko", 10).unwrap().artists.len(), 1);
    }

    #[test]
    fn voice_roles_are_marked_and_read_in_romaji_only_in_search() {
        let lib = Library::open_in_memory().unwrap();
        let riko = artist(&lib, "リコ");
        let actor = artist(&lib, "富田美憂");
        let mut credit = ArtistCredit::single("リコ", Some(riko.id));
        credit.push("(CV:", "富田美憂", Some(actor.id));
        credit.0[1].join_phrase = ")".into();
        song(&lib, "Song of Riko", credit);
        assert_eq!(lib.mark_voice_roles().unwrap(), 1);
        assert_eq!(lib.mark_voice_roles().unwrap(), 0, "once");
        let row = lib.artist_latin(riko.id).unwrap().unwrap();
        assert!(row.character);
        assert_eq!((row.name.as_deref(), row.source.as_str(), row.fetched_at), (Some("Riko"), ROMAJI, 0));
        assert!(lib.artist_latin(actor.id).unwrap().is_none(), "the actor is a person");
        // Found by it, but «Riko» of another credit is not the role.
        assert_eq!(lib.search("Riko", 10).unwrap().artists.len(), 1);
        assert_eq!(lib.artist_by_alias("Riko").unwrap(), None);
        // Still looked up online once (MusicBrainz may know the role).
        assert_eq!(lib.artists_needing_latin(1, 10).unwrap().len(), 2);
    }
}
