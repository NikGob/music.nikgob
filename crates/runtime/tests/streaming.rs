//! Stage 3 runtime flows on the fake services: unified search merging,
//! YouTube sound for a Spotify track without Spotify sound, like → wishlist
//! → torrent → the file on the same track, no torrent, dismiss, saving from
//! YouTube, likes both ways without echo, sharing.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_core::indexer::{
    BoxFuture, Candidate, CandidateHandle, Indexer, IndexerError, SearchQuery, TorrentPayload,
};
use nikgob_core::model::{
    Codec, LikeTarget, Source, SourceKind, TrackId, WishlistOrigin, WishlistStatus, score,
};
use nikgob_core::streaming::{FakeService, Service, StreamService, StreamTrack};
use nikgob_library::Library;
use nikgob_runtime::{
    ImportConfig, ImportSetup, PlayRequest, PlaySource, Playback, Runtime,
    RuntimeConfig,
};
use nikgob_view::{Command, Prefs, ShareTarget, ViewState};

fn write_wav(path: &Path, secs: f32) {
    let rate = 44_100;
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let mut seed = 7u32;
    for i in 0..(secs * rate as f32) as u32 {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = ((seed >> 16) as f32 / 65_536.0 - 0.5) * 0.2;
        let tone = (i as f32 * 330.0 * std::f32::consts::TAU / rate as f32).sin() * 0.3;
        let s = ((tone + noise) * 20_000.0) as i16;
        w.write_sample(s).unwrap();
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
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
        "timed out waiting for {what}: toasts {:?}, wishlist {:?}",
        s.toasts.iter().map(|t| &t.text).collect::<Vec<_>>(),
        s.downloads.wishlist.items
    );
}

fn wait_until(what: &str, f: impl Fn() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(60), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(30));
    }
}

fn find(service: &FakeService, id: &str) -> StreamTrack {
    service.tracks().into_iter().find(|t| t.id.id == id).unwrap()
}

/// Every fake future is ready at once.
fn now<T>(f: BoxFuture<'_, T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(f)
}

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

/// A fake indexer: every search answers `results`; `fetch_torrent` hands
/// out `payload`.
struct FakeIndexer {
    results: Vec<Candidate>,
    payload: Option<Vec<u8>>,
    asked: Mutex<Vec<String>>,
}

impl Indexer for FakeIndexer {
    fn name(&self) -> &str {
        "fake"
    }
    fn search(&self, q: &SearchQuery) -> BoxFuture<'_, Result<Vec<Candidate>, IndexerError>> {
        self.asked.lock().unwrap().push(q.text.clone());
        let r = self.results.clone();
        Box::pin(async move { Ok(r) })
    }
    fn fetch_torrent(&self, _: &Candidate) -> BoxFuture<'_, Result<TorrentPayload, IndexerError>> {
        let p = self.payload.clone();
        Box::pin(async move { p.map(TorrentPayload::TorrentFile).ok_or(IndexerError::NothingToFetch) })
    }
}

fn candidate(title: &str) -> Candidate {
    Candidate {
        id: title.into(),
        title: title.into(),
        indexer_name: "Fake".into(),
        size_bytes: Some(60_000_000),
        seeders: Some(12),
        leechers: None,
        published: None,
        categories: vec![3040],
        info_url: None,
        info_hash: None,
        handle: CandidateHandle::default(),
    }
}

struct Rig {
    rt: Runtime,
    asked: Arc<Mutex<Vec<PlayRequest>>>,
    _dir: tempfile::TempDir,
    cfg: ImportConfig,
}

