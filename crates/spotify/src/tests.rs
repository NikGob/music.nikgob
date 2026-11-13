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
fn the_cache_is_measured_again_only_while_songs_stream() {
    let now = Instant::now();
    let mut c = CacheSize { measured: Some((10, now)), opened: None };
    assert_eq!(c.known(), Some(10), "nothing streamed: the size stands");
    c.opened = Some(now);
    assert_eq!(c.known(), None, "a song was opened: measured again");
    c.measured = Some((20, now + CACHE_SETTLE / 2));
    assert_eq!(c.known(), None, "still settling");
    c.measured = Some((30, now + CACHE_SETTLE + Duration::from_secs(1)));
    assert_eq!(c.known(), Some(30));
}

/// The only Client IDs tests sign in with (never a real app).
const TEST_CLIENT: &str = "test-client-id";

#[test]
fn the_session_reads_the_playlists_that_are_not_the_accounts() {
    use Access::*;
    // Development Mode gives no entries of those; the user's app reads the rest.
    assert!(session_reads_playlist(External));
    assert!(session_reads_playlist(Unknown));
    assert!(!session_reads_playlist(Owned));
    assert!(!session_reads_playlist(Collaborative));
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
    Spotify::with_parts(SpotifyConfig::new(dir), store, endpoints(server))
}

fn memory(dir: &Path) -> Store {
    Store::with_backend(dir.to_path_buf(), Box::new(credentials::Memory::default()))
}

/// A temporary folder, removed when dropped.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A connected service (a grant of the test app) answering from `server`.
async fn connected(name: &str, server: &MockServer) -> (Arc<Spotify>, TempDir) {
    let dir = temp_dir(name);
    let s = service(&dir, memory(&dir), server);
    wait_restored(&s).await;
    s.web.install(api::tests_support::token(TEST_CLIENT));
    (s, TempDir(dir))
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
    let end = Instant::now() + Duration::from_secs(30);
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
    let store = memory(&dir);
    let s = service(&dir, store.clone(), &server);
    wait_restored(&s).await;
    assert_eq!(s.account(), Account::LoggedOut);

    let port = free_port();
    let browser = tokio::spawn(approve_in_browser(s.clone(), port, "c1"));
    s.login_web(web_grant(TEST_CLIENT, port)).await.unwrap();
    browser.await.unwrap();
    assert_eq!(s.personal_client_id().as_deref(), Some(TEST_CLIENT));
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

    // Restored without a browser from the store and the state file.
    drop(s);
    let restored = service(&dir, store.clone(), &server);
    wait_restored(&restored).await;
    assert!(restored.web.has_grant());
    assert_eq!(restored.personal_client_id().as_deref(), Some(TEST_CLIENT));
    assert!(matches!(
        restored.account(),
        Account::LoggedIn {
            premium: Some(true),
            ..
        }
    ));

    restored.logout();
    assert_eq!(restored.account(), Account::LoggedOut);
    assert!(!restored.web.has_grant());
    assert_eq!(
        restored.personal_client_id().as_deref(),
        Some(TEST_CLIENT),
        "the key is the user's app: it stays for the next sign-in"
    );
    let after = service(&dir, store, &server);
    wait_restored(&after).await;
    assert_eq!(after.account(), Account::LoggedOut, "logout is remembered");
    let _ = std::fs::remove_dir_all(dir);
}

