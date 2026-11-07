mod common;

use std::time::Duration;

use nikgob_metadata::musicbrainz::{MusicBrainz, RecordingQuery, artist_credit_string};
use nikgob_metadata::{Http, MetadataConfig, MetadataError, RetryPolicy};
use wiremock::matchers::{header_regex, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("fixtures/", $name))
    };
}

const REC: &str = "b1a9c0e9-d987-4042-ae91-78d6a3267d69";
const RELEASE: &str = "2f2a1b6c-6d8e-4f53-9a7f-5d5a0f3b9c01";
const RG: &str = "7c5d4a3b-1e2f-4a6b-8c9d-0e1f2a3b4c5d";
const ARTIST: &str = "8e2a3a4c-8f0b-4a3e-bd0a-0e7a4d1c2f11";

fn json(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.as_bytes().to_vec(), "application/json")
}

fn client(server: &MockServer) -> MusicBrainz {
    MusicBrainz::with_base_url(common::http(), &format!("{}/ws/2/", server.uri())).unwrap()
}

#[tokio::test]
async fn lookup_recording_parses_credits_releases_and_sends_expected_request() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/ws/2/recording/{REC}")))
        .and(query_param("fmt", "json"))
        .and(query_param(
            "inc",
            "artists+releases+release-groups+isrcs+media",
        ))
        .and(header_regex(
            "User-Agent",
            r"^music\.nikgob/\d+\.\d+\.\d+ \( tests@example\.invalid \)$",
        ))
        .respond_with(json(fixture!("mb_recording_lookup.json")))
        .expect(1)
        .mount(&server)
        .await;

    let rec = client(&server).lookup_recording(REC).await.unwrap();
    assert_eq!(rec.id, REC);
    assert_eq!(rec.title, "Группа крови");
    assert_eq!(rec.duration(), Some(Duration::from_millis(284_000)));
    assert_eq!(rec.isrcs, ["RUA011100001"]);
    assert_eq!(rec.first_release_date.as_deref(), Some("1988-06-01"));
    assert_eq!(rec.artist_credit_string(), "Кино");
    assert_eq!(rec.artist_credit[0].artist.id, ARTIST);
    assert_eq!(rec.releases.len(), 2);

    let album = &rec.releases[0];
    assert_eq!(album.id, RELEASE);
    assert_eq!(album.status.as_deref(), Some("Official"));
    assert_eq!(album.country.as_deref(), Some("SU"));
    assert_eq!(album.barcode, None, "empty barcode becomes None");
    let rg = album.release_group.as_ref().unwrap();
    assert_eq!(rg.id, RG);
    assert_eq!(rg.primary_type.as_deref(), Some("Album"));
    assert!(rg.secondary_types.is_empty());
    assert_eq!(album.media[0].tracks[0].position, 9);

    let comp = &rec.releases[1];
    assert_eq!(comp.date, None, "empty date becomes None");
    assert_eq!(
        comp.release_group.as_ref().unwrap().secondary_types,
        ["Compilation"]
    );
}

#[tokio::test]
async fn search_recordings_builds_lucene_query_and_reads_search_shape() {
    let server = MockServer::start().await;
    let query = RecordingQuery::new("Группа крови")
        .artist("Кино")
        .release("Группа крови")
        .duration(Duration::from_secs(284));
    let expected = r#"recording:"Группа крови" AND artist:"Кино" AND release:"Группа крови" AND dur:[274000 TO 294000]"#;
    assert_eq!(query.to_lucene(), expected);

    Mock::given(method("GET"))
        .and(path("/ws/2/recording"))
        .and(query_param("fmt", "json"))
        .and(query_param("query", expected))
        .and(query_param("limit", "10"))
        .respond_with(json(fixture!("mb_recording_search.json")))
        .expect(1)
        .mount(&server)
        .await;

    let found = client(&server).search_recordings(&query).await.unwrap();
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].score, Some(100));
    // Search results name the media track list `track`, not `tracks`.
    let track = &found[0].releases[0].media[0].tracks[0];
    assert_eq!(track.number, "9");
    assert_eq!(track.length, Some(284_000));
    assert_eq!(found[1].disambiguation, "live, 1990-06-24: Luzhniki");
    assert!(found[1].releases.is_empty());
}

