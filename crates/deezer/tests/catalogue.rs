//! The Deezer catalogue against a local mock of its public API (wiremock and
//! JSON fixtures shaped like Deezer's answers; never the network).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nikgob_core::model::ReleaseType;
use nikgob_core::streaming::{Account, EntityKind, LoginInput, SearchKind, Service, StreamError, StreamService};
use nikgob_deezer::{Deezer, DeezerConfig};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixture(name: &str) -> String {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn json(name: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(fixture(name), "application/json")
}

async fn serve(server: &MockServer, at: &str, body: &str, times: u64) {
    Mock::given(method("GET"))
        .and(path(at))
        .respond_with(json(body))
        .expect(times)
        .mount(server)
        .await;
}

fn deezer(server: &MockServer) -> Arc<Deezer> {
    Deezer::new(DeezerConfig { base_url: server.uri() }).unwrap()
}

#[tokio::test]
async fn the_quick_search_asks_three_categories_and_the_chip_also_playlists() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(query_param("q", "лёд"))
        .and(query_param("limit", "25"))
        .respond_with(json("search_tracks.json"))
        .expect(2)
        .mount(&server)
        .await;
    serve(&server, "/search/album", "search_albums.json", 2).await;
    serve(&server, "/search/artist", "search_artists.json", 2).await;
    serve(&server, "/search/playlist", "search_playlists.json", 1).await;
    let dz = deezer(&server);
    let parts = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&parts);
    let top = dz
        .search_top(" лёд ", Arc::new(move |p| {
            assert!(!p.tracks.is_empty() && p.albums.is_empty(), "the tracks come first");
            seen.fetch_add(1, Ordering::SeqCst);
        }))
        .await
        .unwrap();
    assert_eq!(parts.load(Ordering::SeqCst), 1);
    assert!(top.playlists.is_empty(), "no playlist request for «Всё»");
    assert_eq!(top.tracks.len(), 3);
    let t = &top.tracks[0];
    assert_eq!(t.id.service, Service::Deezer);
    assert_eq!(t.id.id, "1551523502");
    assert_eq!(t.title, "Лёд");
    assert_eq!(t.artist_line(), "Монеточка");
    assert_eq!(t.duration_ms, Some(186_000));
    assert_eq!(t.isrc, None, "search hits carry no ISRC");
    assert_eq!(t.album.as_ref().map(|a| a.title.as_str()), Some("Раскраски для взрослых"));
    assert_eq!(
        t.cover_url.as_deref(),
        Some("https://cdn-images.dzcdn.net/images/cover/4b2c1f0e0a1b2c3d4e5f60718293a4b5/1000x1000-000000-80-0-0.jpg")
    );
    // No cover url: built from the md5.
    assert_eq!(
        top.tracks[1].cover_url.as_deref(),
        Some("https://cdn-images.dzcdn.net/images/cover/aa11bb22cc33dd44ee55ff6677889900/1000x1000-000000-80-0-0.jpg")
    );
    assert!(!top.tracks[2].playable && top.tracks[2].duration_ms.is_none());
    assert_eq!(top.albums.len(), 2);
    assert_eq!(top.albums[0].total_tracks, Some(12));
    assert_eq!(top.albums[1].release_type, ReleaseType::Single);
    assert_eq!(top.artists[0].name, "Монеточка");
    assert!(top.artists[0].image_url.is_some());
    assert_eq!(top.artists[1].image_url, None, "Deezer's placeholder is no picture");

    // The same quick search again: from the cache, no request.
    let again = dz.search_top("ЛЁД", Arc::new(|_| {})).await.unwrap();
    assert_eq!(again, top);

    let all = dz.search("лёд").await.unwrap();
    assert_eq!(all.playlists.len(), 1);
    let p = &all.playlists[0];
    assert_eq!((p.name.as_str(), p.owner.as_deref(), p.total_tracks), ("Лёд и снег", Some("Deezer Editor"), Some(40)));
    assert!(!p.editable);
}

