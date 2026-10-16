use nikgob_core::model::{Codec, SourceKind, score};
use nikgob_core::streaming::FakeService;

use super::*;
use crate::{
    ArtistSort, ArtistSortKey, ReleaseSort, ReleaseSortKey, TrackFilter, TrackSort, TrackSortKey,
};

fn lib() -> Library {
    Library::open_in_memory().unwrap()
}

/// Track `n` of the fake Spotify catalog (0..=3 "Группа крови", 4..=6 "First
/// Love", 7 "道", 8..=11 "Discovery", 12..=14 "Homogenic").
fn sp(n: usize) -> StreamTrack {
    FakeService::spotify().tracks()[n].clone()
}

fn find(service: &FakeService, id: &str) -> StreamTrack {
    service
        .tracks()
        .into_iter()
        .find(|t| t.id.id == id)
        .unwrap()
}

/// What the library listings show: tracks, releases, artists, search hits.
fn listed(lib: &Library) -> (u32, usize, usize, usize) {
    (
        lib.count_tracks(TrackFilter::All).unwrap(),
        lib.releases(0, 100, ReleaseSort::asc(ReleaseSortKey::Title))
            .unwrap()
            .len(),
        lib.artists(0, 100, ArtistSort::asc(ArtistSortKey::Name))
            .unwrap()
            .len(),
        lib.search("группа", 10).unwrap().tracks.len(),
    )
}

#[test]
fn stream_track_stays_outside_the_library_until_liked() {
    let lib = lib();
    let t = sp(0); // "Группа крови" by Кино
    let id = lib.upsert_stream_track(&t).unwrap();
    assert_eq!(lib.track_in_library(id).unwrap(), Some(false));
    assert_eq!(listed(&lib), (0, 0, 0, 0), "nothing is listed");
    assert_eq!(lib.count_releases().unwrap(), 0);
    assert_eq!(lib.count_artists().unwrap(), 0);
    assert!(
        lib.tracks(0, 10, TrackSort::asc(TrackSortKey::Title))
            .unwrap()
            .is_empty()
    );

    // Still a real track: point lookups, history and the stream source.
    let row = lib.get_track_row(id).unwrap().unwrap();
    assert_eq!(row.title, "Группа крови");
    assert_eq!(row.album.as_deref(), Some("Группа крови"));
    assert_eq!(row.active_kind, Some(SourceKind::Spotify));
    assert!(!row.has_local);
    let src = lib.active_source(id).unwrap().unwrap();
    assert_eq!(src.locator, t.id.id);
    assert_eq!(src.codec, Codec::Vorbis);
    assert_eq!(src.quality_score, score::STREAM_SPOTIFY);
    assert_eq!(lib.track_by_remote(&t.id).unwrap(), Some(id));

    lib.like(id).unwrap();
    assert_eq!(lib.track_in_library(id).unwrap(), Some(true));
    assert_eq!(listed(&lib), (1, 1, 1, 1));
    assert_eq!(lib.liked_tracks(0, 10).unwrap().len(), 1);
    let rel = lib
        .releases(0, 10, ReleaseSort::asc(ReleaseSortKey::Title))
        .unwrap();
    assert_eq!((rel[0].local_tracks, rel[0].known_tracks), (0, 1));

    lib.unlike(id).unwrap();
    assert_eq!(listed(&lib), (0, 0, 0, 0), "unliked: out again");
}

#[test]
fn playlists_bring_stream_tracks_in_and_take_them_out() {
    let lib = lib();
    let a = lib.upsert_stream_track(&sp(0)).unwrap();
    let b = lib.upsert_stream_track(&sp(1)).unwrap();
    let p = lib.create_playlist("Вечер").unwrap();
    let ea = lib.playlist_add(p.id, a).unwrap();
    lib.playlist_add(p.id, b).unwrap();
    assert_eq!(listed(&lib).0, 2);
    assert_eq!(lib.playlist_len(p.id).unwrap(), 2);
    lib.playlist_remove(ea.id).unwrap();
    assert_eq!(lib.track_in_library(a).unwrap(), Some(false));
    assert_eq!(lib.track_in_library(b).unwrap(), Some(true));
    lib.delete_playlist(p.id).unwrap();
    assert_eq!(lib.track_in_library(b).unwrap(), Some(false));
    assert_eq!(listed(&lib).0, 0);

    // Liking an album of stream tracks likes them all into the library.
    let rid = lib.get_track(a).unwrap().unwrap().release_id.unwrap();
    lib.like_release(rid, true).unwrap();
    assert_eq!(listed(&lib).0, 2);
}

