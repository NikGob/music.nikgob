//! The graph of similar music in the runtime (docs/recs-graph.md) over a
//! library of files only, no history at all: the background pass asks the
//! fake services (YouTube Music's radio of each track after a song search,
//! the similar artists of YouTube Music and Deezer after an artist search)
//! and the open data (MusicBrainz's artist and tags, then ListenBrainz's
//! similar artists and popular recordings, on a mock server), keeps the
//! answers in the library, and the next build resolves them to the library
//! and scores with them; the switches of «Рекомендации» keep it off.
//! Deezer's top tracks give the popularity without ListenBrainz (its
//! popularity wants the user's token), and the pass goes through the whole
//! library without waiting for the next one.

use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_core::model::{Analysis, AnalysisSource, Artist, ArtistCredit, ArtistId, Codec, Release, Source, SourceKind, Track};
use nikgob_core::streaming::{FakeService, StreamService};
use nikgob_library::Library;
use nikgob_metadata::{Deezer, Http, ListenBrainz, Metadata, MetadataConfig, MusicBrainz, RateLimits};
use nikgob_runtime::{ModelPreview, NullPlayback, Runtime, RuntimeConfig, StreamSchedule};
use nikgob_view::Prefs;

fn schedule() -> StreamSchedule {
    StreamSchedule {
        tick: Duration::from_millis(50),
        analysis_start: Duration::ZERO,
        analysis_idle: Duration::from_secs(60),
        recs_debounce: Duration::from_millis(20),
        discover_gap: (Duration::from_millis(1), Duration::from_millis(5)),
        graph_gap_youtube: (Duration::from_millis(1), Duration::from_millis(3)),
        graph_gap_deezer: (Duration::from_millis(1), Duration::from_millis(3)),
        graph_gap_open: (Duration::ZERO, Duration::from_millis(2)),
        ..StreamSchedule::default()
    }
}

fn start(lib: &Library, streams: Vec<Arc<dyn StreamService>>, prefs: Prefs) -> Runtime {
    start_with(lib, streams, prefs, schedule())
}

fn start_with(lib: &Library, streams: Vec<Arc<dyn StreamService>>, prefs: Prefs, schedule: StreamSchedule) -> Runtime {
    Runtime::start_scheduled(
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
        schedule,
    )
}

fn wait_until(what: &str, f: impl Fn() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(60), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(30));
    }
}

/// Songs the fake services know, as files with a sound and no history.
const SONGS: &[(&str, &[&str])] = &[
    ("Кино", &["Группа крови", "Война"]),
    ("Daft Punk", &["One More Time", "Aerodynamic"]),
    ("Аквариум", &["Рок-н-ролл мёртв", "Капитан Африка"]),
    ("Наутилус Помпилиус", &["Скованные одной цепью"]),
    ("Justice", &["Genesis"]),
];

fn library() -> (Library, Vec<(String, ArtistId)>) {
    let lib = Library::open_in_memory().unwrap();
    let mut artists = Vec::new();
    for (k, (name, titles)) in SONGS.iter().enumerate() {
        let a = lib.upsert_artist(&Artist::new(*name)).unwrap();
        artists.push(((*name).to_owned(), a.id));
        let credit = ArtistCredit::single(*name, Some(a.id));
        let r = lib.upsert_release(&Release::new(format!("{name} album"), credit.clone())).unwrap();
        for (n, title) in titles.iter().enumerate() {
            let mut t = Track::new(*title, credit.clone());
            t.release_id = Some(r.id);
            t.track_no = n as u32 + 1;
            t.duration_ms = Some(200_000);
            let t = lib.upsert_track(&t).unwrap();
            lib.add_source(&Source::new(t.id, SourceKind::LocalFile, format!("missing/{name}/{n}.flac"), Codec::Flac)).unwrap();
            let v: Vec<f32> = (0..nikgob_recs::DIM).map(|i| ((k * 5 + i * 3) % 11) as f32 * 0.7 + n as f32 * 0.1).collect();
            lib.put_analysis(&Analysis { track_id: t.id, bliss_vector: v, analyzed_with: nikgob_recs::ANALYZED_WITH.to_owned(), source: AnalysisSource::File })
                .unwrap();
        }
    }
    (lib, artists)
}

fn of(artists: &[(String, ArtistId)], name: &str) -> ArtistId {
    artists.iter().find(|a| a.0 == name).unwrap().1
}

