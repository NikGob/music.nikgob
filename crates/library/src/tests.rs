use std::fs;
use std::path::Path;

use nikgob_core::model::{
    Analysis, Artist, ArtistCredit, Codec, EventContext, EventType, LikeTarget, Palette,
    PlaylistId, Release, ReleaseType, Source, SourceKind, SourceOrigin, Track, TrackId,
};
use serde_json::Value;

use crate::{
    ArtistSort, ArtistSortKey, Library, LibraryError, PlaylistKind, ReleaseSort, ReleaseSortKey,
    SCHEMA_VERSION, TrackFilter, TrackSort, TrackSortKey,
};

// ------------------------------------------------------------------ helpers

fn lib() -> Library {
    Library::open_in_memory().unwrap()
}

fn artist(lib: &Library, name: &str) -> Artist {
    lib.upsert_artist(&Artist::new(name)).unwrap()
}

fn credit(a: &Artist) -> ArtistCredit {
    ArtistCredit::single(a.name.clone(), Some(a.id))
}

fn release(lib: &Library, a: &Artist, title: &str, year: Option<i32>) -> Release {
    let mut r = Release::new(title, credit(a));
    r.year = year;
    lib.upsert_release(&r).unwrap()
}

fn track(lib: &Library, a: &Artist, rel: Option<&Release>, title: &str, no: u32) -> Track {
    let mut t = Track::new(title, credit(a));
    t.release_id = rel.map(|r| r.id);
    t.track_no = no;
    t.duration_ms = Some(180_000 + no * 1000);
    lib.upsert_track(&t).unwrap()
}

fn source(t: &Track, kind: SourceKind, locator: &str, codec: Codec, kbps: Option<u32>) -> Source {
    let mut s = Source::new(t.id, kind, locator, codec);
    s.bitrate = kbps;
    s.with_quality(false)
}

fn local(lib: &Library, t: &Track, locator: &str, codec: Codec, kbps: Option<u32>) -> Source {
    lib.add_source(&source(t, SourceKind::LocalFile, locator, codec, kbps))
        .unwrap()
}

fn titles(rows: &[crate::TrackRow]) -> Vec<&str> {
    rows.iter().map(|r| r.title.as_str()).collect()
}

fn entry_tracks(lib: &Library, p: PlaylistId) -> Vec<String> {
    lib.playlist_entries(p, 0, 10_000)
        .unwrap()
        .into_iter()
        .map(|e| e.track.title)
        .collect()
}

fn rewrite_json(path: &Path, f: impl FnOnce(&mut Value)) {
    let mut v: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    f(&mut v);
    fs::write(path, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
}

// --------------------------------------------------------------- migrations

#[test]
fn migrates_from_empty() {
    let lib = lib();
    assert_eq!(lib.schema_version().unwrap(), SCHEMA_VERSION);
    let tables: Vec<String> = lib
        .read(|c| {
            let mut stmt = c.prepare("SELECT name FROM sqlite_master WHERE type = 'table'")?;
            let names = stmt.query_map([], |r| r.get(0))?;
            Ok(names.collect::<rusqlite::Result<Vec<String>>>()?)
        })
        .unwrap();
    for want in [
        "artist",
        "release",
        "track",
        "source",
        "master",
        "replica",
        "node",
        "wishlist",
        "download",
        "analysis",
        "palette",
        "playlist",
        "playlist_entry",
        "likes",
        "event",
        "track_fts",
        "release_fts",
        "artist_fts",
        "playlist_fts",
        "meta",
    ] {
        assert!(tables.iter().any(|t| t == want), "missing table {want}");
    }
}

#[test]
fn file_db_reopens_and_refuses_newer_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sub/library.db");
    let id;
    {
        let lib = Library::open(&path).unwrap();
        let a = artist(&lib, "Кино");
        id = a.id;
        // The reader pool sees committed data.
        assert_eq!(lib.get_artist(id).unwrap().unwrap().name, "Кино");
    }
    let lib = Library::open(&path).unwrap();
    assert_eq!(lib.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(lib.get_artist(id).unwrap().unwrap().name, "Кино");
    let device = lib.device_id();
    drop(lib);
    assert_eq!(
        Library::open(&path).unwrap().device_id(),
        device,
        "device id persists"
    );

    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", SCHEMA_VERSION + 5)
            .unwrap();
    }
    assert!(matches!(
        Library::open(&path),
        Err(LibraryError::SchemaTooNew { .. })
    ));
}

#[test]
fn readers_do_not_wait_for_an_open_write_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(dir.path().join("library.db")).unwrap();
    let a = artist(&lib, "Кино");
    let writer = lib.clone();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let handle = std::thread::spawn(move || {
        writer
            .write(|tx| {
                tx.execute("UPDATE artist SET name = 'Changed'", [])?;
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            })
            .unwrap();
    });
    started_rx.recv().unwrap();
    // The write transaction is open and holding the writer lock; reads still work
    // and see the last committed state.
    assert_eq!(lib.get_artist(a.id).unwrap().unwrap().name, "Кино");
    assert_eq!(lib.count_artists().unwrap(), 1);
    release_tx.send(()).unwrap();
    handle.join().unwrap();
    assert_eq!(lib.get_artist(a.id).unwrap().unwrap().name, "Changed");
}

#[test]
fn concurrent_reads_and_writes() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(dir.path().join("library.db")).unwrap();
    let a = artist(&lib, "A");
    let mut threads = Vec::new();
    for w in 0..2 {
        let lib = lib.clone();
        let a = a.clone();
        threads.push(std::thread::spawn(move || {
            for i in 0..50 {
                track(&lib, &a, None, &format!("w{w}-{i}"), i);
            }
        }));
    }
    for _ in 0..6 {
        let lib = lib.clone();
        threads.push(std::thread::spawn(move || {
            for _ in 0..50 {
                lib.tracks(0, 20, TrackSort::asc(TrackSortKey::Title))
                    .unwrap();
                lib.count_tracks(TrackFilter::All).unwrap();
            }
        }));
    }
    for t in threads {
        t.join().unwrap();
    }
    assert_eq!(lib.count_tracks(TrackFilter::All).unwrap(), 100);
}

// ----------------------------------------------------------------- upserts

#[test]
fn upserts_are_idempotent() {
    let lib = lib();
    let a = artist(&lib, "Кино");
    let again = lib.upsert_artist(&a).unwrap();
    assert_eq!(a, again);
    assert_eq!(lib.count_artists().unwrap(), 1);

    let r = release(&lib, &a, "Группа крови", Some(1988));
    let r2 = lib.upsert_release(&r).unwrap();
    assert_eq!(r, r2);
    // Stored once; listed only once it has a track.
    assert!(lib.get_release(r.id).unwrap().is_some());
    assert_eq!(lib.count_releases().unwrap(), 0);

    let mut t = Track::new("Группа крови", credit(&a));
    t.release_id = Some(r.id);
    t.track_no = 1;
    let t1 = lib.upsert_track(&t).unwrap();
    let t2 = lib.upsert_track(&t).unwrap();
    assert_eq!(t1, t2);
    assert_eq!(lib.count_releases().unwrap(), 1);
    assert_eq!(lib.count_tracks(TrackFilter::All).unwrap(), 1);

    // Changing a field updates in place and keeps `added_at`.
    let mut changed = t1.clone();
    changed.title = "Группа крови (live)".into();
    let t3 = lib.upsert_track(&changed).unwrap();
    assert_eq!(t3.id, t1.id);
    assert_eq!(t3.title, "Группа крови (live)");
    assert_eq!(t3.added_at, t1.added_at);
    assert_eq!(lib.count_tracks(TrackFilter::All).unwrap(), 1);
}

#[test]
fn mbid_matching_returns_the_existing_row() {
    let lib = lib();
    let mut a = Artist::new("Kino");
    a.mbid = Some("mb-artist".into());
    let first = lib.upsert_artist(&a).unwrap();

    let mut other = Artist::new("Кино");
    other.mbid = Some("mb-artist".into());
    let second = lib.upsert_artist(&other).unwrap();
    assert_eq!(second.id, first.id);
    assert_eq!(second.name, "Кино", "fields are refreshed");
    assert_eq!(lib.count_artists().unwrap(), 1);

    // A `None` mbid never erases a known one.
    let mut no_mbid = second.clone();
    no_mbid.mbid = None;
    assert_eq!(
        lib.upsert_artist(&no_mbid).unwrap().mbid.as_deref(),
        Some("mb-artist")
    );

    let mut r = Release::new("Album", credit(&second));
    r.mbid = Some("mb-release".into());
    let r1 = lib.upsert_release(&r).unwrap();
    let mut r_dup = Release::new("Album (remaster)", credit(&second));
    r_dup.mbid = Some("mb-release".into());
    let r2 = lib.upsert_release(&r_dup).unwrap();
    assert_eq!(r1.id, r2.id);
    assert_eq!(r2.title, "Album (remaster)");

    // Same recording on the same release matches; on another release it is a new track.
    let mut t = Track::new("Song", credit(&second));
    t.recording_mbid = Some("mb-rec".into());
    t.release_id = Some(r1.id);
    let t1 = lib.upsert_track(&t).unwrap();
    let mut t_dup = Track::new("Song", credit(&second));
    t_dup.recording_mbid = Some("mb-rec".into());
    t_dup.release_id = Some(r1.id);
    assert_eq!(lib.upsert_track(&t_dup).unwrap().id, t1.id);
    let mut comp = Track::new("Song", credit(&second));
    comp.recording_mbid = Some("mb-rec".into());
    let on_comp = lib.upsert_track(&comp).unwrap();
    assert_ne!(on_comp.id, t1.id);
    assert_eq!(lib.tracks_by_recording_mbid("mb-rec").unwrap().len(), 2);
}

#[test]
fn caller_provided_track_id_is_kept() {
    let lib = lib();
    let a = artist(&lib, "A");
    let wanted = TrackId::new();
    let mut t = Track::new("From tags", credit(&a));
    t.id = wanted;
    assert_eq!(lib.upsert_track(&t).unwrap().id, wanted);
}

#[test]
fn track_needs_an_existing_release() {
    let lib = lib();
    let a = artist(&lib, "A");
    let mut t = Track::new("Orphan", credit(&a));
    t.release_id = Some(nikgob_core::model::ReleaseId::new());
    assert!(lib.upsert_track(&t).is_err());
}

