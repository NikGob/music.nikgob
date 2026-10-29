//! One artist, several spellings (table `artist_alias`, docs/stage4.md
//! §12.13): «Ohara Yuiko» from the files is 大原ゆい子 of the services,
//! «Fujisawa Yoshiaki» and «Yoshiaki Fujisawa» are 藤澤慶昌. One track,
//! two titles (table `track_alias`): «ベラフの子守歌» of a service is the
//! file's «Belaf's Lullaby»; the title of the file stays, the other one
//! is searched too ([`add_track_alias_in`]).
//!
//! * [`merge_artists_in`] folds one artist row into another: every track
//!   and album credit, the ids in the services, the photo, the like and
//!   «не рекомендовать» move over, the credits say the name that stays, and
//!   the name that went is an alias of it;
//! * [`alias_of`] finds an artist by an alias, so a service or a file that
//!   still spells the old name lands on the artist that stays;
//! * [`Library::merge_artist_doubles`] finds the doubles once
//!   (`artist_aliases_merged.v1`): one name in another width, order, kana
//!   or Cyrillic reading ([`nikgob_core::names::same_name`]), and a kanji
//!   name with its romaji where the library shows it is one artist (an
//!   album credited to the kanji whose songs are all credited to the
//!   romaji; one song at one place of an album under both). The name in
//!   its own script is the one that stays ([`canonical`]); two artists of
//!   one credit, or with two MusicBrainz ids, are never one.

use std::collections::{BTreeSet, HashMap};

use nikgob_core::model::{ArtistCredit, ArtistId, TrackId};
use nikgob_core::names::{is_latin, is_native, latin_key, name_key, same_name, swapped};
use rusqlite::{Connection, OptionalExtension, params};

use crate::db::Library;
use crate::error::Result;
use crate::util::credit_columns;

/// `meta` key of [`Library::merge_artist_doubles`].
pub(crate) const DOUBLES_KEY: &str = "artist_aliases_merged.v1";

/// Records `name` as another spelling of `artist` (nothing for its own name).
pub(crate) fn add_alias(conn: &Connection, artist: &str, name: &str, source: &str) -> Result<()> {
    let key = name_key(name);
    if key.is_empty() {
        return Ok(());
    }
    let own: Option<String> = conn.query_row("SELECT name FROM artist WHERE id = ?1", [artist], |r| r.get(0)).optional()?;
    if own.is_some_and(|n| name_key(&n) == key) {
        return Ok(());
    }
    conn.execute(
        "INSERT OR IGNORE INTO artist_alias(artist_id, name, key, source) VALUES (?1, ?2, ?3, ?4)",
        params![artist, name.trim(), key, source],
    )?;
    Ok(())
}

/// The artist one of whose aliases is `name`, the oldest when several.
pub(crate) fn alias_of(conn: &Connection, name: &str) -> Result<Option<String>> {
    let key = name_key(name);
    if key.is_empty() {
        return Ok(None);
    }
    Ok(conn
        .query_row(
            "SELECT al.artist_id FROM artist_alias al JOIN artist a ON a.id = al.artist_id
              WHERE al.key = ?1 ORDER BY a.seq LIMIT 1",
            [key],
            |r| r.get(0),
        )
        .optional()?)
}

