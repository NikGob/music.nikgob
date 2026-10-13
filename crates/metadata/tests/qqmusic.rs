mod common;

use std::time::Duration;

use nikgob_metadata::{QqMusic, qq_lines};
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(server: &MockServer) -> QqMusic {
    QqMusic::with_base_url(common::http(), &server.uri()).unwrap()
}

async fn mount_search(server: &MockServer, items: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path("/splcloud/fcgi-bin/smartbox_new.fcg"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"code": 0, "data": {"song": {"count": 1, "itemlist": items}}}),
            ),
        )
        .mount(server)
        .await;
}

const LYRIC_PATH: &str = "/lyric/fcgi-bin/fcg_query_lyric_new.fcg";

#[tokio::test]
async fn finds_lyrics_decodes_entities_and_attaches_translation() {
    let server = MockServer::start().await;
    mount_search(
        &server,
        json!([
            {"mid": "wrong", "name": "Ghost Avenue", "singer": "Someone Else"},
            {"mid": "m1", "name": "Ghost Avenue", "singer": "Eve"},
        ]),
    )
    .await;
    Mock::given(method("GET"))
        .and(path(LYRIC_PATH))
        .and(query_param("songmid", "m1"))
        .and(query_param("nobase64", "1"))
        .and(header("Referer", "https://y.qq.com/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"MusicJsonCallback({"retcode":0,"lyric":"[ti:Ghost Avenue]\n[00:00.00]ゴースト &#45; Eve\n[00:00.26]词&#58;Eve\n[00:01.07]do&apos;nt&#32;go\n","trans":"[00:01.07]别走\n"})"#,
        ))
        .expect(1)
        .mount(&server)
        .await;

    let l = client(&server)
        .find("Eve", "Ghost Avenue", None, Some(Duration::from_secs(173)))
        .await
        .unwrap()
        .expect("lyrics");
    assert_eq!(l.mid, "m1");
    let lines = qq_lines(&l);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text, "do'nt go");
    assert_eq!(lines[0].translation.as_deref(), Some("别走"));
}

#[tokio::test]
async fn lyrics_longer_than_the_track_are_rejected() {
    let server = MockServer::start().await;
    mount_search(
        &server,
        json!([{"mid": "m1", "name": "Song", "singer": "Eve"}]),
    )
    .await;
    Mock::given(method("GET"))
        .and(path(LYRIC_PATH))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"lyric": "[00:01.00]a\n[05:00.00]b\n"})),
        )
        .mount(&server)
        .await;
    let c = client(&server);
    assert!(
        c.find("Eve", "Song", None, Some(Duration::from_secs(120)))
            .await
            .unwrap()
            .is_none()
    );
    assert!(c.find("Eve", "Song", None, None).await.unwrap().is_some());
}

#[tokio::test]
async fn no_hits_or_wrong_artist_is_none() {
    let server = MockServer::start().await;
    mount_search(&server, json!([])).await;
    assert!(
        client(&server)
            .find("Eve", "Song", None, None)
            .await
            .unwrap()
            .is_none()
    );

    let server = MockServer::start().await;
    mount_search(
        &server,
        json!([{"mid": "m", "name": "Song", "singer": "Completely Different"}]),
    )
    .await;
    assert!(
        client(&server)
            .find("Eve", "Song", None, None)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn instrumental_has_no_lines() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(LYRIC_PATH))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"lyric": "[00:00.00]此歌曲为没有填词的纯音乐，请您欣赏"})),
        )
        .mount(&server)
        .await;
    let l = client(&server).lyrics("x").await.unwrap().expect("lyrics");
    assert!(qq_lines(&l).is_empty());
}