// ------------------------------------------------------------ active source

#[test]
fn active_source_prefers_local_then_quality() {
    let lib = lib();
    let a = artist(&lib, "A");
    let t = track(&lib, &a, None, "Song", 1);
    assert_eq!(lib.set_active_source(t.id).unwrap(), None);
    assert!(!lib.get_track_row(t.id).unwrap().unwrap().has_local);

    // A YouTube stream only: playable, but not local.
    let yt = lib
        .add_source(&source(
            &t,
            SourceKind::Youtube,
            "yt123",
            Codec::Opus,
            Some(160),
        ))
        .unwrap();
    assert!(yt.is_active);
    let row = lib.get_track_row(t.id).unwrap().unwrap();
    assert_eq!(row.active_kind, Some(SourceKind::Youtube));
    assert!(!row.has_local);

    // Spotify beats YouTube at any quality.
    let sp = lib
        .add_source(&source(
            &t,
            SourceKind::Spotify,
            "spotify:track:1",
            Codec::Vorbis,
            Some(96),
        ))
        .unwrap();
    assert_eq!(lib.active_source(t.id).unwrap().unwrap().id, sp.id);

    // A mediocre local file beats both streams.
    let mp3 = local(&lib, &t, "a/128.mp3", Codec::Mp3, Some(128));
    let active = lib.active_source(t.id).unwrap().unwrap();
    assert_eq!(active.id, mp3.id);
    assert!(lib.get_track_row(t.id).unwrap().unwrap().has_local);

    // Among local files the higher quality wins.
    let opus = local(&lib, &t, "a/160.opus", Codec::Opus, Some(160));
    assert_eq!(lib.active_source(t.id).unwrap().unwrap().id, opus.id);
    let flac = lib
        .add_source(&{
            let mut s = Source::new(t.id, SourceKind::LocalFile, "a/hires.flac", Codec::Flac);
            s.sample_rate = Some(96_000);
            s.bit_depth = Some(24);
            s.with_quality(false)
        })
        .unwrap();
    assert_eq!(lib.active_source(t.id).unwrap().unwrap().id, flac.id);
    let row = lib.get_track_row(t.id).unwrap().unwrap();
    assert_eq!(row.codec, Some(Codec::Flac));
    assert_eq!(row.quality, Some(flac.quality_score));

    // Fake FLAC loses to Opus.
    let fake = lib
        .add_source(
            &Source::new(t.id, SourceKind::LocalFile, "a/fake.flac", Codec::Flac)
                .with_quality(true),
        )
        .unwrap();
    assert!(fake.quality_score < opus.quality_score);

    // Exactly one active source, ordered first in the listing.
    let all = lib.sources_of_track(t.id).unwrap();
    assert_eq!(all.len(), 6);
    assert_eq!(all.iter().filter(|s| s.is_active).count(), 1);
    assert_eq!(all[0].id, flac.id);

    // Unavailable sources are skipped; the next best takes over.
    lib.set_source_available(flac.id, false).unwrap();
    assert_eq!(lib.active_source(t.id).unwrap().unwrap().id, opus.id);
    lib.set_source_available(flac.id, true).unwrap();
    assert_eq!(lib.active_source(t.id).unwrap().unwrap().id, flac.id);
}

#[test]
fn add_source_is_idempotent_on_locator() {
    let lib = lib();
    let a = artist(&lib, "A");
    let t = track(&lib, &a, None, "Song", 1);
    let mut s = source(
        &t,
        SourceKind::LocalFile,
        "Music/a.opus",
        Codec::Opus,
        Some(160),
    );
    s.origin = SourceOrigin::Torrent {
        info_hash: "abc".into(),
        path_in_torrent: "01.flac".into(),
    };
    s.file_hash = Some("hash1".into());
    let first = lib.add_source(&s).unwrap();
    // Same file seen again under a fresh id: same row, refreshed.
    let mut again = source(
        &t,
        SourceKind::LocalFile,
        "Music/a.opus",
        Codec::Opus,
        Some(192),
    );
    again.origin = s.origin.clone();
    let second = lib.add_source(&again).unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(second.bitrate, Some(192));
    assert_eq!(
        second.file_hash.as_deref(),
        Some("hash1"),
        "known hash is kept"
    );
    assert_eq!(second.origin, s.origin);
    assert_eq!(lib.sources_of_track(t.id).unwrap().len(), 1);
    assert_eq!(lib.sources_by_hash("hash1").unwrap().len(), 1);
}

// ---------------------------------------------------------------- playlists

#[test]
fn many_inserts_at_the_same_spot_stay_ordered() {
    let lib = lib();
    let a = artist(&lib, "A");
    let p = lib.create_playlist("Mix").unwrap();
    let tracks: Vec<Track> = (0..120)
        .map(|i| track(&lib, &a, None, &format!("t{i:03}"), i))
        .collect();

    // First two tracks, then always squeeze into slot 1.
    lib.playlist_add(p.id, tracks[0].id).unwrap();
    lib.playlist_add(p.id, tracks[1].id).unwrap();
    for t in &tracks[2..] {
        lib.playlist_insert(p.id, t.id, 1).unwrap();
    }
    let got = entry_tracks(&lib, p.id);
    let mut want: Vec<String> = vec!["t000".into()];
    want.extend((2..120).rev().map(|i| format!("t{i:03}")));
    want.push("t001".into());
    assert_eq!(got, want);

    // Positions are unique and strictly increasing.
    let entries = lib.playlist_entries(p.id, 0, 1000).unwrap();
    assert!(
        entries
            .windows(2)
            .all(|w| w[0].entry.position < w[1].entry.position)
    );

    // Insert in the middle, at the front and clamp past the end.
    let extra: Vec<Track> = (120..123)
        .map(|i| track(&lib, &a, None, &format!("t{i:03}"), i))
        .collect();
    let mid = lib.playlist_insert(p.id, extra[0].id, 60).unwrap();
    let first = lib.playlist_insert(p.id, extra[1].id, 0).unwrap();
    let last = lib.playlist_insert(p.id, extra[2].id, 10_000).unwrap();
    let ids: Vec<_> = lib
        .playlist_entries(p.id, 0, 1000)
        .unwrap()
        .into_iter()
        .map(|e| e.entry.id)
        .collect();
    assert_eq!(ids[0], first.id);
    assert_eq!(ids[60 + 1], mid.id);
    assert_eq!(*ids.last().unwrap(), last.id);
    assert_eq!(lib.playlist_len(p.id).unwrap(), 123);
}

#[test]
fn move_and_remove_entries() {
    let lib = lib();
    let a = artist(&lib, "A");
    let p = lib.create_playlist("Mix").unwrap();
    let names = ["a", "b", "c", "d", "e"];
    let entries: Vec<_> = names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let t = track(&lib, &a, None, n, i as u32);
            lib.playlist_add(p.id, t.id).unwrap()
        })
        .collect();
    assert_eq!(entry_tracks(&lib, p.id), ["a", "b", "c", "d", "e"]);

    // Move e to the front, a to the end, c to slot 2.
    lib.playlist_move(entries[4].id, 0).unwrap();
    assert_eq!(entry_tracks(&lib, p.id), ["e", "a", "b", "c", "d"]);
    lib.playlist_move(entries[0].id, 99).unwrap();
    assert_eq!(entry_tracks(&lib, p.id), ["e", "b", "c", "d", "a"]);
    lib.playlist_move(entries[2].id, 2).unwrap();
    assert_eq!(entry_tracks(&lib, p.id), ["e", "b", "c", "d", "a"]);
    lib.playlist_move(entries[2].id, 3).unwrap();
    assert_eq!(entry_tracks(&lib, p.id), ["e", "b", "d", "c", "a"]);

    // Removal is a tombstone: hidden but still in the table.
    lib.playlist_remove(entries[1].id).unwrap();
    assert_eq!(entry_tracks(&lib, p.id), ["e", "d", "c", "a"]);
    assert!(
        lib.playlist_remove(entries[1].id).is_err(),
        "already removed"
    );
    let dead: i64 = lib
        .read(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM playlist_entry WHERE deleted = 1",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(dead, 1);

    // Re-adding a track already in the playlist is a no-op returning its entry.
    let t = track(&lib, &a, None, "a", 9);
    let first = lib.playlist_add(p.id, t.id).unwrap();
    let again = lib.playlist_add(p.id, t.id).unwrap();
    assert_eq!(first.id, again.id);
    let many = lib.playlist_add_many(p.id, &[t.id, t.id]).unwrap();
    assert!(many.iter().all(|e| e.id == first.id));
    assert_eq!(lib.playlist_len(p.id).unwrap(), 5);

    // A removed track can be added back.
    lib.playlist_remove(first.id).unwrap();
    assert_ne!(lib.playlist_add(p.id, t.id).unwrap().id, first.id);
    assert_eq!(lib.playlist_len(p.id).unwrap(), 5);
}

#[test]
fn equal_positions_get_rebalanced() {
    // Two devices inserting into the same gap produce equal positions; a later
    // insert between them must still work and keep the order.
    let lib = lib();
    let a = artist(&lib, "A");
    let p = lib.create_playlist("Mix").unwrap();
    let t1 = track(&lib, &a, None, "x", 1);
    let t2 = track(&lib, &a, None, "y", 2);
    let t3 = track(&lib, &a, None, "z", 3);
    let e1 = lib.playlist_add(p.id, t1.id).unwrap();
    let e2 = lib.playlist_add(p.id, t2.id).unwrap();
    lib.write(|tx| {
        tx.execute(
            "UPDATE playlist_entry SET position = ?1",
            [e1.position.clone()],
        )?;
        Ok(())
    })
    .unwrap();
    // Order falls back to the entry id (v7: e1 was created first).
    assert!(e1.id < e2.id);
    assert_eq!(entry_tracks(&lib, p.id), ["x", "y"]);
    let mid = lib.playlist_insert(p.id, t3.id, 1).unwrap();
    assert_eq!(entry_tracks(&lib, p.id), ["x", "z", "y"]);
    let entries = lib.playlist_entries(p.id, 0, 10).unwrap();
    assert!(
        entries
            .windows(2)
            .all(|w| w[0].entry.position < w[1].entry.position)
    );
    assert_eq!(entries[1].entry.id, mid.id);
}

