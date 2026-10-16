//! The runtime end to end, offline: import a file → it is in the library →
//! edit its tags (the file moves) → trash it → undo.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_library::Library;
use nikgob_runtime::{ImportConfig, ImportSetup, NullPlayback, Runtime, RuntimeConfig};
use nikgob_view::{Command, Prefs, TagEdit, ViewState};

fn write_wav(path: &Path, secs: f32) {
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
        let noise = ((seed >> 16) as f32 / 65_536.0 - 0.5) * 0.2;
        let tone = (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.3;
        let s = ((tone + noise) * 20_000.0) as i16;
        w.write_sample(s).unwrap();
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
}

fn wait_for(rt: &Runtime, what: &str, f: impl Fn(&ViewState) -> bool) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(60) {
        if f(&rt.state()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let s = rt.state();
    panic!(
        "timed out waiting for {what}: toasts {:?}, downloads {:?}",
        s.toasts.iter().map(|t| &t.text).collect::<Vec<_>>(),
        s.downloads
    );
}

#[test]
fn import_retag_trash_undo() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cfg = ImportConfig::with_root(root);
    std::fs::create_dir_all(&cfg.inbox_dir).unwrap();
    let wav = root.join("song.wav");
    write_wav(&wav, 3.0);

    let lib = Library::open_in_memory().unwrap();
    let rt = Runtime::start(
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
    );

    rt.send(Command::ImportFiles(vec![wav.clone()]));
    wait_for(&rt, "the import", |s| {
        s.library.track_count == 1 && s.downloads.history.iter().any(|j| j.subtitle.contains("добавлено 1"))
    });
    let track = rt.state().library.tracks[0].id;

    rt.send(Command::OpenTagEditor(track));
    wait_for(&rt, "the editor", |s| s.tag_editor.track == Some(track));
    let mut edit: TagEdit = rt.state().tag_editor.current.clone();
    edit.title = "Новое имя".into();
    edit.artist = "Исполнитель".into();
    edit.album = "Альбом".into();
    edit.year = "2001".into();
    edit.track_no = "2".into();
    rt.send(Command::SaveTags { track, tags: edit });
    wait_for(&rt, "the retag", |s| s.tag_editor.track.is_none());
    let src = lib.active_source(track).unwrap().unwrap();
    let expected = cfg
        .library_dir
        .join("Исполнитель")
        .join("2001 - Альбом")
        .join("02 - Новое имя.opus");
    assert_eq!(Path::new(&src.locator), expected);
    assert!(expected.is_file());
    assert_eq!(lib.get_track(track).unwrap().unwrap().title, "Новое имя");

    rt.send(Command::TrashTrack(track));
    wait_for(&rt, "the trash", |_| !expected.exists());
    assert!(lib.active_source(track).unwrap().is_none());
    assert_eq!(std::fs::read_dir(&cfg.trash_dir).unwrap().count(), 1);
    // Gone from the library at once (no "0/1" album), listed in the Trash.
    wait_for(&rt, "the Trash view", |s| {
        s.library.track_count == 0 && !s.library.releases.iter().any(|r| r.title == "Альбом") && s.library.trash.len() == 1
    });
    assert_eq!(rt.state().library.trash[0].days_left, 30);

    rt.send(Command::UndoTrash(track));
    wait_for(&rt, "the undo", |_| expected.is_file());
    assert!(lib.active_source(track).unwrap().is_some());
    wait_for(&rt, "the restored album", |s| {
        s.library.track_count == 1 && s.library.releases.iter().any(|r| r.title == "Альбом") && s.library.trash.is_empty()
    });

    // For good: the file, its lossless master and every row go.
    assert!(lib.get_master(track).unwrap().is_some(), "a WAV import keeps a master");
    rt.send(Command::DeleteForever(vec![track]));
    wait_for(&rt, "the purge", |s| s.library.track_count == 0 && !s.library.releases.iter().any(|r| r.title == "Альбом"));
    assert!(!expected.exists());
    assert!(lib.get_track(track).unwrap().is_none());
    assert!(lib.get_master(track).unwrap().is_none());
    let masters: Vec<_> = std::fs::read_dir(cfg.masters_dir()).map(|d| d.flatten().collect()).unwrap_or_default();
    assert!(masters.is_empty(), "master files left: {masters:?}");
    assert!(!expected.parent().unwrap().exists(), "the empty album folder goes too");

    // The finished import is in the persistent history.
    assert!(lib.job_log(Some("import"), 10).unwrap().iter().any(|e| e.subtitle.contains("добавлено 1")));

    // With the Trash off, deleting erases at once.
    let wav2 = root.join("other.wav");
    write_wav(&wav2, 2.0);
    rt.send(Command::ImportFiles(vec![wav2]));
    wait_for(&rt, "the second import", |s| s.library.track_count == 1);
    let second = rt.state().library.tracks[0].id;
    let file = std::path::PathBuf::from(lib.active_source(second).unwrap().unwrap().locator);
    let in_trash = std::fs::read_dir(&cfg.trash_dir).unwrap().count();
    rt.send(Command::SetPrefs(Box::new(Prefs { use_trash: false, ..rt.state().prefs.clone() })));
    rt.send(Command::TrashTrack(second));
    wait_for(&rt, "the direct delete", |s| s.library.track_count == 0);
    assert!(!file.exists());
    assert!(lib.get_track(second).unwrap().is_none());
    assert_eq!(std::fs::read_dir(&cfg.trash_dir).unwrap().count(), in_trash);
}

/// A .torrent whose data is already in Downloads/: the client verifies it,
/// the payload is imported with a torrent origin, cleaning the seed moves
/// the lossless master out of Downloads/.
#[test]
fn torrent_download_imports_and_cleans_up() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cfg = ImportConfig::with_root(root);
    let album = cfg.downloads_dir.join("Album");
    std::fs::create_dir_all(&album).unwrap();
    write_wav(&album.join("01 - Song.wav"), 2.0);
    write_wav(&album.join("02 - Other.wav"), 1.5);
    let torrent = tokio::runtime::Runtime::new().unwrap().block_on(async {
        let t = librqbit::create_torrent(
            &album,
            librqbit::CreateTorrentOptions::default(),
            &librqbit::spawn_utils::BlockingSpawner::new(1),
        )
        .await
        .unwrap();
        t.as_bytes().unwrap().to_vec()
    });
    let tfile = root.join("album.torrent");
    std::fs::write(&tfile, torrent).unwrap();

    let lib = Library::open_in_memory().unwrap();
    let rt = Runtime::start(
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
    );
    rt.send(Command::AddTorrentFile(tfile));
    wait_for(&rt, "the torrent import", |s| s.library.track_count == 2);
    let track = rt.state().library.tracks[0].id;
    let src = lib.active_source(track).unwrap().unwrap();
    assert!(matches!(src.origin, nikgob_core::model::SourceOrigin::Torrent { .. }), "{src:?}");
    wait_for(&rt, "seeding", |s| s.downloads.seeding.len() == 1);

    rt.send(Command::CleanSeeding);
    wait_for(&rt, "cleanup", |s| s.downloads.seeding.is_empty() && !album.exists());
    let masters: Vec<_> = std::fs::read_dir(cfg.masters_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".wav"))
        .collect();
    assert_eq!(masters.len(), 2, "{masters:?}: the lossless originals are kept as masters");
    // One line in the history (not one more from the backfill of older
    // torrents), and it is still there after a restart.
    let album_rows = |s: &ViewState| s.downloads.history.iter().filter(|j| j.title == "Album").count();
    assert_eq!(album_rows(&rt.state()), 1);
    assert_eq!(lib.job_log(Some("torrent"), 10).unwrap().len(), 1);
    drop(rt);
    let rt = Runtime::start(
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
    );
    wait_for(&rt, "the history after a restart", |s| album_rows(s) == 1);
}

