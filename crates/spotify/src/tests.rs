//! Service-level tests: routing, logins against a mock Spotify, restore,
//! and live tests (ignored) against the real one.

use super::*;
use std::time::Instant;
use tokio::io::AsyncWriteExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[test]
fn both_tls_stacks_get_a_crypto_provider() {
    install_crypto_provider();
    // Without a process default this panics: ring (librespot) and aws-lc-rs
    // (reqwest) are both compiled in.
    let _ = rustls::ClientConfig::builder();
    assert!(reqwest::Client::builder().build().is_ok());
}

#[test]
fn routing_matrix() {
    use Access::*;
    for op in [
        Op::Catalog,
        Op::UserData,
        Op::PlaylistCreate,
        Op::PlaylistRead(Owned),
        Op::PlaylistRead(Collaborative),
        Op::PlaylistEdit(Owned),
        Op::PlaylistEdit(Collaborative),
    ] {
        assert_eq!(plan(op, true), Source::Personal, "{op:?}");
        assert_eq!(plan(op, false), Source::Shared, "{op:?}");
    }
    for op in [
        Op::PlaylistLibrary,
        Op::PlaylistSearch,
        Op::PlaylistRead(External),
        Op::PlaylistRead(Unknown),
        Op::PlaylistEdit(External),
        Op::PlaylistEdit(Unknown),
    ] {
        assert_eq!(plan(op, true), Source::Shared, "{op:?}");
    }
    // The session reads what the shared app would have.
    assert!(session_reads_playlist(External, true));
    assert!(session_reads_playlist(Owned, false));
    assert!(!session_reads_playlist(Owned, true));
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nikgob-spotify-{name}-{}-{}",
        std::process::id(),
        auth::now_secs()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn endpoints(server: &MockServer) -> Endpoints {
    Endpoints {
        api: server.uri(),
        token: format!("{}/api/token", server.uri()),
    }
}

fn service(dir: &Path, store: Store, server: &MockServer) -> Arc<Spotify> {
    Spotify::with_parts(
        SpotifyConfig::new(dir),
        store,
        endpoints(server),
        endpoints(server),
    )
}

async fn mock_spotify(user: &str, product: Option<&str>) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/token"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"access_token":"access","refresh_token":"refresh","expires_in":3600,"scope":"x"}"#,
        ))
        .mount(&server)
        .await;
    let mut me = serde_json::json!({ "id": user, "display_name": format!("Name of {user}") });
    if let Some(p) = product {
        me["product"] = p.into();
    }
    Mock::given(method("GET"))
        .and(path("/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(me))
        .mount(&server)
        .await;
    server
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn wait_restored(s: &Spotify) {
    let end = Instant::now() + Duration::from_secs(5);
    while lock(&s.state).restoring {
        assert!(Instant::now() < end, "restore did not finish");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Plays the browser: waits for the prompt, then sends the redirect.
async fn approve_in_browser(s: Arc<Spotify>, port: u16, code: &str) {
    let end = Instant::now() + Duration::from_secs(5);
    let url = loop {
        if let Account::LoggingIn(Some(LoginPrompt::OpenUrl(url))) = s.account() {
            break url;
        }
        assert!(Instant::now() < end, "no prompt");
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let url = url::Url::parse(&url).unwrap();
    let state = url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    let mut browser = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    browser
        .write_all(format!("GET /login?code={code}&state={state} HTTP/1.1\r\n\r\n").as_bytes())
        .await
        .unwrap();
}

fn web_grant(client_id: &str, port: u16) -> Grant {
    Grant {
        client_id: client_id.into(),
        redirect_port: port,
        scopes: auth::WEB_SCOPES,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_browser_login_verifies_persists_and_is_restored() {
    let server = mock_spotify("user1", Some("premium")).await;
    let dir = temp_dir("login");
    let store = Store::with_backend(dir.clone(), Box::new(credentials::Memory::default()));
    let s = service(&dir, store.clone(), &server);
    wait_restored(&s).await;
    assert_eq!(s.account(), Account::LoggedOut);

    let port = free_port();
    let browser = tokio::spawn(approve_in_browser(s.clone(), port, "c1"));
    s.login_web(Source::Shared, web_grant(crate::auth::shared_client_id(), port))
        .await
        .unwrap();
    browser.await.unwrap();
    assert_eq!(
        s.account(),
        Account::LoggedIn {
            name: "Name of user1".into(),
            premium: Some(true),
            // No playback authorization yet.
            audio: false,
        }
    );
    assert!(matches!(
        s.open_audio("4uLU6hMCjMI75M1A2tKUQC"),
        Err(StreamError::PremiumRequired)
    ));

    // A personal app signed in as someone else is refused.
    let other = mock_spotify("user2", None).await;
    let personal = Spotify::with_parts(
        SpotifyConfig::new(&dir),
        store.clone(),
        endpoints(&server),
        endpoints(&other),
    );
    wait_restored(&personal).await;
    assert!(personal.shared.has_grant(), "restored from the store");
    let port = free_port();
    let browser = tokio::spawn(approve_in_browser(personal.clone(), port, "c2"));
    let e = personal
        .login_web(Source::Personal, web_grant("my-app", port))
        .await
        .unwrap_err();
    browser.await.unwrap();
    assert!(matches!(e, StreamError::Network(m) if m.contains("другим аккаунтом")));
    assert!(!personal.personal.has_grant());
    assert!(personal.account().is_logged_in(), "the shared grant stays");

    // Restored without a browser from the store and the state file.
    drop(s);
    let restored = service(&dir, store.clone(), &server);
    wait_restored(&restored).await;
    assert!(restored.shared.has_grant());
    assert!(matches!(
        restored.account(),
        Account::LoggedIn {
            premium: Some(true),
            ..
        }
    ));

    restored.logout();
    assert_eq!(restored.account(), Account::LoggedOut);
    assert!(!restored.shared.has_grant());
    let after = service(&dir, store, &server);
    wait_restored(&after).await;
    assert_eq!(after.account(), Account::LoggedOut, "logout is remembered");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_busy_port_fails_the_login_with_a_human_message() {
    let server = mock_spotify("u", None).await;
    let dir = temp_dir("busy");
    let s = service(
        &dir,
        Store::with_backend(dir.clone(), Box::new(credentials::Memory::default())),
        &server,
    );
    wait_restored(&s).await;
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let e = s
        .login_web(Source::Shared, web_grant(crate::auth::shared_client_id(), port))
        .await
        .unwrap_err();
    let message = format!("Порт {port} занят другой программой");
    assert!(
        matches!(&e, StreamError::Network(m) if m.contains(&message)),
        "{e:?}"
    );
    assert!(matches!(s.account(), Account::Failed(m) if m.contains(&message)));
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn simple_answers() {
    let server = mock_spotify("u", None).await;
    let dir = temp_dir("simple");
    let s = service(
        &dir,
        Store::with_backend(dir.clone(), Box::new(credentials::Memory::default())),
        &server,
    );
    assert_eq!(
        s.web_url(EntityKind::Album, "abc"),
        "https://open.spotify.com/album/abc"
    );
    assert_eq!(
        s.web_url(EntityKind::Playlist, "p"),
        "https://open.spotify.com/playlist/p"
    );
    assert_eq!(
        s.save_audio("x", &dir).await.unwrap_err(),
        StreamError::Unsupported
    );
    assert_eq!(
        s.login(LoginInput::Cookies("a=b".into()))
            .await
            .unwrap_err(),
        StreamError::Unsupported
    );
    assert_eq!(
        s.login(LoginInput::PersonalApp {
            client_id: " ".into()
        })
        .await
        .unwrap_err(),
        StreamError::NotConfigured
    );
    wait_restored(&s).await;
    assert_eq!(s.liked().await.unwrap_err(), StreamError::NotLoggedIn);
    assert!(s.search("  ").await.unwrap().tracks.is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn catalogue_reads_go_to_the_right_grant_and_are_cached() {
    let server = mock_spotify("me", None).await;
    let personal_server = MockServer::start().await;
    let dir = temp_dir("routing");
    // The personal app points at its own mock: whatever reaches the shared
    // one for catalogue reads would be a routing bug.
    let s = Spotify::with_parts(
        SpotifyConfig::new(&dir),
        Store::with_backend(dir.clone(), Box::new(credentials::Memory::default())),
        endpoints(&server),
        endpoints(&personal_server),
    );
    wait_restored(&s).await;
    s.shared
        .install(api::tests_support::token(crate::auth::shared_client_id()));
    s.personal.install(api::tests_support::token("mine"));
    lock(&s.state).saved.profile = Some(Profile {
        id: "me".into(),
        name: "Me".into(),
        product: None,
    });

    Mock::given(method("GET"))
        .and(path("/albums/al"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"id":"al","name":"A","album_type":"album","total_tracks":3,
                "tracks":{"items":[{"id":"t1","name":"One","track_number":1}],
                          "limit":1,"offset":0,"next":"more","total":3}}"#,
        ))
        .expect(1)
        .mount(&personal_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/albums/al/tracks"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"items":[{"id":"t2","name":"Two","track_number":2},
                         {"id":"t3","name":"Three","track_number":3}],
                "limit":50,"offset":1,"next":null,"total":3}"#,
        ))
        .expect(1)
        .mount(&personal_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/tracks"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"tracks":[{"id":"t1","name":"One","external_ids":{"isrc":"AA1"}},
                          null,
                          {"id":"t3","name":"Three","external_ids":{"isrc":"AA3"}}]}"#,
        ))
        .expect(1)
        .mount(&personal_server)
        .await;
    let album = s.album("al").await.unwrap();
    assert_eq!(album.tracks.len(), 3);
    assert_eq!(album.tracks[0].isrc.as_deref(), Some("AA1"));
    assert_eq!(album.tracks[1].isrc, None);
    assert_eq!(album.tracks[2].album.as_ref().unwrap().title, "A");
    let again = s.album("al").await.unwrap();
    assert_eq!(again, album, "cached");

    // Other people's playlists stay on the shared app.
    Mock::given(method("GET"))
        .and(path("/playlists/theirs"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"id":"theirs","name":"Theirs","snapshot_id":"s1","owner":{"id":"dj","display_name":"DJ"},
                "items":{"total":2,"limit":1,"offset":0,"next":"n",
                         "items":[{"item":{"id":"x1","type":"track","name":"X1"}}]}}"#,
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/playlists/theirs/items"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"total":2,"limit":50,"offset":1,"next":null,
                "items":[{"item":{"id":"x2","type":"track","name":"X2"}}]}"#,
        ))
        .expect(1)
        .mount(&server)
        .await;
    lock(&s.access).insert("theirs".into(), Access::External);
    let p = s.playlist("theirs").await.unwrap();
    assert_eq!(p.version.as_deref(), Some("s1"));
    assert!(!p.editable);
    assert_eq!(
        p.tracks
            .iter()
            .map(|t| t.id.id.as_str())
            .collect::<Vec<_>>(),
        ["x1", "x2"]
    );

    // Likes go to the personal app, as URIs.
    Mock::given(method("PUT"))
        .and(path("/me/library"))
        .and(wiremock::matchers::query_param("uris", "spotify:track:t1"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&personal_server)
        .await;
    s.set_liked("t1", true).await.unwrap();

    // Edits of an owned playlist: removals, then appends, the last snapshot.
    lock(&s.access).insert("mine".into(), Access::Owned);
    Mock::given(method("DELETE"))
        .and(path("/playlists/mine/items"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"snapshot_id":"v2"}"#))
        .expect(1)
        .mount(&personal_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/playlists/mine/items"))
        .respond_with(ResponseTemplate::new(201).set_body_string(r#"{"snapshot_id":"v3"}"#))
        .expect(2)
        .mount(&personal_server)
        .await;
    let add: Vec<String> = (0..150).map(|i| format!("t{i}")).collect();
    let v = s
        .playlist_edit("mine", &add, &["old".into()])
        .await
        .unwrap();
    assert_eq!(v.as_deref(), Some("v3"));

    server.verify().await;
    personal_server.verify().await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn liked_songs_are_read_page_by_page() {
    let server = mock_spotify("me", None).await;
    let dir = temp_dir("liked");
    let s = service(
        &dir,
        Store::with_backend(dir.clone(), Box::new(credentials::Memory::default())),
        &server,
    );
    wait_restored(&s).await;
    s.shared
        .install(api::tests_support::token(crate::auth::shared_client_id()));
    struct Pages;
    impl wiremock::Respond for Pages {
        fn respond(&self, r: &wiremock::Request) -> ResponseTemplate {
            let offset: u32 = r
                .url
                .query_pairs()
                .find(|(k, _)| k == "offset")
                .and_then(|(_, v)| v.parse().ok())
                .unwrap_or(0);
            let total = 120;
            let items: Vec<serde_json::Value> = (offset..(offset + 50).min(total))
                .map(|i| serde_json::json!({"track": {"id": format!("t{i}"), "name": "x"}}))
                .collect();
            let next = (offset + 50 < total).then_some("next");
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "items": items, "total": total, "limit": 50, "offset": offset, "next": next
            }))
        }
    }
    Mock::given(method("GET"))
        .and(path("/me/tracks"))
        .respond_with(Pages)
        .expect(3)
        .mount(&server)
        .await;
    let liked = s.liked().await.unwrap();
    assert_eq!(liked.len(), 120);
    assert_eq!(liked[119].id.id, "t119");
    server.verify().await;
    let _ = std::fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------
// Live tests: they need the user's Spotify account and open the browser.
//
//   set NIKGOB_SPOTIFY_LIVE_DIR=C:\Users\<you>\AppData\Roaming\nikgob\spotify-live
//   cargo test -p nikgob-spotify -- --ignored --nocapture --test-threads=1
//
// The first run logs in (browser for the Web API, then once more for
// playback); later runs reuse the grants from Credential Manager. Set
// NIKGOB_SPOTIFY_CLIENT_ID to also try a personal app.
// ---------------------------------------------------------------------------

fn live() -> Arc<Spotify> {
    // The live tests sign in to the user's real account with the real
    // browser and credential store: only on purpose.
    assert!(auth::live_tests(), "live Spotify tests need NIKGOB_SPOTIFY_LIVE=1");
    let dir = std::env::var_os("NIKGOB_SPOTIFY_LIVE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("nikgob-spotify-live"));
    Spotify::new(SpotifyConfig::new(dir))
}

async fn live_logged_in(s: &Arc<Spotify>, playback: bool) {
    wait_restored(s).await;
    if !s.shared.has_grant() {
        eprintln!("log in to Spotify in the browser…");
        s.login(LoginInput::Browser).await.unwrap();
    }
    if let Ok(id) = std::env::var("NIKGOB_SPOTIFY_CLIENT_ID")
        && !s.personal.has_grant()
    {
        eprintln!("log in with the personal app…");
        s.login(LoginInput::PersonalApp { client_id: id })
            .await
            .unwrap();
    }
    if playback && lock(&s.state).playback.is_none() {
        eprintln!("authorize playback in the browser…");
        s.login(LoginInput::Playback).await.unwrap();
    }
    eprintln!("account: {:?}", s.account());
}

/// Search, album, artist, playlist, liked and recent against the real API.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the user's Spotify account"]
async fn live_catalogue_and_library() {
    let s = live();
    live_logged_in(&s, false).await;
    let found = s.search("Daft Punk Discovery").await.unwrap();
    eprintln!(
        "search: {} tracks, {} albums, {} artists, {} playlists",
        found.tracks.len(),
        found.albums.len(),
        found.artists.len(),
        found.playlists.len()
    );
    let album = &found.albums[0];
    let album = s.album(&album.id.id).await.unwrap();
    eprintln!(
        "album {} — {} tracks, isrc of the first: {:?}",
        album.title,
        album.tracks.len(),
        album.tracks[0].isrc
    );
    let artist = s.artist(&found.artists[0].id.id).await.unwrap();
    eprintln!("artist {} — {} releases", artist.name, artist.albums.len());
    let isrc = s.track(&found.tracks[0].id.id).await.unwrap().isrc.unwrap();
    assert!(s.find_isrc(&isrc).await.unwrap().is_some());
    let mine = s.my_playlists().await.unwrap();
    eprintln!("{} playlists", mine.len());
    if let Some(p) = mine.first() {
        let p = s.playlist(&p.id.id).await.unwrap();
        eprintln!(
            "playlist {} — {} tracks, version {:?}",
            p.name,
            p.tracks.len(),
            p.version
        );
    }
    if let Some(p) = found.playlists.first() {
        let p = s.playlist(&p.id.id).await.unwrap();
        eprintln!("someone's playlist {} — {} tracks", p.name, p.tracks.len());
    }
    eprintln!("{} liked", s.liked().await.unwrap().len());
    eprintln!("{} recently played", s.recent().await.unwrap().len());
}

/// Opens a track over librespot and reads it through.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the user's Spotify Premium account"]
async fn live_audio() {
    use std::io::{Read, Seek, SeekFrom};
    let s = live();
    live_logged_in(&s, true).await;
    eprintln!(
        "top tracks: {:?}",
        s.artist_top_tracks("4tZwfgrHOc3mvqYlEYSvVi")
            .await
            .map(|t| t.len())
    );
    let id = "0DiWol3AO6WpXZgp0goxAV"; // Daft Punk — One More Time
    let mut stream = s.open_audio(id).unwrap();
    let reader = std::thread::spawn(move || {
        let mut head = [0u8; 4];
        stream.read_exact(&mut head).unwrap();
        assert_eq!(&head, b"OggS");
        eprintln!(
            "len {:?}, gain {:?} dB, duration {:?}",
            stream.byte_len(),
            stream.gain_db(),
            stream.duration()
        );
        let len = stream.byte_len().unwrap();
        stream.seek(SeekFrom::Start(len / 2)).unwrap();
        let mut buf = vec![0u8; 256 * 1024];
        stream.read_exact(&mut buf).unwrap();
    });
    tokio::task::spawn_blocking(move || reader.join().unwrap())
        .await
        .unwrap();
}

// ---- search latency against a slow mock ------------------------------------

const MOCK_LATENCY: Duration = Duration::from_millis(300);

fn search_body() -> serde_json::Value {
    serde_json::json!({
        "tracks": {"items": [{"id": "t1", "name": "Лёд", "duration_ms": 200000,
                              "artists": [{"id": "a1", "name": "Кто-то"}],
                              "external_ids": {"isrc": "RUA1D2300001"}}],
                   "limit": 10, "offset": 0, "next": null},
        "albums": {"items": [], "limit": 10, "offset": 0, "next": null},
        "artists": {"items": [], "limit": 10, "offset": 0, "next": null},
        "playlists": {"items": [], "limit": 10, "offset": 0, "next": null}
    })
}

/// A logged-in service whose two grants answer from `shared` / `personal`.
async fn slow_service(name: &str, shared: &MockServer, personal: Option<&MockServer>) -> Arc<Spotify> {
    let dir = temp_dir(name);
    let s = Spotify::with_parts(
        SpotifyConfig::new(&dir),
        Store::with_backend(dir.clone(), Box::new(credentials::Memory::default())),
        endpoints(shared),
        endpoints(personal.unwrap_or(shared)),
    );
    wait_restored(&s).await;
    s.shared.install(api::tests_support::token(crate::auth::shared_client_id()));
    if personal.is_some() {
        s.personal.install(api::tests_support::token("mine"));
    }
    s
}

async fn searches(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path() == "/search")
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn search_is_one_request_per_grant_and_never_waits_out_throttling() {
    // 1. Personal + shared, every request 300 ms.
    let shared = MockServer::start().await;
    let personal = MockServer::start().await;
    for server in [&shared, &personal] {
        Mock::given(method("GET"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(search_body()).set_delay(MOCK_LATENCY))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/me/tracks"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"items":[],"limit":50,"offset":0,"next":"more"}"#)
                    .set_delay(MOCK_LATENCY),
            )
            .mount(server)
            .await;
    }
    let s = slow_service("lat1", &shared, Some(&personal)).await;
    let t = Instant::now();
    let r = s.search("лёд").await;
    let plain = t.elapsed();
    assert!(r.is_ok() && plain < Duration::from_millis(900), "{plain:?}");
    assert_eq!((searches(&personal).await, searches(&shared).await), (1, 1));
    // The same query again comes from the cache.
    s.search("лёд").await.unwrap();
    assert_eq!(searches(&personal).await, 1);
    eprintln!(
        "[1] plain: {} ms, {} + {} /search requests, ok={}",
        t.elapsed().as_millis(),
        searches(&personal).await,
        searches(&shared).await,
        r.is_ok()
    );

    // 2. Background work (likes pulls, sync) holds the grant's request slots.
    let mut bg = Vec::new();
    for _ in 0..6 {
        let s = s.clone();
        bg.push(tokio::spawn(async move { s.liked().await }));
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    let t = Instant::now();
    let r = s.search("лёд 2").await;
    eprintln!("[2] background busy: {} ms, ok={}", t.elapsed().as_millis(), r.is_ok());
    assert!(r.is_ok() && t.elapsed() < Duration::from_millis(900));
    for b in bg {
        b.abort();
    }

    // 3. The shared app (playlists in search) is throttled: 429 Retry-After 20.
    let shared3 = MockServer::start().await;
    let personal3 = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "20").set_delay(MOCK_LATENCY))
        .mount(&shared3)
        .await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(search_body()).set_delay(MOCK_LATENCY))
        .mount(&personal3)
        .await;
    let s3 = slow_service("lat3", &shared3, Some(&personal3)).await;
    let t = Instant::now();
    let parts = Arc::new(Mutex::new(Vec::new()));
    let sink = {
        let parts = parts.clone();
        let t = Instant::now();
        Arc::new(move |p: StreamSearch| lock(&parts).push((t.elapsed(), p.tracks.len())))
    };
    let r = tokio::time::timeout(Duration::from_secs(40), s3.search_in_parts("лёд", sink)).await;
    assert!(t.elapsed() < Duration::from_millis(900), "{:?}", t.elapsed());
    assert_eq!(lock(&parts).len(), 1, "the catalogue came first");
    eprintln!(
        "[3] shared throttled 20 s: {} ms, {} + {} requests, {:?}",
        t.elapsed().as_millis(),
        searches(&personal3).await,
        searches(&shared3).await,
        r.map(|r| r.map(|x| x.tracks.len()))
    );

    // 4. Shared app only, throttled with a short Retry-After (5 s).
    let shared4 = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "5").set_delay(MOCK_LATENCY))
        .mount(&shared4)
        .await;
    let s4 = slow_service("lat4", &shared4, None).await;
    let t = Instant::now();
    let r = tokio::time::timeout(Duration::from_secs(40), s4.search("лёд")).await;
    eprintln!(
        "[4] shared only, throttled 5 s: {} ms, {} requests",
        t.elapsed().as_millis(),
        searches(&shared4).await,
    );
    assert!(t.elapsed() < Duration::from_millis(900));
    assert!(matches!(r, Ok(Err(StreamError::RateLimited { .. }))));
    // Paused: the next search does not even ask.
    let _ = s4.search("лёд 2").await;
    assert_eq!(searches(&shared4).await, 1);
}