#[test]
fn playlist_lifecycle_and_virtual_liked() {
    let lib = lib();
    let a = artist(&lib, "A");
    let t1 = track(&lib, &a, None, "one", 1);
    let t2 = track(&lib, &a, None, "two", 2);

    let list = lib.list_playlists().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].kind, PlaylistKind::Liked);
    assert_eq!(list[0].name, "Любимые");
    assert_eq!(list[0].id, None);

    let p = lib.create_playlist("Дорога").unwrap();
    lib.playlist_add(p.id, t1.id).unwrap();
    lib.rename_playlist(p.id, "В дороге").unwrap();
    lib.set_playlist_cover(p.id, Some("covers/p.jpg")).unwrap();
    let got = lib.get_playlist(p.id).unwrap().unwrap();
    assert_eq!(got.name, "В дороге");
    assert_eq!(got.cover.as_deref(), Some("covers/p.jpg"));
    lib.set_playlist_cover(p.id, None).unwrap();
    assert_eq!(lib.get_playlist(p.id).unwrap().unwrap().cover, None);

    lib.like(t2.id).unwrap();
    lib.like(t1.id).unwrap();
    let list = lib.list_playlists().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(
        list[0].track_count, 2,
        "liked virtual playlist counts liked tracks"
    );
    assert_eq!(list[1].name, "В дороге");
    assert_eq!(list[1].track_count, 1);

    // Likes: LWW set semantics.
    assert!(lib.is_liked(t1.id).unwrap());
    assert!(!lib.is_liked(LikeTarget::Artist(a.id)).unwrap());
    let liked = lib.liked_tracks(0, 10).unwrap();
    assert_eq!(titles(&liked), ["one", "two"], "most recently liked first");
    assert!(liked.iter().all(|r| r.liked));
    lib.unlike(t1.id).unwrap();
    assert!(!lib.is_liked(t1.id).unwrap());
    assert_eq!(titles(&lib.liked_tracks(0, 10).unwrap()), ["two"]);
    lib.like(t1.id).unwrap();
    assert!(lib.is_liked(t1.id).unwrap());
    lib.like(a.id).unwrap();
    assert_eq!(lib.liked_artists(0, 10).unwrap().len(), 1);

    lib.delete_playlist(p.id).unwrap();
    assert!(lib.get_playlist(p.id).unwrap().is_none());
    assert_eq!(lib.list_playlists().unwrap().len(), 1);
    assert!(lib.playlist_add(p.id, t1.id).is_err());
}

#[test]
fn editing_entries_does_not_bump_the_playlist_row() {
    let lib = lib();
    let a = artist(&lib, "A");
    let t = track(&lib, &a, None, "t", 1);
    let p = lib.create_playlist("P").unwrap();
    lib.playlist_add(p.id, t.id).unwrap();
    let after = lib.get_playlist(p.id).unwrap().unwrap();
    assert_eq!(after.updated_at, p.updated_at);
}

// ------------------------------------------------------------------- search

#[test]
fn search_cyrillic_case_and_prefix() {
    let lib = lib();
    let kino = artist(&lib, "Кино");
    let rel = release(&lib, &kino, "Звезда по имени Солнце", Some(1989));
    track(&lib, &kino, Some(&rel), "Звезда по имени Солнце", 1);
    track(&lib, &kino, Some(&rel), "Звёздочка", 2);
    track(&lib, &kino, Some(&rel), "Группа крови", 3);
    let other = artist(&lib, "Аквариум");
    track(&lib, &other, None, "Город золотой", 1);

    // Case-insensitive, whole word.
    for q in ["Кино", "кино", "КИНО", " кино "] {
        let r = lib.search(q, 10).unwrap();
        assert_eq!(r.artists.len(), 1, "{q}");
        assert_eq!(r.artists[0].artist.name, "Кино");
        assert_eq!(r.tracks.len(), 3, "tracks credited to Кино match on artist");
        assert_eq!(r.releases.len(), 1);
    }

    // Prefix with ё typed: finds both ё and е spellings.
    let r = lib.search("звёзд", 10).unwrap();
    // (the third track matches through its album title "Звезда по имени Солнце")
    assert_eq!(
        {
            let mut t = titles(&r.tracks);
            t.sort();
            t
        },
        ["Группа крови", "Звезда по имени Солнце", "Звёздочка"]
    );
    assert_eq!(r.releases.len(), 1);
    // ... and typing е finds the ё spelling too.
    let r = lib.search("звезд", 10).unwrap();
    assert_eq!(r.tracks.len(), 3);
    let r = lib.search("ЗВЁЗДОЧ", 10).unwrap();
    assert_eq!(titles(&r.tracks), ["Звёздочка"]);

    // All words must match, each as a prefix; word order is free.
    let r = lib.search("гр кро", 10).unwrap();
    assert_eq!(titles(&r.tracks), ["Группа крови"]);
    let r = lib.search("кро гр", 10).unwrap();
    assert_eq!(titles(&r.tracks), ["Группа крови"]);
    assert!(lib.search("группа аквариум", 10).unwrap().tracks.is_empty());

    // Album title is searchable from the track side.
    let r = lib.search("имени солнце", 10).unwrap();
    assert_eq!(r.tracks.len(), 3);

    // Nothing searchable, or syntax-looking input, never errors.
    assert!(lib.search("", 10).unwrap().is_empty());
    assert!(lib.search("  ...  ", 10).unwrap().is_empty());
    for q in [
        "\"",
        "кино\"",
        "AND",
        "a OR",
        "(",
        "кино*",
        "-кино",
        "NEAR(",
        "col:кино",
    ] {
        lib.search(q, 10).unwrap();
    }
    assert!(lib.search("zzz", 10).unwrap().is_empty());
}

#[test]
fn search_latin_diacritics_and_й() {
    let lib = lib();
    let a = artist(&lib, "Beyoncé");
    track(&lib, &a, None, "Halo", 1);
    track(&lib, &a, None, "Йога", 2);
    assert_eq!(lib.search("beyonce", 10).unwrap().artists.len(), 1);
    assert_eq!(lib.search("BEYONCÉ", 10).unwrap().artists.len(), 1);
    assert_eq!(lib.search("йо", 10).unwrap().tracks.len(), 1);
    // Documented behaviour: only Latin diacritics are removed by the tokenizer;
    // Cyrillic й stays distinct from и, ё is handled by the explicit folding.
    assert!(lib.search("иога", 10).unwrap().tracks.is_empty());
}

#[test]
fn search_follows_updates_and_playlists() {
    let lib = lib();
    let a = artist(&lib, "Земфира");
    let rel = release(&lib, &a, "Прости меня моя любовь", Some(2000));
    let t = track(&lib, &a, Some(&rel), "Ариведерчи", 1);
    assert_eq!(lib.search("ариве", 10).unwrap().tracks.len(), 1);

    // Retitling a track moves it in the index.
    let mut changed = t.clone();
    changed.title = "Хочешь".into();
    lib.upsert_track(&changed).unwrap();
    assert!(lib.search("ариве", 10).unwrap().tracks.is_empty());
    assert_eq!(lib.search("хоче", 10).unwrap().tracks.len(), 1);

    // Retitling the release updates the album field of its tracks.
    assert_eq!(lib.search("прости", 10).unwrap().tracks.len(), 1);
    let mut r2 = rel.clone();
    r2.title = "Спасибо".into();
    lib.upsert_release(&r2).unwrap();
    assert!(lib.search("прости", 10).unwrap().tracks.is_empty());
    assert!(lib.search("прости", 10).unwrap().releases.is_empty());
    assert_eq!(lib.search("спасибо", 10).unwrap().tracks.len(), 1);
    assert_eq!(lib.search("спасибо", 10).unwrap().releases.len(), 1);

    // Playlists: found by name, dropped after delete or rename.
    let p = lib.create_playlist("Для бега").unwrap();
    assert_eq!(lib.search("бега", 10).unwrap().playlists.len(), 1);
    lib.rename_playlist(p.id, "Для сна").unwrap();
    assert!(lib.search("бега", 10).unwrap().playlists.is_empty());
    assert_eq!(lib.search("сна", 10).unwrap().playlists.len(), 1);
    lib.delete_playlist(p.id).unwrap();
    assert!(lib.search("сна", 10).unwrap().playlists.is_empty());

    // Rebuilding from the base tables gives the same answers.
    lib.rebuild_search_index().unwrap();
    assert_eq!(lib.search("хоче", 10).unwrap().tracks.len(), 1);
    assert_eq!(lib.search("спасибо", 10).unwrap().tracks.len(), 1);
}

#[test]
fn search_limit_applies_per_category() {
    let lib = lib();
    let a = artist(&lib, "Band");
    for i in 0..10 {
        track(&lib, &a, None, &format!("Song {i}"), i);
    }
    let r = lib.search("song", 4).unwrap();
    assert_eq!(r.tracks.len(), 4);
}

// -------------------------------------------------------------- paging/sort

