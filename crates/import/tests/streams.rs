//! Files of liked stream tracks: a downloaded or user-supplied file attaches
//! to the stream track's id (by ISRC / MBID, by artist + title + length, or
//! by the job's targets); another length is asked about; audio saved from a
//! stream is a low-quality local source that a real file upgrades.

mod common;

use common::*;
use nikgob_core::model::{Codec, SourceKind, SourceOrigin, TrackId, score};
use nikgob_core::streaming::{FakeService, StreamTrack};
use nikgob_import::tags::{self, TagSet};
use nikgob_import::{ImportEvent, ImportTargets, Importer, events::STREAM_OPTION};
use nikgob_library::Library;

const RATE: u32 = 44_100;

fn importer(root: &Root) -> (Importer, Library) {
    let lib = Library::open_in_memory().unwrap();
    (Importer::new(root.cfg.clone(), lib.clone(), None), lib)
}

/// "Группа крови" (Кино) of the fake Spotify catalog, `secs` long, liked.
fn liked_stream_track(lib: &Library, secs: u64) -> (TrackId, StreamTrack) {
    let mut t = FakeService::spotify()
        .tracks()
        .into_iter()
        .find(|t| t.id.id == "sp-gruppa-1")
        .unwrap();
    t.duration_ms = Some(secs * 1000);
    let id = lib.upsert_stream_track(&t).unwrap();
    lib.like(id).unwrap();
    (id, t)
}

fn wav(root: &Root, name: &str, secs: f64, tags: &TagSet) -> std::path::PathBuf {
    let path = root.inbox(name);
    write_wav(&path, &broadband(RATE, secs, 7), RATE);
    tags::write_tags(&path, tags).unwrap();
    path
}

fn tags_of(title: &str, artist: &str) -> TagSet {
    TagSet {
        title: Some(title.into()),
        artists: vec![artist.into()],
        artist: Some(artist.into()),
        album: Some("Сборник".into()),
        album_artist: Some(artist.into()),
        track_no: Some(7),
        year: Some(2001),
        ..TagSet::default()
    }
}

