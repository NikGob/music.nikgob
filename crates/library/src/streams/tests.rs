use nikgob_core::model::{Codec, SourceKind, WishlistItem, WishlistStatus, score};
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
    // Without an ISRC the other service's copy is found by name and length.
    let y2 = find(&youtube, "yt-gruppa-1");
    assert!(y2.isrc.is_none());
    let s2 = find(&spotify, "sp-gruppa-1");
    assert_eq!(
        lib.upsert_stream_track(&y2).unwrap(),
        lib.upsert_stream_track(&s2).unwrap()
    );
}

/// A YouTube copy of a fake track, as another version of the song.
fn yt_variant(id: &str, title: &str, secs: u64) -> StreamTrack {
    let mut t = find(&FakeService::youtube(), "yt-gruppa-1");
    t.id = RemoteId::new(Service::Youtube, id);
    t.title = title.into();
    t.duration_ms = Some(secs * 1000);
    t
}

/// The old upsert: a stream track of its own even when the song is known.
fn separate(lib: &Library, t: &StreamTrack) -> TrackId {
    lib.write(|tx| upsert_stream_track_in(tx, t, Some(TrackId::new())))
        .unwrap()
}

#[test]
fn the_same_song_from_both_services_is_one_track_other_versions_are_not() {
    let lib = lib();
    let spotify = FakeService::spotify();
    let youtube = FakeService::youtube();
    // Liked on both, YouTube first: no ISRC on the YouTube side.
    let y = find(&youtube, "yt-gruppa-1");
    let id = lib.upsert_stream_track(&y).unwrap();
    lib.like(id).unwrap();
    let s = find(&spotify, "sp-gruppa-1");
    assert_eq!(lib.upsert_stream_track(&s).unwrap(), id);
    lib.like(id).unwrap();
    assert_eq!(listed(&lib).0, 1, "one track in the library");
    assert_eq!(
        lib.active_source(id).unwrap().unwrap().kind,
        SourceKind::Spotify
    );
    assert_eq!(
        lib.external_ids(EntityKind::Track, *id.as_uuid())
            .unwrap()
            .len(),
        2
    );
    // A remaster tag is the same song; 2 s longer still is.
    let remaster = yt_variant("yt-x1", "Группа крови (Remastered 2019)", 287);
    let lib = self::lib();
    let id = lib.upsert_stream_track(&s).unwrap();
    assert_eq!(lib.upsert_stream_track(&remaster).unwrap(), id);
    // Other versions and lengths are other tracks.
    let lib = self::lib();
    let id = lib.upsert_stream_track(&s).unwrap();
    for (n, (title, secs)) in [
        ("Группа крови (Remix)", 285),
        ("Группа крови (Live)", 285),
        ("Группа крови (TV Size)", 285),
        ("Группа крови (Instrumental)", 285),
        ("Группа крови", 290),
    ]
    .into_iter()
    .enumerate()
    {
        let other = lib
            .upsert_stream_track(&yt_variant(&format!("yt-v{n}"), title, secs))
            .unwrap();
        assert_ne!(other, id, "{title} ({secs} s)");
    }
    // A clip of the same length is not the song either.
    let mut clip = yt_variant("yt-clip", "Группа крови", 285);
    clip.kind = nikgob_core::streaming::TrackKind::Video;
    assert_ne!(lib.upsert_stream_track(&clip).unwrap(), id);
    // Another album of the artist: another recording.
    let mut live_album = yt_variant("yt-live", "Группа крови", 285);
    live_album.album.as_mut().unwrap().title = "Последний концерт".into();
    assert_ne!(lib.upsert_stream_track(&live_album).unwrap(), id);
}

#[test]
fn a_local_file_of_the_song_gets_the_stream_by_name() {
    let lib = lib();
    let kino = lib.upsert_artist(&Artist::new("Кино")).unwrap();
    let credit = ArtistCredit::single("Кино", Some(kino.id));
    let release = lib
        .upsert_release(&Release::new("Группа крови", credit.clone()))
        .unwrap();
    let mut local = Track::new("Группа крови", credit);
    local.release_id = Some(release.id);
    local.duration_ms = Some(284_500);
    let local = lib.upsert_track(&local).unwrap();
    lib.add_source(&Source::new(
        local.id,
        SourceKind::LocalFile,
        "Music/g.flac",
        Codec::Flac,
    ))
    .unwrap();
    let y = find(&FakeService::youtube(), "yt-gruppa-1");
    assert_eq!(lib.upsert_stream_track(&y).unwrap(), local.id);
    assert_eq!(
        lib.active_source(local.id).unwrap().unwrap().kind,
        SourceKind::LocalFile
    );
}