#[test]
fn track_paging_and_sorting() {
    let lib = lib();
    let ab = artist(&lib, "Beta");
    let aa = artist(&lib, "alpha");
    let r1 = release(&lib, &ab, "Zebra", Some(2001));
    let r2 = release(&lib, &aa, "Apple", Some(1999));
    // 25 tracks: 10 on Zebra (Beta), 10 on Apple (alpha), 5 singles by Beta.
    for i in 0..10 {
        track(&lib, &ab, Some(&r1), &format!("z{i:02}"), i + 1);
        track(&lib, &aa, Some(&r2), &format!("a{i:02}"), i + 1);
    }
    for i in 0..5 {
        track(&lib, &ab, None, &format!("s{i:02}"), 0);
    }
    assert_eq!(lib.count_tracks(TrackFilter::All).unwrap(), 25);

    let page = |off, lim, sort| lib.tracks(off, lim, sort).unwrap();
    let by_title = TrackSort::asc(TrackSortKey::Title);
    let all = page(0, 100, by_title);
    assert_eq!(all.len(), 25);
    assert!(all.windows(2).all(|w| w[0].title <= w[1].title));

    // Pages stitch together exactly.
    let mut stitched = Vec::new();
    for off in (0..25).step_by(10) {
        stitched.extend(page(off, 10, by_title));
    }
    assert_eq!(stitched, all);
    assert!(page(25, 10, by_title).is_empty());

    let desc = page(0, 100, TrackSort::desc(TrackSortKey::Title));
    let mut rev = all.clone();
    rev.reverse();
    assert_eq!(desc, rev);

    // By artist: case-insensitive ("alpha" before "Beta"), then album order.
    let by_artist = page(0, 100, TrackSort::asc(TrackSortKey::Artist));
    assert_eq!(by_artist[0].artist, "alpha");
    assert_eq!(by_artist[0].title, "a00");
    assert_eq!(by_artist[9].title, "a09");
    assert_eq!(by_artist[10].artist, "Beta");

    // By album: no-album tracks (empty key) first, then Apple, then Zebra.
    let by_album = page(0, 100, TrackSort::asc(TrackSortKey::Album));
    assert_eq!(by_album[0].album, None);
    assert_eq!(by_album[5].album.as_deref(), Some("Apple"));
    assert_eq!(by_album[15].album.as_deref(), Some("Zebra"));
    assert_eq!(by_album[5].track_no, 1);
    assert_eq!(by_album[14].track_no, 10);

    let by_added = page(0, 3, TrackSort::asc(TrackSortKey::Added));
    assert_eq!(by_added.len(), 3);

    // Rows carry what the list needs.
    let row = all.iter().find(|r| r.title == "z03").unwrap();
    assert_eq!(row.artist, "Beta");
    assert_eq!(row.album.as_deref(), Some("Zebra"));
    assert_eq!(row.track_no, 4);
    assert_eq!(row.duration_ms, Some(184_000));
    assert!(!row.has_local && row.quality.is_none());

    // Local-only filter follows the active source.
    let t = lib
        .tracks_filtered(0, 100, by_title, TrackFilter::All)
        .unwrap()
        .into_iter()
        .find(|r| r.title == "s00")
        .unwrap();
    let tr = lib.get_track(t.id).unwrap().unwrap();
    local(&lib, &tr, "s00.opus", Codec::Opus, Some(160));
    assert_eq!(lib.count_tracks(TrackFilter::LocalOnly).unwrap(), 1);
    assert_eq!(lib.count_tracks(TrackFilter::NoLocalFile).unwrap(), 24);
    let local_rows = lib
        .tracks_filtered(0, 100, by_title, TrackFilter::LocalOnly)
        .unwrap();
    assert_eq!(titles(&local_rows), ["s00"]);
    assert!(local_rows[0].has_local);
    assert_eq!(local_rows[0].codec, Some(Codec::Opus));
    let by_quality = page(0, 1, TrackSort::desc(TrackSortKey::Quality));
    assert_eq!(by_quality[0].title, "s00");
}

#[test]
fn release_and_artist_paging_with_local_counts() {
    let lib = lib();
    let beatles = artist(&lib, "The Beatles");
    let kino = artist(&lib, "Кино");
    let zero = artist(&lib, "Zero");
    let mut abbey = Release::new("Abbey Road", credit(&beatles));
    abbey.year = Some(1969);
    abbey.total_tracks = Some(17);
    let abbey = lib.upsert_release(&abbey).unwrap();
    let blood = release(&lib, &kino, "Группа крови", Some(1988));
    let undated = release(&lib, &zero, "Undated", None);
    let early = release(&lib, &zero, "Early", Some(1960));
    // Albums are listed once they have a track (stream-only ones too).
    track(&lib, &zero, Some(&undated), "u1", 1);
    track(&lib, &zero, Some(&early), "e1", 1);

    // 3 of 17 locally for Abbey Road; one more track known but not local.
    for i in 1..=4 {
        let t = track(&lib, &beatles, Some(&abbey), &format!("ab{i}"), i);
        if i <= 3 {
            local(&lib, &t, &format!("ab{i}.opus"), Codec::Opus, Some(160));
        }
    }
    let p = lib.release_progress(abbey.id).unwrap().unwrap();
    assert_eq!((p.local, p.total), (3, 17));
    assert!(!p.is_complete());

    // No declared total: known track count is the total.
    let t = track(&lib, &kino, Some(&blood), "b1", 1);
    local(&lib, &t, "b1.opus", Codec::Opus, Some(160));
    track(&lib, &kino, Some(&blood), "b2", 2);
    let p = lib.release_progress(blood.id).unwrap().unwrap();
    assert_eq!((p.local, p.total), (1, 2));
    assert!(lib.release_progress(undated.id).unwrap().unwrap().total == 1);

    let by_year = lib
        .releases(0, 10, ReleaseSort::asc(ReleaseSortKey::Year))
        .unwrap();
    let order: Vec<_> = by_year.iter().map(|r| r.release.title.as_str()).collect();
    assert_eq!(order, ["Early", "Abbey Road", "Группа крови", "Undated"]);
    let by_year_desc = lib
        .releases(0, 10, ReleaseSort::desc(ReleaseSortKey::Year))
        .unwrap();
    let order: Vec<_> = by_year_desc
        .iter()
        .map(|r| r.release.title.as_str())
        .collect();
    assert_eq!(
        order,
        ["Группа крови", "Abbey Road", "Early", "Undated"],
        "unknown year stays last"
    );
    let by_title = lib
        .releases(0, 10, ReleaseSort::asc(ReleaseSortKey::Title))
        .unwrap();
    assert_eq!(by_title[0].release.title, "Abbey Road");
    assert_eq!(by_title[0].local_tracks, 3);
    assert_eq!(by_title[0].known_tracks, 4);
    assert_eq!(by_title[0].progress().total, 17);
    let by_artist = lib
        .releases(0, 2, ReleaseSort::asc(ReleaseSortKey::Artist))
        .unwrap();
    assert_eq!(
        by_artist[0].release.title, "Abbey Road",
        "The Beatles < Zero < Кино"
    );
    assert_eq!(
        lib.releases(2, 10, ReleaseSort::asc(ReleaseSortKey::Artist))
            .unwrap()
            .len(),
        2
    );
    assert_eq!(lib.count_releases().unwrap(), 4);

    let artists = lib
        .artists(0, 10, ArtistSort::asc(ArtistSortKey::Name))
        .unwrap();
    let names: Vec<_> = artists.iter().map(|a| a.artist.name.as_str()).collect();
    assert_eq!(names, ["The Beatles", "Zero", "Кино"]);
    assert_eq!(artists[0].release_count, 1);
    assert_eq!(artists[0].local_track_count, 3);
    assert_eq!(artists[1].release_count, 2);
    assert_eq!(artists[2].local_track_count, 1);
    assert_eq!(
        lib.artists(1, 1, ArtistSort::asc(ArtistSortKey::Name))
            .unwrap()[0]
            .artist
            .name,
        "Zero"
    );

    // Artist page helpers.
    let rels = lib.releases_of_artist(zero.id).unwrap();
    let order: Vec<_> = rels.iter().map(|r| r.release.title.as_str()).collect();
    assert_eq!(order, ["Early", "Undated"]);
    assert_eq!(lib.tracks_of_artist(beatles.id, 0, 10).unwrap().len(), 4);
    let rows = lib.tracks_of_release(abbey.id).unwrap();
    assert_eq!(titles(&rows), ["ab1", "ab2", "ab3", "ab4"]);
    assert_eq!(lib.count_artists().unwrap(), 3);
}

#[test]
fn featured_artists_show_up_in_credit_text_and_links() {
    let lib = lib();
    let main = artist(&lib, "Main");
    let guest = artist(&lib, "Guest");
    let mut c = credit(&main);
    c.push(" feat. ", guest.name.clone(), Some(guest.id));
    let mut t = Track::new("Duet", c);
    t.track_no = 1;
    let t = lib.upsert_track(&t).unwrap();
    let row = lib.get_track_row(t.id).unwrap().unwrap();
    assert_eq!(row.artist, "Main feat. Guest");
    assert_eq!(lib.tracks_of_artist(guest.id, 0, 10).unwrap().len(), 1);
    assert_eq!(lib.tracks_of_artist(main.id, 0, 10).unwrap().len(), 1);
    assert_eq!(lib.search("guest", 10).unwrap().tracks.len(), 1);
    assert_eq!(
        lib.get_track(t.id).unwrap().unwrap().artist_credit.0.len(),
        2
    );
}

// ------------------------------------------------------------------- events

#[test]
fn events_and_stats() {
    let lib = lib();
    let a = artist(&lib, "A");
    let t1 = track(&lib, &a, None, "one", 1);
    let t2 = track(&lib, &a, None, "two", 2);
    let rec = |t: &Track, ty| {
        lib.record_event(
            t.id,
            ty,
            Some(SourceKind::LocalFile),
            Some(1000),
            EventContext::Album,
        )
        .unwrap()
    };
    rec(&t1, EventType::Start);
    rec(&t1, EventType::Skip);
    rec(&t1, EventType::Start);
    let last = rec(&t1, EventType::Complete);
    rec(&t2, EventType::Start);
    rec(&t2, EventType::Seek);

    let s = lib.track_stats(t1.id).unwrap();
    assert_eq!((s.plays, s.completes, s.skips), (2, 1, 1));
    assert_eq!(s.last_played, Some(last.ts));
    let s2 = lib.track_stats(t2.id).unwrap();
    assert_eq!((s2.plays, s2.completes, s2.skips), (1, 0, 0));
    let none = lib.track_stats(TrackId::new()).unwrap();
    assert_eq!(none.plays, 0);
    assert_eq!(none.last_played, None);

    assert_eq!(lib.all_track_stats().unwrap().len(), 2);
    let recent = lib.recent_events(4).unwrap();
    assert_eq!(recent.len(), 4);
    assert!(recent.windows(2).all(|w| w[0].ts >= w[1].ts));
    assert_eq!(recent[0].device, lib.device_id());
    assert_eq!(recent[0].context, EventContext::Album);
    assert_eq!(lib.events_of_track(t2.id, 10).unwrap().len(), 2);
    assert_eq!(lib.count_events().unwrap(), 6);

    // Append-only and idempotent on id.
    assert!(!lib.record_event_raw(&last).unwrap());
    assert_eq!(lib.count_events().unwrap(), 6);
}

// -------------------------------------------------------------------- trash

