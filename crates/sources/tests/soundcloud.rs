//! SoundCloud's originals: the source over a fake of the service (formats,
//! matching, signed in or out), and over the real client against a mocked
//! api-v2 that answers as it did on 2026-11-15 (the search's flags; the
//! download link only for a signed-in user). No real network.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use nikgob_core::indexer::BoxFuture;
use nikgob_core::source::{Fetched, Progress, Source, SourceKey, Want, WantScope};
use nikgob_core::streaming::{Account, LoginInput, StreamError, StreamService};
use nikgob_soundcloud::{Downloadable, Soundcloud, SoundcloudConfig};
use nikgob_sources::select::{pick, rank};
use nikgob_sources::soundcloud::{Originals, SoundcloudOriginals};
use wiremock::matchers::{header, header_exists, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn quiet() -> Progress {
    Arc::new(|_, _| {})
}

/// A WAV of 24-bit stereo at 48 kHz.
fn wav_bytes() -> Vec<u8> {
    let mut b = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
    b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(2u16.to_le_bytes());
    b.extend(48_000u32.to_le_bytes());
    b.extend((48_000u32 * 6).to_le_bytes());
    b.extend(6u16.to_le_bytes());
    b.extend(24u16.to_le_bytes());
    b.extend(b"data");
    b.extend(6000u32.to_le_bytes());
    b.extend(std::iter::repeat_n(0u8, 6000));
    b
}

/// MPEG-1 Layer III frames at 320 kbps (an MP3 uploaded as «….wav»).
fn mp3_bytes() -> Vec<u8> {
    let len = 144 * 320 * 1000 / 44_100;
    let mut b = Vec::new();
    for _ in 0..6 {
        let mut f = vec![0xFF, 0xFB, 0xE0, 0x00];
        f.resize(len, 0x11);
        b.extend(f);
    }
    b
}

fn part(bytes: &[u8], whole: u64, kind: &str) -> ResponseTemplate {
    ResponseTemplate::new(206)
        .insert_header("Content-Range", format!("bytes 0-{}/{whole}", bytes.len() - 1).as_str())
        .set_body_raw(bytes.to_vec(), kind)
}

/// The service as the source sees it.
struct Fake {
    signed_in: bool,
    hits: Vec<Downloadable>,
    /// Track id → its original's link.
    links: Vec<(u64, String)>,
    finds: AtomicUsize,
    asked: Mutex<Vec<u64>>,
}

impl Originals for Fake {
    fn signed_in(&self) -> bool {
        self.signed_in
    }

    fn find<'a>(&'a self, _query: &'a str) -> BoxFuture<'a, Result<Vec<Downloadable>, StreamError>> {
        self.finds.fetch_add(1, Ordering::SeqCst);
        let hits = self.hits.clone();
        Box::pin(async move { Ok(hits) })
    }

    fn link(&self, id: u64) -> BoxFuture<'_, Result<String, StreamError>> {
        self.asked.lock().unwrap().push(id);
        let link = self.links.iter().find(|(t, _)| *t == id).map(|(_, l)| l.clone());
        Box::pin(async move {
            if !self.signed_in {
                return Err(StreamError::NotLoggedIn);
            }
            link.ok_or(StreamError::Http { status: 403 })
        })
    }
}

fn critizice() -> Want {
    Want {
        artists: vec!["Ell Kingston".into()],
        title: Some("Critizice Edit".into()),
        duration_ms: Some(378_000),
        ..Want::default()
    }
}

/// The search's tracks: the wished one as a WAV, the same song re-uploaded
/// by a fan as an MP3 named «.wav», and another song.
fn hits() -> Vec<Downloadable> {
    vec![
        Downloadable {
            id: 2411553153,
            title: "Ell Kingston, Enzo Draper - Critizice Edit [Free Download].wav".into(),
            artists: vec!["Ell Kingston".into(), "Enzo Draper".into()],
            uploader: "Ell Kingston".into(),
            duration_ms: Some(378_181),
            url: Some("https://soundcloud.com/user-443177929/ell-kingston-enzo-draper-1".into()),
            ..Downloadable::default()
        },
        Downloadable {
            id: 77,
            title: "Ell Kingston - Critizice Edit (FREE DOWNLOAD.wav)".into(),
            artists: vec!["house reposts".into()],
            uploader: "house reposts".into(),
            duration_ms: Some(378_100),
            ..Downloadable::default()
        },
        Downloadable { id: 78, title: "LA PAZ - NIXON (FREE DOWNLOAD.wav)".into(), artists: vec!["NIXON".into()], uploader: "NIXON".into(), ..Downloadable::default() },
    ]
}

