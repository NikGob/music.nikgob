//! The taste profiles at work on a copy of a real library: the playlists
//! the user plays in the background, and «Похоже на» of a few seeds — a
//! soundtrack, a song of each of the two largest sung profiles and a track
//! that suits the background — each with its first fifteen, their profiles
//! and worlds: `NIKGOB_REAL_LIBRARY=path/to/library.sqlite cargo test -p
//! nikgob-runtime --test real_profiles -- --ignored --nocapture`. The file
//! itself is never written.

use std::collections::HashMap;

use nikgob_core::model::TrackId;
use nikgob_library::Library;
use nikgob_recs::{Ended, Item, RadioSeed, RadioState, Session};
use nikgob_runtime::ModelPreview;

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

fn line(lib: &Library, preview: &ModelPreview, id: TrackId) -> String {
    let t = lib.get_track(id).ok().flatten();
    let name = t.map_or_else(|| id.to_string(), |t| format!("{} · {}", t.title, t.artist_credit.display()));
    let world = preview.world(id).and_then(|w| preview.world_title(w)).unwrap_or_default();
    format!("{name} [{}] {{{world}}}", preview.profile_name(lib, id).unwrap_or_default())
}

#[test]
#[ignore = "needs NIKGOB_REAL_LIBRARY"]
fn radios_keep_to_their_profiles() {
    let Some(src) = std::env::var_os("NIKGOB_REAL_LIBRARY") else {
        eprintln!("NIKGOB_REAL_LIBRARY is not set");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let lib = copy_of(&src, dir.path());
    let preview = ModelPreview::of(&lib).unwrap();
    println!("{}", preview.listening());
    println!("played in the background: {:?}", preview.background_playlists(&lib));
    let rows = lib.rec_tracks().unwrap();
    // The seeds: the most played track of each of a few profiles, a
    // soundtrack's and one that suits the background.
    let mut plays: HashMap<TrackId, usize> = HashMap::new();
    for e in lib.listening_events().unwrap() {
        if e.event_type == nikgob_core::model::EventType::Complete {
            *plays.entry(e.track_id).or_default() += 1;
        }
    }
    let mut by_profile: HashMap<String, (usize, TrackId)> = HashMap::new();
    let mut quiet: Option<(usize, TrackId)> = None;
    for r in rows.iter().filter(|r| r.in_library) {
        let n = plays.get(&r.id).copied().unwrap_or(0);
        if let Some(p) = preview.profile_name(&lib, r.id) {
            let e = by_profile.entry(p).or_insert((n, r.id));
            if n > e.0 {
                *e = (n, r.id);
            }
        }
        if preview.suits_background(r.id) && quiet.is_none_or(|q| n > q.0) {
            quiet = Some((n, r.id));
        }
    }
    let mut seeds: Vec<(String, TrackId)> = by_profile.into_iter().map(|(p, (_, t))| (p, t)).collect();
    seeds.sort();
    seeds.extend(quiet.map(|q| ("suits the background".to_owned(), q.1)));
    let mut out_of_profile = 0;
    let mut runs = 0;
    for (what, seed) in seeds {
        println!("\n«Похоже на» {} ({what}):", line(&lib, &preview, seed));
        let mut radio = RadioState::new(RadioSeed::Track(seed));
        let mut session = Session::default();
        let mut ts = nikgob_library::now_ms();
        session.start(seed, ts, nikgob_core::model::EventContext::Radio, None);
        session.end(seed, Ended::Complete);
        let mut worlds = vec![preview.world(seed)];
        let own = preview.profile_name(&lib, seed);
        let mut n = 0;
        for _ in 0..2 {
            radio.attune(session.clone(), None);
            for p in preview.extend(&mut radio, 10) {
                let Item::Track(t) = p.item else { continue };
                n += 1;
                if n <= 15 {
                    println!("  {n:2} {}", line(&lib, &preview, t));
                }
                out_of_profile += usize::from(preview.profile_name(&lib, t) != own);
                worlds.push(preview.world(t));
                ts += 200_000;
                session.start(t, ts, nikgob_core::model::EventContext::Radio, None);
                session.end(t, Ended::Complete);
            }
        }
        runs += worlds.windows(3).filter(|w| w[0].is_some() && w[0] == w[1] && w[1] == w[2]).count();
    }
    println!("\nout of the seed's profile: {out_of_profile}; a world thrice running: {runs}");
    assert_eq!(runs, 0, "a world thrice running");
}

/// The mixes of a world's page for MADE IN ABYSS and the world played most
/// after it, with their tracks.
#[test]
#[ignore = "needs NIKGOB_REAL_LIBRARY"]
fn world_mixes() {
    let Some(src) = std::env::var_os("NIKGOB_REAL_LIBRARY") else {
        eprintln!("NIKGOB_REAL_LIBRARY is not set");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let lib = copy_of(&src, dir.path());
    let preview = ModelPreview::of(&lib).unwrap();
    for title in ["MADE IN ABYSS", "Hollow Knight", "Серега Пират"] {
        let Some(w) = preview.world_named(title) else {
            println!("no world «{title}»");
            continue;
        };
        println!("\n{title}:");
        for (kind, tracks) in preview.world_mixes(w) {
            println!("  {kind:?} ({}):", tracks.len());
            for t in tracks.iter().take(8) {
                println!("    {}", line(&lib, &preview, *t));
            }
        }
    }
}
