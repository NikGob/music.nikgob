//! Single-file rips: an image of a whole disc with its cue sheet becomes
//! one track per entry of the sheet, the image and the sheet its master.

mod common;

use common::*;
use nikgob_core::model::{Codec, SourceOrigin};
use nikgob_import::tags;
use nikgob_import::{ImportEvent, Importer, purge_track};
use nikgob_library::Library;

const RATE: u32 = 44_100;

/// A sheet in windows-1251, as trackers ship them. Track 2 has a pregap
/// (INDEX 00) that stays at the end of track 1.
const SHEET: &str = "REM GENRE Rock\r\n\
    REM DATE 1988\r\n\
    PERFORMER \"Кино\"\r\n\
    TITLE \"Группа крови\"\r\n\
    FILE \"CDImage.wav\" WAVE\r\n\
    \x20 TRACK 01 AUDIO\r\n\
    \x20   TITLE \"Группа крови\"\r\n\
    \x20   INDEX 01 00:00:00\r\n\
    \x20 TRACK 02 AUDIO\r\n\
    \x20   TITLE \"Закрой за мной дверь\"\r\n\
    \x20   INDEX 00 00:06:50\r\n\
    \x20   INDEX 01 00:07:00\r\n\
    \x20 TRACK 03 AUDIO\r\n\
    \x20   TITLE \"Война\"\r\n\
    \x20   INDEX 01 00:14:00\r\n";

fn importer(cfg: &nikgob_import::ImportConfig) -> (Importer, Library) {
    let lib = Library::open_in_memory().unwrap();
    (Importer::new(cfg.clone(), lib.clone(), None), lib)
}

fn imported(events: &[ImportEvent]) -> Vec<nikgob_core::model::TrackId> {
    events
        .iter()
        .filter_map(|e| match e {
            ImportEvent::Imported { track_id, .. } => Some(*track_id),
            _ => None,
        })
        .collect()
}

/// Three different 7 s songs back to back (21 s).
fn disc() -> Vec<f32> {
    let mut all = Vec::new();
    for seed in [11, 22, 33] {
        all.extend(broadband(RATE, 7.0, seed));
    }
    all
}