#[tokio::test]
async fn a_single_track_has_its_isrc_and_every_artist_main_first() {
    let server = MockServer::start().await;
    serve(&server, "/track/1551523502", "track.json", 1).await;
    let dz = deezer(&server);
    let t = dz.track("1551523502").await.unwrap();
    assert_eq!(t.isrc.as_deref(), Some("RUA312100012"));
    assert_eq!(t.artist_line(), "Монеточка, Гость");
    assert_eq!((t.track_no, t.disc_no), (Some(3), Some(1)));
    assert_eq!(t.album.as_ref().and_then(|a| a.year), Some(2021));
    // Cached: the mock expects one request.
    let many = dz.tracks(&["1551523502".into(), "abc".into()]).await.unwrap();
    assert_eq!(many.len(), 2);
    assert_eq!(many[0].as_ref().map(|t| t.id.id.as_str()), Some("1551523502"));
    assert!(many[1].is_none(), "not a Deezer id");
}

#[tokio::test]
async fn an_album_lists_its_tracks_numbered_without_fetching_each() {
    let server = MockServer::start().await;
    serve(&server, "/album/302127", "album.json", 1).await;
    let dz = deezer(&server);
    let a = dz.album("302127").await.unwrap();
    assert_eq!((a.title.as_str(), a.year, a.release_type), ("Discovery", Some(2001), ReleaseType::Album));
    assert_eq!(a.artists[0].name, "Daft Punk");
    assert_eq!(a.tracks.len(), 3);
    let nums: Vec<_> = a.tracks.iter().map(|t| t.track_no).collect();
    assert_eq!(nums, vec![Some(1), Some(2), Some(3)]);
    assert!(a.tracks.iter().all(|t| t.isrc.is_none()));
    assert!(a.tracks.iter().all(|t| t.album.as_ref().is_some_and(|r| r.title == "Discovery")));
    assert_eq!(a.tracks[1].cover_url, a.cover_url, "album tracks take the album's cover");
    // Opened again: from the cache.
    assert_eq!(dz.album("302127").await.unwrap(), a);
    dz.forget(EntityKind::Album, "302127");
}

#[tokio::test]
async fn an_artist_comes_with_its_discography_newest_first() {
    let server = MockServer::start().await;
    serve(&server, "/artist/27", "artist.json", 1).await;
    Mock::given(method("GET"))
        .and(path("/artist/27/albums"))
        .and(query_param("limit", "100"))
        .respond_with(json("artist_albums.json"))
        .expect(1)
        .mount(&server)
        .await;
    let dz = deezer(&server);
    let a = dz.artist("27").await.unwrap();
    assert_eq!(a.name, "Daft Punk");
    let titles: Vec<_> = a.albums.iter().map(|al| al.title.as_str()).collect();
    assert_eq!(titles, vec!["Random Access Memories", "Get Lucky (Radio Edit)", "Discovery"]);
    assert!(a.albums.iter().all(|al| al.artists.first().is_some_and(|ar| ar.name == "Daft Punk")));
    assert_eq!(a.albums[1].release_type, ReleaseType::Single);
    assert!(a.albums[0].cover_url.as_deref().is_some_and(|u| u.contains("311bba0fc112d15f72c8b5a65f0456c1")));
    // The day of release, for new releases.
    assert_eq!(a.albums[0].release_date.as_deref(), Some("2013-05-17"));
    assert_eq!(a.albums[2].release_date.as_deref(), Some("2001-03-07"));
    assert!(a.albums[0].released_at_ms() > a.albums[1].released_at_ms());
}

