//! The one-time repairs of the catalog on a copy of a real library, with
//! every change they log and the albums they were made for:
//! `NIKGOB_REAL_LIBRARY=path/to/library.sqlite cargo test -p nikgob-library
//! --test real_catalog -- --ignored --nocapture`. The file itself is never
//! written; the copy lives in a temporary directory.

use nikgob_library::Library;
use rusqlite::Connection;

/// Prints the library's change notes.
struct Notes;

impl log::Log for Notes {
    fn enabled(&self, m: &log::Metadata<'_>) -> bool {
        m.level() <= log::Level::Info && m.target().starts_with("nikgob_library")
    }

    fn log(&self, r: &log::Record<'_>) {
        if self.enabled(r.metadata()) {
            println!("    {}", r.args());
        }
    }

    fn flush(&self) {}
}

fn copy_of(src: &std::path::Path, dir: &std::path::Path) -> std::path::PathBuf {
    let db = dir.join("library.sqlite");
    std::fs::copy(src, &db).unwrap();
    for ext in ["-wal", "-shm"] {
        let mut side = src.as_os_str().to_owned();
        side.push(ext);
        if std::path::Path::new(&side).exists() {
            let mut to = db.clone().into_os_string();
            to.push(ext);
            std::fs::copy(&side, to).unwrap();
        }
    }
    db
}

/// `(rows, with a file, total)` of a release, and its places.
fn album(c: &Connection, id: &str) -> Option<(i64, i64, Option<i64>, Vec<String>)> {
    let total: Option<i64> = c.query_row("SELECT total_tracks FROM release WHERE id = ?1", [id], |r| r.get(0)).ok()?;
    let mut stmt = c
        .prepare(
            "SELECT disc_no, track_no, title, has_local FROM track WHERE release_id = ?1 AND deleted_at IS NULL
              ORDER BY disc_no, track_no, seq",
        )
        .unwrap();
    let rows: Vec<(i64, i64, String, bool)> = stmt
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let files = rows.iter().filter(|r| r.3).count() as i64;
    let places = rows.iter().map(|(d, n, t, f)| format!("{d}-{n} {t}{}", if *f { "" } else { " (no file)" })).collect();
    Some((rows.len() as i64, files, total, places))
}