fn imported(events: &[ImportEvent]) -> Vec<TrackId> {
    events
        .iter()
        .filter_map(|e| match e {
            ImportEvent::Imported { track_id, .. } => Some(*track_id),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_of_a_liked_stream_track_attaches_to_its_id() {
    let root = Root::new();
    let (importer, lib) = importer(&root);
    let (id, _) = liked_stream_track(&lib, 6);
    let playlist = lib.create_playlist("Вечер").unwrap();
    lib.playlist_add(playlist.id, id).unwrap();
    // Another album, no ids: artist + title + length within 3 s.
    let file = wav(&root, "a.wav", 5.0, &tags_of("Группа Крови", "Кино"));

    let events = importer
        .import_paths(vec![file], SourceOrigin::Inbox)
        .wait()
        .await;
    assert_eq!(imported(&events), vec![id], "{events:?}");
    let src = lib.active_source(id).unwrap().unwrap();
    assert_eq!(src.kind, SourceKind::LocalFile);
    assert!(lib.is_liked(id).unwrap(), "the like stays");
    assert_eq!(lib.playlist_entries(playlist.id, 0, 10).unwrap()[0].track.id, id);
    assert_eq!(lib.sources_of_track(id).unwrap().len(), 2, "the stream stays as a backup");
}

#[tokio::test(flavor = "multi_thread")]
async fn another_length_is_asked_about_not_attached() {
    let root = Root::new();
    let (importer, lib) = importer(&root);
    let (id, _) = liked_stream_track(&lib, 285);
    let file = wav(&root, "tv.wav", 4.0, &tags_of("Группа крови", "Кино"));
    let events = importer
        .import_paths(vec![file], SourceOrigin::Inbox)
        .wait()
        .await;
    assert!(imported(&events).is_empty(), "{events:?}");
    let item = importer.pending().pop().expect("parked");
    assert_eq!(item.options.len(), 1);
    assert_eq!(item.options[0].source, STREAM_OPTION);
    assert_eq!(item.options[0].title, "Группа крови");
    // Yes: it is that track after all.
    let outcome = importer.resolve(item.id, Some(0)).await.unwrap();
    assert!(
        matches!(outcome, nikgob_import::FileOutcome::Imported { track_id, .. } if track_id == id),
        "{outcome:?}"
    );
    assert!(lib.active_source(id).unwrap().unwrap().kind.is_local());

    // No: a track of its own; the stream track keeps waiting for its file.
    let root = Root::new();
    let (importer, lib) = self::importer(&root);
    let (id, _) = liked_stream_track(&lib, 285);
    let file = wav(&root, "tv.wav", 4.0, &tags_of("Группа крови", "Кино"));
    importer
        .import_paths(vec![file], SourceOrigin::Inbox)
        .wait()
        .await;
    let item = importer.pending().pop().expect("parked");
    let outcome = importer.resolve(item.id, None).await.unwrap();
    let nikgob_import::FileOutcome::Imported { track_id, .. } = outcome else {
        panic!("{outcome:?}");
    };
    assert_ne!(track_id, id);
    assert!(!lib.active_source(id).unwrap().unwrap().kind.is_local());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_job_for_a_wishlist_item_attaches_its_file_by_number_or_by_force() {
    let root = Root::new();
    let (importer, lib) = importer(&root);
    let (id, _) = liked_stream_track(&lib, 6);
    // A torrent file named a little differently, number 1 like the stream track.
    let mut t = tags_of("Gruppa krovy", "Kino");
    t.track_no = Some(1);
    let file = wav(&root, "01.wav", 6.0, &t);
    let events = importer
        .import_paths_for(vec![file], SourceOrigin::Inbox, ImportTargets::new(vec![id]))
        .wait()
        .await;
    assert_eq!(imported(&events), vec![id], "{events:?}");

    // Forced: the user attached this file to the item, whatever it says.
    let root = Root::new();
    let (importer, lib) = self::importer(&root);
    let (id, _) = liked_stream_track(&lib, 285);
    let file = wav(&root, "whatever.wav", 3.0, &tags_of("Unrelated", "Someone"));
    let targets = ImportTargets {
        tracks: vec![id],
        force: true,
    };
    let events = importer
        .import_paths_for(vec![file], SourceOrigin::Inbox, targets)
        .wait()
        .await;
    assert_eq!(imported(&events), vec![id], "{events:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn audio_saved_from_a_stream_is_kept_as_is_and_upgraded_by_a_real_file() {
    let root = Root::new();
    let (importer, lib) = importer(&root);
    let (id, _) = liked_stream_track(&lib, 6);
    let saved = root.dir.path().join("yt-gruppa-1.wav");
    write_wav(&saved, &broadband(RATE, 6.0, 7), RATE);
    let targets = ImportTargets {
        tracks: vec![id],
        force: true,
    };
    let events = importer
        .import_paths_for(vec![saved.clone()], SourceOrigin::Stream, targets)
        .wait()
        .await;
    assert_eq!(imported(&events), vec![id], "{events:?}");
    assert!(!events.iter().any(|e| matches!(e, ImportEvent::MasterRegistered(_))));
    let src = lib.active_source(id).unwrap().unwrap();
    assert!(src.kind.is_local());
    assert_eq!(src.codec, Codec::Wav, "not converted");
    assert_eq!(src.quality_score, score::SAVED_STREAM);
    assert_eq!(src.origin, SourceOrigin::Stream);
    assert!(saved.exists(), "a file outside the Inbox is copied");
    let track = lib.get_track(id).unwrap().unwrap();
    assert_eq!(track.title, "Группа крови", "an untagged file takes the track's metadata");
    let album = lib.get_release(track.release_id.unwrap()).unwrap().unwrap();
    assert_eq!(album.title, "Группа крови");

    // The torrent comes: same album and number, a real lossless file.
    let mut t = tags_of("Группа крови", "Кино");
    t.album = Some("Группа крови".into());
    t.track_no = Some(1);
    t.year = Some(1988);
    let flac = wav(&root, "01 - Группа крови.wav", 6.0, &t);
    let events = importer
        .import_paths(vec![flac], SourceOrigin::Inbox)
        .wait()
        .await;
    assert!(
        events.iter().any(|e| matches!(e, ImportEvent::Imported { track_id, upgraded: true, .. } if *track_id == id)),
        "{events:?}"
    );
    let src = lib.active_source(id).unwrap().unwrap();
    assert_eq!(src.codec, Codec::Opus);
    assert!(src.quality_score > score::SAVED_STREAM);
}