#[test]
fn a_library_without_history_gets_its_graph_from_the_services() {
    let (lib, artists) = library();
    let yt = Arc::new(FakeService::youtube().logged_out());
    let dz = Arc::new(FakeService::deezer());
    let _rt = start(&lib, vec![yt.clone(), dz.clone()], Prefs::default());
    let sources = || lib.graph_edges().unwrap().into_iter().map(|r| r.source).collect::<std::collections::HashSet<_>>();
    wait_until("the services' answers", || {
        let s = sources();
        s.contains("youtube_radio") && s.contains("youtube_artists") && s.contains("deezer_artists")
    });
    // Asked once each: a source's answer stands for a month.
    let asked = lib.graph_fetches().unwrap();
    let mut keys: Vec<(String, String, String)> = asked.iter().map(|f| (f.subject_kind.clone(), f.subject_id.clone(), f.source.clone())).collect();
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), asked.len());
    // The next build resolves them to the library and scores with them.
    wait_until("the graph in the model", || {
        let p = ModelPreview::of(&lib).unwrap();
        let (t, a) = p.graph_size();
        t > 0 && a > 0
    });
    let p = ModelPreview::of(&lib).unwrap();
    let g = &p.model().graph;
    assert!(g.artist(of(&artists, "Кино"), of(&artists, "Аквариум")) > 0.0);
    assert!(g.artist(of(&artists, "Daft Punk"), of(&artists, "Justice")) > 0.0);
    assert_eq!(g.artist(of(&artists, "Justice"), of(&artists, "Аквариум")), 0.0);
    // The library is the taste: profiles from the first launch.
    assert!(!p.model().profiles.list.is_empty());
    // Off, the same library builds without it.
    let off = ModelPreview::of_with(&lib, &Prefs { graph_use: false, ..Prefs::default() }).unwrap();
    assert_eq!(off.graph_size(), (0, 0));
}

#[test]
fn the_switches_keep_the_services_unasked() {
    let (lib, _) = library();
    let yt = Arc::new(FakeService::youtube().logged_out());
    let dz = Arc::new(FakeService::deezer());
    let prefs = Prefs { graph_services: false, graph_open_data: false, ..Prefs::default() };
    let _rt = start(&lib, vec![yt.clone(), dz.clone()], prefs);
    // Builds come and go (ticks of 50 ms); the graph asks nothing.
    std::thread::sleep(Duration::from_millis(2500));
    assert!(lib.graph_fetches().unwrap().is_empty(), "{:?}", lib.graph_fetches().unwrap());
    for f in [&yt, &dz] {
        assert_eq!(f.calls("radio") + f.calls("similar_artists") + f.calls("search_kind"), 0, "{:?}", f.all_calls());
    }
}

