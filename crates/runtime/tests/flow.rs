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

    rt.send(Command::UndoTrash(track));
    wait_for(&rt, "the undo", |_| expected.is_file());
    assert!(lib.active_source(track).unwrap().is_some());
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