#[test]
fn upsert_is_idempotent_and_refreshes_metadata() {
    let lib = lib();
    let mut t = sp(4);
    let id = lib.upsert_stream_track(&t).unwrap();
    t.title = "Automatic (Remastered)".into();
    t.duration_ms = Some(307_000);
    assert_eq!(lib.upsert_stream_track(&t).unwrap(), id);
    let got = lib.get_track(id).unwrap().unwrap();
    assert_eq!(got.title, "Automatic (Remastered)");
    assert_eq!(got.duration_ms, Some(307_000));
    assert_eq!(got.track_no, 1);
    assert_eq!(lib.sources_of_track(id).unwrap().len(), 1);
    // Album and artist were linked, and are reused by the next track.
    let other = lib.upsert_stream_track(&sp(5)).unwrap();
    let other = lib.get_track(other).unwrap().unwrap();
    assert_eq!(other.release_id, got.release_id);
    assert_eq!(
        other.artist_credit.primary().unwrap().artist_id,
        got.artist_credit.primary().unwrap().artist_id
    );
    let album = t.album.as_ref().unwrap().id.clone().unwrap();
    assert_eq!(lib.release_by_remote(&album).unwrap(), got.release_id);
    let artist = t.artists[0].id.clone().unwrap();
    assert_eq!(
        lib.artist_by_remote(&artist).unwrap(),
        got.artist_credit.primary().unwrap().artist_id
    );
    assert_eq!(got.artist_credit.display(), "宇多田ヒカル");
}

#[test]
fn same_isrc_from_the_other_service_is_a_second_source() {
    let lib = lib();
    let spotify = FakeService::spotify();
    let youtube = FakeService::youtube();
    let s = find(&spotify, "sp-discovery-1");
    let y = find(&youtube, "yt-discovery-1");
    assert_eq!(s.isrc, y.isrc);
    let id = lib.upsert_stream_track(&y).unwrap();
    assert_eq!(lib.upsert_stream_track(&s).unwrap(), id);
    assert_eq!(lib.sources_of_track(id).unwrap().len(), 2);
    assert_eq!(
        lib.active_source(id).unwrap().unwrap().kind,
        SourceKind::Spotify
    );
    assert_eq!(
        lib.external_ids(EntityKind::Track, *id.as_uuid()).unwrap(),
        vec![s.id.clone(), y.id.clone()]
    );
    // Without an ISRC the other service's copy is a separate track.
    let y2 = find(&youtube, "yt-gruppa-1");
    assert!(y2.isrc.is_none());
    let s2 = find(&spotify, "sp-gruppa-1");
    assert_ne!(
        lib.upsert_stream_track(&y2).unwrap(),
        lib.upsert_stream_track(&s2).unwrap()
    );
}

#[test]
fn a_local_track_with_the_isrc_keeps_its_tags_and_place() {
    let lib = lib();
    let kino = lib.upsert_artist(&Artist::new("Кино")).unwrap();
    let mut local = Track::new(
        "Группа Крови (tag)",
        ArtistCredit::single("Кино", Some(kino.id)),
    );
    local.isrc = Some("ZZFAK8800001".into());
    let local = lib.upsert_track(&local).unwrap();
    lib.add_source(&Source::new(
        local.id,
        SourceKind::LocalFile,
        "Music/a.flac",
        Codec::Flac,
    ))
    .unwrap();
    let before = listed(&lib);

    let id = lib.upsert_stream_track(&sp(0)).unwrap();
    assert_eq!(id, local.id);
    assert_eq!(
        lib.get_track(id).unwrap().unwrap().title,
        "Группа Крови (tag)"
    );
    assert_eq!(
        lib.active_source(id).unwrap().unwrap().kind,
        SourceKind::LocalFile
    );
    assert_eq!(lib.track_in_library(id).unwrap(), Some(true));
    assert_eq!(listed(&lib), before);
    lib.like(id).unwrap();
    lib.unlike(id).unwrap();
    assert_eq!(lib.track_in_library(id).unwrap(), Some(true), "local stays");

    // Another song of the same artist lands on the library's artist.
    let other = lib.upsert_stream_track(&sp(1)).unwrap();
    let credit = lib.get_track(other).unwrap().unwrap().artist_credit;
    assert_eq!(credit.primary().unwrap().artist_id, Some(kino.id));
}