/// Rewrites the credits (json and links) of every track and album that
/// credit `from` so they credit `into` under `name`. Returns the tracks.
fn move_credits(conn: &Connection, from: &str, into: &str, name: &str) -> Result<Vec<String>> {
    let from_id: ArtistId = from.parse().map_err(|_| crate::error::LibraryError::Invalid(from.to_owned()))?;
    let into_id: ArtistId = into.parse().map_err(|_| crate::error::LibraryError::Invalid(into.to_owned()))?;
    let mut tracks = Vec::new();
    for (table, links, owner) in [("track", "track_artist", "track_id"), ("release", "release_artist", "release_id")] {
        let ids: Vec<(String, String)> = {
            let mut stmt = conn.prepare_cached(&format!(
                "SELECT id, artist_credit FROM {table} WHERE id IN (SELECT {owner} FROM {links} WHERE artist_id = ?1)
                    OR artist_credit LIKE '%' || ?1 || '%'"
            ))?;
            let rows = stmt.query_map([from], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for (id, json) in ids {
            let Ok(mut credit) = serde_json::from_str::<ArtistCredit>(&json) else { continue };
            for p in &mut credit.0 {
                if p.artist_id == Some(from_id) {
                    p.artist_id = Some(into_id);
                    p.name = name.to_owned();
                }
            }
            // The same artist twice in one credit («Ohara Yuiko, 大原ゆい子»): once.
            let mut seen = Vec::new();
            credit.0.retain(|p| p.artist_id.is_none_or(|a| !seen.contains(&a) && {
                seen.push(a);
                true
            }));
            let (json, display, key) = credit_columns(&credit)?;
            conn.execute(
                &format!("UPDATE {table} SET artist_credit = ?2, artist_display = ?3, artist_key = ?4 WHERE id = ?1"),
                params![id, json, display, key],
            )?;
            crate::catalog::write_credit_links(conn, links, owner, &id, &credit)?;
            if table == "track" {
                tracks.push(id);
            }
        }
    }
    Ok(tracks)
}

/// The search row of a track again (its artists' aliases changed).
pub(crate) fn refresh_track_search(conn: &Connection, track: &str) -> Result<()> {
    conn.execute("DELETE FROM track_fts WHERE rowid = (SELECT seq FROM track WHERE id = ?1)", [track])?;
    conn.execute(
        "INSERT INTO track_fts(rowid, title, artist, album)
         SELECT t.seq,
                replace(replace(t.title || coalesce(' ' || (SELECT group_concat(title, ' ') FROM track_alias
                                                             WHERE track_id = t.id), ''), 'ё', 'е'), 'Ё', 'Е'),
                replace(replace(t.artist_display || coalesce(' ' || (SELECT group_concat(al.name, ' ')
                            FROM track_artist ta JOIN artist_alias al ON al.artist_id = ta.artist_id
                           WHERE ta.track_id = t.id), ''), 'ё', 'е'), 'Ё', 'Е'),
                replace(replace(coalesce((SELECT title FROM release WHERE id = t.release_id), ''), 'ё', 'е'), 'Ё', 'Е')
           FROM track t WHERE t.id = ?1",
        [track],
    )?;
    Ok(())
}

/// Records `title` as another title of `track` (nothing for its own title
/// or one it has). Returns whether it is new.
pub(crate) fn add_track_alias_in(conn: &Connection, track: &str, title: &str, source: &str) -> Result<bool> {
    let key = crate::util::sort_key(title);
    if key.is_empty() {
        return Ok(false);
    }
    let own: Option<String> = conn.query_row("SELECT title_key FROM track WHERE id = ?1", [track], |r| r.get(0)).optional()?;
    if own.as_ref().is_none_or(|k| *k == key) {
        return Ok(false);
    }
    Ok(conn.execute(
        "INSERT OR IGNORE INTO track_alias(track_id, title, key, source) VALUES (?1, ?2, ?3, ?4)",
        params![track, title.trim(), key, source],
    )? > 0)
}

/// Folds the artist `from` into `into` (see the module docs). Returns
/// false when either is missing or they are one row.
pub(crate) fn merge_artists_in(conn: &Connection, from: &str, into: &str, device: &str, now: i64) -> Result<bool> {
    if from == into {
        return Ok(false);
    }
    let row = |id: &str| -> Result<Option<(String, Option<String>, String)>> {
        Ok(conn
            .query_row("SELECT name, mbid, images FROM artist WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?)
    };
    let (Some((from_name, from_mbid, from_images)), Some((into_name, into_mbid, into_images))) = (row(from)?, row(into)?) else {
        return Ok(false);
    };
    // Its names find `into` from now on.
    add_alias(conn, into, &from_name, "merged")?;
    conn.execute(
        "INSERT OR IGNORE INTO artist_alias(artist_id, name, key, source)
         SELECT ?2, name, key, source FROM artist_alias WHERE artist_id = ?1",
        params![from, into],
    )?;
    conn.execute("DELETE FROM artist_alias WHERE artist_id = ?1", [from])?;
    let tracks = move_credits(conn, from, into, &into_name)?;
    conn.execute("UPDATE external_id SET local_id = ?2 WHERE kind = 'artist' AND local_id = ?1", params![from, into])?;
    // A photo it has and `into` lacks.
    let has_photo: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM artist_photo WHERE artist_id = ?1 AND path IS NOT NULL)",
        [into],
        |r| r.get(0),
    )?;
    if !has_photo {
        conn.execute("DELETE FROM artist_photo WHERE artist_id = ?1 AND path IS NULL", [into])?;
        conn.execute("UPDATE OR IGNORE artist_photo SET artist_id = ?2 WHERE artist_id = ?1", params![from, into])?;
    }
    conn.execute("DELETE FROM artist_photo WHERE artist_id = ?1", [from])?;
    if into_images == "[]" && from_images != "[]" {
        conn.execute("UPDATE artist SET images = ?2 WHERE id = ?1", params![into, from_images])?;
    }
    // Its like and «не рекомендовать».
    let liked: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM likes WHERE target_kind = 'artist' AND target_id = ?1 AND deleted = 0)",
        [from],
        |r| r.get(0),
    )?;
    if liked {
        conn.execute(
            "INSERT INTO likes(target_kind, target_id, updated_at, updated_by, deleted) VALUES ('artist', ?1, ?2, ?3, 0)
             ON CONFLICT(target_kind, target_id) DO UPDATE SET deleted = 0, updated_at = excluded.updated_at,
                updated_by = excluded.updated_by",
            params![into, now, device],
        )?;
        conn.execute(
            "UPDATE likes SET deleted = 1, updated_at = ?2, updated_by = ?3 WHERE target_kind = 'artist' AND target_id = ?1",
            params![from, now, device],
        )?;
    }
    conn.execute(
        "UPDATE OR IGNORE rec_feedback SET target_id = ?2 WHERE target_kind = 'artist' AND target_id = ?1",
        params![from, into],
    )?;
    conn.execute("DELETE FROM rec_feedback WHERE target_kind = 'artist' AND target_id = ?1", [from])?;
    conn.execute("DELETE FROM meta WHERE key = ?1", [format!("cache.discography.{from}")])?;
    conn.execute("DELETE FROM artist WHERE id = ?1", [from])?;
    // After the delete: an MBID may be held by one row only.
    if into_mbid.is_none() && from_mbid.is_some() {
        conn.execute("UPDATE artist SET mbid = ?2 WHERE id = ?1", params![into, from_mbid])?;
    }
    for t in &tracks {
        refresh_track_search(conn, t)?;
    }
    note!("library: artist «{from_name}» {from} is «{into_name}» {into}, merged");
    Ok(true)
}

