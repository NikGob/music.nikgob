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

/// The `.wav` files in `dir` and its folders (masters are kept by name in
/// album folders).
fn wavs_in(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(wavs_in(&p));
        } else if p.extension().is_some_and(|x| x == "wav") {
            out.push(p);
        }
    }
    out
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
            prefs: Prefs { language: nikgob_view::LangPref::Ru, ..Prefs::default() },
            prefs_path: None,
            settings_path: None,
            import: Some(ImportSetup {
                config: cfg.clone(),
                metadata: None,
                watch_inbox: false,
                torrent_state_dir: root.join("torrent-state"),
                torrent_offline: true,
                models_dir: None,
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

    // «Источник и качество» of a track that is not playing: its own file.
    rt.send(Command::TrackInfo(Some(track)));
    wait_for(&rt, "the track's info", |s| s.track_info.as_ref().is_some_and(|i| i.track == Some(track)));
    let info = rt.state().track_info.clone().unwrap();
    assert_eq!(info.path.as_deref(), Some(expected.as_path()));
    assert!(info.tags.contains(&("Название".to_owned(), "Новое имя".to_owned())), "{:?}", info.tags);
    assert!(info.sound.is_some() && info.source.is_some());
    assert!(rt.state().info.track.is_none(), "nothing plays: the player's info stays empty");
    rt.send(Command::TrackInfo(None));
    wait_for(&rt, "the info let go", |s| s.track_info.is_none());

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
    // On a blocking thread: the rows go first, then the files.
    let masters_left = || std::fs::read_dir(cfg.masters_dir()).map(|d| d.flatten().count()).unwrap_or(0);
    wait_for(&rt, "the purge", |s| {
        s.library.track_count == 0
            && !s.library.releases.iter().any(|r| r.title == "Альбом")
            && !expected.exists()
            && masters_left() == 0
    });
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
    wait_for(&rt, "the second import", |s| s.library.track_count == 1 && s.library.tracks.len() == 1);
    let second = rt.state().library.tracks[0].id;
    let file = std::path::PathBuf::from(lib.active_source(second).unwrap().unwrap().locator);
    let in_trash = std::fs::read_dir(&cfg.trash_dir).unwrap().count();
    rt.send(Command::SetPrefs(Box::new(Prefs { use_trash: false, ..rt.state().prefs.clone() })));
    rt.send(Command::TrashTrack(second));
    wait_for(&rt, "the direct delete", |s| s.library.track_count == 0 && !file.exists());
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
            prefs: Prefs { language: nikgob_view::LangPref::Ru, ..Prefs::default() },
            prefs_path: None,
            settings_path: None,
            import: Some(ImportSetup {
                config: cfg.clone(),
                metadata: None,
                watch_inbox: false,
                torrent_state_dir: root.join("torrent-state"),
                torrent_offline: true,
                models_dir: None,
            }),
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    rt.send(Command::AddTorrentFile(tfile));
    // The count and the rows of the list are published apart.
    wait_for(&rt, "the torrent import", |s| s.library.track_count == 2 && s.library.tracks.len() == 2);
    let track = rt.state().library.tracks[0].id;
    let src = lib.active_source(track).unwrap().unwrap();
    assert!(matches!(src.origin, nikgob_core::model::SourceOrigin::Torrent { .. }), "{src:?}");
    wait_for(&rt, "seeding", |s| s.downloads.seeding.len() == 1);

    // The cleanup copies and hashes masters on a job of its own: while it
    // waits for the masters folder, the player loop goes on.
    let held = nikgob_import::master_writes();
    rt.send(Command::CleanSeeding);
    wait_for(&rt, "the seed stopped", |s| s.downloads.seeding.is_empty());
    let mut p = rt.state().prefs.clone();
    p.normalization = !p.normalization;
    let want = p.normalization;
    rt.send(Command::SetPrefs(Box::new(p)));
    wait_for(&rt, "a command while the cleanup waits", |s| s.prefs.normalization == want);
    // An import and a move of the masters folder asked for meanwhile wait
    // for the cleanup: the import could read a payload it deletes, the move
    // would miss what it writes into the old folder.
    let more = root.join("more.wav");
    write_wav(&more, 1.0);
    rt.send(Command::ImportFiles(vec![more]));
    let moved_to = root.join("flac-archive");
    rt.send(Command::MoveMastersDir(moved_to.clone()));
    std::thread::sleep(Duration::from_millis(500));
    {
        let s = rt.state();
        assert_eq!(s.library.track_count, 2, "no import while the cleanup runs");
        assert!(s.downloads.active.is_empty(), "{:?}", s.downloads.active);
    }
    assert!(!moved_to.exists(), "no move while the cleanup runs");
    assert!(album.join("01 - Song.wav").is_file(), "nothing goes before the masters folder is free");
    drop(held);
    wait_for(&rt, "cleanup", |s| s.downloads.seeding.is_empty() && !album.exists());
    // The torrent's masters are kept by name in the old folder now: the move
    // is refused (they would stay on the old disk), and the waiting import
    // puts its master in the old folder too.
    wait_for(&rt, "the refused move", |s| s.toasts.iter().any(|t| t.text.contains("Не удалось перенести FLAC")));
    wait_for(&rt, "the import after the move", |s| s.library.track_count == 3 && s.downloads.active.is_empty());
    assert_eq!(rt.state().prefs.masters_dir, cfg.masters_dir(), "the folder stays as it was");
    let (old, new) = (wavs_in(&cfg.masters_dir()), wavs_in(&moved_to));
    assert_eq!(old.len(), 3, "{old:?}: the lossless originals are kept as masters");
    assert!(new.is_empty(), "{new:?}");
    let rows = lib.masters_without_replicas(100).unwrap();
    assert_eq!(rows.len(), 3);
    for m in rows {
        let p = std::path::PathBuf::from(m.file_path.expect("kept by name"));
        assert!(p.is_file(), "{p:?}");
    }
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
            prefs: Prefs { language: nikgob_view::LangPref::Ru, ..Prefs::default() },
            prefs_path: None,
            settings_path: None,
            import: Some(ImportSetup {
                config: cfg.clone(),
                metadata: None,
                watch_inbox: false,
                torrent_state_dir: root.join("torrent-state"),
                torrent_offline: true,
                models_dir: None,
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
            library: lib.clone(),
            streams: Vec::new(),
            library_dir: std::env::temp_dir(),
            prefs: Prefs { language: nikgob_view::LangPref::Ru, ..Prefs::default() },
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
/// non-torrent id ("torrent not found"). An import can be cancelled (by the
/// importer itself), never paused: the pause is refused with a toast and
/// the import runs to the end.
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
            prefs: Prefs { language: nikgob_view::LangPref::Ru, ..Prefs::default() },
            prefs_path: None,
            settings_path: None,
            import: Some(ImportSetup {
                config: cfg.clone(),
                metadata: None,
                watch_inbox: false,
                torrent_state_dir: root.join("torrent-state"),
                torrent_offline: true,
                models_dir: None,
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
    let (id, pausable) = {
        let s = rt.state();
        let job = s.downloads.active.first().or(s.downloads.history.first()).unwrap();
        (job.id, job.pausable)
    };
    assert!(!pausable, "a folder import offers no pause");

    rt.send(Command::PauseJob(id, true));
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
            prefs: Prefs { language: nikgob_view::LangPref::Ru, ..Prefs::default() },
            prefs_path: Some(prefs_path.clone()),
            settings_path: None,
            import: Some(ImportSetup {
                config: cfg.clone(),
                metadata: None,
                watch_inbox: false,
                torrent_state_dir: root.join("torrent-state"),
                torrent_offline: true,
                models_dir: None,
            }),
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );

    rt.send(Command::ImportFiles(vec![wav1]));
    wait_for(&rt, "first import", |s| s.library.track_count == 1 && s.library.tracks.len() == 1);
    let track1 = rt.state().library.tracks[0].id;
    let opus1 = std::path::PathBuf::from(lib.active_source(track1).unwrap().unwrap().locator);
    assert!(opus1.is_file(), "Opus is in the library");
    assert!(old_masters.is_dir(), "initial master in default masters_dir");
    let named = std::path::PathBuf::from(lib.get_master(track1).unwrap().unwrap().file_path.expect("kept by name"));
    assert!(named.starts_with(&old_masters));
    // A master kept the older way (`{hash}.wav` right in the folder).
    let older = old_masters.join("older.part.wav");
    write_wav(&older, 1.0);
    let older_hash = nikgob_import::fsutil::hash_file(&older).unwrap();
    let flat = old_masters.join(format!("{older_hash}.wav"));
    std::fs::rename(&older, &flat).unwrap();

    // A master kept by name in an album folder: the move to another
    // folder/drive is refused, nothing moves, the folder stays as it was.
    let new_masters = root.join("hdd_flac_archive");
    rt.send(Command::MoveMastersDir(new_masters.clone()));
    wait_for(&rt, "the refused move", |s| s.toasts.iter().any(|t| t.text.contains("Не удалось перенести FLAC")));
    assert_eq!(rt.state().prefs.masters_dir, old_masters);
    assert!(flat.is_file() && named.is_file());
    assert!(wavs_in(&new_masters).is_empty());

    // With flat masters alone the move goes: they move, verified.
    lib.delete_master(track1).unwrap();
    rt.send(Command::MoveMastersDir(new_masters.clone()));
    wait_for(&rt, "masters relocation", |s| {
        s.prefs.masters_dir == new_masters
            && s.toasts.iter().any(|t| t.text.contains("Перенесено FLAC"))
    });

    assert!(!flat.exists(), "the flat master moved, verified");
    assert!(new_masters.join(format!("{older_hash}.wav")).is_file());
    assert!(named.is_file(), "a file in an album folder is not touched by the move");
    assert!(opus1.is_file(), "Opus stays on the fast library disk");
    let moved_files: Vec<_> = std::fs::read_dir(&new_masters)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(moved_files.iter().any(|n| n.ends_with(".json")), "{moved_files:?}");

    // Subsequent imports store new masters directly into the new folder.
    let wav2 = root.join("second.wav");
    write_wav(&wav2, 3.2);
    rt.send(Command::ImportFiles(vec![wav2]));
    wait_for(&rt, "second import", |s| s.library.track_count == 2);
    assert_eq!(wavs_in(&new_masters).len(), 2, "second master went straight into new_masters");
    assert!(named.is_file());
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
            prefs: Prefs { language: nikgob_view::LangPref::Ru, ..Prefs::default() },
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
                    models_dir: None,
                }),
            },
            Box::new(NullPlayback),
            Arc::new(|| {}),
        )
    };

    let rt = start();
    rt.send(Command::ImportFiles(vec![wav.clone()]));
    wait_for(&rt, "the import", |s| s.library.track_count == 1 && s.library.tracks.len() == 1);
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
    assert_eq!(s.player.origin, nikgob_view::QueueOrigin::Custom("Тест".into()));
    drop(s);
    rt.send(Command::TogglePlay);
    wait_for(&rt, "play from there", |s| {
        s.player.status == nikgob_view::Status::Playing && s.player.position >= Duration::from_millis(1500)
    });
}

