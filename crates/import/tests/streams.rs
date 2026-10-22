//! Files of liked stream tracks: a downloaded or user-supplied file attaches
//! to the stream track's id (by ISRC / MBID, by artist + title + length, or
//! by the job's targets); another length is asked about; audio saved from a
//! stream is a low-quality local source that a real file upgrades.

mod common;

use common::*;
use nikgob_core::model::{Codec, SourceKind, SourceOrigin, TrackId, score};
use nikgob_core::streaming::{FakeService, StreamTrack};
use nikgob_import::tags::{self, TagSet};
use nikgob_import::{
    ImportEvent, ImportTargets, Importer,
    events::{STREAM_OPTION, TAGS_OPTION},
};
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
    assert_eq!(item.options.len(), 2);
    assert_eq!(item.options[0].source, TAGS_OPTION, "the file's own tags first");
    assert_eq!(item.options[1].source, STREAM_OPTION);
    assert_eq!(item.options[1].title, "Группа крови");
    assert_eq!(item.options[1].duration_ms, Some(285_000));
    // Yes: it is that track after all.
    let outcome = importer.resolve(item.id, Some(1)).await.unwrap();
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
        ..ImportTargets::default()
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
        ..ImportTargets::default()
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

#[tokio::test(flavor = "multi_thread")]
async fn the_confirm_queue_survives_a_restart() {
    let root = Root::new();
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(dir.path().join("library.sqlite")).unwrap();
    let (id, _) = liked_stream_track(&lib, 285);
    let file = wav(&root, "tv.wav", 4.0, &tags_of("Группа крови", "Кино"));
    {
        let importer = Importer::new(root.cfg.clone(), lib.clone(), None);
        importer.import_paths(vec![file.clone()], SourceOrigin::Inbox).wait().await;
        assert_eq!(importer.pending().len(), 1);
    }
    // The player starts again: the item is still there, and still works.
    let importer = Importer::new(root.cfg.clone(), lib.clone(), None);
    let item = importer.pending().pop().expect("restored");
    assert_eq!(item.path, file);
    assert_eq!(item.options[1].source, STREAM_OPTION);
    let outcome = importer.resolve(item.id, Some(1)).await.unwrap();
    assert!(
        matches!(outcome, nikgob_import::FileOutcome::Imported { track_id, .. } if track_id == id),
        "{outcome:?}"
    );
    assert!(importer.pending().is_empty());
    assert!(Importer::new(root.cfg.clone(), lib, None).pending().is_empty(), "gone from the database too");
}

/// A stream track of «Ugly Is Beautiful» by Oliver Tree, `secs` long, as
/// «Хочу скачать» makes it (outside the library until its file comes).
fn album_track(lib: &Library, no: u32, title: &str, secs: u64) -> TrackId {
    use nikgob_core::streaming::{AlbumRef, ArtistRef, RemoteId, Service};
    let mut t = StreamTrack::new(RemoteId::new(Service::Deezer, format!("dz-{no}")), title);
    t.artists = vec![ArtistRef::new(None, "Oliver Tree")];
    t.album = Some(AlbumRef::new(Some(RemoteId::new(Service::Deezer, "dz-ugly")), "Ugly Is Beautiful"));
    t.track_no = Some(no);
    t.disc_no = Some(1);
    t.duration_ms = Some(secs * 1000);
    lib.upsert_stream_track(&t).unwrap()
}

/// A torrent folder of `files` (number, title, length), tagged by `tags`.
fn torrent_folder(root: &Root, name: &str, files: &[(u32, &str, f64)], tags: impl Fn(u32, &str) -> TagSet) -> std::path::PathBuf {
    let dir = root.cfg.downloads_dir.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    for (no, title, secs) in files {
        let path = dir.join(format!("{no}. {title}.wav"));
        write_wav(&path, &broadband(RATE, *secs, u64::from(*no) + 11), RATE);
        tags::write_tags(&path, &tags(*no, title)).unwrap();
    }
    dir
}

