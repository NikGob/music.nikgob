//! Dogmazic against a mocked play.dogmazic.net: its public search page as
//! it answered on 2026-11-15 (`pages/dogmazic_search.html`, trimmed), the
//! download that answers a range with the head of the file, and the
//! Subsonic API for a user with a login. No real network.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use nikgob_core::model::Codec;
use nikgob_core::source::{Fetched, Format, Offer, OfferScope, Progress, Source, SourceKey, Want, WantScope};
use nikgob_sources::dogmazic::Dogmazic;
use nikgob_sources::select::rank;
use nikgob_sources::subsonic::token;
use wiremock::matchers::{header_exists, method, path, query_param};
use wiremock::{Match, Mock, MockServer, Request, ResponseTemplate};

fn page(name: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pages").join(name)).unwrap()
}

fn quiet() -> Progress {
    Arc::new(|_, _| {})
}

/// An MP3 as the site keeps them: an ID3 tag, then MPEG-1 Layer III
/// frames at 320 kbps.
fn mp3_bytes() -> Vec<u8> {
    let mut b = b"ID3\x03\0\0\0\0\x01\x00".to_vec();
    b.resize(10 + 128, 0);
    let len = 144 * 320 * 1000 / 44_100;
    for _ in 0..8 {
        let mut f = vec![0xFF, 0xFB, 0xE0, 0x00];
        f.resize(len, 0x55);
        b.extend(f);
    }
    b
}