/// One artist of the library as the doubles search sees it.
#[derive(Clone, Debug)]
struct Who {
    id: String,
    name: String,
    mbid: Option<String>,
    tracks: i64,
    seq: i64,
}

/// The artist of a group that stays: the name in its own script, then the
/// one with a MusicBrainz id, then the one with more tracks, the oldest.
fn canonical<'a>(group: &[&'a Who]) -> &'a Who {
    group
        .iter()
        .copied()
        .max_by_key(|w| (is_native(&w.name), w.mbid.is_some(), w.tracks, std::cmp::Reverse(w.seq)))
        .expect("a group has members")
}

fn find(parent: &mut [usize], i: usize) -> usize {
    let mut r = i;
    while parent[r] != r {
        r = parent[r];
    }
    let mut j = i;
    while parent[j] != r {
        let next = parent[j];
        parent[j] = r;
        j = next;
    }
    r
}

/// Pairs of artists the library shows to be one (see the module docs),
/// with why.
fn evidence(conn: &Connection, who: &[Who]) -> Result<Vec<(usize, usize, &'static str)>> {
    let at: HashMap<&str, usize> = who.iter().enumerate().map(|(i, w)| (w.id.as_str(), i)).collect();
    let mut pairs = Vec::new();
    // One name in another width, order or reading.
    let mut by_key: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, w) in who.iter().enumerate() {
        let mut keys = vec![format!("n:{}", name_key(&w.name))];
        if let Some(l) = latin_key(&w.name) {
            keys.push(format!("l:{}", l.replace(' ', "")));
        }
        if let Some(s) = swapped(&w.name) {
            keys.push(format!("l:{}", s.replace(' ', "")));
        }
        for k in keys {
            by_key.entry(k).or_default().push(i);
        }
    }
    for ids in by_key.values() {
        for (n, &a) in ids.iter().enumerate() {
            for &b in &ids[n + 1..] {
                if a != b && same_name(&who[a].name, &who[b].name) {
                    pairs.push((a, b, "name"));
                }
            }
        }
    }
    // An album credited to a kanji name whose songs are credited to one
    // Latin name (at least half of them, none to the kanji one).
    let mut stmt = conn.prepare_cached(
        "SELECT r.id, (SELECT group_concat(artist_id) FROM release_artist WHERE release_id = r.id)
           FROM release r WHERE EXISTS (SELECT 1 FROM track t WHERE t.release_id = r.id AND t.deleted_at IS NULL)",
    )?;
    let releases: Vec<(String, Option<String>)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    drop(stmt);
    let mut credited = conn.prepare_cached(
        "SELECT t.id, (SELECT group_concat(artist_id) FROM (SELECT artist_id FROM track_artist ta
                                                              WHERE ta.track_id = t.id ORDER BY position))
           FROM track t WHERE t.release_id = ?1 AND t.deleted_at IS NULL",
    )?;
    for (rid, artists) in releases {
        let mine: Vec<&str> = artists.as_deref().map(|a| a.split(',').collect()).unwrap_or_default();
        let songs: Vec<Vec<String>> = credited
            .query_map([&rid], |r| r.get::<_, Option<String>>(1))?
            .map(|r| r.map(|a| a.map(|a| a.split(',').map(str::to_owned).collect()).unwrap_or_default()))
            .collect::<rusqlite::Result<_>>()?;
        if songs.is_empty() {
            continue;
        }
        let on_songs: BTreeSet<&str> = songs.iter().flatten().map(String::as_str).collect();
        let missing: Vec<usize> = mine
            .iter()
            .filter(|a| !on_songs.contains(**a))
            .filter_map(|a| at.get(*a).copied())
            .filter(|&i| is_native(&who[i].name) && latin_key(&who[i].name).is_none())
            .collect();
        let [x] = missing[..] else { continue };
        let mut mains: HashMap<&str, usize> = HashMap::new();
        for s in &songs {
            if let Some(m) = s.first() {
                *mains.entry(m.as_str()).or_default() += 1;
            }
        }
        let Some((&y, &n)) = mains.iter().max_by_key(|(id, n)| (**n, std::cmp::Reverse(**id))) else { continue };
        if n * 2 < songs.len() || mine.contains(&y) {
            continue;
        }
        if let Some(&y) = at.get(y)
            && is_latin(&who[y].name)
        {
            pairs.push((x, y, "album"));
        }
    }
    drop(credited);
    // One song at one place of an album under a kanji and a Latin name.
    let mut stmt = conn.prepare_cached(
        "SELECT a.title, b.title, (SELECT artist_id FROM track_artist WHERE track_id = a.id ORDER BY position LIMIT 1),
                (SELECT artist_id FROM track_artist WHERE track_id = b.id ORDER BY position LIMIT 1)
           FROM track a JOIN track b ON b.release_id = a.release_id AND b.disc_no = a.disc_no
                                    AND b.track_no = a.track_no AND b.seq > a.seq
          WHERE a.release_id IS NOT NULL AND a.track_no > 0 AND a.deleted_at IS NULL AND b.deleted_at IS NULL
            AND abs(a.duration_ms - b.duration_ms) <= 1500",
    )?;
    let rows: Vec<(String, String, Option<String>, Option<String>)> =
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<rusqlite::Result<_>>()?;
    for (ta, tb, aa, ab) in rows {
        let (Some(a), Some(b)) = (aa.and_then(|a| at.get(a.as_str()).copied()), ab.and_then(|b| at.get(b.as_str()).copied())) else {
            continue;
        };
        let same_title = crate::streams::song_key(&ta) == crate::streams::song_key(&tb);
        if a != b && same_title && is_native(&who[a].name) != is_native(&who[b].name) {
            pairs.push((a, b, "place"));
        }
    }
    Ok(pairs)
}

