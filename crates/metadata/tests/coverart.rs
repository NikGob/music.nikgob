mod common;

use nikgob_metadata::MetadataError;
use nikgob_metadata::coverart::{CoverArtArchive, CoverImage, CoverTarget};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("fixtures/", $name))
    };
}

const RELEASE: &str = "2f2a1b6c-6d8e-4f53-9a7f-5d5a0f3b9c01";
const RG: &str = "7c5d4a3b-1e2f-4a6b-8c9d-0e1f2a3b4c5d";
const JPEG: &[u8] = b"\xff\xd8\xff\xe0 pretend this is a jpeg";

fn client(server: &MockServer) -> CoverArtArchive {
    CoverArtArchive::with_base_url(common::http(), &server.uri()).unwrap()
}

fn listing(server: &MockServer) -> serde_json::Value {
    let u = |name: &str| format!("{}/img/{name}", server.uri());
    json!({
        "images": [
            {
                "types": ["Front"], "front": true, "back": false, "approved": false,
                "image": u("unapproved.jpg"), "thumbnails": {"250": u("unapproved-250.jpg")}
            },
            {
                "types": ["Front", "Booklet"], "front": true, "back": false, "approved": true,
                "image": u("orig.jpg"),
                "thumbnails": {"250": u("250.jpg"), "500": u("500.jpg"), "1200": u("1200.jpg")}
            }
        ]
    })
}

#[tokio::test]
async fn front_cover_picks_the_front_image_and_largest_fitting_thumbnail() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/release/{RELEASE}")))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            fixture!("caa_release.json").as_bytes().to_vec(),
            "application/json",
        ))
        .expect(1)
        .mount(&server)
        .await;

    let image = client(&server)
        .front_cover(&CoverTarget::Release(RELEASE.into()))
        .await
        .unwrap()
        .expect("front image");
    assert!(image.front);
    assert!(image.image.ends_with("/1001.jpg"), "back cover is skipped");
    assert!(image.url_for(1200).ends_with("1001-1200.jpg"));
    assert!(image.url_for(2000).ends_with("1001-1200.jpg"));
    assert!(image.url_for(1199).ends_with("1001-500.jpg"));
    assert!(image.url_for(500).ends_with("1001-500.jpg"));
    assert!(image.url_for(300).ends_with("1001-250.jpg"));
    assert!(
        image.url_for(100).ends_with("1001-250.jpg"),
        "nothing fits: smallest thumbnail, not the huge original"
    );
}

#[test]
fn legacy_thumbnail_names_and_missing_thumbnails() {
    let mut image = CoverImage {
        image: "https://x/orig.jpg".into(),
        ..CoverImage::default()
    };
    assert_eq!(image.url_for(1200), "https://x/orig.jpg");
    image
        .thumbnails
        .insert("small".into(), "https://x/s.jpg".into());
    image
        .thumbnails
        .insert("large".into(), "https://x/l.jpg".into());
    assert_eq!(image.url_for(1200), "https://x/l.jpg");
    assert_eq!(image.url_for(250), "https://x/s.jpg");
}

#[tokio::test]
async fn fetch_front_downloads_the_1200_rendition_and_prefers_approved() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/release/{RELEASE}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(listing(&server)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/img/1200.jpg"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(JPEG.to_vec(), "image/jpeg"))
        .expect(1)
        .mount(&server)
        .await;

    let cover = client(&server)
        .fetch_front(&CoverTarget::Release(RELEASE.into()), 1200)
        .await
        .unwrap()
        .expect("cover");
    assert!(cover.url.ends_with("/img/1200.jpg"));
    assert_eq!(cover.bytes, JPEG);
}

#[tokio::test]
async fn missing_art_is_none_and_release_group_is_the_fallback() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/release/{RELEASE}")))
        .respond_with(ResponseTemplate::new(404).set_body_string("No cover art found"))
        .expect(2) // one direct call, one from the fallback helper
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/release-group/{RG}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(listing(&server)))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/img/500.jpg"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(JPEG.to_vec(), "image/jpeg"))
        .mount(&server)
        .await;

    let cl = client(&server);
    assert!(
        cl.front_cover(&CoverTarget::Release(RELEASE.into()))
            .await
            .unwrap()
            .is_none()
    );
    let cover = cl
        .fetch_front_with_fallback(RELEASE, Some(RG), 500)
        .await
        .unwrap()
        .expect("cover from release group");
    assert!(cover.url.ends_with("/img/500.jpg"));
}

#[tokio::test]
async fn invalid_ids_and_download_errors() {
    let server = MockServer::start().await;
    let cl = client(&server);
    assert!(matches!(
        cl.front_cover(&CoverTarget::Release("../x".into())).await,
        Err(MetadataError::InvalidArgument(_))
    ));
    Mock::given(method("GET"))
        .and(path("/img/gone.jpg"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    assert!(matches!(
        cl.download(&format!("{}/img/gone.jpg", server.uri())).await,
        Err(MetadataError::NotFound)
    ));
}
