//! Against Spotify's real CDN and build (run by hand, ~10 MB download):
//! `cargo test -p nikgob-soloist --test official_build -- --ignored --nocapture`.
//! Downloads and unpacks the official build through the manager, gives it a
//! made-up key, and checks it starts, opens its API and waits to be chosen
//! in a Spotify app (the pairing step), then that quitting stops it. No
//! Premium account is involved: nothing is paired or played.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_core::streaming::{SoloistAction, SoloistPhase};
use nikgob_soloist::{Config, KeyStore, Soloist, install};

#[derive(Default)]
struct Keys(Mutex<Option<String>>);

impl KeyStore for Keys {
    fn load(&self) -> nikgob_soloist::BoxFuture<'_, Option<String>> {
        let k = self.0.lock().unwrap().clone();
        Box::pin(async move { k })
    }
    fn save(&self, key: String) -> nikgob_soloist::BoxFuture<'_, Result<(), String>> {
        *self.0.lock().unwrap() = Some(key);
        Box::pin(async { Ok(()) })
    }
    fn clear(&self) {
        *self.0.lock().unwrap() = None;
    }
}

async fn wait_for(s: &Soloist, what: &str, timeout: Duration, ok: impl Fn(&SoloistPhase) -> bool) -> SoloistPhase {
    let until = Instant::now() + timeout;
    loop {
        let p = s.status().phase;
        if ok(&p) {
            return p;
        }
        assert!(Instant::now() < until, "{what}: still {p:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "downloads Spotify's build and runs it; by hand"]
async fn download_start_and_wait_for_pairing() {
    let dir = tempfile::tempdir().unwrap();
    let keys = Arc::new(Keys::default());
    let s = Soloist::new(
        Config {
            root: dir.path().to_owned(),
            device_name: "nikgob-test".into(),
            user_agent: "nikgob-test".into(),
        },
        keys.clone(),
        tokio::runtime::Handle::current(),
    );
    wait_for(&s, "absent", Duration::from_secs(5), |p| *p == SoloistPhase::Absent).await;
    s.ask_size();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let size = s.status().size;
    println!("archive size: {size:?}");
    assert!(size.is_some_and(|n| n > 1_000_000 && n < 100_000_000));
    s.set_wanted(true);
    s.act(SoloistAction::Download);
    let p = wait_for(&s, "downloaded", Duration::from_secs(120), |p| {
        !matches!(p, SoloistPhase::Downloading { .. } | SoloistPhase::Absent)
    })
    .await;
    assert_eq!(p, SoloistPhase::NeedsKey, "downloaded, no key yet");
    let st = s.status();
    let left = st.expires_at.unwrap() - install::now();
    println!("expires in {} days", left / install::DAY);
    assert!(left > 0 && left <= install::LIFETIME + install::DAY);
    s.act(SoloistAction::SetKey("not-a-real-key".into()));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(keys.0.lock().unwrap().as_deref(), Some("not-a-real-key"), "the key went to the store");
    let p = wait_for(&s, "pairing", Duration::from_secs(30), |p| {
        matches!(p, SoloistPhase::Pairing | SoloistPhase::Failed(_) | SoloistPhase::NeedsKey)
    })
    .await;
    assert_eq!(p, SoloistPhase::Pairing, "running, waiting to be chosen in a Spotify app");
    let log = std::fs::read_to_string(dir.path().join("data").join("soloist-output.log")).unwrap();
    assert!(!log.contains("not-a-real-key"), "the key never reaches the log");
    s.shutdown();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!dir.path().join("data").join("ws.port").exists(), "it stopped and removed its port file");
}
