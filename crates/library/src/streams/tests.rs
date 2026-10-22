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
    // Another album of the artist, 2 s off: another recording (as close
    // as 1.5 s it would be the song whatever the album).
    let mut live_album = yt_variant("yt-live", "Группа крови", 287);
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
fn the_v2_repair_folds_dual_titles_and_other_albums_into_the_file() {
    let lib = lib();
    let fmb = lib.upsert_artist(&Artist::new("FUNKY MONKEY BΛBY'S")).unwrap();
    let credit = ArtistCredit::single("FUNKY MONKEY BΛBY'S", Some(fmb.id));
    let release = lib
        .upsert_release(&Release::new("夏子", credit.clone()))
        .unwrap();
    let mut local = Track::new("夏子", credit);
    local.release_id = Some(release.id);
    local.duration_ms = Some(212_800);
    let local = lib.upsert_track(&local).unwrap();
    lib.add_source(&Source::new(local.id, SourceKind::LocalFile, "Music/n.flac", Codec::Flac))
        .unwrap();
    // Made before the rule: the stream copy on its own single, in its own
    // album card; and a live take that stays.
    let mut y = yt_variant("yt-natsuko", "夏子 - Natsuko", 213);
    y.artists[0].name = "Funky Monkey Bλby's".into();
    let album = y.album.as_mut().unwrap();
    album.title = "Natsuko".into();
    album.id = Some(RemoteId::new(Service::Youtube, "yt-alb-natsuko"));
    album.artists = y.artists.clone();
    let stream = separate(&lib, &y);
    let mut live = y.clone();
    live.id = RemoteId::new(Service::Youtube, "yt-natsuko-live");
    live.title = "夏子 - Live".into();
    let live = separate(&lib, &live);
    // A clip of the song goes into its file (docs/stage4.md §12.5).
    let mut clip = y.clone();
    clip.id = RemoteId::new(Service::Youtube, "yt-natsuko-mv");
    clip.kind = nikgob_core::streaming::TrackKind::Video;
    let clip = separate(&lib, &clip);
    for t in [stream, live, clip] {
        lib.like(t).unwrap();
    }
    let titles = |lib: &Library| -> Vec<String> {
        let mut t: Vec<String> = lib
            .releases(0, 100, ReleaseSort::asc(ReleaseSortKey::Title))
            .unwrap()
            .into_iter()
            .map(|r| r.release.title)
            .collect();
        t.sort();
        t
    };
    assert_eq!(titles(&lib), ["Natsuko", "夏子"]);
    // A library repaired by v1 is repaired again.
    lib.write(|tx| meta_set(tx, "stream_duplicates_merged.v1", "1")).unwrap();

    assert_eq!(lib.merge_duplicate_stream_tracks().unwrap(), 2);
    assert!(lib.get_track(stream).unwrap().is_none());
    assert!(lib.get_track(live).unwrap().is_some(), "a version stays");
    assert!(lib.get_track(clip).unwrap().is_none(), "the clip is the file's");
    assert_eq!(lib.track_by_remote(&y.id).unwrap(), Some(local.id));
    assert!(lib.is_liked(local.id).unwrap());
    assert_eq!(
        lib.active_source(local.id).unwrap().unwrap().kind,
        SourceKind::LocalFile,
        "the file keeps playing"
    );
    assert_eq!(titles(&lib), ["Natsuko", "夏子"], "the live take keeps its card");
    lib.unlike(live).unwrap();
    assert_eq!(titles(&lib), ["夏子"], "the stream album is gone");
    assert_eq!(lib.merge_duplicate_stream_tracks().unwrap(), 0, "once");
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
    assert!(lib.schema_version().unwrap() >= 9);
}

/// The fake YouTube clip of «Группа крови» (no album, kind video) and its song.
fn yt_clip_and_song() -> (StreamTrack, StreamTrack) {
    let yt = FakeService::youtube();
    (find(&yt, "yt-video-kino"), find(&yt, "yt-gruppa-1"))
}