fn rig(lib: &Library, streams: Vec<Arc<dyn StreamService>>, import: bool) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let cfg = ImportConfig::with_root(dir.path());
    for d in [&cfg.library_dir, &cfg.inbox_dir, &cfg.downloads_dir, &cfg.trash_dir] {
        std::fs::create_dir_all(d).unwrap();
    }
    let asked = Arc::new(Mutex::new(Vec::new()));
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
            streams,
            library_dir: cfg.library_dir.clone(),
            prefs: Prefs::default(),
            prefs_path: None,
            settings_path: None,
            import: import.then(|| ImportSetup {
                config: cfg.clone(),
                metadata: None,
                watch_inbox: false,
                torrent_state_dir: dir.path().join("torrent-state"),
                torrent_offline: true,
            }),
        },
        Box::new(Recorder(asked.clone())),
        Arc::new(|| {}),
    );
    Rig {
        rt,
        asked,
        _dir: dir,
        cfg,
    }
}

fn wish_status(lib: &Library, track: TrackId) -> Option<WishlistStatus> {
    lib.wishlist_for_track(track)
        .unwrap()
        .last()
        .map(|i| i.status)
}

#[test]
fn search_merges_services_and_the_library_wins() {
    let lib = Library::open_in_memory().unwrap();
    // "One More Time" is in the library already (a file, same ISRC).
    let omt = lib.upsert_stream_track(&find(&FakeService::spotify(), "sp-discovery-1")).unwrap();
    lib.add_source(&Source::new(omt, SourceKind::LocalFile, "Music/omt.opus", Codec::Opus))
        .unwrap();
    let streams: Vec<Arc<dyn StreamService>> = vec![
        Arc::new(FakeService::spotify().logged_out()),
        Arc::new(FakeService::youtube().logged_out()),
    ];
    let r = rig(&lib, streams, false);
    r.rt.send(Command::Search("daft".into()));
    wait_for(&r.rt, "both sections", |s| {
        s.search.streams.len() == 2 && s.search.streams.iter().all(|x| !x.searching)
    });
    let s = r.rt.state();
    let (sp, yt) = (&s.search.streams[0], &s.search.streams[1]);
    assert_eq!((sp.service, yt.service), (Service::Spotify, Service::Youtube));
    assert!(s.search.tracks.iter().any(|t| t.id == omt), "the library hit");
    assert!(!sp.tracks.iter().any(|t| t.title == "One More Time"), "library wins");
    let aero = sp.tracks.iter().find(|t| t.title == "Aerodynamic").expect("spotify row");
    assert_eq!(aero.also_on, vec![Service::Youtube]);
    assert!(yt.tracks.is_empty(), "every YouTube song is a Spotify one: {:?}", yt.tracks);
    assert_eq!(sp.albums.len(), 1);
    assert!(yt.albums.is_empty(), "the same album");
    drop(s);

    // By name and length when YouTube has no ISRC; the clip stays.
    r.rt.send(Command::Search("кино".into()));
    wait_for(&r.rt, "the new query", |s| {
        s.search.query == "кино" && s.search.streams.iter().all(|x| !x.searching)
    });
    let s = r.rt.state();
    let yt: Vec<&str> = s.search.streams[1].tracks.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(yt, ["Группа крови (клип)"]);
    assert_eq!(s.search.streams[0].tracks.len(), 4);
}

#[test]
fn spotify_without_sound_plays_the_same_track_from_youtube() {
    let lib = Library::open_in_memory().unwrap();
    // Spotify only, and YouTube does not know the ISRC of this one.
    let track = lib.upsert_stream_track(&find(&FakeService::spotify(), "sp-gruppa-1")).unwrap();
    let streams: Vec<Arc<dyn StreamService>> = vec![
        Arc::new(FakeService::spotify().logged_out()),
        Arc::new(FakeService::youtube().logged_out()),
    ];
    let r = rig(&lib, streams, false);
    r.rt.send(Command::PlayTracks {
        tracks: vec![track],
        start: 0,
        context: "test".into(),
    });
    wait_until("a play request", || !r.asked.lock().unwrap().is_empty());
    let req = r.asked.lock().unwrap()[0].clone();
    assert_eq!(
        req.source,
        PlaySource::Stream {
            service: Service::Youtube,
            id: "yt-gruppa-1".into()
        }
    );
    // Found once, attached to the same track; the label says YouTube.
    assert_eq!(lib.track_by_remote(&find(&FakeService::youtube(), "yt-gruppa-1").id).unwrap(), Some(track));
    wait_for(&r.rt, "the label", |s| s.player.audio_service == Some(Service::Youtube));
    let events = lib.events_of_track(track, 10).unwrap();
    assert!(events.iter().any(|e| e.source_kind == Some(SourceKind::Youtube)), "{events:?}");
}

