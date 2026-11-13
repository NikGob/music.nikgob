use nikgob_core::indexer::{Indexer, IndexerError, SearchQuery, TorrentPayload};
use nikgob_core::settings::{ProwlarrSettings, Settings};
use nikgob_indexer_prowlarr::Prowlarr;
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Match, Mock, MockServer, Request, ResponseTemplate};

const KEY: &str = "test-key";

fn client(server: &MockServer) -> Prowlarr {
    Prowlarr::new(&ProwlarrSettings {
        url: server.uri(),
        api_key: KEY.into(),
    })
    .unwrap()
}

/// All values of a repeated query parameter, in order.
struct Repeated(&'static str, Vec<&'static str>);

impl Match for Repeated {
    fn matches(&self, req: &Request) -> bool {
        let got: Vec<String> = req
            .url
            .query_pairs()
            .filter(|(k, _)| k == self.0)
            .map(|(_, v)| v.into_owned())
            .collect();
        got == self.1
    }
}

fn release(server: &MockServer) -> serde_json::Value {
    json!({
        "guid": "https://tracker.example/t/42",
        "title": "Artist - Album (2019) [FLAC]",
        "indexer": "RuTracker",
        "indexerId": 7,
        "size": 523456789_i64,
        "seeders": 12,
        "leechers": 3,
        "publishDate": "2019-05-01T10:00:00Z",
        "categories": [{"id": 3000, "name": "Audio"}, {"id": 3040, "name": "Audio/Lossless"}],
        "infoUrl": "https://tracker.example/t/42",
        "infoHash": "0123456789abcdef0123456789abcdef01234567",
        "downloadUrl": format!("{}/7/download?apikey=SECRET&file=x", server.uri()),
        "magnetUrl": null,
        "protocol": "torrent"
    })
}

#[tokio::test]
async fn search_maps_results_and_sends_expected_request() {
    let server = MockServer::start().await;
    let mut usenet = release(&server);
    usenet["protocol"] = json!("usenet");
    usenet["guid"] = json!("nzb");
    Mock::given(method("GET"))
        .and(path("/api/v1/search"))
        .and(header("X-Api-Key", KEY))
        .and(query_param("query", "Artist Album"))
        .and(query_param("type", "search"))
        .and(Repeated("categories", vec!["3000", "3010", "3040", "3050", "3060"]))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([release(&server), usenet])))
        .expect(1)
        .mount(&server)
        .await;

    let found = client(&server)
        .search(&SearchQuery::new("Artist Album"))
        .await
        .unwrap();

    assert_eq!(found.len(), 1, "usenet result must be filtered out");
    let c = &found[0];
    assert_eq!(c.id, "7:https://tracker.example/t/42");
    assert_eq!(c.title, "Artist - Album (2019) [FLAC]");
    assert_eq!(c.indexer_name, "RuTracker");
    assert_eq!(c.size_bytes, Some(523456789));
    assert_eq!(c.seeders, Some(12));
    assert_eq!(c.leechers, Some(3));
    assert_eq!(c.published.as_deref(), Some("2019-05-01T10:00:00Z"));
    assert_eq!(c.categories, vec![3000, 3040]);
    assert_eq!(
        c.info_hash.as_deref(),
        Some("0123456789abcdef0123456789abcdef01234567")
    );
}

