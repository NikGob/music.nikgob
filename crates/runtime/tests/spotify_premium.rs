//! Spotify needs Premium: a Spotify that is
//! not connected is asked nothing at all, while the library's Spotify
//! tracks still play from YouTube Music; a login that finds no Premium ends
//! signed out with the message, and the view says Spotify is not usable.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_core::model::TrackId;
use nikgob_core::streaming::{
    Account, FakeService, LoginInput, RemoteId, SPOTIFY_PREMIUM_REQUIRED, Service, StreamService,
};
use nikgob_library::Library;
use nikgob_runtime::{PlayRequest, PlaySource, Playback, Runtime, RuntimeConfig};
use nikgob_view::{AccountView, Command, Prefs, StreamPage, ViewState};

/// Remembers what the engine was asked to play.
struct Recorder(Arc<Mutex<Vec<PlayRequest>>>);

impl Playback for Recorder {
    fn play(&mut self, item: PlayRequest) {
        self.0.lock().unwrap().push(item);
    }
    fn set_next(&mut self, _: Option<PlayRequest>) {}
    fn pause(&mut self) {}
    fn resume(&mut self) {}
    fn stop(&mut self) {}
    fn seek(&mut self, _: Duration) {}
    fn set_volume(&mut self, _: f32) {}
    fn position(&self) -> Duration {
        Duration::ZERO
    }
}

fn wait_for(what: &str, f: impl Fn() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn start(lib: &Library, streams: Vec<Arc<dyn StreamService>>) -> (Runtime, Arc<Mutex<Vec<PlayRequest>>>) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
            streams,
            library_dir: std::env::temp_dir(),
            prefs: Prefs::default(),
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(Recorder(asked.clone())),
        Arc::new(|| {}),
    );
    (rt, asked)
}

fn account(s: &ViewState, service: Service) -> AccountView {
    s.accounts.iter().find(|a| a.service == service).cloned().expect("an account view")
}

#[test]
fn a_spotify_that_is_not_connected_is_asked_nothing() {
    let lib = Library::open_in_memory().unwrap();
    let sp = Arc::new(FakeService::spotify().logged_out());
    let yt = Arc::new(FakeService::youtube().logged_out());
    // A Spotify track already in the library (imported back when it was
    // connected), and YouTube knows the same song.
    let one = sp.tracks().into_iter().find(|t| t.id.id == "sp-gruppa-1").unwrap();
    let track: TrackId = lib.upsert_stream_track(&one).unwrap();
    let (rt, asked) = start(&lib, vec![sp.clone(), yt.clone()]);
    wait_for("the account views", || rt.state().accounts.len() == Service::ALL.len());
    let s = rt.state();
    assert!(!account(&s, Service::Spotify).usable, "not connected: not usable");
    assert!(account(&s, Service::Youtube).usable, "YouTube Music works without a login");
    drop(s);

    rt.send(Command::Search("daft".into()));
    wait_for("the YouTube section", || {
        let s = rt.state();
        s.search.query == "daft" && s.search.streams.iter().any(|x| x.service == Service::Youtube && !x.searching)
    });
    assert!(
        !rt.state().search.streams.iter().any(|x| x.service == Service::Spotify),
        "no Spotify section is searched"
    );
    rt.send(Command::LoadStream(StreamPage::Album(RemoteId::new(Service::Spotify, "sp-alb-discovery"))));
    wait_for("the page says why", || {
        rt.state().stream_page.as_ref().is_some_and(|p| p.error.as_deref() == Some("Spotify не подключён"))
    });
    rt.send(Command::ImportFromService(Service::Spotify));
    // The library's Spotify track plays from YouTube Music.
    rt.send(Command::PlayTracks { tracks: vec![track], start: 0, context: "test".into() });
    wait_for("a play request", || !asked.lock().unwrap().is_empty());
    assert!(matches!(
        &asked.lock().unwrap()[0].source,
        PlaySource::Stream { service: Service::Youtube, .. }
    ));
    // A few ticks of likes checks and sync would have asked by now.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(sp.total_calls(), 0, "Spotify was asked: {:?}", sp.all_calls());
}

#[test]
fn a_login_without_premium_ends_signed_out_with_the_message() {
    let lib = Library::open_in_memory().unwrap();
    let sp = Arc::new(FakeService::spotify().without_premium());
    let (rt, _) = start(&lib, vec![sp.clone()]);
    rt.send(Command::StreamLogin {
        service: Service::Spotify,
        input: LoginInput::PersonalApp { client_id: "test-client-id".into() },
    });
    wait_for("the Premium message", || {
        rt.state().accounts.iter().any(|a| {
            a.service == Service::Spotify && !a.busy && a.error.as_deref() == Some(SPOTIFY_PREMIUM_REQUIRED)
        })
    });
    let a = account(&rt.state(), Service::Spotify);
    assert!(!a.account.is_logged_in() && !a.usable && !a.audio);
    assert_eq!(a.account, Account::Failed(SPOTIFY_PREMIUM_REQUIRED.into()));
    assert!(rt.state().toasts.is_empty(), "no «подключён» toast");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(sp.calls("liked_head") + sp.calls("liked"), 0, "no likes pulled");
}

#[test]
fn a_login_switches_the_service_on() {
    // A fresh install ships Spotify off; connecting it is turning it on.
    let lib = Library::open_in_memory().unwrap();
    let sp = Arc::new(FakeService::spotify().logged_out());
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
            streams: vec![sp.clone() as Arc<dyn StreamService>],
            library_dir: std::env::temp_dir(),
            prefs: Prefs {
                spotify_enabled: false,
                ..Prefs::default()
            },
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(Recorder(Arc::new(Mutex::new(Vec::new())))),
        Arc::new(|| {}),
    );
    wait_for("the account views", || rt.state().accounts.len() == Service::ALL.len());
    assert!(!account(&rt.state(), Service::Spotify).enabled);
    rt.send(Command::StreamLogin {
        service: Service::Spotify,
        input: LoginInput::Browser,
    });
    wait_for("Spotify switched on", || {
        let s = rt.state();
        s.prefs.spotify_enabled && account(&s, Service::Spotify).usable
    });
    assert!(account(&rt.state(), Service::Spotify).audio, "it plays at once");
}
