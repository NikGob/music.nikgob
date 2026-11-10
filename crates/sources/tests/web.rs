//! The web sources against mocked sites: khinsider's saved pages, a
//! Qobuz-DL instance and hifi-api instances answering as they do. No real
//! network.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use nikgob_core::source::{Fetched, OfferScope, Progress, Source, SourceError, SourceKey, Want, WantScope};
use nikgob_sources::khinsider::Khinsider;
use nikgob_sources::qobuz::Qobuz;
use nikgob_sources::select::{pick, rank};
use nikgob_sources::tidal::Tidal;
use serde_json::json;
use wiremock::matchers::{path, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn page(name: &str, base: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pages").join(name);
    std::fs::read_to_string(p).unwrap().replace("{{BASE}}", base)
}

fn html(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body, "text/html; charset=UTF-8")
}

fn flac_bytes() -> Vec<u8> {
    let mut b = b"fLaC".to_vec();
    b.extend(std::iter::repeat_n(7u8, 4096));
    b
}

fn quiet() -> Progress {
    Arc::new(|_, _| {})
}

fn supraland() -> Want {
    Want {
        artists: vec!["Con Fetti Pesto".into(), "David Münnich".into()],
        title: Some("Das Nennt Ihr Himmel (Supraland Six Inches Under Credits Song)".into()),
        album: Some("Das Nennt Ihr Himmel (Supraland Six Inches Under Credits Song)".into()),
        duration_ms: Some(188_000),
        ..Want::default()
    }
}

#[tokio::test]
async fn khinsider_finds_the_song_of_a_game_and_downloads_its_flac() {
    let server = MockServer::start().await;
    let base = server.uri();
    Mock::given(path("/search"))
        .and(query_param("search", "Supraland Six Inches Under"))
        .respond_with(html(page("khinsider_search.html", &base)))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/game-soundtracks/album/supraland-six-inches-under-original-soundtrack-2022"))
        .respond_with(html(page("khinsider_album.html", &base)))
        .mount(&server)
        .await;
    Mock::given(path_regex(r"^/game-soundtracks/album/supraland-six-inches-under-original-soundtrack-2022/03\..*\.mp3$"))
        .respond_with(html(page("khinsider_song.html", &base)))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path_regex(r"^/soundtracks/.*/03\..*\.flac$"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(flac_bytes(), "audio/flac"))
        .expect(1)
        .mount(&server)
        .await;

    let kh = Khinsider::with_pace(&base, Duration::ZERO);
    let want = supraland();
    let offers = kh.search(&want).await.unwrap();
    assert_eq!(offers.len(), 1, "only the song asked for: {offers:?}");
    let o = &offers[0];
    assert_eq!(o.track_title.as_deref(), Some("Das Nennt Ihr Himmel (Credits Song)"));
    assert_eq!(o.duration_ms, Some(188_000));
    assert!(o.lossless());
    let ranked = rank(offers, &want, &[SourceKey::Torrents, SourceKey::Khinsider]);
    assert_eq!(pick(&ranked, None), Some(0), "a FLAC of the game named: sure");

    let dir = tempfile::tempdir().unwrap();
    let Fetched::Files(files) = kh.fetch(&ranked[0].offer, &want, dir.path(), quiet()).await.unwrap() else {
        panic!("files expected")
    };
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].file_name().unwrap().to_string_lossy(), "03. Das Nennt Ihr Himmel.flac");
    assert_eq!(std::fs::read(&files[0]).unwrap(), flac_bytes());
}

#[tokio::test]
async fn khinsider_is_not_asked_for_what_is_not_a_soundtrack() {
    let server = MockServer::start().await;
    Mock::given(path("/search")).respond_with(ResponseTemplate::new(500)).expect(0).mount(&server).await;
    let kh = Khinsider::with_pace(&server.uri(), Duration::ZERO);
    let want = Want {
        artists: vec!["SLAVA MARLOW".into()],
        title: Some("Каблуки".into()),
        album: Some("ТУЗИК".into()),
        ..Want::default()
    };
    assert!(kh.search(&want).await.unwrap().is_empty());
}

#[tokio::test]
async fn khinsider_behind_its_bot_check_sits_out() {
    let server = MockServer::start().await;
    Mock::given(path("/search"))
        .respond_with(ResponseTemplate::new(403).set_body_raw("<title>Just a moment...</title>", "text/html"))
        .expect(1)
        .mount(&server)
        .await;
    let kh = Khinsider::with_pace(&server.uri(), Duration::ZERO);
    let err = kh.search(&supraland()).await.unwrap_err();
    assert!(matches!(err, SourceError::Blocked(_)), "{err:?}");
    // Asked again at once: no request, and still told as the bot check
    // (the site never asked us to wait).
    let again = kh.search(&supraland()).await.unwrap_err();
    assert!(nikgob_sources::http::is_check(&again), "{again:?}");
}

