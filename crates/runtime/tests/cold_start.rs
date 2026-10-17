//! Stage 3 part B on the fake services: cold-start imports from export files
//! (Spotify Extended Streaming History and Account data, Google Takeout
//! YouTube Music; fixtures in `tests/fixtures/exports`) and from the API,
//! their idempotency, playlist sync modes, artist discography.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_core::indexer::BoxFuture;
use nikgob_core::model::{
    Artist, ArtistCredit, Codec, Release, Source, SourceKind, Track, TrackId, WishlistOrigin,
    WishlistStatus,
};
use nikgob_core::streaming::{
    FakeService, PlaylistSyncMode, RemoteId, Service, StreamService, StreamTrack,
};
use nikgob_library::{Library, TrackFilter};
use nikgob_runtime::{NullPlayback, Runtime, RuntimeConfig};
use nikgob_view::{Command, Prefs, ViewState};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/exports")
}

fn start(lib: &Library, streams: Vec<Arc<dyn StreamService>>) -> Runtime {
    Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
            streams,
            library_dir: std::env::temp_dir(),
            prefs: Prefs::default(),
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    )
}

fn wait_for(rt: &Runtime, what: &str, f: impl Fn(&ViewState) -> bool) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(60) {
        if f(&rt.state()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    let s = rt.state();
    panic!(
        "timed out waiting for {what}: toasts {:?}, jobs {:?} / {:?}",
        s.toasts.iter().map(|t| &t.text).collect::<Vec<_>>(),
        s.downloads.active,
        s.downloads.history
    );
}

fn wait_until(what: &str, f: impl Fn() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(60), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(30));
    }
}

fn now<T>(f: BoxFuture<'_, T>) -> T {
    tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(f)
}

fn is_cold(title: &str) -> bool {
    title.starts_with("Импорт выгрузки") || title.starts_with("Импорт из")
}

/// Runs one import and waits for its history entry; returns its subtitle.
fn import(rt: &Runtime, c: Command) -> String {
    let before = rt.state().downloads.history.iter().filter(|j| is_cold(&j.title)).count();
    rt.send(c);
    wait_for(rt, "the import", |s| {
        s.downloads.history.iter().filter(|j| is_cold(&j.title)).count() > before
            && !s.downloads.active.iter().any(|j| is_cold(&j.title))
    });
    let s = rt.state();
    let job = s.downloads.history.iter().find(|j| is_cold(&j.title)).unwrap();
    assert_eq!(job.error, None, "{job:?}");
    job.subtitle.clone()
}

fn find(service: &FakeService, id: &str) -> StreamTrack {
    service.tracks().into_iter().find(|t| t.id.id == id).unwrap()
}

fn track_of(lib: &Library, service: Service, id: &str) -> Option<TrackId> {
    lib.track_by_remote(&RemoteId::new(service, id)).unwrap()
}

fn playlist_named(lib: &Library, name: &str) -> Option<nikgob_core::model::PlaylistId> {
    lib.list_playlists()
        .unwrap()
        .into_iter()
        .find(|p| p.name == name)
        .and_then(|p| p.id)
}

fn playlist_remotes(lib: &Library, p: nikgob_core::model::PlaylistId, service: Service) -> Vec<String> {
    lib.playlist_entries(p, 0, 1000)
        .unwrap()
        .iter()
        .filter_map(|e| {
            lib.external_ids(nikgob_core::streaming::EntityKind::Track, *e.track.id.as_uuid())
                .unwrap()
                .into_iter()
                .find(|r| r.service == service)
                .map(|r| r.id)
        })
        .collect()
}

/// What an import may have changed, to compare runs.
fn snapshot(lib: &Library) -> (u64, usize, usize, usize, u32) {
    (
        lib.count_events().unwrap(),
        lib.list_playlists().unwrap().len(),
        lib.list_wishlist().unwrap().len(),
        lib.liked_track_ids().unwrap().len(),
        lib.count_tracks(TrackFilter::All).unwrap(),
    )
}

