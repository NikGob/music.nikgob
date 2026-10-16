mod common;

use common::*;
use nikgob_core::model::{Codec, SourceKind, SourceOrigin};
use nikgob_import::tags::{self, TagSet};
use nikgob_import::{
    EncodeOptions, Encoder, ImportEvent, Importer, RopusEncoder, delete_track_to_trash,
    rebuild_from_files, trash_purge,
};
use nikgob_library::{Library, ReleaseSort, TrackFilter};

const RATE: u32 = 44_100;

fn song_tags(title: &str) -> TagSet {
    TagSet {
        title: Some(title.into()),
        artists: vec!["Кино".into()],
        artist: Some("Кино".into()),
        album: Some("Группа крови".into()),
        album_artist: Some("Кино".into()),
        track_no: Some(3),
        track_total: Some(9),
        year: Some(1988),
        ..TagSet::default()
    }
}

fn importer(root: &Root) -> (Importer, Library) {
    let lib = Library::open_in_memory().unwrap();
    (Importer::new(root.cfg.clone(), lib.clone(), None), lib)
}

fn imported(events: &[ImportEvent]) -> Vec<(nikgob_core::model::TrackId, bool)> {
    events
        .iter()
        .filter_map(|e| match e {
            ImportEvent::Imported {
                track_id, upgraded, ..
            } => Some((*track_id, *upgraded)),
            _ => None,
        })
        .collect()
}