#[test]
fn trash_and_purge() {
    let lib = lib();
    let a = artist(&lib, "A");
    let t = track(&lib, &a, None, "Song", 1);
    let old = local(&lib, &t, "Music/old.mp3", Codec::Mp3, Some(128));
    let new = local(&lib, &t, "Music/new.opus", Codec::Opus, Some(160));
    let stream = lib
        .add_source(&source(
            &t,
            SourceKind::Youtube,
            "vid",
            Codec::Opus,
            Some(160),
        ))
        .unwrap();
    assert_eq!(lib.active_source(t.id).unwrap().unwrap().id, new.id);

    // Upgrade flow: the old file goes to Trash.
    let day = 24 * 60 * 60 * 1000;
    let now = crate::now_ms();
    lib.trash_source_at(old.id, Some("Trash/old.mp3"), now - 40 * day)
        .unwrap();
    assert_eq!(
        lib.sources_of_track(t.id).unwrap().len(),
        2,
        "trashed sources are hidden"
    );
    assert_eq!(lib.active_source(t.id).unwrap().unwrap().id, new.id);

    // A file with the same path can be imported again while the old one sits in Trash.
    let reimported = local(&lib, &t, "Music/old.mp3", Codec::Mp3, Some(128));
    assert_ne!(reimported.id, old.id);

    // Trashing the last local file falls back to the stream, track stays.
    lib.trash_source_at(new.id, Some("Trash/new.opus"), now - 5 * day)
        .unwrap();
    lib.trash_source_at(reimported.id, None, now - 1000)
        .unwrap();
    let row = lib.get_track_row(t.id).unwrap().unwrap();
    assert!(!row.has_local);
    assert_eq!(row.active_kind, Some(SourceKind::Youtube));
    assert_eq!(lib.active_source(t.id).unwrap().unwrap().id, stream.id);

    let trash = lib.list_trash().unwrap();
    assert_eq!(trash.len(), 3);
    assert_eq!(trash[0].source.id, reimported.id, "newest first");
    assert_eq!(trash[0].track_title, "Song");
    let oldest = trash.last().unwrap();
    assert_eq!(oldest.source.id, old.id);
    assert_eq!(oldest.trash_path.as_deref(), Some("Trash/old.mp3"));

    // Purge by age: only the 40-day-old one goes; the caller gets its file path.
    let purged = lib.purge_trash(30).unwrap();
    assert_eq!(purged.len(), 1);
    assert_eq!(purged[0].source_id, old.id);
    assert_eq!(purged[0].path.as_deref(), Some("Trash/old.mp3"));
    assert_eq!(purged[0].track_id, t.id);
    assert_eq!(lib.list_trash().unwrap().len(), 2);
    assert!(lib.get_source(old.id).unwrap().is_none());
    assert!(
        lib.purge_trash(30).unwrap().is_empty(),
        "nothing left to purge"
    );

    // Without a recorded trash path the original locator is reported.
    let purged = lib.purge_trash_before(now + 1).unwrap();
    assert_eq!(purged.len(), 2);
    let by_id = |id| purged.iter().find(|p| p.source_id == id).unwrap();
    assert_eq!(by_id(reimported.id).path.as_deref(), Some("Music/old.mp3"));
    assert_eq!(by_id(new.id).path.as_deref(), Some("Trash/new.opus"));
    assert!(lib.list_trash().unwrap().is_empty());
    // The stream source is untouched and the track survives.
    assert!(lib.get_track(t.id).unwrap().is_some());
    assert_eq!(lib.sources_of_track(t.id).unwrap().len(), 1);
}

#[test]
fn purge_never_reports_the_path_of_a_live_source() {
    let lib = lib();
    let a = artist(&lib, "A");
    let t = track(&lib, &a, None, "Song", 1);
    // The old file was already gone when it was trashed (no trash path), then a
    // better file landed on the very same path.
    let old = local(&lib, &t, "Music/song.mp3", Codec::Mp3, Some(128));
    lib.trash_source_at(old.id, None, 1).unwrap();
    let new = local(&lib, &t, "Music/song.mp3", Codec::Mp3, Some(320));
    assert_ne!(new.id, old.id);

    let purged = lib.purge_trash_before(i64::MAX).unwrap();
    assert_eq!(purged.len(), 1);
    assert_eq!(purged[0].source_id, old.id);
    assert_eq!(purged[0].path, None, "the live file must stay");
    assert!(lib.get_source(new.id).unwrap().is_some());
}

#[test]
fn restore_from_trash() {
    let lib = lib();
    let a = artist(&lib, "A");
    let t = track(&lib, &a, None, "Song", 1);
    let s = local(&lib, &t, "Music/a.opus", Codec::Opus, Some(160));
    lib.trash_source(s.id, Some("Trash/a.opus")).unwrap();
    assert!(!lib.get_track_row(t.id).unwrap().unwrap().has_local);
    let back = lib.restore_source(s.id).unwrap();
    assert!(back.trashed_at.is_none());
    assert!(back.is_active);
    assert!(lib.get_track_row(t.id).unwrap().unwrap().has_local);
}

// ------------------------------------------------------------ palette/analysis

#[test]
fn palette_and_analysis_roundtrip() {
    let lib = lib();
    let a = artist(&lib, "A");
    let mut r = Release::new("R", credit(&a));
    r.release_type = ReleaseType::Ep;
    r.cover_ref = Some("cover.jpg".into());
    let r = lib.upsert_release(&r).unwrap();
    let t = track(&lib, &a, Some(&r), "T", 1);
    local(&lib, &t, "t.opus", Codec::Opus, Some(160));

    assert!(lib.get_palette(r.id).unwrap().is_none());
    assert_eq!(lib.releases_needing_palette(10).unwrap(), [r.id]);
    let mut p = Palette::new(r.id, 0xFF12_34AB);
    lib.put_palette(&p).unwrap();
    assert_eq!(lib.get_palette(r.id).unwrap().unwrap(), p);
    p.seed_argb = 0xFFFF_FFFF;
    p.ambient_blur = Some(vec![1, 2, 3]);
    p.scheme_blob = Some(vec![9; 100]);
    lib.put_palette(&p).unwrap();
    assert_eq!(lib.get_palette(r.id).unwrap().unwrap(), p);
    assert!(lib.releases_needing_palette(10).unwrap().is_empty());

    assert_eq!(lib.tracks_needing_analysis("bliss-1", 10).unwrap(), [t.id]);
    let an = Analysis {
        track_id: t.id,
        bliss_vector: (0..20).map(|i| i as f32 * 0.25 - 1.0).collect(),
        analyzed_with: "bliss-1".into(),
    };
    lib.put_analysis(&an).unwrap();
    assert_eq!(lib.get_analysis(t.id).unwrap().unwrap(), an);
    assert!(
        lib.tracks_needing_analysis("bliss-1", 10)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        lib.tracks_needing_analysis("bliss-2", 10).unwrap(),
        [t.id],
        "new version re-queues"
    );
}

// ---------------------------------------------------------------- userdata

/// Builds a library with a few tracks that share ids across two "devices".
struct Pair {
    a: Library,
    b: Library,
    tracks: Vec<Track>,
}

fn pair() -> Pair {
    let a = lib();
    let b = lib();
    a.set_device_id("dev-a".into()).unwrap();
    b.set_device_id("dev-b".into()).unwrap();
    let art = artist(&a, "Artist");
    b.upsert_artist(&art).unwrap();
    let tracks: Vec<Track> = (0..4)
        .map(|i| {
            let t = track(&a, &art, None, &format!("t{i}"), i);
            b.upsert_track(&t).unwrap();
            t
        })
        .collect();
    Pair { a, b, tracks }
}

#[test]
fn export_import_roundtrip_rebuilds_user_data() {
    let Pair { a, b, tracks } = pair();
    let p = a.create_playlist("Дорога").unwrap();
    let e0 = a.playlist_add(p.id, tracks[0].id).unwrap();
    a.playlist_add(p.id, tracks[1].id).unwrap();
    a.playlist_insert(p.id, tracks[2].id, 1).unwrap();
    let gone = a.playlist_add(p.id, tracks[3].id).unwrap();
    a.playlist_remove(gone.id).unwrap();
    a.like(tracks[0].id).unwrap();
    a.like(tracks[1].id).unwrap();
    a.unlike(tracks[1].id).unwrap();
    a.like(LikeTarget::Artist(
        tracks[0].artist_credit.0[0].artist_id.unwrap(),
    ))
    .unwrap();
    a.record_event(
        tracks[0].id,
        EventType::Start,
        None,
        None,
        EventContext::Playlist,
    )
    .unwrap();
    a.record_event(
        tracks[0].id,
        EventType::Complete,
        None,
        Some(5),
        EventContext::Playlist,
    )
    .unwrap();
    let dead = a.create_playlist("Удалён").unwrap();
    a.delete_playlist(dead.id).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("Music/.musiclib/userdata");
    let rep = a.export_userdata(&out).unwrap();
    assert_eq!(
        (rep.playlists, rep.playlist_entries, rep.likes, rep.events),
        (2, 4, 3, 2)
    );
    assert_eq!(rep.files_written, 3);
    for f in ["playlists.json", "likes.json", "events.jsonl"] {
        assert!(out.join(f).is_file(), "{f}");
    }
    let leftovers: Vec<_> = fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp files are renamed away: {leftovers:?}"
    );
    assert_eq!(
        fs::read_to_string(out.join("events.jsonl"))
            .unwrap()
            .lines()
            .count(),
        2
    );

    // Exporting unchanged data again touches nothing.
    assert_eq!(a.export_userdata(&out).unwrap().files_written, 0);

    // A fresh database (same catalog ids) rebuilt from the export.
    let report = b.import_userdata(&out).unwrap();
    assert_eq!(report.invalid_lines, 0);
    assert_eq!(report.events_added, 2);
    assert_eq!(entry_tracks(&b, p.id), ["t0", "t2", "t1"]);
    assert_eq!(b.get_playlist(p.id).unwrap().unwrap().name, "Дорога");
    assert!(
        b.get_playlist(dead.id).unwrap().is_none(),
        "tombstone carried over"
    );
    assert!(b.is_liked(tracks[0].id).unwrap());
    assert!(!b.is_liked(tracks[1].id).unwrap());
    assert_eq!(b.liked_artists(0, 10).unwrap().len(), 1);
    assert_eq!(b.count_events().unwrap(), 2);
    assert_eq!(b.playlist_entries(p.id, 0, 1).unwrap()[0].entry.id, e0.id);
    assert_eq!(b.search("дорога", 5).unwrap().playlists.len(), 1);

    // Importing again changes nothing.
    let again = b.import_userdata(&out).unwrap();
    assert_eq!(again.applied, 0);
    assert_eq!(again.events_added, 0);
    assert_eq!(b.count_events().unwrap(), 2);

    // New events appended later are exported (count changed -> rewritten).
    a.record_event(
        tracks[2].id,
        EventType::Skip,
        None,
        None,
        EventContext::Radio,
    )
    .unwrap();
    assert_eq!(a.export_userdata(&out).unwrap().files_written, 1);
    assert_eq!(b.import_userdata(&out).unwrap().events_added, 1);
}

