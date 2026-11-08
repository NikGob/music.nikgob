//! The Internet Archive against a mocked site answering with its recorded
//! (trimmed) JSON. No real network.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use nikgob_core::source::{Fetched, OfferScope, Progress, Source, SourceError, SourceKey, Want, WantScope};
use nikgob_sources::archive::Archive;
use nikgob_sources::select::{pick, rank};
use wiremock::matchers::{path, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pages").join(name)).unwrap()
}

fn json(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body, "application/json")
}

fn flac_bytes() -> Vec<u8> {
    let mut b = b"fLaC".to_vec();
    b.extend(std::iter::repeat_n(7u8, 4096));
    b
}

fn quiet() -> Progress {
    Arc::new(|_, _| {})
}

fn noch() -> Want {
    Want {
        artists: vec!["Кино".into()],
        album: Some("Ночь".into()),
        scope: WantScope::Album,
        album_tracks: 4,
        ..Want::default()
    }
}

/// The search, the one item with FLAC among them (a zip-only item, a live
/// one and an MP3 one around it), the album's four FLACs.
async fn site() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(path("/advancedsearch.php"))
        .and(query_param("output", "json"))
        .respond_with(json(fixture("archive_search.json")))
        .mount(&server)
        .await;
    Mock::given(path("/metadata/kino-noch-1985"))
        .respond_with(json(fixture("archive_item.json")))
        .mount(&server)
        .await;
    // The MP3 item has nothing usable for the album.
    Mock::given(path("/metadata/kino-noch-mp3")).respond_with(json("{}".into())).mount(&server).await;
    Mock::given(path_regex(r"^/download/kino-noch-1985/.*\.flac$"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(flac_bytes(), "audio/flac"))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn the_archive_finds_an_album_and_downloads_its_flacs() {
    let server = site().await;
    let ia = Archive::with_pace(&server.uri(), Duration::ZERO);
    let want = noch();
    let offers = ia.search(&want).await.unwrap();
    assert_eq!(offers.len(), 1, "{offers:?}");
    assert_eq!(offers[0].scope, OfferScope::Album);
    assert_eq!(offers[0].source, SourceKey::Archive);
    let ranked = rank(offers, &want, &[SourceKey::Archive]);
    assert_eq!(pick(&ranked, None), Some(0));

    let dir = tempfile::tempdir().unwrap();
    let Fetched::Files(files) = ia.fetch(&ranked[0].offer, &want, dir.path(), quiet()).await.unwrap() else {
        panic!("files expected")
    };
    let mut names: Vec<String> = files.iter().map(|f| f.file_name().unwrap().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["01 - Видели ночь.flac", "02 - Фильмы.flac", "03 - Твой номер.flac", "04 - Танец.flac"]);
    assert_eq!(std::fs::read(&files[0]).unwrap(), flac_bytes());
    // The live item and the zip-only one were never looked into.
    let asked: Vec<String> = server.received_requests().await.unwrap().iter().map(|r| r.url.path().to_owned()).collect();
    assert!(!asked.iter().any(|p| p.contains("live") || p.contains("zip-only")), "{asked:?}");
    assert!(!asked.iter().any(|p| p.ends_with(".mp3")), "the derived MP3s are not downloaded");
}

#[tokio::test]
async fn the_rest_of_a_partly_downloaded_album_comes_alone() {
    let server = site().await;
    let ia = Archive::with_pace(&server.uri(), Duration::ZERO);
    let want = Want { got: vec!["Видели ночь".into(), "Танец".into()], ..noch() };
    let offers = ia.search(&want).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let Fetched::Files(files) = ia.fetch(&offers[0], &want, dir.path(), quiet()).await.unwrap() else { panic!() };
    assert_eq!(files.len(), 2);
}

#[tokio::test]
async fn a_short_wait_asked_for_is_waited_out_and_asked_again() {
    let server = site().await;
    Mock::given(path("/advancedsearch.php"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    let ia = Archive::with_pace(&server.uri(), Duration::ZERO);
    assert_eq!(ia.search(&noch()).await.unwrap().len(), 1);
}

#[tokio::test]
async fn an_archive_that_keeps_refusing_sits_out() {
    let server = MockServer::start().await;
    Mock::given(path("/advancedsearch.php"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "3600"))
        .expect(1)
        .mount(&server)
        .await;
    let ia = Archive::with_pace(&server.uri(), Duration::ZERO);
    let err = ia.search(&noch()).await.unwrap_err();
    assert!(matches!(err, SourceError::RateLimited { .. }), "{err:?}");
    // Asked again at once: no request, it waits.
    let again = ia.search(&noch()).await.unwrap_err();
    assert!(matches!(again, SourceError::RateLimited { .. }), "{again:?}");
}
