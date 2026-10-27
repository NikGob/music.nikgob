//! A real export imported into a copy of a real library, the way
//! Settings → «Источники» → «Из файлов выгрузки…» does it, with the real
//! YouTube Music and Deezer (their state in a folder next to the copy):
//!
//! `NIKGOB_IMPORT_DB=<copy>/library.sqlite NIKGOB_TAKEOUT=<export folder>
//! cargo test -p nikgob --test takeout_copy -- --ignored --nocapture`
//!
//! Never point it at the library in use: it writes to the database it is
//! given.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_core::streaming::StreamService;
use nikgob_library::Library;
use nikgob_runtime::{NullPlayback, Runtime, RuntimeConfig};
use nikgob_view::{Command, Prefs};

#[test]
#[ignore = "needs NIKGOB_IMPORT_DB and NIKGOB_TAKEOUT, and the network"]
fn import_a_real_export_into_a_copy() {
    let (Some(db), Some(export)) = (std::env::var_os("NIKGOB_IMPORT_DB"), std::env::var_os("NIKGOB_TAKEOUT")) else {
        return;
    };
    let db = PathBuf::from(db);
    let dir = db.parent().unwrap().to_owned();
    let _ = env_logger::builder().is_test(true).filter_level(log::LevelFilter::Info).try_init();
    let lib = Library::open(&db).unwrap();
    let mut streams: Vec<Arc<dyn StreamService>> = Vec::new();
    streams.push(nikgob_youtube::Youtube::new(nikgob_youtube::YoutubeConfig::new(dir.join("youtube"))).unwrap());
    streams.push(nikgob_deezer::Deezer::new(nikgob_deezer::DeezerConfig::default()).unwrap());
    let empty = dir.join("root");
    std::fs::create_dir_all(&empty).unwrap();
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
            streams,
            library_dir: empty,
            prefs: Prefs::default(),
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    let started = Instant::now();
    rt.send(Command::ImportExportFiles(PathBuf::from(export)));
    let mut last = String::new();
    loop {
        assert!(started.elapsed() < Duration::from_secs(3 * 3600), "the import took too long");
        let s = rt.state();
        if let Some(job) = s.downloads.history.iter().find(|j| j.title.starts_with("Импорт выгрузки")) {
            println!("done in {:?}: {} {:?}", started.elapsed(), job.subtitle, job.error);
            for t in &s.toasts {
                println!("toast: {}", t.text);
            }
            break;
        }
        if let Some(job) = s.downloads.active.iter().find(|j| j.title.starts_with("Импорт выгрузки"))
            && job.subtitle != last
        {
            last = job.subtitle.clone();
            println!("{:>6.0?} {last}", started.elapsed());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    drop(rt);
}
