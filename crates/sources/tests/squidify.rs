//! The Subsonic source against a mocked Navidrome answering as
//! squidify.org did on 2026-11-09 (the answers in `pages/squidify_*.json`
//! are its own, trimmed): a guest login that may stream but not download.
//! No real network.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use nikgob_core::source::{Fetched, OfferScope, Progress, Source, SourceError, SourceKey, Want, WantScope};
use nikgob_sources::select::{pick, rank};
use nikgob_sources::squidify::{Squidify, token};
use serde_json::json;
use wiremock::matchers::{path, query_param};
use wiremock::{Match, Mock, MockServer, Request, ResponseTemplate};

fn page(name: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pages").join(name)).unwrap()
}

fn json_answer(body: String) -> ResponseTemplate {
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

/// What squidify.org answers a guest's `download`.
const DOWNLOADS_OFF: &str = r#"{"subsonic-response":{"status":"failed","version":"1.16.1","type":"navidrome","serverVersion":"0.64.2 (10114574)","openSubsonic":true,"error":{"code":50,"message":"downloads are disabled"}}}"#;
/// And a wrong password.
const WRONG_LOGIN: &str = r#"{"subsonic-response":{"status":"failed","version":"1.16.1","type":"navidrome","serverVersion":"0.64.2 (10114574)","openSubsonic":true,"error":{"code":40,"message":"Wrong username or password"}}}"#;

/// A request logged in as `user` with a right salted token of `password`.
struct Login {
    user: &'static str,
    password: &'static str,
}

impl Match for Login {
    fn matches(&self, req: &Request) -> bool {
        let q: std::collections::HashMap<String, String> = req.url.query_pairs().into_owned().collect();
        let get = |k: &str| q.get(k).map(String::as_str).unwrap_or_default();
        get("u") == self.user
            && !get("s").is_empty()
            && get("t") == token(self.password, get("s"))
            && !q.contains_key("p")
            && get("v") == "1.16.1"
            && get("f") == "json"
    }
}

const GUEST: Login = Login { user: "Guest", password: "Guest" };

#[tokio::test]
async fn a_soundtrack_song_comes_through_the_raw_stream_when_downloads_are_off() {
    let server = MockServer::start().await;
    Mock::given(path("/rest/search3"))
        .and(GUEST)
        .and(query_param("songCount", "20"))
        .respond_with(json_answer(page("squidify_search3.json")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/rest/download"))
        .and(GUEST)
        .respond_with(json_answer(DOWNLOADS_OFF.into()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/rest/stream"))
        .and(GUEST)
        .and(query_param("id", "5ttUyvJhrznn4CixNwkRDD"))
        .and(query_param("format", "raw"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(flac_bytes(), "audio/flac"))
        .expect(2)
        .mount(&server)
        .await;

    let src = Squidify::with_pace(&server.uri(), "", "", Duration::ZERO);
    assert_eq!(src.key(), SourceKey::Squidify);
    let want = Want {
        artists: vec!["Seatbelts".into()],
        title: Some("Tank!".into()),
        album: Some("Cowboy Bebop OST 1".into()),
        duration_ms: Some(210_000),
        ..Want::default()
    };
    let offers = src.search(&want).await.unwrap();
    assert_eq!(offers.len(), 4);
    let ranked = rank(offers, &want, &[SourceKey::Squidify]);
    assert_eq!(ranked[0].offer.id, "5ttUyvJhrznn4CixNwkRDD");
    assert!(ranked[0].verdict.sure);
    assert_eq!(ranked[0].offer.format.as_ref().unwrap().label(), "FLAC");
    assert_eq!(ranked[0].offer.size_bytes, Some(25_047_351));
    assert!(!ranked.iter().any(|r| r.offer.track_title.as_deref().is_some_and(|t| t.contains("Live") || t.contains("TV Edit"))));
    assert_eq!(pick(&ranked, None), Some(0));

    let dir = tempfile::tempdir().unwrap();
    let Fetched::Files(files) = src.fetch(&ranked[0].offer, &want, dir.path(), quiet()).await.unwrap() else { panic!() };
    assert_eq!(files[0].file_name().unwrap().to_string_lossy(), "Tank!.flac");
    assert_eq!(std::fs::read(&files[0]).unwrap(), flac_bytes());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "the refused download left nothing");
    // Known now: the next file goes to the stream at once.
    let again = tempfile::tempdir().unwrap();
    src.fetch(&ranked[0].offer, &want, again.path(), quiet()).await.unwrap();
}

#[tokio::test]
async fn an_album_comes_song_by_song_without_the_ones_already_here() {
    let server = MockServer::start().await;
    Mock::given(path("/rest/search3"))
        .and(query_param("albumCount", "10"))
        .respond_with(json_answer(
            json!({"subsonic-response": {"status": "ok", "version": "1.16.1", "searchResult3": {"album": [
                {"id": "1rAoYQZ8yPl6FJ9xYrUMdu", "name": "Samurai Champloo Music Record: Departure", "artist": "Nujabes", "songCount": 18},
                {"id": "62ANPC3iaSo9vEBGkkwgXB", "name": "Cowboy Bebop OST 1", "artist": "Various Artists", "songCount": 17, "year": 1998}
            ]}}})
            .to_string(),
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/rest/getAlbum"))
        .and(query_param("id", "62ANPC3iaSo9vEBGkkwgXB"))
        .respond_with(json_answer(page("squidify_album.json")))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(path("/rest/download"))
        .respond_with(json_answer(DOWNLOADS_OFF.into()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/rest/stream"))
        .and(query_param("format", "raw"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(flac_bytes(), "audio/flac"))
        .expect(2)
        .mount(&server)
        .await;

    let src = Squidify::with_pace(&server.uri(), "", "", Duration::ZERO);
    let want = Want {
        artists: vec!["Various Artists".into()],
        album: Some("Cowboy Bebop OST 1".into()),
        scope: WantScope::Album,
        got: vec!["Rush".into()],
        ..Want::default()
    };
    let offers = src.search(&want).await.unwrap();
    assert_eq!(offers.len(), 1, "another album is not opened");
    let ranked = rank(offers, &want, &[]);
    assert_eq!(ranked[0].offer.scope, OfferScope::Album);
    assert!(ranked[0].verdict.sure);
    assert_eq!(ranked[0].offer.format.as_ref().unwrap().label(), "FLAC");
    assert_eq!(ranked[0].offer.size_bytes, Some(25_047_351 + 25_289_944 + 23_783_129));
    let dir = tempfile::tempdir().unwrap();
    let Fetched::Files(files) = src.fetch(&ranked[0].offer, &want, dir.path(), quiet()).await.unwrap() else { panic!() };
    let names: Vec<String> = files.iter().map(|f| f.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["01 - Tank!.flac", "03 - Spokey Dokey.flac"]);
}

#[tokio::test]
async fn a_wrong_login_sits_the_source_out() {
    let server = MockServer::start().await;
    Mock::given(path("/rest/search3"))
        .and(Login { user: "me", password: "nope" })
        .respond_with(json_answer(WRONG_LOGIN.into()))
        .expect(1)
        .mount(&server)
        .await;
    let src = Squidify::with_pace(&format!("{}/rest/", server.uri()), "me", "nope", Duration::ZERO);
    let want = Want {
        artists: vec!["Seatbelts".into()],
        title: Some("Tank!".into()),
        ..Want::default()
    };
    assert!(matches!(src.search(&want).await, Err(SourceError::Blocked(_))));
    // Not asked again while it sits out.
    let again = src.search(&want).await.unwrap_err();
    assert!(again.is_refusal(), "{again:?}");
}

/// A server whose logins take no token (LDAP): asked once more with the
/// password, and so from then on.
#[tokio::test]
async fn a_server_without_tokens_gets_the_password() {
    let server = MockServer::start().await;
    Mock::given(path("/rest/search3"))
        .and(query_param("p", "enc:7077"))
        .respond_with(json_answer(page("squidify_search3.json")))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(path("/rest/search3"))
        .and(Login { user: "me", password: "pw" })
        .respond_with(json_answer(
            json!({"subsonic-response": {"status": "failed", "version": "1.16.1", "error": {"code": 41, "message": "Token authentication not supported for LDAP users"}}}).to_string(),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let src = Squidify::with_pace(&server.uri(), "me", "pw", Duration::ZERO);
    let want = Want {
        artists: vec!["Seatbelts".into()],
        title: Some("Tank!".into()),
        duration_ms: Some(210_000),
        ..Want::default()
    };
    assert_eq!(src.search(&want).await.unwrap().len(), 4);
    assert_eq!(src.search(&want).await.unwrap().len(), 4);
}