#[test]
fn lucene_phrases_are_escaped() {
    let q = RecordingQuery::new(r#"Say "Hi" \o/"#).artist("  ").limit(5);
    assert_eq!(q.to_lucene(), r#"recording:"Say \"Hi\" \\o/""#);
}

#[tokio::test]
async fn lookup_release_reads_tracks_labels_and_recordings() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/ws/2/release/{RELEASE}")))
        .and(query_param(
            "inc",
            "recordings+artist-credits+labels+media+release-groups",
        ))
        .respond_with(json(fixture!("mb_release_lookup.json")))
        .mount(&server)
        .await;

    let rel = client(&server).lookup_release(RELEASE).await.unwrap();
    assert_eq!(rel.title, "Группа крови");
    assert_eq!(rel.label_name(), Some("Мелодия"));
    assert_eq!(
        rel.label_info[0].catalog_number.as_deref(),
        Some("A60 00415")
    );
    assert_eq!(rel.media.len(), 1);
    let disc = &rel.media[0];
    assert_eq!(disc.position, Some(1));
    assert_eq!(disc.format.as_deref(), Some("Vinyl"));
    assert_eq!(disc.tracks.len(), 2);
    assert_eq!(disc.tracks[1].number, "A2");
    assert_eq!(disc.tracks[1].recording.as_ref().unwrap().id, REC);
    assert_eq!(artist_credit_string(&rel.artist_credit), "Кино");
    assert!(rel.is_official());
}

#[tokio::test]
async fn browse_release_group_releases_returns_a_page() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ws/2/release"))
        .and(query_param("release-group", RG))
        .and(query_param("limit", "100"))
        .and(query_param("offset", "0"))
        .respond_with(json(fixture!("mb_release_group_releases.json")))
        .expect(1)
        .mount(&server)
        .await;

    let page = client(&server)
        .browse_release_group_releases(RG, 500, 0)
        .await
        .unwrap();
    assert_eq!(page.total, 3);
    assert_eq!(page.items.len(), 3);
    assert_eq!(page.items[1].date.as_deref(), Some("2000"));
    assert_eq!(page.items[2].status.as_deref(), Some("Bootleg"));
    assert!(!page.items[2].is_official());
    assert!(page.items[2].artist_credit.is_empty());
}

#[tokio::test]
async fn artist_lookup_and_discography() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/ws/2/artist/{ARTIST}")))
        .respond_with(json(fixture!("mb_artist.json")))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/ws/2/release-group"))
        .and(query_param("artist", ARTIST))
        .respond_with(json(fixture!("mb_artist_release_groups.json")))
        .expect(1)
        .mount(&server)
        .await;

    let mb = client(&server);
    let artist = mb.lookup_artist(ARTIST).await.unwrap();
    assert_eq!(artist.name, "Кино");
    assert_eq!(artist.sort_name, "Kino");
    assert_eq!(artist.kind.as_deref(), Some("Group"));
    assert_eq!(artist.country.as_deref(), Some("SU"));

    let groups = mb.artist_discography(ARTIST).await.unwrap();
    assert_eq!(groups.len(), 3, "total fits in one page, so one request");
    assert_eq!(groups[0].first_release_date.as_deref(), Some("1988-06-01"));
    assert_eq!(groups[1].secondary_types, ["Compilation"]);
    assert_eq!(groups[1].first_release_date, None);
    assert_eq!(groups[2].primary_type.as_deref(), Some("Single"));
    assert!(groups[2].secondary_types.is_empty(), "null becomes empty");
}