/// A WAV of 16-bit stereo at 44.1 kHz.
fn wav_bytes() -> Vec<u8> {
    let mut b = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
    b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(2u16.to_le_bytes());
    b.extend(44_100u32.to_le_bytes());
    b.extend((44_100u32 * 4).to_le_bytes());
    b.extend(4u16.to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(b"data");
    b.extend(4096u32.to_le_bytes());
    b.extend(std::iter::repeat_n(0u8, 4096));
    b
}

/// The answer to a range: the bytes asked for and the whole size.
fn part(bytes: &[u8], whole: u64, kind: &str) -> ResponseTemplate {
    ResponseTemplate::new(206)
        .insert_header("Content-Range", format!("bytes 0-{}/{whole}", bytes.len() - 1).as_str())
        .set_body_raw(bytes.to_vec(), kind)
}

fn kali() -> Want {
    Want {
        artists: vec!["Kali-Frogz".into()],
        title: Some("Time Rated Morning".into()),
        album: Some("Flatbone-EP".into()),
        duration_ms: Some(213_000),
        ..Want::default()
    }
}

async fn search_page(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/search.php"))
        .and(query_param("rule_1", "title"))
        .and(query_param("rule_1_input", "Time Rated Morning"))
        .and(query_param("rule_2", "artist"))
        .and(query_param("rule_2_input", "Kali-Frogz"))
        .and(query_param("operator", "and"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(page("dogmazic_search.html"), "text/html; charset=UTF-8"))
        .expect(1)
        .mount(server)
        .await;
}

/// Without a login: the public pages. Only the song that is the wish is
/// looked at, its first bytes make it an MP3 320, and it comes whole.
#[tokio::test]
async fn a_song_of_the_public_pages_is_offered_for_what_its_file_is() {
    let server = MockServer::start().await;
    search_page(&server).await;
    let mp3 = mp3_bytes();
    Mock::given(path("/stream.php"))
        .and(query_param("action", "download"))
        .and(query_param("song_id", "1430"))
        .and(header_exists("range"))
        .respond_with(part(&mp3, 8_531_968, "audio/mpeg"))
        .with_priority(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/stream.php"))
        .and(query_param("song_id", "1430"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(mp3.clone(), "audio/mpeg"))
        .expect(1)
        .mount(&server)
        .await;
    // The other songs of the page are not looked at.
    Mock::given(path("/stream.php")).and(query_param("song_id", "1429")).respond_with(ResponseTemplate::new(500)).expect(0).mount(&server).await;
    Mock::given(path("/stream.php")).and(query_param("song_id", "1431")).respond_with(ResponseTemplate::new(500)).expect(0).mount(&server).await;

    let src = Dogmazic::with_pace(&server.uri(), "", "", Duration::ZERO);
    assert_eq!((src.key(), src.name()), (SourceKey::Dogmazic, "Dogmazic"));
    let want = kali();
    let offers = src.search(&want).await.unwrap();
    assert_eq!(offers.len(), 1, "{offers:?}");
    let o = &offers[0];
    assert_eq!((o.id.as_str(), o.artist.as_deref(), o.track_title.as_deref()), ("1430", Some("Kali-Frogz"), Some("Time Rated Morning")));
    assert_eq!(o.album.as_deref(), Some("Flatbone-EP"));
    assert_eq!(o.duration_ms, Some(213_000));
    assert_eq!(o.format.as_ref().unwrap().label(), "MP3 320");
    assert!(!o.lossless(), "an MP3 never counts as lossless");
    assert_eq!(o.size_bytes, Some(8_531_968));
    let ranked = rank(offers.clone(), &want, &[SourceKey::Dogmazic]);
    assert!(ranked[0].verdict.sure);
    let flac = Offer { format: Some(Format::new(Codec::Flac)), ..o.clone() };
    assert!(flac.quality() > o.quality(), "a FLAC from anywhere beats it");

    let dir = tempfile::tempdir().unwrap();
    let Fetched::Files(files) = src.fetch(o, &want, dir.path(), quiet()).await.unwrap() else { panic!() };
    assert_eq!(files[0].file_name().unwrap().to_string_lossy(), "Time Rated Morning.mp3");
    assert_eq!(std::fs::read(&files[0]).unwrap(), mp3);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

/// A WAV upload is lossless; a song whose file is a page is not offered.
#[tokio::test]
async fn a_wav_is_lossless_and_a_page_is_nothing() {
    let server = MockServer::start().await;
    search_page(&server).await;
    let wav = wav_bytes();
    Mock::given(path("/stream.php"))
        .and(query_param("song_id", "1430"))
        .and(header_exists("range"))
        .respond_with(part(&wav, wav.len() as u64, "audio/x-wav"))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(path("/stream.php"))
        .and(query_param("song_id", "1430"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(wav.clone(), "audio/x-wav"))
        .mount(&server)
        .await;
    let src = Dogmazic::with_pace(&server.uri(), "", "", Duration::ZERO);
    let want = kali();
    let offers = src.search(&want).await.unwrap();
    assert_eq!(offers[0].format.as_ref().unwrap().label(), "WAV");
    assert!(offers[0].lossless());
    let dir = tempfile::tempdir().unwrap();
    let Fetched::Files(files) = src.fetch(&offers[0], &want, dir.path(), quiet()).await.unwrap() else { panic!() };
    assert_eq!(files[0].extension().unwrap(), "wav");

    let gone = MockServer::start().await;
    search_page(&gone).await;
    Mock::given(path("/stream.php"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("<html>Morceau introuvable</html>", "text/html"))
        .mount(&gone)
        .await;
    let src = Dogmazic::with_pace(&gone.uri(), "", "", Duration::ZERO);
    assert!(src.search(&want).await.unwrap().is_empty());
}

/// Without a login an album is not asked for at all (the pages give
/// songs); with one, the Subsonic API answers with each song's codec.
#[tokio::test]
async fn an_album_needs_the_login_and_the_login_is_the_api() {
    let server = MockServer::start().await;
    let src = Dogmazic::with_pace(&server.uri(), "", "", Duration::ZERO);
    let album = Want { artists: vec!["Kali-Frogz".into()], album: Some("Flatbone-EP".into()), scope: WantScope::Album, ..Want::default() };
    assert!(src.search(&album).await.unwrap().is_empty());
    assert!(server.received_requests().await.unwrap().is_empty(), "nothing asked");

    struct Login;
    impl Match for Login {
        fn matches(&self, req: &Request) -> bool {
            let q: std::collections::HashMap<String, String> = req.url.query_pairs().into_owned().collect();
            let get = |k: &str| q.get(k).map(String::as_str).unwrap_or_default();
            get("u") == "me" && get("t") == token("pw", get("s"))
        }
    }
    let search3 = r#"{"subsonic-response":{"status":"ok","version":"1.16.1","type":"ampache","serverVersion":"8.4.0","openSubsonic":true,
        "searchResult3":{"song":[{"id":"1430","title":"Time Rated Morning","album":"Flatbone-EP","artist":":: Kali-Frogz ::","track":1,
        "size":8531968,"contentType":"audio/mpeg","suffix":"mp3","duration":213,"bitRate":320,"isVideo":false}]}}}"#;
    Mock::given(path("/rest/search3"))
        .and(Login)
        .respond_with(ResponseTemplate::new(200).set_body_raw(search3, "application/json"))
        .mount(&server)
        .await;
    let user = Dogmazic::with_pace(&server.uri(), "me", "pw", Duration::ZERO);
    let offers = user.search(&kali()).await.unwrap();
    let o = offers.iter().find(|o| o.id == "1430").expect("the song");
    assert_eq!((o.source, o.scope), (SourceKey::Dogmazic, OfferScope::Track));
    assert_eq!(o.format.as_ref().unwrap().label(), "MP3 320");
    assert!(!o.lossless());
    assert!(server.received_requests().await.unwrap().iter().all(|r| r.url.path() == "/rest/search3"), "no page asked");
}
