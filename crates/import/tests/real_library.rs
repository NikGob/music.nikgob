//! A dry run of the start-up repairs on a copy of a real library:
//! `NIKGOB_REAL_LIBRARY=path/to/library.sqlite cargo test -p nikgob-import
//! --test real_library -- --ignored --nocapture`. The file itself is never
//! written; the copy lives in a temporary directory.

use nikgob_core::streaming::EntityKind;
use nikgob_library::{Library, ReleaseSort};

fn title_of(lib: &Library, id: nikgob_core::model::TrackId) -> String {
    let Some(t) = lib.get_track(id).unwrap() else {
        return "?".into();
    };
    let album = t
        .release_id
        .and_then(|r| lib.get_release(r).unwrap())
        .map(|r| r.title)
        .unwrap_or_default();
    format!("{:?} [{album}, {} ms]", t.title, t.duration_ms.unwrap_or(0))
}

#[test]
#[ignore = "needs NIKGOB_REAL_LIBRARY"]
fn repairs_of_a_real_library() {
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
    let releases = |lib: &Library| lib.releases(0, 100_000, ReleaseSort::asc(Default::default())).unwrap().len();

    let before: Vec<_> = lib
        .stream_only_tracks()
        .unwrap()
        .into_iter()
        .map(|t| {
            let ids = lib.external_ids(EntityKind::Track, *t.id.as_uuid()).unwrap();
            (t.id, title_of(&lib, t.id), ids)
        })
        .collect();
    let cards = releases(&lib);
    let merged = lib.merge_duplicate_stream_tracks().unwrap();
    println!("stream-only tracks: {}, merged: {merged}", before.len());
    for (id, title, ids) in &before {
        if lib.get_track(*id).unwrap().is_some() {
            continue;
        }
        let into = ids.iter().find_map(|r| lib.track_by_remote(r).unwrap());
        let into = into.map_or_else(|| "?".to_owned(), |i| title_of(&lib, i));
        println!("  {title}\n    -> {into}");
    }
    println!("album cards: {cards} -> {}", releases(&lib));

    let offers = nikgob_import::find_release_editions(&lib).unwrap();
    println!("edition offers: {}", offers.len());
    for g in &offers {
        let others: Vec<_> = g.others.iter().map(|r| r.release.title.as_str()).collect();
        println!(
            "  {} ({}): keep {:?} ({} tracks), fold {:?}: {} same songs, {} new",
            g.keep.release.artist_credit.display(),
            if g.group.starts_with("songs:") { "by songs" } else { "release group" },
            g.keep.release.title,
            g.keep.known_tracks,
            others,
            g.same_songs,
            g.new_songs
        );
    }
}
