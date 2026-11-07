//! Stage 3 part B on the fake services: cold-start imports from export files
//! (Spotify Extended Streaming History and Account data, Google Takeout
//! YouTube Music; fixtures in `tests/fixtures/exports`) and from the API,
//! their idempotency, playlist sync modes, artist discography.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_core::indexer::BoxFuture;
use nikgob_core::model::{
    Artist, ArtistCredit, Codec, Release, Source, SourceKind, Track, TrackId,
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

    // Likes: stream tracks in the library; nothing to download.
    let joga = track_of(&lib, Service::Spotify, "sp-homogenic-2").unwrap();
    let michi = track_of(&lib, Service::Spotify, "sp-michi-1").unwrap();
    for t in [joga, michi] {
        assert!(lib.is_liked(t).unwrap());
        assert_eq!(lib.track_in_library(t).unwrap(), Some(true));
    }
    assert_eq!(lib.liked_track_ids().unwrap().len(), 2, "the local file of YourLibrary is unknown");

    // The playlist, its tracks in the library.
    let road = playlist_named(&lib, "Дорога").expect("the playlist");
    assert_eq!(playlist_remotes(&lib, road, Service::Spotify), ["sp-gruppa-2", "sp-firstlove-3"]);
    assert!(lib.playlist_remote(road).unwrap().is_none(), "no remote id in the file");
    assert!(lib.list_wishlist().unwrap().is_empty(), "an import never touches the wishlist");

    // Again: nothing twice. An unlike is respected.
    let before = snapshot(&lib);
    let sub = import(&rt, Command::ImportExportFiles(fixtures().join("spotify")));
    assert!(sub.contains("новых: ничего"), "{sub}");
    assert_eq!(snapshot(&lib), before);
    rt.send(Command::Like(nikgob_core::model::LikeTarget::Track(michi), false));
    wait_until("the unlike", || !lib.is_liked(michi).unwrap());
    import(&rt, Command::ImportExportFiles(fixtures().join("spotify")));
    assert!(!lib.is_liked(michi).unwrap(), "unliked stays unliked");
    assert!(lib.list_wishlist().unwrap().is_empty());
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

    // Two plays of «- Topic» channels, the first heard out by the gap to
    // the next entry (the plain video, a search, a removed video are no
    // plays), as imported plays of the history.
    assert_eq!(lib.count_events().unwrap(), 3);
    let gruppa = track_of(&lib, Service::Youtube, "yt-gruppa-1").unwrap();
    let row = lib.get_track_row(gruppa).unwrap().unwrap();
    assert_eq!(row.title, "Группа крови");
    let ev = lib.events_of_track(gruppa, 10).unwrap();
    assert_eq!(ev.len(), 2);
    assert!(ev.iter().any(|e| e.event_type == nikgob_core::model::EventType::Complete));
    assert!(ev.iter().all(|e| e.origin == Some(nikgob_core::model::EventOrigin::Import)
        && e.context_id.as_deref() == Some("import:youtube_history")));
    assert!(track_of(&lib, Service::Youtube, "removedvid1").is_none());
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

/// Takeout files lying loose (no `Takeout/YouTube…` folders): the history
/// page, the music library, playlists named through `playlists.csv`; a
/// playlist of plain videos and «Watch later» stay out.
#[test]
fn loose_takeout_files_with_the_html_history_and_the_music_library() {
    let dir = tempfile::tempdir().unwrap();
    let put = |name: &str, text: &str| std::fs::write(dir.path().join(name), text).unwrap();
    let cell = |header: &str, body: &str| {
        format!(
            "<div class=\"outer-cell mdl-cell mdl-cell--12-col mdl-shadow--2dp\"><div class=\"mdl-grid\">\
             <div class=\"header-cell mdl-cell mdl-cell--12-col\"><p class=\"mdl-typography--title\">{header}<br></p></div>\
             <div class=\"content-cell mdl-cell mdl-cell--6-col mdl-typography--body-1\">{body}</div></div></div>"
        )
    };
    let watched = |id: &str, title: &str| {
        format!(
            "Watched\u{a0}<a href=\"https://www.youtube.com/watch?v={id}\">{title}</a><br>\
             <a href=\"https://www.youtube.com/channel/UCyyyyyyyyyyyyyyyyyyyyyy\">Blogger</a><br>Oct 15, 2026, 3:00:00\u{202f}PM IDT<br>"
        )
    };
    let mut cells = vec![cell(
        "YouTube Music",
        "Watched\u{a0}<a href=\"https://music.youtube.com/watch?v=yt-gruppa-1\">Группа крови</a><br>\
         <a href=\"https://www.youtube.com/channel/UCxxxxxxxxxxxxxxxxxxxxxx\">Кино - Topic</a><br>Oct 15, 2026, 4:55:46\u{202f}PM IDT<br>",
    )];
    for id in ["plainvid-01", "plainvid-02", "plainvid-03"] {
        cells.push(cell("YouTube", &watched(id, "VR setup")));
    }
    put(
        "watch-history.html",
        &format!("<html><head><style>.mdl-grid{{}}</style></head><body>{}</body></html>", cells.concat()),
    );
    put(
        "music library songs.csv",
        "Video ID,Song Title,Album Title,Artist Name 1,Artist Name 2\n yt-homogenic-3,Bachelorette,Homogenic,Björk,\n",
    );
    put(
        "playlists.csv",
        "Playlist ID,Add new videos to top,Playlist Title (Original),Playlist Visibility\n\
         PLfakeEveningPlaylist01,False,Вечер,Private\n\
         PLfakeVrPlaylist0000001,False,vr,Private\n\
         PLfakeWatchLater0000001,False,Watch later,Private\n",
    );
    let list = |ids: &[&str]| {
        let rows: String = ids.iter().map(|id| format!("{id},2026-01-01T00:00:00+00:00\n")).collect();
        format!("Video ID,Playlist Video Creation Timestamp\n{rows}")
    };
    put("Вечер-videos.csv", &list(&["yt-gruppa-1", "yt-firstlove-3"]));
    put("vr-videos.csv", &list(&["plainvid-01", "plainvid-02", "plainvid-03"]));
    put("Watch later-videos.csv", &list(&["yt-discovery-1"]));
    put("saves data.csv", "Title,Playable ID,Timestamp\nClean It: Restaurant Cleanup,Ugkxg96n,2025-11-09T00:16:16+00:00\n");

    let lib = Library::open_in_memory().unwrap();
    let youtube = Arc::new(FakeService::youtube().logged_out());
    let rt = start(&lib, vec![youtube.clone()]);
    let subtitle = import(&rt, Command::ImportExportFiles(dir.path().to_path_buf()));
    assert!(subtitle.ends_with(" · пропущен 1 плейлист не с музыкой"), "{subtitle}");
    assert!(
        rt.state().toasts.iter().any(|t| t.text == "Импорт: 1 прослушивание, 1 песня медиатеки, 1 плейлист · пропущен 1 плейлист не с музыкой"),
        "{:?}",
        rt.state().toasts.iter().map(|t| &t.text).collect::<Vec<_>>()
    );

    // The YouTube Music play only.
    assert_eq!(lib.count_events().unwrap(), 1);
    assert!(track_of(&lib, Service::Youtube, "yt-gruppa-1").is_some());
    assert!(track_of(&lib, Service::Youtube, "plainvid-01").is_none());
    // The library song is taste only: no like, not in the library, nothing
    // to download.
    let saved = track_of(&lib, Service::Youtube, "yt-homogenic-3").unwrap();
    assert!(!lib.is_liked(saved).unwrap());
    assert_eq!(lib.track_in_library(saved).unwrap(), Some(false));
    assert_eq!(lib.get_track(saved).unwrap().unwrap().title, "Bachelorette");
    assert_eq!(lib.saved_tracks().unwrap(), vec![saved]);
    assert!(lib.liked_track_ids().unwrap().is_empty());
    assert!(lib.list_wishlist().unwrap().is_empty());
    // «Вечер» named and linked through the index; «vr» and «Watch later» are not here.
    let evening = playlist_named(&lib, "Вечер").unwrap();
    assert_eq!(playlist_remotes(&lib, evening, Service::Youtube), ["yt-gruppa-1", "yt-firstlove-3"]);
    let link = lib.playlist_remote(evening).unwrap().unwrap();
    assert_eq!(link.remote, RemoteId::new(Service::Youtube, "PLfakeEveningPlaylist01"));
    assert!(playlist_named(&lib, "vr").is_none());
    assert!(playlist_named(&lib, "Watch later").is_none());
    assert!(track_of(&lib, Service::Youtube, "yt-discovery-1").is_none());

    let before = snapshot(&lib);
    import(&rt, Command::ImportExportFiles(dir.path().to_path_buf()));
    assert_eq!(snapshot(&lib), before, "idempotent");
}

#[test]
fn import_from_the_api_and_again() {
    let lib = Library::open_in_memory().unwrap();
    let spotify = Arc::new(FakeService::spotify());
    let rt = start(&lib, vec![spotify.clone()]);
    import(&rt, Command::ImportFromService(Service::Spotify));
    // Likes: whichever of the first pull (which waits in the wishlist) and
    // the import (which does not) came first.
    assert_eq!(lib.liked_track_ids().unwrap().len(), 3);
    for id in spotify.liked_ids() {
        let t = track_of(&lib, Service::Spotify, &id).unwrap();
        let w = lib.wishlist_for_track(t).unwrap();
        assert!(w.len() <= 1, "{id}");
        assert!(w.iter().all(|w| w.status == WishlistStatus::Waiting));
    }
    // The playlists' tracks wait for nothing.
    for e in lib.playlist_entries(playlist_named(&lib, "Вечер").unwrap(), 0, 100).unwrap() {
        if !lib.is_liked(e.track.id).unwrap() {
            assert!(lib.wishlist_for_track(e.track.id).unwrap().is_empty());
        }
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

    // Spotify connected: its discography first (else Deezer's; a Spotify
    // that is not connected is asked nothing).
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

/// A library track with a file, on its album.
fn local_track(lib: &Library, title: &str, artist: &str, album: &str, file: &str) -> TrackId {
    let credit = ArtistCredit::single(artist, None);
    let release = lib.upsert_release(&Release::new(album, credit.clone())).unwrap();
    let mut t = Track::new(title, credit);
    t.release_id = Some(release.id);
    let t = lib.upsert_track(&t).unwrap();
    lib.add_source(&Source::new(t.id, SourceKind::LocalFile, file, Codec::Opus)).unwrap();
    t.id
}

fn start_with(lib: &Library, streams: Vec<Arc<dyn StreamService>>, prefs: Prefs) -> Runtime {
    Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
            streams,
            library_dir: std::env::temp_dir(),
            prefs,
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    )
}

/// A Takeout of a user whose library has the songs its playlists and
/// history name by uploads of them: the playlist gets the library's tracks,
/// the song the catalogue lists for an upload the library lacks, the raw
/// upload only when no song agrees; the history lands on the library's
/// tracks and adds nothing to the library; the music library's songs teach
/// the taste and are neither likes nor library tracks; nothing waits in the
/// wishlist.
#[test]
fn takeout_videos_become_the_library_songs_and_nothing_else_comes_in() {
    let dir = tempfile::tempdir().unwrap();
    let put = |name: &str, text: &str| std::fs::write(dir.path().join(name), text).unwrap();
    let cell = |header: &str, id: &str, title: &str, channel: &str, time: &str| {
        format!(
            "<div class=\"outer-cell mdl-cell mdl-cell--12-col mdl-shadow--2dp\"><div class=\"mdl-grid\">\
             <div class=\"header-cell mdl-cell mdl-cell--12-col\"><p class=\"mdl-typography--title\">{header}<br></p></div>\
             <div class=\"content-cell mdl-cell mdl-cell--6-col mdl-typography--body-1\">\
             Watched\u{a0}<a href=\"https://music.youtube.com/watch?v={id}\">{title}</a><br>\
             <a href=\"https://www.youtube.com/channel/UCxxxxxxxxxxxxxxxxxxxxxx\">{channel}</a><br>Oct 15, 2026, {time}\u{202f}PM IDT<br>\
             </div></div></div>"
        )
    };
    let cells = [
        cell("YouTube Music", "upload-hk-01", "Hollow Knight OST - Enter Hallownest", "Amellifera", "4:00:00"),
        cell("YouTube Music", "riko-topic-1", "Riko&#39;s Theme", "Kevin Penkin - Topic", "4:03:00"),
        cell("YouTube Music", "riko-topic-1", "Riko&#39;s Theme", "Kevin Penkin - Topic", "4:07:00"),
        cell("YouTube Music", "podcast-0001", "Episode 12: our week", "Some Podcast", "4:40:00"),
        cell("YouTube", "vlog-000001", "My trip", "Blogger", "5:30:00"),
    ];
    put(
        "watch-history (1).html",
        &format!("<html><head><style>.mdl-grid{{}}</style></head><body>{}</body></html>", cells.concat()),
    );
    put(
        "music library songs (1).csv",
        "Video ID,Song Title,Album Title,Artist Name 1,Artist Name 2\n\
         riko-topic-1,Riko's Theme,MADE IN ABYSS ORIGINAL SOUNDTRACK,Kevin Penkin,\n\
         yt-x-kept,Kept Song,Kept Album,Kept Band,Guest\n",
    );
    put(
        "playlists (1).csv",
        "Playlist ID,Add new videos to top,Playlist Title (Original),Playlist Visibility\n\
         PLfakeHollowKnight00001,False,Hollow Knight OST,Private\n",
    );
    put(
        "Hollow Knight OST-videos (1).csv",
        "Video ID,Playlist Video Creation Timestamp\n\
         upload-hk-01,2026-01-01T00:00:00+00:00\n\
         yt-video-greenpath,2026-01-01T00:00:00+00:00\n\
         yt-video-trip,2026-01-01T00:00:00+00:00\n",
    );

    let lib = Library::open_in_memory().unwrap();
    let hallownest = local_track(&lib, "Enter Hallownest", "Christopher Larkin", "Hollow Knight: Original Soundtrack", "Music/a.opus");
    let riko = local_track(&lib, "Riko's Theme", "Kevin Penkin", "MADE IN ABYSS ORIGINAL SOUNDTRACK", "Music/b.opus");
    let tracks = lib.count_tracks(TrackFilter::All).unwrap();
    let albums = |lib: &Library| {
        let mut a: Vec<String> = lib
            .releases(0, 100, nikgob_library::Sort::desc(nikgob_library::ReleaseSortKey::Added))
            .unwrap()
            .into_iter()
            .map(|r| r.release.title)
            .collect();
        a.sort();
        a
    };
    // An upload the library lacks, whose song YouTube Music lists; an
    // upload of no song.
    let youtube = Arc::new(
        FakeService::youtube()
            .logged_out()
            .with_video("greenpath", "Hollow Knight - Greenpath [HD]", "Amellifera", 181, None)
            .with_song("greenpath", "Greenpath", "Christopher Larkin", "Hollow Knight (Original Soundtrack)", 180)
            .with_video("trip", "My trip to Hallownest", "Traveler", 600, None)
            .with_song("kept", "Kept Song", "Kept Band", "Kept Album", 200),
    );
    let rt = start(&lib, vec![youtube.clone()]);
    import(&rt, Command::ImportExportFiles(dir.path().to_path_buf()));

    // The playlist: the library's track, the catalogue's song (with its
    // album and cover), the upload of no song as it is.
    let p = playlist_named(&lib, "Hollow Knight OST").expect("named through the index despite the « (1)»");
    let entries = lib.playlist_entries(p, 0, 10).unwrap();
    let ids: Vec<TrackId> = entries.iter().map(|e| e.track.id).collect();
    let greenpath = track_of(&lib, Service::Youtube, "yt-x-greenpath").expect("the song of the upload");
    let trip = track_of(&lib, Service::Youtube, "yt-video-trip").expect("the raw upload");
    assert_eq!(ids, [hallownest, greenpath, trip]);
    let g = lib.get_track_row(greenpath).unwrap().unwrap();
    assert_eq!((g.title.as_str(), g.artist.as_str()), ("Greenpath", "Christopher Larkin"));
    assert!(g.cover_ref.is_some(), "with its cover");
    assert_eq!(g.album.as_deref(), Some("Hollow Knight (Original Soundtrack)"));
    // The upload ids find their song again (history and the next import).
    assert_eq!(lib.import_key(nikgob_core::streaming::EntityKind::Track, "youtube:yt-video-greenpath").unwrap(), Some(*greenpath.as_uuid()));

    // History: the music only, on the library's tracks; no duplicate.
    assert_eq!(lib.events_of_track(hallownest, 10).unwrap().len(), 2, "a start and its end by the gap");
    let riko_events = lib.events_of_track(riko, 10).unwrap();
    assert_eq!(riko_events.iter().filter(|e| e.event_type == nikgob_core::model::EventType::Start).count(), 2);
    assert!(riko_events.iter().all(|e| e.origin == Some(nikgob_core::model::EventOrigin::Import)));
    assert!(track_of(&lib, Service::Youtube, "riko-topic-1").is_none(), "no second Riko's Theme");
    assert!(track_of(&lib, Service::Youtube, "podcast-0001").is_none());
    assert!(track_of(&lib, Service::Youtube, "vlog-000001").is_none());

    // The music library: taste only.
    let kept = track_of(&lib, Service::Youtube, "yt-x-kept").unwrap();
    let mut saved = lib.saved_tracks().unwrap();
    saved.sort();
    let mut want = vec![riko, kept];
    want.sort();
    assert_eq!(saved, want);
    assert_eq!(lib.track_in_library(kept).unwrap(), Some(false));
    assert!(lib.liked_track_ids().unwrap().is_empty(), "«Любимые» stays as it was");
    assert!(lib.list_wishlist().unwrap().is_empty(), "nothing to download");

    // The library gained the playlist's two new tracks and its song's
    // album, nothing of the history or the music library.
    assert_eq!(lib.count_tracks(TrackFilter::All).unwrap(), tracks + 2);
    assert_eq!(
        albums(&lib),
        ["Hollow Knight (Original Soundtrack)", "Hollow Knight: Original Soundtrack", "MADE IN ABYSS ORIGINAL SOUNDTRACK"]
    );

    // Again: nothing twice, and the services are not asked again.
    let before = snapshot(&lib);
    youtube.reset_calls();
    import(&rt, Command::ImportExportFiles(dir.path().to_path_buf()));
    assert_eq!(snapshot(&lib), before, "idempotent");
    assert_eq!(youtube.calls("search_kind"), 0, "the answers are kept");
}

/// Of a playlist of music, a video watched on plain YouTube that nothing
/// says is music (no song found for it, no music in its title) stays out;
/// a fan's clip of a song, a clip of an artist the library has and a video
/// never watched stay. A song of the library's file under a title in
/// another script is the file, with that title as its alias. The albums
/// the playlist brought are not «Недавно добавлено».
#[test]
fn a_music_playlist_drops_its_videos_of_no_music_and_finds_songs_in_another_script() {
    let dir = tempfile::tempdir().unwrap();
    let put = |name: &str, text: &str| std::fs::write(dir.path().join(name), text).unwrap();
    let cell = |id: &str, title: &str, channel: &str, time: &str| {
        format!(
            "<div class=\"outer-cell mdl-cell mdl-cell--12-col mdl-shadow--2dp\"><div class=\"mdl-grid\">\
             <div class=\"header-cell mdl-cell mdl-cell--12-col\"><p class=\"mdl-typography--title\">YouTube<br></p></div>\
             <div class=\"content-cell mdl-cell mdl-cell--6-col mdl-typography--body-1\">\
             Watched\u{a0}<a href=\"https://www.youtube.com/watch?v={id}\">{title}</a><br>\
             <a href=\"https://www.youtube.com/channel/UCxxxxxxxxxxxxxxxxxxxxxx\">{channel}</a><br>Oct 15, 2026, {time}\u{202f}PM IDT<br>\
             </div></div></div>"
        )
    };
    let cells = [
        cell("yt-video-story", "DRONES - THE WHOLE STORY OF THE CARTOON", "Animation Channel", "4:00:00"),
        cell("yt-video-mad", "【ショー MAD】SHOW × A REASON Full (フル歌詞付き)", "Fan Editor", "4:10:00"),
        cell("yt-video-clip", "Underdog - Eve Music Video", "Eve", "4:20:00"),
    ];
    put(
        "watch-history.html",
        &format!("<html><head><style>.mdl-grid{{}}</style></head><body>{}</body></html>", cells.concat()),
    );
    put(
        "Save-videos.csv",
        "Video ID,Playlist Video Creation Timestamp\n\
         yt-video-story,2026-01-01T00:00:00+00:00\n\
         yt-video-mad,2026-01-01T00:00:00+00:00\n\
         yt-video-clip,2026-01-01T00:00:00+00:00\n\
         yt-video-never,2026-01-01T00:00:00+00:00\n\
         yt-x-belaf,2026-01-01T00:00:00+00:00\n\
         yt-x-her,2026-01-01T00:00:00+00:00\n",
    );
    let lib = Library::open_in_memory().unwrap();
    local_track(&lib, "Kaikai Kitan", "Eve", "Kaikai Kitan", "Music/eve.opus");
    let belaf = {
        let credit = ArtistCredit::single("Kevin Penkin", None);
        let release = lib.upsert_release(&Release::new("MADE IN ABYSS ORIGINAL SOUNDTRACK 3", credit.clone())).unwrap();
        let mut t = Track::new("Belaf's Lullaby", credit);
        t.release_id = Some(release.id);
        t.duration_ms = Some(199_950);
        let t = lib.upsert_track(&t).unwrap();
        lib.add_source(&Source::new(t.id, SourceKind::LocalFile, "Music/belaf.opus", Codec::Opus)).unwrap();
        t.id
    };
    let youtube = Arc::new(
        FakeService::youtube()
            .logged_out()
            .with_video("story", "DRONES - THE WHOLE STORY OF THE CARTOON", "Animation Channel", 900, None)
            .with_video("mad", "【ショー MAD】SHOW × A REASON Full (フル歌詞付き)", "Fan Editor", 250, None)
            .with_video("clip", "Underdog - Eve Music Video", "Eve", 200, None)
            .with_video("never", "A Video Never Watched", "Somebody", 200, None)
            .with_song("belaf", "ベラフの子守歌", "Kevin Penkin", "MADE IN ABYSS ORIGINAL SOUNDTRACK 3", 200)
            .with_song("her", "her", "JVKE", "this is what ____ feels like", 185),
    );
    let rt = start(&lib, vec![youtube]);
    import(&rt, Command::ImportExportFiles(dir.path().to_path_buf()));

    let p = playlist_named(&lib, "Save").expect("a playlist of music");
    let titles: Vec<String> = lib.playlist_entries(p, 0, 10).unwrap().iter().map(|e| e.track.title.clone()).collect();
    assert_eq!(
        titles,
        ["【ショー MAD】SHOW × A REASON Full (フル歌詞付き)", "Underdog - Eve Music Video", "A Video Never Watched", "Belaf's Lullaby", "her"]
    );
    let story = track_of(&lib, Service::Youtube, "yt-video-story");
    assert!(story.is_none_or(|t| lib.track_in_library(t).unwrap() == Some(false)), "the cartoon is not in the library");
    assert_eq!(lib.playlist_entries(p, 0, 10).unwrap()[3].track.id, belaf, "the file, not a second song");
    assert!(track_of(&lib, Service::Youtube, "yt-x-belaf").is_none());
    assert_eq!(lib.track_aliases(belaf).unwrap(), ["ベラフの子守歌"]);
    assert_eq!(lib.get_track(belaf).unwrap().unwrap().title, "Belaf's Lullaby", "the file's title stays");
    assert!(lib.search("ベラフの子守歌", 5).unwrap().tracks.iter().any(|t| t.id == belaf), "found by its other title");
    // The streams the playlist brought are in the library, not «Недавно добавлено».
    let mad = track_of(&lib, Service::Youtube, "yt-video-mad").unwrap();
    assert_eq!(lib.track_in_library(mad).unwrap(), Some(true));
    let brought: Vec<String> =
        lib.releases_of_imported_playlists().unwrap().iter().map(|r| lib.get_release(*r).unwrap().unwrap().title).collect();
    assert_eq!(brought, ["this is what ____ feels like"]);
}

/// «Песни медиатеки YouTube Music» off: that file is not read.
#[test]
fn the_music_library_songs_can_be_left_out() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("music library songs.csv"),
        "Video ID,Song Title,Album Title,Artist Name 1\nyt-x-kept,Kept Song,Kept Album,Kept Band\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("Mix-videos.csv"),
        "Video ID,Playlist Video Creation Timestamp\nyt-x-kept,2026-01-01T00:00:00+00:00\n",
    )
    .unwrap();
    let lib = Library::open_in_memory().unwrap();
    let prefs = Prefs { import_saved_songs: false, ..Prefs::default() };
    let rt = start_with(&lib, vec![Arc::new(FakeService::youtube().logged_out().with_song("kept", "Kept Song", "Kept Band", "Kept Album", 200))], prefs);
    import(&rt, Command::ImportExportFiles(dir.path().to_path_buf()));
    assert!(lib.saved_tracks().unwrap().is_empty());
    assert!(playlist_named(&lib, "Mix").is_some(), "the rest is imported");
}
