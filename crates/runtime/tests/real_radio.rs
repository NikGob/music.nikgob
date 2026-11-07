//! A track's radio on a copy of a real library, as autoplay plays it from
//! the MADE IN ABYSS soundtrack's «Hail Hex» with «С новым»: how many of
//! one world come running, how far from the seed it goes, and how much new
//! music it brings from YouTube Music's radio of the seed (the answer the
//! library's cache keeps): `NIKGOB_REAL_LIBRARY=path/to/library.sqlite
//! cargo test -p nikgob-runtime --test real_radio -- --ignored
//! --nocapture`. The file itself is never written.

use nikgob_core::model::TrackId;
use nikgob_library::Library;
use nikgob_recs::{Ended, Item, NewTrack, RadioSeed, RadioState, Session};
use nikgob_runtime::ModelPreview;

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
fn a_radio_of_a_soundtrack() {
    let Some(src) = std::env::var_os("NIKGOB_REAL_LIBRARY") else {
        eprintln!("NIKGOB_REAL_LIBRARY is not set");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let lib = copy_of(&src, dir.path());
    let preview = ModelPreview::of(&lib).unwrap();
    let seed: TrackId = HAIL_HEX.parse().unwrap();
    if lib.get_track(seed).ok().flatten().is_none() {
        eprintln!("no «Hail Hex» in this library");
        return;
    }
    let new = preview.cached_new(&lib, "radio_new.youtube.radio.2CayMslBQp0");
    println!("new music in the cached radio of the seed: {}", new.len());
    let mut radio = RadioState::new(RadioSeed::Track(seed));
    radio.add_new(new.iter().map(|t| NewTrack::from_stream(t, None)));
    let mut session = Session::default();
    let mut ts = nikgob_library::now_ms();
    session.start(seed, ts, nikgob_core::model::EventContext::Radio, None);
    session.end(seed, Ended::Complete);
    let mut worlds: Vec<Option<u64>> = vec![preview.world(seed)];
    let (mut fresh, mut total, mut far) = (0usize, 0usize, 0usize);
    let mut played: Vec<String> = Vec::new();
    for batch in 0..4 {
        radio.attune(session.clone(), None);
        let picks = preview.extend_new(&mut radio, 10, 0.2);
        for p in &picks {
            total += 1;
            match &p.item {
                Item::Track(t) => {
                    let d = preview.distance(seed, *t);
                    if d.is_some_and(|d| d > 1.2) {
                        far += 1;
                    }
                    let w = preview.world(*t);
                    played.push(format!(
                        "  {batch} {} [{}] d {:.2}",
                        line(&lib, *t),
                        w.and_then(|w| preview.world_title(w)).unwrap_or_default(),
                        d.unwrap_or(f32::NAN)
                    ));
                    worlds.push(w);
                    ts += 200_000;
                    session.start(*t, ts, nikgob_core::model::EventContext::Radio, None);
                    session.end(*t, Ended::Complete);
                }
                Item::Stream(r) => {
                    fresh += 1;
                    let t = new.iter().find(|t| t.id == *r).map_or_else(|| r.to_string(), |t| format!("{} · {}", t.title, t.artist_line()));
                    played.push(format!("  {batch} NEW {t}"));
                    worlds.push(None);
                }
            }
        }
    }
    println!("{}", played.join("\n"));
    let runs = worlds.windows(3).filter(|w| w[0].is_some() && w[0] == w[1] && w[1] == w[2]).count();
    println!("{total} picks: {fresh} new ({:.0} %), {far} far from the seed, {runs} runs of one world", 100.0 * fresh as f32 / total as f32);
    assert_eq!(runs, 0, "a world thrice running");
    if new.len() >= 10 {
        assert!(fresh * 6 >= total, "about a fifth new: {fresh} of {total}");
    }
}