/// Two artists that may not be one: two MusicBrainz ids, or both named in
/// one credit.
fn apart(conn: &Connection, a: &Who, b: &Who) -> Result<bool> {
    if let (Some(x), Some(y)) = (&a.mbid, &b.mbid)
        && x != y
    {
        return Ok(true);
    }
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM track_artist x JOIN track_artist y ON y.track_id = x.track_id
                        WHERE x.artist_id = ?1 AND y.artist_id = ?2)
             OR EXISTS(SELECT 1 FROM release_artist x JOIN release_artist y ON y.release_id = x.release_id
                        WHERE x.artist_id = ?1 AND y.artist_id = ?2)",
        params![a.id, b.id],
        |r| r.get(0),
    )?)
}

/// The doubles to merge: `(from, into, why)` by artist id.
pub(crate) fn plan_doubles(conn: &Connection) -> Result<Vec<(String, String, String)>> {
    let who: Vec<Who> = {
        let mut stmt = conn.prepare_cached(
            "SELECT a.id, a.name, a.mbid, (SELECT count(*) FROM track_artist ta WHERE ta.artist_id = a.id), a.seq
               FROM artist a ORDER BY a.seq",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Who { id: r.get(0)?, name: r.get(1)?, mbid: r.get(2)?, tracks: r.get(3)?, seq: r.get(4)? })
        })?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut parent: Vec<usize> = (0..who.len()).collect();
    let mut why: HashMap<usize, Vec<&'static str>> = HashMap::new();
    for (a, b, w) in evidence(conn, &who)? {
        if apart(conn, &who[a], &who[b])? {
            continue;
        }
        let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
        // Never two MusicBrainz ids in one group.
        let mbids = |r: usize, parent: &mut Vec<usize>| -> BTreeSet<String> {
            (0..who.len()).filter(|&i| find(parent, i) == r).filter_map(|i| who[i].mbid.clone()).collect()
        };
        if ra != rb {
            let (ma, mb) = (mbids(ra, &mut parent), mbids(rb, &mut parent));
            if !ma.is_empty() && !mb.is_empty() && ma != mb {
                continue;
            }
            parent[ra] = rb;
        }
        why.entry(a).or_default().push(w);
        why.entry(b).or_default().push(w);
    }
    let mut groups: HashMap<usize, Vec<&Who>> = HashMap::new();
    for i in 0..who.len() {
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(&who[i]);
    }
    let mut out = Vec::new();
    let mut roots: Vec<_> = groups.into_iter().filter(|(_, g)| g.len() > 1).collect();
    roots.sort_by_key(|(_, g)| g.iter().map(|w| w.seq).min());
    for (_, g) in roots {
        let into = canonical(&g);
        for w in g {
            if w.id != into.id {
                let i = who.iter().position(|x| x.id == w.id).unwrap_or(0);
                let reasons: BTreeSet<&str> = why.get(&i).into_iter().flatten().copied().collect();
                out.push((w.id.clone(), into.id.clone(), reasons.into_iter().collect::<Vec<_>>().join("+")));
            }
        }
    }
    Ok(out)
}

