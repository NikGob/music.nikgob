//! A readable name for an artist named in a script the user may not read
//! (table `artist_latin`): «佐宗綾子» is «Ayako Saso», the role «リコ» is
//! «Riko». The name is shown beside the artist's own, never instead of it.
//!
//! * The runtime looks the name up (MusicBrainz, Wikidata, the services)
//!   and records it with [`Library::set_artist_latin`]; a lookup that found
//!   nothing is recorded too and asked again later
//!   ([`Library::artists_needing_latin`]).
//! * The name is an alias of the artist ([`crate::aliases`]): search finds
//!   the artist by either name. A name proved the artist's (not a role's,
//!   not a romaji reading) also names it wherever it is credited, so a
//!   service that spells «Ayako Saso» lands on 佐宗綾子 and makes no second
//!   artist; an artist row the library already has under that name, or
//!   under the MusicBrainz id the lookup proved, is merged into it.
//! * A role (a voice credit `リコ(CV:富田美憂)`, or a MusicBrainz artist of
//!   type Character) is marked: it gets no photo lookups
//!   ([`Library::mark_voice_roles`]).

use std::collections::{BTreeSet, HashMap};

use nikgob_core::model::{ArtistCredit, ArtistId, UnixMs};
use nikgob_core::names::{romaji_name, same_name};
use rusqlite::{Connection, OptionalExtension, params};