#[tokio::test]
async fn similar_artists_top_tracks_and_radio_are_one_request_each_and_cached() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/artist/27/related"))
        .and(query_param("limit", "20"))
        .respond_with(json("artist_related.json"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/artist/27/top"))
        .and(query_param("limit", "25"))
        .respond_with(json("artist_top.json"))
        .expect(1)
        .mount(&server)
        .await;
    serve(&server, "/artist/13612387/radio", "artist_radio.json", 1).await;
    let dz = deezer(&server);

    let similar = dz.similar_artists("27").await.unwrap();
    let names: Vec<_> = similar.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["Justice", "Cassius"], "an empty entry is dropped");
    assert_eq!(similar[0].id.id, "4050205");
    assert!(similar[0].image_url.is_some());
    assert_eq!(similar[1].image_url, None, "Deezer's placeholder is no picture");

    let top = dz.artist_top("27").await.unwrap();
    assert_eq!(top.len(), 2);
    assert_eq!(top[0].title, "Harder, Better, Faster, Stronger");
    assert_eq!(top[0].album.as_ref().map(|a| a.title.as_str()), Some("Discovery"));
    assert_eq!(top[0].duration_ms, Some(224_000));
    assert_eq!(top[1].artist_line(), "Daft Punk, Pharrell Williams, Nile Rodgers");
    assert!(top[1].cover_url.as_deref().is_some_and(|u| u.contains("311bba0fc112d15f72c8b5a65f0456c1")));

    let radio = dz.artist_radio("13612387").await.unwrap();
    assert_eq!(radio.len(), 3);
    assert_eq!(radio[2].artist_line(), "Oxxxymiron");

    // Again: from the cache (each mock expects one request).
    assert_eq!(dz.similar_artists("27").await.unwrap(), similar);
    assert_eq!(dz.artist_top("27").await.unwrap(), top);
    assert_eq!(dz.artist_radio("13612387").await.unwrap(), radio);
    assert_eq!(dz.artist_top("not-an-id").await, Err(StreamError::Unavailable));
}

#[tokio::test]
async fn related_tracks_are_the_radio_of_the_artist_and_there_is_no_track_radio() {
    let server = MockServer::start().await;
    serve(&server, "/track/1551523502", "track.json", 1).await;
    serve(&server, "/artist/13612387/radio", "artist_radio.json", 1).await;
    let dz = deezer(&server);
    let related = dz.related("1551523502").await.unwrap();
    let titles: Vec<_> = related.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, ["Каждый раз", "Где нас нет"], "without the track itself");
    // The track and the radio are cached now.
    assert_eq!(dz.related("1551523502").await.unwrap(), related);
    assert_eq!(dz.artist_radio("13612387").await.unwrap().len(), 3);
    assert_eq!(dz.radio("1551523502").await, Err(StreamError::Unsupported));
    assert!(dz.cached_audio("1551523502").is_none());
}

#[tokio::test]
async fn discovery_obeys_the_quota_pause() {
    let server = MockServer::start().await;
    serve(&server, "/artist/27/related", "quota.json", 1).await;
    let dz = deezer(&server);
    assert!(matches!(dz.similar_artists("27").await, Err(StreamError::RateLimited { .. })));
    // Paused: no request reaches Deezer (the mock expects exactly one).
    assert!(matches!(dz.artist_top("27").await, Err(StreamError::RateLimited { .. })));
    assert!(matches!(dz.similar_artists("27").await, Err(StreamError::RateLimited { .. })));
}

/// The real API, once by hand: `cargo test -p nikgob-deezer -- --ignored`.
#[tokio::test]
#[ignore = "network"]
async fn live_discovery() {
    let dz = Deezer::new(DeezerConfig::default()).unwrap();
    // Daft Punk.
    let similar = dz.similar_artists("27").await.unwrap();
    assert!(!similar.is_empty(), "{similar:?}");
    let top = dz.artist_top("27").await.unwrap();
    assert!(top.iter().any(|t| t.artist_line().contains("Daft Punk")), "{top:?}");
    let radio = dz.artist_radio("27").await.unwrap();
    assert!(!radio.is_empty());
    let related = dz.related(&top[0].id.id).await.unwrap();
    assert!(related.iter().all(|t| t.id != top[0].id));
    let artist = dz.artist("27").await.unwrap();
    assert!(artist.albums.iter().any(|a| a.release_date.is_some()), "{:?}", artist.albums);
}