#[test]
fn a_liked_clip_becomes_its_song_and_keeps_the_like_identity() {
    let lib = lib();
    let (clip, song) = yt_clip_and_song();
    let (new, _) = lib.take_remote_likes(std::slice::from_ref(&clip), &[], 1).unwrap();
    let track = new[0];
    let row = lib.get_track_row(track).unwrap().unwrap();
    assert!(row.is_clip(), "a clip ranks and shows as one");
    assert_eq!(row.youtube_id.as_deref(), Some("yt-video-kino"));
    assert_eq!(row.album, None);
    let clips = lib.liked_youtube_clips().unwrap();
    assert_eq!(clips.len(), 1);
    assert_eq!(clips[0].track, track);
    assert_eq!(clips[0].video.title, "Группа крови (клип)");
    assert_eq!(clips[0].video.artists[0].name, "Кино");

    let now = lib.clip_to_song(track, &clip.id, &song).unwrap();
    assert_eq!(now, track, "the same track, now the song");
    let row = lib.get_track_row(track).unwrap().unwrap();
    assert_eq!(row.title, "Группа крови");
    assert_eq!(row.album.as_deref(), Some("Группа крови"));
    assert!(row.cover_ref.as_deref().is_some_and(|c| c.starts_with("https://")));
    assert_eq!(row.duration_ms, Some(285_000));
    assert!(!row.is_clip());
    assert_eq!(row.youtube_id.as_deref(), Some("yt-gruppa-1"));
    assert!(row.liked);
    let sources = lib.sources_of_track(track).unwrap();
    assert_eq!(sources.len(), 1, "the clip is no source any more");
    assert_eq!(sources[0].locator, "yt-gruppa-1");
    // The like is still the clip's, both ways.
    let likes = lib.remote_likes_of(track).unwrap();
    assert_eq!(likes.len(), 1);
    assert_eq!(likes[0].remote, clip.id);
    assert_eq!(lib.track_by_remote(&clip.id).unwrap(), Some(track));
    assert!(lib.liked_youtube_clips().unwrap().is_empty(), "done");
    // The clip seen again (a search, a pull) stays the song, no second source.
    assert_eq!(lib.upsert_stream_track(&clip).unwrap(), track);
    assert_eq!(lib.get_track_row(track).unwrap().unwrap().title, "Группа крови");
    assert_eq!(lib.sources_of_track(track).unwrap().len(), 1);
    // And the song itself is the same track.
    assert_eq!(lib.upsert_stream_track(&song).unwrap(), track);
}

#[test]
fn a_clip_whose_song_is_in_the_library_is_merged_into_it() {
    let lib = lib();
    let (clip, song) = yt_clip_and_song();
    // The same song liked on Spotify first.
    let spotify = sp(0);
    let (new, _) = lib.take_remote_likes(std::slice::from_ref(&spotify), &[], 1).unwrap();
    let song_track = new[0];
    let (new, _) = lib.take_remote_likes(std::slice::from_ref(&clip), &[], 2).unwrap();
    let clip_track = new[0];
    assert_ne!(clip_track, song_track, "a clip is never matched by its name");
    let into = lib.clip_to_song(clip_track, &clip.id, &song).unwrap();
    assert_eq!(into, song_track, "one song, one track");
    assert!(lib.get_track_row(clip_track).unwrap().is_none());
    assert_eq!(lib.liked_tracks(0, 10).unwrap().len(), 1);
    let kinds: Vec<SourceKind> = lib.sources_of_track(song_track).unwrap().iter().map(|s| s.kind).collect();
    assert_eq!(kinds.len(), 2);
    assert!(kinds.contains(&SourceKind::Spotify) && kinds.contains(&SourceKind::Youtube));
    assert!(lib.remote_likes_of(song_track).unwrap().iter().any(|l| l.remote == clip.id));
}

