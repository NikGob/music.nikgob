//! «Скачано»: every downloaded track is reachable a page at a time, and the
//! count is the real one, not the page's.

use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_core::model::{ArtistCredit, Codec, Source, SourceKind, Track};
use nikgob_library::Library;
use nikgob_runtime::{NullPlayback, Runtime, RuntimeConfig};
use nikgob_view::{Command, Prefs, ViewState};

fn wait_for(rt: &Runtime, what: &str, f: impl Fn(&ViewState) -> bool) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(30) {
        if f(&rt.state()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

fn shown(s: &ViewState) -> usize {
    s.library.downloaded.iter().map(|d| d.tracks.len()).sum()
}

#[test]
fn every_downloaded_track_is_reachable_a_page_at_a_time() {
    let lib = Library::open_in_memory().unwrap();
    let n = 620;
    for i in 0..n {
        let t = lib.upsert_track(&Track::new(format!("Song {i}"), ArtistCredit::single("Band", None))).unwrap();
        lib.add_source(&Source::new(t.id, SourceKind::LocalFile, format!("Music/{i}.opus"), Codec::Opus)).unwrap();
    }
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib,
            streams: Vec::new(),
            library_dir: std::env::temp_dir(),
            prefs: Prefs::default(),
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    rt.send(Command::LoadDownloaded);
    wait_for(&rt, "the first page", |s| shown(s) > 0);
    // Not held while the runtime publishes.
    let (total, first) = {
        let s = rt.state();
        (s.library.downloaded_total, shown(&s))
    };
    assert_eq!(total, n, "the count is of them all");
    assert_eq!(first, 500);
    rt.send(Command::LoadMoreDownloaded);
    wait_for(&rt, "the next page", |s| shown(s) == n);
    assert_eq!(rt.state().library.downloaded_total, n);
}