fn fake(signed_in: bool, server: &MockServer) -> Arc<Fake> {
    Arc::new(Fake {
        signed_in,
        hits: hits(),
        links: vec![(2411553153, format!("{}/o/wav", server.uri())), (77, format!("{}/o/mp3", server.uri()))],
        finds: AtomicUsize::new(0),
        asked: Mutex::new(Vec::new()),
    })
}

#[tokio::test]
async fn signed_out_nothing_is_offered_or_asked() {
    let server = MockServer::start().await;
    let sc = fake(false, &server);
    let src = SoundcloudOriginals::new(sc.clone());
    assert_eq!((src.key(), src.name()), (SourceKey::Soundcloud, "SoundCloud"));
    assert!(src.search(&critizice()).await.unwrap().is_empty());
    assert_eq!(sc.finds.load(Ordering::SeqCst), 0);
    assert!(server.received_requests().await.unwrap().is_empty());
}

/// Signed in: the matching tracks' files are looked at, a WAV is offered
/// as lossless and picked over the MP3 that only calls itself «.wav»; the
/// other song is never asked for; an album is not looked for.
#[tokio::test]
async fn signed_in_the_original_is_offered_for_what_its_bytes_are() {
    let server = MockServer::start().await;
    let (wav, mp3) = (wav_bytes(), mp3_bytes());
    Mock::given(path("/o/wav")).and(header_exists("range")).respond_with(part(&wav, 72_600_044, "audio/wav")).with_priority(1).mount(&server).await;
    Mock::given(path("/o/wav")).respond_with(ResponseTemplate::new(200).set_body_raw(wav.clone(), "audio/wav")).mount(&server).await;
    Mock::given(path("/o/mp3")).and(header_exists("range")).respond_with(part(&mp3, 15_124_000, "audio/x-wav")).with_priority(1).mount(&server).await;
    let sc = fake(true, &server);
    let src = SoundcloudOriginals::new(sc.clone());
    let want = critizice();
    let offers = src.search(&want).await.unwrap();
    assert_eq!(offers.len(), 2, "{offers:?}");
    assert!(!sc.asked.lock().unwrap().contains(&78), "the other song is not looked at");
    let wav_offer = offers.iter().find(|o| o.id == "2411553153").unwrap();
    assert_eq!(wav_offer.format.as_ref().unwrap().label(), "WAV 24/48");
    assert!(wav_offer.lossless());
    assert_eq!(wav_offer.size_bytes, Some(72_600_044));
    assert_eq!(wav_offer.track_title.as_deref(), Some("Critizice Edit"));
    let mp3_offer = offers.iter().find(|o| o.id == "77").unwrap();
    assert_eq!(mp3_offer.format.as_ref().unwrap().label(), "MP3 320");
    assert!(!mp3_offer.lossless(), "an MP3 called .wav is an MP3");
    let ranked = rank(offers.clone(), &want, &[SourceKey::Soundcloud]);
    let at = pick(&ranked, None).unwrap();
    assert_eq!(ranked[at].offer.id, "2411553153");

    let dir = tempfile::tempdir().unwrap();
    let Fetched::Files(files) = src.fetch(&ranked[at].offer, &want, dir.path(), quiet()).await.unwrap() else { panic!() };
    assert_eq!(files[0].file_name().unwrap().to_string_lossy(), "Critizice Edit.wav");
    assert_eq!(std::fs::read(&files[0]).unwrap(), wav);
    let asked_before = sc.asked.lock().unwrap().len();
    assert!(asked_before >= 3, "the link is asked again right before the download");

    let album = Want { scope: WantScope::Album, album: Some("Critizice".into()), ..want };
    let finds = sc.finds.load(Ordering::SeqCst);
    assert!(src.search(&album).await.unwrap().is_empty());
    assert_eq!(sc.finds.load(Ordering::SeqCst), finds);
}

