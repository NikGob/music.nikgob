//! The share pages against a local mock: MusicLink's resolve redirect and
//! API, and the song.link page check.

mod common;

use nikgob_metadata::{MusicLink, Odesli};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TRACK: &str = "https://www.deezer.com/track/3966082521";
const ALBUM: &str = "https://www.deezer.com/album/544629332";

#[tokio::test]
async fn musiclink_follows_the_redirect_once_per_link() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/resolve"))
        .and(query_param("q", TRACK))
        .respond_with(ResponseTemplate::new(307).insert_header("location", "/song/mzlff/ostav-sebe"))
        .expect(1)
        .mount(&server)
        .await;
    // An album new to it: the paste box page, built in the browser.
    Mock::given(method("GET"))
        .and(path("/resolve"))
        .and(query_param("q", ALBUM))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html></html>"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/resolve"))
        .and(query_param("q", "https://www.deezer.com/track/500"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let ml = MusicLink::with_base_url(common::http(), &server.uri()).unwrap();
    for _ in 0..2 {
        assert_eq!(
            ml.page(TRACK).await.unwrap().as_deref(),
            Some("https://musiclink.one/song/mzlff/ostav-sebe")
        );
        assert_eq!(ml.page(ALBUM).await.unwrap(), None);
    }
    assert!(ml.page("https://www.deezer.com/track/500").await.is_err());
}

#[tokio::test]
async fn musiclink_api_builds_with_the_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/resolve"))
        .and(query_param("q", ALBUM))
        .and(header("authorization", "Bearer k1"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            br#"{"success":true,"data":[{"type":"album","url":"https://musiclink.one/album/mzlff/nichego-plokhogo"}]}"#.to_vec(),
            "application/json",
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/resolve"))
        .and(header("authorization", "Bearer bad"))
        .respond_with(ResponseTemplate::new(401).set_body_string(r#"{"success":false,"error":"Invalid API key"}"#))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/resolve"))
        .and(query_param("q", TRACK))
        .and(header("authorization", "Bearer k1"))
        .respond_with(ResponseTemplate::new(404).set_body_string(r#"{"success":false,"error":"Nothing matched"}"#))
        .mount(&server)
        .await;

    let ml = MusicLink::with_base_url(common::http(), &server.uri()).unwrap();
    assert_eq!(
        ml.api_page("k1", ALBUM).await.unwrap().as_deref(),
        Some("https://musiclink.one/album/mzlff/nichego-plokhogo")
    );
    assert_eq!(ml.api_page("k1", TRACK).await.unwrap(), None);
    let err = ml.api_page("bad", "https://www.deezer.com/album/1").await.unwrap_err();
    assert!(matches!(err, nikgob_metadata::MetadataError::Status { status: 401, .. }), "{err:?}");

    // Another key is asked again although the first one's miss is cached.
    Mock::given(method("GET"))
        .and(path("/v2/resolve"))
        .and(query_param("q", TRACK))
        .and(header("authorization", "Bearer k2"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            br#"{"success":true,"data":[{"type":"track","url":"https://musiclink.one/song/mzlff/ostav-sebe"}]}"#.to_vec(),
            "application/json",
        ))
        .expect(1)
        .mount(&server)
        .await;
    for _ in 0..2 {
        assert_eq!(
            ml.api_page("k2", TRACK).await.unwrap().as_deref(),
            Some("https://musiclink.one/song/mzlff/ostav-sebe")
        );
    }
    assert_eq!(ml.api_page("k1", TRACK).await.unwrap(), None);
}

#[tokio::test]
async fn oversized_answers_are_errors() {
    let server = MockServer::start().await;
    let big = format!(r#"<meta property="og:title" content="x"/>{}"#, " ".repeat(2 * 1024 * 1024));
    Mock::given(method("GET"))
        .and(path("/d/9"))
        .respond_with(ResponseTemplate::new(200).set_body_string(big))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/resolve"))
        .respond_with(ResponseTemplate::new(500).set_body_string("e".repeat(1024 * 1024)))
        .mount(&server)
        .await;
    let odesli = Odesli::with_base_url(common::http(), &server.uri()).unwrap();
    assert!(odesli.has_page("https://album.link/d/9").await.is_err());
    let ml = MusicLink::with_base_url(common::http(), &server.uri()).unwrap();
    let err = ml.api_page("k", TRACK).await.unwrap_err();
    assert!(matches!(err, nikgob_metadata::MetadataError::Parse(_)), "{err:?}");
}

#[tokio::test]
async fn a_songlink_page_is_checked_once() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/d/544629332"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<html><head><meta property="og:title" content="Ничего плохого by Mzlff"/></head></html>"#,
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/d/1"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><head></head></html>"))
        .mount(&server)
        .await;

    let odesli = Odesli::with_base_url(common::http(), &server.uri()).unwrap();
    for _ in 0..2 {
        assert!(odesli.has_page("https://album.link/d/544629332").await.unwrap());
    }
    assert!(!odesli.has_page("https://album.link/d/1").await.unwrap());
}

/// The real sites (`cargo test -p nikgob-metadata --test odesli -- --ignored`).
#[tokio::test]
#[ignore = "asks musiclink.one and song.link"]
async fn live_pages() {
    let http = nikgob_metadata::Http::new(&nikgob_metadata::MetadataConfig::new(common::CONTACT)).unwrap();
    let ml = MusicLink::new(http.clone());
    let song = ml.page(TRACK).await.unwrap().expect("a song page");
    assert!(song.starts_with("https://musiclink.one/song/"), "{song}");
    let known = ml.page("https://open.spotify.com/album/2noRn2Aes5aoNVsU6iWThc").await.unwrap();
    assert_eq!(known.as_deref(), Some("https://musiclink.one/album/daft-punk/discovery"));
    let odesli = Odesli::new(http);
    assert!(odesli.has_page("https://album.link/d/544629332").await.unwrap());
    assert!(!odesli.has_page("https://album.link/y/MPREb_OLmD8O5IYNS").await.unwrap());
}
