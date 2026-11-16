//! The duplicate repairs on a copy of a real library, with every pair they
//! merge and the duplicates left after them:
//! `NIKGOB_REAL_LIBRARY=path/to/library.sqlite cargo test -p nikgob-library
//! --test real_duplicates -- --ignored --nocapture`. The file itself is
//! never written; the copy lives in a temporary directory.

use std::collections::HashMap;

use nikgob_library::Library;
use rusqlite::{Connection, params};

/// A live track as the report shows it: «title» · artists [album #disc-no,
/// ms] and what it has.
fn describe(c: &Connection, id: &str) -> String {
    c.query_row(
        "SELECT t.title, t.artist_display, coalesce(r.title, ''), t.disc_no, t.track_no, coalesce(t.duration_ms, 0),
                t.has_local, t.in_library,
                (SELECT group_concat(s.kind || ':' || CASE WHEN s.kind = 'local_file' THEN 'file' ELSE s.locator END, ' ')
                   FROM source s WHERE s.track_id = t.id AND s.trashed_at IS NULL)
           FROM track t LEFT JOIN release r ON r.id = t.release_id WHERE t.id = ?1",
        [id],
        |r| {
            Ok(format!(
                "«{}» · {} [{} #{}-{}, {} ms]{}{} {}",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
                if r.get::<_, bool>(6)? { " file" } else { "" },
                if r.get::<_, bool>(7)? { " in library" } else { " outside" },
                r.get::<_, Option<String>>(8)?.unwrap_or_default(),
            ))
        },
    )
    .unwrap_or_else(|_| format!("{id} (gone)"))
}

fn groups(c: &Connection, sql: &str) -> Vec<Vec<String>> {
    let mut stmt = c.prepare(sql).unwrap();
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    rows.into_iter().map(|ids| ids.split(',').map(str::to_owned).collect()).collect()
}

#[test]
#[ignore = "needs NIKGOB_REAL_LIBRARY"]
fn duplicates_of_a_real_library() {
    let Some(src) = std::env::var_os("NIKGOB_REAL_LIBRARY") else {
        eprintln!("NIKGOB_REAL_LIBRARY is not set");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("library.sqlite");
    std::fs::copy(&src, &db).unwrap();
    for ext in ["-wal", "-shm"] {
        let mut side = src.clone();
        side.push(ext);
        if std::path::Path::new(&side).exists() {
            let mut to = db.clone().into_os_string();
            to.push(ext);
            std::fs::copy(&side, to).unwrap();
        }
    }
    let lib = Library::open(&db).unwrap();
    let c = Connection::open(&db).unwrap();
    // A write through it would fire the change triggers, which need the library's functions.
    nikgob_library::register_functions(&c).unwrap();

    // Stream-only tracks and their sources, to find where each one went.
    let mut stmt = c
        .prepare(
            "SELECT t.id, s.kind, s.locator FROM track t JOIN source s ON s.track_id = t.id
              WHERE t.deleted_at IS NULL AND t.has_local = 0 AND s.kind != 'local_file'",
        )
        .unwrap();
    let mut before: HashMap<String, (String, Vec<(String, String)>)> = HashMap::new();
    for row in stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
        .unwrap()
    {
        let (id, kind, locator) = row.unwrap();
        let e = before.entry(id.clone()).or_insert_with(|| (describe(&c, &id), Vec::new()));
        e.1.push((kind, locator));
    }
    drop(stmt);
    let abyss = "01a168d5-7fd8-70f1-a872-1695a71eca2e";
    let count = |c: &Connection| -> i64 {
        c.query_row("SELECT count(*) FROM track WHERE release_id = ?1 AND deleted_at IS NULL", [abyss], |r| r.get(0))
            .unwrap()
    };
    let abyss_before = count(&c);

    let merged = lib.merge_duplicate_stream_tracks().unwrap();
    println!("stream_duplicates_merged.v5: {merged} merged");
    let mut pairs = Vec::new();
    for (id, (was, sources)) in &before {
        let gone: bool = c
            .query_row("SELECT NOT EXISTS(SELECT 1 FROM track WHERE id = ?1)", [id], |r| r.get(0))
            .unwrap();
        if !gone {
            continue;
        }
        let (kind, locator) = &sources[0];
        let into: Option<String> = c
            .query_row(
                "SELECT track_id FROM source WHERE kind = ?1 AND locator = ?2",
                params![kind, locator],
                |r| r.get(0),
            )
            .ok();
        pairs.push(format!("  {was}\n    -> {}", into.map_or_else(|| "?".into(), |i| describe(&c, &i))));
    }
    pairs.sort();
    for p in &pairs {
        println!("{p}");
    }
    let split = lib.split_combined_credits().unwrap();
    println!("combined_credits_split.v2: {split} tracks and albums");

    let rows = lib.tracks_of_release_all(abyss.parse().unwrap()).unwrap();
    let progress = lib.release_progress(abyss.parse().unwrap()).unwrap();
    println!(
        "MADE IN ABYSS ORIGINAL SOUNDTRACK 4: {abyss_before} tracks -> {} ({} on its page), {progress:?}",
        count(&c),
        rows.len()
    );
    for r in &rows {
        println!("  {}-{} {} · {}{}", r.disc_no, r.track_no, r.title, r.artist, if r.has_local { "" } else { " (stream)" });
    }

    // What is left: two live tracks at one place of a release, or one song
    // (title and artist keys) twice on a release.
    let places = groups(
        &c,
        "SELECT group_concat(id) FROM track WHERE deleted_at IS NULL AND release_id IS NOT NULL AND track_no > 0
          GROUP BY release_id, disc_no, track_no HAVING count(*) > 1",
    );
    println!("left at one place of a release: {}", places.len());
    for g in &places {
        println!("  -");
        for id in g {
            println!("    {}", describe(&c, id));
        }
    }
    let songs = groups(
        &c,
        "SELECT group_concat(id) FROM track WHERE deleted_at IS NULL AND release_id IS NOT NULL
          GROUP BY release_id, title_key, artist_key HAVING count(*) > 1",
    );
    println!("left as one title twice on a release: {}", songs.len());
    for g in &songs {
        println!("  -");
        for id in g {
            println!("    {}", describe(&c, id));
        }
    }
    assert_eq!(lib.merge_duplicate_stream_tracks().unwrap(), 0, "once");
}