fn qobuz_track(id: u64, artist: &str, title: &str, secs: u32, bits: u8, khz: f64) -> serde_json::Value {
    json!({
        "id": id, "title": title, "version": null, "duration": secs, "isrc": "JPU902300001",
        "track_number": 1, "media_number": 1, "maximum_bit_depth": bits, "maximum_sampling_rate": khz,
        "performer": {"name": artist, "id": 1},
        "album": {"id": "a1", "title": title, "artist": {"name": artist}},
        "streamable": true
    })
}

#[tokio::test]
async fn qobuz_gives_the_track_in_hi_res_through_the_next_instance() {
    let down = MockServer::start().await;
    Mock::given(path("/api/get-music")).respond_with(ResponseTemplate::new(502)).mount(&down).await;
    let server = MockServer::start().await;
    Mock::given(path("/api/get-music"))
        .and(query_param("q", "Eve Underdog"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "data": {
                "query": "Eve Underdog",
                "albums": {"items": []},
                "tracks": {"items": [
                    qobuz_track(11, "Eve", "Underdog", 194, 24, 48.0),
                    qobuz_track(12, "Kobasolo", "Underdog", 194, 16, 44.1),
                    qobuz_track(13, "Eve", "Underdog (Instrumental)", 194, 24, 48.0)
                ]}
            }
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/api/download-music"))
        .and(query_param("track_id", "11"))
        .and(query_param("quality", "27"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"success": true, "data": {"url": format!("{}/file/11.flac", server.uri())}})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/file/11.flac"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(flac_bytes(), "audio/flac"))
        .mount(&server)
        .await;

    let q = Qobuz::with_pace(&[down.uri(), server.uri()], Duration::ZERO);
    let want = Want {
        artists: vec!["Eve".into()],
        title: Some("Underdog".into()),
        album: Some("Underdog".into()),
        duration_ms: Some(194_000),
        ..Want::default()
    };
    let offers = q.search(&want).await.unwrap();
    assert_eq!(offers.len(), 3);
    let ranked = rank(offers, &want, &[SourceKey::Qobuz]);
    assert_eq!(ranked.len(), 1, "a cover and an instrumental are out");
    assert_eq!(ranked[0].offer.format.as_ref().unwrap().label(), "FLAC 24/48");
    assert_eq!(pick(&ranked, None), Some(0));
    let dir = tempfile::tempdir().unwrap();
    let Fetched::Files(files) = q.fetch(&ranked[0].offer, &want, dir.path(), quiet()).await.unwrap() else { panic!() };
    assert_eq!(files[0].file_name().unwrap().to_string_lossy(), "Underdog.flac");
}

#[tokio::test]
async fn qobuz_albums_come_track_by_track() {
    let server = MockServer::start().await;
    Mock::given(path("/api/get-music"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "data": {"albums": {"items": [{"id": "alb9", "title": "nyan.pop", "artist": {"name": "nyan.mp3"}, "maximum_bit_depth": 16, "maximum_sampling_rate": 44.1, "streamable": true}]},
                     "tracks": {"items": []}}
        })))
        .mount(&server)
        .await;
    Mock::given(path("/api/get-album"))
        .and(query_param("album_id", "alb9"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "data": {"id": "alb9", "title": "nyan.pop", "tracks": {"items": [
                qobuz_track(1, "nyan.mp3", "Я не интроверт", 100, 16, 44.1),
                qobuz_track(2, "nyan.mp3", "Второй", 120, 16, 44.1)
            ]}}
        })))
        .mount(&server)
        .await;
    Mock::given(path("/api/download-music"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"success": true, "data": {"url": format!("{}/f.flac", server.uri())}})))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(path("/f.flac"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(flac_bytes(), "audio/flac"))
        .mount(&server)
        .await;
    let q = Qobuz::with_pace(&[server.uri()], Duration::ZERO);
    let want = Want {
        artists: vec!["nyan.mp3".into()],
        album: Some("nyan.pop".into()),
        scope: WantScope::Album,
        ..Want::default()
    };
    let ranked = rank(q.search(&want).await.unwrap(), &want, &[]);
    assert_eq!(ranked[0].offer.scope, OfferScope::Album);
    assert!(ranked[0].verdict.sure);
    let dir = tempfile::tempdir().unwrap();
    let Fetched::Files(files) = q.fetch(&ranked[0].offer, &want, dir.path(), quiet()).await.unwrap() else { panic!() };
    let names: Vec<String> = files.iter().map(|f| f.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["01 - Я не интроверт.flac", "01 - Второй.flac"]);
}

#[tokio::test]
async fn an_instance_that_says_no_is_not_a_crash() {
    let server = MockServer::start().await;
    Mock::given(path("/api/get-music"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"success": false, "error": "Qobuz token expired"})))
        .mount(&server)
        .await;
    let q = Qobuz::with_pace(&[server.uri()], Duration::ZERO);
    let want = Want { artists: vec!["Eve".into()], title: Some("Underdog".into()), ..Want::default() };
    // The instance's dead token sits that instance out for hours: an error,
    // never a crash.
    let e = q.search(&want).await.unwrap_err();
    assert!(matches!(e, SourceError::RateLimited { .. } | SourceError::Http { status: 400 }), "{e:?}");
}