#[test]
fn a_clip_without_a_song_is_marked_and_plays_after_a_song() {
    let lib = lib();
    let (mut clip, song) = yt_clip_and_song();
    // Imported before clips were told apart: the full YouTube score.
    clip.kind = TrackKind::Song;
    let (new, _) = lib.take_remote_likes(std::slice::from_ref(&clip), &[], 1).unwrap();
    let track = new[0];
    assert!(!lib.get_track_row(track).unwrap().unwrap().is_clip());
    assert_eq!(lib.liked_youtube_clips().unwrap().len(), 1, "no album: a candidate");
    assert!(lib.mark_clip(&clip.id).unwrap());
    assert!(!lib.mark_clip(&clip.id).unwrap(), "once");
    assert!(lib.get_track_row(track).unwrap().unwrap().is_clip());
    assert_eq!(lib.liked_youtube_clips().unwrap().len(), 1, "still a clip: asked again later");
    // A song source of the same service plays first.
    lib.attach_stream_source(track, &song).unwrap();
    let src = lib.active_source(track).unwrap().unwrap();
    assert_eq!(src.locator, "yt-gruppa-1");
    assert_eq!(src.quality_score, score::STREAM_YOUTUBE);
}

/// Everything a track is made of: its row, sources, ids and remote likes.
type Whole = (Option<crate::TrackRow>, Vec<(String, i32, bool)>, Vec<RemoteId>, Vec<RemoteLike>);

/// Everything a track is made of, to compare before and after.
fn whole(lib: &Library, track: TrackId) -> Whole {
    let mut sources: Vec<(String, i32, bool)> = lib
        .sources_of_track(track)
        .unwrap()
        .into_iter()
        .map(|s| (s.locator, s.quality_score, s.is_active))
        .collect();
    sources.sort();
    (
        lib.get_track_row(track).unwrap(),
        sources,
        lib.external_ids(EntityKind::Track, *track.as_uuid()).unwrap(),
        lib.remote_likes_of(track).unwrap(),
    )
}

#[test]
fn a_clip_replaced_by_hand_is_the_song_keeps_its_like_and_comes_back_whole() {
    let lib = lib();
    let (clip, _) = yt_clip_and_song();
    let (new, _) = lib.take_remote_likes(std::slice::from_ref(&clip), &[], 1).unwrap();
    let track = new[0];
    lib.mark_clip(&clip.id).unwrap();
    lib.put_lyrics_text(track, "lyricsplus", Some("[]")).unwrap();
    let before = whole(&lib, track);
    // A preview played the song first: a «seen» track holds its source.
    let yt = FakeService::youtube();
    let pick = find(&yt, "yt-gruppa-2");
    let seen = lib.upsert_stream_track(&pick).unwrap();
    assert_ne!(seen, track);
    let seen_before = whole(&lib, seen);

    let snap = lib.replace_with_song(track, std::slice::from_ref(&pick)).unwrap();
    let row = lib.get_track_row(track).unwrap().unwrap();
    assert_eq!(row.title, pick.title);
    assert_eq!(row.album.as_deref(), Some("Группа крови"));
    assert!(!row.is_clip());
    assert!(row.liked);
    assert_eq!(row.youtube_id.as_deref(), Some("yt-gruppa-2"));
    let sources = lib.sources_of_track(track).unwrap();
    assert_eq!(sources.len(), 1, "the clip is no source any more");
    assert_eq!(lib.remote_likes_of(track).unwrap()[0].remote, clip.id, "the like is still the clip's");
    assert_eq!(lib.track_by_remote(&clip.id).unwrap(), Some(track));
    assert_eq!(lib.track_by_remote(&pick.id).unwrap(), Some(track));
    assert!(lib.lyrics_text(track, "lyricsplus").unwrap().is_none(), "the clip's lyrics went");
    assert!(lib.liked_youtube_clips().unwrap().is_empty());

    lib.restore_track(&snap).unwrap();
    assert_eq!(whole(&lib, track), before);
    assert_eq!(whole(&lib, seen), seen_before, "the preview's track has its source back");
    assert!(lib.lyrics_text(track, "lyricsplus").unwrap().is_some());
    assert_eq!(lib.liked_tracks(0, 10).unwrap().len(), 1);
}

#[test]
fn a_track_with_a_file_is_not_replaced() {
    let lib = lib();
    let (clip, song) = yt_clip_and_song();
    let (new, _) = lib.take_remote_likes(std::slice::from_ref(&clip), &[], 1).unwrap();
    let mut file = Source::new(new[0], SourceKind::LocalFile, "Кино/1.opus", Codec::Opus);
    file.quality_score = 500;
    lib.add_source(&file).unwrap();
    assert!(lib.replace_with_song(new[0], std::slice::from_ref(&song)).is_err());
}