#[test]
fn a_like_downloads_the_track_and_the_file_lands_on_the_same_id() {
    let lib = Library::open_in_memory().unwrap();
    let mut t = find(&FakeService::spotify(), "sp-gruppa-1");
    t.duration_ms = Some(3_000); // as long as the file below
    let track = lib.upsert_stream_track(&t).unwrap();
    let streams: Vec<Arc<dyn StreamService>> = vec![Arc::new(FakeService::spotify().logged_out())];
    let r = rig(&lib, streams, true);

    // The album's torrent, its data already in Downloads/.
    let album = r.cfg.downloads_dir.join("Кино - Группа крови (1988) FLAC");
    std::fs::create_dir_all(&album).unwrap();
    write_wav(&album.join("01 - Группа крови.wav"), 3.0);
    write_wav(&album.join("02 - Закрой за мной дверь.wav"), 2.0);
    let torrent = tokio::runtime::Runtime::new().unwrap().block_on(async {
        librqbit::create_torrent(
            &album,
            librqbit::CreateTorrentOptions::default(),
            &librqbit::spawn_utils::BlockingSpawner::new(1),
        )
        .await
        .unwrap()
        .as_bytes()
        .unwrap()
        .to_vec()
    });
    let indexer = Arc::new(FakeIndexer {
        results: vec![
            candidate("Кино - Группа крови (1988) [FLAC]"),
            candidate("Ария - Кровь за кровь FLAC"),
        ],
        payload: Some(torrent),
        asked: Mutex::new(Vec::new()),
    });
    r.rt.set_indexer(indexer.clone());

    r.rt.send(Command::Like(LikeTarget::Track(track), true));
    wait_until("the file", || {
        lib.active_source(track).unwrap().is_some_and(|s| s.kind.is_local())
    });
    wait_until("the item done", || wish_status(&lib, track) == Some(WishlistStatus::Done));
    assert_eq!(indexer.asked.lock().unwrap()[0], "Кино Группа крови");
    let item = lib.wishlist_for_track(track).unwrap().pop().unwrap();
    assert_eq!(item.origin, WishlistOrigin::Like);
    assert!(lib.is_liked(track).unwrap(), "the like stays on the same track");
    // Only the track's file was taken from the album's torrent.
    let dl = lib.list_downloads().unwrap();
    assert_eq!(dl[0].selected_files, vec!["0".to_owned()]);
    assert_eq!(dl[0].wishlist_id, Some(item.id));
    wait_for(&r.rt, "the wishlist view", |s| {
        s.downloads.wishlist.count(WishlistStatus::Done) == 1
    });
}

