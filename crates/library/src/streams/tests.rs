use nikgob_core::model::{Codec, SourceKind, WishlistItem, WishlistStatus, score};
use nikgob_core::streaming::FakeService;

use super::same::Song;

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

    // Saving an album of stream tracks puts them all into the library,
    // without liking them; unsaving takes them out again.
    let rid = lib.get_track(a).unwrap().unwrap().release_id.unwrap();
    lib.like_release(rid, true).unwrap();
    assert_eq!(listed(&lib).0, 2);
    assert_eq!(lib.liked_count().unwrap(), 0);
    lib.like_release(rid, false).unwrap();
    assert_eq!(listed(&lib).0, 0);

    // Liking an album that likes its tracks likes them all into the library.
    lib.set_album_likes_tracks(true);
    lib.like_release(rid, true).unwrap();
    assert_eq!(listed(&lib).0, 2);
    assert_eq!(lib.liked_count().unwrap(), 2);
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

/// YouTube Music's top search result names no album and no length: the
/// track made from it gets them from the song's details, and the top result
/// seen again does not take them away.
#[test]
fn a_stream_without_its_album_keeps_the_album_the_track_has() {
    let lib = lib();
    let full = find(&FakeService::youtube(), "yt-gruppa-1");
    let mut top = full.clone();
    top.album = None;
    top.duration_ms = None;
    top.track_no = None;
    let id = lib.upsert_stream_track(&top).unwrap();
    let got = lib.get_track(id).unwrap().unwrap();
    assert_eq!((got.release_id, got.duration_ms), (None, None));
    assert_eq!(lib.upsert_stream_track(&full).unwrap(), id);
    let completed = lib.get_track(id).unwrap().unwrap();
    assert!(completed.release_id.is_some());
    assert!(completed.duration_ms.is_some());
    assert_eq!(lib.upsert_stream_track(&top).unwrap(), id);
    let again = lib.get_track(id).unwrap().unwrap();
    assert_eq!(again.release_id, completed.release_id);
    assert_eq!(again.track_no, completed.track_no);
    assert_eq!(again.duration_ms, completed.duration_ms);
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
    // A clip of the song goes into its file.
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

/// The «медиатека» marks go out with the user data, with the stream tracks
/// they mark (outside the library), and come back as marks, not likes.
#[test]
fn saved_marks_survive_a_userdata_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let a = lib();
    let outside = a.ensure_stream_tracks(&[sp(1)]).unwrap()[0];
    let liked = a.upsert_stream_track(&sp(0)).unwrap();
    a.like(liked).unwrap();
    a.mark_saved(Service::Spotify, &[(sp(1).id.id.clone(), outside), (sp(0).id.id.clone(), liked)]).unwrap();
    let rep = a.export_userdata(dir.path()).unwrap();
    assert_eq!((rep.saved, rep.stream_tracks), (2, 2));
    assert!(dir.path().join(crate::SAVED_FILE).exists());

    let b = lib();
    let rep = b.import_userdata(dir.path()).unwrap();
    assert_eq!(rep.saved, 2);
    let mut saved = b.saved_tracks().unwrap();
    saved.sort();
    let mut want = vec![outside, liked];
    want.sort();
    assert_eq!(saved, want);
    assert_eq!(b.track_in_library(outside).unwrap(), Some(false), "a mark brings nothing into the library");
    assert_eq!(b.like_state(outside).unwrap(), None);
    assert_eq!(b.track_by_remote(&sp(1).id).unwrap(), Some(outside));
    // Again: nothing twice, and the export of the import is the same file.
    b.import_userdata(dir.path()).unwrap();
    assert_eq!(b.saved_tracks().unwrap().len(), 2);
    let dir2 = tempfile::tempdir().unwrap();
    b.export_userdata(dir2.path()).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join(crate::SAVED_FILE)).unwrap(),
        std::fs::read_to_string(dir2.path().join(crate::SAVED_FILE)).unwrap()
    );
}

