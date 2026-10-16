mod common;

use std::time::Duration;

use nikgob_metadata::{BetterLyrics, Deezer, LrcRed, LyricsPlus, MetadataError};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TTML: &str = r#"<tt xmlns:lrc="http://lrc.red/lyric-ttml-internal"><body><div><p begin="1" end="2" lrc:key="L1"><span begin="1" end="2">Hi</span></p></div></body></tt>"#;

#[tokio::test]
async fn lyricsplus_sends_known_fields_and_404_is_a_miss() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/lyrics/get"))
        .and(query_param("title", "Song"))
        .and(query_param("artist", "Artist"))
        .and(query_param("album", "Album"))
        .and(query_param("duration", "200.5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "type":"Line", "lyrics":[{"time":1000,"duration":500,"text":"Hi"}]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = LyricsPlus::with_base_url(common::http(), &server.uri()).unwrap();
    let lines = client
        .find(
            "Artist",
            "Song",
            Some("Album"),
            Some(Duration::from_millis(200_500)),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lines[0].end, Some(Duration::from_millis(1500)));
    assert!(
        client
            .find("Artist", "Missing", None, None)
            .await
            .unwrap()
            .is_none()
    );
    assert!(client.find("", "Song", None, None).await.is_err());
}

#[tokio::test]
async fn lyricsplus_json_error_404_is_not_a_network_error() {
    let server = MockServer::start().await;
    Mock::given(path("/v2/lyrics/get"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_json(serde_json::json!({"error":"No lyrics found"})),
        )
        .mount(&server)
        .await;
    let client = LyricsPlus::with_base_url(common::http(), &server.uri()).unwrap();
    assert!(
        client
            .find("Artist", "Song", None, None)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn lrcred_fetches_verified_deezer_isrc_and_ignores_wrong_hits() {
    let server = MockServer::start().await;
    Mock::given(path("/search/track"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data":[
            {"id":1,"title":"Song","artist":{"name":"Wrong"},"duration":200,"isrc":"USUG11904206"},
            {"id":2,"title":"Song","artist":{"name":"Artist"},"duration":204,"isrc":"USUG11904206"},
            {"id":3,"title":"Song","artist":{"name":"Artist"},"duration":200}
        ]}))).mount(&server).await;
    Mock::given(path("/track/3"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id":3,"title":"Song","artist":{"name":"Artist"},"duration":200,"isrc":"USUG11904206"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/s/USUG11904206.ttml"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(TTML, "application/xml"))
        .expect(1)
        .mount(&server)
        .await;
    let client = LrcRed::with_base_url(common::http(), &server.uri()).unwrap();
    let deezer = Deezer::with_base_url(common::http(), &server.uri()).unwrap();
    let lines = client
        .find(
            &deezer,
            None,
            "Artist",
            "Song",
            Some(Duration::from_secs(200)),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lines[0].text, "Hi");
    assert!(client.get("USUG11904207").await.unwrap().is_none());
    assert!(client.get("../bad").await.is_err());
    let requests = server.received_requests().await.unwrap();
    assert!(
        !requests
            .iter()
            .any(|r| matches!(r.url.path(), "/track/1" | "/track/2"))
    );
}

#[tokio::test]
async fn better_lyrics_mirrors_server_errors_but_not_misses_or_rate_limits() {
    for status in [502, 503, 404, 429] {
        let server = MockServer::start().await;
        let mirror = MockServer::start().await;
        Mock::given(path("/getLyrics"))
            .respond_with(ResponseTemplate::new(status))
            .mount(&server)
            .await;
        Mock::given(path("/getLyrics"))
            .and(query_param("s", "Song"))
            .and(query_param("a", "Artist"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"ttml":TTML})),
            )
            .expect(if status >= 500 { 1 } else { 0 })
            .mount(&mirror)
            .await;
        let client =
            BetterLyrics::with_base_urls(common::http(), &server.uri(), &mirror.uri()).unwrap();
        let result = client.find("Artist", "Song").await;
        match status {
            404 => assert!(result.unwrap().is_none()),
            429 => assert!(matches!(result, Err(MetadataError::RateLimited(_)))),
            _ => assert_eq!(result.unwrap().unwrap()[0].text, "Hi"),
        }
    }
}

#[tokio::test]
async fn better_lyrics_mirrors_transport_failures() {
    let mirror = MockServer::start().await;
    Mock::given(path("/getLyrics"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ttml":TTML})))
        .expect(1)
        .mount(&mirror)
        .await;
    let client =
        BetterLyrics::with_base_urls(common::http(), "http://127.0.0.1:9/", &mirror.uri()).unwrap();
    assert_eq!(
        client.find("Artist", "Song").await.unwrap().unwrap()[0].text,
        "Hi"
    );
}

#[tokio::test]
#[ignore = "live service sanity check"]
async fn live_new_lyrics_sources() {
    let http = common::http();
    let lines = LyricsPlus::new(http.clone())
        .find(
            "The Weeknd",
            "Blinding Lights",
            None,
            Some(Duration::from_secs(200)),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(!lines.is_empty());
    let lines = LrcRed::new(http.clone())
        .get("USUG11904206")
        .await
        .unwrap()
        .unwrap();
    assert!(lines.iter().any(|l| !l.words.is_empty()));
    let lines = BetterLyrics::new(http)
        .find("The Weeknd", "Blinding Lights")
        .await
        .unwrap()
        .unwrap();
    assert!(lines.iter().any(|l| !l.words.is_empty()));
}