/// Repeat-all over a queue where nothing has a file used to recurse forever
/// (skip forward wraps around) and overflow the worker's stack.
#[test]
fn queue_without_playable_files_does_not_loop_forever() {
    use nikgob_core::model::{Artist, ArtistCredit, Track, TrackId};

    let lib = Library::open_in_memory().unwrap();
    let artist = lib.upsert_artist(&Artist::new("A")).unwrap();
    let tracks: Vec<TrackId> = (0..3)
        .map(|i| {
            let t = Track::new(format!("t{i}"), ArtistCredit::single("A", Some(artist.id)));
            lib.upsert_track(&t).unwrap().id
        })
        .collect();
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
    rt.send(Command::CycleRepeat); // off -> all
    rt.send(Command::PlayTracks {
        tracks,
        start: 0,
        context: "test".into(),
    });
    wait_for(&rt, "the toast", |s| s.toasts.iter().any(|t| t.text.contains("Нет файла")));
    // The worker is still alive and answers further commands.
    rt.send(Command::CreatePlaylist("p".into()));
    wait_for(&rt, "the playlist", |s| s.library.playlists.iter().any(|p| p.name == "p"));
}

/// Pause/cancel of a folder import used to reach the torrent client with a
/// non-torrent id ("torrent not found"). Imports are not controllable: the
/// commands are refused with a toast and the import runs to the end.
#[test]
fn pausing_a_folder_import_never_reaches_the_torrent_client() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cfg = ImportConfig::with_root(root);
    std::fs::create_dir_all(&cfg.inbox_dir).unwrap();
    let folder = root.join("Album");
    std::fs::create_dir_all(&folder).unwrap();
    for i in 0..3 {
        write_wav(&folder.join(format!("{i}.wav")), 2.0 + i as f32);
    }

    let rt = Runtime::start(
        RuntimeConfig {
            library: Library::open_in_memory().unwrap(),
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
    );

    rt.send(Command::ImportFolder(folder));
    wait_for(&rt, "the import job", |s| {
        !s.downloads.active.is_empty() || !s.downloads.history.is_empty()
    });
    // The state guard must not outlive this block: the worker needs the lock.
    let (id, controllable) = {
        let s = rt.state();
        let job = s.downloads.active.first().or(s.downloads.history.first()).unwrap();
        (job.id, job.controllable)
    };
    assert!(!controllable, "a folder import offers no pause/cancel");

    rt.send(Command::PauseJob(id, true));
    rt.send(Command::CancelJob(id));
    wait_for(&rt, "the refusal", |s| s.toasts.iter().any(|t| t.text.contains("Импорт нельзя")));
    wait_for(&rt, "the import", |s| s.library.track_count == 3 && s.downloads.active.is_empty());
    let s = rt.state();
    assert!(
        s.toasts.iter().all(|t| !t.text.contains("торрент") && !t.text.contains("Торрент")),
        "{:?}",
        s.toasts.iter().map(|t| &t.text).collect::<Vec<_>>()
    );
    assert!(s.downloads.active.iter().all(|j| j.error.is_none()));
}

