mod common;

use std::time::Duration;

use nikgob_metadata::{Netease, netease_lines};
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(server: &MockServer) -> Netease {
    Netease::with_base_url(common::http(), &server.uri()).unwrap()
}

fn search_body() -> serde_json::Value {
    json!({"result": {"songs": [
        {"id": 1, "name": "洗脳 (Remix)", "artists": [{"name": "Other"}], "album": {"name": "x"}, "duration": 200_000},
        {"id": 2, "name": "洗脳", "artists": [{"name": "DECO*27"}], "album": {"name": "y"}, "duration": 171_384},
    ]}})
}

#[tokio::test]
async fn finds_by_duration_and_fetches_lyrics() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/search/get"))
        .and(query_param("type", "1"))
        .and(header("Referer", "https://music.163.com"))
        .respond_with(ResponseTemplate::new(200).set_body_json(search_body()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/song/lyric"))
        .and(query_param("id", "2"))
        .and(header("Referer", "https://music.163.com"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "lrc": {"lyric": "[00:00.000] 作词 : DECO*27\n[00:03.275] 嫉妬\n"},
            "tlyric": {"lyric": "[00:03.275] 嫉妒\n"},
            "romalrc": {"lyric": ""},
        })))
        .expect(1)
        .mount(&server)
        .await;

    let l = client(&server)
        .find(
            "DECO*27",
            "洗脳",
            None,
            Some(Duration::from_millis(171_000)),
        )
        .await
        .unwrap()
        .expect("lyrics");
    assert_eq!(l.id, 2);
    assert!(l.romanization.is_none() && l.yrc.is_none());
    let lines = netease_lines(&l);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text, "嫉妬");
    assert_eq!(lines[0].translation.as_deref(), Some("嫉妒"));
}

#[tokio::test]
async fn romanized_artist_is_accepted_on_a_tight_duration() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/search/get"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"result": {"songs": [
            {"id": 7, "name": "Ghost Avenue", "artists": [{"name": "ずっと真夜中でいいのに。"}], "duration": 173_606}
        ]}})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/song/lyric"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"lrc": {"lyric": "[00:01.000]a"}})),
        )
        .mount(&server)
        .await;
    let c = client(&server);
    let tight = c
        .find(
            "Zutomayo",
            "Ghost Avenue",
            None,
            Some(Duration::from_millis(173_000)),
        )
        .await
        .unwrap();
    assert!(tight.is_some());
    let loose = c
        .find(
            "Zutomayo",
            "Ghost Avenue",
            None,
            Some(Duration::from_millis(175_500)),
        )
        .await
        .unwrap();
    assert!(loose.is_none(), "2.1 s off needs a matching artist");
}

#[tokio::test]
async fn wrong_duration_or_missing_result_is_none() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/search/get"))
        .respond_with(ResponseTemplate::new(200).set_body_json(search_body()))
        .mount(&server)
        .await;
    let c = client(&server);
    let none = c
        .find("DECO*27", "洗脳", None, Some(Duration::from_secs(100)))
        .await
        .unwrap();
    assert!(none.is_none());

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/search/get"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"code": 200})))
        .mount(&server)
        .await;
    assert!(
        client(&server)
            .find("a", "b", None, None)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn instrumental_and_uncollected() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/song/lyric"))
        .and(query_param("id", "5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"pureMusic": true})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/song/lyric"))
        .and(query_param("id", "6"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"nolyric": true, "uncollected": true})),
        )
        .mount(&server)
        .await;
    let c = client(&server);
    let l = c.lyrics(5).await.unwrap().expect("instrumental");
    assert!(l.instrumental);
    assert!(netease_lines(&l).is_empty());
    assert!(c.lyrics(6).await.unwrap().is_none());
}
