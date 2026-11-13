//! The home of a copy of a real library, built as the runtime builds it,
//! every section printed with its counts:
//! `NIKGOB_REAL_LIBRARY=path/to/library.sqlite cargo test -p nikgob-runtime
//! --test real_home -- --ignored --nocapture`. The file itself is never
//! written; the copy lives in a temporary directory, and no player starts.

use nikgob_library::Library;
use nikgob_view::{ContinueCard, MixCard};

fn card(c: &MixCard) -> String {
    let kicker = if c.kicker.is_empty() { String::new() } else { format!("{} / ", c.kicker) };
    format!("{kicker}«{}» · {} · {} covers", c.title, c.subtitle, c.covers.len())
}

#[test]
#[ignore = "needs NIKGOB_REAL_LIBRARY"]
fn the_home_of_a_real_library() {
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
    let p = nikgob_runtime::preview_home(&lib, dir.path()).unwrap();
    let h = &p.home;
    println!(
        "catalog {} tracks, {} sound vectors, {} events, {} likes, history: {}; built in {:?}",
        p.catalog, p.vectors, p.events, p.likes, p.history, p.took
    );
    match &h.hero {
        Some(c) => println!("hero: {} ({} library tracks)", card(c), h.mix.len()),
        None => println!("hero: none"),
    }
    match &h.daylist {
        Some(c) => println!("daylist: {} ({} tracks)", card(c), p.daylist_tracks),
        None => println!("daylist: none"),
    }
    println!("tiles: {}", h.tiles.len());
    for (t, (id, n)) in h.tiles.iter().zip(&p.tile_tracks) {
        assert_eq!(t.id, *id);
        println!("  {} ({n} tracks)", card(t));
    }
    let ready = nikgob_runtime::preview_ready_tiles(&lib).unwrap();
    println!("kinds that qualify today: {}", ready.len());
    for (kind, title, n) in &ready {
        println!("  {}: «{title}» ({n} tracks)", kind.key());
    }
    println!("the next seven days:");
    for (d, tiles) in nikgob_runtime::preview_tile_days(&lib, 7).unwrap().iter().enumerate() {
        let line: Vec<String> = tiles.iter().map(|(k, title, _, n)| format!("{} «{title}» ({n})", k.key())).collect();
        println!("  day {d}: {}", line.join(", "));
    }
    println!("history: {}; shortcuts: {}", h.history, h.shortcuts.len());
    for s in &h.shortcuts {
        match s {
            nikgob_view::Shortcut::Release(r) => println!("  album {} · {}", r.title, r.artist),
            nikgob_view::Shortcut::Playlist(p, c) => println!("  playlist {} · {} covers", p.name, c.len()),
            nikgob_view::Shortcut::Artist(a) => println!("  artist {}", a.name),
        }
    }
    println!("lanes: {}", h.lanes.len());
    for l in &h.lanes {
        let mins: u64 = l.tracks.iter().filter_map(|t| t.duration).map(|d| d.as_secs()).sum::<u64>() / 60;
        println!("  «{}» · {} tracks, {mins} min · {} covers · {}", l.title, l.tracks.len(), l.covers.len(), l.artists.join(", "));
        for t in l.tracks.iter().take(10) {
            println!("      {} — {}", t.artist, t.title);
        }
    }
    println!("listen again: {}", h.listen_again.len());
    for t in &h.listen_again {
        println!("  {} · {}", t.title, t.artist);
    }
    println!("from library: {}", h.from_library.len());
    for a in &h.from_library {
        println!("  {} · {} · {} of {}", a.card.title, a.card.artist, a.heard, a.total);
    }
    println!("worlds: {}", h.worlds.len());
    for w in &h.worlds {
        println!(
            "  {} [{}] {} | {} | heard {} of {} | {} {} | cover {}",
            w.title,
            w.key,
            w.kicker,
            w.facts,
            w.heard,
            w.total,
            w.start.label(),
            w.primary_tip,
            w.cover.is_some()
        );
        for c in &w.cards {
            println!("    {} · {}", c.kicker, c.title);
        }
    }
    println!("moods: {}", h.moods.len());
    for m in &h.moods {
        println!("  {}", card(m));
    }
    println!("style mixes: {}", h.style_mixes.len());
    for m in &h.style_mixes {
        println!("  {}", card(m));
    }
    println!("artist mixes: {}", h.artist_mixes.len());
    for a in &h.artist_mixes {
        println!("  {}", a.name);
    }
    println!(
        "discover: {} cached discoveries, {} cached new releases (the view's rows need the services)",
        p.discover_cached, p.new_releases_cached
    );
    println!("continue: {}", h.continue_listening.len());
    for c in &h.continue_listening {
        match c {
            ContinueCard::Release(r) => println!("  {}", r.title),
            ContinueCard::Stream(s) => println!("  {} (stream)", s.title),
        }
    }
    println!("long unplayed: {}", h.long_unplayed.len());
    for r in &h.long_unplayed {
        println!("  {}", r.title);
    }
    println!("recently added: {}", h.recently_added.len());
    for r in &h.recently_added {
        println!("  {}", r.title);
    }
    // The page kept for the next start (`home.json`) reads back as it was.
    let started = std::time::Instant::now();
    let kept = serde_json::to_vec(h).unwrap();
    let back: nikgob_view::HomeView = serde_json::from_slice(&kept).unwrap();
    println!("the kept page: {} KB, written and read in {:?}", kept.len() / 1024, started.elapsed());
    assert!(back == *h, "the kept page reads back as it was");

    // «Миксы только из скачанного»: the same library, files only.
    let prefs = nikgob_view::Prefs {
        mixes_downloaded_only: true,
        ..nikgob_view::Prefs::default()
    };
    let p = nikgob_runtime::preview_home_with(&lib, dir.path(), &prefs).unwrap();
    let h = &p.home;
    println!("downloaded only, built in {:?}:", p.took);
    match &h.hero {
        Some(c) => {
            let streams = h.mix.iter().filter(|t| !t.has_local).count();
            println!("  hero: {} ({} library tracks, {streams} without a file)", card(c), h.mix.len());
        }
        None => println!("  hero: none"),
    }
    match &h.daylist {
        Some(c) => println!("  daylist: {} ({} tracks)", card(c), p.daylist_tracks),
        None => println!("  daylist: none"),
    }
    for (t, (_, n)) in h.tiles.iter().zip(&p.tile_tracks) {
        println!("  tile {} ({n} tracks)", card(t));
    }
    for m in &h.style_mixes {
        println!("  style {}", card(m));
    }
}