#[test]
fn duplicates_made_before_are_merged_once_with_their_likes_and_wishlist() {
    let lib = lib();
    let y = find(&FakeService::youtube(), "yt-gruppa-1");
    let a = lib.upsert_stream_track(&sp(0)).unwrap();
    let b = separate(&lib, &y);
    assert_ne!(a, b);
    let remix = separate(&lib, &yt_variant("yt-r", "Группа крови (Remix)", 285));
    for t in [a, b, remix] {
        lib.like(t).unwrap();
    }
    let p = lib.create_playlist("P").unwrap();
    lib.playlist_add_many(p.id, &[b]).unwrap();
    lib.put_remote_like(&RemoteLike {
        remote: y.id.clone(),
        track_id: b,
        pulled: true,
        seen_at: 1,
    })
    .unwrap();
    let mut wa = WishlistItem::new(WishlistStatus::Waiting, 10);
    wa.track_id = Some(a);
    let mut wb = WishlistItem::new(WishlistStatus::Waiting, 20);
    wb.track_id = Some(b);
    lib.put_wishlist_item(&wa).unwrap();
    lib.put_wishlist_item(&wb).unwrap();
    assert_eq!(listed(&lib).0, 3);

    assert_eq!(lib.merge_duplicate_stream_tracks().unwrap(), 1);
    assert!(lib.get_track(b).unwrap().is_none());
    assert_eq!(listed(&lib).0, 2, "the remix stays");
    assert!(lib.is_liked(a).unwrap());
    assert_eq!(lib.track_by_remote(&y.id).unwrap(), Some(a));
    assert_eq!(lib.remote_likes(Service::Youtube).unwrap()[0].track_id, a);
    assert_eq!(lib.sources_of_track(a).unwrap().len(), 2);
    let entries = lib.playlist_entries(p.id, 0, 10).unwrap();
    assert_eq!(entries.iter().map(|e| e.entry.track_id).collect::<Vec<_>>(), vec![a]);
    let items = lib.list_wishlist().unwrap();
    assert_eq!(items.len(), 1, "one wishlist item per track");
    assert_eq!(items[0].id, wa.id, "the older one");
    assert_eq!(items[0].track_id, Some(a));
    // Once: a duplicate made now stays (the upsert never makes one).
    let c = separate(&lib, &yt_variant("yt-c", "Группа крови", 285));
    assert_eq!(lib.merge_duplicate_stream_tracks().unwrap(), 0);
    assert!(lib.get_track(c).unwrap().is_some());
}

#[test]
fn attaching_a_stream_with_a_track_of_its_own_merges_the_two() {
    let lib = lib();
    let a = lib.upsert_stream_track(&sp(0)).unwrap();
    let y = find(&FakeService::youtube(), "yt-gruppa-1");
    let b = separate(&lib, &y);
    lib.like(b).unwrap();
    let mut w = WishlistItem::new(WishlistStatus::Waiting, 10);
    w.track_id = Some(b);
    lib.put_wishlist_item(&w).unwrap();
    lib.attach_stream_source(a, &y).unwrap();
    assert!(lib.get_track(b).unwrap().is_none());
    assert!(lib.is_liked(a).unwrap());
    assert_eq!(lib.wishlist_for_track(a).unwrap().len(), 1);
    assert_eq!(lib.track_by_remote(&y.id).unwrap(), Some(a));
}

