mod common;

use std::time::Duration;

use nikgob_metadata::MetadataError;
use nikgob_metadata::deezer::Deezer;
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("fixtures/", $name))
    };
}

fn json_body(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.as_bytes().to_vec(), "application/json")
}

fn client(server: &MockServer) -> Deezer {
    Deezer::with_base_url(common::http(), &server.uri()).unwrap()
}

#[tokio::test]
async fn search_track_uses_structured_query_and_parses_hits() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/track"))
        .and(query_param("q", r#"artist:"Кино" track:"Группа крови""#))
        .and(query_param("limit", "5"))
        .respond_with(json_body(fixture!("deezer_search.json")))
        .expect(1)
        .mount(&server)
        .await;

    let hits = client(&server)
        .search_track(Some("Кино"), "Группа крови", 5)
        .await
        .unwrap();
    assert_eq!(hits.len(), 2);
    let first = &hits[0];
    assert_eq!(first.id, 3_135_556);
    assert_eq!(first.artist_name(), "Кино");
    assert_eq!(first.duration(), Some(Duration::from_secs(285)));
    assert_eq!(first.isrc, None, "search results carry no ISRC");
    let album = first.album.as_ref().unwrap();
    assert_eq!(album.title, "Группа крови");
    assert!(album.best_cover().unwrap().contains("1000x1000"));
    assert!(hits[1].album.as_ref().unwrap().cover_big.is_none());
    assert!(
        hits[1]
            .album
            .as_ref()
            .unwrap()
            .best_cover()
            .unwrap()
            .contains("1000x1000")
    );
}

#[tokio::test]
async fn quotes_in_the_query_are_stripped() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/track"))
        .and(query_param("q", r#"track:"Say Hi""#))
        .respond_with(json_body(r#"{"data":[],"total":0}"#))
        .expect(1)
        .mount(&server)
        .await;
    let hits = client(&server)
        .search_track(None, r#"Say "Hi""#, 10)
        .await
        .unwrap();
    assert!(hits.is_empty());
}

#[tokio::test]
async fn track_lookup_has_isrc_and_position() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/track/3135556"))
        .respond_with(json_body(fixture!("deezer_track.json")))
        .mount(&server)
        .await;
    let track = client(&server).track(3_135_556).await.unwrap();
    assert_eq!(track.isrc.as_deref(), Some("RUA011100001"));
    assert_eq!(track.track_position, Some(9));
    assert_eq!(track.disk_number, Some(1));
    assert_eq!(track.release_date.as_deref(), Some("1988-06-01"));
}

#[tokio::test]
async fn album_lookup_returns_tracks_and_covers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/album/302127"))
        .respond_with(json_body(fixture!("deezer_album.json")))
        .expect(1)
        .mount(&server)
        .await;
    let album = client(&server).album(302_127).await.unwrap();
    assert_eq!(album.title, "Группа крови");
    assert_eq!(album.upc.as_deref(), Some("4600000000000"));
    assert_eq!(album.record_type.as_deref(), Some("album"));
    assert_eq!(album.artist.as_ref().unwrap().name, "Кино");
    assert_eq!(album.tracks.len(), 2);
    assert_eq!(album.tracks[1].title, "Группа крови");
    assert!(album.best_cover().unwrap().contains("1000x1000"));
}

#[tokio::test]
async fn album_pages_through_missing_tracks() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/album/7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 7, "title": "Long", "nb_tracks": 3,
            "tracks": {"data": [{"id": 1, "title": "a"}, {"id": 2, "title": "b"}]}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/album/7/tracks"))
        .and(query_param("index", "2"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data": [{"id": 3, "title": "c"}]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let album = client(&server).album(7).await.unwrap();
    let ids: Vec<u64> = album.tracks.iter().map(|t| t.id).collect();
    assert_eq!(ids, [1, 2, 3]);
}

#[tokio::test]
async fn errors_come_in_the_body_with_http_200() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/track/1"))
        .respond_with(json_body(fixture!("deezer_error.json")))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/track/2"))
        .respond_with(json_body(
            r#"{"error":{"type":"Exception","message":"Quota limit exceeded","code":4}}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/track/3"))
        .respond_with(json_body(
            r#"{"error":{"type":"OAuthException","message":"Something odd","code":300}}"#,
        ))
        .mount(&server)
        .await;
    let cl = client(&server);
    assert!(matches!(cl.track(1).await, Err(MetadataError::NotFound)));
    assert!(matches!(
        cl.track(2).await,
        Err(MetadataError::RateLimited(None))
    ));
    assert!(matches!(
        cl.track(3).await,
        Err(MetadataError::Service(m)) if m == "Something odd"
    ));
}