/// An export that the database no longer matches (a backup of the
/// database put back after it) is stale; written again, it is not.
#[test]
fn an_export_of_another_state_is_stale() {
    let dir = tempfile::tempdir().unwrap();
    let a = lib();
    assert!(!a.userdata_export_stale(dir.path()).unwrap(), "nothing exported here yet");
    let backup = lib();
    let t = a.upsert_stream_track(&sp(0)).unwrap();
    a.like(t).unwrap();
    a.export_userdata(dir.path()).unwrap();
    assert!(!a.userdata_export_stale(dir.path()).unwrap());
    // The database of before the like, put back.
    assert!(backup.userdata_export_stale(dir.path()).unwrap());
    backup.export_userdata(dir.path()).unwrap();
    assert!(!backup.userdata_export_stale(dir.path()).unwrap());
    assert!(std::fs::read_to_string(dir.path().join(crate::LIKES_FILE)).unwrap().contains("\"likes\": []"));
    // A mark changes what the export holds.
    let s = backup.ensure_stream_tracks(&[sp(1)]).unwrap()[0];
    backup.mark_saved(Service::Spotify, &[(sp(1).id.id.clone(), s)]).unwrap();
    assert!(backup.userdata_export_stale(dir.path()).unwrap());
    // An export of an older build (no fingerprint) is stale.
    backup.export_userdata(dir.path()).unwrap();
    std::fs::remove_file(dir.path().join(crate::FINGERPRINT_FILE)).unwrap();
    assert!(backup.userdata_export_stale(dir.path()).unwrap());
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

/// Once: a stream that is a file of its album under a title in another
/// script goes into the file, its title an alias the search finds; a
/// stream of another album, another artist, another length or a crowded
/// length stays, and so does one whose file has the service already.
#[test]
fn a_stream_titled_in_another_script_goes_into_its_file_once() {
    let lib = lib();
    let ost = "MADE IN ABYSS ORIGINAL SOUNDTRACK 3";
    let belaf = file_track(&lib, "Belaf's Lullaby", &["Kevin Penkin"], ost, 199_950);
    let gravity = file_track(&lib, "Gravity", &["Kevin Penkin"], ost, 240_000);
    file_track(&lib, "Hanezeve", &["Kevin Penkin"], ost, 241_200);
    let faith = file_track(&lib, "Faith", &["Kevin Penkin"], "MADE IN ABYSS ORIGINAL SOUNDTRACK 2", 150_000);
    let song = |id: &str, title: &str, artist: &str, album: &str, secs: u64| {
        let mut t = StreamTrack::new(RemoteId::new(Service::Youtube, id), title);
        t.artists = vec![ArtistRef::new(None, artist)];
        t.album = Some(AlbumRef::new(None, album));
        t.duration_ms = Some(secs * 1000);
        let id = lib.ensure_stream_tracks(&[t]).unwrap()[0];
        let p = lib.create_playlist(&format!("p {id}")).unwrap();
        lib.playlist_add(p.id, id).unwrap();
        id
    };
    let jp = song("ytbelaf0001", "ベラフの子守歌", "Kevin Penkin", ost, 200);
    let other_album = song("ytfaith0001", "信仰", "Kevin Penkin", "MADE IN ABYSS ORIGINAL SOUNDTRACK", 150);
    let other_artist = song("ytstrange01", "知らない歌", "Someone Else", ost, 200);
    let crowded = song("ytcrowd0001", "重力", "Kevin Penkin", ost, 241);
    let latin = song("ytlatin0001", "Belafs Lullaby Remake", "Kevin Penkin", ost, 200);
    let tracks = lib.count_tracks(TrackFilter::All).unwrap();

    assert_eq!(lib.merge_other_script_titles().unwrap(), 1);
    assert!(lib.get_track(jp).unwrap().is_none());
    assert_eq!(lib.track_by_remote(&RemoteId::new(Service::Youtube, "ytbelaf0001")).unwrap(), Some(belaf));
    assert_eq!(lib.get_track(belaf).unwrap().unwrap().title, "Belaf's Lullaby", "the file's title stays");
    assert_eq!(lib.track_aliases(belaf).unwrap(), ["ベラフの子守歌"]);
    assert_eq!(lib.search("ベラフの子守歌", 5).unwrap().tracks.iter().map(|t| t.id).collect::<Vec<_>>(), [belaf]);
    assert_eq!(lib.count_tracks(TrackFilter::All).unwrap(), tracks - 1);
    for t in [other_album, other_artist, crowded, latin] {
        assert!(lib.get_track(t).unwrap().is_some(), "{t} stays");
    }
    assert!(lib.track_aliases(gravity).unwrap().is_empty() && lib.track_aliases(faith).unwrap().is_empty());
    lib.rebuild_search_index().unwrap();
    assert_eq!(lib.search("ベラフの子守歌", 5).unwrap().tracks.iter().map(|t| t.id).collect::<Vec<_>>(), [belaf], "the rebuilt index knows the alias");
    // Once: a twin made now stays.
    let again = song("ytbelaf0002", "ベラフの子守歌", "Kevin Penkin", ost, 200);
    assert_eq!(lib.merge_other_script_titles().unwrap(), 0);
    assert!(lib.get_track(again).unwrap().is_some());
    // The alias goes with its track into another one.
    lib.merge_track_into(belaf, gravity).unwrap();
    assert_eq!(lib.track_aliases(gravity).unwrap(), ["ベラフの子守歌"]);
}

/// A playlist in an album's order: the streams between two files of a disc,
/// as many as the places between, are those files — by their lengths, or,
/// of unknown length, by a title in another script. A run that does not
/// fill its gap, a stream of another length, a title of unknown length in
/// the same script, or a file the playlist has already stays as it is.
#[test]
fn streams_between_two_files_of_an_album_in_a_playlist_are_its_files() {
    let lib = lib();
    let ost = "MADE IN ABYSS ORIGINAL SOUNDTRACK 3";
    let artist = lib.upsert_artist(&Artist::new("Kevin Penkin")).unwrap();
    let credit = ArtistCredit::single("Kevin Penkin", Some(artist.id));
    let release = lib.upsert_release(&Release::new(ost, credit.clone())).unwrap();
    let file = |no: u32, title: &str, ms: u32| {
        let mut t = Track::new(title, credit.clone());
        t.release_id = Some(release.id);
        t.disc_no = 2;
        t.track_no = no;
        t.duration_ms = Some(ms);
        let t = lib.upsert_track(&t).unwrap();
        lib.add_source(&Source::new(t.id, SourceKind::LocalFile, format!("Music/{no}.opus"), Codec::Opus)).unwrap();
        t.id
    };
    let f: Vec<TrackId> = [
        (1, "Capital of the Unreturned", 156_957),
        (2, "Belaf's Lullaby", 199_950),
        (3, "Doll x Dragon", 162_911),
        (4, "Counterpoint of Abyss", 259_197),
        (5, "VOH", 459_782),
        (6, "Ouni", 202_299),
        (7, "I. AM. BURNING. MAN", 149_385),
        (8, "Bird x Sound", 202_017),
        (9, "Vueko", 216_310),
    ]
    .into_iter()
    .map(|(n, t, ms)| file(n, t, ms))
    .collect();
    let stream = |id: &str, title: &str, artist: &str, secs: Option<u64>| {
        let mut t = StreamTrack::new(RemoteId::new(Service::Youtube, id), title);
        t.artists = vec![ArtistRef::new(None, artist)];
        t.duration_ms = secs.map(|s| s * 1000);
        lib.ensure_stream_tracks(&[t]).unwrap()[0]
    };
    let belaf = stream("ytbelaf0001", "ベラフの子守歌", "Release", None);
    let doll = stream("ytdoll00001", "Ningyō x 龍", "Kevin Penkin", Some(163));
    let bird = stream("ytbird00001", "Tori x Onkyō", "Kevin Penkin", Some(203));
    let p = lib.create_playlist("Made in Abyss OST 3").unwrap();
    lib.playlist_add_many(p.id, &[f[0], belaf, doll, f[3], f[5], bird, f[8]]).unwrap();
    assert_eq!(lib.fold_playlist_gaps(p.id).unwrap(), 2);
    let ids: Vec<TrackId> = lib.playlist_entries(p.id, 0, 10).unwrap().iter().map(|e| e.track.id).collect();
    assert_eq!(ids, [f[0], f[1], f[2], f[3], f[5], bird, f[8]], "a gap of two places with one stream stays");
    assert_eq!(lib.track_aliases(f[1]).unwrap(), ["ベラフの子守歌"]);
    assert_eq!(lib.track_aliases(f[2]).unwrap(), ["Ningyō x 龍"]);
    assert_eq!(lib.track_by_remote(&RemoteId::new(Service::Youtube, "ytdoll00001")).unwrap(), Some(f[2]));
    assert_eq!(lib.get_track(f[1]).unwrap().unwrap().title, "Belaf's Lullaby");

    // What stays.
    let long = stream("ytlong00001", "ベラフの子守歌 (Extended)", "Kevin Penkin", Some(300));
    let latin = stream("ytlatin0001", "Lullaby", "Someone", None);
    let again = stream("ytagain0001", "ベラフの子守歌", "Release", None);
    for (name, list, why) in [
        ("Long", vec![f[0], long, f[2]], "another length"),
        ("Latin", vec![f[2], latin, f[4]], "a title to compare"),
        ("Back", vec![f[3], again, f[1]], "the other way round"),
        ("Has it", vec![f[0], again, f[2], f[1]], "the file is in the playlist already"),
    ] {
        let q = lib.create_playlist(name).unwrap();
        lib.playlist_add_many(q.id, &list).unwrap();
        assert_eq!(lib.fold_playlist_gaps(q.id).unwrap(), 0, "{why}");
    }
    for t in [long, latin, again] {
        assert!(lib.get_track(t).unwrap().is_some());
    }
}

/// A file that has the stream's service already is no file of another
/// stream of it under another title.
#[test]
fn a_file_with_the_service_already_takes_no_stream_in_another_script() {
    let lib = lib();
    let ost = "MADE IN ABYSS ORIGINAL SOUNDTRACK 3";
    let belaf = file_track(&lib, "Belaf's Lullaby", &["Kevin Penkin"], ost, 199_950);
    let mut own = StreamTrack::new(RemoteId::new(Service::Youtube, "ytownbelaf1"), "Belaf's Lullaby");
    own.artists = vec![ArtistRef::new(None, "Kevin Penkin")];
    own.duration_ms = Some(200_000);
    lib.attach_stream_source(belaf, &own).unwrap();
    let mut jp = StreamTrack::new(RemoteId::new(Service::Youtube, "ytbelaf0001"), "ベラフの子守歌");
    jp.artists = vec![ArtistRef::new(None, "Kevin Penkin")];
    jp.album = Some(AlbumRef::new(None, ost));
    jp.duration_ms = Some(200_000);
    let jp = lib.ensure_stream_tracks(&[jp]).unwrap()[0];
    lib.like(jp).unwrap();
    assert_eq!(lib.merge_other_script_titles().unwrap(), 0);
    assert!(lib.get_track(jp).unwrap().is_some());
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

/// «MADE IN ABYSS ORIGINAL SOUNDTRACK 4» (20 songs) as the library had it:
/// two of its files, and the same two songs as YouTube Music names them,
/// added as stream tracks of the release at the same places:
/// `(release, [file 4, file 8], [stream 4, stream 8])`.
fn abyss(lib: &Library) -> (ReleaseId, [TrackId; 2], [TrackId; 2]) {
    let kevin = lib.upsert_artist(&Artist::new("Kevin Penkin")).unwrap();
    let immi = lib.upsert_artist(&Artist::new("immi")).unwrap();
    let mut release = Release::new(
        "MADE IN ABYSS ORIGINAL SOUNDTRACK 4",
        ArtistCredit::single("Kevin Penkin", Some(kevin.id)),
    );
    release.total_tracks = Some(20);
    let release = lib.upsert_release(&release).unwrap();
    let mut pair = ArtistCredit::single("Kevin Penkin", Some(kevin.id));
    pair.push(" & ", "immi", Some(immi.id));
    let files = [
        abyss_file(lib, release.id, "KATSURAKUTEI", ArtistCredit::single("Kevin Penkin", Some(kevin.id)), 4, 124_853),
        abyss_file(lib, release.id, "RIKO NIKUNABE", pair, 8, 141_669),
    ];
    let stream = |id: &str, title: &str, artist: &str, no: u32, ms: u64| {
        let t = separate(lib, &yt(id, title, artist, ms, false));
        // The credit as YouTube gave it: one name for the pair.
        let credit = format!(r#"[{{"artist_id":null,"name":"{artist}","join_phrase":""}}]"#);
        lib.write(|tx| {
            tx.execute(
                "UPDATE track SET release_id = ?2, disc_no = 1, track_no = ?3, artist_credit = ?4 WHERE id = ?1",
                params![t.to_string(), release.id.to_string(), no, credit],
            )?;
            Ok(())
        })
        .unwrap();
        t
    };
    let streams = [
        stream("feFFauF1GWk", "滑落亭", "Kevin Penkin", 4, 125_000),
        stream("aZY3gguR2uM", "Riko肉鍋", "Kevin Penkin、immi", 8, 142_000),
    ];
    (release.id, files, streams)
}

fn abyss_file(lib: &Library, release: ReleaseId, title: &str, credit: ArtistCredit, no: u32, ms: u32) -> TrackId {
    let mut t = Track::new(title, credit);
    t.release_id = Some(release);
    t.disc_no = 1;
    t.track_no = no;
    t.duration_ms = Some(ms);
    let t = lib.upsert_track(&t).unwrap();
    lib.add_source(&Source::new(t.id, SourceKind::LocalFile, format!("Music/{release}/{no:02} {title}.opus"), Codec::Opus))
        .unwrap();
    t.id
}

#[test]
fn one_place_of_one_release_is_one_song_whatever_the_titles() {
    let place = Some(("abyss-4".to_owned(), 1, 4));
    let file = Song::new("KATSURAKUTEI", "Kevin Penkin", None, Some(124_853), None).at(place.clone());
    let service = Song::new("滑落亭", "Kevin Penkin", None, Some(125_000), None).at(place.clone());
    assert!(file.same(&service));
    assert!(!file.same(&Song { place: None, ..service.clone() }), "no place, no rule");
    let elsewhere = Song { place: Some(("abyss-4".into(), 1, 5)), ..service.clone() };
    assert!(!file.same(&elsewhere), "another number");
    let other_album = Song { place: Some(("abyss-3".into(), 1, 4)), ..service.clone() };
    assert!(!file.same(&other_album), "another release");
    let far = Song { duration_ms: Some(127_000), ..service.clone() };
    assert!(!file.same(&far), "2.1 s apart");
    let stranger = Song::new("滑落亭", "Someone Else", None, Some(125_000), None).at(place.clone());
    assert!(!file.same(&stranger), "no shared artist");
    let isrc = Song { isrc: Some("ZZFAK9900009".into()), ..service.clone() };
    let both = Song { isrc: Some("ZZFAK9900001".into()), ..file.clone() };
    assert!(!both.same(&isrc), "two different ISRCs");
    // The credits overlap name by name.
    let place8 = Some(("abyss-4".to_owned(), 1, 8));
    let file = Song::new("RIKO NIKUNABE", "Kevin Penkin", None, Some(141_669), None)
        .with_names(&["Kevin Penkin", "immi"])
        .at(place8.clone());
    let service = Song::new("Riko肉鍋", "Kevin Penkin、immi", None, Some(142_000), None)
        .with_names(&["Kevin Penkin、immi"])
        .at(place8.clone());
    assert!(file.same(&service) && service.same(&file));
    let guest = Song::new("Riko肉鍋", "immi", None, Some(142_000), None).at(place8);
    assert!(file.same(&guest), "a guest of the file is an artist of it");
    assert_eq!(same::credit_keys("Kevin Penkin、immi"), ["kevin penkin", "immi"]);
    assert_eq!(same::credit_keys("A × B feat. C"), ["a", "b", "c"]);
    assert_eq!(same::main_artist_key("Kevin Penkin、immi"), "kevin penkin");
}

#[test]
fn the_v5_repair_folds_the_service_titles_into_the_files_at_their_places() {
    let lib = lib();
    let (release, files, streams) = abyss(&lib);
    // Two files at one place stay two: a wrong tag is no duplicate.
    let kevin = ArtistCredit::single("Kevin Penkin", None);
    let lullaby = abyss_file(&lib, release, "Gaburoon Memory Lullaby", kevin.clone(), 9, 83_963);
    let twin = abyss_file(&lib, release, "Gaburoon Memory Lullaby (Inst.)", kevin, 9, 83_500);
    lib.like(streams[0]).unwrap();
    let p = lib.create_playlist("Бездна").unwrap();
    lib.playlist_add(p.id, streams[1]).unwrap();
    let count = |lib: &Library| -> i64 {
        lib.read(|c| {
            Ok(c.query_row(
                "SELECT count(*) FROM track WHERE release_id = ?1 AND deleted_at IS NULL",
                [release.to_string()],
                |r| r.get(0),
            )?)
        })
        .unwrap()
    };
    assert_eq!(count(&lib), 6, "22 of 20 in small");
    // A library repaired by v4 is repaired again.
    lib.write(|tx| meta_set(tx, "stream_duplicates_merged.v4", "1")).unwrap();

    assert_eq!(lib.merge_duplicate_stream_tracks().unwrap(), 2);
    assert_eq!(count(&lib), 4);
    for (file, stream, video) in [(files[0], streams[0], "feFFauF1GWk"), (files[1], streams[1], "aZY3gguR2uM")] {
        assert!(lib.get_track(stream).unwrap().is_none(), "{video}: merged");
        let sources = lib.sources_of_track(file).unwrap();
        assert!(sources.iter().any(|s| s.kind == SourceKind::Youtube && s.locator == video), "{video}: its source moved");
        assert_eq!(lib.active_source(file).unwrap().unwrap().kind, SourceKind::LocalFile, "the file plays");
    }
    assert!(lib.is_liked(files[0]).unwrap(), "the like moved");
    let entries = lib.playlist_entries(p.id, 0, 10).unwrap();
    assert_eq!(entries.iter().map(|e| e.track.id).collect::<Vec<_>>(), [files[1]], "the playlist entry moved");
    assert_eq!(lib.get_track_row(files[0]).unwrap().unwrap().title, "KATSURAKUTEI", "the file keeps its title");
    assert!(lib.get_track(lullaby).unwrap().is_some() && lib.get_track(twin).unwrap().is_some(), "two files stay two");
    let rows = lib.tracks_of_release_all(release).unwrap();
    assert_eq!(
        rows.iter().map(|r| (r.track_no, r.title.as_str())).collect::<Vec<_>>(),
        [(4, "KATSURAKUTEI"), (8, "RIKO NIKUNABE"), (9, "Gaburoon Memory Lullaby"), (9, "Gaburoon Memory Lullaby (Inst.)")]
    );
    let progress = lib.release_progress(release).unwrap().unwrap();
    assert_eq!((progress.local, progress.total), (4, 20));
    assert_eq!(lib.merge_duplicate_stream_tracks().unwrap(), 0, "once");
}

/// «MADE IN ABYSS ORIGINAL SOUNDTRACK 3» on two discs: YouTube Music
/// numbers them as one, so its 4 and 5 are the files 2-1 and 2-2, and its
/// «ベラフの子守歌» credits the character who sings it, not the composer.
#[test]
fn a_service_numbering_the_discs_as_one_finds_the_files_of_the_second() {
    let lib = lib();
    let kevin = lib.upsert_artist(&Artist::new("Kevin Penkin")).unwrap();
    let credit = || ArtistCredit::single("Kevin Penkin", Some(kevin.id));
    let release = lib
        .upsert_release(&Release::new("MADE IN ABYSS ORIGINAL SOUNDTRACK 3", credit()))
        .unwrap()
        .id;
    abyss_file(&lib, release, "Tomorrowland", credit(), 1, 120_000);
    // As long as the songs give or take three seconds: no length alone
    // tells them apart.
    abyss_file(&lib, release, "Eyes of Irumyuui", credit(), 2, 199_561);
    abyss_file(&lib, release, "Mallets of Abyss", credit(), 3, 165_984);
    let belaf = abyss_file(&lib, release, "Belaf's Lullaby", credit(), 1, 199_950);
    let doll = abyss_file(&lib, release, "Doll x Dragon", credit(), 2, 162_911);
    lib.write(|tx| {
        tx.execute("UPDATE track SET disc_no = 2 WHERE id IN (?1, ?2)", params![belaf.to_string(), doll.to_string()])?;
        Ok(())
    })
    .unwrap();
    let stream = |id: &str, title: &str, credit: &str, no: u32, ms: u64| {
        let t = separate(&lib, &yt(id, title, "Kevin Penkin", ms, false));
        lib.write(|tx| {
            tx.execute(
                "UPDATE track SET release_id = ?2, disc_no = 1, track_no = ?3, artist_credit = ?4 WHERE id = ?1",
                params![t.to_string(), release.to_string(), no, credit],
            )?;
            Ok(())
        })
        .unwrap();
        t
    };
    let jp = stream(
        "BCBcTjDEnmM",
        "ベラフの子守歌",
        r#"[{"artist_id":null,"name":"ベラフ","join_phrase":"（CV："},{"artist_id":null,"name":"斎賀みつき","join_phrase":"）"}]"#,
        4,
        200_000,
    );
    let kevin_credit = r#"[{"artist_id":null,"name":"Kevin Penkin","join_phrase":""}]"#;
    let mixed = stream("Kxfcg5COTRg", "Ningyō x 龍", kevin_credit, 5, 163_000);
    // Past both discs: no file there.
    let past = stream("pastdiscs01", "VOH", kevin_credit, 6, 162_000);

    assert_eq!(lib.merge_duplicate_stream_tracks().unwrap(), 2);
    for (stream, file, video) in [(jp, belaf, "BCBcTjDEnmM"), (mixed, doll, "Kxfcg5COTRg")] {
        assert!(lib.get_track(stream).unwrap().is_none(), "{video}: merged");
        let sources = lib.sources_of_track(file).unwrap();
        assert!(sources.iter().any(|s| s.locator == video), "{video}: its source moved to the file");
    }
    assert!(lib.get_track(past).unwrap().is_some(), "a number past the discs stays");
}

#[test]
fn an_album_page_lists_each_place_once() {
    let lib = lib();
    let (release, files, streams) = abyss(&lib);
    // Before any repair: the service's titles at the places of the files
    // are not listed (and not counted) twice.
    let rows = lib.tracks_of_release_all(release).unwrap();
    assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), files);
    // A liked one is listed: it is in the library.
    lib.like(streams[1]).unwrap();
    assert_eq!(lib.tracks_of_release_all(release).unwrap().len(), 3);
}

#[test]
fn a_tracklist_from_a_service_lands_on_the_files_at_their_places() {
    let lib = lib();
    let (release, files, streams) = abyss(&lib);
    // As before the whole tracklist was added: only the files.
    for t in streams {
        lib.write(|tx| {
            tx.execute("DELETE FROM source WHERE track_id = ?1", [t.to_string()])?;
            tx.execute("DELETE FROM external_id WHERE local_id = ?1", [t.to_string()])?;
            tx.execute("DELETE FROM track WHERE id = ?1", [t.to_string()])?;
            Ok(())
        })
        .unwrap();
    }
    let artists = vec![nikgob_core::streaming::ArtistRef::new(None, "Kevin Penkin")];
    let mut album = StreamAlbum {
        id: RemoteId::new(Service::Youtube, "MPREb_abyss4"),
        title: "MADE IN ABYSS ORIGINAL SOUNDTRACK 4".into(),
        artists: artists.clone(),
        year: Some(2026),
        release_date: None,
        release_type: Default::default(),
        cover_url: None,
        total_tracks: Some(20),
        tracks: Vec::new(),
    };
    for (no, id, title, artist, ms) in [
        (3, "yt-3", "Cravagli", "Kevin Penkin", 126_000),
        (4, "feFFauF1GWk", "滑落亭", "Kevin Penkin", 125_000),
        (8, "aZY3gguR2uM", "Riko肉鍋", "Kevin Penkin、immi", 142_000),
    ] {
        let mut t = StreamTrack::new(RemoteId::new(Service::Youtube, id), title);
        t.artists = vec![nikgob_core::streaming::ArtistRef::new(None, artist)];
        t.track_no = Some(no);
        t.duration_ms = Some(ms);
        album.tracks.push(t);
    }
    assert_eq!(lib.adopt_stream_album(release, &album).unwrap(), 1, "only «Cravagli» is new");
    let rows = lib.tracks_of_release_all(release).unwrap();
    assert_eq!(
        rows.iter().map(|r| (r.track_no, r.title.as_str(), r.has_local)).collect::<Vec<_>>(),
        [(3, "Cravagli", false), (4, "KATSURAKUTEI", true), (8, "RIKO NIKUNABE", true)]
    );
    for (file, video) in [(files[0], "feFFauF1GWk"), (files[1], "aZY3gguR2uM")] {
        assert!(lib.sources_of_track(file).unwrap().iter().any(|s| s.locator == video), "{video} is a source of the file");
    }
    assert_eq!(lib.adopt_stream_album(release, &album).unwrap(), 0, "nothing twice");
}

/// A stream track of YouTube at a place of `release` (as an adopted
/// tracklist leaves it), credited as one name.
fn stream_at(lib: &Library, release: ReleaseId, id: &str, title: &str, artist: &str, no: u32, ms: u64) -> TrackId {
    let t = separate(lib, &yt(id, title, artist, ms, false));
    let credit = format!(r#"[{{"artist_id":null,"name":"{artist}","join_phrase":""}}]"#);
    lib.write(|tx| {
        tx.execute(
            "UPDATE track SET release_id = ?2, disc_no = 1, track_no = ?3, artist_credit = ?4 WHERE id = ?1",
            params![t.to_string(), release.to_string(), no, credit],
        )?;
        Ok(())
    })
    .unwrap();
    t
}

fn release_of(lib: &Library, title: &str, artist: &str) -> ReleaseId {
    let a = lib.upsert_artist(&Artist::new(artist)).unwrap();
    lib.upsert_release(&Release::new(title, ArtistCredit::single(artist, Some(a.id)))).unwrap().id
}

#[test]
fn a_twin_of_one_service_at_the_place_of_a_file_is_merged() {
    let lib = lib();
    let release = release_of(&lib, "HMU", "Tobias Dray");
    let file = abyss_file(&lib, release, "HMU", ArtistCredit::single("Tobias Dray", None), 1, 172_400);
    // The file already plays from YouTube under the album's video.
    lib.add_source(&Source::new(file, SourceKind::Youtube, "AFujPyUFepc", Codec::Opus)).unwrap();
    // The single's video, at the same place.
    let twin = stream_at(&lib, release, "1jfYg31Q8jw", "HMU", "Tobias Dray", 1, 173_000);
    // Another title at the place with the same service stays.
    let file2 = abyss_file(&lib, release, "Intro", ArtistCredit::single("Tobias Dray", None), 2, 60_000);
    lib.add_source(&Source::new(file2, SourceKind::Youtube, "yt-intro", Codec::Opus)).unwrap();
    let other = stream_at(&lib, release, "yt-other", "Outro", "Tobias Dray", 2, 60_500);

    assert_eq!(lib.merge_duplicate_stream_tracks().unwrap(), 1);
    assert!(lib.get_track(twin).unwrap().is_none());
    let sources = lib.sources_of_track(file).unwrap();
    assert!(sources.iter().any(|s| s.locator == "1jfYg31Q8jw") && sources.iter().any(|s| s.locator == "AFujPyUFepc"));
    assert_eq!(lib.active_source(file).unwrap().unwrap().kind, SourceKind::LocalFile);
    assert!(lib.get_track(other).unwrap().is_some(), "another title");
}

#[test]
fn one_title_at_one_place_needs_no_shared_artist() {
    let lib = lib();
    let release = release_of(&lib, "Clover", "大原ゆい子");
    let file = abyss_file(&lib, release, "Clover", ArtistCredit::single("Ohara Yuiko", None), 1, 260_049);
    let stream = stream_at(&lib, release, "DisOzS-ZjPQ", "Clover", "大原ゆい子", 1, 260_000);
    // Not the song: too far apart, another place, another title.
    let far = abyss_file(&lib, release, "Kimi no Kioku", ArtistCredit::single("Ohara Yuiko", None), 2, 240_000);
    let far_stream = stream_at(&lib, release, "yt-kioku", "Kimi no Kioku", "大原ゆい子", 2, 242_000);
    let moved = stream_at(&lib, release, "yt-moved", "Clover", "大原ゆい子", 3, 260_000);
    let other = stream_at(&lib, release, "yt-other", "Hanabi", "大原ゆい子", 1, 260_000);

    assert_eq!(lib.merge_duplicate_stream_tracks().unwrap(), 1);
    assert!(lib.get_track(stream).unwrap().is_none());
    assert!(lib.sources_of_track(file).unwrap().iter().any(|s| s.locator == "DisOzS-ZjPQ"));
    for t in [far_stream, moved, other] {
        assert!(lib.get_track(t).unwrap().is_some(), "{t} stays");
    }
    assert!(lib.get_track(far).unwrap().is_some());

    let place = Some(("clover".to_owned(), 1, 1));
    let a = Song::new("Clover", "Ohara Yuiko", None, Some(260_049), None).at(place.clone());
    let b = Song::new("Clover", "大原ゆい子", None, Some(260_000), None).at(place.clone());
    assert!(a.same(&b) && a.twin(&b));
    assert!(!a.same(&Song { duration_ms: Some(262_000), ..b.clone() }), "2 s apart");
    assert!(!a.same(&Song { place: Some(("clover".into(), 1, 2)), ..b.clone() }), "another place");
    let inst = Song::new("Clover (Instrumental)", "大原ゆい子", None, Some(260_000), None).at(place);
    assert!(!a.same(&inst), "a version");
}

#[test]
fn one_song_filed_twice_under_a_sound_mark_typo_is_one_track() {
    let lib = lib();
    let release = release_of(&lib, "Mushoku Tensei OST 1 Special Edition", "Fujisawa Yoshiaki");
    let by = ArtistCredit::single("Fujisawa Yoshiaki", None);
    let paulo = abyss_file(&lib, release, "パウロ VS ルディ", by.clone(), 19, 135_602);
    let ghislaine = abyss_file(&lib, release, "ギレーヌと二人きり", by.clone(), 19, 120_000);
    let baulo = abyss_file(&lib, release, "バウロ VS ルディ", by.clone(), 23, 135_602);
    // Two songs at one place, a version, a length too far: they stay.
    let piano = abyss_file(&lib, release, "パウロ VS ルディ (Piano)", by.clone(), 30, 135_602);
    let late = abyss_file(&lib, release, "ハウロ VS ルディ", by, 31, 135_700);

    assert_eq!(lib.merge_duplicate_stream_tracks().unwrap(), 1);
    assert!(lib.get_track(paulo).unwrap().is_none(), "the one sharing its place went");
    let sources = lib.sources_of_track(baulo).unwrap();
    assert_eq!(sources.iter().filter(|s| s.kind == SourceKind::LocalFile).count(), 2, "the second file is a spare copy");
    for t in [ghislaine, piano, late] {
        assert!(lib.get_track(t).unwrap().is_some());
    }
}

#[test]
fn a_stream_of_a_library_album_does_not_go_into_a_file_of_another_album() {
    let lib = lib();
    // A stream at its place on an album with files, the file of the song
    // on another album.
    let album = release_of(&lib, "El Camino", "The Black Keys");
    abyss_file(&lib, album, "Lonely Boy", ArtistCredit::single("The Black Keys", None), 1, 193_000);
    let stream = stream_at(&lib, album, "yt-dead", "Dead and Gone", "The Black Keys", 2, 221_000);
    let other = release_of(&lib, "Dead and Gone", "The Black Keys");
    let file = abyss_file(&lib, other, "Dead and Gone", ArtistCredit::single("The Black Keys", None), 1, 221_400);
    lib.merge_duplicate_stream_tracks().unwrap();
    assert!(lib.get_track(stream).unwrap().is_some(), "its place stays");
    assert!(!lib.sources_of_track(file).unwrap().iter().any(|s| s.locator == "yt-dead"));
}

/// A YouTube album of `tracks` (`(no, video, title, ms)`) by `artist`.
fn yt_album(id: &str, title: &str, artist: &str, tracks: &[(u32, &str, &str, u64)]) -> StreamAlbum {
    let artists = vec![nikgob_core::streaming::ArtistRef::new(None, artist)];
    StreamAlbum {
        id: RemoteId::new(Service::Youtube, id),
        title: title.into(),
        artists: artists.clone(),
        year: Some(2020),
        release_date: None,
        release_type: Default::default(),
        cover_url: None,
        total_tracks: Some(tracks.len() as u32),
        tracks: tracks
            .iter()
            .map(|(no, video, title, ms)| {
                let mut t = StreamTrack::new(RemoteId::new(Service::Youtube, *video), *title);
                t.artists = artists.clone();
                t.track_no = Some(*no);
                t.duration_ms = Some(*ms);
                t
            })
            .collect(),
    }
}

/// `(number, title)` of every live row of `release`.
fn places(lib: &Library, release: ReleaseId) -> Vec<(u32, String)> {
    lib.read(|c| {
        let mut stmt = c.prepare(
            "SELECT track_no, title FROM track WHERE release_id = ?1 AND deleted_at IS NULL ORDER BY disc_no, track_no, seq",
        )?;
        let rows = stmt.query_map([release.to_string()], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    })
    .unwrap()
}

#[test]
fn an_adopted_tracklist_keeps_a_row_for_every_place() {
    let lib = lib();
    let by = ArtistCredit::single("Oliver Tree", None);
    let standard = release_of(&lib, "Ugly Is Beautiful", "Oliver Tree");
    let deluxe = release_of(&lib, "Ugly Is Beautiful: Shorter, Thicker & Uglier (Deluxe)", "Oliver Tree");
    let s1 = abyss_file(&lib, standard, "Me, Myself & I", by.clone(), 1, 177_000);
    let s3 = abyss_file(&lib, standard, "Cash Machine", by.clone(), 3, 159_000);
    let d1 = abyss_file(&lib, deluxe, "Me, Myself & I", by.clone(), 1, 177_000);
    let d2 = abyss_file(&lib, deluxe, "Alien Boy", by.clone(), 2, 160_000);
    let d3 = abyss_file(&lib, deluxe, "Cash Machine", by, 3, 159_000);
    // The deluxe's #2 already has the standard's video.
    lib.add_source(&Source::new(d2, SourceKind::Youtube, "yt-alien", Codec::Opus)).unwrap();
    lib.set_external_id(EntityKind::Track, &RemoteId::new(Service::Youtube, "yt-alien"), d2.0).unwrap();
    let album = yt_album(
        "MPREb_ugly",
        "Ugly Is Beautiful",
        "Oliver Tree",
        &[(1, "yt-me", "Me, Myself & I", 177_000), (2, "yt-alien", "Alien Boy", 160_000), (3, "yt-cash", "Cash Machine", 159_000)],
    );

    assert_eq!(lib.adopt_stream_album(standard, &album).unwrap(), 1, "#2 is new");
    assert_eq!(places(&lib, standard), [(1, "Me, Myself & I".into()), (2, "Alien Boy".into()), (3, "Cash Machine".into())]);
    assert_eq!(places(&lib, deluxe).len(), 3);
    let alien = lib.track_by_remote(&RemoteId::new(Service::Youtube, "yt-alien")).unwrap().unwrap();
    assert_eq!(lib.get_track(alien).unwrap().unwrap().release_id, Some(standard), "the stream came here");
    assert_eq!(lib.active_source(d2).unwrap().unwrap().kind, SourceKind::LocalFile, "the deluxe keeps its file");
    for (file, video) in [(s1, "yt-me"), (s3, "yt-cash")] {
        assert!(lib.sources_of_track(file).unwrap().iter().any(|s| s.locator == video), "{video} on the standard's file");
    }
    for t in [d1, d3] {
        assert!(!lib.sources_of_track(t).unwrap().iter().any(|s| s.kind == SourceKind::Youtube), "the deluxe gains nothing");
    }
    assert_eq!(lib.adopt_stream_album(standard, &album).unwrap(), 0, "nothing twice");
}

#[test]
fn an_album_id_of_an_empty_release_is_taken_over() {
    let lib = lib();
    let band = "FUNKY MONKEY BΛBY\u{2019}S";
    let empty = release_of(&lib, "Natsuko", band);
    lib.set_external_id(EntityKind::Album, &RemoteId::new(Service::Youtube, "MPREb_natsuko"), empty.0).unwrap();
    let release = release_of(&lib, "夏子", band);
    abyss_file(&lib, release, "夏子", ArtistCredit::single(band, None), 1, 212_800);
    let album = yt_album(
        "MPREb_natsuko",
        "Natsuko",
        band,
        &[(1, "yt-natsuko", "夏子 - Natsuko", 213_000), (2, "yt-inst", "夏子 (Instrumental)", 213_000)],
    );
    assert_eq!(lib.adopt_stream_album(release, &album).unwrap(), 1, "the instrumental");
    assert_eq!(lib.release_by_remote(&album.id).unwrap(), Some(release));
    assert!(lib.get_release(empty).unwrap().is_none(), "the empty one went");
}

#[test]
fn a_song_on_two_albums_is_found_on_the_stream_album_first() {
    let lib = lib();
    let by = ArtistCredit::single("藤澤慶昌", None);
    let ost = release_of(&lib, "Mushoku Tensei II Season 2 OST", "藤澤慶昌");
    let special = release_of(&lib, "Mushoku Tensei II OST 1 Special Edition", "藤澤慶昌");
    let a = abyss_file(&lib, ost, "Tabi no Hidarite", by.clone(), 1, 120_000);
    // (Another number: one file name per number.)
    let b = abyss_file(&lib, special, "Tabi no Hidarite", by, 2, 120_000);
    lib.set_external_id(EntityKind::Album, &RemoteId::new(Service::Youtube, "MPREb_special"), special.0).unwrap();
    let mut t = yt("yt-tabi", "Tabi no Hidarite", "藤澤慶昌", 120_000, false);
    t.album = Some(nikgob_core::streaming::AlbumRef {
        id: Some(RemoteId::new(Service::Youtube, "MPREb_special")),
        title: "Mushoku Tensei II OST 1 Special Edition".into(),
        artists: Vec::new(),
        year: None,
    });
    assert_eq!(lib.track_for_stream(&t).unwrap(), Some(b));
    t.album = None;
    assert_eq!(lib.track_for_stream(&t).unwrap(), Some(a), "without an album: the older file");
}

#[test]
fn versions_clips_and_other_artists_never_merge() {
    let lib = lib();
    let album = release_of(&lib, "Album", "Artist");
    let by = ArtistCredit::single("Artist", None);
    let song = abyss_file(&lib, album, "7 weeks & 3 days", by.clone(), 1, 200_000);
    let mut kept = Vec::new();
    for (id, title, ms) in [
        ("yt-inst", "7 weeks & 3 days (Instrumental)", 200_000),
        ("yt-live", "7 weeks & 3 days (Live)", 201_000),
        ("yt-piano", "7 weeks & 3 days (Piano Version)", 200_500),
    ] {
        let t = separate(&lib, &yt(id, title, "Artist", ms, false));
        lib.like(t).unwrap();
        kept.push(t);
    }
    // A clip much longer than its song, «Лёд» by another artist.
    let her = abyss_file(&lib, album, "her", by.clone(), 3, 200_000);
    let clip = separate(&lib, &yt("yt-her", "her (Official Video)", "Artist", 276_000, true));
    lib.like(clip).unwrap();
    kept.push(clip);
    let ice = abyss_file(&lib, album, "Лёд", by, 2, 180_000);
    let other = separate(&lib, &yt("yt-ice", "Лёд", "Другой", 180_000, false));
    lib.like(other).unwrap();
    kept.push(other);
    assert_eq!(lib.merge_duplicate_stream_tracks().unwrap(), 0);
    for t in kept.into_iter().chain([song, ice, her]) {
        assert!(lib.get_track(t).unwrap().is_some(), "{t} stays");
    }
}

/// «Скачать» on a stream track, then the import cancelled: its file came
/// and was taken back. The track leaves the library again, no row of it
/// is left there without a file.
#[test]
fn a_file_taken_back_takes_its_stream_track_out_of_the_library() {
    let lib = lib();
    let id = lib.upsert_stream_track(&sp(0)).unwrap();
    assert_eq!(lib.track_in_library(id).unwrap(), Some(false));
    let file = lib.add_source(&Source::new(id, SourceKind::LocalFile, "Music/Кино/01 Группа крови.flac", Codec::Flac)).unwrap();
    assert_eq!(lib.track_in_library(id).unwrap(), Some(true), "a file brings it in");
    lib.forget_source(file.id).unwrap();
    assert_eq!(lib.track_in_library(id).unwrap(), Some(false), "out again with its file");
    assert_eq!(listed(&lib), (0, 0, 0, 0));
    assert_eq!(lib.active_source(id).unwrap().unwrap().kind, SourceKind::Spotify);
}

#[test]
fn the_active_source_follows_the_kind_priority() {
    // The active source (the track's «стрим · …» label) is ranked the way
    // playback ranks sources: YouTube > SoundCloud > Deezer.
    let lib = lib();
    let id = lib
        .upsert_stream_track(&find(&FakeService::youtube(), "yt-gruppa-1"))
        .unwrap();
    let youtube = lib.active_source(id).unwrap().unwrap();
    assert_eq!(youtube.kind, SourceKind::Youtube);
    for (kind, locator) in [(SourceKind::Deezer, "dz-1"), (SourceKind::Soundcloud, "sc-1")] {
        let mut s = nikgob_core::model::Source::new(id, kind, locator, Codec::Flac);
        s.quality_score = 1_000;
        lib.add_source(&s).unwrap();
    }
    lib.set_active_source(id).unwrap();
    assert_eq!(lib.active_source(id).unwrap().unwrap().kind, SourceKind::Youtube);
    lib.set_source_available(youtube.id, false).unwrap();
    lib.set_active_source(id).unwrap();
    assert_eq!(lib.active_source(id).unwrap().unwrap().kind, SourceKind::Soundcloud);
}

#[test]
fn an_adopted_tracklist_brings_the_albums_year() {
    use nikgob_core::model::{ArtistCredit, Release, Track};
    let lib = lib();
    let album = FakeService::youtube()
        .albums()
        .into_iter()
        .find(|a| a.id.id == "yt-alb-gruppa")
        .unwrap();
    let release = |title: &str, year: Option<i32>| {
        let mut r = Release::new(title, ArtistCredit::single("Кино", None));
        r.year = year;
        let r = lib.upsert_release(&r).unwrap();
        let mut t = Track::new("Группа крови", ArtistCredit::single("Кино", None));
        t.release_id = Some(r.id);
        t.track_no = 1;
        lib.upsert_track(&t).unwrap();
        r.id
    };
    let bare = release("Группа крови", None);
    lib.adopt_stream_album(bare, &album).unwrap();
    assert_eq!(lib.get_release(bare).unwrap().unwrap().year, Some(1988));
    // A year the album has stays.
    let dated = release("Группа крови (1990)", Some(1990));
    lib.adopt_stream_album(dated, &album).unwrap();
    assert_eq!(lib.get_release(dated).unwrap().unwrap().year, Some(1990));
    assert!(!lib.fill_release_year(dated, Some(1988)).unwrap());
    let other = release("Другой", None);
    assert!(lib.fill_release_year(other, Some(2001)).unwrap());
    assert_eq!(lib.get_release(other).unwrap().unwrap().year, Some(2001));
    assert!(!lib.fill_release_year(other, None).unwrap());
}

#[test]
fn found_tracklists_of_albums_without_a_year_are_asked_again_once() {
    use nikgob_core::model::{ArtistCredit, Release};
    let lib = lib();
    let bare = lib.upsert_release(&Release::new("A", ArtistCredit::single("X", None))).unwrap();
    let mut r = Release::new("B", ArtistCredit::single("X", None));
    r.year = Some(2020);
    let dated = lib.upsert_release(&r).unwrap();
    let missed = lib.upsert_release(&Release::new("C", ArtistCredit::single("X", None))).unwrap();
    let now = crate::now_ms();
    lib.cache_put(&format!("tracklist.{}", bare.id), now, "found").unwrap();
    lib.cache_put(&format!("tracklist.{}", dated.id), now, "found").unwrap();
    lib.cache_put(&format!("tracklist.{}", missed.id), now, "miss").unwrap();
    assert_eq!(lib.ask_album_years_again().unwrap(), 1);
    assert!(lib.cache_get(&format!("tracklist.{}", bare.id)).unwrap().is_none());
    assert!(lib.cache_get(&format!("tracklist.{}", dated.id)).unwrap().is_some());
    assert!(lib.cache_get(&format!("tracklist.{}", missed.id)).unwrap().is_some());
    // Once.
    lib.cache_put(&format!("tracklist.{}", bare.id), now, "found").unwrap();
    assert_eq!(lib.ask_album_years_again().unwrap(), 0);
}

/// «夏子» as the library had it: the file of its first song (the album
/// counts three), and a release «Natsuko» a radio made of the same album's
/// «SMILE» under YouTube Music's id. The album found under that id takes
/// the radio's release over: «SMILE» moves to «夏子», «Natsuko» goes. A
/// release the user kept something of is never taken.
#[test]
fn an_album_found_under_the_id_of_a_radios_release_takes_it_over() {
    let lib = lib();
    let file = file_track(&lib, "夏子", &["FUNKY MONKEY BΛBY'S"], "夏子", 212_800);
    let release = lib.get_track(file).unwrap().unwrap().release_id.unwrap();
    let artists = vec![nikgob_core::streaming::ArtistRef::new(None, "Funky Monkey Bλby's")];
    let album_ref = nikgob_core::streaming::AlbumRef::new(Some(RemoteId::new(Service::Youtube, "yt-natsuko")), "Natsuko");
    let song = |id: &str, title: &str, no: u32, ms: u64| {
        let mut t = StreamTrack::new(RemoteId::new(Service::Youtube, id), title);
        t.artists = artists.clone();
        t.album = Some(album_ref.clone());
        t.track_no = Some(no);
        t.duration_ms = Some(ms);
        t
    };
    let tracks = [
        song("yt-natsuko-1", "夏子 - Natsuko", 1, 213_000),
        song("yt-smile", "SMILE", 2, 158_000),
        song("yt-natsuko-3", "夏子 - Natsuko (Instrumental)", 3, 213_000),
    ];
    // The radio's «SMILE»: a release of its own, linked to the album id.
    let smile = lib.upsert_stream_track(&tracks[1]).unwrap();
    let radios = lib.get_track(smile).unwrap().unwrap().release_id.unwrap();
    assert_ne!(radios, release);
    assert_eq!(lib.release_by_remote(&RemoteId::new(Service::Youtube, "yt-natsuko")).unwrap(), Some(radios));
    let album = nikgob_core::streaming::StreamAlbum {
        id: RemoteId::new(Service::Youtube, "yt-natsuko"),
        title: "Natsuko".into(),
        artists: artists.clone(),
        year: Some(2026),
        release_date: None,
        release_type: Default::default(),
        cover_url: None,
        total_tracks: Some(3),
        tracks: tracks.to_vec(),
    };
    // Someone's song on it: left alone, as before.
    lib.like(smile).unwrap();
    assert_eq!(lib.adopt_stream_album(release, &album).unwrap(), 0);
    assert!(lib.get_release(radios).unwrap().is_some());
    lib.unlike(smile).unwrap();
    // Nobody's: taken over.
    assert_eq!(lib.adopt_stream_album(release, &album).unwrap(), 2, "«SMILE» and the instrumental");
    let rows = lib.tracks_of_release_all(release).unwrap();
    assert_eq!(rows.iter().map(|r| (r.track_no, r.has_local)).collect::<Vec<_>>(), [(1, true), (2, false), (3, false)]);
    assert_eq!(rows[0].id, file);
    assert_eq!(rows[1].id, smile, "the radio's track moved, not copied");
    assert!(lib.get_release(radios).unwrap().is_none(), "the radio's release went");
    assert_eq!(lib.release_by_remote(&album.id).unwrap(), Some(release));
    assert_eq!(lib.release_listing(release).unwrap().unwrap().holes(), 0, "no «Трек 2», «Трек 3»");
    assert_eq!(lib.track_in_library(smile).unwrap(), Some(false), "still nobody's song");
    assert_eq!(lib.adopt_stream_album(release, &album).unwrap(), 0, "nothing twice");
}
