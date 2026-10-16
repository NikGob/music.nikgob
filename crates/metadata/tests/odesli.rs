mod common;

use nikgob_metadata::odesli::{Odesli, parse_links};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("fixtures/", $name))
    };
}

const TRACK: &str = "https://open.spotify.com/track/0DiWol3AO6WpXZgp0goxAV";

#[test]
fn parses_the_page_url_and_the_entity() {
    let page = parse_links(fixture!("odesli_song.json").as_bytes())
        .unwrap()
        .expect("a page");
    assert_eq!(page.page_url, "https://song.link/s/0DiWol3AO6WpXZgp0goxAV");
    assert_eq!(page.title.as_deref(), Some("One More Time"));
    assert_eq!(page.artist.as_deref(), Some("Daft Punk"));

    let album = parse_links(fixture!("odesli_album.json").as_bytes())
        .unwrap()
        .unwrap();
    assert!(album.page_url.starts_with("https://album.link/"));
    assert_eq!(album.title.as_deref(), Some("Discovery"));

    assert_eq!(parse_links(br#"{"userCountry":"US"}"#).unwrap(), None);
    assert!(parse_links(b"not json").is_err());
}

#[tokio::test]
async fn asks_once_per_url_and_an_unknown_link_is_none() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/links"))
        .and(query_param("url", TRACK))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture!("odesli_song.json").as_bytes().to_vec(), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/links"))
        .and(query_param("url", "https://open.spotify.com/track/nope"))
        .respond_with(ResponseTemplate::new(400).set_body_raw(
            br#"{"statusCode":400,"code":"could_not_resolve_entity"}"#.to_vec(),
            "application/json",
        ))
        .expect(1)
        .mount(&server)
        .await;

    let odesli = Odesli::with_base_url(common::http(), &server.uri()).unwrap();
    for _ in 0..3 {
        let page = odesli.page(TRACK).await.unwrap().unwrap();
        assert_eq!(page.page_url, "https://song.link/s/0DiWol3AO6WpXZgp0goxAV");
    }
    for _ in 0..2 {
        assert_eq!(
            odesli.page("https://open.spotify.com/track/nope").await.unwrap(),
            None
        );
    }
}