fn failures(events: &[ImportEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            ImportEvent::Failed { error, .. } => Some(error.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn wav_in_the_inbox_becomes_a_tagged_opus_track_with_everything_derived() {
    let root = Root::new();
    let (importer, lib) = importer(&root);
    let wav = root.inbox("song.wav");
    write_wav(&wav, &broadband(RATE, 6.0, 1), RATE);
    tags::write_tags(&wav, &song_tags("Группа крови")).unwrap();
    std::fs::write(root.inbox("cover.png"), solid_png([200, 30, 40], 800, 800)).unwrap();

    let events = importer
        .import_paths(vec![root.cfg.inbox_dir.clone()], SourceOrigin::Inbox)
        .wait()
        .await;
    assert!(failures(&events).is_empty(), "{events:?}");
    assert!(matches!(events[0], ImportEvent::Started { files: 1, .. }));
    let done = imported(&events);
    assert_eq!(done.len(), 1, "{events:?}");
    let (track_id, upgraded) = done[0];
    assert!(!upgraded);
    assert!(events.iter().any(|e| matches!(e, ImportEvent::MasterRegistered(m) if m.track_id == track_id)));
    assert!(matches!(
        events.last(),
        Some(ImportEvent::Finished {
            imported: 1,
            failed: 0,
            pending: 0,
            ..
        })
    ));

    // Layout: Music/<Artist>/<YYYY - Album>/<NN - Title>.opus + cover files.
    let album = root.cfg.library_dir.join("Кино").join("1988 - Группа крови");
    let file = album.join("03 - Группа крови.opus");
    assert!(file.is_file(), "{:?}", std::fs::read_dir(&album).map(|r| r.count()));
    assert!(album.join("cover.jpg").is_file());
    assert!(album.join("ambient.png").is_file());
    assert_eq!(count_files(&album), 3, "no temp files left behind");

    // Library rows.
    let src = lib.active_source(track_id).unwrap().expect("active source");
    assert_eq!(src.kind, SourceKind::LocalFile);
    assert_eq!(src.codec, Codec::Opus);
    assert_eq!(src.locator, file.to_string_lossy());
    assert_eq!(src.quality_score, 900, "keeps the rank of the lossless original");
    assert_eq!(src.sample_rate, Some(48_000));
    assert!(src.bitrate.is_some_and(|b| (60..300).contains(&b)), "{src:?}");
    assert_eq!(src.origin, SourceOrigin::Inbox);
    assert_eq!(
        src.file_hash.as_deref(),
        Some(blake3::hash(&std::fs::read(&file).unwrap()).to_hex().as_str())
    );
    let track = lib.get_track(track_id).unwrap().unwrap();
    assert_eq!(track.title, "Группа крови");
    assert_eq!((track.disc_no, track.track_no), (1, 3));
    assert!(track.duration_ms.is_some_and(|d| (5_900..6_100).contains(&d)));
    let release = lib.get_release(track.release_id.unwrap()).unwrap().unwrap();
    assert_eq!(release.title, "Группа крови");
    assert_eq!(release.year, Some(1988));
    assert_eq!(release.total_tracks, Some(9));
    assert_eq!(release.cover_ref.as_deref(), Some("Кино/1988 - Группа крови/cover.jpg"));
    assert_eq!(lib.count_artists().unwrap(), 1);

    // Palette seed from the red cover and the 16x16 ambient texture.
    let palette = lib.get_palette(release.id).unwrap().expect("palette");
    let (r, g) = ((palette.seed_argb >> 16) & 0xFF, (palette.seed_argb >> 8) & 0xFF);
    assert!(r > g + 60, "{:08X}", palette.seed_argb);
    let ambient = image::load_from_memory(&palette.ambient_blur.unwrap()).unwrap();
    assert_eq!((ambient.width(), ambient.height()), (16, 16));

    // Tags in the file: our id, MBID-less identity, loudness, fingerprint, cover.
    let (t, info) = tags::read_tags(&file).unwrap();
    assert_eq!(t.track_id, Some(track_id));
    assert_eq!(t.title.as_deref(), Some("Группа крови"));
    assert_eq!(t.album_artist.as_deref(), Some("Кино"));
    assert_eq!(t.track_no, Some(3));
    assert!(t.r128_track_gain.is_some());
    assert!(t.acoustid_fingerprint.is_some());
    assert!(t.cover.is_some());
    assert_eq!(info.codec, Codec::Opus);

    // The FLAC/WAV original is a master: moved out of the Inbox, with a sidecar.
    assert!(!wav.exists(), "Inbox original moved to masters");
    let masters: Vec<_> = std::fs::read_dir(root.cfg.masters_dir())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(masters.iter().any(|n| n.ends_with(".wav")), "{masters:?}");
    assert!(masters.iter().any(|n| n.ends_with(".json")), "{masters:?}");

    // The database can be rebuilt from the files, keeping the ids.
    let fresh = Library::open_in_memory().unwrap();
    let report = rebuild_from_files(&fresh, &root.cfg).unwrap();
    assert_eq!(report.files, 1, "{report:?}");
    assert_eq!(report.tracks_created, 1);
    assert_eq!(report.ids_assigned, 0);
    let rebuilt = fresh.get_track(track_id).unwrap().expect("same id");
    assert_eq!(rebuilt.title, "Группа крови");
    let rsrc = fresh.active_source(track_id).unwrap().unwrap();
    assert_eq!(rsrc.quality_score, 900, "rank restored from the master sidecar");
    assert!(fresh.get_palette(rebuilt.release_id.unwrap()).unwrap().is_some());
    assert_eq!(fresh.count_artists().unwrap(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn awkward_names_are_made_safe_and_lossy_files_are_copied_untouched() {
    let root = Root::new();
    let (importer, lib) = importer(&root);
    // An Opus file counts as lossy: copied, never re-encoded.
    let wav = root.dir.path().join("src.wav");
    write_wav(&wav, &broadband(RATE, 4.0, 2), RATE);
    let opus = root.inbox("whatever.opus");
    RopusEncoder
        .encode_to_opus(&wav, &opus, &EncodeOptions { bitrate_kbps: 128 })
        .unwrap();
    let before = std::fs::read(&opus).unwrap();
    let mut t = song_tags("What? CON: the \"end\"...");
    t.album = Some("AC/DC: Live".into());
    t.album_artist = Some("AC/DC".into());
    t.artist = Some("AC/DC".into());
    t.artists = vec!["AC/DC".into()];
    t.track_no = Some(1);
    tags::write_tags(&opus, &t).unwrap();

    let events = importer
        .import_paths(vec![opus.clone()], SourceOrigin::Inbox)
        .wait()
        .await;
    assert!(failures(&events).is_empty(), "{events:?}");
    let (track_id, _) = imported(&events)[0];
    let src = lib.active_source(track_id).unwrap().unwrap();
    let path = std::path::PathBuf::from(&src.locator);
    let rel = path.strip_prefix(&root.cfg.library_dir).unwrap();
    let parts: Vec<_> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        parts,
        ["AC-DC", "1988 - AC-DC - Live", "01 - What_ CON - the 'end'.opus"],
        "{parts:?}"
    );
    assert!(!opus.exists(), "the Inbox copy is gone after a lossy import");
    assert!(
        !events.iter().any(|e| matches!(e, ImportEvent::MasterRegistered(_))),
        "lossy files have no master"
    );
    // Same audio bytes: only the tags changed, the Opus packets are untouched.
    let after = std::fs::read(&path).unwrap();
    assert!(after.len() >= before.len() / 2);
    assert_eq!(src.codec, Codec::Opus);
    assert!((600..=820).contains(&src.quality_score), "{}", src.quality_score);
    assert_eq!(src.origin, SourceOrigin::Inbox);
}

#[tokio::test(flavor = "multi_thread")]
async fn fake_lossless_is_scored_as_lossy_and_kept_unconverted() {
    let root = Root::new();
    let (importer, lib) = importer(&root);
    let fake = root.inbox("fake.wav");
    write_wav(&fake, &lowpassed_noise(RATE, 20.0, 16_000.0, 7), RATE);
    tags::write_tags(&fake, &song_tags("Fake")).unwrap();

    let events = importer
        .import_paths(vec![fake.clone()], SourceOrigin::Inbox)
        .wait()
        .await;
    assert!(failures(&events).is_empty(), "{events:?}");
    let (track_id, _) = imported(&events)[0];
    let src = lib.active_source(track_id).unwrap().unwrap();
    assert_eq!(src.quality_score, 690, "fake FLAC ranks below MP3 V0");
    assert_eq!(src.codec, Codec::Wav, "never lossy to lossy: kept as a lossless copy");
    assert!(src.locator.ends_with(".wav"));
    assert!(!fake.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn upgrade_replaces_the_file_keeps_the_track_and_worse_copies_are_duplicates() {
    let root = Root::new();
    let (importer, lib) = importer(&root);
    let audio = broadband(RATE, 8.0, 3);
    let work = root.dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let wav = work.join("master.wav");
    write_wav(&wav, &audio, RATE);

    let make_low = |name: &str| {
        let p = root.inbox(name);
        RopusEncoder
            .encode_to_opus(&wav, &p, &EncodeOptions { bitrate_kbps: 96 })
            .unwrap();
        tags::write_tags(&p, &song_tags("Группа крови")).unwrap();
        p
    };

    // 1. A 96 kbps Opus arrives first.
    let events = importer
        .import_paths(vec![make_low("low.opus")], SourceOrigin::Inbox)
        .wait()
        .await;
    assert!(failures(&events).is_empty(), "{events:?}");
    let (track_id, upgraded) = imported(&events)[0];
    assert!(!upgraded);
    let old = lib.active_source(track_id).unwrap().unwrap();
    assert!(old.quality_score < 800, "{}", old.quality_score);
    let old_path = std::path::PathBuf::from(&old.locator);
    assert!(old_path.is_file());

    // 2. The lossless version upgrades it: same track id, old file in Trash.
    let hi = root.inbox("hi.wav");
    std::fs::copy(&wav, &hi).unwrap();
    tags::write_tags(&hi, &song_tags("Группа крови")).unwrap();
    let events = importer
        .import_paths(vec![hi.clone()], SourceOrigin::Inbox)
        .wait()
        .await;
    assert!(failures(&events).is_empty(), "{events:?}");
    assert_eq!(imported(&events), [(track_id, true)], "{events:?}");
    let new = lib.active_source(track_id).unwrap().unwrap();
    assert_eq!(new.quality_score, 900);
    assert_ne!(new.id, old.id);
    assert!(std::path::Path::new(&new.locator).is_file());
    assert_eq!(lib.count_tracks(TrackFilter::All).unwrap(), 1, "no second track");
    let trash = lib.list_trash().unwrap();
    assert_eq!(trash.len(), 1);
    assert_eq!(trash[0].source.id, old.id);
    let in_trash = std::path::PathBuf::from(trash[0].trash_path.clone().unwrap());
    assert!(in_trash.is_file());
    let name = in_trash.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.as_bytes()[4] == b'-' && name.as_bytes()[7] == b'-', "date prefix: {name}");
    assert_eq!(lib.sources_of_track(track_id).unwrap().len(), 1);

    // 3. A worse copy and the very same lossless file again are duplicates;
    //    Inbox files go to Trash.
    let low2 = make_low("low2.opus");
    let hi2 = root.inbox("hi2.wav");
    std::fs::copy(&wav, &hi2).unwrap();
    tags::write_tags(&hi2, &song_tags("Группа крови")).unwrap();
    let events = importer
        .import_paths(vec![low2.clone(), hi2.clone()], SourceOrigin::Inbox)
        .wait()
        .await;
    assert!(failures(&events).is_empty(), "{events:?}");
    let dups: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            ImportEvent::Duplicate { kept, .. } => Some(*kept),
            _ => None,
        })
        .collect();
    assert_eq!(dups, [track_id, track_id], "{events:?}");
    assert!(imported(&events).is_empty());
    assert!(!low2.exists() && !hi2.exists(), "duplicates leave the Inbox");
    assert_eq!(lib.active_source(track_id).unwrap().unwrap().id, new.id);
    assert_eq!(lib.count_tracks(TrackFilter::All).unwrap(), 1);

    // 4. Deleting the track sends the file to Trash and the track out of the
    //    library (into the Trash view); the album folder keeps its cover for
    //    a restore.
    let moved = delete_track_to_trash(&lib, &root.cfg.trash_dir, &root.cfg.library_dir, track_id).unwrap();
    assert_eq!(moved.len(), 1);
    assert!(moved[0].is_file());
    assert!(!std::path::Path::new(&new.locator).exists());
    assert_eq!(lib.count_tracks(TrackFilter::All).unwrap(), 0);
    assert_eq!(lib.count_releases().unwrap(), 0);
    assert_eq!(lib.deleted_tracks().unwrap().len(), 1);
    assert!(lib.get_track(track_id).unwrap().is_some());

    // 5. Restore brings the file and the track back; deleting again for the purge.
    let r = nikgob_import::restore_track(&lib, track_id).unwrap();
    assert_eq!((r.files_restored, r.files_failed), (1, 0));
    assert!(std::path::Path::new(&new.locator).is_file());
    assert_eq!(lib.count_tracks(TrackFilter::LocalOnly).unwrap(), 1);
    let moved = delete_track_to_trash(&lib, &root.cfg.trash_dir, &root.cfg.library_dir, track_id).unwrap();

    // 6. Purge after the retention period removes rows and files, the old
    //    replaced file included, and the empty album folder.
    std::thread::sleep(std::time::Duration::from_millis(20));
    assert_eq!(trash_purge(&lib, 0).unwrap().rows, 0, "a deleted track's files go with it");
    let report = nikgob_import::purge_deleted_tracks(&lib, &root.cfg, 0).unwrap();
    assert_eq!(report.rows, 1, "{report:?}");
    assert!(report.files_deleted >= 2, "{report:?}");
    assert!(lib.list_trash().unwrap().is_empty());
    assert!(lib.get_track(track_id).unwrap().is_none());
    assert!(!moved[0].exists() && !in_trash.exists());
    assert!(!root.cfg.library_dir.join("Кино").exists(), "empty album folders are pruned");
}

#[tokio::test(flavor = "multi_thread")]
async fn files_without_tags_still_import_from_their_names() {
    let root = Root::new();
    let (importer, lib) = importer(&root);
    let dir = root.dir.path().join("elsewhere");
    std::fs::create_dir_all(&dir).unwrap();
    let wav = dir.join("07 - Наутилус Помпилиус - Скованные одной цепью.wav");
    write_wav(&wav, &broadband(RATE, 5.0, 4), RATE);

    let events = importer
        .import_paths(vec![wav.clone()], SourceOrigin::Inbox)
        .wait()
        .await;
    assert!(failures(&events).is_empty(), "{events:?}");
    let (id, _) = imported(&events)[0];
    let row = lib.get_track_row(id).unwrap().unwrap();
    assert!(row.title.contains("Скованные"), "{row:?}");
    assert_eq!(row.track_no, 7);
    assert!(row.artist.contains("Наутилус"), "{row:?}");
    // A file outside the Inbox is left alone and its master is a copy.
    assert!(wav.exists());
    let masters = std::fs::read_dir(root.cfg.masters_dir()).unwrap().count();
    assert_eq!(masters, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn torrent_payloads_are_not_moved_and_unreadable_files_fail_cleanly() {
    let root = Root::new();
    let (importer, lib) = importer(&root);
    let album = root.cfg.downloads_dir.join("Some Album [FLAC]");
    std::fs::create_dir_all(&album).unwrap();
    let flac = album.join("01 - Song.wav");
    write_wav(&flac, &broadband(RATE, 5.0, 5), RATE);
    tags::write_tags(&flac, &song_tags("Song")).unwrap();
    let bad = album.join("02 - Broken.wav");
    std::fs::write(&bad, b"this is not audio").unwrap();
    std::fs::write(album.join("x.torrent"), b"d4:infod4:name1:aee").unwrap();

    let origin = SourceOrigin::Torrent {
        info_hash: "abc123".into(),
        path_in_torrent: String::new(),
    };
    let events = importer.import_paths(vec![album.clone()], origin).wait().await;
    let (id, _) = imported(&events)[0];
    assert_eq!(failures(&events).len(), 1, "{events:?}");
    assert!(events.iter().any(|e| matches!(e, ImportEvent::TorrentFound(_))));
    assert!(flac.exists(), "torrent data stays for seeding");
    assert!(bad.exists());
    let src = lib.active_source(id).unwrap().unwrap();
    assert_eq!(
        src.origin,
        SourceOrigin::Torrent {
            info_hash: "abc123".into(),
            path_in_torrent: "Some Album [FLAC]/01 - Song.wav".into()
        }
    );
    // The master is the downloaded file itself, nothing was copied.
    let masters = std::fs::read_dir(root.cfg.masters_dir()).unwrap().flatten().count();
    assert_eq!(masters, 1, "only the sidecar");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs ffmpeg on PATH to write a libFLAC-compatible FLAC"]
async fn flac_masters_are_converted_and_kept() {
    let root = Root::new();
    let (importer, lib) = importer(&root);
    let wav = root.dir.path().join("src.wav");
    write_wav(&wav, &broadband(RATE, 5.0, 9), RATE);
    let flac = root.inbox("a.flac");
    ffmpeg_flac(&wav, &flac);
    tags::write_tags(&flac, &song_tags("Flac Song")).unwrap();

    let events = importer
        .import_paths(vec![flac.clone()], SourceOrigin::Inbox)
        .wait()
        .await;
    assert!(failures(&events).is_empty(), "{events:?}");
    let (id, _) = imported(&events)[0];
    let src = lib.active_source(id).unwrap().unwrap();
    assert_eq!((src.codec.clone(), src.quality_score), (Codec::Opus, 900));
    let master = events
        .iter()
        .find_map(|e| match e {
            ImportEvent::MasterRegistered(m) => Some(m.clone()),
            _ => None,
        })
        .expect("master");
    assert_eq!(master.codec, Codec::Flac);
    assert!(root.cfg.masters_dir().join(format!("{}.flac", master.file_hash)).is_file());
    assert!(!flac.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn retag_deletes_emptied_fields_and_renames_on_a_case_change() {
    let root = Root::new();
    let (importer, lib) = importer(&root);
    let wav = root.inbox("song.wav");
    write_wav(&wav, &broadband(RATE, 4.0, 5), RATE);
    let mut t = song_tags("song");
    t.genre = Some("Rock".into());
    tags::write_tags(&wav, &t).unwrap();
    let events = importer
        .import_paths(vec![wav], SourceOrigin::Inbox)
        .wait()
        .await;
    let (track_id, _) = imported(&events)[0];

    // Only the letter case of the title changes: same file, not "(2)".
    let path = importer
        .retag(
            track_id,
            nikgob_import::TagChanges {
                title: Some("Song".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(path.file_name().unwrap().to_string_lossy(), "03 - Song.opus");
    assert_eq!(count_files(path.parent().unwrap()), 1, "{path:?}");

    // Emptied fields leave the file too.
    let path = importer
        .retag(
            track_id,
            nikgob_import::TagChanges {
                album: Some(String::new()),
                genre: Some(String::new()),
                year: Some(None),
                ..Default::default()
            },
        )
        .unwrap();
    let (set, _) = tags::read_tags(&path).unwrap();
    assert_eq!(set.album, None);
    assert_eq!(set.genre, None);
    assert_eq!(set.year, None);
    assert_eq!(set.title.as_deref(), Some("Song"));
    assert_eq!(set.track_id, Some(track_id));
    assert_eq!(lib.active_source(track_id).unwrap().unwrap().locator, path.to_string_lossy());
}

fn album_tags(title: &str, no: u32, artist: &str, album_artist: Option<&str>, album: &str) -> TagSet {
    TagSet {
        title: Some(title.into()),
        artists: artist.split(", ").map(Into::into).collect(),
        artist: Some(artist.into()),
        album: Some(album.into()),
        album_artist: album_artist.map(Into::into),
        track_no: Some(no),
        year: Some(2022),
        ..TagSet::default()
    }
}

fn write_album_track(dir: &std::path::Path, tags: &TagSet) {
    std::fs::create_dir_all(dir).unwrap();
    let no = tags.track_no.unwrap();
    let wav = dir.join(format!("{no:02}.wav"));
    write_wav(&wav, &broadband(RATE, 3.0, 10 + u64::from(no)), RATE);
    tags::write_tags(&wav, tags).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_folder_with_a_guest_track_and_other_casing_is_one_release() {
    let root = Root::new();
    let (importer, lib) = importer(&root);
    let dir = root.inbox("Suzume");
    write_album_track(&dir, &album_tags("A", 1, "Radwimps", Some("Radwimps"), "Suzume"));
    write_album_track(&dir, &album_tags("B", 2, "RADWIMPS", Some("RADWIMPS"), "Suzume"));
    write_album_track(&dir, &album_tags("C", 3, "Radwimps", Some("Radwimps"), "Suzume"));
    // The guest track has no album artist, its credit names a second artist.
    write_album_track(&dir, &album_tags("D", 4, "RADWIMPS, Kazuma Jinnouchi", None, "Suzume"));

    let events = importer.import_paths(vec![dir], SourceOrigin::Inbox).wait().await;
    assert!(failures(&events).is_empty(), "{events:?}");
    assert_eq!(imported(&events).len(), 4, "{events:?}");
    assert_eq!(lib.count_releases().unwrap(), 1);
    let release = lib.releases(0, 10, ReleaseSort::asc(Default::default())).unwrap().remove(0);
    assert_eq!(release.known_tracks, 4);
    assert_eq!(release.release.artist_credit.display().to_lowercase(), "radwimps");
    assert_eq!(lib.count_artists().unwrap(), 2, "Radwimps and Kazuma Jinnouchi, no joint artist");

    // A guest track imported later on its own joins the album and does not rename it.
    let late = root.inbox("late");
    write_album_track(&late, &album_tags("E", 5, "Radwimps, Kazuma Jinnouchi", None, "Suzume"));
    let events = importer.import_paths(vec![late], SourceOrigin::Inbox).wait().await;
    assert!(failures(&events).is_empty(), "{events:?}");
    assert_eq!(lib.count_releases().unwrap(), 1);
    let release = lib.releases(0, 10, ReleaseSort::asc(Default::default())).unwrap().remove(0);
    assert_eq!(release.known_tracks, 5);
    assert_eq!(release.release.artist_credit.display().to_lowercase(), "radwimps");
}

#[tokio::test(flavor = "multi_thread")]
async fn casing_and_suffix_variants_of_the_album_title_do_not_split_the_folder() {
    let root = Root::new();
    let (importer, lib) = importer(&root);
    let dir = root.inbox("Made in Abyss OST 2");
    for (no, album) in [
        (1, "Made in Abyss Original Soundtrack 2"),
        (2, "MADE IN ABYSS ORIGINAL SOUNDTRACK 2"),
        (3, "Made in Abyss Original Soundtrack 2"),
        (4, "Made in Abyss Original Soundtrack 2 Bonus"),
    ] {
        let artist = if no == 2 { "KEVIN PENKIN" } else { "Kevin Penkin" };
        write_album_track(&dir, &album_tags(&format!("T{no}"), no, artist, Some(artist), album));
    }
    let events = importer.import_paths(vec![dir], SourceOrigin::Inbox).wait().await;
    assert!(failures(&events).is_empty(), "{events:?}");
    assert_eq!(imported(&events).len(), 4, "{events:?}");
    assert_eq!(lib.count_releases().unwrap(), 1);
    assert_eq!(lib.count_artists().unwrap(), 1);
    let release = lib.releases(0, 10, ReleaseSort::asc(Default::default())).unwrap().remove(0);
    assert_eq!(release.known_tracks, 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn unrelated_files_in_one_folder_stay_separate_releases() {
    let root = Root::new();
    let (importer, lib) = importer(&root);
    let dir = root.inbox("downloads");
    for no in 1..=3u32 {
        let (artist, album) = (format!("Artist {no}"), format!("Album {no}"));
        write_album_track(&dir, &album_tags(&format!("T{no}"), no, &artist, Some(&artist), &album));
    }
    let events = importer.import_paths(vec![dir], SourceOrigin::Inbox).wait().await;
    assert!(failures(&events).is_empty(), "{events:?}");
    assert_eq!(lib.count_releases().unwrap(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_of_another_album_is_not_pulled_into_a_two_against_one_folder() {
    let root = Root::new();
    let (importer, lib) = importer(&root);
    let dir = root.inbox("mix");
    write_album_track(&dir, &album_tags("A1", 1, "Artist A", Some("Artist A"), "Album A"));
    write_album_track(&dir, &album_tags("A2", 2, "Artist A", Some("Artist A"), "Album A"));
    write_album_track(&dir, &album_tags("B1", 3, "Artist B", Some("Artist B"), "Album B"));
    let events = importer.import_paths(vec![dir], SourceOrigin::Inbox).wait().await;
    assert!(failures(&events).is_empty(), "{events:?}");
    assert_eq!(lib.count_releases().unwrap(), 2);
    let b = lib.search("B1", 5).unwrap().tracks.remove(0);
    let release = lib.get_release(lib.get_track(b.id).unwrap().unwrap().release_id.unwrap()).unwrap().unwrap();
    assert_eq!(release.title, "Album B");
    let (tags, _) = tags::read_tags(std::path::Path::new(&lib.active_source(b.id).unwrap().unwrap().locator)).unwrap();
    assert_eq!((tags.album.as_deref(), tags.album_artist.as_deref()), (Some("Album B"), Some("Artist B")));
}

fn release_with(lib: &Library, title: &str, credit: &str, mbid: Option<&str>, tracks: &[u32]) -> nikgob_core::model::ReleaseId {
    use nikgob_core::model::{ArtistCredit, Release, Track};
    let mut release = Release::new(title, ArtistCredit::single(credit, None));
    release.mbid = mbid.map(Into::into);
    let release = lib.upsert_release(&release).unwrap();
    for &no in tracks {
        let mut t = Track::new(format!("T{no}"), ArtistCredit::single(credit, None));
        t.release_id = Some(release.id);
        t.track_no = no;
        lib.upsert_track(&t).unwrap();
    }
    release.id
}

#[test]
fn releases_split_by_an_earlier_version_are_merged() {
    let lib = Library::open_in_memory().unwrap();
    let main = release_with(&lib, "Suzume (Motion Picture Soundtrack)", "Radwimps", None, &[1, 2]);
    let same = release_with(&lib, "SUZUME (MOTION PICTURE SOUNDTRACK)", "RADWIMPS", None, &[3]);
    let guest = release_with(&lib, "Suzume (Motion Picture Soundtrack)", "RADWIMPS, Kazuma Jinnouchi", None, &[4]);
    let other = release_with(&lib, "Another Album", "Radwimps", None, &[1]);
    assert_eq!(nikgob_import::merge_split_releases(&lib).unwrap(), 2);
    assert_eq!(lib.count_releases().unwrap(), 2);
    assert_eq!(lib.tracks_of_release(main).unwrap().len(), 4);
    assert!(lib.get_release(same).unwrap().is_none() && lib.get_release(guest).unwrap().is_none());
    assert!(lib.get_release(other).unwrap().is_some());
    assert_eq!(nikgob_import::merge_split_releases(&lib).unwrap(), 0, "nothing left to merge");
}

#[test]
fn merging_never_joins_editions_artists_or_clashing_numbers() {
    let lib = Library::open_in_memory().unwrap();
    // Two different MusicBrainz editions next to a release without an id: none of them merge.
    let big = release_with(&lib, "Album", "A", None, &[1, 2]);
    release_with(&lib, "Album", "A", Some("mbid-1"), &[3]);
    release_with(&lib, "Album", "A", Some("mbid-2"), &[4]);
    // Explicitly different album artists with the same title.
    release_with(&lib, "Live", "A & B", None, &[1]);
    release_with(&lib, "Live", "A & C", None, &[2]);
    // The same track numbers twice: another edition.
    release_with(&lib, "Hits", "X", None, &[1, 2]);
    release_with(&lib, "Hits", "x", None, &[2]);
    assert_eq!(nikgob_import::merge_split_releases(&lib).unwrap(), 0);
    assert_eq!(lib.count_releases().unwrap(), 7);
    assert_eq!(lib.tracks_of_release(big).unwrap().len(), 2);
}

#[test]
fn merging_keeps_the_one_known_musicbrainz_id() {
    let lib = Library::open_in_memory().unwrap();
    let main = release_with(&lib, "Album", "A", None, &[1, 2]);
    release_with(&lib, "Album", "A", Some("mbid-1"), &[3]);
    assert_eq!(nikgob_import::merge_split_releases(&lib).unwrap(), 1);
    assert_eq!(lib.count_releases().unwrap(), 1);
    assert_eq!(lib.get_release(main).unwrap().unwrap().mbid.as_deref(), Some("mbid-1"));
    assert_eq!(lib.tracks_of_release(main).unwrap().len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn masters_relocate_to_another_folder_with_hash_verification_and_cleanup() {
    let root = Root::new();
    let (importer, lib) = importer(&root);
    let wav = root.inbox("song.wav");
    write_wav(&wav, &broadband(RATE, 4.0, 11), RATE);
    tags::write_tags(&wav, &song_tags("Звезда по имени Солнце")).unwrap();

    let events = importer
        .import_paths(vec![root.cfg.inbox_dir.clone()], SourceOrigin::Inbox)
        .wait()
        .await;
    assert!(failures(&events).is_empty(), "{events:?}");
    let (track_id, _) = imported(&events)[0];

    let old_masters = root.cfg.masters_dir();
    assert!(old_masters.is_dir());
    let opus_locator = lib.active_source(track_id).unwrap().unwrap().locator;
    assert!(std::path::Path::new(&opus_locator).is_file());

    let new_masters = root.dir.path().join("external_drive").join("FLAC");
    let report = nikgob_import::relocate_masters(&lib, &old_masters, &new_masters, |_, _, _| {}).unwrap();
    assert_eq!(report.audio_moved, 1);
    assert_eq!(report.sidecars_moved, 1);
    assert!(report.bytes_moved > 0);
    assert!(!old_masters.exists(), "old masters folder removed once empty");

    // Opus file stays untouched on the fast library drive.
    assert!(std::path::Path::new(&opus_locator).is_file());

    // New masters folder has both the verified lossless original and the updated sidecar.
    let mut moved_cfg = root.cfg.clone().with_masters_dir(&new_masters);
    assert_eq!(moved_cfg.masters_dir(), new_masters);
    let entries: Vec<_> = std::fs::read_dir(&new_masters)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(entries.iter().any(|n| n.ends_with(".wav")), "{entries:?}");
    assert!(entries.iter().any(|n| n.ends_with(".json")), "{entries:?}");

    // Rebuilding from files still finds the master quality score in the new folder.
    let fresh = Library::open_in_memory().unwrap();
    moved_cfg.masters_dir = new_masters;
    let rb = rebuild_from_files(&fresh, &moved_cfg).unwrap();
    assert_eq!(rb.files, 1);
    assert_eq!(fresh.active_source(track_id).unwrap().unwrap().quality_score, 900);
}


#[test]
fn editions_of_one_album_fold_into_the_fullest_one() {
    use nikgob_core::model::{ArtistCredit, Codec, Release, Source, SourceKind, Track};
    let lib = Library::open_in_memory().unwrap();
    let credit = ArtistCredit::single("Kevin Penkin", None);
    let release = |title: &str, mbid: &str| {
        let mut r = Release::new(title, credit.clone());
        r.mbid = Some(mbid.into());
        r.release_group_mbid = Some("group".into());
        lib.upsert_release(&r).unwrap().id
    };
    let track = |release, title: &str, no: u32, rec: Option<&str>, codec: Codec, score: i32| {
        let mut t = Track::new(title, credit.clone());
        t.release_id = Some(release);
        t.track_no = no;
        t.recording_mbid = rec.map(Into::into);
        let t = lib.upsert_track(&t).unwrap();
        let mut s = Source::new(t.id, SourceKind::LocalFile, format!("C:/Music/{}", t.id), codec);
        s.quality_score = score;
        lib.add_source(&s).unwrap();
        t.id
    };
    let intl = release("MADE IN ABYSS ORIGINAL SOUNDTRACK", "intl");
    let a = track(intl, "Made in Abyss", 1, Some("rec-1"), Codec::Opus, 900);
    let b = track(intl, "Days in the Sun", 4, Some("rec-4"), Codec::Opus, 900);
    track(intl, "Riko's Theme", 18, Some("rec-18"), Codec::Opus, 900);
    let jp = release("TVアニメ「メイドインアビス」オリジナルサウンドトラック", "jp");
    let a2 = track(jp, "Made in Abyss", 1, Some("rec-1"), Codec::Mp3, 700);
    // Lost its number and its recording id.
    let b2 = track(jp, "Days in the Sun", 0, None, Codec::Mp3, 700);
    // Only this edition has it.
    let extra = track(jp, "Bonus", 30, Some("rec-30"), Codec::Mp3, 700);
    lib.like(nikgob_core::model::LikeTarget::Track(a2)).unwrap();

    let found = nikgob_import::find_release_editions(&lib).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].keep.release.id, intl);
    assert_eq!((found[0].same_songs, found[0].new_songs), (2, 1));
    // Finding changes nothing.
    assert_eq!(lib.count_releases().unwrap(), 2);
    nikgob_import::fold_editions(&lib, &found[0]).unwrap();
    assert_eq!(lib.count_releases().unwrap(), 1);
    assert_eq!(lib.get_release(intl).unwrap().unwrap().title, "MADE IN ABYSS ORIGINAL SOUNDTRACK");
    let tracks = lib.tracks_of_release(intl).unwrap();
    assert_eq!(tracks.len(), 4, "the three songs plus the bonus one");
    assert!(tracks.iter().any(|t| t.id == extra));
    for (dup, kept) in [(a2, a), (b2, b)] {
        assert!(lib.get_track(dup).unwrap().is_none());
        // The better file still plays; the other one is a spare source.
        assert_eq!(lib.sources_of_track(kept).unwrap().len(), 2);
        assert_eq!(lib.active_source(kept).unwrap().unwrap().codec, Codec::Opus);
    }
    assert!(lib.get_track_row(a).unwrap().unwrap().liked, "the like moved over");
    assert!(nikgob_import::find_release_editions(&lib).unwrap().is_empty(), "nothing left");
}