/// The header of Now Playing names what the queue was started from; an
/// album, a playlist or an artist also gives its page to open, after a
/// restart too. A queue of picked songs has none.
#[test]
fn the_queue_names_the_page_it_was_started_from() {
    use nikgob_core::model::{Artist, ArtistCredit, Codec, Release, Source, SourceKind, Track};
    use nikgob_view::Route;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let lib = Library::open_in_memory().unwrap();
    let artist = lib.upsert_artist(&Artist::new("A")).unwrap();
    let credit = ArtistCredit::single("A", Some(artist.id));
    let release = lib.upsert_release(&Release::new("Album", credit.clone())).unwrap();
    let ids: Vec<_> = (0..2)
        .map(|i| {
            let mut t = Track::new(format!("t{i}"), credit.clone());
            t.release_id = Some(release.id);
            t.track_no = i + 1;
            let t = lib.upsert_track(&t).unwrap();
            let file = root.join(format!("t{i}.wav"));
            write_wav(&file, 3.0);
            lib.add_source(&Source::new(t.id, SourceKind::LocalFile, file.display().to_string(), Codec::Wav)).unwrap();
            t.id
        })
        .collect();
    let playlist = lib.create_playlist("P").unwrap();
    lib.playlist_add(playlist.id, ids[1]).unwrap();
    let start = || {
        Runtime::start(
            RuntimeConfig {
                library: lib.clone(),
                streams: Vec::new(),
                library_dir: root.to_owned(),
                prefs: Prefs::default(),
                prefs_path: Some(root.join("prefs.json")),
                settings_path: None,
                import: None,
            },
            Box::new(NullPlayback),
            Arc::new(|| {}),
        )
    };
    let page = |s: &ViewState| s.player.context_page.clone();
    let rt = start();
    rt.send(Command::PlayPlaylist { id: playlist.id, start: None, shuffle: false });
    wait_for(&rt, "the playlist's page", |s| {
        page(s) == Some(Route::Playlist(playlist.id)) && s.player.context_header(nikgob_view::Locale::Ru).0 == "Играет из плейлиста «P»"
    });
    rt.send(Command::PlayArtist { id: artist.id, shuffle: false });
    wait_for(&rt, "the artist's page", |s| page(s) == Some(Route::Artist(artist.id)));
    rt.send(Command::PlayTracks { tracks: ids.clone(), start: 0, context: "Тест".into() });
    wait_for(&rt, "no page", |s| s.player.origin == nikgob_view::QueueOrigin::Custom("Тест".into()) && page(s).is_none());
    rt.send(Command::PlayRelease { id: release.id, start: Some(ids[1]), shuffle: false });
    wait_for(&rt, "the album's page", |s| {
        page(s) == Some(Route::Release(release.id)) && s.player.context_header(nikgob_view::Locale::Ru).0 == "Играет из альбома «Album»"
    });
    rt.send(Command::Pause);
    wait_for(&rt, "paused", |s| s.player.status == nikgob_view::Status::Paused);
    drop(rt);

    let rt = start();
    // The album's page after a restart.
    wait_for(&rt, "the restored queue and its page", |s| {
        s.player.current.as_ref().is_some_and(|t| t.id == ids[1]) && page(s) == Some(Route::Release(release.id))
    });
    // Another queue: the old page goes with the old queue.
    rt.send(Command::PlayTracks { tracks: ids.clone(), start: 0, context: "Тест".into() });
    wait_for(&rt, "no page again", |s| s.player.origin == nikgob_view::QueueOrigin::Custom("Тест".into()) && page(s).is_none());
}

/// A like of a song with a file and a playlist edit publish what changed in
/// the library (the album's heart, the songs' hearts, the playlist cards)
/// without reading it all again; a stream song liked joins the library.
#[test]
fn likes_and_playlist_edits_update_the_library_in_place() {
    use nikgob_core::model::{Artist, ArtistCredit, Codec, LikeTarget, Release, Source, SourceKind, Track};

    let lib = Library::open_in_memory().unwrap();
    let artist = lib.upsert_artist(&Artist::new("A")).unwrap();
    let credit = ArtistCredit::single("A", Some(artist.id));
    let release = lib.upsert_release(&Release::new("Album", credit.clone())).unwrap();
    let tracks: Vec<Track> = (0..2)
        .map(|i| {
            let mut t = Track::new(format!("t{i}"), credit.clone());
            t.release_id = Some(release.id);
            t.track_no = i + 1;
            let t = lib.upsert_track(&t).unwrap();
            lib.add_source(&Source::new(t.id, SourceKind::LocalFile, format!("Music/t{i}.opus"), Codec::Opus))
                .unwrap();
            t
        })
        .collect();
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
            streams: Vec::new(),
            library_dir: std::env::temp_dir(),
            prefs: Prefs { album_like_tracks: true, ..Prefs::default() },
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    rt.send(Command::LoadLibrary);
    wait_for(&rt, "the library", |s| s.library.loaded && s.library.tracks.len() == 2);
    let album = |s: &ViewState| s.library.releases.iter().find(|r| r.id == release.id).map(|r| r.liked);
    let row = |s: &ViewState, i: usize| s.library.tracks.iter().find(|t| t.id == tracks[i].id).map(|t| t.liked);
    assert_eq!(album(&rt.state()), Some(false));

    rt.send(Command::Like(LikeTarget::Track(tracks[0].id), true));
    wait_for(&rt, "the first heart", |s| row(s, 0) == Some(true));
    assert_eq!(album(&rt.state()), Some(false), "one of two");
    rt.send(Command::AddToPlaylist(nikgob_view::LIKED_PLAYLIST, vec![tracks[1].id]));
    wait_for(&rt, "the album's heart", |s| album(s) == Some(true) && row(s, 1) == Some(true));
    rt.send(Command::Like(LikeTarget::Track(tracks[1].id), false));
    wait_for(&rt, "the album's heart gone", |s| album(s) == Some(false) && row(s, 1) == Some(false));

    rt.send(Command::CreatePlaylist("Дорога".into()));
    wait_for(&rt, "the playlist", |s| s.library.playlists.iter().any(|p| p.name == "Дорога"));
    let id = rt.state().library.playlists.iter().find(|p| p.name == "Дорога").unwrap().id;
    // Never a track twice: what it has is told, the rest added.
    let toast = |text: &'static str| move |s: &ViewState| s.toasts.iter().any(|t| t.text == text);
    rt.send(Command::AddToPlaylist(id, vec![tracks[0].id]));
    wait_for(&rt, "the first added", toast("Добавлено в «Дорога»"));
    rt.send(Command::AddToPlaylist(id, vec![tracks[0].id]));
    wait_for(&rt, "the one it has", toast("Уже есть в «Дорога»"));
    rt.send(Command::AddToPlaylist(id, vec![tracks[0].id, tracks[1].id]));
    wait_for(&rt, "the other added", toast("Добавлено 1, 1 уже был в «Дорога»"));
    assert_eq!(lib.playlist_len(id).unwrap(), 2);
    rt.send(Command::AddToPlaylist(nikgob_view::LIKED_PLAYLIST, vec![tracks[0].id]));
    wait_for(&rt, "a like it has", toast("Уже есть в «Любимые»"));
    rt.send(Command::RenamePlaylist(id, "Путь".into()));
    wait_for(&rt, "the new name", |s| s.library.playlists.iter().any(|p| p.id == id && p.name == "Путь"));
    rt.send(Command::DeletePlaylist(id));
    wait_for(&rt, "the playlist gone", |s| !s.library.playlists.iter().any(|p| p.id == id));

    // A song without a file joins the library with its like.
    let stream = lib.upsert_track(&Track::new("stream", credit)).unwrap();
    lib.add_source(&Source::new(stream.id, SourceKind::Spotify, "spotify:track:x", Codec::Vorbis))
        .unwrap();
    assert_eq!(rt.state().library.track_count, 2);
    rt.send(Command::Like(LikeTarget::Track(stream.id), true));
    wait_for(&rt, "the stream song in", |s| s.library.track_count == 3);
}

