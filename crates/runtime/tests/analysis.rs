//! The background sound analysis end to end: an imported file gets its
//! bliss vector in the library and the progress reaches the view.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_core::model::AnalysisSource;
use nikgob_library::Library;
use nikgob_runtime::{ImportConfig, ImportSetup, NullPlayback, Runtime, RuntimeConfig, StreamSchedule};
use nikgob_view::{AnalysisProgress, Command, Prefs, ViewState};

fn write_wav(path: &Path, secs: f32, freq: f32) {
    let rate = 44_100;
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let mut seed = 1u32;
    for i in 0..(secs * rate as f32) as u32 {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = ((seed >> 16) as f32 / 65_536.0 - 0.5) * 0.1;
        let t = i as f32 / rate as f32;
        let env = (-5.0 * (t % 0.4)).exp();
        let tone = (t * freq * std::f32::consts::TAU).sin() * 0.4 * env;
        let s = ((tone + noise) * 20_000.0) as i16;
        w.write_sample(s).unwrap();
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
}

fn wait_for(rt: &Runtime, what: &str, f: impl Fn(&ViewState) -> bool) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(120) {
        if f(&rt.state()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}: {:?}", rt.state().analysis);
}

#[test]
fn imported_files_are_analysed_in_the_background() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cfg = ImportConfig::with_root(root);
    std::fs::create_dir_all(&cfg.inbox_dir).unwrap();
    let wav = root.join("song.wav");
    write_wav(&wav, 6.0, 330.0);

    let lib = Library::open_in_memory().unwrap();
    let schedule = StreamSchedule {
        analysis_start: Duration::ZERO,
        analysis_idle: Duration::from_millis(200),
        ..StreamSchedule::default()
    };
    let rt = Runtime::start_scheduled(
        RuntimeConfig {
            library: lib.clone(),
            streams: Vec::new(),
            library_dir: cfg.library_dir.clone(),
            prefs: Prefs::default(),
            prefs_path: None,
            settings_path: None,
            import: Some(ImportSetup {
                config: cfg.clone(),
                metadata: None,
                watch_inbox: false,
                torrent_state_dir: root.join("torrent-state"),
                torrent_offline: true,
            }),
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
        schedule,
    );

    rt.send(Command::ImportFiles(vec![wav]));
    wait_for(&rt, "the import", |s| s.library.track_count == 1);
    let track = rt.state().library.tracks[0].id;
    wait_for(&rt, "the analysis", |s| {
        s.analysis
            == AnalysisProgress {
                files_done: 1,
                files_total: 1,
                streams_done: 0,
                streams_total: 0,
            }
    });
    assert!(rt.state().analysis.complete());
    let an = lib.get_analysis(track).unwrap().expect("stored");
    assert_eq!(an.analyzed_with, nikgob_recs::ANALYZED_WITH);
    assert_eq!(an.source, AnalysisSource::File);
    assert_eq!(an.bliss_vector.len(), nikgob_recs::DIM);
    assert!(lib.tracks_needing_analysis(nikgob_recs::ANALYZED_WITH, 10).unwrap().is_empty());
    drop(rt);
}
