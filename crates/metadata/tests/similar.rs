//! The clients of the graph of similar music against a mock server:
//! ListenBrainz's similar artists, recordings and popularity, Last.fm's
//! similar tracks and artists with the user's key, MusicBrainz's artist
//! search with its tags.

mod common;

use nikgob_metadata::musicbrainz::MusicBrainz;
use nikgob_metadata::{LastFm, ListenBrainz, MetadataError};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const RADIOHEAD: &str = "a74b1b7f-71a5-4011-9441-d0b5e4122711";
const KARMA: &str = "9e2ad5bc-c6f9-40d2-a36f-3122ee2072a3";

fn json(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.as_bytes().to_vec(), "application/json")
}

#[tokio::test]
async fn listenbrainz_lists_similar_artists_recordings_and_tops() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/labs/similar-artists/json"))
        .and(query_param("artist_mbids", RADIOHEAD))
        .respond_with(json(&format!(
            r#"[{{"artist_mbid":"5b11f4ce-a62d-471e-81fc-a69a8278c7da","name":"Nirvana","score":11156,"reference_mbid":"{RADIOHEAD}"}},
                {{"artist_mbid":"9c9f1380-2516-4fc9-a3e6-f9f61941d090","name":"Muse","score":10286}}]"#
        )))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/labs/similar-recordings/json"))
        .and(query_param("recording_mbids", KARMA))
        .respond_with(json(
            r#"[{"recording_mbid":"ef4c6855-949e-4e22-b41e-8e0a2d372d5f","recording_name":"No Surprises","artist_credit_name":"Radiohead","score":155}]"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/popularity/top-recordings-for-artist/{RADIOHEAD}")))
        .respond_with(json(&format!(
            r#"[{{"artist_mbids":["{RADIOHEAD}"],"artist_name":"Radiohead","recording_mbid":"{KARMA}","recording_name":"Karma Police","total_listen_count":900}}]"#
        )))
        .mount(&server)
        .await;
    // An id it does not know: 400 from the labs.
    Mock::given(method("GET"))
        .and(path("/labs/similar-artists/json"))
        .and(query_param("artist_mbids", "00000000-0000-0000-0000-000000000000"))
        .respond_with(ResponseTemplate::new(400).set_body_string("unknown"))
        .mount(&server)
        .await;
    let lb = ListenBrainz::with_base_urls(common::http(), &format!("{}/labs/", server.uri()), &format!("{}/api/", server.uri())).unwrap();
    let artists = lb.similar_artists(RADIOHEAD).await.unwrap();
    assert_eq!(artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["Nirvana", "Muse"]);
    let recs = lb.similar_recordings(KARMA).await.unwrap();
    assert_eq!((recs[0].name.as_str(), recs[0].artist.as_str()), ("No Surprises", "Radiohead"));
    let top = lb.top_recordings(RADIOHEAD).await.unwrap();
    assert_eq!((top[0].mbid.as_str(), top[0].name.as_str(), top[0].artist.as_str()), (KARMA, "Karma Police", "Radiohead"));
    assert!(lb.similar_artists("00000000-0000-0000-0000-000000000000").await.unwrap().is_empty());
    assert!(matches!(lb.similar_artists("not an id").await, Err(MetadataError::InvalidArgument(_))));
}

#[tokio::test]
async fn lastfm_needs_a_key_and_reads_its_lists() {
    let server = MockServer::start().await;
    assert!(LastFm::with_base_url(common::http(), &server.uri(), "  ").unwrap().is_none());
    Mock::given(method("GET"))
        .and(query_param("method", "track.getsimilar"))
        .and(query_param("api_key", "k123"))
        .and(query_param("artist", "Radiohead"))
        .and(query_param("track", "Karma Police"))
        .respond_with(json(
            r#"{"similartracks":{"track":[{"name":"No Surprises","mbid":"","match":"0.91","artist":{"name":"Radiohead","mbid":""}},
                {"name":"Paranoid Android","match":0.5,"artist":{"name":"Radiohead"}}]}}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(query_param("method", "artist.getsimilar"))
        .and(query_param("artist", "Radiohead"))
        .respond_with(json(r#"{"similarartists":{"artist":[{"name":"Thom Yorke","mbid":"8ed2e0b3-aa4c-4e13-bec3-dc7393ed4d6b","match":"1"}]}}"#))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(query_param("artist", "Nobody"))
        .respond_with(json(r#"{"error":6,"message":"Track not found"}"#))
        .mount(&server)
        .await;
    let fm = LastFm::with_base_url(common::http(), &server.uri(), "k123").unwrap().unwrap();
    assert!(!format!("{fm:?}").contains("k123"));
    let t = fm.similar_tracks("Radiohead", "Karma Police").await.unwrap();
    assert_eq!(t.len(), 2);
    assert_eq!((t[0].name.as_str(), t[0].artist.as_str(), t[0].mbid.clone()), ("No Surprises", "Radiohead", None));
    assert!((t[0].score - 0.91).abs() < 1e-6);
    let a = fm.similar_artists("Radiohead").await.unwrap();
    assert_eq!(a[0].name, "Thom Yorke");
    assert!(fm.similar_tracks("Nobody", "x").await.unwrap().is_empty());

    // A bad key is an error, not an empty list.
    let bad = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(403).set_body_string(r#"{"error":10,"message":"Invalid API key"}"#))
        .mount(&bad)
        .await;
    let fm = LastFm::with_base_url(common::http(), &bad.uri(), "wrong").unwrap().unwrap();
    let err = fm.similar_artists("Radiohead").await.unwrap_err();
    assert!(matches!(&err, MetadataError::Service(m) if m.contains("10")), "{err:?}");
    assert!(!err.to_string().contains("wrong"));
}

#[tokio::test]
async fn musicbrainz_finds_an_artist_with_its_tags() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ws/2/artist"))
        .and(query_param("query", "artist:\"Radiohead\""))
        .respond_with(json(&format!(
            r#"{{"artists":[{{"id":"{RADIOHEAD}","name":"Radiohead","score":100,
                "tags":[{{"count":19,"name":"rock"}},{{"count":43,"name":"Alternative Rock"}},{{"count":0,"name":"grunge"}}]}},
               {{"id":"11111111-1111-1111-1111-111111111111","name":"Radiohead Tribute","score":80}}]}}"#
        )))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/ws/2/artist/{RADIOHEAD}")))
        .and(query_param("inc", "tags+genres"))
        .respond_with(json(&format!(
            r#"{{"id":"{RADIOHEAD}","name":"Radiohead","tags":[{{"count":5,"name":"rock"}}],"genres":[{{"count":30,"name":"art rock","id":"x"}},{{"count":7,"name":"rock"}}]}}"#
        )))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/ws/2/artist"))
        .and(query_param("query", "artist:\"Nobody Known\""))
        .respond_with(json(r#"{"artists":[{"id":"22222222-2222-2222-2222-222222222222","name":"Somebody","score":100}]}"#))
        .mount(&server)
        .await;
    let mb = MusicBrainz::with_base_url(common::http(), &format!("{}/ws/2/", server.uri())).unwrap();
    let a = mb.search_artist_tags("Radiohead").await.unwrap().unwrap();
    assert_eq!(a.id, RADIOHEAD);
    assert_eq!(a.tags, vec![("alternative rock".to_owned(), 43), ("rock".to_owned(), 19), ("grunge".to_owned(), 0)]);
    assert!(mb.search_artist_tags("Nobody Known").await.unwrap().is_none());
    let t = mb.artist_tags(RADIOHEAD).await.unwrap();
    assert_eq!(t.tags, vec![("art rock".to_owned(), 30), ("rock".to_owned(), 7)]);
}