/// By default a like of an album saves it alone: its tracks keep their
/// likes, it sits on the shelf of liked albums, and an unlike takes only it
/// away. A like of a single's song saves the single only when asked.
#[test]
fn an_album_like_saves_the_album_and_leaves_its_tracks() {
    use nikgob_core::model::{Artist, ArtistCredit, Codec, LikeTarget, Release, ReleaseType, Source, SourceKind, Track};

    let lib = Library::open_in_memory().unwrap();
    let artist = lib.upsert_artist(&Artist::new("A")).unwrap();
    let credit = ArtistCredit::single("A", Some(artist.id));
    let release = lib.upsert_release(&Release::new("Album", credit.clone())).unwrap();
    let mut single = Release::new("Single", credit.clone());
    single.release_type = ReleaseType::Single;
    let single = lib.upsert_release(&single).unwrap();
    let add = |title: &str, release: &Release, no: u32| {
        let mut t = Track::new(title, credit.clone());
        t.release_id = Some(release.id);
        t.track_no = no;
        let t = lib.upsert_track(&t).unwrap();
        lib.add_source(&Source::new(t.id, SourceKind::LocalFile, format!("Music/{title}.opus"), Codec::Opus)).unwrap();
        t
    };
    let tracks = [add("t0", &release, 1), add("t1", &release, 2)];
    let song = add("s", &single, 1);
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
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
    rt.send(Command::LoadLibrary);
    wait_for(&rt, "the library", |s| s.library.loaded && s.library.tracks.len() == 3);
    let album = |s: &ViewState, id| s.library.releases.iter().find(|r| r.id == id).map(|r| r.liked);
    let shelf = |s: &ViewState| s.library.liked_albums.iter().map(|(_, r)| r.id).collect::<Vec<_>>();

    rt.send(Command::Like(LikeTarget::Track(tracks[0].id), true));
    rt.send(Command::Like(LikeTarget::Track(tracks[1].id), true));
    wait_for(&rt, "both tracks liked", |s| s.library.tracks.iter().filter(|t| t.liked).count() == 2);
    assert_eq!(album(&rt.state(), release.id), Some(false), "liked tracks do not like the album");

    rt.send(Command::Like(LikeTarget::Release(release.id), true));
    wait_for(&rt, "the album saved", |s| album(s, release.id) == Some(true) && shelf(s) == [release.id]);
    rt.send(Command::Like(LikeTarget::Release(release.id), false));
    wait_for(&rt, "the album unsaved", |s| album(s, release.id) == Some(false) && shelf(s).is_empty());
    assert_eq!(lib.liked_count().unwrap(), 2, "the tracks keep their own likes");

    rt.send(Command::Like(LikeTarget::Track(song.id), true));
    wait_for(&rt, "the song liked", |s| s.library.tracks.iter().any(|t| t.id == song.id && t.liked));
    assert_eq!(album(&rt.state(), single.id), Some(false));
    rt.send(Command::SetPrefs(Box::new(Prefs { single_like_saves: true, ..rt.state().prefs.clone() })));
    rt.send(Command::Like(LikeTarget::Track(song.id), false));
    rt.send(Command::Like(LikeTarget::Track(song.id), true));
    wait_for(&rt, "the single saved", |s| album(s, single.id) == Some(true) && shelf(s) == [single.id]);
    rt.send(Command::Like(LikeTarget::Track(song.id), false));
    wait_for(&rt, "the single unsaved", |s| album(s, single.id) == Some(false) && shelf(s).is_empty());

    // The old way: the album likes its tracks, its heart follows them.
    rt.send(Command::SetPrefs(Box::new(Prefs { album_like_tracks: true, ..rt.state().prefs.clone() })));
    rt.send(Command::Like(LikeTarget::Release(release.id), false));
    wait_for(&rt, "its tracks unliked", |s| s.library.tracks.iter().all(|t| !t.liked));
    rt.send(Command::Like(LikeTarget::Release(release.id), true));
    wait_for(&rt, "its tracks liked", |s| album(s, release.id) == Some(true) && s.library.tracks.iter().filter(|t| t.liked).count() == 2);
}

/// Under «Только скачанное» an album that is only partly downloaded still
/// plays whole, its songs that only stream too (each looked up when it
/// comes up): played, from one of its songs, after the queue, and the
/// library's own switch alike. Its artist's songs and a playlist keep to
/// the songs with a file; a song that only streams plays there when it is
/// the one picked.
#[test]
fn only_downloaded_still_plays_an_album_whole() {
    use nikgob_core::model::{Artist, ArtistCredit, Codec, Release, Source, SourceKind, Track};

    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open_in_memory().unwrap();
    let artist = lib.upsert_artist(&Artist::new("A")).unwrap();
    let credit = ArtistCredit::single("A", Some(artist.id));
    let release = lib.upsert_release(&Release::new("Album", credit.clone())).unwrap();
    let tracks: Vec<Track> = (0..3)
        .map(|i| {
            let mut t = Track::new(format!("t{i}"), credit.clone());
            t.release_id = Some(release.id);
            t.track_no = i + 1;
            let t = lib.upsert_track(&t).unwrap();
            if i == 1 {
                lib.add_source(&Source::new(t.id, SourceKind::Spotify, "spotify:track:x", Codec::Vorbis)).unwrap();
            } else {
                let file = dir.path().join(format!("t{i}.wav"));
                write_wav(&file, 2.0);
                lib.add_source(&Source::new(t.id, SourceKind::LocalFile, file.display().to_string(), Codec::Wav))
                    .unwrap();
            }
            t
        })
        .collect();
    let ids: Vec<_> = tracks.iter().map(|t| t.id).collect();
    let playlist = lib.create_playlist("P").unwrap();
    for t in [ids[1], ids[2], ids[0]] {
        lib.playlist_add(playlist.id, t).unwrap();
    }
    let prefs = Prefs { mixes_downloaded_only: true, ..Prefs::default() };
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
            streams: Vec::new(),
            library_dir: dir.path().to_owned(),
            prefs,
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    let queued = |s: &ViewState| {
        let mut ids: Vec<_> = s.queue.current.iter().map(|t| t.id).chain(s.queue.next.iter().map(|e| e.track.id)).collect();
        ids.extend(s.queue.prev.iter().map(|e| e.track.id));
        ids
    };
    // Each step plays another song first, so a state of the step before
    // never passes for it.
    let playing = |s: &ViewState, t: usize, n: usize, stream: bool| {
        let q = queued(s);
        s.queue.current.as_ref().is_some_and(|c| c.id == ids[t]) && q.len() == n && q.contains(&ids[1]) == stream
    };
    // An artist plays as a mix and a playlist as the user's own, both
    // shuffled by default: from either song with a file.
    let downloaded_two = |s: &ViewState| {
        let q = queued(s);
        s.queue.current.as_ref().is_some_and(|c| c.id == ids[0] || c.id == ids[2]) && q.len() == 2 && !q.contains(&ids[1])
    };
    rt.send(Command::PlayRelease { id: release.id, start: None, shuffle: false });
    wait_for(&rt, "the whole album", |s| playing(s, 0, 3, true));
    rt.send(Command::PlayArtist { id: artist.id, shuffle: false });
    wait_for(&rt, "the artist's downloaded songs", downloaded_two);
    rt.send(Command::PlayRelease { id: release.id, start: Some(ids[2]), shuffle: false });
    wait_for(&rt, "the whole album from a song", |s| playing(s, 2, 3, true));
    rt.send(Command::PlayPlaylist { id: playlist.id, start: None, shuffle: false });
    wait_for(&rt, "the playlist's downloaded songs", downloaded_two);
    // Picked in the playlist, the song that only streams joins (it cannot
    // play here: no service, so the next one plays).
    rt.send(Command::PlayPlaylist { id: playlist.id, start: Some(ids[1]), shuffle: false });
    wait_for(&rt, "the picked stream song", |s| queued(s).len() == 3);
    rt.send(Command::PlayArtist { id: artist.id, shuffle: false });
    wait_for(&rt, "the artist's downloaded songs again", downloaded_two);
    rt.send(Command::EnqueueRelease(release.id));
    wait_for(&rt, "the whole album after them", |s| {
        let q = queued(s);
        q.len() == 5 && q.contains(&ids[1])
    });
    // The library's switch instead of the home's: the same.
    let prefs = Prefs { library_only_downloaded: true, mixes_downloaded_only: false, ..rt.state().prefs.clone() };
    rt.send(Command::SetPrefs(Box::new(prefs)));
    wait_for(&rt, "the library's switch", |s| s.prefs.library_only_downloaded && !s.prefs.mixes_downloaded_only);
    rt.send(Command::PlayRelease { id: release.id, start: Some(ids[0]), shuffle: false });
    wait_for(&rt, "the whole album again", |s| playing(s, 0, 3, true));
}

/// An album only one song of which is the user's (a file; its
/// instrumental only streams from the album's tracklist): «Слушать» and
/// «Перемешать» play that song, «Слушать весь альбом» both, «Мои треки»
/// the song again, a pick of the instrumental the whole album from it;
/// added whole, the album plays all of it.
#[test]
fn an_album_of_one_song_of_the_users_plays_that_song() {
    use nikgob_core::model::{Artist, ArtistCredit, AlbumMode, Codec, Release, Source, SourceKind, Track};

    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open_in_memory().unwrap();
    let artist = lib.upsert_artist(&Artist::new("A")).unwrap();
    let credit = ArtistCredit::single("A", Some(artist.id));
    let release = lib.upsert_release(&Release::new("Song", credit.clone())).unwrap();
    let mut song = Track::new("Song", credit.clone());
    song.release_id = Some(release.id);
    song.track_no = 1;
    let song = lib.upsert_track(&song).unwrap();
    let file = dir.path().join("song.wav");
    write_wav(&file, 2.0);
    lib.add_source(&Source::new(song.id, SourceKind::LocalFile, file.display().to_string(), Codec::Wav)).unwrap();
    let mut inst = Track::new("Song (Instrumental)", credit.clone());
    inst.release_id = Some(release.id);
    inst.track_no = 2;
    let inst = lib.upsert_track(&inst).unwrap();
    lib.add_source(&Source::new(inst.id, SourceKind::Spotify, "spotify:track:inst", Codec::Vorbis)).unwrap();
    assert!(lib.get_release_row(release.id).unwrap().unwrap().progress().partial());
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
            streams: Vec::new(),
            library_dir: dir.path().to_owned(),
            prefs: Prefs { language: nikgob_view::LangPref::Ru, album_mode: AlbumMode::Auto, ..Prefs::default() },
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    let queued = |s: &ViewState| {
        let mut ids: Vec<_> = s.queue.current.iter().map(|t| t.id).chain(s.queue.next.iter().map(|e| e.track.id)).collect();
        ids.extend(s.queue.prev.iter().map(|e| e.track.id));
        ids
    };
    // Each step changes the queue, so a state of the step before never
    // passes for it. The whole album leaves its instrumental out until
    // the album's switch says it plays.
    rt.send(Command::PlayReleaseWhole { id: release.id, start: None, shuffle: false });
    wait_for(&rt, "the whole album once, its instrumental apart", |s| queued(s) == [song.id]);
    rt.send(Command::AlbumVersions { id: release.id, on: true });
    rt.send(Command::PlayReleaseWhole { id: release.id, start: None, shuffle: false });
    wait_for(&rt, "the whole album with its instrumental", |s| queued(s).len() == 2);
    rt.send(Command::PlayRelease { id: release.id, start: None, shuffle: true });
    wait_for(&rt, "the user's song, shuffled", |s| queued(s) == [song.id]);
    // Without a service the instrumental cannot play: that it was tried
    // says the whole album started from it.
    rt.send(Command::PlayRelease { id: release.id, start: Some(inst.id), shuffle: false });
    wait_for(&rt, "the whole album from the instrumental", |s| s.toasts.iter().any(|t| t.text.contains("не подключён")));
    rt.send(Command::PlayReleaseWhole { id: release.id, start: None, shuffle: false });
    wait_for(&rt, "the whole album once more", |s| queued(s).len() == 2);
    rt.send(Command::PlayReleaseMine { id: release.id, start: None, shuffle: false });
    wait_for(&rt, "«Мои треки»", |s| queued(s) == [song.id]);
    rt.send(Command::AddWholeAlbum { id: release.id, on: true });
    rt.send(Command::PlayRelease { id: release.id, start: None, shuffle: false });
    wait_for(&rt, "the album added whole, its versions on", |s| queued(s).len() == 2);
    rt.send(Command::AlbumVersions { id: release.id, on: false });
    rt.send(Command::PlayRelease { id: release.id, start: None, shuffle: false });
    wait_for(&rt, "the album added whole, its versions off", |s| queued(s) == [song.id]);
}

