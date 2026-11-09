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
    wait_for(&rt, "the second import", |s| s.library.track_count == 1 && s.library.tracks.len() == 1);
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
            prefs: Prefs::default(),
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
            prefs: Prefs::default(),
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
    assert_eq!(s.player.context.as_deref(), Some("Тест"));
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
        page(s) == Some(Route::Playlist(playlist.id)) && s.player.context_line() == "Играет из плейлиста «P»"
    });
    rt.send(Command::PlayArtist { id: artist.id, shuffle: false });
    wait_for(&rt, "the artist's page", |s| page(s) == Some(Route::Artist(artist.id)));
    rt.send(Command::PlayTracks { tracks: ids.clone(), start: 0, context: "Тест".into() });
    wait_for(&rt, "no page", |s| s.player.context.as_deref() == Some("Тест") && page(s).is_none());
    rt.send(Command::PlayRelease { id: release.id, start: Some(ids[1]), shuffle: false });
    wait_for(&rt, "the album's page", |s| {
        page(s) == Some(Route::Release(release.id)) && s.player.context_line() == "Играет из альбома «Album»"
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
    wait_for(&rt, "no page again", |s| s.player.context.as_deref() == Some("Тест") && page(s).is_none());
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
            library: lib,
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
    rt.send(Command::PlayRelease { id: release.id, start: None, shuffle: false });
    wait_for(&rt, "the whole album", |s| playing(s, 0, 3, true));
    rt.send(Command::PlayArtist { id: artist.id, shuffle: false });
    wait_for(&rt, "the artist's downloaded songs", |s| playing(s, 0, 2, false));
    rt.send(Command::PlayRelease { id: release.id, start: Some(ids[2]), shuffle: false });
    wait_for(&rt, "the whole album from a song", |s| playing(s, 2, 3, true));
    rt.send(Command::PlayPlaylist { id: playlist.id, start: None, shuffle: false });
    wait_for(&rt, "the playlist's downloaded songs", |s| playing(s, 2, 2, false));
    // Picked in the playlist, the song that only streams joins (it cannot
    // play here: no service, so the next one plays).
    rt.send(Command::PlayPlaylist { id: playlist.id, start: Some(ids[1]), shuffle: false });
    wait_for(&rt, "the picked stream song", |s| queued(s).len() == 3);
    rt.send(Command::PlayArtist { id: artist.id, shuffle: false });
    wait_for(&rt, "the artist's downloaded songs again", |s| playing(s, 0, 2, false));
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
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib,
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
    assert!(rt.state().toasts.iter().any(|t| t.text == "Следующим: «Album»"));
    // Only the downloaded: the album still goes in whole.
    rt.send(Command::SetPref(nikgob_view::PrefPatch::MixesDownloadedOnly(true)));
    rt.send(Command::PlayTracks { tracks: ids.clone(), start: 0, context: "q".into() });
    wait_for(&rt, "the queue again", |s| order(s) == ids && s.prefs.mixes_downloaded_only);
    rt.send(Command::PlayReleaseNext(album.id));
    let want = vec![ids[0], a1.id, a2.id, a3.id, ids[1], ids[2]];
    wait_for(&rt, "the whole album after the current track", |s| order(s) == want);
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
    assert!(rt.state().toasts.iter().any(|t| t.text == "Следующим: «Heavy Metal 3»"));

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

/// «Перемешивание остаётся включённым»: on (the default), a new album keeps
/// shuffle on; off, it turns shuffle off and plays in order, «Перемешать»
/// still turns it on, and shuffle off by hand brings the order back.
#[test]
fn a_new_album_keeps_shuffle_as_the_setting_says() {
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
            library: lib,
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
    assert!(rt.state().prefs.keep_shuffle && !rt.state().prefs.shuffle_radio);
    rt.send(Command::ToggleShuffle);
    wait_for(&rt, "shuffle on", |s| s.player.shuffle);
    rt.send(Command::PlayRelease { id: release.id, start: Some(ids[1]), shuffle: false });
    wait_for(&rt, "the album under shuffle", |s| current(s) == Some(ids[1]) && s.player.shuffle && same(&order(s)));

    let prefs = Prefs { keep_shuffle: false, ..rt.state().prefs.clone() };
    rt.send(Command::SetPrefs(Box::new(prefs)));
    wait_for(&rt, "the setting off", |s| !s.prefs.keep_shuffle);
    rt.send(Command::PlayRelease { id: release.id, start: Some(ids[2]), shuffle: false });
    wait_for(&rt, "the album in order, shuffle off", |s| current(s) == Some(ids[2]) && !s.player.shuffle && order(s) == ids);
    // «Перемешать» turns it on all the same.
    rt.send(Command::PlayRelease { id: release.id, start: None, shuffle: true });
    wait_for(&rt, "the album shuffled", |s| s.player.shuffle && s.queue.prev.is_empty() && same(&order(s)));
    let playing = current(&rt.state());
    rt.send(Command::ToggleShuffle);
    wait_for(&rt, "the order back", |s| !s.player.shuffle && order(s) == ids && current(s) == playing);

    // The library's own lists (downloaded, «Слушать снова», worlds) go the
    // same way: with the setting off, shuffle goes off and they play in order.
    rt.send(Command::ToggleShuffle);
    wait_for(&rt, "shuffle on again", |s| s.player.shuffle);
    rt.send(Command::PlayTracks { tracks: ids.clone(), start: 1, context: "скачанного".into() });
    wait_for(&rt, "the downloaded in order, shuffle off", |s| {
        current(s) == Some(ids[1]) && !s.player.shuffle && order(s) == ids
    });
    // With it on, «Слушать всё» under shuffle plays the whole list shuffled.
    let prefs = Prefs { keep_shuffle: true, ..rt.state().prefs.clone() };
    rt.send(Command::SetPrefs(Box::new(prefs)));
    wait_for(&rt, "the setting on", |s| s.prefs.keep_shuffle);
    rt.send(Command::ToggleShuffle);
    wait_for(&rt, "shuffle on", |s| s.player.shuffle);
    rt.send(Command::PlayAll { tracks: ids.clone(), context: "Слушать снова".into() });
    wait_for(&rt, "the whole list under shuffle", |s| s.player.shuffle && s.queue.prev.is_empty() && same(&order(s)));
}