#[test]
fn moving_masters_dir_transfers_verifies_and_deletes_old_masters_while_keeping_opus() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cfg = ImportConfig::with_root(root);
    let old_masters = cfg.masters_dir();
    std::fs::create_dir_all(&cfg.inbox_dir).unwrap();
    let wav1 = root.join("first.wav");
    write_wav(&wav1, 2.5);

    let lib = Library::open_in_memory().unwrap();
    let prefs_path = root.join("prefs.json");
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
                torrent_state_dir: root.join("torrent-state"),
                torrent_offline: true,
            }),
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );

    rt.send(Command::ImportFiles(vec![wav1]));
    wait_for(&rt, "first import", |s| s.library.track_count == 1);
    let track1 = rt.state().library.tracks[0].id;
    let opus1 = std::path::PathBuf::from(lib.active_source(track1).unwrap().unwrap().locator);
    assert!(opus1.is_file(), "Opus is in the library");
    assert!(old_masters.is_dir(), "initial master in default masters_dir");

    // Move FLAC masters to another folder/drive.
    let new_masters = root.join("hdd_flac_archive");
    rt.send(Command::MoveMastersDir(new_masters.clone()));
    wait_for(&rt, "masters relocation", |s| {
        s.prefs.masters_dir == new_masters
            && s.toasts.iter().any(|t| t.text.contains("Перенесено FLAC"))
    });

    assert!(!old_masters.exists(), "old masters folder cleaned up after verified transfer");
    assert!(opus1.is_file(), "Opus stays on the fast library disk");
    let moved_files: Vec<_> = std::fs::read_dir(&new_masters)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(moved_files.iter().any(|n| n.ends_with(".wav")), "{moved_files:?}");
    assert!(moved_files.iter().any(|n| n.ends_with(".json")), "{moved_files:?}");

    // Subsequent imports store new masters directly into the new folder.
    let wav2 = root.join("second.wav");
    write_wav(&wav2, 3.2);
    rt.send(Command::ImportFiles(vec![wav2]));
    wait_for(&rt, "second import", |s| s.library.track_count == 2);
    let count_wavs = std::fs::read_dir(&new_masters)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".wav"))
        .count();
    assert_eq!(count_wavs, 2, "second master went straight into new_masters");
    assert!(!old_masters.exists());
}


