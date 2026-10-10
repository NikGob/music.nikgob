mod common;

use std::time::Duration;

use nikgob_metadata::acoustid::AcoustId;
use nikgob_metadata::{MetadataConfig, MetadataError};
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("fixtures/", $name))
    };
}

const KEY: &str = "test-client-key";
const FINGERPRINT: &str = "AQADtEmUSImkCEXx3kfx";

fn json(status: u16, body: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_raw(body.as_bytes().to_vec(), "application/json")
}

#[tokio::test]
async fn lookup_posts_form_and_returns_scored_recordings_best_first() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/lookup"))
        .and(header("Content-Type", "application/x-www-form-urlencoded"))
        .and(body_string_contains(format!("client={KEY}")))
        .and(body_string_contains("duration=284"))
        .and(body_string_contains(format!("fingerprint={FINGERPRINT}")))
        .and(body_string_contains(
            "meta=recordings%2Breleasegroups%2Bcompress",
        ))
        .respond_with(json(200, fixture!("acoustid_lookup.json")))
        .expect(1)
        .mount(&server)
        .await;

    let client = AcoustId::with_base_url(common::http(), KEY, &server.uri()).unwrap();
    let results = client
        .lookup(FINGERPRINT, Duration::from_millis(284_400))
        .await
        .unwrap();

    assert_eq!(results.len(), 3);
    assert!((results[0].score - 0.97).abs() < 1e-9, "sorted by score");
    assert!(results[0].score > results[1].score && results[1].score > results[2].score);
    let rec = &results[0].recordings[0];
    assert_eq!(rec.id, "b1a9c0e9-d987-4042-ae91-78d6a3267d69");
    assert_eq!(rec.title.as_deref(), Some("Группа крови"));
    assert_eq!(rec.duration(), Some(Duration::from_secs(284)));
    assert_eq!(rec.artist_names(), "Кино");
    assert_eq!(rec.release_groups[0].kind.as_deref(), Some("Album"));
    assert!(
        results[2].recordings.is_empty(),
        "result without recordings is allowed"
    );
}

#[tokio::test]
async fn key_and_fingerprint_never_appear_in_the_url() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(json(200, r#"{"status":"ok","results":[]}"#))
        .mount(&server)
        .await;
    let client = AcoustId::with_base_url(common::http(), KEY, &server.uri()).unwrap();
    let results = client
        .lookup(FINGERPRINT, Duration::from_secs(10))
        .await
        .unwrap();
    assert!(results.is_empty());
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.query(), None);
}

#[tokio::test]
async fn api_errors_become_service_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(json(400, fixture!("acoustid_error.json")))
        .mount(&server)
        .await;
    let client = AcoustId::with_base_url(common::http(), KEY, &server.uri()).unwrap();
    let err = client
        .lookup(FINGERPRINT, Duration::from_secs(10))
        .await
        .unwrap_err();
    assert!(matches!(err, MetadataError::Service(m) if m == "invalid API key"));

    // Same error object with HTTP 200.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(json(200, fixture!("acoustid_error.json")))
        .mount(&server)
        .await;
    let client = AcoustId::with_base_url(common::http(), KEY, &server.uri()).unwrap();
    let err = client
        .lookup(FINGERPRINT, Duration::from_secs(10))
        .await
        .unwrap_err();
    assert!(matches!(err, MetadataError::Service(_)));
}

#[tokio::test]
async fn empty_fingerprint_is_rejected_without_a_request() {
    let server = MockServer::start().await;
    let client = AcoustId::with_base_url(common::http(), KEY, &server.uri()).unwrap();
    assert!(matches!(
        client.lookup("  ", Duration::from_secs(10)).await,
        Err(MetadataError::InvalidArgument(_))
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[test]
fn key_comes_from_config_and_is_never_printed() {
    let without = MetadataConfig::new("me@example.com");
    assert!(matches!(
        AcoustId::new(common::http(), &without),
        Err(MetadataError::NotConfigured(_))
    ));
    let blank = MetadataConfig::new("me@example.com").with_acoustid_key("   ");
    assert!(AcoustId::new(common::http(), &blank).is_err());

    let with = MetadataConfig::new("me@example.com").with_acoustid_key(KEY);
    let client = AcoustId::new(common::http(), &with).unwrap();
    assert!(!format!("{with:?}").contains(KEY));
    assert!(format!("{with:?}").contains("redacted"));
    // The client itself prints no key either.
    assert!(!format!("{client:?}").contains(KEY));
}