#[test]
fn spotify_export_imports_history_likes_and_playlists_once() {
    let lib = Library::open_in_memory().unwrap();
    // Logged out: no like pull, no completing from the API.
    let rt = start(&lib, vec![Arc::new(FakeService::spotify().logged_out())]);
    let sub = import(&rt, Command::ImportExportFiles(fixtures().join("spotify")));
    assert!(sub.contains("новых: 3 прослушивания, 2 лайка, 1 плейлист"), "{sub}");

    // History: start + complete, start + skip, a local file (start +
    // complete); the podcast episode and the account-data history (the
    // extended one covers it) are left out.
    assert_eq!(lib.count_events().unwrap(), 6);
    let gruppa = track_of(&lib, Service::Spotify, "sp-gruppa-1").expect("a seen track");
    assert_eq!(lib.track_in_library(gruppa).unwrap(), Some(false), "history is not the library");
    assert!(!lib.is_liked(gruppa).unwrap());
    assert!(lib.wishlist_for_track(gruppa).unwrap().is_empty(), "history never wishlists");
    let ev = lib.events_of_track(gruppa, 10).unwrap();
    assert_eq!(ev.len(), 2);
    assert!(ev.iter().all(|e| e.device.as_str() == "import" && e.source_kind == Some(SourceKind::Spotify)));
    let local = lib.track_by_name("Local Artist", "Local Song").unwrap().expect("seen by name");
    assert_eq!(lib.track_in_library(local).unwrap(), Some(false));

    // Likes: stream tracks in the library, waiting in the wishlist.
    let joga = track_of(&lib, Service::Spotify, "sp-homogenic-2").unwrap();
    let michi = track_of(&lib, Service::Spotify, "sp-michi-1").unwrap();
    for t in [joga, michi] {
        assert!(lib.is_liked(t).unwrap());
        assert_eq!(lib.track_in_library(t).unwrap(), Some(true));
        let w = lib.wishlist_for_track(t).unwrap();
        assert_eq!(w.len(), 1);
        assert_eq!((w[0].status, w[0].origin), (WishlistStatus::Waiting, WishlistOrigin::Import));
    }
    assert_eq!(lib.liked_track_ids().unwrap().len(), 2, "the local file of YourLibrary is unknown");

    // The playlist, its tracks in the library and waiting too.
    let road = playlist_named(&lib, "Дорога").expect("the playlist");
    assert_eq!(playlist_remotes(&lib, road, Service::Spotify), ["sp-gruppa-2", "sp-firstlove-3"]);
    let door = track_of(&lib, Service::Spotify, "sp-gruppa-2").unwrap();
    assert_eq!(lib.wishlist_for_track(door).unwrap()[0].status, WishlistStatus::Waiting);
    assert!(lib.playlist_remote(road).unwrap().is_none(), "no remote id in the file");

    // Again: nothing twice. An unlike and a dismissal are respected.
    let before = snapshot(&lib);
    let sub = import(&rt, Command::ImportExportFiles(fixtures().join("spotify")));
    assert!(sub.contains("новых: ничего"), "{sub}");
    assert_eq!(snapshot(&lib), before);
    rt.send(Command::Like(nikgob_core::model::LikeTarget::Track(michi), false));
    wait_until("the unlike", || !lib.is_liked(michi).unwrap());
    let w = lib.wishlist_for_track(joga).unwrap()[0].id;
    rt.send(Command::WishlistDismiss(vec![w]));
    wait_until("the dismissal", || lib.wishlist_for_track(joga).unwrap()[0].status == WishlistStatus::Dismissed);
    import(&rt, Command::ImportExportFiles(fixtures().join("spotify")));
    assert!(!lib.is_liked(michi).unwrap(), "unliked stays unliked");
    let w = lib.wishlist_for_track(joga).unwrap();
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].status, WishlistStatus::Dismissed, "dismissed never comes back");
}

#[test]
fn a_zip_works_like_its_folder_and_tracks_are_completed_from_spotify() {
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("my_spotify_data.zip");
    {
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let root = fixtures().join("spotify");
        for (name, rel) in [
            ("Spotify Extended Streaming History/Streaming_History_Audio_2020_0.json", "Spotify Extended Streaming History/Streaming_History_Audio_2020_0.json"),
            ("MyData/YourLibrary.json", "MyData/YourLibrary.json"),
        ] {
            zip.start_file(name, opts).unwrap();
            std::io::Write::write_all(&mut zip, &std::fs::read(root.join(rel)).unwrap()).unwrap();
        }
        zip.finish().unwrap();
    }
    let lib = Library::open_in_memory().unwrap();
    let spotify = Arc::new(FakeService::spotify());
    let rt = start(&lib, vec![spotify.clone()]);
    import(&rt, Command::ImportExportFiles(zip_path));
    // 6 events of the file (the like pull at start-up adds none).
    assert_eq!(lib.count_events().unwrap(), 6);
    // "Aerodynamic" came from the file without its length; the service
    // completes it in the background.
    let aero = track_of(&lib, Service::Spotify, "sp-discovery-2").unwrap();
    wait_until("the completed track", || {
        lib.get_track(aero).unwrap().unwrap().duration_ms == Some(212_000)
    });
    assert_eq!(lib.get_track(aero).unwrap().unwrap().track_no, 2);
    assert!(lib.is_liked(track_of(&lib, Service::Spotify, "sp-michi-1").unwrap()).unwrap());
}