#[test]
fn a_downloaded_file_makes_a_stream_track_a_library_track() {
    let lib = lib();
    let id = lib.upsert_stream_track(&sp(2)).unwrap();
    assert_eq!(lib.track_in_library(id).unwrap(), Some(false));
    lib.add_source(&Source::new(
        id,
        SourceKind::LocalFile,
        "Music/war.opus",
        Codec::Opus,
    ))
    .unwrap();
    assert_eq!(lib.track_in_library(id).unwrap(), Some(true));
    assert_eq!(
        lib.active_source(id).unwrap().unwrap().kind,
        SourceKind::LocalFile
    );
    assert_eq!(listed(&lib).0, 1);
}

#[test]
fn tracks_without_stream_sources_never_leave() {
    let lib = lib();
    let a = lib.upsert_artist(&Artist::new("A")).unwrap();
    let t = lib
        .upsert_track(&Track::new("Bare", ArtistCredit::single("A", Some(a.id))))
        .unwrap();
    lib.like(t.id).unwrap();
    lib.unlike(t.id).unwrap();
    assert_eq!(lib.track_in_library(t.id).unwrap(), Some(true));
    assert_eq!(lib.recompute_in_library_all().unwrap(), 0);
    assert_eq!(lib.count_tracks(TrackFilter::All).unwrap(), 1);
}

#[test]
fn external_ids_crud() {
    let lib = lib();
    let remote = RemoteId::new(Service::Youtube, "UC123");
    let local = Uuid::now_v7();
    assert_eq!(
        lib.external_id_local(EntityKind::Artist, &remote).unwrap(),
        None
    );
    lib.set_external_id(EntityKind::Artist, &remote, local)
        .unwrap();
    assert_eq!(
        lib.external_id_local(EntityKind::Artist, &remote).unwrap(),
        Some(local)
    );
    assert_eq!(
        lib.external_id_local(EntityKind::Track, &remote).unwrap(),
        None
    );
    assert_eq!(
        lib.external_ids(EntityKind::Artist, local).unwrap(),
        vec![remote.clone()]
    );
    // No such artist row: nothing to hand out.
    assert_eq!(lib.artist_by_remote(&remote).unwrap(), None);
    let other = Uuid::now_v7();
    lib.set_external_id(EntityKind::Artist, &remote, other)
        .unwrap();
    assert_eq!(
        lib.external_id_local(EntityKind::Artist, &remote).unwrap(),
        Some(other)
    );
    assert!(lib.remove_external_id(EntityKind::Artist, &remote).unwrap());
    assert!(!lib.remove_external_id(EntityKind::Artist, &remote).unwrap());
}