/// Switching a playlist away from its custom picture deletes our copy in
/// `.musiclib/covers`, never the user's original.
#[test]
fn custom_playlist_cover_copy_goes_when_switching_to_auto() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cfg = ImportConfig::with_root(root);
    std::fs::create_dir_all(&cfg.library_dir).unwrap();
    let lib = Library::open_in_memory().unwrap();
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
            streams: Vec::new(),
            library_dir: cfg.library_dir.clone(),
            prefs: Prefs::default(),
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    let p = lib.create_playlist("P").unwrap();
    let mine = root.join("mine.png");
    std::fs::write(&mine, b"not really a png").unwrap();
    rt.send(Command::SetPlaylistCover(p.id, mine.clone()));
    let t = Instant::now();
    let copy = loop {
        if let Some(c) = lib.get_playlist(p.id).unwrap().and_then(|p| p.cover) {
            break cfg.library_dir.join(c);
        }
        assert!(t.elapsed() < Duration::from_secs(30), "the cover was not set");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(copy.is_file());
    rt.send(Command::SetPlaylistCoverMode(p.id, nikgob_view::PlaylistCoverMode::Smart));
    let t = Instant::now();
    while lib.get_playlist(p.id).unwrap().unwrap().cover.is_some() {
        assert!(t.elapsed() < Duration::from_secs(30), "the mode was not set");
        std::thread::sleep(Duration::from_millis(50));
    }
    wait_for(&rt, "the copy to go", |_| !copy.exists());
    assert!(mine.is_file(), "the user's own file stays");
}
#[test]
fn volume_and_queue_come_back_after_a_restart_paused_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cfg = ImportConfig::with_root(root);
    std::fs::create_dir_all(&cfg.inbox_dir).unwrap();
    let wav = root.join("song.wav");
    write_wav(&wav, 3.0);
    let lib = Library::open_in_memory().unwrap();
    let start = || {
        Runtime::start(
            RuntimeConfig {
                library: lib.clone(),
                streams: Vec::new(),
                library_dir: cfg.library_dir.clone(),
                prefs: Prefs::default(),
                prefs_path: Some(root.join("prefs.json")),
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
        )
    };

    let rt = start();
    rt.send(Command::ImportFiles(vec![wav.clone()]));
    wait_for(&rt, "the import", |s| s.library.track_count == 1);
    let track = rt.state().library.tracks[0].id;
    rt.send(Command::PlayTracks { tracks: vec![track], start: 0, context: "Тест".into() });
    wait_for(&rt, "playing", |s| s.player.current.as_ref().is_some_and(|t| t.id == track));
    rt.send(Command::SetVolume(0.3));
    rt.send(Command::CycleRepeat);
    rt.send(Command::Pause);
    rt.send(Command::Seek(Duration::from_millis(1500)));
    wait_for(&rt, "the seek", |s| s.player.position == Duration::from_millis(1500));
    drop(rt);
    assert!(root.join("session.json").is_file());

    let rt = start();
    wait_for(&rt, "the restored track", |s| s.player.current.as_ref().is_some_and(|t| t.id == track));
    let s = rt.state();
    assert!((s.player.volume - 0.3).abs() < 1e-6);
    assert_eq!(s.player.repeat, nikgob_view::Repeat::All);
    assert_eq!(s.player.status, nikgob_view::Status::Paused);
    assert_eq!(s.player.position, Duration::from_millis(1500));
    assert_eq!(s.player.context.as_deref(), Some("Тест"));
    drop(s);
    rt.send(Command::TogglePlay);
    wait_for(&rt, "play from there", |s| {
        s.player.status == nikgob_view::Status::Playing && s.player.position >= Duration::from_millis(1500)
    });
}
