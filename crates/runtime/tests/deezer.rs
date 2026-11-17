//! The Deezer catalogue and the merged «Всё» search on the fake services:
//! one answer from the library and every usable service, each song once and
//! shown by the copy that plays best, a service that cannot answer left out,
//! a Deezer track played from YouTube Music (its ISRC asked of Deezer
//! first), likes that stay local, discographies from Deezer without Spotify.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_core::model::{Artist, ArtistCredit, Codec, Release, Source, SourceKind, Track};
use nikgob_core::streaming::{EntityKind, FakeService, RemoteId, Service, StreamService, StreamTrack};
use nikgob_library::Library;
use nikgob_runtime::{PlayRequest, PlaySource, Playback, Runtime, RuntimeConfig};
use nikgob_view::{
    Command, Prefs, SearchAnswer, SearchArtist, SearchPick, SearchSources, SearchTrack, TopResult, ViewState,
};

struct Recorder(Arc<Mutex<Vec<PlayRequest>>>);

impl Playback for Recorder {
    fn play(&mut self, item: PlayRequest) {
        self.0.lock().unwrap().push(item);
    }
    fn set_next(&mut self, _: Option<PlayRequest>) {}
    fn pause(&mut self) {}
    fn resume(&mut self) {}
    fn stop(&mut self) {}
    fn seek(&mut self, _: Duration) {}
    fn set_volume(&mut self, _: f32) {}
    fn position(&self) -> Duration {
        Duration::ZERO
    }
}

struct Rig {
    rt: Runtime,
    asked: Arc<Mutex<Vec<PlayRequest>>>,
    _dir: tempfile::TempDir,
}

fn rig(lib: &Library, streams: Vec<Arc<dyn StreamService>>) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
            streams,
            library_dir: dir.path().join("library"),
            prefs: Prefs::default(),
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(Recorder(asked.clone())),
        Arc::new(|| {}),
    );
    Rig { rt, asked, _dir: dir }
}