/// A Web login whose id differs from the playback login's username keeps the
/// playback login: Spotify does not document the two as one identity, so a
/// difference is only logged.
#[tokio::test(flavor = "multi_thread")]
async fn a_web_login_never_drops_the_playback_login_over_an_id_difference() {
    for kept_for in ["user1", "User2"] {
        let server = mock_spotify("user2", Some("premium")).await;
        let dir = temp_dir("switch");
        let s = service(&dir, memory(&dir), &server);
        wait_restored(&s).await;
        {
            let mut st = lock(&s.state);
            st.playback = Some(PlaybackCredential { username: kept_for.into(), auth_data: vec![1, 2, 3] });
            st.add_slot(Slot::Playback);
            st.saved.playback_username = Some(kept_for.into());
        }
        let generation = s.sessions.generation();
        let port = free_port();
        let browser = tokio::spawn(approve_in_browser(s.clone(), port, "c1"));
        s.login_web(web_grant(TEST_CLIENT, port)).await.unwrap();
        browser.await.unwrap();
        let st = lock(&s.state);
        assert!(st.playback.is_some(), "{kept_for}");
        assert!(st.saved.slots.contains(&Slot::Playback), "{kept_for}");
        assert_eq!(s.sessions.generation(), generation, "no session closed: {kept_for}");
        drop(st);
        let _ = std::fs::remove_dir_all(dir);
    }
    assert!(same_account("abc", "ABC") && !same_account("abc", "abd"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_busy_port_fails_the_login_with_a_human_message() {
    let server = mock_spotify("u", None).await;
    let dir = temp_dir("busy");
    let s = service(&dir, memory(&dir), &server);
    wait_restored(&s).await;
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let e = s.login_web(web_grant(TEST_CLIENT, port)).await.unwrap_err();
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
    let s = service(&dir, memory(&dir), &server);
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
    // «Войти» again needs a Client ID remembered from a sign-in.
    assert_eq!(s.login(LoginInput::Browser).await.unwrap_err(), StreamError::NotConfigured);
    assert_eq!(s.liked().await.unwrap_err(), StreamError::NotLoggedIn);
    assert!(s.search("  ").await.unwrap().tracks.is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn reads_and_writes_go_to_the_users_app_and_are_cached() {
    let personal_server = mock_spotify("me", None).await;
    let dir = temp_dir("routing");
    let s = service(&dir, memory(&dir), &personal_server);
    wait_restored(&s).await;
    s.web.install(api::tests_support::token(TEST_CLIENT));
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

    // Other people's playlists: the session reads them when it is up (none
    // in tests); the user's app is asked otherwise.
    Mock::given(method("GET"))
        .and(path("/playlists/theirs"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"id":"theirs","name":"Theirs","snapshot_id":"s1","owner":{"id":"dj","display_name":"DJ"},
                "items":{"total":2,"limit":1,"offset":0,"next":"n",
                         "items":[{"item":{"id":"x1","type":"track","name":"X1"}}]}}"#,
        ))
        .expect(1)
        .mount(&personal_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/playlists/theirs/items"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"total":2,"limit":50,"offset":1,"next":null,
                "items":[{"item":{"id":"x2","type":"track","name":"X2"}}]}"#,
        ))
        .expect(1)
        .mount(&personal_server)
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

    // Likes go as URIs.
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

    personal_server.verify().await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn liked_songs_are_read_page_by_page() {
    let server = mock_spotify("me", None).await;
    let (s, _dir_s) = connected("liked", &server).await;
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
}

// ---------------------------------------------------------------------------
// Live tests: they need the user's Spotify account and open the browser.
//
//   set NIKGOB_SPOTIFY_LIVE_DIR=C:\Users\<you>\AppData\Roaming\nikgob\spotify-live
//   cargo test -p nikgob-spotify -- --ignored --nocapture --test-threads=1
//
// The first run logs in (browser for the Web API with the user's own app
// from NIKGOB_SPOTIFY_CLIENT_ID, then once more for playback); later runs
// reuse the grants from Credential Manager. Needs Premium.
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
    if !s.web.has_grant() {
        let id = std::env::var("NIKGOB_SPOTIFY_CLIENT_ID")
            .expect("live Spotify tests need NIKGOB_SPOTIFY_CLIENT_ID (the user's own app)");
        eprintln!("log in with the user's app in the browser…");
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
        "playlists": {"items": [{"id": "p1", "name": "Лёд и пламя", "snapshot_id": "s",
                                 "owner": {"id": "dj", "display_name": "DJ"}}],
                      "limit": 10, "offset": 0, "next": null}
    })
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
async fn search_is_one_request_and_never_waits_out_throttling() {
    // 1. Every request 300 ms: one `/search` for every type, playlists too.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(search_body()).set_delay(MOCK_LATENCY))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/me/tracks"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"items":[],"limit":50,"offset":0,"next":"more"}"#)
                .set_delay(MOCK_LATENCY),
        )
        .mount(&server)
        .await;
    let (s, _dir_s) = connected("lat1", &server).await;
    let t = Instant::now();
    let r = s.search("лёд").await.unwrap();
    let plain = t.elapsed();
    assert!(plain < Duration::from_millis(900), "{plain:?}");
    assert_eq!((r.tracks.len(), r.playlists.len()), (1, 1));
    assert_eq!(searches(&server).await, 1);
    let asked = server.received_requests().await.unwrap();
    let types = asked
        .iter()
        .find(|r| r.url.path() == "/search")
        .and_then(|r| r.url.query_pairs().find(|(k, _)| k == "type").map(|(_, v)| v.into_owned()));
    assert_eq!(types.as_deref(), Some("track,album,artist,playlist"));
    // The same query again comes from the cache.
    s.search("лёд").await.unwrap();
    assert_eq!(searches(&server).await, 1);
    eprintln!("[1] plain: {} ms", plain.as_millis());

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

    // 3. Throttled with a short Retry-After (5 s): not waited out.
    let throttled = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "5").set_delay(MOCK_LATENCY))
        .mount(&throttled)
        .await;
    let (s3, _dir_s3) = connected("lat3", &throttled).await;
    let t = Instant::now();
    let r = tokio::time::timeout(Duration::from_secs(40), s3.search("лёд")).await;
    eprintln!(
        "[3] throttled 5 s: {} ms, {} requests",
        t.elapsed().as_millis(),
        searches(&throttled).await,
    );
    assert!(t.elapsed() < Duration::from_millis(900));
    assert!(matches!(r, Ok(Err(StreamError::RateLimited { .. }))));
    // Paused: the next search does not even ask.
    let _ = s3.search("лёд 2").await;
    assert_eq!(searches(&throttled).await, 1);
}