#[test]
fn import_resolves_conflicts_last_writer_wins() {
    let Pair { a, b, tracks } = pair();
    let p = a.create_playlist("Original").unwrap();
    let e = a.playlist_add(p.id, tracks[0].id).unwrap();
    a.like(tracks[0].id).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let from_a = dir.path().join("a");
    a.export_userdata(&from_a).unwrap();
    b.import_userdata(&from_a).unwrap();

    // Both devices now edit the same things "concurrently"; timestamps are set
    // explicitly in the exported files so the outcome is deterministic.
    b.rename_playlist(p.id, "B name").unwrap();
    b.playlist_remove(e.id).unwrap();
    b.unlike(tracks[0].id).unwrap();
    let from_b = dir.path().join("b");
    b.export_userdata(&from_b).unwrap();
    let base = crate::now_ms();

    // File "a2": A's later edits - rename (newer than B's), re-like (newer), entry revived? (older)
    a.rename_playlist(p.id, "A name").unwrap();
    a.unlike(tracks[0].id).unwrap();
    a.like(tracks[0].id).unwrap();
    let from_a2 = dir.path().join("a2");
    a.export_userdata(&from_a2).unwrap();

    let set = |dir: &Path, playlist_ts: i64, entry_ts: i64, like_ts: i64| {
        rewrite_json(&dir.join("playlists.json"), |v| {
            let pl = &mut v["playlists"][0];
            pl["updated_at"] = (base + playlist_ts).into();
            pl["entries"][0]["updated_at"] = (base + entry_ts).into();
        });
        rewrite_json(&dir.join("likes.json"), |v| {
            v["likes"][0]["updated_at"] = (base + like_ts).into();
        });
    };
    // B's file: rename@100, entry tombstone@100, unlike@100.
    set(&from_b, 100, 100, 100);
    // A2 file: rename@200 (wins), entry live@50 (loses to B's tombstone), like@200 (wins).
    set(&from_a2, 200, 50, 200);
    rewrite_json(&from_a2.join("playlists.json"), |v| {
        v["playlists"][0]["entries"][0]["deleted"] = false.into();
    });

    // Start from a clean target that already holds A's original state.
    let c = lib();
    c.set_device_id("dev-c".into()).unwrap();
    let art = tracks[0].artist_credit.0[0].artist_id.unwrap();
    c.upsert_artist(&Artist {
        id: art,
        ..Artist::new("Artist")
    })
    .unwrap();
    for t in &tracks {
        c.upsert_track(t).unwrap();
    }
    c.import_userdata(&from_a).unwrap();
    assert_eq!(entry_tracks(&c, p.id), ["t0"]);
    assert!(c.is_liked(tracks[0].id).unwrap());

    // Import B (newer than original on all three): rename, removal and unlike apply.
    let r = c.import_userdata(&from_b).unwrap();
    assert!(r.applied >= 3, "{r:?}");
    assert_eq!(c.get_playlist(p.id).unwrap().unwrap().name, "B name");
    assert!(entry_tracks(&c, p.id).is_empty(), "tombstone respected");
    assert!(!c.is_liked(tracks[0].id).unwrap());

    // Import A2: its rename and like are newer and win; its stale live entry
    // (older than B's tombstone) must not revive the entry.
    c.import_userdata(&from_a2).unwrap();
    assert_eq!(c.get_playlist(p.id).unwrap().unwrap().name, "A name");
    assert!(
        c.is_liked(tracks[0].id).unwrap(),
        "newer like beats older unlike"
    );
    assert!(
        entry_tracks(&c, p.id).is_empty(),
        "older live row cannot revive a newer tombstone"
    );

    // Order independence: B after A2 changes nothing (all of B's rows are older).
    let r = c.import_userdata(&from_b).unwrap();
    assert_eq!(r.applied, 0, "{r:?}");
    assert_eq!(c.get_playlist(p.id).unwrap().unwrap().name, "A name");
    assert!(c.is_liked(tracks[0].id).unwrap());
}

#[test]
fn import_ties_prefer_tombstones_then_device() {
    let lib = lib();
    lib.set_device_id("dev-m".into()).unwrap();
    let a = artist(&lib, "A");
    let t = track(&lib, &a, None, "t", 1);
    lib.like(t.id).unwrap();
    let dir = tempfile::tempdir().unwrap();
    lib.export_userdata(dir.path()).unwrap();

    // Same timestamp, incoming is a tombstone -> tombstone wins.
    rewrite_json(&dir.path().join("likes.json"), |v| {
        v["likes"][0]["deleted"] = true.into();
        v["likes"][0]["updated_by"] = "dev-a".into();
    });
    lib.import_userdata(dir.path()).unwrap();
    assert!(!lib.is_liked(t.id).unwrap());

    // Same timestamp and both tombstones -> larger device id wins (deterministic).
    rewrite_json(&dir.path().join("likes.json"), |v| {
        v["likes"][0]["updated_by"] = "dev-z".into();
    });
    let r = lib.import_userdata(dir.path()).unwrap();
    assert_eq!(r.applied, 1);
    let by: String = lib
        .read(|c| Ok(c.query_row("SELECT updated_by FROM likes", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(by, "dev-z");

    // A live row at the same timestamp does not beat the tombstone.
    rewrite_json(&dir.path().join("likes.json"), |v| {
        v["likes"][0]["deleted"] = false.into();
        v["likes"][0]["updated_by"] = "dev-zz".into();
    });
    lib.import_userdata(dir.path()).unwrap();
    assert!(!lib.is_liked(t.id).unwrap());
}

#[test]
fn import_rejects_newer_format_and_tolerates_bad_event_lines() {
    let lib = lib();
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("likes.json"),
        r#"{"version": 99, "likes": []}"#,
    )
    .unwrap();
    assert!(matches!(
        lib.import_userdata(dir.path()),
        Err(LibraryError::UnsupportedFormat(99))
    ));
    fs::remove_file(dir.path().join("likes.json")).unwrap();

    // Missing dir / files: nothing to do. Garbage event lines are counted, not fatal.
    assert_eq!(
        lib.import_userdata(dir.path().join("missing"))
            .unwrap()
            .applied,
        0
    );
    fs::write(
        dir.path().join("events.jsonl"),
        "not json\n\n{\"also\": \"bad\"}\n",
    )
    .unwrap();
    let r = lib.import_userdata(dir.path()).unwrap();
    assert_eq!(r.invalid_lines, 2);
    assert_eq!(r.events_added, 0);
}

/// Rough timing of the UI queries on a big library: `cargo test -p nikgob-library --release -- --ignored --nocapture`.
#[test]
#[ignore = "timing smoke test"]
fn perf_smoke() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(dir.path().join("library.db")).unwrap();
    let a = artist(&lib, "Артист");
    let mut rels = Vec::new();
    for r in 0..2_000 {
        rels.push(release(
            &lib,
            &a,
            &format!("Альбом {r}"),
            Some(1970 + (r % 50)),
        ));
    }
    let t0 = std::time::Instant::now();
    for i in 0..50_000u32 {
        let t = track(
            &lib,
            &a,
            Some(&rels[(i % 2_000) as usize]),
            &format!("Трек номер {i}"),
            i / 2_000 + 1,
        );
        if i % 2 == 0 {
            local(&lib, &t, &format!("m/{i}.opus"), Codec::Opus, Some(160));
        }
    }
    println!("insert 50k tracks + 25k sources: {:?}", t0.elapsed());
    let time = |name: &str, f: &dyn Fn()| {
        let t = std::time::Instant::now();
        f();
        println!("{name}: {:?}", t.elapsed());
    };
    time("tracks page @40000 by title", &|| {
        lib.tracks(40_000, 60, TrackSort::asc(TrackSortKey::Title))
            .unwrap();
    });
    time("tracks page @40000 by artist", &|| {
        lib.tracks(40_000, 60, TrackSort::asc(TrackSortKey::Artist))
            .unwrap();
    });
    time("tracks page @40000 by album", &|| {
        lib.tracks(40_000, 60, TrackSort::asc(TrackSortKey::Album))
            .unwrap();
    });
    time("count tracks", &|| {
        lib.count_tracks(TrackFilter::All).unwrap();
    });
    time("releases page @1500", &|| {
        lib.releases(1_500, 60, ReleaseSort::asc(ReleaseSortKey::Artist))
            .unwrap();
    });
    time("search 'трек ном'", &|| {
        lib.search("трек ном", 30).unwrap();
    });
    time("search 'альбом 17'", &|| {
        lib.search("альбом 17", 30).unwrap();
    });
}

#[test]
fn moving_a_source_to_another_track_fixes_both_tracks() {
    let lib = lib();
    let a = artist(&lib, "A");
    let t1 = track(&lib, &a, None, "One", 1);
    let t2 = track(&lib, &a, None, "Two", 2);
    let s1 = local(&lib, &t1, "Music/one.opus", Codec::Opus, Some(160));
    let s2 = local(&lib, &t2, "Music/two.opus", Codec::Opus, Some(160));
    assert!(s1.is_active && s2.is_active);

    // The file of track one turns out to belong to track two.
    let mut moved = s1.clone();
    moved.track_id = t2.id;
    moved.is_active = false;
    lib.add_source(&moved).unwrap();

    assert!(lib.active_source(t1.id).unwrap().is_none());
    assert!(!lib.get_track_row(t1.id).unwrap().unwrap().has_local);
    assert_eq!(lib.sources_of_track(t2.id).unwrap().len(), 2);
    let on_two = lib.sources_of_track(t2.id).unwrap();
    assert_eq!(on_two.iter().filter(|s| s.is_active).count(), 1);
}

#[test]
fn merging_releases_moves_tracks_and_removes_the_source_release() {
    let lib = lib();
    let a = artist(&lib, "Radwimps");
    let main = release(&lib, &a, "Suzume", None);
    let mut piece = Release::new("Suzume", credit(&a));
    piece.year = Some(2022);
    piece.cover_ref = Some("c.jpg".into());
    let piece = lib.upsert_release(&piece).unwrap();
    let t1 = track(&lib, &a, Some(&main), "One", 1);
    let t2 = track(&lib, &a, Some(&piece), "Two", 2);
    local(&lib, &t2, "two.opus", Codec::Opus, Some(160));
    lib.like(LikeTarget::Release(piece.id)).unwrap();

    assert_eq!(lib.merge_releases(piece.id, main.id).unwrap(), 1);
    assert!(lib.get_release(piece.id).unwrap().is_none());
    let merged = lib.get_release(main.id).unwrap().unwrap();
    assert_eq!((merged.year, merged.cover_ref.as_deref()), (Some(2022), Some("c.jpg")));
    let rows = lib.tracks_of_release(main.id).unwrap();
    assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![t1.id, t2.id]);
    assert_eq!(lib.count_releases().unwrap(), 1);
    assert!(lib.is_liked(LikeTarget::Release(main.id)).unwrap());
    assert_eq!(lib.search("two", 5).unwrap().tracks.len(), 1);
}

