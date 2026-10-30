//! Stage 3 runtime flows on the fake services: unified search merging,
//! YouTube sound for a Spotify track without Spotify sound, like → wishlist
//! → torrent → the file on the same track, no torrent, dismiss, saving from
//! YouTube, likes both ways without echo, sharing; «Скачать» without a like,
//! likes that download nothing, the undo of a dismissal, the service chips
//! of the search, following a service's playlist.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_core::indexer::{
    BoxFuture, Candidate, CandidateHandle, Indexer, IndexerError, SearchQuery, TorrentPayload,
};
use nikgob_core::model::{
    Codec, LikeTarget, Source, SourceKind, TrackId, WishlistOrigin, WishlistScope, WishlistStatus, score,
};
use nikgob_core::streaming::{FakeService, PlaylistSyncMode, RemoteId, Service, StreamService, StreamTrack};
use nikgob_library::Library;
use nikgob_runtime::{
    ImportConfig, ImportSetup, PlayRequest, PlaySource, Playback, Runtime,
    RuntimeConfig,
};
use nikgob_view::{Command, Link, Prefs, Route, SearchSources, SettingLink, ShareTarget, ViewState};

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

/// Spotify connected (one that is not is asked nothing at all), with no
/// likes of its own to pull into the library.
fn spotify_without_likes() -> FakeService {
    let sp = FakeService::spotify();
    for id in sp.liked_ids() {
        sp.set_liked_remotely(&id, false);
    }
    sp
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
        Arc::new(spotify_without_likes()),
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
    // Download toasts are off by default.
    r.rt.send(Command::SetPrefs(Box::new(Prefs { download_toasts: true, ..Prefs::default() })));
    r.rt.send(Command::Like(LikeTarget::Track(track), true));
    wait_for(&r.rt, "the toast", |s| s.toasts.iter().any(|t| t.text.starts_with("Не нашёл, где скачать")));
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
    let r = rig(&lib, vec![Arc::new(spotify_without_likes())], false);
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

#[test]
fn a_song_liked_on_both_services_is_one_track_and_one_wishlist_item() {
    let lib = Library::open_in_memory().unwrap();
    let spotify = Arc::new(FakeService::spotify());
    let youtube = Arc::new(FakeService::youtube());
    // A remix of a liked song, same artist and length, liked here before.
    let mut remix = find(&youtube, "yt-firstlove-1");
    remix.id = nikgob_core::streaming::RemoteId::new(Service::Youtube, "yt-automatic-remix");
    remix.title = "Automatic (Remix)".into();
    let remix = lib.upsert_stream_track(&remix).unwrap();
    lib.like(remix).unwrap();
    let r = rig(&lib, vec![spotify.clone(), youtube.clone()], false);
    wait_until("both pulls", || {
        lib.remote_likes(Service::Spotify).unwrap().len() == 3
            && lib.remote_likes(Service::Youtube).unwrap().len() == 3
    });
    wait_for(&r.rt, "the wishlist", |s| s.downloads.wishlist.count(WishlistStatus::Waiting) == 3);
    // "Группа крови" and "Automatic" have no ISRC on YouTube.
    for (sp, yt) in [
        ("sp-gruppa-1", "yt-gruppa-1"),
        ("sp-firstlove-1", "yt-firstlove-1"),
        ("sp-discovery-1", "yt-discovery-1"),
    ] {
        let a = lib.track_by_remote(&find(&spotify, sp).id).unwrap().unwrap();
        let b = lib.track_by_remote(&find(&youtube, yt).id).unwrap().unwrap();
        assert_eq!(a, b, "{sp} = {yt}");
        assert_eq!(lib.wishlist_for_track(a).unwrap().len(), 1, "{sp}: one item");
    }
    assert_eq!(lib.liked_track_ids().unwrap().len(), 4, "three songs and the remix");
    let automatic = lib.track_by_remote(&find(&spotify, "sp-firstlove-1").id).unwrap().unwrap();
    assert_ne!(automatic, remix, "the remix stays its own track");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(r.rt.state().downloads.wishlist.items.len(), 3);
}

#[test]
fn an_album_says_the_same_count_on_its_card_its_page_and_in_the_library() {
    let lib = Library::open_in_memory().unwrap();
    let spotify = FakeService::spotify();
    // One of the four songs of "Discovery" is a file, one more is liked.
    let omt = lib.upsert_stream_track(&find(&spotify, "sp-discovery-1")).unwrap();
    lib.add_source(&Source::new(omt, SourceKind::LocalFile, "Music/omt.opus", Codec::Opus))
        .unwrap();
    let aero = lib.upsert_stream_track(&find(&spotify, "sp-discovery-2")).unwrap();
    lib.like(aero).unwrap();
    let r = rig(&lib, vec![Arc::new(spotify_without_likes())], false);
    r.rt.send(Command::Search("daft".into()));
    wait_for(&r.rt, "the section", |s| s.search.streams.iter().any(|x| !x.searching && !x.albums.is_empty()));
    let card = r.rt.state().search.streams[0].albums[0].clone();
    assert_eq!(card.title, "Discovery");
    assert_eq!(card.have, nikgob_view::AlbumHave { local: 1, in_library: 2, total: 4 });
    assert_eq!(card.have.label().as_deref(), Some("1 из 4"));

    r.rt.send(Command::LoadStream(nikgob_view::StreamPage::Album(card.remote.clone())));
    wait_for(&r.rt, "the page", |s| s.stream_page.as_ref().is_some_and(|p| !p.loading && !p.tracks.is_empty()));
    let page = r.rt.state().stream_page.clone().unwrap();
    assert_eq!(page.have, card.have, "page and card agree");
    assert_eq!(page.have.files_label().as_deref(), Some("1 из 4 у тебя"));
    assert_eq!(page.tracks.iter().filter(|t| t.has_local).count(), 1);

    // The library card of the linked album counts the same.
    let release = card.release.unwrap();
    let p = lib.release_progress(release).unwrap().unwrap();
    assert_eq!((p.local, p.in_library, p.total), (1, 2, 4));
}

#[test]
fn stream_album_have_counts_its_rows() {
    let lib = Library::open_in_memory().unwrap();
    let spotify = FakeService::spotify();
    // A song of "Discovery" has its file on another library album.
    let aero = lib.upsert_stream_track(&find(&spotify, "sp-discovery-2")).unwrap();
    lib.like(aero).unwrap();
    let omt = lib.upsert_stream_track(&find(&spotify, "sp-discovery-1")).unwrap();
    lib.add_source(&Source::new(omt, SourceKind::LocalFile, "Music/omt.opus", Codec::Opus))
        .unwrap();
    let home = lib.get_track(aero).unwrap().unwrap().release_id.unwrap();
    let mut other = nikgob_core::model::Release::new("Discovery (Deluxe)", lib.get_track(omt).unwrap().unwrap().artist_credit);
    other.total_tracks = Some(1);
    let other = lib.upsert_release(&other).unwrap();
    let mut t = lib.get_track(omt).unwrap().unwrap();
    t.release_id = Some(other.id);
    lib.upsert_track(&t).unwrap();
    let r = rig(&lib, vec![Arc::new(spotify_without_likes())], false);
    r.rt.send(Command::Search("daft".into()));
    wait_for(&r.rt, "the section", |s| s.search.streams.iter().any(|x| !x.searching && !x.albums.is_empty()));
    let card = r.rt.state().search.streams[0].albums[0].clone();
    r.rt.send(Command::LoadStream(nikgob_view::StreamPage::Album(card.remote.clone())));
    wait_for(&r.rt, "the page", |s| s.stream_page.as_ref().is_some_and(|p| !p.loading && !p.tracks.is_empty()));
    let page = r.rt.state().stream_page.clone().unwrap();
    assert_eq!(page.release, Some(home));
    // Its own four rows: the song filed elsewhere is had, and says where.
    assert_eq!(page.have.total, page.tracks.len() as u32);
    assert_eq!(page.have.local, 1);
    let at = page.tracks.iter().find(|t| t.has_local).unwrap();
    assert_eq!(at.elsewhere.as_deref(), Some("Discovery (Deluxe)"));
    assert!(page.tracks.iter().filter(|t| !t.has_local).all(|t| t.elsewhere.is_none()));
    // The library album counts its own lines.
    let (title, have) = page.library.clone().unwrap();
    assert_eq!(title, "Discovery");
    assert_eq!(have.local, 0);
}

/// The last toast whose text starts with `start`.
fn toast_starting(rt: &Runtime, start: &str) -> Option<nikgob_view::Toast> {
    rt.state().toasts.iter().rev().find(|t| t.text.starts_with(start)).cloned()
}

fn album_items(lib: &Library, release: nikgob_core::model::ReleaseId) -> Vec<nikgob_core::model::WishlistItem> {
    lib.wishlist_for_release(release)
        .unwrap()
        .into_iter()
        .filter(|i| i.scope == WishlistScope::Album)
        .collect()
}

#[test]
fn download_without_a_like_waits_for_torrents_and_leaves_the_likes_alone() {
    let lib = Library::open_in_memory().unwrap();
    let track = lib.upsert_stream_track(&find(&FakeService::spotify(), "sp-gruppa-2")).unwrap();
    let r = rig(&lib, vec![Arc::new(FakeService::spotify().logged_out())], false);
    r.rt.send(Command::DownloadTracks(vec![track]));
    wait_for(&r.rt, "the toast", |s| s.toasts.iter().any(|t| t.text.starts_with("В «Хочу скачать»")));
    let item = lib.wishlist_for_track(track).unwrap().pop().unwrap();
    assert_eq!(
        (item.origin, item.scope, item.status),
        (WishlistOrigin::Manual, WishlistScope::Track, WishlistStatus::New)
    );
    assert!(!lib.is_liked(track).unwrap(), "no like");
    assert_eq!(lib.track_in_library(track).unwrap(), Some(false), "in the library once its file is here");
    // No download source: the toast says so and leads to the settings.
    let toast = toast_starting(&r.rt, "В «Хочу скачать»").unwrap();
    assert!(toast.text.ends_with("скачается, когда включишь источник скачивания"), "{}", toast.text);
    assert_eq!(toast.open, Some(("Настроить".to_owned(), Link::Setting(SettingLink::Torrents))));

    // Asked again: it is on its way already, still one item.
    r.rt.send(Command::DownloadTracks(vec![track]));
    wait_for(&r.rt, "the second toast", |s| s.toasts.iter().any(|t| t.text == "Уже в «Хочу скачать»"));
    assert_eq!(lib.wishlist_for_track(track).unwrap().len(), 1);

    // A like and its undo leave the download alone.
    r.rt.send(Command::Like(LikeTarget::Track(track), true));
    r.rt.send(Command::Like(LikeTarget::Track(track), false));
    wait_for(&r.rt, "the unlike", |s| s.toasts.iter().any(|t| t.text == "Убрано из Любимых"));
    assert!(!lib.is_liked(track).unwrap());
    assert_eq!(wish_status(&lib, track), Some(WishlistStatus::New), "«Скачать» stays");
}

#[test]
fn download_a_stream_track_and_a_stream_album() {
    let lib = Library::open_in_memory().unwrap();
    let r = rig(&lib, vec![Arc::new(spotify_without_likes())], false);
    // A row of the search: known to the runtime by its remote id.
    r.rt.send(Command::Search("кино".into()));
    wait_for(&r.rt, "the section", |s| s.search.streams.iter().any(|x| !x.searching && !x.tracks.is_empty()));
    let row = r.rt.state().search.streams[0].tracks[1].clone();
    r.rt.send(Command::DownloadStream { tracks: vec![row.remote.clone()] });
    wait_until("the track item", || {
        lib.track_by_remote(&row.remote).unwrap().is_some_and(|t| wish_status(&lib, t).is_some())
    });
    let track = lib.track_by_remote(&row.remote).unwrap().unwrap();
    assert!(!lib.is_liked(track).unwrap());
    assert_eq!(lib.wishlist_for_track(track).unwrap()[0].origin, WishlistOrigin::Manual);

    // The album: one album item for the songs without a file.
    let album = RemoteId::new(Service::Spotify, "sp-alb-gruppa");
    r.rt.send(Command::DownloadStreamAlbum(album.clone()));
    wait_until("the album item", || {
        lib.release_by_remote(&album).unwrap().is_some_and(|rel| !album_items(&lib, rel).is_empty())
    });
    let release = lib.release_by_remote(&album).unwrap().unwrap();
    let items = album_items(&lib, release);
    assert_eq!(items.len(), 1);
    assert_eq!((items[0].origin, items[0].status), (WishlistOrigin::Manual, WishlistStatus::New));
    assert!(lib.liked_track_ids().unwrap().is_empty(), "nothing liked");
    // From the library album's page: queued already.
    r.rt.send(Command::DownloadRelease(release));
    wait_for(&r.rt, "the toast", |s| s.toasts.iter().filter(|t| t.text == "Уже в «Хочу скачать»").count() == 1);
    assert_eq!(album_items(&lib, release).len(), 1);
}

#[test]
fn with_likes_that_download_nothing_a_like_is_only_a_like() {
    let lib = Library::open_in_memory().unwrap();
    let track = lib.upsert_stream_track(&find(&FakeService::spotify(), "sp-gruppa-3")).unwrap();
    let r = rig(&lib, vec![Arc::new(FakeService::spotify().logged_out())], false);
    r.rt.send(Command::SetPrefs(Box::new(Prefs { like_downloads: false, ..Prefs::default() })));
    r.rt.send(Command::Like(LikeTarget::Track(track), true));
    wait_for(&r.rt, "the toast", |s| s.toasts.iter().any(|t| t.text == "Добавлено в Любимые"));
    assert!(lib.is_liked(track).unwrap());
    assert!(lib.wishlist_for_track(track).unwrap().is_empty(), "nothing to download");
    // «Скачать» downloads all the same.
    r.rt.send(Command::DownloadTracks(vec![track]));
    wait_until("the item", || wish_status(&lib, track) == Some(WishlistStatus::New));
}

#[test]
fn a_dismissed_download_comes_back_with_its_undo() {
    let lib = Library::open_in_memory().unwrap();
    let track = lib.upsert_stream_track(&find(&FakeService::youtube(), "yt-gruppa-2")).unwrap();
    let r = rig(&lib, vec![Arc::new(FakeService::youtube().logged_out())], false);
    r.rt.send(Command::DownloadTracks(vec![track]));
    wait_until("the item", || wish_status(&lib, track).is_some());
    let id = lib.wishlist_for_track(track).unwrap()[0].id;
    r.rt.send(Command::WishlistDismiss(vec![id]));
    wait_for(&r.rt, "the undo", |s| {
        s.toasts.iter().any(|t| t.action.as_ref().is_some_and(|(_, c)| *c == Command::WishlistRestore(vec![id])))
    });
    assert_eq!(wish_status(&lib, track), Some(WishlistStatus::Dismissed));
    r.rt.send(Command::WishlistRestore(vec![id]));
    wait_until("back", || wish_status(&lib, track) == Some(WishlistStatus::New));
}

#[test]
fn under_a_service_chip_its_songs_in_the_library_stay_in_its_list() {
    let lib = Library::open_in_memory().unwrap();
    let omt = lib.upsert_stream_track(&find(&FakeService::spotify(), "sp-discovery-1")).unwrap();
    lib.add_source(&Source::new(omt, SourceKind::LocalFile, "Music/omt.opus", Codec::Opus))
        .unwrap();
    let r = rig(&lib, vec![Arc::new(spotify_without_likes())], false);
    r.rt.send(Command::SetSearchSources(SearchSources {
        library: false,
        spotify: true,
        youtube: false,
        deezer: false,
        torrents: false,
        mixed: false,
    }));
    r.rt.send(Command::Search("daft".into()));
    wait_for(&r.rt, "the Spotify section", |s| {
        s.search.query == "daft"
            && s.search.streams.iter().any(|x| x.service == Service::Spotify && !x.searching && !x.tracks.is_empty())
    });
    let s = r.rt.state();
    assert!(s.search.tracks.is_empty(), "no library section under the chip");
    let sp = s.search.streams.iter().find(|x| x.service == Service::Spotify).unwrap();
    let row = sp.tracks.iter().find(|t| t.title == "One More Time").expect("kept under the chip");
    assert!(row.has_local, "marked as on disk");
}

#[test]
fn a_followed_playlist_is_linked_in_follow_mode() {
    let lib = Library::open_in_memory().unwrap();
    let r = rig(&lib, vec![Arc::new(spotify_without_likes())], false);
    let remote = RemoteId::new(Service::Spotify, "sp-pl-mix");
    r.rt.send(Command::FollowStreamPlaylist(remote.clone()));
    wait_for(&r.rt, "the toast", |s| s.toasts.iter().any(|t| t.text == "«Fake Mix» в библиотеке"));
    let link = lib.playlist_by_remote(&remote).unwrap().unwrap();
    assert_eq!(link.sync_mode, PlaylistSyncMode::Follow, "someone else's playlist: follow");
    assert_eq!(link.remote_version.as_deref(), Some("1"));
    assert_eq!(lib.get_playlist(link.playlist_id).unwrap().unwrap().name, "Fake Mix");
    assert_eq!(lib.playlist_entries(link.playlist_id, 0, 100).unwrap().len(), 3);
    let toast = toast_starting(&r.rt, "«Fake Mix» в библиотеке").unwrap();
    assert_eq!(toast.open, Some(("Открыть".to_owned(), Link::Route(Route::Playlist(link.playlist_id)))));
    // Again: it is there already, not a second copy.
    r.rt.send(Command::FollowStreamPlaylist(remote.clone()));
    wait_for(&r.rt, "the second toast", |s| s.toasts.iter().any(|t| t.text == "«Fake Mix» уже в библиотеке"));
    assert_eq!(lib.list_playlists().unwrap().iter().filter(|p| p.name == "Fake Mix").count(), 1);
}

#[test]
fn an_album_is_downloaded_only_when_every_track_has_its_file() {
    use nikgob_core::streaming::{AlbumRef, ArtistRef};
    let lib = Library::open_in_memory().unwrap();
    // «Хочу скачать» made the album's tracks: outside the library, no files.
    let track = |no: u32, title: &str, ms: u64| {
        let mut t = StreamTrack::new(RemoteId::new(Service::Deezer, format!("dz-{no}")), title);
        t.artists = vec![ArtistRef::new(None, "Кино")];
        t.album = Some(AlbumRef::new(Some(RemoteId::new(Service::Deezer, "dz-alb")), "Группа крови"));
        t.track_no = Some(no);
        t.duration_ms = Some(ms);
        lib.upsert_stream_track(&t).unwrap()
    };
    let one = track(1, "Группа крови", 3_000);
    let two = track(2, "Закрой за мной дверь", 2_000);
    track(3, "Война", 2_500);
    let release = lib.get_track(one).unwrap().unwrap().release_id.unwrap();
    let mut item = nikgob_core::model::WishlistItem::new(WishlistStatus::Waiting, 1);
    item.scope = WishlistScope::Album;
    item.release_id = Some(release);
    item.origin = WishlistOrigin::Manual;
    item.title = Some("Группа крови".into());
    lib.put_wishlist_item(&item).unwrap();
    let r = rig(&lib, Vec::new(), true);

    // The user's folder has two of the three songs.
    let dir = r.cfg.inbox_dir.join("Кино - Группа крови (1988)");
    std::fs::create_dir_all(&dir).unwrap();
    write_wav(&dir.join("01 - Группа крови.wav"), 3.0);
    write_wav(&dir.join("02 - Закрой за мной дверь.wav"), 2.0);
    r.rt.send(Command::WishlistAttach { item: item.id, what: nikgob_view::WishlistAttach::Folder(dir) });
    let local = |t| lib.active_source(t).unwrap().is_some_and(|s| s.kind.is_local());
    wait_until("the files on the album's tracks", || local(one) && local(two));
    let settled = || lib.get_wishlist_item(item.id).unwrap().unwrap();
    wait_until("the item settled", || settled().status == WishlistStatus::Done);
    assert_eq!(settled().note.as_deref(), Some("Скачано 2 из 3"), "not «Скачано»: a song is missing");
    wait_for(&r.rt, "the row says what is missing", |s| {
        s.downloads.wishlist.items.iter().any(|row| row.id == item.id && row.missing == 1)
    });
    assert!(r.rt.state().downloads.confirm.is_empty(), "nothing to confirm");
}

#[test]
fn an_album_page_with_one_file_shows_the_whole_album_from_a_service() {
    use nikgob_core::model::{Artist, ArtistCredit, Release, Track};
    let lib = Library::open_in_memory().unwrap();
    // One song of «Группа крови» is here, as a file; nothing says the album
    // has more.
    let kino = lib.upsert_artist(&Artist::new("Кино")).unwrap();
    let credit = ArtistCredit::single("Кино", Some(kino.id));
    let release = lib.upsert_release(&Release::new("Группа крови", credit.clone())).unwrap();
    let mut t = Track::new("Группа крови", credit);
    t.release_id = Some(release.id);
    t.duration_ms = Some(284_500);
    let file = lib.upsert_track(&t).unwrap();
    lib.add_source(&Source::new(file.id, SourceKind::LocalFile, "Music/g.flac", Codec::Flac)).unwrap();
    // YouTube Music without likes of its own to pull.
    let yt = FakeService::youtube();
    for id in yt.liked_ids() {
        yt.set_liked_remotely(&id, false);
    }
    let r = rig(&lib, vec![Arc::new(yt)], false);

    r.rt.send(Command::LoadRelease(release.id));
    wait_for(&r.rt, "the whole tracklist on the page", |s| {
        s.release.as_ref().is_some_and(|d| d.card.id == release.id && d.tracks.len() == 4)
    });
    let page = r.rt.state().release.clone().unwrap();
    assert_eq!((page.card.local_tracks, page.card.total_tracks), (1, Some(4)), "«1 из 4 у тебя»");
    assert_eq!(page.tracks[0].id, file.id, "the file is its first song");
    assert!(page.tracks[1..].iter().all(|t| !t.has_local && t.source == Some(SourceKind::Youtube)), "the rest stream");
    assert_eq!(lib.count_tracks(nikgob_library::TrackFilter::All).unwrap(), 1, "they stay outside the library");
    wait_for(&r.rt, "the card says 1 of 4", |s| {
        s.library.releases.iter().any(|c| c.id == release.id && c.total_tracks == Some(4) && c.local_tracks == 1)
    });
    // Opened again: nothing is asked twice.
    r.rt.send(Command::LoadRelease(release.id));
    wait_for(&r.rt, "the page again", |s| s.release.as_ref().is_some_and(|d| d.tracks.len() == 4));
    assert_eq!(lib.tracks_of_release_all(release.id).unwrap().len(), 4);
}

/// Under «Только скачанное» an album with one file plays that file alone:
/// the rest of its tracklist, found on a service after it started, does not
/// join the queue.
#[test]
fn only_downloaded_keeps_an_albums_tracklist_found_later_out_of_the_queue() {
    use nikgob_core::model::{Artist, ArtistCredit, Release, Track};
    let lib = Library::open_in_memory().unwrap();
    let kino = lib.upsert_artist(&Artist::new("Кино")).unwrap();
    let credit = ArtistCredit::single("Кино", Some(kino.id));
    let release = lib.upsert_release(&Release::new("Группа крови", credit.clone())).unwrap();
    let mut t = Track::new("Группа крови", credit);
    t.release_id = Some(release.id);
    t.duration_ms = Some(284_500);
    let file = lib.upsert_track(&t).unwrap();
    lib.add_source(&Source::new(file.id, SourceKind::LocalFile, "Music/g.flac", Codec::Flac)).unwrap();
    let yt = FakeService::youtube();
    for id in yt.liked_ids() {
        yt.set_liked_remotely(&id, false);
    }
    let r = rig(&lib, vec![Arc::new(yt)], false);
    r.rt.send(Command::SetPref(nikgob_view::PrefPatch::MixesDownloadedOnly(true)));
    wait_for(&r.rt, "the filter on", |s| s.prefs.mixes_downloaded_only);

    r.rt.send(Command::PlayRelease { id: release.id, start: None, shuffle: false });
    // The card says 1 of 4 once the tracklist is in (and the queue was
    // offered the rest).
    wait_for(&r.rt, "the whole tracklist", |s| {
        s.library.releases.iter().any(|c| c.id == release.id && c.total_tracks == Some(4))
    });
    assert_eq!(lib.tracks_of_release_all(release.id).unwrap().len(), 4);
    let st = r.rt.state();
    let queued: Vec<TrackId> = st
        .queue
        .prev
        .iter()
        .map(|e| e.track.id)
        .chain(st.queue.current.iter().map(|t| t.id))
        .chain(st.queue.next.iter().map(|e| e.track.id))
        .collect();
    assert_eq!(queued, [file.id], "only the file");
}

/// «Хочу скачать» shows neither a song that has its file already nor the
/// tracks of an album wished whole, and the same song asked again under a
/// twin track is still one item.
#[test]
fn the_wishlist_shows_one_row_per_thing_to_download() {
    use nikgob_core::model::{ArtistCredit, Release, Track, WishlistItem};
    let lib = Library::open_in_memory().unwrap();
    let release = lib.upsert_release(&Release::new("Big Walk Theme", ArtistCredit::single("aksfx", None))).unwrap();
    let mut t = Track::new("Big Walk Theme", ArtistCredit::single("aksfx", None));
    t.release_id = Some(release.id);
    let walk = lib.upsert_track(&t).unwrap().id;
    let ghost = lib.upsert_track(&Track::new("Ghost Avenue", ArtistCredit::single("Eve", None))).unwrap().id;
    let mut f = Source::new(ghost, SourceKind::LocalFile, "Music/Eve/Ghost Avenue.flac", Codec::Flac);
    f.quality_score = 900;
    lib.add_source(&f).unwrap();
    let put = |track: Option<TrackId>, scope: WishlistScope, title: &str| {
        let mut w = WishlistItem::new(WishlistStatus::NoTorrent, 1);
        w.track_id = track;
        w.release_id = Some(release.id).filter(|_| track != Some(ghost));
        w.scope = scope;
        w.title = Some(title.into());
        lib.put_wishlist_item(&w).unwrap();
    };
    // Made after its file came (the import missed it).
    put(Some(ghost), WishlistScope::Track, "Ghost Avenue");
    // A one-track album wished as its track and as the album.
    put(Some(walk), WishlistScope::Track, "Big Walk Theme");
    put(None, WishlistScope::Album, "Big Walk Theme");

    let a = lib.upsert_track(&Track::new("Twin", ArtistCredit::single("A", None))).unwrap().id;
    let b = lib.upsert_track(&Track::new("Twin", ArtistCredit::single("A", None))).unwrap().id;
    assert_ne!(a, b);
    let r = rig(&lib, Vec::new(), false);
    r.rt.send(Command::DownloadTracks(vec![a]));
    wait_for(&r.rt, "the toast", |s| s.toasts.iter().any(|t| t.text.starts_with("В «Хочу скачать»")));
    r.rt.send(Command::DownloadTracks(vec![b]));
    wait_for(&r.rt, "the second toast", |s| s.toasts.iter().any(|t| t.text == "Уже в «Хочу скачать»"));
    assert!(lib.wishlist_for_track(b).unwrap().is_empty(), "no second item for the twin");
    // The song with its file is «Скачано» (by the start's repair), the
    // album is one row.
    wait_for(&r.rt, "the rows", |s| {
        let mut rows: Vec<(&str, WishlistScope, WishlistStatus)> =
            s.downloads.wishlist.items.iter().map(|i| (i.title.as_str(), i.scope, i.status)).collect();
        rows.sort_by_key(|r| r.0);
        rows == [
            ("Big Walk Theme", WishlistScope::Album, WishlistStatus::NoTorrent),
            ("Ghost Avenue", WishlistScope::Track, WishlistStatus::Done),
            ("Twin", WishlistScope::Track, WishlistStatus::New),
        ]
    });
}