fn count(c: &Connection, sql: &str) -> i64 {
    c.query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
#[ignore = "needs NIKGOB_REAL_LIBRARY"]
fn repairs_of_a_real_catalog() {
    let Some(src) = std::env::var_os("NIKGOB_REAL_LIBRARY") else {
        eprintln!("NIKGOB_REAL_LIBRARY is not set");
        return;
    };
    let _ = log::set_logger(&Notes).map(|()| log::set_max_level(log::LevelFilter::Info));
    let dir = tempfile::tempdir().unwrap();
    let db = copy_of(std::path::Path::new(&src), dir.path());
    let lib = Library::open(&db).unwrap();
    let c = Connection::open(&db).unwrap();
    // A write through it would fire the change triggers, which need the library's functions.
    nikgob_library::register_functions(&c).unwrap();

    const OLIVER_STANDARD: &str = "01a165c6-08a2-76c2-90af-02b12ce56bb9";
    const OLIVER_DELUXE: &str = "01a1631b-6225-7562-8e49-ac0eb4f557a8";
    const EL_CAMINO: &str = "01a14b36-af31-7547-84d6-13cd8552a9c7";
    const NGNL_ZERO: &str = "01a1269b-89a5-72e0-a705-8edc822bda12";
    const MUSHOKU_OST1_SE: &str = "01a12682-7c6c-777d-a0e2-c4ad6bfd1637";
    const MUSHOKU_OST2_SE: &str = "01a12686-a2cc-761b-837b-fa14da0e3416";
    let watched = [
        ("Ugly Is Beautiful", OLIVER_STANDARD),
        ("Ugly Is Beautiful (Deluxe)", OLIVER_DELUXE),
        ("El Camino", EL_CAMINO),
        ("NO GAME NO LIFE ZERO OST", NGNL_ZERO),
        ("Mushoku Tensei OST 1 SE", MUSHOKU_OST1_SE),
        ("Mushoku Tensei OST 2 SE", MUSHOKU_OST2_SE),
    ];
    let show = |c: &Connection, verbose: bool| {
        for (name, id) in watched {
            match album(c, id) {
                Some((rows, files, total, places)) => {
                    println!("  {name}: {rows} rows, {files} with files, total {total:?}");
                    if verbose {
                        for p in places {
                            println!("      {p}");
                        }
                    }
                }
                None => println!("  {name}: gone"),
            }
        }
    };
    let summary = |c: &Connection| {
        println!(
            "  tracks {}, releases {}, empty releases {}, artists {}, artists without anything {}, places with two files {}",
            count(c, "SELECT count(*) FROM track WHERE deleted_at IS NULL"),
            count(c, "SELECT count(*) FROM release"),
            count(c, "SELECT count(*) FROM release r WHERE NOT EXISTS (SELECT 1 FROM track t WHERE t.release_id = r.id)"),
            count(c, "SELECT count(*) FROM artist"),
            count(
                c,
                "SELECT count(*) FROM artist a WHERE NOT EXISTS (SELECT 1 FROM track_artist WHERE artist_id = a.id)
                    AND NOT EXISTS (SELECT 1 FROM release_artist WHERE artist_id = a.id)"
            ),
            count(
                c,
                "SELECT count(*) FROM (SELECT 1 FROM track WHERE deleted_at IS NULL AND has_local = 1 AND track_no > 0
                   GROUP BY release_id, max(disc_no, 1), track_no HAVING count(*) > 1)"
            ),
        );
    };
    println!("before:");
    summary(&c);
    show(&c, true);

    println!("orphans_dropped.v1: {}", lib.drop_orphans().unwrap());
    println!("combined_credits_split.v3: {}", lib.split_combined_credits().unwrap());
    println!("various_artists_unbound.v1: {}", lib.unbind_various_artists().unwrap());
    let doubles = lib.merge_artist_doubles().unwrap();
    println!("artist_aliases_merged.v1: {} artists merged", doubles.len());
    println!("stream_duplicates_merged.v6: {}", lib.merge_duplicate_stream_tracks().unwrap());
    println!("edition_places_fixed.v1: {}", lib.fix_edition_places().unwrap());
    println!("album_editions_folded.v1: {}", lib.fold_album_editions().unwrap());
    println!("tracklists_reidentified.v1: {}", lib.reidentify_tracklists().unwrap());

    println!("after:");
    summary(&c);
    show(&c, true);
    let ids = |id: &str| -> Vec<String> {
        let mut stmt = c
            .prepare("SELECT service || ' ' || remote_id FROM external_id WHERE kind = 'release' AND local_id = ?1 ORDER BY 1")
            .unwrap();
        stmt.query_map([id], |r| r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap()
    };
    println!("  deluxe's ids: {:?}", ids(OLIVER_DELUXE));
    let mut stmt = c.prepare("SELECT title, scope, coalesce(release_id, '-') FROM wishlist WHERE artist = 'Oliver Tree'").unwrap();
    for w in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))).unwrap() {
        println!("  wish {:?}", w.unwrap());
    }
    drop(stmt);
    let mut stmt = c.prepare("SELECT name FROM artist ORDER BY name").unwrap();
    let names: Vec<String> = stmt.query_map([], |r| r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
    drop(stmt);
    println!("  artists now: {}", names.join(" · "));
    let mut stmt = c.prepare("SELECT a.name, al.name FROM artist_alias al JOIN artist a ON a.id = al.artist_id ORDER BY 1, 2").unwrap();
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).unwrap() {
        let (a, b) = row.unwrap();
        println!("  alias: {a} ← {b}");
    }
    drop(stmt);

    let (rows, files, total, _) = album(&c, OLIVER_DELUXE).unwrap();
    assert_eq!((rows, files, total.unwrap_or(rows).max(rows)), (21, 21, 21), "Oliver Tree: 21 of 21");
    assert!(album(&c, OLIVER_STANDARD).is_none(), "the standard edition is folded into the deluxe");
    let (rows, _, total, _) = album(&c, EL_CAMINO).unwrap();
    assert_eq!((rows, total), (11, Some(11)), "El Camino: the 11-track edition");
    assert_eq!(
        count(
            &c,
            "SELECT count(*) FROM (SELECT 1 FROM track WHERE deleted_at IS NULL AND has_local = 1 AND track_no > 0
               GROUP BY release_id, max(disc_no, 1), track_no HAVING count(*) > 1)"
        ),
        0,
        "no two files at one place"
    );

    // Every album: its page has a line per place it counts, its card the same counts.
    let mut stmt = c.prepare("SELECT id FROM release").unwrap();
    let releases: Vec<String> = stmt.query_map([], |r| r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
    drop(stmt);
    let mut holes = Vec::new();
    for id in &releases {
        let id = id.parse().unwrap();
        let l = lib.release_listing(id).unwrap().unwrap();
        assert_eq!(l.progress.total as usize, l.entries.len());
        if let Some(card) = lib.release_progress(id).unwrap() {
            assert_eq!((card.local, card.total), (l.progress.local, l.progress.total), "{id}");
        }
        if l.holes() > 0 {
            let title = lib.get_release(id).unwrap().unwrap().title;
            holes.push(format!("{title}: {} of {}, {} holes", l.progress.local, l.progress.total, l.holes()));
        }
    }
    println!("albums with holes: {}", holes.len());
    for h in &holes {
        println!("  {h}");
    }

    // Once.
    assert_eq!(lib.drop_orphans().unwrap(), 0);
    assert_eq!(lib.split_combined_credits().unwrap(), 0);
    assert!(!lib.unbind_various_artists().unwrap());
    assert!(lib.merge_artist_doubles().unwrap().is_empty());
    assert_eq!(lib.merge_duplicate_stream_tracks().unwrap(), 0);
    assert!(!lib.fix_edition_places().unwrap());
    assert!(!lib.fold_album_editions().unwrap());
    assert!(!lib.reidentify_tracklists().unwrap());
    let backups: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("backup-before-"))
        .collect();
    println!("backups: {backups:?}");
}
