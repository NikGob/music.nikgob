//! The lyrics aligner end to end on a real song (ignored: it needs the
//! downloaded models and a song file, which it only reads):
//!
//! `NIKGOB_MODELS=<models dir> NIKGOB_ALIGN_FILE=<song> NIKGOB_ALIGN_LRC=<lrc file>
//! cargo test -p nikgob-runtime --test align_live -- --ignored --nocapture`
//!
//! The song plays (silently), its line-synced text is checked against the
//! recording and shown with word timing made from it.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_core::lyrics::LyricsSource;
use nikgob_core::model::{ArtistCredit, Codec, Source, SourceKind, Track};
use nikgob_library::Library;
use nikgob_runtime::{ImportConfig, ImportSetup, NullPlayback, Runtime, RuntimeConfig};
use nikgob_view::{Command, Prefs};

#[test]
#[ignore = "needs the lyrics models and a song file"]
fn a_line_text_is_checked_and_shown_with_timing_from_the_recording() {
    let models = PathBuf::from(std::env::var("NIKGOB_MODELS").unwrap());
    let file = PathBuf::from(std::env::var("NIKGOB_ALIGN_FILE").unwrap());
    let lrc = std::fs::read_to_string(std::env::var("NIKGOB_ALIGN_LRC").unwrap()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cfg = ImportConfig::with_root(dir.path());
    std::fs::create_dir_all(&cfg.library_dir).unwrap();
    let lib = Library::open_in_memory().unwrap();
    let track = lib.upsert_track(&Track::new("Song", ArtistCredit::single("Artist", None))).unwrap();
    lib.add_source(&Source::new(track.id, SourceKind::LocalFile, file.to_string_lossy(), Codec::Opus)).unwrap();
    lib.put_lyrics(track.id, Some(&lrc), None, false).unwrap();
    for source in ["lyricsplus", "lrcred", "betterlyrics", "genius"] {
        lib.put_lyrics_text(track.id, source, None).unwrap();
    }
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
            streams: Vec::new(),
            library_dir: cfg.library_dir.clone(),
            prefs: Prefs { lyrics_models_auto: false, ..Prefs::default() },
            prefs_path: None,
            settings_path: None,
            import: Some(ImportSetup {
                config: cfg,
                metadata: None,
                watch_inbox: false,
                torrent_state_dir: dir.path().join("torrent-state"),
                torrent_offline: true,
                models_dir: Some(models),
            }),
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    rt.send(Command::PlayTracks { tracks: vec![track.id], start: 0, context: "align test".into() });
    let start = Instant::now();
    while !rt.state().lyrics.made {
        assert!(start.elapsed() < Duration::from_secs(600), "no timing made: {:?}", rt.state().align.job);
        std::thread::sleep(Duration::from_millis(200));
    }
    let v = rt.state().lyrics.clone();
    println!("made in {:?} on {:?}", start.elapsed(), rt.state().align.on_gpu);
    assert_eq!(v.source, Some(LyricsSource::Lrclib));
    let given = nikgob_metadata::parse_lrc_lines(&lrc);
    for (g, m) in given.iter().zip(&v.lines).take(8) {
        println!("{:7.2} → {:7.2}  {}", g.start.as_secs_f32(), m.start.as_secs_f32(), m.text);
    }
    assert!(v.lines.iter().any(|l| !l.words.is_empty()));
    let rows = lib.lyrics_aligned(track.id).unwrap();
    println!("{:?}", rows.iter().map(|r| (&r.source, r.fit, &r.timing, r.offset_ms, r.covered)).collect::<Vec<_>>());
    assert!(rows.iter().any(|r| r.source == "lrclib" && r.body.is_some()));
}