#[test]
fn account_data_history_matches_known_tracks_by_name() {
    let lib = Library::open_in_memory().unwrap();
    let known = lib.upsert_stream_track(&find(&FakeService::spotify(), "sp-gruppa-1")).unwrap();
    let tracks_before = lib.count_tracks(TrackFilter::All).unwrap();
    let rt = start(&lib, vec![]);
    import(&rt, Command::ImportExportFiles(fixtures().join("spotify-account")));
    // Кино — Группа крови lands on the known track; Аквариум is a new seen
    // track (two plays: one skipped, one complete); "Unknown Track" is
    // left out.
    assert_eq!(lib.events_of_track(known, 10).unwrap().len(), 2);
    let gorod = lib.track_by_name("Аквариум", "Город золотой").unwrap().expect("a seen track");
    let ev = lib.events_of_track(gorod, 10).unwrap();
    assert_eq!(ev.len(), 4);
    assert_eq!(ev.iter().filter(|e| e.event_type == nikgob_core::model::EventType::Skip).count(), 1);
    assert_eq!(lib.count_tracks(TrackFilter::All).unwrap(), tracks_before, "nothing new in the library");
    assert_eq!(lib.count_events().unwrap(), 6);
    import(&rt, Command::ImportExportFiles(fixtures().join("spotify-account")));
    assert_eq!(lib.count_events().unwrap(), 6, "idempotent");
}

#[test]
fn takeout_youtube_music_history_likes_and_playlists() {
    let lib = Library::open_in_memory().unwrap();
    let youtube = Arc::new(FakeService::youtube().logged_out());
    let rt = start(&lib, vec![youtube.clone()]);
    import(&rt, Command::ImportExportFiles(fixtures().join("takeout")));

    // Three YouTube Music plays (the YouTube video and the search are not).
    assert_eq!(lib.count_events().unwrap(), 3);
    let gruppa = track_of(&lib, Service::Youtube, "yt-gruppa-1").unwrap();
    let row = lib.get_track_row(gruppa).unwrap().unwrap();
    assert_eq!(row.title, "Группа крови");
    let digital = track_of(&lib, Service::Youtube, "yt-discovery-3").unwrap();
    assert_eq!(lib.events_of_track(digital, 10).unwrap().len(), 1, "a start, nothing else known");

    // Likes from the localised liked-music playlist, titled by the service.
    let bachelorette = track_of(&lib, Service::Youtube, "yt-homogenic-3").unwrap();
    assert!(lib.is_liked(bachelorette).unwrap());
    assert_eq!(lib.get_track(bachelorette).unwrap().unwrap().title, "Bachelorette");
    assert!(lib.is_liked(track_of(&lib, Service::Youtube, "yt-discovery-1").unwrap()).unwrap());
    assert_eq!(lib.liked_track_ids().unwrap().len(), 2);
    assert!(playlist_named(&lib, "Понравившаяся музыка").is_none());

    // The other playlist: a snapshot linked to its id.
    let evening = playlist_named(&lib, "Вечер").unwrap();
    assert_eq!(playlist_remotes(&lib, evening, Service::Youtube), ["yt-gruppa-1", "yt-firstlove-3"]);
    let link = lib.playlist_remote(evening).unwrap().unwrap();
    assert_eq!(link.sync_mode, PlaylistSyncMode::Once);
    assert_eq!(link.remote, RemoteId::new(Service::Youtube, "PLfakeEveningPlaylist01"));
    assert_eq!(lib.list_playlists().unwrap().len(), 2, "Любимые + Вечер");

    let before = snapshot(&lib);
    import(&rt, Command::ImportExportFiles(fixtures().join("takeout")));
    assert_eq!(snapshot(&lib), before, "idempotent");
}