fn bts(url: &str) -> String {
    let manifest = json!({"mimeType": "audio/flac", "codecs": "flac", "encryptionType": "NONE", "urls": [url]});
    base64::engine::general_purpose::STANDARD.encode(manifest.to_string())
}

#[tokio::test]
async fn tidal_through_hifi_api_reads_the_bts_manifest() {
    let server = MockServer::start().await;
    Mock::given(path("/search/"))
        .and(query_param("s", "Yorushika 火星人 Martian"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "version": "2.4",
            "data": {"limit": 25, "offset": 0, "totalNumberOfItems": 1, "items": [{
                "id": 4242, "title": "Martian", "duration": 233, "isrc": "JPU902300777",
                "audioQuality": "LOSSLESS", "allowStreaming": true, "streamReady": true,
                "trackNumber": 3, "volumeNumber": 1,
                "artist": {"id": 1, "name": "Yorushika"}, "artists": [{"id": 1, "name": "Yorushika"}],
                "album": {"id": 77, "title": "second person"}
            }]}
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/track/"))
        .and(query_param("id", "4242"))
        .and(query_param("quality", "LOSSLESS"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "version": "2.4",
            "data": {"trackId": 4242, "audioQuality": "LOSSLESS", "manifestMimeType": "application/vnd.tidal.bts",
                     "manifest": bts(&format!("{}/cdn/4242.flac", server.uri())), "bitDepth": 16, "sampleRate": 44100}
        })))
        .mount(&server)
        .await;
    Mock::given(path("/cdn/4242.flac"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(flac_bytes(), "audio/flac"))
        .mount(&server)
        .await;
    let t = Tidal::with_pace(&[server.uri()], Duration::ZERO);
    let want = Want {
        artists: vec!["Yorushika".into()],
        title: Some("火星人 - Martian".into()),
        album: Some("second person".into()),
        duration_ms: Some(233_000),
        ..Want::default()
    };
    let ranked = rank(t.search(&want).await.unwrap(), &want, &[]);
    assert_eq!(ranked.len(), 1);
    assert!(ranked[0].verdict.sure, "artist, a form of the title, the length");
    let dir = tempfile::tempdir().unwrap();
    let Fetched::Files(files) = t.fetch(&ranked[0].offer, &want, dir.path(), quiet()).await.unwrap() else { panic!() };
    assert_eq!(std::fs::read(&files[0]).unwrap(), flac_bytes());
    assert_eq!(files[0].extension().unwrap(), "flac");
}

#[test]
fn tidal_refuses_dash_and_drm_manifests() {
    let dash = json!({"data": {"manifestMimeType": "application/dash+xml", "manifest": "PE1QRD4="}});
    assert!(nikgob_sources::tidal::file_url(&dash).is_err());
    let drm = json!([{"manifest": base64::engine::general_purpose::STANDARD.encode(
        json!({"encryptionType": "OLD_AES", "urls": ["x"]}).to_string()), "manifestMimeType": "application/vnd.tidal.bts"}]);
    assert!(nikgob_sources::tidal::file_url(&drm).is_err());
    let old = json!([{"id": 1}, {"manifest": "x"}, {"OriginalTrackUrl": "https://cdn/x.flac"}]);
    assert_eq!(nikgob_sources::tidal::file_url(&old).unwrap(), "https://cdn/x.flac");
}

#[test]
fn khinsider_search_page_as_saved_from_the_site() {
    let html = page("khinsider_search_saved.html", "");
    let albums = nikgob_sources::khinsider::search_results(&html);
    let names: Vec<&str> = albums.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["Supraland", "Supraland", "Supraland Six Inches Under"]);
    assert!(albums[2].href.starts_with("/game-soundtracks/album/supraland-six-inches-under-"));
}

#[test]
fn khinsider_album_and_song_pages_as_saved_from_the_site() {
    use nikgob_core::model::Codec;
    let album = nikgob_sources::khinsider::album_page(&page("khinsider_album_saved.html", ""));
    assert_eq!(album.name, "Supraland Six Inches Under");
    assert_eq!(album.songs.len(), 3);
    let first = &album.songs[0];
    assert_eq!((first.name.as_str(), first.number, first.disc, first.duration_ms), ("Area1-Action", 1, 1, Some(64_000)));
    assert_eq!(first.flac_bytes, None, "this album has MP3 and OGG only");
    assert_eq!(first.lossy.as_ref().map(|(c, _)| c.clone()), Some(Codec::Mp3));
    assert!(first.href.ends_with("/01.%2520Area1-Action.mp3"));
    let files = nikgob_sources::khinsider::song_files(&page("khinsider_song_saved.html", ""));
    assert_eq!(files.len(), 2);
    assert!(files[0].0.ends_with("01.%20Area1-Action.mp3") && !files[0].1, "MP3 before OGG");
    assert!(files[1].0.ends_with(".ogg"));
}