#[test]
fn stream_covers_are_stored_and_a_cover_file_wins() {
    let lib = lib();
    let s = sp(0);
    let id = lib.upsert_stream_track(&s).unwrap();
    let release = lib.get_track(id).unwrap().unwrap().release_id.unwrap();
    let cover = |lib: &Library| lib.get_release(release).unwrap().unwrap().cover_ref;
    assert!(s.cover_url.is_some());
    assert_eq!(cover(&lib), s.cover_url);
    assert!(is_web_ref(cover(&lib).as_deref().unwrap()));
    // The YouTube copy does not replace the Spotify cover...
    let y = find(&FakeService::youtube(), "yt-gruppa-1");
    lib.upsert_stream_track(&y).unwrap();
    assert_eq!(cover(&lib), s.cover_url);
    // ...a new url of the owning service does.
    let mut s2 = s.clone();
    s2.cover_url = Some("https://fake.invalid/sp/gruppa-640.jpg".into());
    lib.upsert_stream_track(&s2).unwrap();
    assert_eq!(cover(&lib), s2.cover_url);
    // A cover file (the download's) stays.
    let mut r = lib.get_release(release).unwrap().unwrap();
    r.cover_ref = Some("Кино/1988 - Группа крови/cover.jpg".into());
    lib.upsert_release(&r).unwrap();
    lib.upsert_stream_track(&s).unwrap();
    assert_eq!(
        cover(&lib).as_deref(),
        Some("Кино/1988 - Группа крови/cover.jpg")
    );
    // An url is no work for the palette.
    let other = lib.upsert_stream_track(&sp(4)).unwrap();
    let other = lib.get_track(other).unwrap().unwrap().release_id.unwrap();
    let queue = lib.releases_needing_palette(10).unwrap();
    assert!(queue.contains(&release));
    assert!(!queue.contains(&other));
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
fn a_stream_found_by_name_attaches_to_the_same_track() {
    let lib = lib();
    let spotify = FakeService::spotify();
    let youtube = FakeService::youtube();
    // "Группа крови": the YouTube copy has no ISRC.
    let id = lib.upsert_stream_track(&find(&spotify, "sp-gruppa-1")).unwrap();
    let mut yt = find(&youtube, "yt-gruppa-1");
    assert!(yt.isrc.is_none());
    lib.attach_stream_source(id, &yt).unwrap();
    assert_eq!(lib.track_by_remote(&yt.id).unwrap(), Some(id));
    let kinds: Vec<SourceKind> = lib.sources_of_track(id).unwrap().iter().map(|s| s.kind).collect();
    assert_eq!(kinds.len(), 2);
    assert!(kinds.contains(&SourceKind::Youtube));
    // The track keeps its title; upserting the YouTube copy finds it now.
    yt.title = "Gruppa krovi (official audio)".into();
    assert_eq!(lib.upsert_stream_track(&yt).unwrap(), id);
    assert_eq!(lib.get_track(id).unwrap().unwrap().title, "Группа крови");
    assert!(matches!(
        lib.attach_stream_source(TrackId::new(), &yt),
        Err(LibraryError::NotFound { .. })
    ));
}

#[test]
fn stream_only_tracks_are_those_without_a_file() {
    let lib = lib();
    let a = lib.upsert_stream_track(&sp(0)).unwrap();
    let b = lib.upsert_stream_track(&sp(1)).unwrap();
    let artist = lib.upsert_artist(&Artist::new("A")).unwrap();
    let bare = lib
        .upsert_track(&Track::new("Bare", ArtistCredit::single("A", Some(artist.id))))
        .unwrap();
    let ids = |lib: &Library| -> Vec<TrackId> {
        lib.stream_only_tracks().unwrap().iter().map(|t| t.id).collect()
    };
    assert_eq!(ids(&lib), vec![a, b], "a track without any source is not a stream track");
    lib.add_source(&Source::new(b, SourceKind::LocalFile, "Music/b.opus", Codec::Opus))
        .unwrap();
    assert_eq!(ids(&lib), vec![a]);
    let _ = bare;
}

#[test]
fn remote_likes_crud() {
    let lib = lib();
    let id = lib.upsert_stream_track(&sp(0)).unwrap();
    assert!(lib.remote_likes(Service::Spotify).unwrap().is_empty());
    assert!(!lib.likes_pulled(Service::Spotify).unwrap());
    let like = RemoteLike {
        remote: sp(0).id,
        track_id: id,
        pulled: true,
        seen_at: 5,
    };
    lib.put_remote_like(&like).unwrap();
    assert_eq!(lib.remote_likes(Service::Spotify).unwrap(), vec![like.clone()]);
    assert!(lib.remote_likes(Service::Youtube).unwrap().is_empty());
    assert_eq!(lib.remote_likes_of(id).unwrap(), vec![like.clone()]);
    lib.put_remote_like(&RemoteLike { pulled: false, ..like.clone() }).unwrap();
    assert!(!lib.remote_likes(Service::Spotify).unwrap()[0].pulled);
    assert!(lib.delete_remote_like(&like.remote).unwrap());
    assert!(!lib.delete_remote_like(&like.remote).unwrap());
    lib.set_likes_pulled(Service::Spotify, 9).unwrap();
    assert!(lib.likes_pulled(Service::Spotify).unwrap());
    assert!(!lib.likes_pulled(Service::Youtube).unwrap());
}

#[test]
fn schema_has_the_streaming_tables() {
    let lib = lib();
    assert_eq!(lib.schema_version().unwrap(), 8);
}
