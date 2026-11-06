//! Spotify Soloist in the runtime: its status reaches the settings, the
//! setup's steps go to the service, and a Spotify track played while it is
//! ready says so (the controls that do not apply to it are disabled).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_core::model::TrackId;
use nikgob_core::streaming::{FakeService, Service, SoloistAction, SoloistPhase, StreamService};
use nikgob_library::Library;
use nikgob_runtime::{PlayRequest, PlaySource, Playback, Runtime, RuntimeConfig};
use nikgob_view::{Command, Prefs, ViewState};

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

fn wait_for(rt: &Runtime, what: &str, f: impl Fn(&ViewState) -> bool) {
    let t = Instant::now();
    while !f(&rt.state()) {
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

fn phase(s: &ViewState) -> Option<SoloistPhase> {
    s.accounts
        .iter()
        .find(|a| a.service == Service::Spotify)
        .and_then(|a| a.soloist.as_ref())
        .map(|st| st.phase.clone())
}

#[test]
fn the_setup_steps_reach_the_service_and_come_back_to_the_settings() {
    let lib = Library::open_in_memory().unwrap();
    let sp = Arc::new(FakeService::spotify().with_soloist(SoloistPhase::Absent, true));
    let (rt, _) = start(&lib, vec![sp.clone()]);
    wait_for(&rt, "the offer", |s| {
        s.accounts
            .iter()
            .any(|a| a.service == Service::Spotify && a.soloist.as_ref().is_some_and(|st| st.offer))
    });
    rt.send(Command::Soloist(SoloistAction::Download));
    wait_for(&rt, "the key step", |s| phase(s) == Some(SoloistPhase::NeedsKey));
    rt.send(Command::Soloist(SoloistAction::SetKey("a-key-of-mine".into())));
    wait_for(&rt, "the pairing step", |s| phase(s) == Some(SoloistPhase::Pairing));
    rt.send(Command::Soloist(SoloistAction::Disable));
    wait_for(&rt, "off", |s| phase(s) == Some(SoloistPhase::Off));
    assert!(!rt.state().player.soloist);
}

#[test]
fn a_spotify_track_played_while_soloist_is_ready_is_marked() {
    let lib = Library::open_in_memory().unwrap();
    let sp = Arc::new(FakeService::spotify().with_soloist(SoloistPhase::Ready, false));
    let one = sp.tracks().into_iter().find(|t| t.id.id == "sp-gruppa-1").unwrap();
    let track: TrackId = lib.upsert_stream_track(&one).unwrap();
    let (rt, asked) = start(&lib, vec![sp.clone()]);
    wait_for(&rt, "the account", |s| phase(s) == Some(SoloistPhase::Ready));
    rt.send(Command::PlayTracks { tracks: vec![track], start: 0, context: "test".into() });
    wait_for(&rt, "the Soloist mark", |s| s.player.soloist);
    assert!(matches!(
        &asked.lock().unwrap()[0].source,
        PlaySource::Stream { service: Service::Spotify, .. }
    ));
}