use crate::aliases::{READING, add_alias, credited_together, merge_artists_in, refresh_track_search};
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
    /// The last lookup online; 0: never.
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
                "SELECT name, source, character, mbid, wikidata, fetched_at FROM artist_latin WHERE artist_id = ?1",
                [artist.to_string()],
                |r| {
                    Ok(ArtistLatin {
                        name: r.get(0)?,
                        source: r.get(1)?,
                        character: r.get(2)?,
                        mbid: r.get(3)?,
                        wikidata: r.get(4)?,
                        fetched_at: r.get(5)?,
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
    /// role, its MusicBrainz id and Wikidata item. Returns how many other
    /// artist rows turned out to be this artist and were merged into it.
    pub fn set_artist_latin(&self, artist: ArtistId, found: &LatinFound) -> Result<usize> {
        let device = self.device_id();
        let now = self.tick();
        let id = artist.to_string();
        self.write(|tx| {
            let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM artist WHERE id = ?1)", [&id], |r| r.get(0))?;
            if !exists {
                return Ok(0);
            }
            tx.execute(
                "INSERT INTO artist_latin(artist_id, name, source, character, mbid, wikidata, fetched_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(artist_id) DO UPDATE SET
                    name = coalesce(excluded.name, artist_latin.name),
                    source = CASE WHEN excluded.name IS NULL THEN artist_latin.source ELSE excluded.source END,
                    character = max(artist_latin.character, excluded.character),
                    mbid = coalesce(excluded.mbid, artist_latin.mbid),
                    wikidata = coalesce(excluded.wikidata, artist_latin.wikidata),
                    fetched_at = excluded.fetched_at",
                params![
                    id,
                    found.name.as_deref().map(str::trim).filter(|n| !n.is_empty()),
                    found.source,
                    found.character,
                    found.mbid,
                    found.wikidata,
                    now_ms()
                ],
            )?;
            let character: bool =
                tx.query_row("SELECT character FROM artist_latin WHERE artist_id = ?1", [&id], |r| r.get(0))?;
            let mut merged = 0;
            // The MusicBrainz id: a row that holds it is this artist.
            if let Some(mbid) = &found.mbid {
                let own: Option<String> = tx.query_row("SELECT mbid FROM artist WHERE id = ?1", [&id], |r| r.get(0))?;
                if own.is_none() {
                    let holder: Option<String> = tx
                        .query_row("SELECT id FROM artist WHERE mbid = ?1", [mbid], |r| r.get(0))
                        .optional()?;
                    if let Some(h) = holder.filter(|h| *h != id)
                        && !credited_together(tx, &h, &id)?
                        && merge_artists_in(tx, &h, &id, device.as_str(), now)?
                    {
                        merged += 1;
                    }
                    tx.execute(
                        "UPDATE artist SET mbid = ?2
                          WHERE id = ?1 AND mbid IS NULL AND NOT EXISTS (SELECT 1 FROM artist WHERE mbid = ?2)",
                        params![id, mbid],
                    )?;
                }
            }
            let Some(name) = found.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) else {
                return Ok(merged);
            };
            // A person's proved name names it everywhere; a role's or a
            // reading only in search.
            let proved = !character && found.source != ROMAJI;
            add_name(tx, &id, name, if proved { "latin" } else { READING })?;
            if !proved {
                return Ok(merged);
            }
            // An artist row of the library under that name is this artist.
            let mbid: Option<String> = tx.query_row("SELECT mbid FROM artist WHERE id = ?1", [&id], |r| r.get(0))?;
            let doubles: Vec<(String, Option<String>)> = {
                let mut stmt = tx.prepare_cached("SELECT id, name, mbid FROM artist WHERE id <> ?1")?;
                let rows = stmt.query_map([&id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
                    .into_iter()
                    .filter(|(_, n, _)| same_name(n, name))
                    .map(|(d, _, m)| (d, m))
                    .collect()
            };
            for (d, m) in doubles {
                if let (Some(x), Some(y)) = (&m, &mbid)
                    && x != y
                {
                    continue;
                }
                if !credited_together(tx, &d, &id)? && merge_artists_in(tx, &d, &id, device.as_str(), now)? {
                    merged += 1;
                }
            }
            Ok(merged)
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

        let found = LatinFound { name: Some("Ayako Saso".into()), source: "musicbrainz".into(), ..LatinFound::default() };
        assert_eq!(lib.set_artist_latin(saso.id, &found).unwrap(), 0);
        assert!(due(now_ms() + day).is_empty(), "a found name is not asked again");
        assert_eq!(lib.artist_latin(saso.id).unwrap().unwrap().name.as_deref(), Some("Ayako Saso"));
        assert_eq!(lib.artist_latin_names().unwrap()[&saso.id], (Some("Ayako Saso".to_owned()), false));
        // Search finds the artist and its songs by the Latin name.
        let hits = lib.search("Saso", 10).unwrap();
        assert_eq!(hits.artists.iter().map(|a| a.artist.id).collect::<Vec<_>>(), vec![saso.id]);
        assert_eq!(hits.tracks.len(), 1);
        // A service spelling it so lands on the artist.
        assert_eq!(lib.artist_by_alias("ayako saso").unwrap(), Some(saso.id));

        // A miss is asked again after a while and keeps nothing it had.
        let other = artist(&lib, "井澤詩織");
        song(&lib, "Other", ArtistCredit::single("井澤詩織", Some(other.id)));
        lib.set_artist_latin(other.id, &LatinFound::default()).unwrap();
        assert!(due(now_ms() - day).is_empty());
        assert_eq!(due(now_ms() + day), vec![other.id]);
    }

    #[test]
    fn a_double_under_the_proved_name_or_the_musicbrainz_id_is_merged() {
        let lib = Library::open_in_memory().unwrap();
        let saso = artist(&lib, "佐宗綾子");
        let latin = artist(&lib, "Saso Ayako");
        let mut held = Artist::new("さそうあやこ");
        held.mbid = Some("0b2f0b5a-0000-4000-8000-000000000001".into());
        let held = lib.upsert_artist(&held).unwrap();
        let a = song(&lib, "A", ArtistCredit::single("佐宗綾子", Some(saso.id)));
        let b = song(&lib, "B", ArtistCredit::single("Saso Ayako", Some(latin.id)));
        let c = song(&lib, "C", ArtistCredit::single("さそうあやこ", Some(held.id)));
        // Someone credited beside her stays someone else.
        let mut duo = ArtistCredit::single("佐宗綾子", Some(saso.id));
        let partner = artist(&lib, "Ayako Saso");
        duo.push(" & ", "Ayako Saso", Some(partner.id));
        song(&lib, "Duet", duo);

        let found = LatinFound {
            name: Some("Ayako Saso".into()),
            source: "musicbrainz".into(),
            mbid: held.mbid.clone(),
            ..LatinFound::default()
        };
        assert_eq!(lib.set_artist_latin(saso.id, &found).unwrap(), 2);
        for t in [a, b, c] {
            assert_eq!(lib.get_track_row(t).unwrap().unwrap().artist, "佐宗綾子");
        }
        assert_eq!(lib.get_artist(saso.id).unwrap().unwrap().mbid, held.mbid);
        assert!(lib.get_artist(latin.id).unwrap().is_none() && lib.get_artist(held.id).unwrap().is_none());
        assert!(lib.get_artist(partner.id).unwrap().is_some());
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