#[test]
fn no_torrent_is_reported_and_a_dismissed_item_never_comes_back() {
    let lib = Library::open_in_memory().unwrap();
    let track = lib.upsert_stream_track(&find(&FakeService::youtube(), "yt-gruppa-3")).unwrap();
    let streams: Vec<Arc<dyn StreamService>> = vec![Arc::new(FakeService::youtube().logged_out())];
    let r = rig(&lib, streams, true);
    r.rt.set_indexer(Arc::new(FakeIndexer {
        results: Vec::new(),
        payload: None,
        asked: Mutex::new(Vec::new()),
    }));
    r.rt.send(Command::Like(LikeTarget::Track(track), true));
    wait_for(&r.rt, "the toast", |s| s.toasts.iter().any(|t| t.text.starts_with("Не нашёл раздачу")));
    let item = lib.wishlist_for_track(track).unwrap().pop().unwrap();
    assert_eq!(item.status, WishlistStatus::NoTorrent);
    assert!(item.next_try_at.unwrap() > nikgob_library::now_ms() + 6 * 86_400_000, "a week later");
    let toast = r.rt.state().toasts.iter().find(|t| t.text.starts_with("Не нашёл")).cloned().unwrap();
    assert_eq!(toast.action.unwrap().1, Command::WishlistSaveFromYoutube(item.id));
    wait_for(&r.rt, "the row", |s| {
        s.downloads.wishlist.items.iter().any(|i| i.id == item.id && i.next_try.is_some())
    });

    r.rt.send(Command::WishlistDismiss(vec![item.id]));
    r.rt.send(Command::Like(LikeTarget::Track(track), false));
    r.rt.send(Command::Like(LikeTarget::Track(track), true));
    wait_for(&r.rt, "the like again", |s| s.downloads.wishlist.count(WishlistStatus::Dismissed) == 1);
    std::thread::sleep(Duration::from_millis(300));
    let items = lib.wishlist_for_track(track).unwrap();
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].status, WishlistStatus::Dismissed);
}

#[test]
fn saved_from_youtube_is_a_low_quality_file_of_the_same_track() {
    let lib = Library::open_in_memory().unwrap();
    let track = lib.upsert_stream_track(&find(&FakeService::youtube(), "yt-gruppa-2")).unwrap();
    let streams: Vec<Arc<dyn StreamService>> = vec![Arc::new(FakeService::youtube().logged_out())];
    let r = rig(&lib, streams, true);
    // No indexer: the item waits as new.
    r.rt.send(Command::Like(LikeTarget::Track(track), true));
    wait_until("the item", || wish_status(&lib, track).is_some());
    let item = lib.wishlist_for_track(track).unwrap().pop().unwrap();
    r.rt.send(Command::WishlistSaveFromYoutube(item.id));
    wait_until("the saved file", || {
        lib.active_source(track).unwrap().is_some_and(|s| s.kind.is_local())
    });
    let src = lib.active_source(track).unwrap().unwrap();
    assert_eq!(src.quality_score, score::SAVED_STREAM);
    assert_eq!(src.origin, nikgob_core::model::SourceOrigin::Stream);
    wait_until("the item back to searching", || {
        wish_status(&lib, track) == Some(WishlistStatus::NoTorrent)
    });
    let item = lib.get_wishlist_item(item.id).unwrap().unwrap();
    assert!(item.note.unwrap().contains("YouTube"));
    assert!(!r.cfg.downloads_dir.join(".youtube").join("yt-gruppa-2.wav").exists(), "temp file gone");
}

#[test]
fn likes_go_both_ways_without_echo() {
    let lib = Library::open_in_memory().unwrap();
    let spotify = Arc::new(FakeService::spotify());
    let r = rig(&lib, vec![spotify.clone()], false);
    // The first pull is an import: the three likes wait in the wishlist.
    wait_for(&r.rt, "the first pull", |s| s.downloads.wishlist.count(WishlistStatus::Waiting) == 3);
    let gruppa = lib.track_by_remote(&find(&spotify, "sp-gruppa-1").id).unwrap().unwrap();
    assert!(lib.is_liked(gruppa).unwrap());
    assert_eq!(lib.wishlist_for_track(gruppa).unwrap()[0].origin, WishlistOrigin::Import);

    // A like here goes to Spotify.
    let michi = lib.upsert_stream_track(&find(&spotify, "sp-michi-1")).unwrap();
    r.rt.send(Command::Like(LikeTarget::Track(michi), true));
    wait_until("the push", || spotify.liked_ids().contains(&"sp-michi-1".to_owned()));

    // In Spotify: one unlike, one new like. A login pulls again.
    now(spotify.set_liked("sp-gruppa-1", false)).unwrap();
    now(spotify.set_liked("sp-homogenic-3", true)).unwrap();
    let before = spotify.liked_ids();
    r.rt.send(Command::StreamLogin {
        service: Service::Spotify,
        input: nikgob_core::streaming::LoginInput::Browser,
    });
    let homogenic = || lib.track_by_remote(&find(&spotify, "sp-homogenic-3").id).unwrap();
    wait_until("the pulled like", || homogenic().is_some_and(|t| lib.is_liked(t).unwrap()));
    wait_until("the pulled unlike", || !lib.is_liked(gruppa).unwrap());
    let h = homogenic().unwrap();
    let item = lib.wishlist_for_track(h).unwrap().pop().unwrap();
    assert_eq!(item.origin, WishlistOrigin::Sync, "a new like after the import");
    assert!(lib.is_liked(michi).unwrap(), "our own like stays");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(spotify.liked_ids(), before, "nothing pushed back");
}

