mod common;

use std::time::Duration;

use nikgob_metadata::lrclib::Lrclib;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("fixtures/", $name))
    };
}

fn json_body(status: u16, body: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_raw(body.as_bytes().to_vec(), "application/json")
}

fn client(server: &MockServer) -> Lrclib {
    Lrclib::with_base_url(common::http(), &server.uri()).unwrap()
}

#[tokio::test]
async fn get_returns_synced_and_plain_lyrics() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/get"))
        .and(query_param("artist_name", "Borislav Slavov"))
        .and(query_param("track_name", "I Want to Live"))
        .and(query_param(
            "album_name",
            "Baldurs Gate 3 (Original Game Soundtrack)",
        ))
        .and(query_param("duration", "233"))
        .respond_with(json_body(200, fixture!("lrclib_get.json")))
        .expect(1)
        .mount(&server)
        .await;

    let lyrics = client(&server)
        .get(
            "Borislav Slavov",
            "I Want to Live",
            Some("Baldurs Gate 3 (Original Game Soundtrack)"),
            Some(Duration::from_millis(232_800)),
        )
        .await
        .unwrap()
        .expect("lyrics");
    assert_eq!(lyrics.id, 3_396_226);
    assert!(!lyrics.instrumental);
    assert!(lyrics.plain_lyrics.as_deref().unwrap().contains("hunger"));
    assert!(lyrics.has_synced());
    let lines = lyrics.synced_lines();
    assert_eq!(lines.len(), 3);
    assert_eq!(
        lines[0],
        (
            Duration::from_millis(17_120),
            "I feel your breath upon my neck".into()
        )
    );
    assert_eq!(lines[1].0, Duration::from_millis(21_500));
    assert_eq!(lines[2], (Duration::from_millis(25_970), String::new()));
}

#[tokio::test]
async fn get_404_is_none() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/get"))
        .respond_with(json_body(404, fixture!("lrclib_not_found.json")))
        .mount(&server)
        .await;
    let found = client(&server)
        .get("Nobody", "Nothing", None, None)
        .await
        .unwrap();
    assert!(found.is_none());
    // album/duration are optional parameters: they are left out when unknown.
    let requests = server.received_requests().await.unwrap();
    let query = requests[0].url.query().unwrap();
    assert!(!query.contains("album_name") && !query.contains("duration"));
}

#[tokio::test]
async fn search_parses_the_array() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/search"))
        .and(query_param("track_name", "Группа крови"))
        .and(query_param("artist_name", "Кино"))
        .respond_with(json_body(200, fixture!("lrclib_search.json")))
        .mount(&server)
        .await;
    let hits = client(&server)
        .search(Some("Кино"), "Группа крови", None)
        .await
        .unwrap();
    assert_eq!(hits.len(), 3);
    assert_eq!(hits[1].duration, Some(285.4));
    assert!(hits[0].synced_lyrics.is_none());
    assert!(hits[0].synced_lines().is_empty());
}

#[tokio::test]
async fn find_falls_back_to_search_and_picks_synced_within_the_duration_window() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/get"))
        .respond_with(json_body(404, fixture!("lrclib_not_found.json")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/search"))
        .respond_with(json_body(200, fixture!("lrclib_search.json")))
        .expect(1)
        .mount(&server)
        .await;
    let best = client(&server)
        .find("Кино", "Группа крови", None, Some(Duration::from_secs(284)))
        .await
        .unwrap()
        .expect("a match");
    // id 11 is 46 s off; id 13 matches exactly but has no synced lyrics; id 12 wins.
    assert_eq!(best.id, 12);
    let lines = best.synced_lines();
    assert_eq!(lines[1].0, Duration::from_millis(18_300));
}

#[tokio::test]
async fn find_returns_none_when_every_hit_is_too_far_off() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/get"))
        .respond_with(json_body(404, fixture!("lrclib_not_found.json")))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/search"))
        .respond_with(json_body(200, fixture!("lrclib_search.json")))
        .mount(&server)
        .await;
    let best = client(&server)
        .find("Кино", "Группа крови", None, Some(Duration::from_secs(60)))
        .await
        .unwrap();
    assert!(best.is_none());
}

#[tokio::test]
async fn find_rejects_search_hits_of_another_song() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/get"))
        .respond_with(json_body(404, fixture!("lrclib_not_found.json")))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/search"))
        .respond_with(json_body(200, fixture!("lrclib_search.json")))
        .mount(&server)
        .await;
    // The duration fits id 12, but it is "Группа крови" by Кино, not this song.
    for (artist, title) in [("Кино", "Звезда по имени Солнце"), ("Земфира", "Группа крови")] {
        let best = client(&server)
            .find(artist, title, None, Some(Duration::from_secs(285)))
            .await
            .unwrap();
        assert!(best.is_none(), "{artist} - {title}: {best:?}");
    }
}