/// «В библиотеку» of a stream song nobody liked: a library track, no like,
/// nothing to download; «Убрать из библиотеки» takes it out, «Отменить»
/// puts it back.
#[test]
fn a_stream_song_goes_into_the_library_without_a_like() {
    use nikgob_core::model::{Artist, ArtistCredit, Codec, Source, SourceKind, Track};

    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open_in_memory().unwrap();
    let artist = lib.upsert_artist(&Artist::new("A")).unwrap();
    let t = lib.upsert_track(&Track::new("Heard", ArtistCredit::single("A", Some(artist.id)))).unwrap();
    lib.add_source(&Source::new(t.id, SourceKind::Spotify, "spotify:track:heard", Codec::Vorbis)).unwrap();
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
            streams: Vec::new(),
            library_dir: dir.path().to_owned(),
            prefs: Prefs::default(),
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    wait_for(&rt, "the library", |s| s.library.loaded && s.library.track_count == 0);
    rt.send(Command::TrackInLibrary { track: t.id, on: true });
    wait_for(&rt, "the song in the library", |s| {
        s.library.track_count == 1 && s.toasts.iter().any(|x| x.text == "Трек в библиотеке")
    });
    let undo = rt.state().toasts.iter().find(|x| x.text == "Трек в библиотеке").and_then(|x| x.action.clone()).map(|a| a.1);
    assert_eq!(undo, Some(Command::TrackLibraryRestore { track: t.id, put: false, choice: None }));
    assert!(rt.state().downloads.wishlist.items.is_empty(), "nothing downloads");
    rt.send(Command::TrackInLibrary { track: t.id, on: false });
    wait_for(&rt, "the song out of the library", |s| s.library.track_count == 0);
}

/// A single of a song and its instrumental, saved along with the like of
/// the song: only the song is the user's (the like of the single stands
/// for it, not for the instrumental), the single still plays that song
/// alone. The song left out of it by hand too: nothing of the single is
/// the user's, it stays a single to choose from, «Слушать» plays nothing
/// and «Скачать недостающие» asks for nothing; «Добавить весь альбом»
/// takes the song back in with the rest, and «Отменить» leaves it out
/// again.
#[test]
fn a_single_saved_with_its_song_or_left_out_never_turns_whole() {
    use nikgob_core::model::{AlbumMode, Artist, ArtistCredit, Codec, LikeTarget, Release, ReleaseType, Source, SourceKind, Track};

    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open_in_memory().unwrap();
    let artist = lib.upsert_artist(&Artist::new("A")).unwrap();
    let credit = ArtistCredit::single("A", Some(artist.id));
    let mut single = Release::new("Song", credit.clone());
    single.release_type = ReleaseType::Single;
    let release = lib.upsert_release(&single).unwrap();
    let mut song = Track::new("Song", credit.clone());
    song.release_id = Some(release.id);
    song.track_no = 1;
    let song = lib.upsert_track(&song).unwrap();
    let file = dir.path().join("song.wav");
    write_wav(&file, 2.0);
    lib.add_source(&Source::new(song.id, SourceKind::LocalFile, file.display().to_string(), Codec::Wav)).unwrap();
    let mut inst = Track::new("Song (Instrumental)", credit.clone());
    inst.release_id = Some(release.id);
    inst.track_no = 2;
    let inst = lib.upsert_track(&inst).unwrap();
    lib.add_source(&Source::new(inst.id, SourceKind::Spotify, "spotify:track:inst", Codec::Vorbis)).unwrap();
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
            streams: Vec::new(),
            library_dir: dir.path().to_owned(),
            prefs: Prefs { single_like_saves: true, album_mode: AlbumMode::Auto, ..Prefs::default() },
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    let queued = |s: &ViewState| -> Vec<_> { s.queue.current.iter().map(|t| t.id).chain(s.queue.next.iter().map(|e| e.track.id)).collect() };
    rt.send(Command::Like(LikeTarget::Track(song.id), true));
    rt.send(Command::LoadRelease(release.id));
    wait_for(&rt, "the single saved with its song", |s| {
        s.release.as_ref().is_some_and(|r| r.card.id == release.id && r.liked && r.partial && r.card.mine == Some(1))
    });
    // Its instrumental plays with it only by the album's switch.
    rt.send(Command::AlbumVersions { id: release.id, on: true });
    rt.send(Command::PlayReleaseWhole { id: release.id, start: None, shuffle: false });
    wait_for(&rt, "the whole single once", |s| queued(s).len() == 2);
    rt.send(Command::PlayRelease { id: release.id, start: None, shuffle: false });
    wait_for(&rt, "the song alone", |s| queued(s) == [song.id]);

    // The song left out of the single too.
    rt.send(Command::AlbumTrackMine { release: release.id, track: song.id, mine: false });
    wait_for(&rt, "none of the single the user's", |s| {
        s.release.as_ref().is_some_and(|r| r.card.id == release.id && r.partial && r.card.mine == Some(0))
    });
    // The song on its own (it has a file): a queue of another list.
    rt.send(Command::PlayTracks { tracks: vec![song.id], start: 0, context: "q".into() });
    wait_for(&rt, "the song on its own", |s| queued(s) == [song.id] && s.player.origin == nikgob_view::QueueOrigin::Custom("q".into()));
    rt.send(Command::PlayRelease { id: release.id, start: None, shuffle: false });
    rt.send(Command::DownloadRelease(release.id));
    // Done in order: once the page loads again, both were handled.
    rt.send(Command::AddWholeAlbum { id: release.id, on: true });
    wait_for(&rt, "the single added whole", |s| s.release.as_ref().is_some_and(|r| r.card.id == release.id && !r.partial));
    assert_eq!(queued(&rt.state()), [song.id], "an album of none of the user's songs plays nothing");
    assert_eq!(rt.state().player.origin, nikgob_view::QueueOrigin::Custom("q".into()));
    assert!(rt.state().downloads.wishlist.items.is_empty(), "nor downloads anything");
    rt.send(Command::PlayRelease { id: release.id, start: None, shuffle: false });
    wait_for(&rt, "the whole single, the song left out before too", |s| queued(s).len() == 2);
    let undo = rt.state().toasts.iter().rev().find_map(|t| t.action.as_ref().map(|a| a.1.clone()));
    let Some(undo @ Command::UndoWholeAlbum { .. }) = undo else { panic!("«Отменить» of the whole single: {undo:?}") };
    rt.send(undo);
    wait_for(&rt, "the song left out again", |s| {
        s.release.as_ref().is_some_and(|r| r.card.id == release.id && r.partial && r.card.mine == Some(0))
    });
}

/// «Сыграть следующим» of an album while a track plays: the whole album in
/// its order right after the current track, then the rest of the queue;
/// under «Только скачанное» too, as playing it.
#[test]
fn an_album_played_next_goes_right_after_the_current_track() {
    use nikgob_core::model::{Artist, ArtistCredit, Codec, Release, Source, SourceKind, Track};

    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open_in_memory().unwrap();
    let artist = lib.upsert_artist(&Artist::new("A")).unwrap();
    let credit = ArtistCredit::single("A", Some(artist.id));
    let add = |title: &str, release: Option<&Release>, no: u32, local: bool| {
        let mut t = Track::new(title, credit.clone());
        t.release_id = release.map(|r| r.id);
        t.track_no = no;
        let t = lib.upsert_track(&t).unwrap();
        if local {
            let file = dir.path().join(format!("{title}.wav"));
            write_wav(&file, 2.0);
            lib.add_source(&Source::new(t.id, SourceKind::LocalFile, file.display().to_string(), Codec::Wav)).unwrap();
        } else {
            lib.add_source(&Source::new(t.id, SourceKind::Spotify, format!("spotify:track:{title}"), Codec::Vorbis)).unwrap();
        }
        t
    };
    let single = lib.upsert_release(&Release::new("Single", credit.clone())).unwrap();
    let queue: Vec<Track> = (0..3).map(|i| add(&format!("q{i}"), Some(&single), i + 1, true)).collect();
    let album = lib.upsert_release(&Release::new("Album", credit.clone())).unwrap();
    // Added out of order: the album plays by its track numbers.
    let a3 = add("a3", Some(&album), 3, true);
    let a1 = add("a1", Some(&album), 1, true);
    let a2 = add("a2", Some(&album), 2, false);
    // The song that only streams is the user's too (liked): the album is
    // all theirs, not a few songs of it.
    lib.like(a2.id).unwrap();
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
            streams: Vec::new(),
            library_dir: dir.path().to_owned(),
            prefs: Prefs::default(),
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    let order = |s: &ViewState| -> Vec<_> { s.queue.current.iter().map(|t| t.id).chain(s.queue.next.iter().map(|e| e.track.id)).collect() };
    let ids: Vec<_> = queue.iter().map(|t| t.id).collect();
    rt.send(Command::PlayTracks { tracks: ids.clone(), start: 0, context: "q".into() });
    wait_for(&rt, "the queue", |s| order(s) == ids);
    rt.send(Command::PlayReleaseNext(album.id));
    let want = vec![ids[0], a1.id, a2.id, a3.id, ids[1], ids[2]];
    wait_for(&rt, "the album after the current track", |s| order(s) == want);
    let loc = rt.state().prefs.resolved_locale();
    assert!(rt.state().toasts.iter().any(|t| t.text == nikgob_core::i18n::PlayerStrings::next_up_quoted_in(loc, "Album")));
    // Only the downloaded: the album still goes in whole.
    rt.send(Command::SetPref(nikgob_view::PrefPatch::MixesDownloadedOnly(true)));
    rt.send(Command::PlayTracks { tracks: ids.clone(), start: 0, context: "q".into() });
    wait_for(&rt, "the queue again", |s| order(s) == ids && s.prefs.mixes_downloaded_only);
    rt.send(Command::PlayReleaseNext(album.id));
    let want = vec![ids[0], a1.id, a2.id, a3.id, ids[1], ids[2]];
    wait_for(&rt, "the whole album after the current track", |s| order(s) == want);
}