#[test]
fn cached_answers_are_forgotten_by_prefix_and_value() {
    let lib = lib();
    lib.cache_put("yt_clip.a", 1, "clip").unwrap();
    lib.cache_put("yt_clip.b", 1, "song").unwrap();
    lib.cache_put("ytXclip.c", 1, "clip").unwrap();
    assert_eq!(lib.cache_forget("yt_clip.", "clip").unwrap(), 1);
    assert!(lib.cache_get("yt_clip.a").unwrap().is_none());
    assert!(lib.cache_get("yt_clip.b").unwrap().is_some());
    assert!(lib.cache_get("ytXclip.c").unwrap().is_some(), "«_» is no wildcard");
}

/// A track with a file: `names` credited (joined by « & »), on its album.
fn file_track(lib: &Library, title: &str, names: &[&str], album: &str, ms: u32) -> TrackId {
    let mut credit = ArtistCredit::default();
    for (i, n) in names.iter().enumerate() {
        let a = lib.upsert_artist(&Artist::new(*n)).unwrap();
        credit.0.push(nikgob_core::model::ArtistCreditPart {
            artist_id: Some(a.id),
            name: (*n).to_owned(),
            join_phrase: if i + 1 < names.len() { " & ".into() } else { String::new() },
        });
    }
    let release = lib.upsert_release(&Release::new(album, credit.clone())).unwrap();
    let mut t = Track::new(title, credit);
    t.release_id = Some(release.id);
    t.duration_ms = Some(ms);
    let t = lib.upsert_track(&t).unwrap();
    lib.add_source(&Source::new(t.id, SourceKind::LocalFile, format!("Music/{}.opus", t.id), Codec::Opus))
        .unwrap();
    t.id
}

/// A YouTube track by `uploader`, a music video when `clip`, without an album.
fn yt(id: &str, title: &str, uploader: &str, ms: u64, clip: bool) -> StreamTrack {
    let mut t = yt_variant(id, title, 0);
    t.duration_ms = Some(ms);
    t.artists[0].name = uploader.into();
    t.artists[0].id = None;
    t.album = None;
    if clip {
        t.kind = nikgob_core::streaming::TrackKind::Video;
    }
    t
}

#[test]
fn the_v3_repair_folds_shared_artists_and_clips_into_their_files() {
    let lib = lib();
    let river = file_track(
        &lib,
        "Underground River (opening version ft. Raj Ramayya)",
        &["Kevin Penkin", "Raj Ramayya"],
        "MADE IN ABYSS ORIGINAL SOUNDTRACK",
        121_573,
    );
    let reason = file_track(&lib, "There is a reason (RUS COVER)", &["j.am"], "There is a reason (RUS COVER)", 292_033);
    let solo_title = "Solo Leveling Season 2 -Arise from the Shadow- Opening | \"ReawakeR\" by LiSA feat Felix of Stray Kids";
    let solo = file_track(&lib, solo_title, &["Crunchyroll"], solo_title, 100_152);
    let aiyou = file_track(&lib, "AIYOU", &["Eve"], "AIYOU", 159_896);
    let this_game = file_track(&lib, "This game", &["鈴木このみ"], "This game", 284_307);

    let mut river_yt = yt(
        "yt-river",
        "Underground River (opening version ft. Raj Ramayya) (feat. Raj Ramayya)",
        "Raj Ramayya",
        122_000,
        false,
    );
    // On the Japanese edition of the album: another album title.
    let mut album = yt_variant("yt-x", "x", 0).album.unwrap();
    album.title = "TVアニメ「メイドインアビス」オリジナルサウンドトラック".into();
    album.id = Some(RemoteId::new(Service::Youtube, "yt-alb-abyss"));
    album.artists = river_yt.artists.clone();
    river_yt.album = Some(album);
    let pairs = [
        (separate(&lib, &river_yt), river),
        (
            separate(
                &lib,
                &yt("yt-reason", "【j.am】 There is a reason 【NO GAME NO LIFE // RUS Cover】(Fobia)", "higanbanban", 293_000, true),
            ),
            reason,
        ),
        (separate(&lib, &yt("yt-solo", solo_title, "Crunchyroll", 101_000, true)), solo),
        (separate(&lib, &yt("yt-aiyou", "AIYOU - Eve Music Video", "Eve", 165_000, true)), aiyou),
    ];
    let cover = separate(
        &lib,
        &yt("yt-game", "[No Game No Life на русском] This Game [Onsa Media]", "Onsa Media Underground", 304_000, true),
    );
    let p = lib.create_playlist("P").unwrap();
    for (stream, _) in &pairs {
        lib.like(*stream).unwrap();
        lib.playlist_add_many(p.id, &[*stream]).unwrap();
    }
    lib.like(cover).unwrap();

    assert_eq!(lib.merge_duplicate_stream_tracks().unwrap(), 4);
    for (stream, file) in pairs {
        assert!(lib.get_track(stream).unwrap().is_none(), "{stream} merged");
        assert!(lib.is_liked(file).unwrap(), "the like moved");
        assert_eq!(lib.active_source(file).unwrap().unwrap().kind, SourceKind::LocalFile, "the file plays");
    }
    let entries = lib.playlist_entries(p.id, 0, 10).unwrap();
    assert_eq!(
        entries.iter().map(|e| e.entry.track_id).collect::<Vec<_>>(),
        vec![river, reason, solo, aiyou],
        "playlist entries moved"
    );
    assert!(lib.get_track(cover).unwrap().is_some(), "a cover by another artist stays");
    assert!(!lib.is_liked(this_game).unwrap());
}

