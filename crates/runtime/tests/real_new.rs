//! New music and dislikes on a copy of a real library (recs-v2 §11,
//! docs/TODO.md §10): the taste profiles found, what the junk filter leaves
//! out of the services' answers kept, what one dislike lowers, and the
//! replay before and after random dislikes: `NIKGOB_REAL_LIBRARY=path/to/
//! library.sqlite cargo test -p nikgob-runtime --test real_new -- --ignored
//! --nocapture`. The file itself is never written; the copy lives in a
//! temporary directory.

use nikgob_core::model::TrackId;
use nikgob_library::Library;
use nikgob_runtime::ModelPreview;

/// «Hail Hex», a seed whose YouTube Music radio is in the cache.
const HAIL_HEX_VIDEO: &str = "2CayMslBQp0";
const HAIL_HEX: &str = "01a168d5-d38e-70b0-b97b-6a01765538f6";

fn copy_of(src: &std::ffi::OsStr, dir: &std::path::Path) -> Library {
    let db = dir.join("library.sqlite");
    std::fs::copy(src, &db).unwrap();
    for ext in ["-wal", "-shm"] {
        let mut side = src.to_owned();
        side.push(ext);
        if std::path::Path::new(&side).exists() {
            let mut to = db.clone().into_os_string();
            to.push(ext);
            std::fs::copy(&side, to).unwrap();
        }
    }
    Library::open(&db).unwrap()
}

fn line(lib: &Library, id: TrackId) -> String {
    match lib.get_track(id).ok().flatten() {
        Some(t) => format!("{} · {}", t.title, t.artist_credit.display()),
        None => id.to_string(),
    }
}

#[test]
#[ignore = "needs NIKGOB_REAL_LIBRARY"]
fn new_music_and_dislikes_of_a_real_library() {
    let Some(src) = std::env::var_os("NIKGOB_REAL_LIBRARY") else {
        eprintln!("NIKGOB_REAL_LIBRARY is not set");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let lib = copy_of(&src, dir.path());
    let preview = ModelPreview::of(&lib).unwrap();

    println!("taste profiles:");
    for l in preview.profiles(&lib) {
        println!("    {l}");
    }

    let key = format!("radio_new.youtube.radio.{HAIL_HEX_VIDEO}");
    let junk = preview.junk_found(&lib, &[key.as_str()]);
    println!("junk left out: {}", junk.len());
    for (t, j) in junk.iter().take(60) {
        let artist = t.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ");
        println!("    {} · {artist} ({:?} s): {j:?}", t.title, t.duration_ms.map(|d| d / 1000));
    }

    let seed: TrackId = HAIL_HEX.parse().unwrap();
    let (lower, same) = preview.dislike_effect(seed);
    println!("a dislike of {}: {} tracks a little lower", line(&lib, seed), lower.len());
    for (t, w) in lower.iter().take(8) {
        println!("    -{w:.2} {}", line(&lib, *t));
    }
    println!("its artist's other tracks: {}", same.len());
    for (t, w) in same.iter().take(8) {
        println!("    -{w:.2} {}", line(&lib, *t));
    }
    assert!(lower.iter().all(|(_, w)| *w <= nikgob_recs::dislike::CAP + 1e-4));

    for n in [5, 20, 50] {
        let (before, after, _) = preview.replay_dislikes(n, 7);
        let (b, a) = (before.get("all", "learned"), after.get("all", "learned"));
        println!(
            "{n:>2} random dislikes: hit@10 {:.3} → {:.3}, mrr {:.3} → {:.3}, coverage {:.3} → {:.3} (n {})",
            b.hit10, a.hit10, b.mrr, a.mrr, before.coverage, after.coverage, b.n
        );
    }
}