impl Library {
    /// Records `title` as another title of `track` that search finds it by
    /// (see the module docs); `source` says where it came from (`import`,
    /// `merged`). Returns whether it is new.
    pub fn add_track_alias(&self, track: TrackId, title: &str, source: &str) -> Result<bool> {
        self.write(|tx| add_track_alias_in(tx, &track.to_string(), title, source))
    }

    /// The other titles of a track.
    pub fn track_aliases(&self, track: TrackId) -> Result<Vec<String>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached("SELECT title FROM track_alias WHERE track_id = ?1 ORDER BY key")?;
            let rows = stmt.query_map([track.to_string()], |r| r.get(0))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Folds the artist `from` into `into` (see the module docs).
    pub fn merge_artists(&self, from: ArtistId, into: ArtistId) -> Result<bool> {
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| merge_artists_in(tx, &from.to_string(), &into.to_string(), device.as_str(), now))
    }

    /// Every alias: `(artist, name)`.
    pub fn artist_aliases(&self) -> Result<Vec<(ArtistId, String)>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached("SELECT artist_id, name FROM artist_alias ORDER BY artist_id, name")?;
            let rows = stmt.query_map([], |r| Ok((crate::util::get_id(r, 0)?, r.get(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// The artist known under the alias `name`, if any.
    pub fn artist_by_alias(&self, name: &str) -> Result<Option<ArtistId>> {
        self.read(|c| Ok(alias_of(c, name)?.and_then(|id| id.parse().ok())))
    }

    /// Merges the doubles of artists (see the module docs) once per
    /// library; returns `(from, into)` names of every merge.
    pub fn merge_artist_doubles(&self) -> Result<Vec<(String, String)>> {
        let device = self.device_id();
        let now = self.tick();
        self.repair(DOUBLES_KEY, |tx| {
            let mut done = Vec::new();
            for (from, into, why) in plan_doubles(tx)? {
                let names = |id: &str| -> Result<String> {
                    Ok(tx.query_row("SELECT name FROM artist WHERE id = ?1", [id], |r| r.get(0)).optional()?.unwrap_or_default())
                };
                let (f, i) = (names(&from)?, names(&into)?);
                if merge_artists_in(tx, &from, &into, device.as_str(), now)? {
                    note!("library: artist double «{f}» → «{i}» ({why})");
                    done.push((f, i));
                }
            }
            let n = done.len();
            Ok((done, n))
        })
        .map(Option::unwrap_or_default)
    }
}


#[cfg(test)]
mod tests {
    use nikgob_core::model::{Artist, ArtistCredit, Codec, Release, Source, SourceKind, Track, TrackId};

    use super::*;

    /// A file of `title` credited to `artists` on the album `album` by `by`.
    fn song(lib: &Library, title: &str, artists: &[&Artist], album: (&str, &Artist), no: u32, ms: u32) -> TrackId {
        let mut credit = ArtistCredit::default();
        for a in artists {
            credit.push(" & ", a.name.clone(), Some(a.id));
        }
        let by = ArtistCredit::single(album.1.name.clone(), Some(album.1.id));
        let found: Option<String> = lib
            .read(|c| Ok(c.query_row("SELECT id FROM release WHERE title = ?1", [album.0], |r| r.get(0)).optional()?))
            .unwrap();
        let release = match found {
            Some(id) => id.parse().unwrap(),
            None => lib.upsert_release(&Release::new(album.0, by)).unwrap().id,
        };
        let mut t = Track::new(title, credit);
        t.release_id = Some(release);
        t.track_no = no;
        t.duration_ms = Some(ms);
        let t = lib.upsert_track(&t).unwrap();
        lib.add_source(&Source::new(t.id, SourceKind::LocalFile, format!("Music/{}.opus", t.id), Codec::Opus)).unwrap();
        t.id
    }

    fn artist(lib: &Library, name: &str) -> Artist {
        lib.upsert_artist(&Artist::new(name)).unwrap()
    }

    fn names(lib: &Library) -> Vec<String> {
        let mut out: Vec<String> = lib
            .read(|c| {
                let mut stmt = c.prepare_cached("SELECT name FROM artist")?;
                let rows = stmt.query_map([], |r| r.get(0))?;
                Ok(rows.collect::<rusqlite::Result<_>>()?)
            })
            .unwrap();
        out.sort();
        out
    }

    #[test]
    fn doubles_are_merged_into_the_name_in_its_own_script() {
        let lib = Library::open_in_memory().unwrap();
        // One name in two scripts and two orders.
        let hitorie = artist(&lib, "ヒトリエ");
        let latin = artist(&lib, "HITORIE");
        let fujisawa = artist(&lib, "藤澤慶昌");
        let f1 = artist(&lib, "Fujisawa Yoshiaki");
        let f2 = artist(&lib, "Yoshiaki Fujisawa");
        let ohara = artist(&lib, "大原ゆい子");
        let yuiko = artist(&lib, "Ohara Yuiko");
        let wide = artist(&lib, "ＹＯＡＳＯＢＩ");
        let narrow = artist(&lib, "YOASOBI");
        let a = song(&lib, "SLEEPWALK", &[&hitorie], ("HOWLS", &hitorie), 1, 200_000);
        let b = song(&lib, "Senseless Wonder", &[&latin], ("ai/SOlate", &latin), 1, 210_000);
        // The kanji album whose songs say the romaji.
        for (no, t) in ["Hajimari", "Kaze no Uta", "Owari"].iter().enumerate() {
            song(&lib, t, &[&f1], ("無職転生 オリジナル・サウンドトラック", &fujisawa), no as u32 + 1, 100_000 + no as u32);
        }
        song(&lib, "Tabi no Hidarite", &[&f2], ("Other", &f2), 1, 120_000);
        // One song at one place under both names.
        song(&lib, "Clover", &[&ohara], ("Clover", &ohara), 1, 245_000);
        song(&lib, "Clover", &[&yuiko], ("Clover", &ohara), 1, 245_400);
        song(&lib, "夜に駆ける", &[&wide], ("THE BOOK", &wide), 1, 258_000);
        song(&lib, "群青", &[&narrow], ("THE BOOK", &wide), 2, 248_000);
        lib.like(latin.id).unwrap();

        let merged = lib.merge_artist_doubles().unwrap();
        assert_eq!(merged.len(), 5, "{merged:?}");
        let mut left = ["ヒトリエ", "大原ゆい子", "藤澤慶昌", "ＹＯＡＳＯＢＩ"].map(String::from).to_vec();
        left.sort();
        assert_eq!(names(&lib), left);
        assert_eq!(lib.get_track_row(b).unwrap().unwrap().artist, "ヒトリエ");
        assert_eq!(lib.get_track_row(a).unwrap().unwrap().artist, "ヒトリエ");
        assert!(lib.is_liked(hitorie.id).unwrap(), "the like moved");
        assert_eq!(lib.artist_by_alias("Hitorie").unwrap(), Some(hitorie.id));
        assert_eq!(lib.artist_by_alias("yoshiaki fujisawa").unwrap(), Some(fujisawa.id));
        assert_eq!(lib.artist_by_alias("Ohara Yuiko").unwrap(), Some(ohara.id));
        // The Latin names still find the songs.
        let hits = lib.search("Fujisawa", 10).unwrap();
        assert_eq!(hits.tracks.len(), 4);
        assert!(hits.tracks.iter().all(|t| t.artist == "藤澤慶昌"));
        let hits = lib.search("Ohara", 10).unwrap().tracks;
        assert!(!hits.is_empty() && hits.iter().all(|t| t.artist == "大原ゆい子"));
        assert!(lib.merge_artist_doubles().unwrap().is_empty(), "once");
    }

    #[test]
    fn artists_who_only_share_a_song_title_or_a_credit_stay_apart() {
        let lib = Library::open_in_memory().unwrap();
        // «Лёд» by two artists, one length.
        let a = artist(&lib, "Artik & Asti");
        let b = artist(&lib, "Мот");
        song(&lib, "Лёд", &[&a], ("Лёд", &a), 1, 200_000);
        song(&lib, "Лёд", &[&b], ("Лёд (single)", &b), 1, 200_000);
        // Two people named in one credit, two MusicBrainz ids, short names.
        let x = artist(&lib, "ヨルシカ");
        let y = artist(&lib, "Yorushika");
        song(&lib, "ただ君に晴れ", &[&x, &y], ("負け犬にアンコールはいらない", &x), 1, 200_000);
        let mut m1 = Artist::new("Kino");
        m1.mbid = Some("11111111-1111-1111-1111-111111111111".into());
        let mut m2 = Artist::new("Кино");
        m2.mbid = Some("22222222-2222-2222-2222-222222222222".into());
        let (m1, m2) = (lib.upsert_artist(&m1).unwrap(), lib.upsert_artist(&m2).unwrap());
        song(&lib, "Song A", &[&m1], ("A", &m1), 1, 100_000);
        song(&lib, "Song B", &[&m2], ("B", &m2), 1, 100_000);
        let s1 = artist(&lib, "ア");
        let s2 = artist(&lib, "A");
        song(&lib, "S1", &[&s1], ("S1", &s1), 1, 100_000);
        song(&lib, "S2", &[&s2], ("S2", &s2), 1, 100_000);
        assert!(lib.merge_artist_doubles().unwrap().is_empty());
        assert_eq!(names(&lib).len(), 8);
    }
}