#[test]
fn a_new_liked_clip_lands_on_the_file_of_its_song() {
    let lib = lib();
    let aiyou = file_track(&lib, "AIYOU", &["Eve"], "AIYOU", 159_896);
    let this_game = file_track(&lib, "This game", &["鈴木このみ"], "This game", 284_307);
    let clip = yt("yt-aiyou", "AIYOU - Eve Music Video", "Eve", 165_000, true);
    assert_eq!(lib.upsert_stream_track(&clip).unwrap(), aiyou);
    assert_eq!(lib.active_source(aiyou).unwrap().unwrap().kind, SourceKind::LocalFile);
    let far = yt("yt-aiyou-long", "AIYOU - Eve Music Video", "Eve", 200_000, true);
    assert_ne!(lib.upsert_stream_track(&far).unwrap(), aiyou, "40 s longer is another cut");
    let cover = yt("yt-game", "[No Game No Life на русском] This Game [Onsa Media]", "Onsa Media Underground", 304_000, true);
    assert_ne!(lib.upsert_stream_track(&cover).unwrap(), this_game);
    let river = file_track(
        &lib,
        "Underground River (opening version ft. Raj Ramayya)",
        &["Kevin Penkin", "Raj Ramayya"],
        "MADE IN ABYSS ORIGINAL SOUNDTRACK",
        121_573,
    );
    let song = yt(
        "yt-river",
        "Underground River (opening version ft. Raj Ramayya) (feat. Raj Ramayya)",
        "Raj Ramayya",
        122_000,
        false,
    );
    assert_eq!(lib.upsert_stream_track(&song).unwrap(), river, "a shared artist is enough");
}

#[test]
fn a_transliterated_file_and_its_cyrillic_stream_are_one_track_shown_in_cyrillic() {
    let lib = lib();
    let file = file_track(&lib, "ChETYRE UKRAINKI", &["MORGENSHTERN"], "LEGENDARNAJa PYL'", 122_760);
    // Liked on YouTube Music before the rule: a track of its own.
    let mut stream = yt("yt-4", "ЧЕТЫРЕ УКРАИНКИ", "MORGENSHTERN", 123_000, false);
    let mut album = yt_variant("yt-x", "x", 0).album.unwrap();
    album.title = "ЛЕГЕНДАРНАЯ ПЫЛЬ".into();
    album.id = Some(RemoteId::new(Service::Youtube, "yt-alb-pyl"));
    album.artists = stream.artists.clone();
    stream.album = Some(album);
    let twin = separate(&lib, &stream);
    lib.like(twin).unwrap();
    let other = separate(&lib, &yt("yt-5", "ТРИ УКРАИНКИ", "MORGENSHTERN", 123_000, false));
    // A library repaired by v3 is repaired again.
    lib.write(|tx| meta_set(tx, "stream_duplicates_merged.v3", "1")).unwrap();

    assert_eq!(lib.merge_duplicate_stream_tracks().unwrap(), 1);
    assert!(lib.get_track(twin).unwrap().is_none());
    assert!(lib.get_track(other).unwrap().is_some(), "another song stays");
    assert!(lib.is_liked(file).unwrap());
    let row = lib.get_track_row(file).unwrap().unwrap();
    assert_eq!(row.title, "ЧЕТЫРЕ УКРАИНКИ", "the Cyrillic spelling is shown");
    assert_eq!(row.album.as_deref(), Some("ЛЕГЕНДАРНАЯ ПЫЛЬ"));
    assert_eq!(lib.active_source(file).unwrap().unwrap().kind, SourceKind::LocalFile);
}