#[test]
fn import_from_the_api_and_again() {
    let lib = Library::open_in_memory().unwrap();
    let spotify = Arc::new(FakeService::spotify());
    let rt = start(&lib, vec![spotify.clone()]);
    import(&rt, Command::ImportFromService(Service::Spotify));
    // Likes: whichever of the first pull and the import came first.
    assert_eq!(lib.liked_track_ids().unwrap().len(), 3);
    for id in spotify.liked_ids() {
        let t = track_of(&lib, Service::Spotify, &id).unwrap();
        let w = lib.wishlist_for_track(t).unwrap();
        assert_eq!(w.len(), 1, "{id}");
        assert_eq!(w[0].status, WishlistStatus::Waiting);
    }
    // Playlists, linked with the default mode (someone else's one is followed).
    let evening = playlist_named(&lib, "Вечер").unwrap();
    let link = lib.playlist_remote(evening).unwrap().unwrap();
    assert_eq!(link.sync_mode, PlaylistSyncMode::Follow);
    assert_eq!(link.remote_version.as_deref(), Some("1"));
    assert_eq!(playlist_remotes(&lib, evening, Service::Spotify).len(), 4);
    assert!(playlist_named(&lib, "Fake Mix").is_some());
    // Recent plays: events only (start + complete each).
    assert_eq!(lib.count_events().unwrap(), 6);
    let aero = track_of(&lib, Service::Spotify, "sp-discovery-2").unwrap();
    assert!(!lib.is_liked(aero).unwrap());
    assert_eq!(lib.track_in_library(aero).unwrap(), Some(false));

    let before = snapshot(&lib);
    let sub = import(&rt, Command::ImportFromService(Service::Spotify));
    assert!(sub.contains("новых: ничего"), "{sub}");
    assert_eq!(snapshot(&lib), before);
}

#[test]
fn followed_and_two_way_playlists() {
    let lib = Library::open_in_memory().unwrap();
    let spotify = Arc::new(FakeService::spotify());
    let rt = start(&lib, vec![spotify.clone()]);
    import(&rt, Command::ImportFromService(Service::Spotify));
    let evening = playlist_named(&lib, "Вечер").unwrap();
    let ids = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        playlist_remotes(&lib, evening, Service::Spotify),
        ids(&["sp-gruppa-1", "sp-firstlove-3", "sp-homogenic-2", "sp-discovery-3"])
    );

    // There: michi in, gruppa out. Here: discovery-4 in, firstlove-3 out.
    now(spotify.playlist_edit("sp-pl-evening", &ids(&["sp-michi-1"]), &ids(&["sp-gruppa-1"]))).unwrap();
    let hbfs = lib.upsert_stream_track(&find(&spotify, "sp-discovery-4")).unwrap();
    rt.send(Command::AddToPlaylist(evening, vec![hbfs]));
    let love = track_of(&lib, Service::Spotify, "sp-firstlove-3").unwrap();
    rt.send(Command::RemoveFromPlaylist(evening, love));
    wait_until("the local edits", || playlist_remotes(&lib, evening, Service::Spotify).len() == 4);

    // Follow: their edits come here, ours stay here.
    rt.send(Command::SetPlaylistSync { playlist: evening, mode: PlaylistSyncMode::Follow });
    wait_until("the followed edits", || {
        playlist_remotes(&lib, evening, Service::Spotify)
            == ids(&["sp-homogenic-2", "sp-discovery-3", "sp-discovery-4", "sp-michi-1"])
    });
    let remote = |s: &FakeService| -> Vec<String> {
        now(s.playlist("sp-pl-evening")).unwrap().tracks.into_iter().map(|t| t.id.id).collect()
    };
    assert_eq!(remote(&spotify), ids(&["sp-firstlove-3", "sp-homogenic-2", "sp-discovery-3", "sp-michi-1"]));

    // TwoWay: ours go there too (discovery-4 in, firstlove-3 out).
    rt.send(Command::SetPlaylistSync { playlist: evening, mode: PlaylistSyncMode::TwoWay });
    wait_until("the pushed edits", || {
        remote(&spotify) == ids(&["sp-homogenic-2", "sp-discovery-3", "sp-michi-1", "sp-discovery-4"])
    });
    wait_until("the new base", || {
        lib.playlist_remote(evening).unwrap().unwrap().remote_version.as_deref() == Some("3")
    });
    // Nothing more to do on the next sync: both sides agree.
    rt.send(Command::SetPlaylistSync { playlist: evening, mode: PlaylistSyncMode::TwoWay });
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(lib.playlist_remote(evening).unwrap().unwrap().remote_version.as_deref(), Some("3"));
    assert_eq!(playlist_remotes(&lib, evening, Service::Spotify).len(), 4);

    // Someone else's playlist cannot be pushed to: followed instead.
    let mix = playlist_named(&lib, "Fake Mix").unwrap();
    rt.send(Command::SetPlaylistSync { playlist: mix, mode: PlaylistSyncMode::TwoWay });
    wait_until("the downgrade", || {
        lib.playlist_remote(mix).unwrap().unwrap().sync_mode == PlaylistSyncMode::Follow
    });

    // Once: the link stays, nothing syncs; the view shows the mode.
    rt.send(Command::SetPlaylistSync { playlist: evening, mode: PlaylistSyncMode::Once });
    now(spotify.playlist_edit("sp-pl-evening", &ids(&["sp-gruppa-2"]), &[])).unwrap();
    rt.send(Command::LoadPlaylist(evening));
    wait_for(&rt, "the sync line", |s| {
        s.playlist.as_ref().and_then(|p| p.sync.as_ref()).is_some_and(|v| v.mode == PlaylistSyncMode::Once)
    });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(playlist_remotes(&lib, evening, Service::Spotify).len(), 4);
}