#[tokio::test(flavor = "multi_thread")]
async fn an_image_with_its_sheet_becomes_tracks_that_share_one_master() {
    let root = Root::new();
    let mut cfg = root.cfg.clone();
    cfg.work_dir = root.dir.path().join("work");
    let (importer, lib) = importer(&cfg);
    let album = root.inbox("Кино - Группа крови");
    std::fs::create_dir_all(&album).unwrap();
    // The sheet names `CDImage.wav`; the image is named like the sheet.
    let image = album.join("Кино - Группа крови.wav");
    write_wav(&image, &disc(), RATE);
    let image_hash = blake3::hash(&std::fs::read(&image).unwrap()).to_hex().to_string();
    let (sheet_bytes, _, _) = encoding_rs::WINDOWS_1251.encode(SHEET);
    let sheet_bytes = sheet_bytes.into_owned();
    std::fs::write(album.join("Кино - Группа крови.cue"), &sheet_bytes).unwrap();

    let events = importer
        .import_paths(vec![root.cfg.inbox_dir.clone()], SourceOrigin::Inbox)
        .wait()
        .await;
    let failed: Vec<_> = events.iter().filter(|e| matches!(e, ImportEvent::Failed { .. })).collect();
    assert!(failed.is_empty(), "{failed:?}");
    assert!(matches!(events[0], ImportEvent::Started { files: 3, .. }), "{:?}", events[0]);
    let tracks = imported(&events);
    assert_eq!(tracks.len(), 3, "{events:?}");

    // Three Opus files, named and tagged by the sheet.
    let dir = root.cfg.library_dir.join("Кино").join("1988 - Группа крови");
    let names = ["01 - Группа крови.opus", "02 - Закрой за мной дверь.opus", "03 - Война.opus"];
    let lengths = [7.0, 7.0, 7.0];
    for (name, secs) in names.iter().zip(lengths) {
        let file = dir.join(name);
        assert!(file.is_file(), "{name}: {:?}", std::fs::read_dir(&dir).map(|r| r.flatten().map(|e| e.file_name()).collect::<Vec<_>>()));
        let (t, info) = tags::read_tags(&file).unwrap();
        assert_eq!(info.codec, Codec::Opus);
        assert_eq!(t.album.as_deref(), Some("Группа крови"));
        assert_eq!(t.album_artist.as_deref(), Some("Кино"));
        assert_eq!(t.artist.as_deref(), Some("Кино"));
        assert_eq!(t.genre.as_deref(), Some("Rock"));
        assert!(t.r128_track_gain.is_some(), "per-track loudness");
        let got = info.duration.as_secs_f64();
        assert!((got - secs).abs() < 0.05, "{name}: {got} s");
    }
    for t in &tracks {
        let track = lib.get_track(*t).unwrap().unwrap();
        let src = lib.active_source(*t).unwrap().unwrap();
        assert_eq!(src.quality_score, 900, "ranked as the lossless image");
        assert_eq!(src.origin, SourceOrigin::Inbox);
        // One master for the album: the image.
        let m = lib.get_master(*t).unwrap().expect("master");
        assert_eq!(m.file_hash, image_hash, "{}", track.title);
        assert_eq!(m.codec, Codec::Wav);
    }
    let mut numbers: Vec<(String, u32)> = tracks
        .iter()
        .map(|t| lib.get_track(*t).unwrap().unwrap())
        .map(|t| (t.title, t.track_no))
        .collect();
    numbers.sort_by_key(|(_, n)| *n);
    assert_eq!(numbers[1], ("Закрой за мной дверь".to_owned(), 2));

    // The image and its sheet are kept as they came, out of the Inbox.
    let masters = root.cfg.masters_dir();
    let kept = masters.join(format!("{image_hash}.wav"));
    assert!(kept.is_file());
    assert_eq!(std::fs::read(masters.join(format!("{image_hash}.cue"))).unwrap(), sheet_bytes);
    let sidecar: serde_json::Value =
        serde_json::from_slice(&std::fs::read(masters.join(format!("{image_hash}.json"))).unwrap()).unwrap();
    assert_eq!(sidecar["track_ids"].as_array().map(Vec::len), Some(3), "{sidecar}");
    assert_eq!(sidecar["cue"].as_str(), Some("Кино - Группа крови.cue"));
    assert!(!image.exists() && !album.join("Кино - Группа крови.cue").exists());
    // The cut tracks are gone.
    assert!(std::fs::read_dir(&cfg.work_dir).map_or(true, |mut r| r.next().is_none()));

    // Deleting a track keeps the image while the others have it.
    purge_track(&lib, &cfg, tracks[0]).unwrap();
    assert!(kept.is_file(), "the other tracks still have the image");
    purge_track(&lib, &cfg, tracks[1]).unwrap();
    assert!(kept.is_file());
    purge_track(&lib, &cfg, tracks[2]).unwrap();
    assert!(!kept.exists(), "the image goes with its last track");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sheet_that_runs_past_its_file_imports_the_file_as_it_is() {
    let root = Root::new();
    let mut cfg = root.cfg.clone();
    cfg.work_dir = root.dir.path().join("work");
    let (importer, lib) = importer(&cfg);
    let album = root.inbox("Album");
    std::fs::create_dir_all(&album).unwrap();
    // 7 s of audio; the sheet's track 2 starts at 0:30.
    let image = album.join("disc.wav");
    write_wav(&image, &broadband(RATE, 7.0, 5), RATE);
    std::fs::write(
        album.join("disc.cue"),
        "FILE \"disc.wav\" WAVE\nTRACK 01 AUDIO\nTITLE \"A\"\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nTITLE \"B\"\nINDEX 01 00:30:00\n",
    )
    .unwrap();
    let events = importer
        .import_paths(vec![root.cfg.inbox_dir.clone()], SourceOrigin::Inbox)
        .wait()
        .await;
    let tracks = imported(&events);
    assert_eq!(tracks.len(), 1, "{events:?}");
    let t = lib.get_track(tracks[0]).unwrap().unwrap();
    assert!(t.duration_ms.is_some_and(|d| (6_900..7_100).contains(&d)), "the whole file: {t:?}");
    assert!(std::fs::read_dir(&cfg.work_dir).map_or(true, |mut r| r.next().is_none()));
}