#[test]
fn merging_keeps_one_mbid_and_refuses_two() {
    let lib = lib();
    let a = artist(&lib, "A");
    let with_mbid = |title: &str, mbid: Option<&str>| {
        let mut r = Release::new(title, credit(&a));
        r.mbid = mbid.map(Into::into);
        lib.upsert_release(&r).unwrap()
    };
    let (plain, one, two) = (with_mbid("X", None), with_mbid("Y", Some("m1")), with_mbid("Z", Some("m2")));
    assert!(matches!(lib.merge_releases(one.id, two.id), Err(LibraryError::Invalid(_))));
    for r in [&plain, &one, &two] {
        assert!(lib.get_release(r.id).unwrap().is_some(), "nothing changed");
    }
    lib.merge_releases(one.id, plain.id).unwrap();
    assert_eq!(lib.get_release(plain.id).unwrap().unwrap().mbid.as_deref(), Some("m1"));
}

// ------------------------------------------------------ deleted tracks (Корзина)

/// Deletes a track the way the import crate does: sources trashed and the
/// track marked, with one timestamp.
fn delete(lib: &Library, t: &Track, at: i64) {
    for s in lib.sources_of_track(t.id).unwrap() {
        lib.trash_source_at(s.id, Some(&format!("Trash/{}", s.locator)), at).unwrap();
    }
    lib.mark_track_deleted(t.id, at).unwrap();
}

#[test]
fn deleted_tracks_leave_every_listing_and_come_back_on_restore() {
    let lib = lib();
    let solo = artist(&lib, "Solo");
    let other = artist(&lib, "Other");
    let gone = release(&lib, &solo, "Gone", Some(2001));
    let kept = release(&lib, &other, "Kept", Some(2002));
    let g1 = track(&lib, &solo, Some(&gone), "Gone One", 1);
    let k1 = track(&lib, &other, Some(&kept), "Kept One", 1);
    let k2 = track(&lib, &other, Some(&kept), "Kept Two", 2);
    for (t, f) in [(&g1, "g1.opus"), (&k1, "k1.opus"), (&k2, "k2.opus")] {
        local(&lib, t, f, Codec::Opus, Some(160));
    }
    let p = lib.create_playlist("Mix").unwrap();
    lib.playlist_add_many(p.id, &[g1.id, k1.id]).unwrap();
    lib.like(g1.id).unwrap();

    delete(&lib, &g1, 1_000);
    delete(&lib, &k2, 2_000);
    let all = TrackSort::asc(TrackSortKey::Title);
    assert_eq!(titles(&lib.tracks(0, 100, all).unwrap()), vec!["Kept One"]);
    assert_eq!(lib.count_tracks(TrackFilter::All).unwrap(), 1);
    // The album with only deleted tracks and its artist disappear; the other
    // album counts its live tracks only (no "0/1" dead weight).
    let rels = lib.releases(0, 100, ReleaseSort::asc(ReleaseSortKey::Title)).unwrap();
    assert_eq!(rels.iter().map(|r| r.release.title.as_str()).collect::<Vec<_>>(), vec!["Kept"]);
    assert_eq!((rels[0].local_tracks, rels[0].known_tracks), (1, 1));
    assert_eq!(lib.count_releases().unwrap(), 1);
    let names: Vec<_> = lib
        .artists(0, 100, ArtistSort::asc(ArtistSortKey::Name))
        .unwrap()
        .into_iter()
        .map(|a| a.artist.name)
        .collect();
    assert_eq!(names, vec!["Other"]);
    assert_eq!(lib.count_artists().unwrap(), 1);
    assert!(lib.search("gone", 10).unwrap().is_empty());
    assert_eq!(entry_tracks(&lib, p.id), vec!["Kept One"]);
    assert_eq!(lib.liked_count().unwrap(), 0);
    assert_eq!(titles(&lib.tracks_of_release(kept.id).unwrap()), vec!["Kept One"]);
    // ... but the importer still finds them, so a re-import revives instead of duplicating.
    assert_eq!(lib.tracks_of_release_with_deleted(gone.id).unwrap().len(), 1);
    assert_eq!(lib.releases_with_deleted(0, 100, ReleaseSort::asc(ReleaseSortKey::Title)).unwrap().len(), 2);
    assert_eq!(lib.artists_with_deleted(0, 100, ArtistSort::asc(ArtistSortKey::Name)).unwrap().len(), 2);

    let trash = lib.deleted_tracks().unwrap();
    assert_eq!(trash.iter().map(|d| d.track.title.as_str()).collect::<Vec<_>>(), vec!["Kept Two", "Gone One"]);
    assert_eq!(trash[1].deleted_at, 1_000);
    assert_eq!(lib.deleted_tracks_before(1_500).unwrap(), vec![g1.id]);

    // Restore: what was trashed with the deletion comes back, older leftovers stay.
    let back = lib.deleted_track_sources(g1.id).unwrap();
    assert_eq!(back.len(), 1);
    lib.restore_source(back[0].source.id).unwrap();
    lib.undelete_track(g1.id).unwrap();
    assert!(lib.get_track_row(g1.id).unwrap().unwrap().has_local);
    assert_eq!(lib.count_releases().unwrap(), 2);
    assert_eq!(entry_tracks(&lib, p.id), vec!["Gone One", "Kept One"]);
    assert_eq!(lib.liked_count().unwrap(), 1, "likes survive the trip to the Trash");

    // A live file for a deleted track (the same song imported again) revives it.
    local(&lib, &k2, "k2-again.opus", Codec::Opus, Some(160));
    assert_eq!(lib.track_deleted_at(k2.id).unwrap(), None);
    assert!(lib.deleted_tracks().unwrap().is_empty());
}

#[test]
fn purging_removes_rows_files_and_empty_albums() {
    let lib = lib();
    let a = artist(&lib, "Solo");
    let r = release(&lib, &a, "Only", None);
    let t = track(&lib, &a, Some(&r), "Song", 1);
    let live = local(&lib, &t, "Music/song.opus", Codec::Opus, Some(160));
    lib.trash_source_at(live.id, Some("Trash/2026-01-01 old.mp3"), 5).unwrap();
    local(&lib, &t, "Music/song.opus", Codec::Opus, Some(256));
    lib.put_master(&nikgob_core::model::Master {
        track_id: t.id,
        codec: Codec::Flac,
        file_hash: "abc".into(),
        size_bytes: 10,
        origin: None,
        replicas: Vec::new(),
    })
    .unwrap();
    let p = lib.create_playlist("P").unwrap();
    lib.playlist_add(p.id, t.id).unwrap();
    lib.like(t.id).unwrap();
    lib.record_event(t.id, EventType::Start, None, None, EventContext::Album).unwrap();

    // Straight from the library (no Trash): live and trashed files are reported.
    let purged = lib.purge_track(t.id).unwrap();
    assert_eq!(purged.live_files, vec!["Music/song.opus"]);
    assert_eq!(purged.trash_files, vec!["Trash/2026-01-01 old.mp3"]);
    assert_eq!(purged.master_hash.as_deref(), Some("abc"));
    assert!(purged.release_removed);
    assert!(lib.get_track(t.id).unwrap().is_none());
    assert!(lib.get_release(r.id).unwrap().is_none());
    assert!(lib.get_artist(a.id).unwrap().is_none(), "an artist left without credits goes");
    assert!(lib.get_master(t.id).unwrap().is_none());
    assert!(lib.list_trash().unwrap().is_empty());
    assert!(lib.liked_track_ids().unwrap().is_empty(), "the like is tombstoned");
    assert_eq!(lib.playlist_len(p.id).unwrap(), 0);
    assert_eq!(lib.count_events().unwrap(), 1, "history is append-only");
    assert!(matches!(lib.purge_track(t.id), Err(LibraryError::NotFound { .. })));
}

#[test]
fn liking_an_album_likes_all_its_tracks() {
    let lib = lib();
    let a = artist(&lib, "A");
    let r = release(&lib, &a, "Album", None);
    let t1 = track(&lib, &a, Some(&r), "One", 1);
    let t2 = track(&lib, &a, Some(&r), "Two", 2);
    let stream_only = track(&lib, &a, Some(&r), "Three", 3);
    local(&lib, &t1, "1.opus", Codec::Opus, Some(160));
    local(&lib, &t2, "2.opus", Codec::Opus, Some(160));
    let liked = |lib: &Library| {
        lib.releases(0, 10, ReleaseSort::asc(ReleaseSortKey::Title)).unwrap()[0].liked
    };
    assert!(!liked(&lib) && !lib.release_liked(r.id).unwrap());

    // Liking the album = liking its local tracks; it reads as liked.
    let ids = lib.like_release(r.id, true).unwrap();
    assert_eq!(ids, vec![t1.id, t2.id]);
    assert!(liked(&lib) && lib.release_liked(r.id).unwrap());
    assert_eq!(titles(&lib.liked_tracks(0, 10).unwrap()), vec!["One", "Two"], "album order");
    assert!(!lib.is_liked(stream_only.id).unwrap());

    // Unliking one track: the album is no longer liked.
    lib.unlike(t2.id).unwrap();
    assert!(!lib.release_liked(r.id).unwrap());
    lib.like(t2.id).unwrap();
    assert!(lib.release_liked(r.id).unwrap());

    // Unliking the album unlikes all its tracks.
    lib.like_release(r.id, false).unwrap();
    assert_eq!(lib.liked_count().unwrap(), 0);
    assert!(!liked(&lib));

    // An album without local files: all its tracks count.
    let r2 = release(&lib, &a, "Streams", None);
    let s1 = track(&lib, &a, Some(&r2), "S1", 1);
    assert_eq!(lib.like_release(r2.id, true).unwrap(), vec![s1.id]);
    assert!(lib.release_liked(r2.id).unwrap());
}