#[tokio::test]
async fn a_playlist_reads_its_remaining_pages() {
    let server = MockServer::start().await;
    serve(&server, "/playlist/9100200300", "playlist.json", 1).await;
    Mock::given(method("GET"))
        .and(path("/playlist/9100200300/tracks"))
        .and(query_param("index", "2"))
        .respond_with(json("playlist_tracks_page2.json"))
        .expect(1)
        .mount(&server)
        .await;
    let dz = deezer(&server);
    let p = dz.playlist("9100200300").await.unwrap();
    assert_eq!(p.version.as_deref(), Some("c0ffee"));
    let titles: Vec<_> = p.tracks.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, vec!["Лёд", "One More Time", "Digital Love"]);
}

#[tokio::test]
async fn an_isrc_finds_its_track_and_no_data_is_none() {
    let server = MockServer::start().await;
    serve(&server, "/track/isrc:RUA312100012", "track.json", 1).await;
    serve(&server, "/track/isrc:ZZZZZ0000000", "no_data.json", 1).await;
    serve(&server, "/album/1", "no_data.json", 1).await;
    let dz = deezer(&server);
    let hit = dz.find_isrc("ru-a31-21-00012").await.unwrap().unwrap();
    assert_eq!(hit.id.id, "1551523502");
    assert_eq!(dz.find_isrc("ZZZZZ0000000").await.unwrap(), None);
    assert_eq!(dz.find_isrc("short").await.unwrap(), None, "not an ISRC: no request");
    assert_eq!(dz.album("1").await, Err(StreamError::Unavailable));
}

#[tokio::test]
async fn the_quota_error_pauses_the_service_and_calls_fail_at_once_meanwhile() {
    let server = MockServer::start().await;
    serve(&server, "/album/302127", "quota.json", 1).await;
    let dz = deezer(&server);
    match dz.album("302127").await {
        Err(StreamError::RateLimited { retry_after: Some(d) }) => assert!(d.as_secs() <= 5),
        other => panic!("{other:?}"),
    }
    assert!(dz.paused_until().is_some());
    // Paused: no request reaches Deezer (the mock expects exactly one).
    assert!(matches!(dz.album("302127").await, Err(StreamError::RateLimited { .. })));
    assert!(matches!(dz.search_top("лёд", Arc::new(|_| {})).await, Err(StreamError::RateLimited { .. })));
}

#[tokio::test]
async fn http_429_is_a_rate_limit_too() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/artist"))
        .respond_with(ResponseTemplate::new(429))
        .expect(1)
        .mount(&server)
        .await;
    let dz = deezer(&server);
    assert!(matches!(
        dz.search_kind("кино", SearchKind::Artists).await,
        Err(StreamError::RateLimited { retry_after: Some(_) })
    ));
}

#[tokio::test]
async fn no_account_no_sound_no_likes() {
    let server = MockServer::start().await;
    let dz = deezer(&server);
    assert!(dz.catalogue_only());
    assert_eq!(dz.account(), Account::LoggedOut);
    assert_eq!(dz.login(LoginInput::Browser).await, Err(StreamError::Unsupported));
    assert!(matches!(dz.open_audio("1"), Err(StreamError::Unsupported)));
    assert_eq!(dz.set_liked("1", true).await, Err(StreamError::Unsupported));
    assert_eq!(dz.liked().await.map(|l| l.len()), Err(StreamError::Unsupported));
    assert_eq!(dz.search_kind("x", SearchKind::Videos).await.map(|s| s.tracks.len()), Ok(0));
    assert_eq!(dz.web_url(EntityKind::Album, "302127"), "https://www.deezer.com/album/302127");
    // Nothing was asked.
    assert!(server.received_requests().await.unwrap_or_default().is_empty());
}