// ---- request counts (stage 3 speed audit) -----------------------------------

/// Requests the mock got for a path ("/me", "/tracks", ...).
async fn hits(server: &MockServer, p: &str) -> usize {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path() == p)
        .count()
}

/// A store holding a grant of the user's app and a state file as a
/// previous start left them.
async fn stored(dir: &Path, state: credentials::State) -> Store {
    let store = memory(dir);
    store
        .save(
            Slot::Personal,
            store.generation(Slot::Personal),
            Secret::Web(api::tests_support::token(TEST_CLIENT)),
        )
        .await
        .unwrap();
    store.save_state(&state);
    store
}

fn profile_state(at: Option<u64>) -> credentials::State {
    credentials::State {
        slots: vec![Slot::Personal],
        personal_client_id: Some(TEST_CLIENT.into()),
        profile: Some(Profile {
            id: "me".into(),
            name: "Me".into(),
            product: Some("premium".into()),
        }),
        profile_at: at,
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_start_asks_for_the_profile_at_most_once_a_day() {
    // Read an hour ago: this start sends nothing.
    let server = mock_spotify("me", Some("premium")).await;
    let dir = temp_dir("start-fresh");
    let store = stored(&dir, profile_state(Some(now_secs() - 3600))).await;
    let s = service(&dir, store, &server);
    wait_restored(&s).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(s.account().is_logged_in());
    let fresh = server.received_requests().await.unwrap().len();
    // Read two days ago: one `GET /me`, and the time is remembered.
    let server2 = mock_spotify("me", Some("premium")).await;
    let dir2 = temp_dir("start-stale");
    let store2 = stored(&dir2, profile_state(Some(now_secs() - 2 * 86_400))).await;
    let s2 = service(&dir2, store2, &server2);
    wait_restored(&s2).await;
    // The profile is asked right after the grants are restored.
    let end = Instant::now() + Duration::from_secs(5);
    while !lock(&s2.state).saved.profile_at.is_some_and(|t| t + 60 > now_secs()) {
        assert!(Instant::now() < end, "the profile was not refreshed");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    let stale = hits(&server2, "/me").await;
    eprintln!("[start] requests: profile fresh {fresh}, profile stale {stale}");
    assert_eq!(fresh, 0);
    assert_eq!(stale, 1);
    assert!(lock(&s2.state).saved.profile_at.is_some_and(|t| t + 60 > now_secs()));
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(dir2);
}

#[tokio::test(flavor = "multi_thread")]
async fn likes_head_is_one_request_and_likes_go_40_per_request() {
    let server = mock_spotify("me", None).await;
    let (s, _dir_s) = connected("head", &server).await;
    let items: Vec<serde_json::Value> = (0..50)
        .map(|i| serde_json::json!({"track": {"id": format!("t{i}"), "name": "x"}}))
        .collect();
    Mock::given(method("GET"))
        .and(path("/me/tracks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": items, "total": 1200, "limit": 50, "offset": 0, "next": "more"
        })))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/me/library"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let head = s.liked_head().await.unwrap();
    assert_eq!((head.tracks.len(), head.total, head.complete), (50, Some(1200), false));
    assert_eq!(hits(&server, "/me/tracks").await, 1, "a full pull of 1200 likes was 24 pages");

    let twelve: Vec<String> = (0..12).map(|i| format!("a{i}")).collect();
    s.set_liked_many(&twelve, true).await.unwrap();
    assert_eq!(hits(&server, "/me/library").await, 1, "an album of 12: one request (was 12)");
    let ninety: Vec<String> = (0..90).map(|i| format!("b{i}")).collect();
    s.set_liked_many(&ninety, true).await.unwrap();
    assert_eq!(hits(&server, "/me/library").await, 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn three_hundred_tracks_are_six_requests_and_versions_are_tiny() {
    let server = mock_spotify("me", None).await;
    let (s, _dir_s) = connected("batch", &server).await;
    struct Batch;
    impl wiremock::Respond for Batch {
        fn respond(&self, r: &wiremock::Request) -> ResponseTemplate {
            let ids = r
                .url
                .query_pairs()
                .find(|(k, _)| k == "ids")
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default();
            let tracks: Vec<serde_json::Value> = ids
                .split(',')
                .map(|id| serde_json::json!({"id": id, "name": "x", "duration_ms": 1000}))
                .collect();
            ResponseTemplate::new(200).set_body_json(serde_json::json!({ "tracks": tracks }))
        }
    }
    Mock::given(method("GET")).and(path("/tracks")).respond_with(Batch).mount(&server).await;
    let ids: Vec<String> = (0..300).map(|i| format!("t{i}")).collect();
    let got = s.tracks(&ids).await.unwrap();
    assert_eq!(got.iter().flatten().count(), 300);
    let batches = hits(&server, "/tracks").await;
    eprintln!("[enrich] 300 tracks: {batches} requests (was 300)");
    assert_eq!(batches, 6);
    // Again: from the cache.
    s.tracks(&ids[..10]).await.unwrap();
    assert_eq!(hits(&server, "/tracks").await, 6);

    Mock::given(method("GET"))
        .and(path("/playlists/p1"))
        .and(wiremock::matchers::query_param("fields", "snapshot_id"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"snapshot_id":"v9"}"#))
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(s.playlist_version("p1").await.unwrap().as_deref(), Some("v9"));
    server.verify().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn what_an_app_lacks_is_asked_once_not_on_every_start() {
    let server = mock_spotify("me", None).await;
    Mock::given(method("GET")).and(path("/tracks")).respond_with(ResponseTemplate::new(403)).mount(&server).await;
    Mock::given(method("GET"))
        .and(path("/tracks/t1"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"id":"t1","name":"One"}"#))
        .mount(&server)
        .await;
    let dir = temp_dir("probes");
    let store = stored(&dir, profile_state(Some(now_secs()))).await;
    let s = service(&dir, store.clone(), &server);
    wait_restored(&s).await;
    assert_eq!(s.tracks(&["t1".into()]).await.unwrap()[0].as_ref().unwrap().title, "One");
    assert_eq!(hits(&server, "/tracks").await, 1);
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(s);
    // The next start knows: straight to the single lookup.
    let again = service(&dir, store, &server);
    wait_restored(&again).await;
    again.forget(EntityKind::Track, "t1");
    again.tracks(&["t1".into()]).await.unwrap();
    assert_eq!(hits(&server, "/tracks").await, 1, "not probed again");
    assert_eq!(hits(&server, "/tracks/t1").await, 2);
    let _ = std::fs::remove_dir_all(dir);
}

// ---- Premium only, no shared app -------------

#[tokio::test(flavor = "multi_thread")]
async fn a_start_deletes_the_shared_apps_grant_quietly() {
    let server = mock_spotify("me", Some("premium")).await;
    let dir = temp_dir("migrate");
    let store = memory(&dir);
    // As an install from before left it: the shared app's grant and a
    // playback login.
    let old = serde_json::to_vec(&Secret::Web(api::tests_support::token("old-shared-client-id"))).unwrap();
    store.put_raw(Slot::Shared, old).await;
    store
        .save(
            Slot::Playback,
            store.generation(Slot::Playback),
            Secret::Playback(PlaybackCredential {
                username: "me".into(),
                auth_data: vec![1, 2, 3],
            }),
        )
        .await
        .unwrap();
    store.save_state(&credentials::State {
        slots: vec![Slot::Shared, Slot::Playback],
        profile: Some(Profile {
            id: "me".into(),
            name: "Me".into(),
            product: Some("premium".into()),
        }),
        profile_at: Some(now_secs()),
        session_premium: Some(true),
        ..Default::default()
    });
    let s = service(&dir, store.clone(), &server);
    wait_restored(&s).await;
    assert_eq!(s.account(), Account::LoggedOut, "not connected, and no error to show");
    assert!(store.raw(Slot::Shared).await.is_none(), "deleted from the credential store");
    let state = store.load_state().await;
    assert_eq!(state.slots, [Slot::Playback], "the slot is dropped");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        server.received_requests().await.unwrap().is_empty(),
        "nothing is asked while not connected"
    );
    let again = service(&dir, store, &server);
    wait_restored(&again).await;
    assert_eq!(again.account(), Account::LoggedOut);
    let _ = std::fs::remove_dir_all(dir);
}

/// After a login that found no Premium: nothing kept, the account says why.
async fn assert_signed_out_for_premium(s: &Spotify, store: &Store) {
    assert_eq!(s.account(), Account::Failed(SPOTIFY_PREMIUM_REQUIRED.into()));
    assert!(!s.web.has_grant());
    assert!(store.raw(Slot::Personal).await.is_none(), "no grant is kept");
    assert!(store.load_state().await.slots.is_empty());
    assert!(lock(&s.state).saved.profile.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_login_without_premium_keeps_nothing() {
    let server = mock_spotify("free-user", Some("free")).await;
    let dir = temp_dir("free");
    let store = memory(&dir);
    let s = service(&dir, store.clone(), &server);
    wait_restored(&s).await;
    let port = free_port();
    let browser = tokio::spawn(approve_in_browser(s.clone(), port, "c1"));
    let e = s.login_web(web_grant(TEST_CLIENT, port)).await.unwrap_err();
    browser.await.unwrap();
    assert_eq!(e, StreamError::PremiumRequired);
    assert_signed_out_for_premium(&s, &store).await;
    // The next start shows it signed out, quietly.
    let again = service(&dir, store, &server);
    wait_restored(&again).await;
    assert_eq!(again.account(), Account::LoggedOut);
    let _ = std::fs::remove_dir_all(dir);
}

fn top_track(id: &str, name: &str, artist: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id, "name": name, "duration_ms": 200000, "is_playable": true,
        "artists": [{"id": format!("ar-{artist}"), "name": artist}],
        "album": {"id": format!("al-{id}"), "name": "Album", "images": []}
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn suggestions_read_the_tops_and_the_recently_played() {
    use wiremock::matchers::query_param;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/me/top/tracks"))
        .and(query_param("time_range", "short_term"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [top_track("t1", "First", "Band"), null, top_track("t2", "Second", "Other")],
            "limit": 50, "offset": 0, "total": 2
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/me/top/artists"))
        .and(query_param("time_range", "medium_term"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [{"id": "ar1", "name": "Band", "images": []}, {"id": "", "name": "No id"}, {"id": "ar2", "name": "Other"}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/me/player/recently-played"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [
                {"track": top_track("t9", "Lately", "Band"), "played_at": "2026-10-20T10:00:00Z"},
                {"track": top_track("t9", "Lately", "Band"), "played_at": "2026-10-20T09:56:00Z"},
                {"track": top_track("t8", "Before", "Band"), "played_at": "2026-10-20T09:50:00Z"}
            ]
        })))
        .mount(&server)
        .await;
    let (s, _dir) = connected("suggest", &server).await;
    let top = s.suggestions(SuggestionFeed::TopTracksMonth).await.unwrap();
    let got: Vec<(&str, SuggestionKind, f32)> = top.tracks.iter().map(|t| (t.track.title.as_str(), t.kind, t.weight)).collect();
    assert_eq!(got, [("First", SuggestionKind::TopMonth, 1.0), ("Second", SuggestionKind::TopMonth, 0.5)]);
    assert_eq!(top.tracks[0].track.id, nikgob_core::streaming::RemoteId::new(Service::Spotify, "t1"));
    assert!(!top.needs_reconnect);
    let artists = s.suggestions(SuggestionFeed::TopArtistsHalfYear).await.unwrap();
    let names: Vec<&str> = artists.artists.iter().map(|a| a.artist.name.as_str()).collect();
    assert_eq!(names, ["Band", "Other"]);
    assert_eq!(artists.artists[0].kind, SuggestionKind::TopHalfYear);
    assert_eq!(artists.artists[0].artist.id, Some(nikgob_core::streaming::RemoteId::new(Service::Spotify, "ar1")));
    let recent = s.suggestions(SuggestionFeed::RecentlyPlayed).await.unwrap();
    let titles: Vec<&str> = recent.tracks.iter().map(|t| t.track.title.as_str()).collect();
    assert_eq!(titles, ["Lately", "Before"], "a repeat counts once");
    assert!(recent.tracks.iter().all(|t| t.kind == SuggestionKind::Recent));
    assert_eq!(s.suggestions(SuggestionFeed::Home).await, Err(StreamError::Unsupported));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_token_without_the_top_scope_asks_for_a_new_sign_in_quietly() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/me/top/tracks"))
        .respond_with(
            ResponseTemplate::new(403).set_body_string(r#"{"error":{"status":403,"message":"Insufficient client scope"}}"#),
        )
        .mount(&server)
        .await;
    let (s, _dir) = connected("scope", &server).await;
    let top = s.suggestions(SuggestionFeed::TopTracksHalfYear).await.unwrap();
    assert!(top.needs_reconnect);
    assert!(top.tracks.is_empty());
    assert!(auth::WEB_SCOPES.contains(&"user-top-read"), "a new sign-in asks for it");
}

const OWNER_NEEDS_PREMIUM: &str =
    r#"{"error":{"status":403,"message":"Active premium subscription required for the owner of the app."}}"#;

#[tokio::test(flavor = "multi_thread")]
async fn an_app_refused_for_an_owner_without_premium_keeps_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/token"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"access_token":"access","refresh_token":"refresh","expires_in":3600}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/me"))
        .respond_with(ResponseTemplate::new(403).set_body_string(OWNER_NEEDS_PREMIUM))
        .mount(&server)
        .await;
    let dir = temp_dir("owner");
    let store = memory(&dir);
    let s = service(&dir, store.clone(), &server);
    wait_restored(&s).await;
    let port = free_port();
    let browser = tokio::spawn(approve_in_browser(s.clone(), port, "c1"));
    let e = s.login_web(web_grant(TEST_CLIENT, port)).await.unwrap_err();
    browser.await.unwrap();
    assert_eq!(e, StreamError::PremiumRequired);
    assert_signed_out_for_premium(&s, &store).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_for_want_of_premium_later_signs_out() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(403).set_body_string(OWNER_NEEDS_PREMIUM))
        .mount(&server)
        .await;
    let dir = temp_dir("lapsed");
    let store = stored(&dir, profile_state(Some(now_secs()))).await;
    let s = service(&dir, store.clone(), &server);
    wait_restored(&s).await;
    assert!(s.account().is_logged_in());
    assert!(s.search("лёд").await.is_err());
    // The sign-out runs on its own task; the first state seen after it is
    // the whole of it (never a plain `LoggedOut` before the reason).
    let end = Instant::now() + Duration::from_secs(30);
    while s.account().is_logged_in() {
        assert!(Instant::now() < end, "not signed out");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_signed_out_for_premium(&s, &store).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_is_asked_while_not_connected() {
    // Nothing is mounted: any request would be a bug (and answer 404).
    let server = MockServer::start().await;
    let dir = temp_dir("offline");
    let s = service(&dir, memory(&dir), &server);
    wait_restored(&s).await;
    let nl = Err::<(), _>(StreamError::NotLoggedIn);
    let ids = vec!["t1".to_owned()];
    assert_eq!(s.search("лёд").await.map(drop), nl);
    assert_eq!(s.search_kind("лёд", SearchKind::Songs).await.map(drop), nl);
    assert_eq!(s.track("t1").await.map(drop), nl);
    assert_eq!(s.tracks(&ids).await.map(drop), nl);
    assert_eq!(s.album("al").await.map(drop), nl);
    assert_eq!(s.artist("ar").await.map(drop), nl);
    assert_eq!(s.artist_top_tracks("ar").await.map(drop), nl);
    assert_eq!(s.playlist("pl").await.map(drop), nl);
    assert_eq!(s.playlist_version("pl").await.map(drop), nl);
    assert_eq!(s.find_isrc("USRC17607839").await.map(drop), nl);
    assert_eq!(s.liked().await.map(drop), nl);
    assert_eq!(s.liked_head().await.map(drop), nl);
    assert_eq!(s.set_liked_many(&ids, true).await, nl);
    assert_eq!(s.my_playlists().await.map(drop), nl);
    assert_eq!(s.create_playlist("x").await.map(drop), nl);
    assert_eq!(s.playlist_edit("pl", &ids, &[]).await.map(drop), nl);
    assert_eq!(s.recent().await.map(drop), nl);
    for &feed in s.suggestion_feeds() {
        assert_eq!(s.suggestions(feed).await.map(drop), nl);
    }
    assert!(matches!(s.open_audio("t1"), Err(StreamError::NotLoggedIn)));
    assert!(s.paused_until().is_none());
    assert!(server.received_requests().await.unwrap().is_empty());
    let _ = std::fs::remove_dir_all(dir);
}