#[test]
fn open_data_finds_the_artist_then_its_neighbours() {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const KINO: &str = "11111111-1111-1111-1111-111111111111";
    const AQUARIUM: &str = "22222222-2222-2222-2222-222222222222";
    let server = tokio::runtime::Runtime::new().unwrap();
    let (mock, uri) = server.block_on(async {
        let mock = MockServer::start().await;
        let json = |body: String| ResponseTemplate::new(200).set_body_raw(body.into_bytes(), "application/json");
        Mock::given(method("GET"))
            .and(path("/ws/2/artist"))
            .and(query_param("query", "artist:\"Кино\""))
            .respond_with(json(format!(r#"{{"artists":[{{"id":"{KINO}","name":"Кино","score":100,"tags":[{{"name":"russian rock","count":9}}]}}]}}"#)))
            .mount(&mock)
            .await;
        Mock::given(method("GET"))
            .and(path("/ws/2/artist"))
            .and(query_param("query", "artist:\"Аквариум\""))
            .respond_with(json(format!(r#"{{"artists":[{{"id":"{AQUARIUM}","name":"Аквариум","score":100,"tags":[{{"name":"russian rock","count":5}}]}}]}}"#)))
            .mount(&mock)
            .await;
        // Any other artist: not found.
        Mock::given(method("GET")).and(path("/ws/2/artist")).respond_with(json(r#"{"artists":[]}"#.to_owned())).mount(&mock).await;
        Mock::given(method("GET"))
            .and(path("/labs/similar-artists/json"))
            .and(query_param("artist_mbids", KINO))
            .respond_with(json(format!(r#"[{{"artist_mbid":"{AQUARIUM}","name":"Аквариум","score":900}},{{"artist_mbid":"33333333-3333-3333-3333-333333333333","name":"ДДТ","score":500}}]"#)))
            .mount(&mock)
            .await;
        Mock::given(method("GET")).and(path("/labs/similar-artists/json")).respond_with(json("[]".to_owned())).mount(&mock).await;
        Mock::given(method("GET"))
            .and(path(format!("/api/popularity/top-recordings-for-artist/{KINO}")))
            .and(wiremock::matchers::header("authorization", "Token user-token"))
            .respond_with(json(r#"[{"recording_mbid":"44444444-4444-4444-4444-444444444444","recording_name":"Война","artist_name":"Кино","total_listen_count":50}]"#.to_owned()))
            .mount(&mock)
            .await;
        Mock::given(method("GET")).and(path_regex_any()).respond_with(json("[]".to_owned())).mount(&mock).await;
        let uri = mock.uri();
        (mock, uri)
    });
    let (lib, artists) = library();
    let rt = start(&lib, Vec::new(), Prefs::default());
    let http = Http::new(&MetadataConfig::new("tests@example.invalid")).unwrap().with_rate_limits(RateLimits::unlimited());
    let mut meta = Metadata::new(&MetadataConfig::new("tests@example.invalid")).unwrap();
    meta.musicbrainz = MusicBrainz::with_base_url(http.clone(), &format!("{uri}/ws/2/")).unwrap();
    meta.deezer = Deezer::with_base_url(http.clone(), &format!("{uri}/deezer/")).unwrap();
    meta.listenbrainz = ListenBrainz::with_base_urls(http, &format!("{uri}/labs/"), &format!("{uri}/api/")).unwrap().with_token(Some("user-token"));
    rt.set_graph_metadata(meta);
    wait_until("ListenBrainz's answers", || lib.graph_edges().unwrap().iter().any(|r| r.source == "listenbrainz"));
    wait_until("the popular recordings", || lib.graph_edges().unwrap().iter().any(|r| r.source == "popularity"));
    assert!(lib.graph_tags().unwrap().iter().any(|t| t.tag == "russian rock"));
    let kino = of(&artists, "Кино").to_string();
    let found = lib.graph_fetches().unwrap().into_iter().find(|f| f.subject_id == kino && f.source == "musicbrainz").unwrap();
    assert_eq!(found.remote_id.as_deref(), Some(KINO));
    wait_until("the graph in the model", || {
        let p = ModelPreview::of(&lib).unwrap();
        p.model().graph.artist(of(&artists, "Кино"), of(&artists, "Аквариум")) > 0.0
    });
    drop(mock);
}

/// Anything else the mock is asked.
fn path_regex_any() -> wiremock::matchers::PathRegexMatcher {
    wiremock::matchers::path_regex(".*")
}

/// A source that wants an account now (ListenBrainz's popular recordings
/// answer 401 to a token it does not take) is asked once a run, not once
/// an artist; the rest of the open data goes on.
#[test]
fn a_source_that_wants_an_account_is_asked_once() {
    use wiremock::matchers::{method, path, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = tokio::runtime::Runtime::new().unwrap();
    let (mock, uri) = server.block_on(async {
        let mock = MockServer::start().await;
        let json = |body: String| ResponseTemplate::new(200).set_body_raw(body.into_bytes(), "application/json");
        // Every artist is found, each with an id of its own.
        for (k, (name, _)) in SONGS.iter().enumerate() {
            let id = format!("{k:08}-1111-1111-1111-111111111111");
            Mock::given(method("GET"))
                .and(path("/ws/2/artist"))
                .and(wiremock::matchers::query_param("query", format!("artist:\"{name}\"")))
                .respond_with(json(format!(r#"{{"artists":[{{"id":"{id}","name":"{name}","score":100,"tags":[{{"name":"rock","count":3}}]}}]}}"#)))
                .mount(&mock)
                .await;
        }
        Mock::given(method("GET"))
            .and(path("/labs/similar-artists/json"))
            .respond_with(json(r#"[{"artist_mbid":"33333333-3333-3333-3333-333333333333","name":"ДДТ","score":500}]"#.to_owned()))
            .mount(&mock)
            .await;
        Mock::given(method("GET"))
            .and(path_regex("^/api/popularity/"))
            .respond_with(ResponseTemplate::new(401).set_body_raw(br#"{"code":401,"error":"provide an Auth token"}"#.to_vec(), "application/json"))
            .mount(&mock)
            .await;
        Mock::given(method("GET")).and(path_regex_any()).respond_with(json("[]".to_owned())).mount(&mock).await;
        let uri = mock.uri();
        (mock, uri)
    });
    let (lib, _) = library();
    let rt = start(&lib, Vec::new(), Prefs::default());
    let http = Http::new(&MetadataConfig::new("tests@example.invalid")).unwrap().with_rate_limits(RateLimits::unlimited());
    let mut meta = Metadata::new(&MetadataConfig::new("tests@example.invalid")).unwrap();
    meta.musicbrainz = MusicBrainz::with_base_url(http.clone(), &format!("{uri}/ws/2/")).unwrap();
    meta.deezer = Deezer::with_base_url(http.clone(), &format!("{uri}/deezer/")).unwrap();
    meta.listenbrainz = ListenBrainz::with_base_urls(http, &format!("{uri}/labs/"), &format!("{uri}/api/")).unwrap().with_token(Some("revoked"));
    rt.set_graph_metadata(meta);
    let similar = || lib.graph_fetches().unwrap().iter().filter(|f| f.source == "listenbrainz").count();
    wait_until("ListenBrainz's similar artists of every artist", || similar() == SONGS.len());
    std::thread::sleep(Duration::from_millis(300));
    let asked = server.block_on(mock.received_requests()).unwrap();
    let popular = asked.iter().filter(|r| r.url.path().starts_with("/api/popularity/")).count();
    assert_eq!(popular, 1, "{:?}", asked.iter().map(|r| r.url.path().to_owned()).collect::<Vec<_>>());
    let closed = lib.graph_fetches().unwrap().into_iter().filter(|f| f.source == "popularity").collect::<Vec<_>>();
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].status, "error");
    drop(mock);
}

/// Without a ListenBrainz token its popularity is never asked: Deezer's
/// open top of each artist (an artist search first) gives it instead when
/// the Deezer lane is off, with the rest of the open data.
#[test]
fn without_a_token_the_popularity_comes_from_deezers_open_top() {
    use wiremock::matchers::{method, path, path_regex, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = tokio::runtime::Runtime::new().unwrap();
    let (mock, uri) = server.block_on(async {
        let mock = MockServer::start().await;
        let json = |body: &str| ResponseTemplate::new(200).set_body_raw(body.as_bytes().to_vec(), "application/json");
        // Two artists of that name: the one with more fans is it.
        Mock::given(method("GET"))
            .and(path("/deezer/search/artist"))
            .and(query_param("q", "Кино"))
            .respond_with(json(r#"{"data":[{"id":8,"name":"Кино","nb_fan":3},{"id":7,"name":"Кино","nb_fan":900},{"id":9,"name":"Кино и немцы","nb_fan":5000}]}"#))
            .mount(&mock)
            .await;
        Mock::given(method("GET")).and(path("/deezer/search/artist")).respond_with(json(r#"{"data":[]}"#)).mount(&mock).await;
        Mock::given(method("GET"))
            .and(path("/deezer/artist/7/top"))
            .respond_with(json(
                r#"{"data":[{"id":71,"title":"Война","artist":{"id":7,"name":"Кино"}},{"id":72,"title":"Группа крови","artist":{"id":7,"name":"Кино"}},{"id":73,"title":"Кукушка","artist":{"id":7,"name":"Кино"}}]}"#,
            ))
            .mount(&mock)
            .await;
        Mock::given(method("GET")).and(path_regex("^/deezer/")).respond_with(json(r#"{"data":[]}"#)).mount(&mock).await;
        Mock::given(method("GET")).and(path("/ws/2/artist")).respond_with(json(r#"{"artists":[]}"#)).mount(&mock).await;
        Mock::given(method("GET")).and(path_regex_any()).respond_with(json("[]")).mount(&mock).await;
        let uri = mock.uri();
        (mock, uri)
    });
    let (lib, artists) = library();
    let rt = start(&lib, Vec::new(), Prefs::default());
    let http = Http::new(&MetadataConfig::new("tests@example.invalid")).unwrap().with_rate_limits(RateLimits::unlimited());
    let mut meta = Metadata::new(&MetadataConfig::new("tests@example.invalid")).unwrap();
    meta.musicbrainz = MusicBrainz::with_base_url(http.clone(), &format!("{uri}/ws/2/")).unwrap();
    meta.deezer = Deezer::with_base_url(http.clone(), &format!("{uri}/deezer/")).unwrap();
    meta.listenbrainz = ListenBrainz::with_base_urls(http, &format!("{uri}/labs/"), &format!("{uri}/api/")).unwrap();
    rt.set_graph_metadata(meta);
    let kino = of(&artists, "Кино").to_string();
    let tracks = lib.rec_tracks().unwrap();
    let top = || {
        lib.graph_edges()
            .unwrap()
            .into_iter()
            .filter(|r| r.source == "deezer_top" && r.subject_id == kino)
            .map(|r| (r.entry.name, r.entry.remote_id))
            .collect::<Vec<_>>()
    };
    wait_until("Deezer's open top of Кино", || top().len() == 3);
    assert_eq!(top()[0], ("Война".to_owned(), Some("71".to_owned())));
    let asked = |source: &str| lib.graph_fetches().unwrap().iter().filter(|f| f.source == source).count();
    wait_until("every artist's top", || asked("deezer_top") == SONGS.len());
    std::thread::sleep(Duration::from_millis(300));
    let requests = server.block_on(mock.received_requests()).unwrap();
    assert!(!requests.iter().any(|r| r.url.path().starts_with("/api/popularity/")), "{:?}", requests.iter().map(|r| r.url.path().to_owned()).collect::<Vec<_>>());
    assert_eq!(asked("popularity"), 0);
    let id = |title: &str| tracks.iter().find(|t| t.title == title).unwrap().id;
    wait_until("the popularity in the prior", || {
        let p = ModelPreview::of(&lib).unwrap();
        let m = p.model();
        let prior = |t| m.graph.prior(m.position(t).unwrap());
        prior(id("Война")) > prior(id("Группа крови"))
    });
    drop(mock);
}

/// Deezer's top tracks of each artist (no key, no account) are the
/// popularity of the library's prior: of two songs of one artist the one
/// its top puts first weighs more, with no ListenBrainz at all.
#[test]
fn deezer_top_tracks_give_the_popularity() {
    let (lib, artists) = library();
    let dz = Arc::new(FakeService::deezer());
    let _rt = start(&lib, vec![dz.clone()], Prefs::default());
    let kino = of(&artists, "Кино").to_string();
    let tracks = lib.rec_tracks().unwrap();
    // The library's songs of Кино in the order of Deezer's top.
    let top = || {
        lib.graph_edges()
            .unwrap()
            .into_iter()
            .filter(|r| r.source == "deezer_top" && r.subject_id == kino)
            .filter_map(|r| tracks.iter().find(|t| t.title == r.entry.name).map(|t| t.id))
            .collect::<Vec<_>>()
    };
    wait_until("Deezer's top of Кино", || top().len() == 2);
    let (first, second) = (top()[0], top()[1]);
    // One request an artist Deezer knows, after the artist search found it.
    let asked = |source: &str| lib.graph_fetches().unwrap().iter().filter(|f| f.source == source).count();
    wait_until("every artist searched", || asked("deezer_artists") == SONGS.len());
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(dz.calls("artist_top"), asked("deezer_top"), "{:?}", dz.all_calls());
    wait_until("the popularity in the prior", || {
        let p = ModelPreview::of(&lib).unwrap();
        let m = p.model();
        let prior = |t| m.graph.prior(m.position(t).unwrap());
        prior(first) > prior(second)
    });
}

/// A library larger than what a lane plans at once is asked through
/// without the pause between passes: the next requests are planned as soon
/// as the ones before are asked, until nothing is due.
#[test]
fn the_pass_goes_through_the_whole_library_at_once() {
    let (lib, artists) = library();
    let yt = Arc::new(FakeService::youtube().logged_out());
    let dz = Arc::new(FakeService::deezer());
    let schedule = StreamSchedule { graph_chunk: 2, ..schedule() };
    let _rt = start_with(&lib, vec![yt.clone(), dz.clone()], Prefs::default(), schedule);
    let tracks = SONGS.iter().map(|(_, t)| t.len()).sum::<usize>();
    let asked = |source: &str| lib.graph_fetches().unwrap().iter().filter(|f| f.source == source).count();
    wait_until("every artist and track", || {
        asked("youtube_artists") == artists.len() && asked("deezer_artists") == artists.len() && asked("youtube_radio") == tracks
    });
    // Each asked once: what a source said stands for a month.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(yt.calls("similar_artists"), artists.len(), "{:?}", yt.all_calls());
    assert_eq!(dz.calls("similar_artists"), artists.len(), "{:?}", dz.all_calls());
}