#[test]
fn a_new_cyrillic_stream_lands_on_its_transliterated_file() {
    let lib = lib();
    let file = file_track(&lib, "ChETYRE UKRAINKI", &["MORGENSHTERN"], "LEGENDARNAJa PYL'", 122_760);
    let stream = yt("yt-4", "ЧЕТЫРЕ УКРАИНКИ", "Моргенштерн", 123_000, false);
    assert_eq!(lib.upsert_stream_track(&stream).unwrap(), file);
    assert_eq!(lib.get_track_row(file).unwrap().unwrap().title, "ЧЕТЫРЕ УКРАИНКИ");
    // No Cyrillic album came: the album keeps its spelling.
    assert_eq!(lib.get_track_row(file).unwrap().unwrap().album.as_deref(), Some("LEGENDARNAJa PYL'"));
}

#[test]
fn an_album_with_one_file_learns_its_whole_tracklist_from_a_service() {
    let lib = lib();
    let file = file_track(&lib, "ChETYRE UKRAINKI", &["MORGENSHTERN"], "LEGENDARNAJa PYL'", 122_760);
    let release = lib.get_track(file).unwrap().unwrap().release_id.unwrap();
    let mut album = nikgob_core::streaming::StreamAlbum {
        id: RemoteId::new(Service::Deezer, "dz-pyl"),
        title: "ЛЕГЕНДАРНАЯ ПЫЛЬ".into(),
        artists: vec![nikgob_core::streaming::ArtistRef::new(None, "MORGENSHTERN")],
        year: Some(2020),
        release_date: None,
        release_type: Default::default(),
        cover_url: None,
        total_tracks: Some(3),
        tracks: Vec::new(),
    };
    for (no, title, ms) in [(1, "ПРОЛОГ", 90_000), (2, "ЧЕТЫРЕ УКРАИНКИ", 123_000), (3, "ЭПИЛОГ", 100_000)] {
        let mut t = StreamTrack::new(RemoteId::new(Service::Deezer, format!("dz-{no}")), title);
        t.artists = album.artists.clone();
        t.track_no = Some(no);
        t.duration_ms = Some(ms);
        album.tracks.push(t);
    }
    assert_eq!(lib.adopt_stream_album(release, &album).unwrap(), 2);
    let rows = lib.tracks_of_release_all(release).unwrap();
    assert_eq!(
        rows.iter().map(|r| (r.track_no, r.title.as_str(), r.has_local)).collect::<Vec<_>>(),
        [(1, "ПРОЛОГ", false), (2, "ЧЕТЫРЕ УКРАИНКИ", true), (3, "ЭПИЛОГ", false)]
    );
    assert_eq!(rows[1].id, file, "the file is the album's second song");
    let r = lib.get_release(release).unwrap().unwrap();
    assert_eq!((r.title.as_str(), r.total_tracks), ("ЛЕГЕНДАРНАЯ ПЫЛЬ", Some(3)));
    assert_eq!(lib.release_by_remote(&album.id).unwrap(), Some(release));
    assert_eq!(lib.count_tracks(TrackFilter::All).unwrap(), 1, "the missing songs stay outside the library");
    let p = lib.release_progress(release).unwrap().unwrap();
    assert_eq!((p.local, p.total), (1, 3), "«1 из 3»");
    assert_eq!(lib.adopt_stream_album(release, &album).unwrap(), 0, "nothing twice");
}