#[test]
fn a_local_playlist_becomes_two_way_with_its_stream_tracks_only() {
    let lib = Library::open_in_memory().unwrap();
    let spotify = Arc::new(FakeService::spotify());
    let a = lib.upsert_stream_track(&find(&spotify, "sp-gruppa-3")).unwrap();
    let b = lib.upsert_stream_track(&find(&spotify, "sp-homogenic-1")).unwrap();
    let mut local = Track::new("Дом", ArtistCredit::single("Я", None));
    local = lib.upsert_track(&local).unwrap();
    lib.add_source(&Source::new(local.id, SourceKind::LocalFile, "Music/dom.flac", Codec::Flac)).unwrap();
    let p = lib.create_playlist("Своё").unwrap();
    lib.playlist_add_many(p.id, &[a, local.id, b]).unwrap();
    let rt = start(&lib, vec![spotify.clone()]);
    rt.send(Command::SetPlaylistSync { playlist: p.id, mode: PlaylistSyncMode::TwoWay });
    wait_until("the remote playlist", || {
        lib.playlist_remote(p.id).unwrap().is_some_and(|l| l.synced_at.is_some() && l.remote_version.is_some())
    });
    let link = lib.playlist_remote(p.id).unwrap().unwrap();
    assert_eq!(link.remote.service, Service::Spotify);
    let remote: Vec<String> = now(spotify.playlist(&link.remote.id)).unwrap().tracks.into_iter().map(|t| t.id.id).collect();
    assert_eq!(remote, ["sp-gruppa-3", "sp-homogenic-1"], "the local file stays local");
    assert_eq!(lib.playlist_len(p.id).unwrap(), 3);
}

#[test]
fn discography_shows_the_albums_the_library_lacks() {
    let lib = Library::open_in_memory().unwrap();
    // A library artist known only by name (a local file of "First Love").
    let artist = lib.upsert_artist(&Artist::new("宇多田ヒカル")).unwrap();
    let credit = ArtistCredit::single("宇多田ヒカル", Some(artist.id));
    let mut release = Release::new("First Love", credit.clone());
    release.year = Some(1999);
    let release = lib.upsert_release(&release).unwrap();
    let mut t = Track::new("Automatic", credit);
    t.release_id = Some(release.id);
    let t = lib.upsert_track(&t).unwrap();
    lib.add_source(&Source::new(t.id, SourceKind::LocalFile, "Music/a.flac", Codec::Flac)).unwrap();

    // Spotify connected (a Spotify that is not is asked nothing).
    let streams: Vec<Arc<dyn StreamService>> = vec![
        Arc::new(FakeService::spotify()),
        Arc::new(FakeService::youtube().logged_out()),
    ];
    let rt = start(&lib, streams);
    rt.send(Command::LoadArtist(artist.id));
    wait_for(&rt, "the discography", |s| {
        s.artist.as_ref().is_some_and(|a| !a.discography.loading && !a.discography.albums.is_empty())
    });
    let s = rt.state();
    let d = &s.artist.as_ref().unwrap().discography;
    let titles: Vec<&str> = d.albums.iter().map(|a| a.title.as_str()).collect();
    assert_eq!(titles, ["道"], "First Love is in the library; YouTube's 道 is the same album");
    assert_eq!(d.albums[0].remote.service, Service::Spotify);
    drop(s);
    // The match is remembered for both services, and the result cached.
    let ids = lib.external_ids(nikgob_core::streaming::EntityKind::Artist, *artist.id.as_uuid()).unwrap();
    assert!(ids.contains(&RemoteId::new(Service::Spotify, "sp-art-utada")), "{ids:?}");
    assert!(ids.contains(&RemoteId::new(Service::Youtube, "yt-art-utada")), "{ids:?}");
    assert!(lib.cache_get(&format!("discography.{}", artist.id)).unwrap().is_some());
}