/// Regression guard for "the history went to zero after an import": every
/// path an import, a re-import, a folder merge or a rescan takes keeps the
/// track ids, so the history stays attached and readable.
#[test]
fn listening_history_survives_reimport_merge_and_trash() {
    let lib = lib();
    let a = artist(&lib, "A");
    let main = release(&lib, &a, "Album", None);
    let piece = release(&lib, &a, "Album ", None);
    let mut t = Track::new("Song", credit(&a));
    t.recording_mbid = Some("rec-1".into());
    t.release_id = Some(piece.id);
    t.track_no = 2;
    let t = lib.upsert_track(&t).unwrap();
    let other = track(&lib, &a, Some(&main), "Other", 1);
    local(&lib, &t, "Music/song.mp3", Codec::Mp3, Some(128));
    for _ in 0..3 {
        lib.record_event(t.id, EventType::Start, None, None, EventContext::Album).unwrap();
        lib.record_event(t.id, EventType::Complete, None, None, EventContext::Album).unwrap();
    }
    lib.record_event(other.id, EventType::Start, None, None, EventContext::Album).unwrap();
    let before = lib.track_stats(t.id).unwrap();
    assert_eq!(before.plays, 3);

    // Startup folder merge.
    lib.merge_releases(piece.id, main.id).unwrap();
    // Re-import of the same recording (matched by MBID on the release) and an upgrade.
    let mut again = Track::new("Song (remaster)", credit(&a));
    again.recording_mbid = Some("rec-1".into());
    again.release_id = Some(main.id);
    assert_eq!(lib.upsert_track(&again).unwrap().id, t.id);
    let old = lib.active_source(t.id).unwrap().unwrap();
    lib.trash_source(old.id, Some("Trash/2026-01-01 song.mp3")).unwrap();
    local(&lib, &t, "Music/song.opus", Codec::Opus, Some(256));
    lib.purge_trash_before(i64::MAX).unwrap();
    // Delete and restore.
    delete(&lib, &t, 10);
    lib.undelete_track(t.id).unwrap();

    assert_eq!(lib.track_stats(t.id).unwrap(), before);
    assert_eq!(lib.count_events().unwrap(), 7);
    let recent = lib.recent_events(10).unwrap();
    assert!(recent.iter().all(|e| lib.get_track(e.track_id).unwrap().is_some()));
    let rel = lib.get_track(t.id).unwrap().unwrap().release_id;
    assert_eq!(rel, Some(main.id), "history points at the merged album");
}

#[test]
fn migration_moves_old_deletions_into_the_trash() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    {
        // A v4 database with one deleted-the-old-way track and one live one.
        let conn = rusqlite::Connection::open(&path).unwrap();
        for (i, sql) in crate::migrations::MIGRATIONS.iter().take(4).enumerate() {
            conn.execute_batch(sql).unwrap();
            conn.pragma_update(None, "user_version", i as u32 + 1).unwrap();
        }
        conn.execute_batch(
            "INSERT INTO track(id, title, title_key, artist_credit, artist_display, artist_key, added_at)
             VALUES ('00000000-0000-0000-0000-000000000001', 'Dead', 'dead', '[]', '', '', 1),
                    ('00000000-0000-0000-0000-000000000002', 'Live', 'live', '[]', '', '', 1);
             INSERT INTO source(id, track_id, kind, locator, codec, added_at, trashed_at, trash_path)
             VALUES ('00000000-0000-0000-0000-00000000000a', '00000000-0000-0000-0000-000000000001',
                     'local_file', 'dead.opus', 'opus', 1, 777, 'Trash/dead.opus'),
                    ('00000000-0000-0000-0000-00000000000b', '00000000-0000-0000-0000-000000000002',
                     'local_file', 'live.opus', 'opus', 1, NULL, NULL);",
        )
        .unwrap();
    }
    let lib = Library::open(&path).unwrap();
    assert_eq!(lib.schema_version().unwrap(), SCHEMA_VERSION);
    let trash = lib.deleted_tracks().unwrap();
    assert_eq!(trash.len(), 1);
    assert_eq!((trash[0].track.title.as_str(), trash[0].deleted_at), ("Dead", 777));
    assert_eq!(lib.count_tracks(TrackFilter::All).unwrap(), 1);
}

#[test]
fn job_log_keeps_the_latest_entries() {
    let lib = lib();
    lib.log_job("import", "Импорт «A»", "добавлено 3", None).unwrap();
    lib.log_job("torrent", "B", "Скачано", None).unwrap();
    lib.log_job("import", "Импорт «C»", "добавлено 0", Some("Не удалось: 1")).unwrap();
    let imports = lib.job_log(Some("import"), 10).unwrap();
    assert_eq!(imports.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), vec!["Импорт «C»", "Импорт «A»"]);
    assert_eq!(imports[0].error.as_deref(), Some("Не удалось: 1"));
    assert_eq!(lib.job_log(None, 10).unwrap().len(), 3);
    for i in 0..crate::JOB_LOG_KEEP + 5 {
        lib.log_job("import", &i.to_string(), "", None).unwrap();
    }
    assert_eq!(lib.job_log(None, 10_000).unwrap().len(), crate::JOB_LOG_KEEP as usize);
}

#[test]
fn albums_without_live_tracks_are_not_listed_but_kept() {
    let lib = lib();
    let a = artist(&lib, "A");
    let old = release(&lib, &a, "Old", None);
    let new = release(&lib, &a, "New", None);
    let mut t = track(&lib, &a, Some(&old), "Song", 1);
    local(&lib, &t, "song.opus", Codec::Opus, Some(160));
    let listed = |lib: &Library| -> Vec<String> {
        lib.releases(0, 10, ReleaseSort::asc(ReleaseSortKey::Title))
            .unwrap()
            .into_iter()
            .map(|r| r.release.title)
            .collect()
    };
    assert_eq!(listed(&lib), ["Old"]);
    // A retag moves the only track into another album: the emptied one is
    // no album card any more, anywhere, but its row stays for the importer.
    t.release_id = Some(new.id);
    lib.upsert_track(&t).unwrap();
    assert_eq!(listed(&lib), ["New"]);
    assert_eq!(lib.count_releases().unwrap(), 1);
    assert_eq!(lib.releases_of_artist(a.id).unwrap().len(), 1);
    assert_eq!(lib.artists(0, 10, ArtistSort::asc(ArtistSortKey::Name)).unwrap()[0].release_count, 1);
    assert!(lib.search("old", 10).unwrap().releases.is_empty());
    assert!(lib.release_progress(old.id).unwrap().is_none());
    assert!(lib.get_release(old.id).unwrap().is_some());
    assert_eq!(lib.releases_with_deleted(0, 10, ReleaseSort::asc(ReleaseSortKey::Title)).unwrap().len(), 2);
}
#[test]
fn migration_drops_cached_lyricsplus_texts_and_picks_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    {
        // A v7 database: one track, liked, with cached lyrics of two sources
        // and picks of each.
        let conn = rusqlite::Connection::open(&path).unwrap();
        for (i, sql) in crate::migrations::MIGRATIONS.iter().take(7).enumerate() {
            conn.execute_batch(sql).unwrap();
            conn.pragma_update(None, "user_version", i as u32 + 1).unwrap();
        }
        conn.execute_batch(
            "INSERT INTO track(id, title, title_key, artist_credit, artist_display, artist_key, added_at)
             VALUES ('00000000-0000-0000-0000-000000000001', 'Лёд', 'лед', '[]', 'WENARO', 'wenaro', 1),
                    ('00000000-0000-0000-0000-000000000002', 'Two', 'two', '[]', 'X', 'x', 1);
             INSERT INTO likes(target_kind, target_id, updated_at, updated_by)
             VALUES ('track', '00000000-0000-0000-0000-000000000001', 1, 'test');
             INSERT INTO lyrics_text(track_id, source, body, fetched_at) VALUES
               ('00000000-0000-0000-0000-000000000001', 'lyricsplus', '[]', 1),
               ('00000000-0000-0000-0000-000000000002', 'lyricsplus', NULL, 1),
               ('00000000-0000-0000-0000-000000000001', 'lrcred', '[]', 1);
             INSERT INTO lyrics_pick(track_id, source) VALUES
               ('00000000-0000-0000-0000-000000000001', 'lyricsplus'),
               ('00000000-0000-0000-0000-000000000002', 'lrclib');",
        )
        .unwrap();
    }
    let lib = Library::open(&path).unwrap();
    assert_eq!(lib.schema_version().unwrap(), SCHEMA_VERSION);
    let one: TrackId = "00000000-0000-0000-0000-000000000001".parse().unwrap();
    let two: TrackId = "00000000-0000-0000-0000-000000000002".parse().unwrap();
    assert!(lib.lyrics_text(one, "lyricsplus").unwrap().is_none());
    assert!(lib.lyrics_text(two, "lyricsplus").unwrap().is_none());
    assert_eq!(lib.lyrics_text(one, "lrcred").unwrap().unwrap().0.as_deref(), Some("[]"));
    assert_eq!(lib.lyrics_pick(one).unwrap(), None);
    assert_eq!(lib.lyrics_pick(two).unwrap().as_deref(), Some("lrclib"));
    // Tracks and likes stay.
    assert_eq!(lib.get_track(one).unwrap().unwrap().title, "Лёд");
    assert!(lib.is_liked(one).unwrap());
    assert!(lib.get_track(two).unwrap().is_some());
}
