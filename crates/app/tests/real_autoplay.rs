//! Autoplay from a seed of a copy of a real library, as it would have gone
//! on when the seed ended: the session up to it (the copy given should end
//! there), the seed's album as the queue, the seed known
//! by its artists and sound when the catalog lacks it (a stream played from
//! its album's page). Prints the picks with their language and profile.
//!
//! `NIKGOB_AUTOPLAY_DB=<a library>/library.sqlite NIKGOB_AUTOPLAY_SEED=<track id>
//! [NIKGOB_AUTOPLAY_WORK=<dir>] [NIKGOB_AUTOPLAY_OUT=<file>] cargo test -p
//! nikgob --test real_autoplay -- --ignored --nocapture`
//!
//! The library given is copied (with its journal) into the work folder the
//! first time and only the copy is opened (graph rows put into it stay for
//! the next run); a work folder inside the app's own data is refused.

use std::path::{Path, PathBuf};

use nikgob_core::model::{EventType, TrackId};
use nikgob_library::Library;
use nikgob_recs::{Ended, Item, RadioParams, RadioSeed, RadioState, Recs, SeedFacts, Session};
use nikgob_runtime::ModelPreview;
use nikgob_view::Prefs;
use serde_json::{Value, json};

fn work_copy(source: &Path, dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    if let Some(data) = directories::ProjectDirs::from("dev", "nikgob", "music.nikgob").and_then(|d| d.data_dir().canonicalize().ok()) {
        assert!(!dir.starts_with(&data), "{} is the app's own data: give another NIKGOB_AUTOPLAY_WORK", dir.display());
    }
    let db = dir.join("library.sqlite");
    assert_ne!(source.canonicalize().unwrap(), db, "NIKGOB_AUTOPLAY_DB is the work copy itself");
    if !db.exists() {
        for ext in ["", "-wal", "-shm"] {
            let from = PathBuf::from(format!("{}{ext}", source.display()));
            if from.exists() {
                std::fs::copy(&from, dir.join(format!("library.sqlite{ext}"))).unwrap();
            }
        }
    }
    db
}

fn facts(lib: &Library, p: &ModelPreview, seed: TrackId) -> Option<SeedFacts> {
    let (cat, _) = p.catalog();
    if cat.get(seed).is_some() {
        return None;
    }
    let t = lib.get_track(seed).unwrap()?;
    let vector = lib.get_analysis(seed).unwrap().and_then(|a| <[f32; nikgob_recs::DIM]>::try_from(a.bliss_vector.as_slice()).ok());
    let lane = nikgob_recs::songtype::title_language(&t.title).map(|l| match l {
        nikgob_recs::Lang::Russian => nikgob_recs::SongLane::Russian,
        nikgob_recs::Lang::Japanese => nikgob_recs::SongLane::Japanese,
        _ => nikgob_recs::SongLane::Unknown,
    });
    Some(SeedFacts { artists: t.artist_credit.artist_ids().collect(), vector, lane })
}

#[test]
#[ignore = "needs NIKGOB_AUTOPLAY_DB and NIKGOB_AUTOPLAY_SEED"]
fn autoplay_from_a_seed_of_a_real_library() {
    let (Some(source), Some(seed)) = (std::env::var_os("NIKGOB_AUTOPLAY_DB"), std::env::var("NIKGOB_AUTOPLAY_SEED").ok()) else { return };
    let dir = std::env::var_os("NIKGOB_AUTOPLAY_WORK").map_or_else(|| std::env::temp_dir().join("nikgob-real-autoplay"), PathBuf::from);
    let db = work_copy(&PathBuf::from(source), &dir);
    let seed: TrackId = seed.parse().expect("a track id");
    let lib = Library::open(&db).unwrap();
    let events = lib.listening_events().unwrap();
    let start = events.iter().filter(|e| e.track_id == seed && e.event_type == EventType::Start).map(|e| e.ts).max().expect("the seed played");
    let mut session = Session::default();
    for e in events.iter().filter(|e| e.ts >= start - 6 * 3_600_000) {
        match e.event_type {
            EventType::Start => {
                session.start(e.track_id, e.ts, e.context, e.origin);
            }
            EventType::Complete => session.end(e.track_id, Ended::Complete),
            _ => session.end(e.track_id, if e.position_ms.unwrap_or(0) < 30_000 { Ended::Early } else { Ended::Late }),
        }
    }
    // The queue: the seed's album as the library has it.
    let row = lib.get_track(seed).unwrap().unwrap();
    let queue: Vec<TrackId> = row.release_id.map(|r| lib.tracks_of_release(r).unwrap().into_iter().map(|t| t.id).collect()).unwrap_or_default();
    let mut out = serde_json::Map::new();
    for (name, prefs) in [("no_graph", Prefs { graph_use: false, ..Prefs::default() }), ("graph", Prefs::default())] {
        let p = ModelPreview::of_with(&lib, &prefs).unwrap();
        let m = p.model();
        let (cat, idx) = p.catalog();
        let recs = Recs::new(cat, &m.taste, idx).with_model(m);
        let mut radio = RadioState::new(RadioSeed::Track(seed)).continuing();
        if let Some(f) = facts(&lib, &p, seed) {
            radio = radio.with_seed_facts(f);
        }
        radio.mark_served(queue.iter().map(|t| Item::Track(*t)));
        radio.attune(session.clone(), None);
        let mut picks = Vec::new();
        for k in 0..2u64 {
            for pick in radio.extend(&recs, 10, &RadioParams { new_share: 0.0, seed: 7 + k }) {
                let Item::Track(t) = pick.item else { continue };
                radio.feedback(pick.item.clone(), nikgob_recs::Outcome::Complete);
                let i = m.position(t);
                let tr = lib.get_track(t).unwrap().unwrap();
                picks.push(json!({
                    "title": tr.title, "artist": tr.artist_credit.display(),
                    "lane": i.map(|i| format!("{:?}", m.song_type_at(i).lane())),
                    "profile": i.and_then(|i| m.profile_at(i)),
                    "in_session": session.tracks().any(|s| s.track == t),
                }));
            }
        }
        let names: Vec<String> = (0..m.profiles.len()).map(|j| format!("{j}: {:?}", m.profiles.list[j].lane)).collect();
        out.insert(
            name.into(),
            json!({ "seed_in_catalog": cat.get(seed).is_some(), "graph_edges": p.graph_size(), "profiles": names,
                    "session_mode": format!("{:?}", m.live(&session).mode), "picks": picks }),
        );
    }
    let v = Value::Object(out);
    println!("{}", serde_json::to_string_pretty(&v).unwrap());
    if let Some(o) = std::env::var_os("NIKGOB_AUTOPLAY_OUT") {
        std::fs::write(o, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    }
}