fn wait_for(rt: &Runtime, what: &str, f: impl Fn(&ViewState) -> bool) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(30) {
        if f(&rt.state()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let s = rt.state();
    panic!("timed out waiting for {what}: answer {:?}, toasts {:?}", s.search.answer, s.toasts.iter().map(|t| &t.text).collect::<Vec<_>>());
}

fn wait_until(what: &str, f: impl Fn() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(30), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn find(service: &FakeService, id: &str) -> StreamTrack {
    service.tracks().into_iter().find(|t| t.id.id == id).unwrap()
}

/// «Всё» for `q`, once every source answered and the top is settled.
fn search_all(r: &Rig, q: &str) -> SearchAnswer {
    r.rt.send(Command::SetSearchSources(SearchSources::all()));
    r.rt.send(Command::Search(q.into()));
    wait_for(&r.rt, "the merged answer", |s| {
        s.search.answer.query == q && !s.search.answer.searching && !s.search.answer.top_pending
    });
    r.rt.state().search.answer.clone()
}

fn titles(a: &SearchAnswer) -> Vec<String> {
    a.tracks.iter().map(|t| t.title().to_owned()).collect()
}

fn stream_row<'a>(a: &'a SearchAnswer, title: &str) -> &'a nikgob_view::StreamTrackRow {
    let top = match &a.top {
        Some(TopResult::Track(t)) => Some(t),
        _ => None,
    };
    a.tracks
        .iter()
        .chain(top)
        .find_map(|t| match t {
            SearchTrack::Stream(r) if r.title == title => Some(r),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no stream row «{title}»: {:?}", titles(a)))
}

#[test]
fn all_merges_the_library_and_every_service_into_one_answer() {
    let lib = Library::open_in_memory().unwrap();
    // "One More Time" is a file in the library.
    let omt = lib.upsert_stream_track(&find(&FakeService::spotify(), "sp-discovery-1")).unwrap();
    lib.add_source(&Source::new(omt, SourceKind::LocalFile, "Music/omt.opus", Codec::Opus)).unwrap();
    let spotify = Arc::new(FakeService::spotify().logged_out());
    let youtube = Arc::new(FakeService::youtube().logged_out());
    let deezer = Arc::new(FakeService::deezer());
    let streams: Vec<Arc<dyn StreamService>> = vec![spotify.clone(), youtube.clone(), deezer.clone()];
    let r = rig(&lib, streams);
    let a = search_all(&r, "daft");

    // Spotify without a login is not asked and says nothing.
    assert_eq!(spotify.calls("search"), 0, "{:?}", spotify.all_calls());
    assert_eq!(a.note, None);
    assert_eq!(a.error, None);
    // Each song once: the library's file, the others from YouTube Music
    // (it plays them itself), Deezer folded in.
    let t = titles(&a);
    for song in ["One More Time", "Aerodynamic", "Digital Love", "Harder, Better, Faster, Stronger"] {
        assert_eq!(t.iter().filter(|x| *x == song).count(), 1, "{song} once: {t:?}");
    }
    assert!(
        a.tracks.iter().any(|x| matches!(x, SearchTrack::Library(row) if row.id == omt)),
        "the library row wins: {t:?}"
    );
    let aero = stream_row(&a, "Aerodynamic");
    assert_eq!(aero.remote.service, Service::Youtube, "a YouTube song plays itself");
    assert!(aero.also_on.contains(&Service::Deezer), "{:?}", aero.also_on);
    // The band is the top result, once (the library's, Deezer's and
    // YouTube's are one), and not again under «Исполнители».
    assert!(matches!(&a.top, Some(TopResult::Artist(SearchArtist::Library(x))) if x.name == "Daft Punk"), "{:?}", a.top);
    let name = |x: &SearchArtist| match x {
        SearchArtist::Library(a) => a.name.clone(),
        SearchArtist::Stream(a) => a.name.clone(),
    };
    assert!(!a.artists.iter().any(|x| name(x) == "Daft Punk"), "{:?}", a.artists);
    assert_eq!(a.albums.len(), 1, "one «Discovery»: {:?}", a.albums);
    // The quick search: Deezer and YouTube asked once each.
    assert_eq!(deezer.calls("search"), 1);
    assert_eq!(youtube.calls("search"), 1);
}

#[test]
fn spotify_with_sound_shows_its_copy_without_sound_youtube_does() {
    for (spotify, want) in [
        (FakeService::spotify(), Service::Spotify),
        (FakeService::spotify().without_audio(), Service::Youtube),
    ] {
        let lib = Library::open_in_memory().unwrap();
        let streams: Vec<Arc<dyn StreamService>> =
            vec![Arc::new(spotify), Arc::new(FakeService::youtube().logged_out()), Arc::new(FakeService::deezer())];
        let r = rig(&lib, streams);
        let a = search_all(&r, "digital");
        let row = stream_row(&a, "Digital Love");
        assert_eq!(row.remote.service, want, "{:?}", a.tracks);
        assert!(matches!(&a.top, Some(TopResult::Track(_))), "the only good match is the top: {:?}", a.top);
        assert!(!titles(&a).iter().any(|t| t == "Digital Love"), "and not again in the list");
    }
}

#[test]
fn a_deezer_track_plays_from_youtube_music_with_the_isrc_deezer_gives() {
    let lib = Library::open_in_memory().unwrap();
    let youtube = Arc::new(FakeService::youtube().logged_out());
    let deezer = Arc::new(FakeService::deezer());
    let streams: Vec<Arc<dyn StreamService>> = vec![youtube.clone(), deezer.clone()];
    let r = rig(&lib, streams);
    // Under Deezer's own chip: its row, without an ISRC (as listings come).
    r.rt.send(Command::SetSearchSources(SearchSources {
        library: false,
        spotify: false,
        youtube: false,
        deezer: true,
        soundcloud: false,
        torrents: false,
        mixed: false,
    }));
    r.rt.send(Command::Search("aerodynamic".into()));
    wait_for(&r.rt, "the Deezer section", |s| {
        s.search.streams.iter().any(|x| x.service == Service::Deezer && !x.searching && !x.tracks.is_empty())
    });
    let row = r.rt.state().search.streams[0].tracks[0].clone();
    assert_eq!(row.remote, RemoteId::new(Service::Deezer, "dz-discovery-2"));
    r.rt.send(Command::PlaySearch {
        tracks: vec![SearchPick::Stream(row.remote.clone())],
        start: 0,
        context: "test".into(),
    });
    wait_until("a play request", || !r.asked.lock().unwrap().is_empty());
    let req = r.asked.lock().unwrap()[0].clone();
    assert_eq!(req.source, PlaySource::Stream { service: Service::Youtube, id: "yt-discovery-2".into() });
    // Deezer was asked the track alone for its ISRC; YouTube found it by it.
    assert_eq!(deezer.calls("track"), 1);
    assert_eq!(youtube.calls("find_isrc"), 1);
    assert_eq!(youtube.calls("search_kind"), 0, "no name search needed");
    let track = lib.track_by_remote(&row.remote).unwrap().unwrap();
    assert_eq!(lib.get_track(track).unwrap().unwrap().isrc.as_deref(), Some("ZZFAK0100002"));
    assert_eq!(lib.track_by_remote(&RemoteId::new(Service::Youtube, "yt-discovery-2")).unwrap(), Some(track));
    // Deezer never plays itself.
    assert!(deezer.opened().is_empty());
}

/// A service's song from the search with autoplay off: its radio plays,
/// the header still says where it was found.
#[test]
fn a_service_song_from_the_search_keeps_its_source_with_autoplay_off() {
    let lib = Library::open_in_memory().unwrap();
    let youtube = Arc::new(FakeService::youtube().logged_out());
    let deezer = Arc::new(FakeService::deezer());
    let r = rig(&lib, vec![youtube, deezer]);
    let prefs = Prefs { autoplay: false, ..r.rt.state().prefs.clone() };
    r.rt.send(Command::SetPrefs(Box::new(prefs)));
    wait_for(&r.rt, "autoplay off", |s| !s.prefs.autoplay);
    let a = search_all(&r, "кино");
    let picks: Vec<SearchPick> = a.tracks.iter().map(|t| t.pick()).collect();
    let start = picks.iter().position(|p| matches!(p, SearchPick::Stream(_))).expect("a service song");
    r.rt.send(Command::PlaySearch { tracks: picks, start, context: "поиска Deezer".into() });
    wait_until("a play request", || !r.asked.lock().unwrap().is_empty());
    wait_for(&r.rt, "the header", |s| s.player.current.is_some() && s.player.context_header(nikgob_view::Locale::Ru).0 == "Играет из поиска Deezer");
}

#[test]
fn a_deezer_track_without_a_youtube_isrc_is_found_by_name() {
    let lib = Library::open_in_memory().unwrap();
    let youtube = Arc::new(FakeService::youtube().logged_out());
    let deezer = Arc::new(FakeService::deezer());
    let r = rig(&lib, vec![youtube.clone(), deezer.clone()]);
    let a = search_all(&r, "кино");
    // «Кино» is the band: the top result.
    assert!(matches!(&a.top, Some(TopResult::Artist(SearchArtist::Stream(x))) if x.name == "Кино"), "{:?}", a.top);
    // The Deezer copy of «Спокойная ночь» has a YouTube twin: one row.
    let night: Vec<&SearchTrack> = a.tracks.iter().filter(|t| t.title() == "Спокойная ночь").collect();
    assert_eq!(night.len(), 1);
    // A Deezer-only play: the album page of Deezer.
    r.rt.send(Command::PlayStreamAlbum {
        id: RemoteId::new(Service::Deezer, "dz-alb-gruppa"),
        start: None,
        shuffle: false,
    });
    wait_until("a play request", || !r.asked.lock().unwrap().is_empty());
    let req = r.asked.lock().unwrap()[0].clone();
    assert_eq!(req.source, PlaySource::Stream { service: Service::Youtube, id: "yt-gruppa-1".into() });
}

#[test]
fn a_like_of_a_deezer_track_stays_local_and_learns_its_isrc() {
    let lib = Library::open_in_memory().unwrap();
    let deezer = Arc::new(FakeService::deezer());
    let r = rig(&lib, vec![Arc::new(FakeService::youtube().logged_out()), deezer.clone()]);
    let dz = RemoteId::new(Service::Deezer, "dz-homogenic-2");
    r.rt.send(Command::SetSearchSources(SearchSources {
        library: false,
        spotify: false,
        youtube: false,
        deezer: true,
        soundcloud: false,
        torrents: false,
        mixed: false,
    }));
    r.rt.send(Command::Search("homogenic".into()));
    wait_for(&r.rt, "the Deezer rows", |s| s.search.streams.iter().any(|x| x.tracks.iter().any(|t| t.remote == dz)));
    r.rt.send(Command::LikeStream { track: dz.clone(), on: true });
    wait_until("the like", || lib.track_by_remote(&dz).unwrap().is_some_and(|t| lib.is_liked(t).unwrap()));
    let track = lib.track_by_remote(&dz).unwrap().unwrap();
    wait_until("the ISRC", || lib.get_track(track).unwrap().unwrap().isrc.as_deref() == Some("ZZFAK9700002"));
    assert!(!lib.wishlist_for_track(track).unwrap().is_empty(), "«Хочу скачать», as the setting says");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(deezer.calls("set_liked") + deezer.calls("set_liked_many"), 0, "nothing goes to Deezer");
    assert_eq!(deezer.calls("liked") + deezer.calls("liked_head") + deezer.calls("my_playlists"), 0);
}

#[test]
fn the_discography_comes_from_deezer_when_spotify_is_not_connected() {
    let lib = Library::open_in_memory().unwrap();
    lib.upsert_stream_track(&find(&FakeService::spotify(), "sp-discovery-1")).unwrap();
    let artist = lib
        .artist_by_remote(&RemoteId::new(Service::Spotify, "sp-art-daft"))
        .unwrap()
        .expect("the library artist");
    let spotify = Arc::new(FakeService::spotify().logged_out());
    let deezer = Arc::new(FakeService::deezer());
    let r = rig(&lib, vec![spotify.clone(), Arc::new(FakeService::youtube().logged_out()), deezer.clone()]);
    r.rt.send(Command::LoadDiscography(artist));
    wait_until("Deezer's artist", || deezer.calls("artist") > 0);
    assert_eq!(spotify.calls("artist") + spotify.calls("search_kind"), 0, "{:?}", spotify.all_calls());
}

/// The only Deezer artist of the name, none of whose albums the library
/// has: its discography is shown, but it is not linked to the library
/// artist (so no photo source or artist page takes it for that artist) and
/// its picture is not kept, now or on a later lookup.
#[test]
fn an_artist_of_the_same_name_alone_is_not_linked() {
    let lib = Library::open_in_memory().unwrap();
    let a = lib.upsert_artist(&Artist::new("Кино")).unwrap();
    let t = lib.upsert_track(&Track::new("Совсем другая песня", ArtistCredit::single("Кино", Some(a.id)))).unwrap();
    lib.add_source(&Source::new(t.id, SourceKind::LocalFile, "/nonexistent/x.flac", Codec::Flac).with_quality(false))
        .unwrap();
    let deezer = Arc::new(FakeService::deezer());
    let r = rig(&lib, vec![deezer.clone()]);
    let key = format!("discography.{}", a.id);
    r.rt.send(Command::LoadDiscography(a.id));
    wait_until("the discography", || lib.cache_get(&key).unwrap().is_some());
    let unlinked = |lib: &Library| {
        let links = lib.external_ids(EntityKind::Artist, *a.id.as_uuid()).unwrap();
        assert!(links.is_empty(), "linked on the name alone: {links:?}");
        assert!(lib.get_artist(a.id).unwrap().unwrap().images.is_empty());
    };
    unlinked(&lib);
    let (_, json) = lib.cache_get(&key).unwrap().unwrap();
    assert!(json.contains("Группа крови"), "its albums are shown: {json}");
    // A day later: searched again, still not linked.
    lib.cache_put(&key, 0, &json).unwrap();
    let before = deezer.calls("search_kind");
    r.rt.send(Command::LoadDiscography(a.id));
    wait_until("the discography again", || lib.cache_get(&key).unwrap().is_some_and(|(at, _)| at > 0));
    assert!(deezer.calls("search_kind") > before);
    unlinked(&lib);
}

/// A merged channel the library links an artist to (by a track's credit):
/// its only album with one of the artist's titles is credited to someone
/// else, so the channel's picture is not saved for the artist.
#[test]
fn a_merged_channel_lends_no_image_through_someone_elses_album() {
    let lib = Library::open_in_memory().unwrap();
    let a = lib.upsert_artist(&Artist::new("gotlibgotlibgotlib")).unwrap();
    let credit = ArtistCredit::single("gotlibgotlibgotlib", Some(a.id));
    let release = lib.upsert_release(&Release::new("Группа крови", credit.clone())).unwrap();
    let mut t = Track::new("Группа крови", credit);
    t.release_id = Some(release.id);
    let t = lib.upsert_track(&t).unwrap();
    lib.add_source(&Source::new(t.id, SourceKind::LocalFile, "/nonexistent/g.flac", Codec::Flac).with_quality(false))
        .unwrap();
    let channel = RemoteId::new(Service::Deezer, "dz-art-kino");
    lib.set_external_id(EntityKind::Artist, &channel, *a.id.as_uuid()).unwrap();
    let deezer = Arc::new(FakeService::deezer());
    let r = rig(&lib, vec![deezer.clone()]);
    let key = format!("discography.{}", a.id);
    r.rt.send(Command::LoadDiscography(a.id));
    wait_until("the discography", || lib.cache_get(&key).unwrap().is_some());
    assert!(deezer.calls("artist") > 0, "asked by the stored link");
    assert!(lib.get_artist(a.id).unwrap().unwrap().images.is_empty(), "no picture from someone else's album");
}

#[test]
fn a_query_with_ye_finds_the_song_written_with_yo() {
    let lib = Library::open_in_memory().unwrap();
    // Only the «ё» spelling finds «Лёд»: the fake matches letters as
    // written, like Deezer.
    let deezer = Arc::new(FakeService::deezer().with_track("led", "Лёд", "WENARO, LXNER", 150));
    let youtube = Arc::new(FakeService::youtube().logged_out());
    let r = rig(&lib, vec![youtube.clone(), deezer.clone()]);
    let a = search_all(&r, "лед");
    let row = stream_row(&a, "Лёд");
    assert_eq!(row.remote, RemoteId::new(Service::Deezer, "dz-x-led"));
    // One variant, asked of Deezer's tracks only.
    assert_eq!(deezer.calls("search"), 1);
    assert_eq!(deezer.calls("search_kind"), 1);
    assert_eq!(youtube.calls("search_kind"), 0, "{:?}", youtube.all_calls());

    // Under Deezer's chip too.
    r.rt.send(Command::SetSearchSources(SearchSources {
        library: false,
        spotify: false,
        youtube: false,
        deezer: true,
        soundcloud: false,
        torrents: false,
        mixed: false,
    }));
    r.rt.send(Command::Search("лед".into()));
    wait_for(&r.rt, "the Deezer section", |s| {
        s.search.streams.iter().any(|x| x.service == Service::Deezer && !x.searching && x.tracks.iter().any(|t| t.title == "Лёд"))
    });
    // «лёд» typed: no variant.
    deezer.reset_calls();
    let _ = search_all(&r, "лёд");
    assert_eq!(deezer.calls("search_kind"), 0);
}

/// With Spotify connected, a Deezer track (no sound of its own) is looked
/// up on Spotify before YouTube Music: Spotify plays it, YouTube Music is
/// not asked.
#[test]
fn a_deezer_track_plays_from_a_connected_spotify_before_youtube_music() {
    let lib = Library::open_in_memory().unwrap();
    let youtube = Arc::new(FakeService::youtube().logged_out());
    let deezer = Arc::new(FakeService::deezer());
    let spotify = Arc::new(FakeService::spotify());
    let mut listed = find(&deezer, "dz-discovery-2");
    listed.isrc = None;
    let track = lib.upsert_stream_track(&listed).unwrap();
    let streams: Vec<Arc<dyn StreamService>> = vec![spotify.clone(), youtube.clone(), deezer.clone()];
    let r = rig(&lib, streams);
    r.rt.send(Command::PlayTracks { tracks: vec![track], start: 0, context: "test".into() });
    wait_until("a play request", || !r.asked.lock().unwrap().is_empty());
    let req = r.asked.lock().unwrap()[0].clone();
    assert_eq!(req.source, PlaySource::Stream { service: Service::Spotify, id: "sp-discovery-2".into() });
    assert_eq!(deezer.calls("track"), 1, "its ISRC from Deezer first");
    assert_eq!(spotify.calls("find_isrc"), 1);
    assert_eq!(youtube.calls("find_isrc") + youtube.calls("search_kind"), 0, "YouTube Music not asked");
}