/// A preset sent as a patch lands on the prefs as the runtime has them:
/// the user's own values are kept in the saved file first, and «Свои»
/// brings them back.
#[test]
fn a_preset_and_custom_come_through_the_runtime() {
    use nikgob_view::{PrefPatch, presets::Area};

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("prefs.json");
    let rt = Runtime::start(
        RuntimeConfig {
            library: Library::open_in_memory().unwrap(),
            streams: Vec::new(),
            library_dir: dir.path().to_owned(),
            prefs: Prefs { fps_limit: 90, ..Prefs::default() },
            prefs_path: Some(file.clone()),
            settings_path: None,
            import: None,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    rt.send(Command::SetPref(PrefPatch::Preset(Area::Load, 0)));
    // The file is written right after the state is published.
    let kept = |file: &Path| {
        let saved = std::fs::read(file).ok().and_then(|b| serde_json::from_slice::<Prefs>(&b).ok());
        saved.and_then(|p| Area::Load.custom(&p)).map(|p| p.fps_limit) == Some(90)
    };
    wait_for(&rt, "the quiet preset, saved", |s| Area::Load.current(&s.prefs) == Some(0) && kept(&file));
    rt.send(Command::SetPref(PrefPatch::PresetCustom(Area::Load)));
    wait_for(&rt, "the user's own values back", |s| s.prefs.fps_limit == 90 && Area::Load.current(&s.prefs).is_none());
    rt.send(Command::SetPref(PrefPatch::SettingsDeep(true)));
    wait_for(&rt, "the detailed settings", |s| s.prefs.settings_deep);
}

/// «В плейлист…» of a library album's card: its tracks by the album's id,
/// in its track order, each once; into «Любимые» each is liked.
#[test]
fn a_library_album_goes_to_a_playlist_by_its_id() {
    use nikgob_core::model::{Artist, ArtistCredit, Release, Track};

    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open_in_memory().unwrap();
    let artist = lib.upsert_artist(&Artist::new("A")).unwrap();
    let credit = ArtistCredit::single("A", Some(artist.id));
    let album = lib.upsert_release(&Release::new("Album", credit.clone())).unwrap();
    // Added out of order: the playlist gets them by their track numbers.
    let add = |title: &str, no: u32| {
        let mut t = Track::new(title, credit.clone());
        t.release_id = Some(album.id);
        t.track_no = no;
        lib.upsert_track(&t).unwrap().id
    };
    let a2 = add("a2", 2);
    let a1 = add("a1", 1);
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
            streams: Vec::new(),
            library_dir: dir.path().to_owned(),
            prefs: Prefs::default(),
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    rt.send(Command::CreatePlaylist("Mix".into()));
    wait_for(&rt, "the playlist", |s| s.library.playlists.iter().any(|p| p.name == "Mix"));
    let mix = rt.state().library.playlists.iter().find(|p| p.name == "Mix").unwrap().id;
    let entries = |lib: &Library| lib.playlist_entries(mix, 0, 100).unwrap().iter().map(|e| e.track.id).collect::<Vec<_>>();
    rt.send(Command::AddAlbumToPlaylist(mix, album.id));
    wait_for(&rt, "the album in the playlist", |_| entries(&lib) == [a1, a2]);
    // Again: nothing twice.
    rt.send(Command::AddAlbumToPlaylist(mix, album.id));
    wait_for(&rt, "the second time", |s| s.toasts.iter().any(|t| t.text == "Уже есть в «Mix»"));
    assert_eq!(entries(&lib), [a1, a2]);
    rt.send(Command::AddAlbumToPlaylist(nikgob_view::LIKED_PLAYLIST, album.id));
    wait_for(&rt, "the album liked", |_| lib.is_liked(a1).unwrap() && lib.is_liked(a2).unwrap());
}


/// A service album's card: «Сыграть следующим», «В конец очереди» and «В
/// плейлист…» by the album's id fetch its tracks as playing it does and
/// put them where the library album's would go.
#[test]
fn a_service_album_plays_next_queues_and_goes_to_a_playlist_by_its_id() {
    use nikgob_core::model::{Artist, ArtistCredit, Codec, Release, Source, SourceKind, Track};
    use nikgob_core::streaming::{FakeService, RemoteId, Service, StreamService};

    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open_in_memory().unwrap();
    let artist = lib.upsert_artist(&Artist::new("A")).unwrap();
    let credit = ArtistCredit::single("A", Some(artist.id));
    let single = lib.upsert_release(&Release::new("Single", credit.clone())).unwrap();
    let ids: Vec<_> = (0..3u32)
        .map(|i| {
            let mut t = Track::new(&format!("q{i}"), credit.clone());
            t.release_id = Some(single.id);
            t.track_no = i + 1;
            let t = lib.upsert_track(&t).unwrap();
            let file = dir.path().join(format!("q{i}.wav"));
            write_wav(&file, 2.0);
            lib.add_source(&Source::new(t.id, SourceKind::LocalFile, file.display().to_string(), Codec::Wav)).unwrap();
            t.id
        })
        .collect();
    let yt: Arc<dyn StreamService> = Arc::new(FakeService::new(Service::Youtube).with_album("hm3", "Heavy Metal 3", 2));
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
            streams: vec![yt],
            library_dir: dir.path().to_owned(),
            prefs: Prefs::default(),
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    let order = |s: &ViewState| -> Vec<_> { s.queue.current.iter().map(|t| t.id).chain(s.queue.next.iter().map(|e| e.track.id)).collect() };
    let album = RemoteId::new(Service::Youtube, "yt-alb-hm3");
    let track = |n: u32| lib.track_by_remote(&RemoteId::new(Service::Youtube, format!("yt-hm3-{n}"))).unwrap();

    rt.send(Command::PlayTracks { tracks: ids.clone(), start: 0, context: "q".into() });
    wait_for(&rt, "the queue", |s| order(s) == ids);
    rt.send(Command::PlayStreamAlbumNext(album.clone()));
    wait_for(&rt, "the album after the current track", |s| order(s).len() == 5);
    let (a1, a2) = (track(1).expect("the first track is known"), track(2).expect("the second track is known"));
    assert_eq!(order(&rt.state()), [ids[0], a1, a2, ids[1], ids[2]]);
    let loc = rt.state().prefs.resolved_locale();
    assert!(rt.state().toasts.iter().any(|t| t.text == nikgob_core::i18n::PlayerStrings::next_up_quoted_in(loc, "Heavy Metal 3")));

    rt.send(Command::PlayTracks { tracks: ids.clone(), start: 0, context: "q".into() });
    wait_for(&rt, "the queue again", |s| order(s) == ids);
    rt.send(Command::EnqueueStreamAlbum(album.clone()));
    wait_for(&rt, "the album at the end", |s| order(s) == [ids[0], ids[1], ids[2], a1, a2]);

    rt.send(Command::CreatePlaylist("Mix".into()));
    wait_for(&rt, "the playlist", |s| s.library.playlists.iter().any(|p| p.name == "Mix"));
    let mix = rt.state().library.playlists.iter().find(|p| p.name == "Mix").unwrap().id;
    rt.send(Command::AddStreamAlbumToPlaylist(mix, album));
    wait_for(&rt, "the album in the playlist", |_| {
        lib.playlist_entries(mix, 0, 100).unwrap().iter().map(|e| e.track.id).collect::<Vec<_>>() == [a1, a2]
    });
}

/// «Перемешивание» by the kind of list: an album plays in its order by
/// default (shuffle goes off), «Перемешать» still turns it on, shuffle off
/// by hand brings the order back; set to «По кнопке» an album keeps the
/// button as it is, and the library's own lists always do.
#[test]
fn a_new_album_shuffles_as_its_kind_says() {
    use nikgob_core::model::{Artist, ArtistCredit, Codec, Release, Source, SourceKind, Track};

    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open_in_memory().unwrap();
    let artist = lib.upsert_artist(&Artist::new("A")).unwrap();
    let credit = ArtistCredit::single("A", Some(artist.id));
    let release = lib.upsert_release(&Release::new("Album", credit.clone())).unwrap();
    let ids: Vec<_> = (0..4)
        .map(|i| {
            let mut t = Track::new(format!("t{i}"), credit.clone());
            t.release_id = Some(release.id);
            t.track_no = i + 1;
            let t = lib.upsert_track(&t).unwrap();
            let file = dir.path().join(format!("t{i}.wav"));
            write_wav(&file, 2.0);
            lib.add_source(&Source::new(t.id, SourceKind::LocalFile, file.display().to_string(), Codec::Wav)).unwrap();
            t.id
        })
        .collect();
    // No radio after the last track: only the album is queued.
    let prefs = Prefs { autoplay: false, ..Prefs::default() };
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
            streams: Vec::new(),
            library_dir: dir.path().to_owned(),
            prefs,
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    let order = |s: &ViewState| {
        let mut q: Vec<_> = s.queue.prev.iter().map(|e| e.track.id).collect();
        q.extend(s.queue.current.iter().map(|t| t.id));
        q.extend(s.queue.next.iter().map(|e| e.track.id));
        q
    };
    let current = |s: &ViewState| s.queue.current.as_ref().map(|t| t.id);
    let same = |a: &[nikgob_core::model::TrackId]| a.len() == ids.len() && ids.iter().all(|t| a.contains(t));
    use nikgob_view::{ListKind, ListShuffle};
    assert_eq!(rt.state().prefs.list_shuffle.of(ListKind::Album), ListShuffle::InOrder);
    assert!(!rt.state().prefs.shuffle_radio);
    rt.send(Command::ToggleShuffle);
    wait_for(&rt, "shuffle on", |s| s.player.shuffle);
    rt.send(Command::PlayRelease { id: release.id, start: Some(ids[2]), shuffle: false });
    wait_for(&rt, "the album in order, shuffle off", |s| current(s) == Some(ids[2]) && !s.player.shuffle && order(s) == ids);
    // «Перемешать» turns it on all the same.
    rt.send(Command::PlayRelease { id: release.id, start: None, shuffle: true });
    wait_for(&rt, "the album shuffled", |s| s.player.shuffle && s.queue.prev.is_empty() && same(&order(s)));
    let playing = current(&rt.state());
    rt.send(Command::ToggleShuffle);
    wait_for(&rt, "the order back", |s| !s.player.shuffle && order(s) == ids && current(s) == playing);

    // «По кнопке»: the album keeps shuffle on.
    let mut prefs = rt.state().prefs.clone();
    prefs.list_shuffle.set(ListKind::Album, ListShuffle::Button);
    rt.send(Command::SetPrefs(Box::new(prefs)));
    wait_for(&rt, "the album by the button", |s| s.prefs.list_shuffle.of(ListKind::Album) == ListShuffle::Button);
    rt.send(Command::ToggleShuffle);
    wait_for(&rt, "shuffle on again", |s| s.player.shuffle);
    rt.send(Command::PlayRelease { id: release.id, start: Some(ids[1]), shuffle: false });
    wait_for(&rt, "the album under shuffle", |s| current(s) == Some(ids[1]) && s.player.shuffle && same(&order(s)));

    // The library's own lists (downloaded, «Слушать снова», worlds) keep
    // the button: under shuffle «Слушать всё» plays the whole list shuffled…
    rt.send(Command::PlayAll { tracks: ids.clone(), context: "Слушать снова".into() });
    wait_for(&rt, "the whole list under shuffle", |s| s.player.shuffle && s.queue.prev.is_empty() && same(&order(s)));
    // …and with the button off, in order.
    rt.send(Command::ToggleShuffle);
    wait_for(&rt, "shuffle off", |s| !s.player.shuffle);
    rt.send(Command::PlayTracks { tracks: ids.clone(), start: 1, context: "скачанного".into() });
    wait_for(&rt, "the downloaded in order", |s| current(s) == Some(ids[1]) && !s.player.shuffle && order(s) == ids);
}

/// The shuffle button is never fought: turned off anywhere (here while a
/// list of no kind plays), an own playlist («Любимые» too) starts in order;
/// turned on, it starts shuffled. Albums start in their order whatever the
/// button. The button changes no rule of the settings.
#[test]
fn the_shuffle_button_is_kept_by_own_playlists_and_changes_no_rule() {
    use nikgob_core::model::{Artist, ArtistCredit, Codec, Release, Source, SourceKind, Track};
    use nikgob_view::{ListKind, ListShuffle};

    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open_in_memory().unwrap();
    let artist = lib.upsert_artist(&Artist::new("A")).unwrap();
    let credit = ArtistCredit::single("A", Some(artist.id));
    let release = lib.upsert_release(&Release::new("Album", credit.clone())).unwrap();
    let ids: Vec<_> = (0..6)
        .map(|i| {
            let mut t = Track::new(format!("t{i}"), credit.clone());
            t.release_id = Some(release.id);
            t.track_no = i + 1;
            let t = lib.upsert_track(&t).unwrap();
            let file = dir.path().join(format!("t{i}.wav"));
            write_wav(&file, 2.0);
            lib.add_source(&Source::new(t.id, SourceKind::LocalFile, file.display().to_string(), Codec::Wav)).unwrap();
            t.id
        })
        .collect();
    let playlist = lib.create_playlist("P").unwrap();
    for &t in &ids {
        lib.playlist_add(playlist.id, t).unwrap();
    }
    let prefs_path = dir.path().join("prefs.json");
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
            streams: Vec::new(),
            library_dir: dir.path().to_owned(),
            prefs: Prefs { autoplay: false, ..Prefs::default() },
            prefs_path: Some(prefs_path.clone()),
            settings_path: None,
            import: None,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    let order = |s: &ViewState| {
        let mut q: Vec<_> = s.queue.prev.iter().map(|e| e.track.id).collect();
        q.extend(s.queue.current.iter().map(|t| t.id));
        q.extend(s.queue.next.iter().map(|e| e.track.id));
        q
    };
    let rules = |s: &ViewState| s.prefs.list_shuffle;
    let before = rules(&rt.state());
    assert_eq!(before.of(ListKind::OwnPlaylist), ListShuffle::Button);

    // Shuffle on while the library's own list (no kind) plays, then off:
    // the playlist starts in order and stays so.
    rt.send(Command::PlayAll { tracks: ids.clone(), context: "Слушать снова".into() });
    wait_for(&rt, "the whole list", |s| s.queue.current.is_some());
    if !rt.state().player.shuffle {
        rt.send(Command::ToggleShuffle);
        wait_for(&rt, "shuffle on", |s| s.player.shuffle);
    }
    rt.send(Command::ToggleShuffle);
    wait_for(&rt, "shuffle off", |s| !s.player.shuffle);
    rt.send(Command::PlayPlaylist { id: playlist.id, start: Some(ids[0]), shuffle: false });
    wait_for(&rt, "the playlist in order", |s| !s.player.shuffle && order(s) == ids);

    // Shuffle on while it plays: the next start of the playlist is shuffled.
    rt.send(Command::ToggleShuffle);
    wait_for(&rt, "shuffle on", |s| s.player.shuffle);
    rt.send(Command::PlayPlaylist { id: playlist.id, start: Some(ids[0]), shuffle: false });
    wait_for(&rt, "the playlist shuffled", |s| s.player.shuffle && s.queue.current.as_ref().map(|t| t.id) == Some(ids[0]));

    // An album starts in its order even with the button on.
    rt.send(Command::PlayRelease { id: release.id, start: None, shuffle: false });
    wait_for(&rt, "the album in order", |s| !s.player.shuffle && order(s) == ids);

    // No rule changed, and nothing was written for the button.
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(rules(&rt.state()), before);
    let on_disk = std::fs::read(&prefs_path).ok().and_then(|b| serde_json::from_slice::<Prefs>(&b).ok());
    assert!(on_disk.is_none_or(|p| p.list_shuffle == before), "the button wrote a rule");
}

/// «Обновить обложку» says at once that it looks, once for each refresh
/// that runs: a second press while one runs adds no toast of its own.
#[test]
fn a_cover_refresh_says_it_looks_once_while_it_runs() {
    use nikgob_core::model::{Artist, ArtistCredit, Release};

    let lib = Library::open_in_memory().unwrap();
    let artist = lib.upsert_artist(&Artist::new("A")).unwrap();
    let release = lib.upsert_release(&Release::new("Album", ArtistCredit::single("A", Some(artist.id)))).unwrap();
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
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
    // Each toast's count: shown again bumps it.
    let shown = |s: &ViewState, texts: &[&str]| -> u32 {
        s.toasts.iter().filter(|t| texts.contains(&t.text.as_str())).map(|t| t.again + 1).sum()
    };
    let looking = |s: &ViewState| shown(s, &["Ищу обложку…"]);
    let ended = |s: &ViewState| shown(s, &["Лучшей обложки не нашлось", "Не получилось обновить обложку"]);
    rt.send(Command::RefreshReleaseCover(release.id));
    rt.send(Command::RefreshReleaseCover(release.id));
    wait_for(&rt, "the refresh's answer", |s| ended(s) > 0 && looking(s) == ended(s));
    std::thread::sleep(Duration::from_millis(200));
    let s = rt.state();
    assert_eq!(looking(&s), ended(&s), "one «Ищу обложку…» for each refresh that ran");
}

/// Switching `PrefPatch::Language` and `PrefPatch::Edit` updates `prefs.language`,
/// persists it to `prefs.json`, and live-refreshes open views (`player`, `info`,
/// `library.playlists`, `library.downloaded`, `release`) while keeping
/// machine-neutral keys untouched.
#[test]
fn language_pref_patch_updates_and_persists_live() {
    use nikgob_core::model::{Artist, ArtistCredit, Codec, Release, ReleaseType, Source, SourceKind, Track};
    use nikgob_core::quality::{QualityCheck, SoundOrigin, Verdict, row_label_in};
    use nikgob_view::{LangPref, LIKED_PLAYLIST, Locale, PrefPatch};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let lib = Library::open_in_memory().unwrap();
    let artist = lib.upsert_artist(&Artist::new("Artist")).unwrap();
    let credit = ArtistCredit::single("Artist", Some(artist.id));
    let mut album = Release::new("Album", credit.clone());
    album.release_type = ReleaseType::Album;
    let release = lib.upsert_release(&album).unwrap();

    let mut t = Track::new("Song", credit.clone());
    t.release_id = Some(release.id);
    t.track_no = 1;
    let t = lib.upsert_track(&t).unwrap();
    let wav = root.join("song.wav");
    write_wav(&wav, 2.0);
    lib.add_source(&Source::new(t.id, SourceKind::LocalFile, wav.display().to_string(), Codec::Wav)).unwrap();
    lib.like(t.id).unwrap();

    let mut flac_track = Track::new("HiRes Song", credit);
    flac_track.release_id = Some(release.id);
    flac_track.track_no = 2;
    let flac_track = lib.upsert_track(&flac_track).unwrap();
    let flac_src = lib
        .add_source(&Source::new(flac_track.id, SourceKind::LocalFile, root.join("fake.flac").display().to_string(), Codec::Flac))
        .unwrap();
    let check = QualityCheck {
        verdict: Verdict::Transcoded,
        cutoff_hz: 18_000.0,
        steepness_db: 24.0,
        origin: SoundOrigin::Lossy { kbps: 192 },
        sample_rate: 44_100,
        spectrum_db: Vec::new(),
        made_from: None,
        file_size: 0,
        file_mtime: 0,
        checked_at: 0,
    };
    lib.put_quality_check(flac_src.id, &check).unwrap();
    let summary = check.summary();

    let prefs_path = root.join("prefs.json");
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
            streams: Vec::new(),
            library_dir: root.to_owned(),
            prefs: Prefs { language: LangPref::Ru, ..Prefs::default() },
            prefs_path: Some(prefs_path.clone()),
            settings_path: None,
            import: None,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    rt.send(Command::PlayPlaylist { id: LIKED_PLAYLIST, start: None, shuffle: false });
    rt.send(Command::LoadDownloaded);
    rt.send(Command::LoadRelease(release.id));
    wait_for(&rt, "initial Ru views ready", |s| {
        s.player.origin == nikgob_view::QueueOrigin::Liked
            && s.info.sound.is_some()
            && s.library.downloaded.first().is_some_and(|g| g.label == "Сегодня")
            && s.release.as_ref().is_some_and(|r| r.card.id == release.id)
    });
    assert_eq!(
        rt.state().release.as_ref().unwrap().tracks.iter().find(|tr| tr.id == flac_track.id).and_then(|tr| tr.quality.as_deref()),
        Some(row_label_in(&Codec::Flac, Some(&summary), Locale::Ru).as_str()),
    );

    rt.send(Command::SetPref(PrefPatch::Language(LangPref::En)));
    wait_for(&rt, "live switch to En", |s| {
        s.prefs.language == LangPref::En
            && s.library.downloaded.first().is_some_and(|g| g.label == "Today")
            && s.release.as_ref().is_some_and(|r| {
                r.tracks.iter().find(|tr| tr.id == flac_track.id).and_then(|tr| tr.quality.as_deref())
                    == Some(row_label_in(&Codec::Flac, Some(&summary), Locale::En).as_str())
            })
    });

    let s = rt.state();
    assert_eq!(s.player.context_header(Locale::En).0, "Playing from playlist “Liked Songs”");
    assert_eq!(s.player.cached_context().0, "Playing from playlist “Liked Songs”");
    assert_eq!(s.player.context_header(Locale::Ru).0, "Играет из плейлиста «Любимые»");
    assert_eq!(s.info.tags.first().map(|(k, _)| k.as_str()), Some("Title"));
    let liked = s.library.playlists.iter().find(|p| p.id == LIKED_PLAYLIST).unwrap();
    assert_eq!(liked.display_name_in(Locale::En), "Liked Songs");
    assert_eq!(liked.display_name_in(Locale::Ru), "Любимые");
    let entry_id = s.queue.current_entry.expect("liked track playing");
    let before = s.prefs.clone();
    drop(s);

    // Search results resolve track quality under the worker's own locale (`En`).
    rt.send(Command::Search("HiRes".into()));
    wait_for(&rt, "search track quality localized in En", |s| {
        s.search.tracks.iter().any(|tr| {
            tr.id == flac_track.id
                && tr.quality.as_deref() == Some(row_label_in(&Codec::Flac, Some(&summary), Locale::En).as_str())
        })
    });

    // Direct playback error on an active track uses localized error wording in `En`.
    rt.playback_sender()
        .send(nikgob_runtime::PlaybackEvent::Error { id: entry_id, message: "audio error".into() })
        .unwrap();
    wait_for(&rt, "playback error toast in En", |s| {
        s.toasts.iter().any(|toast| toast.text == "Song: could not play")
    });

    // Enqueue and empty-play toasts follow the active English locale.
    rt.send(Command::Enqueue(vec![t.id]));
    wait_for(&rt, "enqueued toast in En", |s| {
        s.toasts.iter().any(|toast| toast.text == nikgob_core::i18n::player_in(Locale::En).added_to_queue)
    });
    let empty_pl = lib.create_playlist("Empty").unwrap();
    rt.send(Command::PlayPlaylist { id: empty_pl.id, start: None, shuffle: false });
    wait_for(&rt, "nothing to play toast in En", |s| {
        s.toasts.iter().any(|toast| toast.text == nikgob_core::i18n::errors_in(Locale::En).nothing_to_play)
    });

    let no_file = lib.upsert_track(&Track::new("NoFile", ArtistCredit::single("Tester", None))).unwrap();
    rt.send(Command::PlayTracks { tracks: vec![no_file.id], start: 0, context: nikgob_view::QueueOrigin::Library });
    wait_for(&rt, "no file for track toast in En", |s| {
        s.toasts.iter().any(|toast| toast.text == nikgob_core::i18n::errors_in(Locale::En).no_file_for_track)
    });

    let saved: Prefs = serde_json::from_slice(&std::fs::read(&prefs_path).unwrap()).unwrap();
    assert_eq!(saved.language, LangPref::En);

    // Editing `language` via `PrefPatch::Edit` routes through `set_prefs` too.
    let mut after = before.clone();
    after.language = LangPref::Ru;
    rt.send(Command::SetPref(PrefPatch::Edit { before: Box::new(before), after: Box::new(after) }));
    wait_for(&rt, "live switch back to Ru via PrefPatch::Edit", |s| {
        s.prefs.language == LangPref::Ru
            && s.library.downloaded.first().is_some_and(|g| g.label == "Сегодня")
            && s.release.as_ref().is_some_and(|r| {
                r.tracks.iter().find(|tr| tr.id == flac_track.id).and_then(|tr| tr.quality.as_deref())
                    == Some(row_label_in(&Codec::Flac, Some(&summary), Locale::Ru).as_str())
            })
    });

    let saved_ru: Prefs = serde_json::from_slice(&std::fs::read(&prefs_path).unwrap()).unwrap();
    assert_eq!(saved_ru.language, LangPref::Ru);
}

#[test]
fn stream_resolve_rate_limit_pause_and_missing_youtube_localize_in_en() {
    use nikgob_core::model::{ArtistCredit, Codec, Source, SourceKind, Track};
    use nikgob_core::streaming::{FakeService, Service};
    use nikgob_view::{LangPref, Locale, QueueOrigin};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(FakeService::youtube().logged_out());
    let sp = FakeService::spotify();

    let stream_track = sp.tracks().into_iter().find(|t| t.id.id == "sp-gruppa-1").unwrap();
    let t_lim = lib.upsert_stream_track(&stream_track).unwrap();

    let t_short = lib
        .upsert_track(&Track::new("ShortPauseTrack", ArtistCredit::single("Tester", None)))
        .unwrap();
    lib.add_source(&Source::new(t_short.id, SourceKind::Deezer, "dz-short-pause", Codec::Opus)).unwrap();

    let t_local = lib
        .upsert_track(&Track::new("UnlistedLocalZxq987", ArtistCredit::single("Tester", None)))
        .unwrap();
    let wav = root.join("local.wav");
    write_wav(&wav, 2.0);
    lib.add_source(&Source::new(t_local.id, SourceKind::LocalFile, wav.display().to_string(), Codec::Wav)).unwrap();

    let rt = Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
            streams: vec![yt.clone()],
            library_dir: root.to_owned(),
            prefs: Prefs { language: LangPref::En, ..Prefs::default() },
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    wait_for(&rt, "accounts initialized", |s| s.accounts.len() == Service::ALL.len());

    // Long lookup rate limit stops playback with localized resolve-limited toast.
    yt.rate_limit_next("search_kind", 1, Some(Duration::from_secs(600)));
    rt.send(Command::PlayTracks { tracks: vec![t_lim], start: 0, context: QueueOrigin::Library });
    let prefix = format!("{}: YouTube Music is asking you to wait until ", stream_track.title);
    wait_for(&rt, "resolve rate-limited toast in En", |s| {
        s.toasts.iter().any(|toast| toast.text.starts_with(&prefix) && toast.text.ends_with(" — try playing this track later"))
    });

    // Short lookup rate limit announces localized auto-resume pause toast.
    yt.rate_limit_next("search_kind", 1, Some(Duration::from_secs(10)));
    rt.send(Command::PlayTracks { tracks: vec![t_short.id], start: 0, context: QueueOrigin::Library });
    wait_for(&rt, "pause toast in En", |s| {
        s.toasts.iter().any(|toast| {
            toast.text == nikgob_core::i18n::ErrorStrings::pause_will_resume_in(Locale::En, "YouTube Music", None)
        })
    });

    // Missing YouTube Music lookup after miss emits localized `not_on_youtube` toast.
    yt.rate_limit_next("search_kind", 0, None);
    let before_calls = yt.calls("search_kind");
    rt.send(Command::PlayTracks { tracks: vec![t_local.id], start: 0, context: QueueOrigin::Library });
    wait_for(&rt, "local track active", |s| {
        s.queue.current.as_ref().map(|t| t.id) == Some(t_local.id)
    });
    rt.send(Command::PlayFromYoutube);
    wait_for(&rt, "YouTube lookup missed", |s| {
        yt.calls("search_kind") > before_calls && s.player.status == nikgob_view::Status::Stopped
    });
    rt.send(Command::PlayTracks { tracks: vec![t_local.id], start: 0, context: QueueOrigin::Library });
    wait_for(&rt, "local track active again", |s| {
        s.queue.current.as_ref().map(|t| t.id) == Some(t_local.id)
    });
    rt.send(Command::PlayFromYoutube);
    wait_for(&rt, "not on YouTube Music toast in En", |s| {
        s.toasts.iter().any(|toast| toast.text == nikgob_core::i18n::errors_in(Locale::En).not_on_youtube)
    });
}

#[test]
fn remote_page_and_settings_errors_localize_in_en() {
    use nikgob_core::streaming::{FakeService, RemoteId, Service};
    use nikgob_view::{LangPref, Locale};

    let err = nikgob_core::i18n::errors_in(Locale::En);
    let yt = Arc::new(FakeService::youtube().logged_out());

    // Missing `settings_path` + remote page fetch failure via `with_page`.
    let rt_no_settings = Runtime::start(
        RuntimeConfig {
            library: Library::open_in_memory().unwrap(),
            streams: vec![yt],
            library_dir: std::env::temp_dir(),
            prefs: Prefs { language: LangPref::En, ..Prefs::default() },
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    rt_no_settings.send(Command::PlayStreamAlbum {
        id: RemoteId::new(Service::Youtube, "missing-album"),
        start: None,
        shuffle: false,
    });
    wait_for(&rt_no_settings, "remote page error toast in En", |s| {
        s.toasts.iter().any(|t| t.text == "Failed: Unavailable")
    });
    rt_no_settings.send(Command::SetIndexer {
        url: "http://127.0.0.1:9696".into(),
        api_key: "key".into(),
    });
    wait_for(&rt_no_settings, "missing settings path toast in En (indexer)", |s| {
        s.toasts.iter().any(|t| t.text == err.no_settings_path)
    });
    let id = rt_no_settings.state().toasts.iter().find(|t| t.text == err.no_settings_path).map(|t| t.id);
    if let Some(id) = id {
        rt_no_settings.send(Command::DismissToast(id));
        wait_for(&rt_no_settings, "dismissed indexer toast", |s| s.toasts.iter().all(|t| t.id != id));
    }
    rt_no_settings.send(Command::SetStreamSecrets {
        spotify_client_id: Some("cid".into()),
        youtube_cookies: None,
    });
    wait_for(&rt_no_settings, "missing settings path toast in En (secrets)", |s| {
        s.toasts.iter().any(|t| t.text == err.no_settings_path)
    });
    drop(rt_no_settings);

    // Unreadable `settings.toml` and invalid Prowlarr config under `LangPref::En`.
    let dir = tempfile::tempdir().unwrap();
    let settings_path = dir.path().join("settings.toml");
    std::fs::write(&settings_path, "not = [valid toml").unwrap();
    let rt_settings = Runtime::start(
        RuntimeConfig {
            library: Library::open_in_memory().unwrap(),
            streams: Vec::new(),
            library_dir: dir.path().to_owned(),
            prefs: Prefs { language: LangPref::En, ..Prefs::default() },
            prefs_path: None,
            settings_path: Some(settings_path.clone()),
            import: None,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    rt_settings.send(Command::SetIndexer {
        url: "http://127.0.0.1:9696".into(),
        api_key: "key".into(),
    });
    wait_for(&rt_settings, "unreadable settings.toml toast in En (indexer)", |s| {
        s.toasts.iter().any(|t| t.text == err.settings_unreadable)
    });
    let id = rt_settings.state().toasts.iter().find(|t| t.text == err.settings_unreadable).map(|t| t.id);
    if let Some(id) = id {
        rt_settings.send(Command::DismissToast(id));
        wait_for(&rt_settings, "dismissed unreadable toast", |s| s.toasts.iter().all(|t| t.id != id));
    }
    rt_settings.send(Command::SetStreamSecrets {
        spotify_client_id: Some("cid".into()),
        youtube_cookies: None,
    });
    wait_for(&rt_settings, "unreadable settings.toml toast in En (secrets)", |s| {
        s.toasts.iter().any(|t| t.text == err.settings_unreadable)
    });

    std::fs::write(&settings_path, "").unwrap();
    rt_settings.send(Command::SetIndexer {
        url: "ftp://not-http".into(),
        api_key: String::new(),
    });
    wait_for(&rt_settings, "invalid prowlarr toast in En", |s| {
        s.toasts.iter().any(|t| t.text == err.prowlarr_invalid)
    });
}

#[test]
fn mesh_and_sync_errors_localize_in_en() {
    use nikgob_core::i18n::{ErrorStrings, errors_in};
    use nikgob_core::streaming::{FakeService, PlaylistSyncMode, RemoteId, Service};
    use nikgob_library::PlaylistRemote;
    use nikgob_view::{LangPref, Locale};

    let err = errors_in(Locale::En);

    // Missing `settings_path` on mesh toggle + unlinked playlist sync without signed-in streams.
    let lib_no_settings = Library::open_in_memory().unwrap();
    let unlinked = lib_no_settings.create_playlist("Unlinked").unwrap();
    let rt_no_settings = Runtime::start(
        RuntimeConfig {
            library: lib_no_settings,
            streams: Vec::new(),
            library_dir: std::env::temp_dir(),
            prefs: Prefs { language: LangPref::En, ..Prefs::default() },
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    rt_no_settings.send(Command::MeshSetEnabled(true));
    wait_for(&rt_no_settings, "missing settings path toast in En (mesh)", |s| {
        s.toasts.iter().any(|t| t.text == err.no_settings_path)
    });
    rt_no_settings.send(Command::SetPlaylistSync {
        playlist: unlinked.id,
        mode: PlaylistSyncMode::TwoWay,
    });
    wait_for(&rt_no_settings, "link playlist sign-in toast in En", |s| {
        s.toasts.iter().any(|t| t.text == err.link_playlist_sign_in)
    });
    drop(rt_no_settings);

    // Unreadable `settings.toml` on mesh toggle + remote sync pull failure.
    let dir = tempfile::tempdir().unwrap();
    let settings_path = dir.path().join("settings.toml");
    std::fs::write(&settings_path, "not = [valid toml").unwrap();
    let lib_sync = Library::open_in_memory().unwrap();
    let linked = lib_sync.create_playlist("Linked").unwrap();
    lib_sync
        .put_playlist_remote(&PlaylistRemote {
            playlist_id: linked.id,
            remote: RemoteId::new(Service::Spotify, "sp-pl"),
            sync_mode: PlaylistSyncMode::Once,
            remote_version: None,
            synced_at: None,
            base_json: None,
        })
        .unwrap();
    let sp = Arc::new(FakeService::spotify());
    sp.fail_next("playlist", 1);
    let rt_sync = Runtime::start(
        RuntimeConfig {
            library: lib_sync,
            streams: vec![sp],
            library_dir: dir.path().to_owned(),
            prefs: Prefs { language: LangPref::En, ..Prefs::default() },
            prefs_path: None,
            settings_path: Some(settings_path),
            import: None,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    rt_sync.send(Command::MeshSetEnabled(true));
    wait_for(&rt_sync, "unreadable settings.toml toast in En (mesh)", |s| {
        s.toasts.iter().any(|t| t.text == err.settings_unreadable)
    });
    rt_sync.send(Command::SetPlaylistSync {
        playlist: linked.id,
        mode: PlaylistSyncMode::TwoWay,
    });
    let expected_sync_err = ErrorStrings::sync_failed_in(Locale::En, err.stream_network);
    wait_for(&rt_sync, "sync failed toast in En", |s| {
        s.toasts.iter().any(|t| t.text == expected_sync_err)
    });
}

#[test]
fn masters_dir_errors_localize_in_en() {
    use nikgob_core::i18n::{ErrorStrings, errors_in};
    use nikgob_view::{LangPref, Locale};

    let err = errors_in(Locale::En);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cfg = ImportConfig::with_root(root);

    // Unavailable when import service is disabled.
    let rt_no_import = Runtime::start(
        RuntimeConfig {
            library: Library::open_in_memory().unwrap(),
            streams: Vec::new(),
            library_dir: cfg.library_dir.clone(),
            prefs: Prefs { language: LangPref::En, ..Prefs::default() },
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    rt_no_import.send(Command::MoveMastersDir(root.join("archive")));
    wait_for(&rt_no_import, "masters move unavailable toast in En", |s| {
        s.toasts.iter().any(|t| t.text == err.masters_move_unavailable)
    });
    drop(rt_no_import);

    // Moving into a prohibited working folder (Inbox / Downloads / music root).
    std::fs::create_dir_all(&cfg.inbox_dir).unwrap();
    let lib = Library::open_in_memory().unwrap();
    let rt_import = Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
            streams: Vec::new(),
            library_dir: cfg.library_dir.clone(),
            prefs: Prefs { language: LangPref::En, ..Prefs::default() },
            prefs_path: Some(root.join("prefs.json")),
            settings_path: None,
            import: Some(ImportSetup {
                config: cfg.clone(),
                metadata: None,
                watch_inbox: false,
                torrent_state_dir: root.join("torrent-state"),
                torrent_offline: true,
                models_dir: None,
            }),
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    rt_import.send(Command::MoveMastersDir(cfg.inbox_dir.join("nested")));
    wait_for(&rt_import, "masters inside work dir toast in En", |s| {
        s.toasts.iter().any(|t| t.text == err.masters_inside_work_dir)
    });

    // Asynchronous `relocate_masters` refusal (`NamedMastersInAlbumFolders`) localizes in full in En.
    let wav = root.join("named.wav");
    write_wav(&wav, 2.5);
    rt_import.send(Command::ImportFiles(vec![wav]));
    wait_for(&rt_import, "imported named master", |s| s.library.track_count == 1 && s.library.tracks.len() == 1);
    let track1 = rt_import.state().library.tracks[0].id;
    let old_masters = cfg.masters_dir();
    let expected_refusal = ErrorStrings::masters_move_failed_in(
        Locale::En,
        &ErrorStrings::masters_named_in_albums_in(Locale::En, &old_masters.display().to_string(), 1),
    );
    rt_import.send(Command::MoveMastersDir(root.join("hdd_flac_archive")));
    wait_for(&rt_import, "complete English refused relocation toast", |s| {
        s.toasts.iter().any(|t| t.text == expected_refusal)
    });

    // Asynchronous hash-verification failure (`HashVerificationFailed`) localizes in full in En.
    lib.delete_master(track1).unwrap();
    let bad_dest = root.join("hdd_flac_archive_bad");
    let fake_hash = "0".repeat(64);
    let bad_src = old_masters.join(format!("{fake_hash}.wav"));
    write_wav(&bad_src, 0.5);
    let actual_hash = nikgob_import::fsutil::hash_file(&bad_src).unwrap();
    let file_size = nikgob_import::fsutil::file_size(&bad_src).unwrap();
    let bad_dst = bad_dest.join(format!("{fake_hash}.wav"));
    let expected_hash_err = ErrorStrings::masters_move_failed_in(
        Locale::En,
        &ErrorStrings::hash_verification_failed_in(
            Locale::En,
            &bad_dst.display().to_string(),
            &fake_hash,
            Some(file_size),
            &actual_hash,
            Some(file_size),
        ),
    );
    rt_import.send(Command::MoveMastersDir(bad_dest));
    wait_for(&rt_import, "complete English hash verification failure toast", |s| {
        s.toasts.iter().any(|t| t.text == expected_hash_err)
    });
}