#[test]
fn playlist_remote_crud() {
    let lib = lib();
    let p = lib.create_playlist("Fake Mix").unwrap();
    let q = lib.create_playlist("Copy").unwrap();
    let remote = RemoteId::new(Service::Spotify, "sp-pl-mix");
    let mut r = PlaylistRemote {
        playlist_id: p.id,
        remote: remote.clone(),
        sync_mode: PlaylistSyncMode::Follow,
        remote_version: Some("1".into()),
        synced_at: None,
        base_json: None,
    };
    lib.put_playlist_remote(&r).unwrap();
    assert_eq!(lib.playlist_remote(p.id).unwrap().as_ref(), Some(&r));
    assert_eq!(lib.playlist_by_remote(&remote).unwrap().as_ref(), Some(&r));
    assert!(
        lib.mark_playlist_synced(p.id, Some("2"), 99, Some("[]"))
            .unwrap()
    );
    let got = lib.playlist_remote(p.id).unwrap().unwrap();
    assert_eq!(
        (got.remote_version.as_deref(), got.synced_at),
        (Some("2"), Some(99))
    );
    r.sync_mode = PlaylistSyncMode::TwoWay;
    lib.put_playlist_remote(&r).unwrap();
    assert_eq!(lib.playlist_remotes().unwrap().len(), 1);
    // Linking the same remote to another playlist moves the link.
    r.playlist_id = q.id;
    lib.put_playlist_remote(&r).unwrap();
    assert_eq!(lib.playlist_remote(p.id).unwrap(), None);
    assert_eq!(
        lib.playlist_by_remote(&remote)
            .unwrap()
            .unwrap()
            .playlist_id,
        q.id
    );
    assert!(lib.delete_playlist_remote(q.id).unwrap());
    assert!(lib.playlist_remotes().unwrap().is_empty());
}

#[test]
fn stream_tracks_survive_a_userdata_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let a = lib();
    let liked = a.upsert_stream_track(&sp(0)).unwrap();
    let in_list = a.upsert_stream_track(&sp(6)).unwrap();
    let heard_only = a.upsert_stream_track(&sp(1)).unwrap();
    let both = a
        .upsert_stream_track(&find(&FakeService::youtube(), "yt-discovery-2"))
        .unwrap();
    assert_eq!(a.upsert_stream_track(&sp(9)).unwrap(), both, "same ISRC");
    a.like(liked).unwrap();
    a.like(both).unwrap();
    let p = a.create_playlist("Вечер").unwrap();
    a.playlist_add(p.id, in_list).unwrap();
    let rep = a.export_userdata(dir.path()).unwrap();
    assert_eq!(rep.stream_tracks, 3, "the one only heard stays out");
    assert!(dir.path().join(crate::STREAMS_FILE).exists());

    let b = lib();
    let rep = b.import_userdata(dir.path()).unwrap();
    assert_eq!(rep.stream_tracks, 3);
    for id in [liked, in_list, both] {
        assert_eq!(b.track_in_library(id).unwrap(), Some(true), "{id}");
        assert_eq!(
            b.get_track_row(id).unwrap().unwrap().title,
            a.get_track_row(id).unwrap().unwrap().title
        );
    }
    assert_eq!(b.get_track(heard_only).unwrap(), None);
    assert_eq!(b.sources_of_track(both).unwrap().len(), 2);
    assert_eq!(b.track_by_remote(&sp(0).id).unwrap(), Some(liked));
    assert_eq!(b.liked_tracks(0, 10).unwrap().len(), 2);
    assert_eq!(
        b.playlist_entries(p.id, 0, 10).unwrap()[0].track.id,
        in_list
    );
    let (ta, tb) = (
        a.get_track(liked).unwrap().unwrap(),
        b.get_track(liked).unwrap().unwrap(),
    );
    assert_eq!(
        (tb.isrc, tb.track_no, tb.duration_ms),
        (ta.isrc, ta.track_no, ta.duration_ms)
    );
    assert_eq!(
        b.get_release(tb.release_id.unwrap())
            .unwrap()
            .unwrap()
            .title,
        "Группа крови"
    );
    // Importing again changes nothing.
    b.import_userdata(dir.path()).unwrap();
    assert_eq!(b.count_tracks(TrackFilter::All).unwrap(), 3);
    // And the export of the import is the same file.
    let dir2 = tempfile::tempdir().unwrap();
    b.export_userdata(dir2.path()).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join(crate::STREAMS_FILE)).unwrap(),
        std::fs::read_to_string(dir2.path().join(crate::STREAMS_FILE)).unwrap()
    );
}

#[test]
fn schema_has_the_streaming_tables() {
    let lib = lib();
    assert_eq!(lib.schema_version().unwrap(), 6);
}