fn torrent() -> SourceOrigin {
    SourceOrigin::Torrent {
        info_hash: "bdc5cd1b6f16431f6d7877651a92ece1821850a6".into(),
        path_in_torrent: String::new(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_torrent_for_a_wishlist_album_lands_on_its_release_and_its_stream_tracks() {
    let root = Root::new();
    let (importer, lib) = importer(&root);
    let one = album_track(&lib, 1, "Me, Myself & I", 3);
    let three = album_track(&lib, 3, "Cash Machine", 4);
    let release = lib.get_track(one).unwrap().unwrap().release_id.unwrap();
    let files = [(1, "Me, Myself & I", 3.0), (2, "1993", 5.0), (3, "Cash Machine", 4.0)];
    // Tagged a little differently from the service.
    let dir = torrent_folder(&root, "Oliver Tree - Ugly Is Beautiful (2020) [FLAC] [Hi-Res 24-Bit]", &files, |no, title| TagSet {
        title: Some(title.into()),
        artist: Some("Oliver Tree".into()),
        album: Some("Ugly is Beautiful".into()),
        track_no: Some(no),
        year: Some(2020),
        ..TagSet::default()
    });
    let targets = ImportTargets { tracks: vec![one, three], release: Some(release), ..ImportTargets::default() };
    let events = importer.import_paths_for(vec![dir], torrent(), targets).wait().await;
    assert!(importer.pending().is_empty(), "nothing to confirm: {events:?}");
    let got = imported(&events);
    assert_eq!(got.len(), 3, "{events:?}");
    assert!(got.contains(&one) && got.contains(&three), "the files took the stream tracks");
    let rows = lib.tracks_of_release_all(release).unwrap();
    assert_eq!(rows.len(), 3, "the third file joined the wishlist's release");
    assert!(rows.iter().all(|r| r.has_local));
    assert_eq!(rows.iter().map(|r| r.track_no).collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(lib.count_releases().unwrap(), 1, "one album card");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_untagged_torrent_folder_is_one_album_named_by_the_folder() {
    let root = Root::new();
    let (importer, lib) = importer(&root);
    let files = [(1, "Me, Myself & I", 3.0), (2, "Cash Machine", 4.0), (3, "Hurt", 3.0)];
    let dir = torrent_folder(&root, "Oliver Tree - Ugly Is Beautiful (2020) [FLAC] [Hi-Res 24-Bit]", &files, |_, _| TagSet::default());
    let events = importer.import_paths(vec![dir], torrent()).wait().await;
    assert_eq!(imported(&events).len(), 3, "{events:?}");
    let releases = lib.releases(0, 10, nikgob_library::ReleaseSort::asc(Default::default())).unwrap();
    assert_eq!(releases.len(), 1, "{releases:?}");
    assert_eq!(releases[0].release.title, "Ugly Is Beautiful");
    assert_eq!(releases[0].release.artist_credit.display(), "Oliver Tree");
    let titles: Vec<String> = lib.tracks_of_release(releases[0].release.id).unwrap().into_iter().map(|t| t.title).collect();
    assert_eq!(titles, ["Me, Myself & I", "Cash Machine", "Hurt"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_torrent_album_scattered_over_strangers_goes_back_together_once() {
    use nikgob_core::model::{Download, DownloadStatus, SeedPolicy, WishlistItem, WishlistScope, WishlistStatus};
    let root = Root::new();
    let (importer, lib) = importer(&root);
    // «Хочу скачать» asked for the album; the service knew one of its songs.
    let wished = album_track(&lib, 2, "Cash Machine", 4);
    let release = lib.get_track(wished).unwrap().unwrap().release_id.unwrap();
    let mut item = WishlistItem::new(WishlistStatus::Done, 1);
    item.scope = WishlistScope::Album;
    item.release_id = Some(release);
    item.title = Some("Ugly Is Beautiful".into());
    item.status = WishlistStatus::Downloading;
    lib.put_wishlist_item(&item).unwrap();
    let SourceOrigin::Torrent { info_hash, .. } = torrent() else { unreachable!() };
    lib.put_download(&Download {
        info_hash,
        magnet: None,
        torrent_file: None,
        selected_files: Vec::new(),
        status: DownloadStatus::Done,
        seed_policy: SeedPolicy::Standard,
        bytes_up: 0,
        bytes_down: 0,
        wishlist_id: Some(item.id),
        created_at: 1,
        updated_at: 1,
    })
    .unwrap();
    // How the old importer left it: each file under a stranger the confirm
    // queue offered for its title.
    let files = [(1, "Me, Myself & I", 3.0), (2, "Cash Machine", 4.0), (3, "Hurt", 3.0)];
    let strangers = ["Sean Kelly", "DRAM", "King Coop"];
    let dir = torrent_folder(&root, "Oliver Tree - Ugly Is Beautiful (2020) [FLAC]", &files, |no, title| TagSet {
        title: Some(title.into()),
        artist: Some(strangers[no as usize - 1].into()),
        album: Some(format!("Album of {}", strangers[no as usize - 1])),
        track_no: Some(1),
        ..TagSet::default()
    });
    let events = importer.import_paths(vec![dir.clone()], torrent()).wait().await;
    assert_eq!(imported(&events).len(), 3, "{events:?}");
    assert_eq!(lib.count_releases().unwrap(), 3, "three strangers' singles");
    // The torrent's own files say who they are.
    for (no, title, _) in files {
        let tags = TagSet {
            title: Some(title.into()),
            artist: Some("Oliver Tree".into()),
            album: Some("Ugly Is Beautiful".into()),
            track_no: Some(no),
            ..TagSet::default()
        };
        tags::write_tags(&dir.join(format!("{no}. {title}.wav")), &tags).unwrap();
    }

    let report = importer.repair_torrent_identities(true).unwrap();
    assert_eq!(report.moved.len(), 3, "{report:?}");
    let rows = lib.tracks_of_release_all(release).unwrap();
    assert_eq!(
        rows.iter().map(|r| (r.track_no, r.title.as_str(), r.artist.as_str(), r.has_local)).collect::<Vec<_>>(),
        [(1, "Me, Myself & I", "Oliver Tree", true), (2, "Cash Machine", "Oliver Tree", true), (3, "Hurt", "Oliver Tree", true)]
    );
    assert_eq!(rows[1].id, wished, "the file took the album's stream track");
    assert_eq!(lib.count_releases().unwrap(), 1, "the strangers' releases are gone");
    let names: Vec<String> = lib
        .artists(0, 100, nikgob_library::ArtistSort::asc(Default::default()))
        .unwrap()
        .into_iter()
        .map(|a| a.artist.name)
        .collect();
    assert_eq!(names, ["Oliver Tree"]);
    assert_eq!(report.wishlist_done, ["Ugly Is Beautiful"]);
    assert_eq!(lib.get_wishlist_item(item.id).unwrap().unwrap().status, WishlistStatus::Done);
    // The files moved to the album's folder, with its tags.
    assert_eq!(report.files_moved, 3);
    for r in &rows {
        let src = lib.active_source(r.id).unwrap().unwrap();
        let path = std::path::PathBuf::from(&src.locator);
        assert!(path.starts_with(root.cfg.library_dir.join("Oliver Tree")), "{path:?}");
        let (t, _) = tags::read_tags(&path).unwrap();
        assert_eq!((t.artist.as_deref(), t.album.as_deref()), (Some("Oliver Tree"), Some("Ugly Is Beautiful")));
    }
    assert!(importer.repair_torrent_identities(true).unwrap().moved.is_empty(), "once");
}

/// A soundtrack's torrent: album artist OMORI, each track by its composer.
fn omori(root: &Root) -> std::path::PathBuf {
    let files = [(1, "Title", 3.0), (2, "By Your Side.", 4.0), (3, "Final Duet", 3.0)];
    let composers = ["Pedro Silva", "Jami Lynne", "bo en"];
    torrent_folder(&root, "OMORI OST - Pedro Silva, Jami Lynne, bo en (2020)", &files, |no, title| TagSet {
        title: Some(title.into()),
        artist: Some(composers[no as usize - 1].into()),
        album_artist: Some("OMORI".into()),
        album: Some("OMORI OST".into()),
        track_no: Some(no),
        ..TagSet::default()
    })
}

fn artists_of(lib: &Library) -> Vec<String> {
    let rel = lib.releases(0, 10, nikgob_library::ReleaseSort::asc(Default::default())).unwrap();
    assert_eq!(rel.len(), 1, "{rel:?}");
    assert_eq!((rel[0].release.title.as_str(), rel[0].release.artist_credit.display()), ("OMORI OST", "OMORI".to_owned()));
    lib.tracks_of_release(rel[0].release.id).unwrap().into_iter().map(|t| t.artist).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_soundtrack_keeps_its_composers_and_the_repairs_leave_it_alone() {
    let root = Root::new();
    let (importer, lib) = importer(&root);
    let dir = omori(&root);
    let events = importer.import_paths(vec![dir], torrent()).wait().await;
    assert_eq!(imported(&events).len(), 3, "{events:?}");
    assert_eq!(artists_of(&lib), ["Pedro Silva", "Jami Lynne", "bo en"], "each track keeps its own artist");
    assert!(importer.repair_torrent_identities(true).unwrap().moved.is_empty(), "nothing is scattered");
    assert!(importer.repair_torrent_artists(true).unwrap().is_empty());
    assert_eq!(artists_of(&lib), ["Pedro Silva", "Jami Lynne", "bo en"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_composers_the_first_repair_took_come_back_from_the_torrent_files() {
    let root = Root::new();
    let (importer, lib) = importer(&root);
    let dir = omori(&root);
    let events = importer.import_paths(vec![dir], torrent()).wait().await;
    let tracks = imported(&events);
    assert_eq!(tracks.len(), 3);
    // What the first rule did: the album artist on two composers' tracks,
    // in the library and in the files.
    for t in &tracks {
        let row = lib.get_track_row(*t).unwrap().unwrap();
        if row.artist != "Pedro Silva" {
            let changes = nikgob_import::TagChanges {
                artist: Some("OMORI".into()),
                album_artist: Some("OMORI".into()),
                keep_album_artist: true,
                ..Default::default()
            };
            importer.retag(*t, changes).unwrap();
        }
    }
    assert_eq!(artists_of(&lib), ["Pedro Silva", "OMORI", "OMORI"]);

    let back = importer.repair_torrent_artists(true).unwrap();
    assert_eq!(back.len(), 2, "{back:?}");
    assert_eq!(artists_of(&lib), ["Pedro Silva", "Jami Lynne", "bo en"]);
    for t in &tracks {
        let src = lib.active_source(*t).unwrap().unwrap();
        let (tags, _) = tags::read_tags(std::path::Path::new(&src.locator)).unwrap();
        assert_eq!(tags.album_artist.as_deref(), Some("OMORI"), "the album artist stays");
        assert_ne!(tags.artist.as_deref(), Some("OMORI"), "the file says its composer again");
    }
    assert!(importer.repair_torrent_artists(true).unwrap().is_empty(), "once");
}
