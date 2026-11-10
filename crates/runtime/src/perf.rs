//! How long the recommendations and the home take on a copy of a real
//! library (one with a long history shows what a short one hides).
//!
//! `NIKGOB_PERF_DB=<a copy>/library.sqlite cargo test -p nikgob-runtime
//! --release perf -- --ignored --nocapture`
//!
//! Only ever a copy: the caches of the recommendations are written into it.

use std::time::Instant;

use nikgob_library::Library;

struct Stderr;

impl log::Log for Stderr {
    fn enabled(&self, m: &log::Metadata<'_>) -> bool {
        m.target().starts_with("nikgob")
    }

    fn log(&self, r: &log::Record<'_>) {
        if self.enabled(r.metadata()) && r.args().to_string().starts_with("recs: snapshot") {
            eprintln!("{}", r.args());
        }
    }

    fn flush(&self) {}
}

#[test]
#[ignore = "needs NIKGOB_PERF_DB"]
fn the_snapshot_and_the_home_of_a_real_library() {
    let Some(db) = std::env::var_os("NIKGOB_PERF_DB") else { return };
    let db = std::path::PathBuf::from(db).canonicalize().unwrap();
    if let Some(data) = std::env::var_os("APPDATA").and_then(|d| std::path::PathBuf::from(d).join("nikgob").canonicalize().ok()) {
        assert!(!db.starts_with(&data), "{} is the app's own library: give a copy", db.display());
    }
    let _ = log::set_logger(&Stderr).map(|()| log::set_max_level(log::LevelFilter::Debug));
    let lib = Library::open(&db).unwrap();
    let prefs = nikgob_view::Prefs::default();
    for round in 0..2 {
        let at = Instant::now();
        let events = lib.listening_events().unwrap().len();
        eprintln!("round {round}: {events} events read in {:?}", at.elapsed());
        let at = Instant::now();
        crate::recs::ModelPreview::of_with(&lib, &prefs).unwrap();
        eprintln!("round {round}: the snapshot in {:?}", at.elapsed());
        let at = Instant::now();
        let visit = crate::home_v2::Visit::now(None, &prefs);
        let home = crate::home::build_with(&lib, &visit);
        eprintln!("round {round}: the home in {:?} ({} albums to continue)", at.elapsed(), home.continue_listening.len());
    }
}
