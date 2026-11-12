//! The listener model on a copy of a real library (recs-v2 §13, §15
//! phase 2): the prequential replay of its history, every start predicted
//! by what was learnt before it, against the affinity of phase 1, and a
//! track's radio on the model, as an album goes on and after two early
//! skips: `NIKGOB_REAL_LIBRARY=path/to/library.sqlite cargo test -p
//! nikgob-runtime --test real_replay -- --ignored --nocapture`. The file
//! itself is never written; the copy lives in a temporary directory.

use std::collections::HashMap;

use nikgob_core::model::{EventContext, EventType, TrackId};
use nikgob_library::{Library, now_ms};
use nikgob_recs::{Ended, Item, Outcome, Pick, RadioSeed, RadioState, Session};
use nikgob_runtime::ModelPreview;

/// The AJR song whose lyrics have one Cyrillic letter.
const AJR: &str = "01a1265c-4b44-73b5-8df8-5f8df8e3af4d";

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

fn picks(lib: &Library, picks: &[Pick]) -> String {
    picks
        .iter()
        .filter_map(|p| match p.item {
            Item::Track(t) => Some(format!("    {} ({:?})", line(lib, t), p.reason)),
            Item::Stream(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
#[ignore = "needs NIKGOB_REAL_LIBRARY"]
fn the_model_of_a_real_library() {
    let Some(src) = std::env::var_os("NIKGOB_REAL_LIBRARY") else {
        eprintln!("NIKGOB_REAL_LIBRARY is not set");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let lib = copy_of(&src, dir.path());
    let r = nikgob_runtime::preview_replay(&lib).unwrap();
    for (split, model, h) in &r.splits {
        println!("{split:>22} {model:>16}: n {:>5} hit@1 {:.3} hit@10 {:.3} mrr {:.3}", h.n, h.hit1, h.hit10, h.mrr);
    }
    println!(
        "early skip AUC: model {:?}, affinity {:?}, history {:?} (on {} starts)",
        r.skip_auc, r.skip_auc_affinity, r.skip_auc_history, r.skip_test
    );
    println!("learnt: {:?}", r.weights.next.w);
    println!("skip: {:?}", r.weights.skip.w);
    let learned = r.get("all", "learned");
    assert!(learned.hit10 >= 0.5, "hit@10 {learned:?}");
    assert!(learned.hit10 > r.get("all", "phase1_affinity").hit10);

    let preview = ModelPreview::of(&lib).unwrap();
    let ajr: TrackId = AJR.parse().unwrap();
    if lib.get_track(ajr).ok().flatten().is_some() {
        let ty = preview.song_type(ajr);
        println!("{}: {ty:?}", line(&lib, ajr));
        assert_ne!(ty.and_then(|t| t.lang), Some(nikgob_recs::Lang::Russian));
    }

    // An album going on: its first two played, the radio of the second.
    let events = lib.listening_events().unwrap();
    let mut starts: HashMap<TrackId, u32> = HashMap::new();
    for e in events.iter().filter(|e| e.event_type == EventType::Start) {
        *starts.entry(e.track_id).or_default() += 1;
    }
    let mut by_album: HashMap<_, u32> = HashMap::new();
    for (t, n) in &starts {
        if let Some(r) = lib.get_track(*t).ok().flatten().and_then(|t| t.release_id) {
            *by_album.entry(r).or_default() += n;
        }
    }
    let mut albums: Vec<_> = by_album.into_iter().collect();
    albums.sort_by(|a, b| b.1.cmp(&a.1));
    let now = now_ms();
    // One not played in the last hour (what played lately rests).
    let mut last_play: HashMap<TrackId, i64> = HashMap::new();
    for e in &events {
        let ts = last_play.entry(e.track_id).or_insert(e.ts);
        *ts = (*ts).max(e.ts);
    }
    let rested = |t: &[nikgob_library::TrackRow]| t.iter().all(|t| last_play.get(&t.id).is_none_or(|ts| now - ts > 3_600_000));
    if let Some(tracks) = albums.iter().map(|(r, _)| lib.tracks_of_release(*r).unwrap()).find(|t| t.len() >= 6 && rested(t)) {
        let mut session = Session::default();
        session.start(tracks[0].id, now - 400_000, EventContext::Album, None);
        session.end(tracks[0].id, Ended::Complete);
        session.start(tracks[1].id, now - 200_000, EventContext::Album, None);
        let mut radio = RadioState::new(RadioSeed::Track(tracks[1].id));
        radio.attune(session, None);
        let next = preview.extend(&mut radio, 8);
        println!("album «{}»: after {} and {}:\n{}", tracks[0].album.clone().unwrap_or_default(), tracks[0].title, tracks[1].title, picks(&lib, &next));
    }

    // Two early skips: the radio of the most played song, its first two
    // picks skipped.
    if let Some((seed, _)) = starts.iter().max_by_key(|(t, n)| (**n, **t)) {
        let mut session = Session::default();
        session.start(*seed, now - 200_000, EventContext::Radio, None);
        session.end(*seed, Ended::Complete);
        let mut radio = RadioState::new(RadioSeed::Track(*seed));
        radio.attune(session, None);
        let first = preview.extend(&mut radio, 6);
        println!("radio of {}:\n{}", line(&lib, *seed), picks(&lib, &first));
        for p in first.iter().take(2) {
            radio.feedback(p.item.clone(), Outcome::skip(8_000));
        }
        let after = preview.extend(&mut radio, 6);
        println!("after two early skips:\n{}", picks(&lib, &after));
        // How near the skipped two the picks sound, before and after.
        let skipped: Vec<TrackId> = first.iter().take(2).filter_map(|p| match p.item {
            Item::Track(t) => Some(t),
            Item::Stream(_) => None,
        }).collect();
        let near = |ps: &[Pick]| {
            let d: Vec<f32> = ps
                .iter()
                .filter_map(|p| match p.item {
                    Item::Track(t) => Some(t),
                    Item::Stream(_) => None,
                })
                .flat_map(|t| skipped.iter().map(move |s| (t, *s)))
                .filter_map(|(t, s)| preview.distance(t, s))
                .collect();
            d.iter().sum::<f32>() / d.len().max(1) as f32
        };
        println!("distance to the skipped: the rest of the first six {:.2}, after the skips {:.2}", near(&first[2..]), near(&after));
    }
}

/// What a copy of a real library is built of, written for the offline
/// evaluation of `nikgob_recs::eval` (its `dataset_report`):
/// `NIKGOB_REAL_LIBRARY=path/to/library.sqlite NIKGOB_RECS_DATASET=out.json
/// cargo test -p nikgob-runtime --test real_replay -- --ignored dataset`.
#[test]
#[ignore = "needs NIKGOB_REAL_LIBRARY and NIKGOB_RECS_DATASET"]
fn the_dataset_of_a_real_library() {
    let (Some(src), Some(out)) = (std::env::var_os("NIKGOB_REAL_LIBRARY"), std::env::var_os("NIKGOB_RECS_DATASET")) else {
        eprintln!("NIKGOB_REAL_LIBRARY or NIKGOB_RECS_DATASET is not set");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let lib = copy_of(&src, dir.path());
    let preview = ModelPreview::of(&lib).unwrap();
    let d = preview.dataset("real library (copy)");
    println!("{} tracks, {} vectors, {} events, {} graph edges", d.tracks.len(), d.vectors.len(), d.events.len(), d.graph_tracks.len() + d.graph_artists.len());
    std::fs::write(out, serde_json::to_vec(&d).unwrap()).unwrap();
}