/// The head looked like a WAV, the file that came is an MP3: it is named
/// for what came.
#[tokio::test]
async fn a_file_is_named_by_its_bytes() {
    let server = MockServer::start().await;
    let (wav, mp3) = (wav_bytes(), mp3_bytes());
    Mock::given(path("/o/mp3")).and(header_exists("range")).respond_with(part(&wav, 15_124_000, "audio/x-wav")).with_priority(1).mount(&server).await;
    Mock::given(path("/o/mp3")).respond_with(ResponseTemplate::new(200).set_body_raw(mp3.clone(), "audio/x-wav")).mount(&server).await;
    let sc = Arc::new(Fake {
        signed_in: true,
        hits: vec![hits()[1].clone()],
        links: vec![(77, format!("{}/o/mp3", server.uri()))],
        finds: AtomicUsize::new(0),
        asked: Mutex::new(Vec::new()),
    });
    let src = SoundcloudOriginals::new(sc);
    let want = critizice();
    let offers = src.search(&want).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(offers[0].format.as_ref().unwrap().label(), "WAV 24/48");
    let Fetched::Files(files) = src.fetch(&offers[0], &want, dir.path(), quiet()).await.unwrap() else { panic!() };
    assert_eq!(files[0].file_name().unwrap().to_string_lossy(), "Critizice Edit.mp3");
    assert_eq!(std::fs::read(&files[0]).unwrap(), mp3);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "no .wav left behind");
}

// -- the real client against a mocked api-v2 ------------------------------

const CLIENT_ID: &str = "qLcgHhXgeEFFaSolTB9I71LBsqncsD93";

/// The search's answer: the flags of a free WAV and of a track sold
/// elsewhere, as api-v2 gave them.
const SEARCH: &str = r#"{"collection":[
  {"id":2411553153,"kind":"track","title":"Ell Kingston, Enzo Draper - Critizice Edit [Free Download].wav","duration":378228,"full_duration":378181,
   "downloadable":true,"has_downloads_left":true,"streamable":true,"policy":"ALLOW",
   "permalink_url":"https://soundcloud.com/user-443177929/ell-kingston-enzo-draper-1",
   "user":{"id":451657941,"username":"Ell Kingston","permalink_url":"https://soundcloud.com/user-443177929"},
   "publisher_metadata":{"artist":"Ell Kingston, Enzo Draper"}},
  {"id":2330746367,"kind":"track","title":"PENYANGKALAN - AMSTERBOY & DEKA WAV ( V.I.P EDIT ) Buy = Free Download","duration":222912,"full_duration":222866,
   "downloadable":false,"has_downloads_left":false,"streamable":true,"policy":"ALLOW",
   "user":{"id":1044969097,"username":"AmsterBoy Official"},"publisher_metadata":{"artist":"AmsterBoy Official"}}
],"next_href":null}"#;

async fn service(server: &MockServer, dir: &Path) -> Arc<Soundcloud> {
    std::fs::write(dir.join("client_id"), CLIENT_ID).unwrap();
    let mut cfg = SoundcloudConfig::new(dir.to_path_buf());
    cfg.api_base = server.uri();
    cfg.site = server.uri();
    cfg.memory_store = true;
    Soundcloud::new(cfg).unwrap()
}