#[tokio::test]
async fn lossless_search_uses_only_lossless_category_and_limit() {
    let server = MockServer::start().await;
    Mock::given(path("/api/v1/search"))
        .and(Repeated("categories", vec!["3040"]))
        .and(query_param("limit", "20"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(1)
        .mount(&server)
        .await;
    let mut q = SearchQuery::new("x").lossless();
    q.limit = Some(20);
    assert!(client(&server).search(&q).await.unwrap().is_empty());
}

#[tokio::test]
async fn search_401_is_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(path("/api/v1/search"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let err = client(&server)
        .search(&SearchQuery::new("x"))
        .await
        .unwrap_err();
    assert!(matches!(err, IndexerError::Unauthorized), "{err:?}");
}

#[tokio::test]
async fn search_500_is_http_error() {
    let server = MockServer::start().await;
    Mock::given(path("/api/v1/search"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let err = client(&server)
        .search(&SearchQuery::new("x"))
        .await
        .unwrap_err();
    assert!(matches!(err, IndexerError::Http { status: 500 }), "{err:?}");
}

#[tokio::test]
async fn search_garbage_is_parse_error() {
    let server = MockServer::start().await;
    Mock::given(path("/api/v1/search"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>"))
        .mount(&server)
        .await;
    let err = client(&server)
        .search(&SearchQuery::new("x"))
        .await
        .unwrap_err();
    assert!(matches!(err, IndexerError::Parse(_)), "{err:?}");
}

async fn first_candidate(
    server: &MockServer,
    release: serde_json::Value,
) -> nikgob_core::indexer::Candidate {
    Mock::given(path("/api/v1/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([release])))
        .mount(server)
        .await;
    client(server)
        .search(&SearchQuery::new("x"))
        .await
        .unwrap()
        .remove(0)
}

#[tokio::test]
async fn fetch_follows_redirect_to_magnet() {
    let server = MockServer::start().await;
    let magnet = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=x";
    Mock::given(method("GET"))
        .and(path("/7/download"))
        .and(header("X-Api-Key", KEY))
        .respond_with(ResponseTemplate::new(301).insert_header("Location", magnet))
        .expect(1)
        .mount(&server)
        .await;
    let c = first_candidate(&server, release(&server)).await;
    let payload = client(&server).fetch_torrent(&c).await.unwrap();
    assert_eq!(payload, TorrentPayload::Magnet(magnet.into()));
}

#[tokio::test]
async fn fetch_returns_bencoded_bytes() {
    let server = MockServer::start().await;
    let body = b"d8:announce3:foo4:infod4:name1:xee".to_vec();
    Mock::given(path("/7/download"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
        .mount(&server)
        .await;
    let c = first_candidate(&server, release(&server)).await;
    let payload = client(&server).fetch_torrent(&c).await.unwrap();
    assert_eq!(payload, TorrentPayload::TorrentFile(body));
}

#[tokio::test]
async fn fetch_follows_http_redirect_hops() {
    let server = MockServer::start().await;
    Mock::given(path("/7/download"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "/real.torrent"))
        .mount(&server)
        .await;
    Mock::given(path("/real.torrent"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"de".to_vec()))
        .mount(&server)
        .await;
    let c = first_candidate(&server, release(&server)).await;
    let payload = client(&server).fetch_torrent(&c).await.unwrap();
    assert_eq!(payload, TorrentPayload::TorrentFile(b"de".to_vec()));
}

#[tokio::test]
async fn fetch_redirect_loop_is_bounded() {
    let server = MockServer::start().await;
    Mock::given(path("/7/download"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "/7/download"))
        .mount(&server)
        .await;
    let mut r = release(&server);
    r["infoHash"] = json!(null);
    let c = first_candidate(&server, r).await;
    let err = client(&server).fetch_torrent(&c).await.unwrap_err();
    assert!(matches!(err, IndexerError::Parse(_)), "{err:?}");
}

#[tokio::test]
async fn fetch_rejects_html_masquerading_as_torrent() {
    let server = MockServer::start().await;
    Mock::given(path("/7/download"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>login</html>"))
        .mount(&server)
        .await;
    let mut r = release(&server);
    r["infoHash"] = json!(null);
    let c = first_candidate(&server, r).await;
    let err = client(&server).fetch_torrent(&c).await.unwrap_err();
    assert!(matches!(err, IndexerError::Parse(_)), "{err:?}");
}

#[tokio::test]
async fn a_tracker_error_falls_back_to_the_magnet_of_the_hash() {
    // The live log: "fetching Eve Kaizin (2022) Mp3 320kbps: indexer answered with HTTP 500".
    let server = MockServer::start().await;
    Mock::given(path("/7/download"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let c = first_candidate(&server, release(&server)).await;
    let payload = client(&server).fetch_torrent(&c).await.unwrap();
    assert_eq!(
        payload,
        TorrentPayload::Magnet(
            "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Artist+-+Album+%282019%29+%5BFLAC%5D".into()
        )
    );
}

#[tokio::test]
async fn a_magnet_url_that_is_a_prowlarr_link_is_followed() {
    // The live log: "magnetUrl is not a magnet link" for a nyaa release.
    let server = MockServer::start().await;
    let magnet = "magnet:?xt=urn:btih:feedfacefeedfacefeedfacefeedfacefeedface";
    Mock::given(path("/9/download"))
        .and(header("X-Api-Key", KEY))
        .respond_with(ResponseTemplate::new(301).insert_header("Location", magnet))
        .expect(1)
        .mount(&server)
        .await;
    let mut r = release(&server);
    r["downloadUrl"] = json!(null);
    r["infoHash"] = json!(null);
    r["magnetUrl"] = json!(format!("{}/9/download?apikey=SECRET&link=abc", server.uri()));
    let c = first_candidate(&server, r).await;
    let payload = client(&server).fetch_torrent(&c).await.unwrap();
    assert_eq!(payload, TorrentPayload::Magnet(magnet.into()));
}

#[tokio::test]
async fn fetch_403_is_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(path("/7/download"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    let c = first_candidate(&server, release(&server)).await;
    let err = client(&server).fetch_torrent(&c).await.unwrap_err();
    assert!(matches!(err, IndexerError::Unauthorized), "{err:?}");
}

#[tokio::test]
async fn fetch_with_only_magnet_url() {
    let server = MockServer::start().await;
    let magnet = "magnet:?xt=urn:btih:abc";
    let mut r = release(&server);
    r["downloadUrl"] = json!("");
    r["magnetUrl"] = json!(magnet);
    let c = first_candidate(&server, r).await;
    let payload = client(&server).fetch_torrent(&c).await.unwrap();
    assert_eq!(payload, TorrentPayload::Magnet(magnet.into()));
}

#[tokio::test]
async fn fetch_with_nothing_is_nothing_to_fetch() {
    let server = MockServer::start().await;
    let mut r = release(&server);
    r["downloadUrl"] = json!(null);
    r["infoHash"] = json!(null);
    let c = first_candidate(&server, r).await;
    let err = client(&server).fetch_torrent(&c).await.unwrap_err();
    assert!(matches!(err, IndexerError::NothingToFetch), "{err:?}");
}

#[test]
fn missing_settings_is_not_configured() {
    let err = Prowlarr::from_settings(&Settings::default()).unwrap_err();
    assert!(matches!(err, IndexerError::NotConfigured), "{err:?}");
}

#[test]
fn invalid_url_is_rejected() {
    let err = Prowlarr::new(&ProwlarrSettings {
        url: "ftp://x".into(),
        api_key: "k".into(),
    })
    .unwrap_err();
    assert!(matches!(err, IndexerError::Parse(_)), "{err:?}");
}

#[test]
fn works_as_dyn_indexer() {
    let p = Prowlarr::new(&ProwlarrSettings {
        url: "http://localhost:9696".into(),
        api_key: "k".into(),
    })
    .unwrap();
    let dynamic: std::sync::Arc<dyn Indexer> = std::sync::Arc::new(p);
    assert_eq!(dynamic.name(), "Prowlarr");
}

#[tokio::test]
async fn check_counts_the_enabled_indexers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/indexer"))
        .and(header("X-Api-Key", KEY))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": 1, "name": "RuTracker", "enable": true},
            {"id": 2, "name": "Old", "enable": false},
            {"id": 3, "name": "Nyaa", "enable": true}
        ])))
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(client(&server).check().await.unwrap(), 2);
}

#[tokio::test]
async fn check_with_a_wrong_key_is_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/indexer"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let err = client(&server).check().await.unwrap_err();
    assert!(matches!(err, IndexerError::Unauthorized), "{err:?}");
}

/// A tracker that streams a "torrent" without end: the fetch must give up
/// at the size limit instead of buffering until the request timeout.
#[tokio::test]
async fn an_endless_torrent_body_is_cut_at_the_limit() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endless = format!("http://{}/endless.torrent", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        let Ok((mut sock, _)) = listener.accept() else { return };
        let mut buf = [0u8; 4096];
        let _ = sock.read(&mut buf);
        let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n");
        let chunk = vec![b'd'; 1 << 20];
        let head = format!("{:x}\r\n", chunk.len());
        loop {
            if sock.write_all(head.as_bytes()).is_err()
                || sock.write_all(&chunk).is_err()
                || sock.write_all(b"\r\n").is_err()
            {
                return;
            }
        }
    });
    let server = MockServer::start().await;
    let mut r = release(&server);
    r["downloadUrl"] = json!(endless);
    r["infoHash"] = json!(null);
    let c = first_candidate(&server, r).await;
    let started = std::time::Instant::now();
    let err = client(&server).fetch_torrent(&c).await.unwrap_err();
    assert!(
        matches!(&err, IndexerError::Parse(m) if m.contains("too large")),
        "{err:?}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
}

#[tokio::test]
async fn a_declared_oversized_torrent_is_refused() {
    let server = MockServer::start().await;
    let mut body = vec![b'd'; (16 << 20) + 1];
    body[1] = b'e';
    Mock::given(path("/7/download"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
        .mount(&server)
        .await;
    let mut r = release(&server);
    r["infoHash"] = json!(null);
    let c = first_candidate(&server, r).await;
    let err = client(&server).fetch_torrent(&c).await.unwrap_err();
    assert!(
        matches!(&err, IndexerError::Parse(m) if m.contains("too large")),
        "{err:?}"
    );
}
