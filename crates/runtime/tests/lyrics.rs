//! Cached online lyrics are embedded only with consent, without sidecars or
//! changing the track identity; a manual pick is saved through the worker too.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_core::lyrics::LyricsSource;
use nikgob_core::model::{ArtistCredit, Codec, Source, SourceKind, Track};
use nikgob_import::tags::{TagSet, read_tags, write_tags};
use nikgob_import::{EncodeOptions, Encoder, RopusEncoder};
use nikgob_library::Library;
use nikgob_runtime::{ImportConfig, ImportSetup, NullPlayback, Runtime, RuntimeConfig};
use nikgob_view::{Command, Prefs};

fn wait_for(check: impl Fn() -> bool) {
    let start = Instant::now();
    while !check() {
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "lyrics worker timed out"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn audio_file(path: &Path) {
    let wav = path.with_extension("wav");
    let mut writer = hound::WavWriter::create(
        &wav,
        hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for _ in 0..48_000 {
        writer.write_sample(0i16).unwrap();
    }
    writer.finalize().unwrap();
    RopusEncoder
        .encode_to_opus(&wav, path, &EncodeOptions { bitrate_kbps: 160 })
        .unwrap();
    std::fs::remove_file(wav).unwrap();
}

#[test]
fn setting_and_manual_pick_embed_tags_without_sidecars_or_duplicate_tracks() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = ImportConfig::with_root(dir.path());
    std::fs::create_dir_all(&cfg.library_dir).unwrap();
    let path = cfg.library_dir.join("Song.opus");
    audio_file(&path);
    let lib = Library::open_in_memory().unwrap();
    let track = lib
        .upsert_track(&Track::new("Song", ArtistCredit::single("Artist", None)))
        .unwrap();
    write_tags(
        &path,
        &TagSet {
            track_id: Some(track.id),
            title: Some("Song".into()),
            ..Default::default()
        },
    )
    .unwrap();
    lib.add_source(&Source::new(
        track.id,
        SourceKind::LocalFile,
        path.to_string_lossy(),
        Codec::Opus,
    ))
    .unwrap();
    let online = nikgob_metadata::parse_lrc_lines(
        "[00:01.001]<00:01.001>Hel<00:01.250>lo <00:02.009>world<00:03.012>",
    );
    lib.put_lyrics_text(
        track.id,
        "lyricsplus",
        Some(&serde_json::to_string(&online).unwrap()),
    )
    .unwrap();
    lib.put_lyrics(track.id, Some("[00:01.00]Another text"), None, false)
        .unwrap();
    // A second track to switch to: the playing file is never rewritten.
    let other_path = cfg.library_dir.join("Other.opus");
    audio_file(&other_path);
    let other = lib
        .upsert_track(&Track::new("Other", ArtistCredit::single("Artist", None)))
        .unwrap();
    write_tags(
        &other_path,
        &TagSet {
            track_id: Some(other.id),
            title: Some("Other".into()),
            ..Default::default()
        },
    )
    .unwrap();
    lib.add_source(&Source::new(
        other.id,
        SourceKind::LocalFile,
        other_path.to_string_lossy(),
        Codec::Opus,
    ))
    .unwrap();
    let original = std::fs::read(&path).unwrap();
    let prefs_path = dir.path().join("prefs.json");
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
            streams: Vec::new(),
            library_dir: cfg.library_dir.clone(),
            prefs: Prefs::default(),
            prefs_path: Some(prefs_path.clone()),
            settings_path: None,
            import: Some(ImportSetup {
                config: cfg.clone(),
                metadata: None,
                watch_inbox: false,
                torrent_state_dir: dir.path().join("torrent-state"),
                torrent_offline: true,
                models_dir: None,
            }),
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    rt.send(Command::PlayTracks {
        tracks: vec![track.id],
        start: 0,
        context: "lyrics test".into(),
    });
    wait_for(|| rt.state().lyrics.source == Some(LyricsSource::LyricsPlus));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        original,
        "default-off must not touch the file"
    );

    let mut prefs = rt.state().prefs.clone();
    prefs.save_found_lyrics = true;
    rt.send(Command::SetPrefs(Box::new(prefs)));
    let expected = nikgob_metadata::lrc::serialize_lrc(&online);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        original,
        "the playing file is written only after it stops playing"
    );
    rt.send(Command::PlayTracks {
        tracks: vec![other.id],
        start: 0,
        context: "lyrics test".into(),
    });
    wait_for(|| {
        read_tags(&path).is_ok_and(|(t, _)| t.lyrics.as_deref() == Some(expected.trim_end()))
    });
    let tags = read_tags(&path).unwrap().0;
    assert_eq!(tags.track_id, Some(track.id));
    assert_eq!(tags.title.as_deref(), Some("Song"));
    assert_eq!(
        nikgob_metadata::parse_lrc_lines(tags.lyrics.as_deref().unwrap()),
        online
    );
    assert!(!path.with_extension("lrc").exists());
    let saved: Prefs = serde_json::from_slice(&std::fs::read(&prefs_path).unwrap()).unwrap();
    assert!(saved.save_found_lyrics);

    rt.send(Command::PickLyricsSource(
        track.id,
        Some(LyricsSource::Lrclib),
    ));
    wait_for(|| {
        read_tags(&path).is_ok_and(|(t, _)| t.lyrics.as_deref() == Some("[00:01.000]Another text"))
    });
    assert_eq!(lib.lyrics_pick(track.id).unwrap().as_deref(), Some("lrclib"));
    // A rescan uses the preserved NIKGOB_TRACK_ID and locator, not the mtime.
    let report = nikgob_import::rebuild_from_files(&lib, &cfg).unwrap();
    assert_eq!(report.tracks_created, 0);
    assert_eq!(report.tracks_existing, 2);
    assert_eq!(lib.sources_of_track(track.id).unwrap().len(), 1);
}