#[tokio::test]
async fn not_found_and_bad_ids() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404).set_body_string(r#"{"error":"Not Found"}"#))
        .mount(&server)
        .await;
    let mb = client(&server);
    assert!(matches!(
        mb.lookup_recording(REC).await,
        Err(MetadataError::NotFound)
    ));
    // MBIDs end up in URL paths; anything else is refused before a request is made.
    assert!(matches!(
        mb.lookup_recording("../../etc").await,
        Err(MetadataError::InvalidArgument(_))
    ));
}

#[tokio::test]
async fn retries_on_503_then_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503).insert_header("Retry-After", "0"))
        .up_to_n_times(2)
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(json(fixture!("mb_artist.json")))
        .expect(1)
        .mount(&server)
        .await;
    let artist = client(&server).lookup_artist(ARTIST).await.unwrap();
    assert_eq!(artist.name, "Кино");
}

#[tokio::test]
async fn a_503_that_says_retry_at_once_waits_a_second() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503).insert_header("Retry-After", "0"))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(json(fixture!("mb_artist.json")))
        .expect(1)
        .mount(&server)
        .await;
    let http = common::http().with_retry_policy(RetryPolicy {
        max_attempts: 3,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_secs(30),
    });
    let mb = MusicBrainz::with_base_url(http, &format!("{}/ws/2/", server.uri())).unwrap();
    let t = std::time::Instant::now();
    mb.lookup_artist(ARTIST).await.unwrap();
    assert!(t.elapsed() >= Duration::from_secs(1), "{:?}", t.elapsed());
}

#[tokio::test]
async fn gives_up_with_rate_limited_after_max_attempts() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "1"))
        .expect(3)
        .mount(&server)
        .await;
    let err = client(&server).lookup_artist(ARTIST).await.unwrap_err();
    // A server-provided Retry-After is capped by the policy max_delay (5 ms in tests).
    assert!(matches!(err, MetadataError::RateLimited(Some(d)) if d <= Duration::from_millis(5)));
}

#[tokio::test]
async fn server_errors_are_reported_with_status() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;
    let err = client(&server).lookup_artist(ARTIST).await.unwrap_err();
    assert!(matches!(err, MetadataError::Status { status: 500, .. }));
}

#[test]
fn musicbrainz_requires_a_contact() {
    let http = Http::new(&MetadataConfig::default()).unwrap();
    assert!(matches!(
        MusicBrainz::new(http),
        Err(MetadataError::NotConfigured(_))
    ));
    let http = Http::new(&MetadataConfig::new("me@example.com")).unwrap();
    assert!(MusicBrainz::new(http).is_ok());
}

#[tokio::test]
async fn search_releases_asks_for_artist_and_title_and_reads_track_counts() {
    let server = MockServer::start().await;
    let body = r#"{"releases":[{"id":"2f2a1b6c-6d8e-4f53-9a7f-5d5a0f3b9c01","score":100,"title":"Ugly Is Beautiful",
        "status":"Official","date":"2020-07-17","artist-credit":[{"name":"Oliver Tree","artist":{"id":"8e2a3a4c-8f0b-4a3e-bd0a-0e7a4d1c2f11","name":"Oliver Tree"}}],
        "release-group":{"id":"7c5d4a3b-1e2f-4a6b-8c9d-0e1f2a3b4c5d","primary-type":"Album"},
        "media":[{"format":"Digital Media","track-count":14}]}]}"#;
    Mock::given(method("GET"))
        .and(path("/ws/2/release"))
        .and(query_param("query", r#"release:"Ugly Is Beautiful" AND artist:"Oliver Tree""#))
        .and(query_param("limit", "5"))
        .respond_with(json(body))
        .expect(1)
        .mount(&server)
        .await;
    let found = client(&server).search_releases("Oliver Tree", "Ugly Is Beautiful", 5).await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].artist_credit_string(), "Oliver Tree");
    assert_eq!(found[0].media[0].track_count, 14);
    assert!(found[0].media[0].tracks.is_empty());
}