#[test]
fn share_copies_a_song_link() {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = tokio::runtime::Runtime::new().unwrap();
    let (mock, uri) = server.block_on(async {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/links"))
            .and(query_param("url", "https://open.spotify.com/track/sp-gruppa-1"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                br#"{"pageUrl":"https://song.link/s/gruppa"}"#.to_vec(),
                "application/json",
            ))
            .mount(&mock)
            .await;
        let uri = mock.uri();
        (mock, uri)
    });
    let lib = Library::open_in_memory().unwrap();
    // A library track without stream ids, found on Spotify by its ISRC.
    let artist = lib.upsert_artist(&nikgob_core::model::Artist::new("Кино")).unwrap();
    let mut t = nikgob_core::model::Track::new(
        "Группа крови",
        nikgob_core::model::ArtistCredit::single("Кино", Some(artist.id)),
    );
    t.isrc = Some("ZZFAK8800001".into());
    let t = lib.upsert_track(&t).unwrap();
    let other = lib
        .upsert_track(&nikgob_core::model::Track::new(
            "Нигде нет",
            nikgob_core::model::ArtistCredit::single("Кино", Some(artist.id)),
        ))
        .unwrap();
    let r = rig(&lib, vec![Arc::new(FakeService::spotify().logged_out())], false);
    r.rt.set_odesli_base(uri);
    r.rt.send(Command::Share(ShareTarget::Track(t.id)));
    wait_for(&r.rt, "the link", |s| {
        s.clipboard.as_ref().is_some_and(|c| c.text == "https://song.link/s/gruppa")
    });
    assert!(r.rt.state().toasts.iter().any(|t| t.text == "Ссылка скопирована"));
    // Nowhere to be found: the name is copied.
    r.rt.send(Command::Share(ShareTarget::Track(other.id)));
    wait_for(&r.rt, "the text", |s| {
        s.clipboard.as_ref().is_some_and(|c| c.text == "Кино — Нигде нет")
    });
    drop(mock);
}

#[test]
fn a_doubtful_release_waits_in_confirm() {
    let lib = Library::open_in_memory().unwrap();
    let track = lib.upsert_stream_track(&find(&FakeService::spotify(), "sp-gruppa-2")).unwrap();
    let r = rig(&lib, vec![Arc::new(FakeService::spotify().logged_out())], true);
    r.rt.set_indexer(Arc::new(FakeIndexer {
        results: vec![candidate("Кино - Группа крови (1988) MP3 320")],
        payload: None,
        asked: Mutex::new(Vec::new()),
    }));
    r.rt.send(Command::Like(LikeTarget::Track(track), true));
    wait_for(&r.rt, "the confirm", |s| {
        s.downloads.confirm.iter().any(|c| c.options.len() == 1 && c.accept_label == "Скачать")
    });
    assert_eq!(wish_status(&lib, track), Some(WishlistStatus::Confirm));
    let id = r.rt.state().downloads.confirm[0].id;
    r.rt.send(Command::ConfirmChoice { id, option: None });
    wait_for(&r.rt, "no torrent", |s| s.downloads.confirm.is_empty());
    assert_eq!(wish_status(&lib, track), Some(WishlistStatus::NoTorrent));
}