#[tokio::test]
async fn the_real_client_gives_originals_only_when_signed_in() {
    let server = MockServer::start().await;
    Mock::given(path("/search/tracks")).respond_with(ResponseTemplate::new(200).set_body_raw(SEARCH, "application/json")).mount(&server).await;
    Mock::given(path("/me"))
        .and(header("authorization", "OAuth tok-123"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(r#"{"id":42,"username":"listener"}"#, "application/json"))
        .mount(&server)
        .await;
    // Without the sign-in api-v2 answers 401 with an empty body.
    Mock::given(path("/tracks/2411553153/download"))
        .and(header("authorization", "OAuth tok-123"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(format!(r#"{{"redirectUri":"{}/o/wav"}}"#, server.uri()), "application/json"),
        )
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(path("/tracks/2411553153/download")).respond_with(ResponseTemplate::new(401)).mount(&server).await;
    let wav = wav_bytes();
    Mock::given(path("/o/wav")).and(header_exists("range")).respond_with(part(&wav, 72_600_044, "audio/wav")).with_priority(1).mount(&server).await;
    Mock::given(path("/o/wav")).respond_with(ResponseTemplate::new(200).set_body_raw(wav.clone(), "audio/wav")).mount(&server).await;

    let dir = tempfile::tempdir().unwrap();
    let sc = service(&server, dir.path()).await;
    // Only the downloadable track of the search.
    let found = sc.downloadable("Ell Kingston Critizice Edit").await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].artists, ["Ell Kingston", "Enzo Draper"]);
    assert_eq!((found[0].uploader.as_str(), found[0].duration_ms), ("Ell Kingston", Some(378_181)));
    // Signed out: no link is asked for, nothing is offered.
    assert_eq!(sc.original_url(2411553153).await, Err(StreamError::NotLoggedIn));
    let src = SoundcloudOriginals::new(sc.clone());
    let want = critizice();
    assert!(src.search(&want).await.unwrap().is_empty());
    assert!(!server.received_requests().await.unwrap().iter().any(|r| r.url.path().ends_with("/download")));

    sc.login(LoginInput::Cookies("OAuth tok-123".into())).await.unwrap();
    assert!(sc.signed_in());
    let offers = src.search(&want).await.unwrap();
    assert_eq!(offers.len(), 1, "{offers:?}");
    assert_eq!(offers[0].format.as_ref().unwrap().label(), "WAV 24/48");
    let dir2 = tempfile::tempdir().unwrap();
    let Fetched::Files(files) = src.fetch(&offers[0], &want, dir2.path(), quiet()).await.unwrap() else { panic!() };
    assert_eq!(std::fs::read(&files[0]).unwrap(), wav);
}

/// A refused download link is the track's: the sign-in stays.
#[tokio::test]
async fn a_refused_link_does_not_sign_out() {
    let server = MockServer::start().await;
    Mock::given(path("/search/tracks")).respond_with(ResponseTemplate::new(200).set_body_raw(SEARCH, "application/json")).mount(&server).await;
    Mock::given(path("/me")).respond_with(ResponseTemplate::new(200).set_body_raw(r#"{"id":42,"username":"listener"}"#, "application/json")).mount(&server).await;
    Mock::given(path("/tracks/2411553153/download")).respond_with(ResponseTemplate::new(401)).mount(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let sc = service(&server, dir.path()).await;
    sc.login(LoginInput::Cookies("OAuth tok-123".into())).await.unwrap();
    let src = SoundcloudOriginals::new(sc.clone());
    assert!(src.search(&critizice()).await.unwrap().is_empty());
    assert!(sc.signed_in());
    assert!(matches!(sc.account(), Account::LoggedIn { .. }), "{:?}", sc.account());
}

/// The originals' search goes without the sign-in: a search that refuses
/// the token (401) is never taken for the sign-in failing.
#[tokio::test]
async fn the_search_for_originals_never_signs_out() {
    let server = MockServer::start().await;
    Mock::given(path("/search/tracks"))
        .and(header_exists("authorization"))
        .respond_with(ResponseTemplate::new(401))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(path("/search/tracks")).respond_with(ResponseTemplate::new(200).set_body_raw(SEARCH, "application/json")).mount(&server).await;
    Mock::given(path("/me")).respond_with(ResponseTemplate::new(200).set_body_raw(r#"{"id":42,"username":"listener"}"#, "application/json")).mount(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let sc = service(&server, dir.path()).await;
    sc.login(LoginInput::Cookies("OAuth tok-123".into())).await.unwrap();
    let found = sc.downloadable("Ell Kingston Critizice Edit").await.unwrap();
    assert_eq!(found.len(), 1);
    assert!(sc.signed_in(), "the sign-in stays");
    assert!(matches!(sc.account(), Account::LoggedIn { .. }), "{:?}", sc.account());
    let searches: Vec<_> = server.received_requests().await.unwrap().into_iter().filter(|r| r.url.path() == "/search/tracks").collect();
    assert!(!searches.is_empty());
    assert!(searches.iter().all(|r| !r.headers.contains_key("authorization")), "no token sent with the search");
}
