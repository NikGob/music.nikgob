//! Gapless: the next track is handed to the engine once. Repeat, shuffle
//! or a queue edit that leave the next track as it was do not hand it over
//! again (the engine would reopen its file); a real change does.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_core::model::{Artist, ArtistCredit, Codec, Source, SourceKind, Track};
use nikgob_library::Library;
use nikgob_runtime::{PlayRequest, Playback, PlaybackEvent, Runtime, RuntimeConfig};
use nikgob_view::{Command, Prefs};

/// Every `set_next` the engine got: the entry, or none.
#[derive(Clone, Default)]
struct Recorder {
    next: Arc<Mutex<Vec<Option<u64>>>>,
    played: Arc<Mutex<Vec<u64>>>,
}

impl Playback for Recorder {
    fn play(&mut self, item: PlayRequest) {
        self.played.lock().unwrap().push(item.id);
    }
    fn set_next(&mut self, item: Option<PlayRequest>) {
        self.next.lock().unwrap().push(item.map(|i| i.id));
    }
    fn pause(&mut self) {}
    fn resume(&mut self) {}
    fn stop(&mut self) {}
    fn seek(&mut self, _: Duration) {}
    fn set_volume(&mut self, _: f32) {}
    fn position(&self) -> Duration {
        Duration::ZERO
    }
}

fn write_wav(path: &Path) {
    let spec = hound::WavSpec { channels: 1, sample_rate: 8_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for i in 0..8_000u32 {
        w.write_sample(((i % 100) as i16 - 50) * 100).unwrap();
    }
    w.finalize().unwrap();
}

fn wait_for(what: &str, f: impl Fn() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(30), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The handovers of a real next track (not the clearings).
fn handed(r: &Recorder) -> Vec<u64> {
    r.next.lock().unwrap().iter().flatten().copied().collect()
}

#[test]
fn the_same_next_track_is_handed_over_once() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open_in_memory().unwrap();
    let artist = lib.upsert_artist(&Artist::new("A")).unwrap();
    let ids: Vec<_> = (0..4)
        .map(|i| {
            let t = lib.upsert_track(&Track::new(format!("t{i}"), ArtistCredit::single("A", Some(artist.id)))).unwrap();
            let file = dir.path().join(format!("t{i}.wav"));
            write_wav(&file);
            lib.add_source(&Source::new(t.id, SourceKind::LocalFile, file.display().to_string(), Codec::Wav)).unwrap();
            t.id
        })
        .collect();
    let rec = Recorder::default();
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib,
            streams: Vec::new(),
            library_dir: dir.path().to_owned(),
            prefs: Prefs { autoplay: false, ..Prefs::default() },
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(rec.clone()),
        Arc::new(|| {}),
    );
    rt.send(Command::PlayTracks { tracks: ids.clone(), start: 0, context: "test".into() });
    wait_for("the next track handed over", || handed(&rec).len() == 1);
    let playing = rec.played.lock().unwrap()[0];
    rt.playback_sender().send(PlaybackEvent::Started { id: playing }).unwrap();
    let next = handed(&rec)[0];
    let st = rt.state();
    let queued: Vec<u64> = st.queue.next.iter().map(|e| e.entry).collect();
    drop(st);
    assert_eq!(queued[0], next);

    // Repeat on, the last entry moved within the rest: the next track stays
    // the same and is not handed over again.
    rt.send(Command::CycleRepeat);
    rt.send(Command::MoveInQueue { entry: queued[2], anchor: queued[1], after: false });
    wait_for("the queue moved", || {
        let s = rt.state();
        s.queue.next.iter().map(|e| e.entry).collect::<Vec<_>>() == [queued[0], queued[2], queued[1]]
    });
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(handed(&rec), [next]);

    // The next track taken out: the one after it is handed over.
    rt.send(Command::RemoveFromQueue(next));
    wait_for("the new next track", || handed(&rec).len() == 2);
    assert_eq!(handed(&rec)[1], queued[2]);

    // The engine took it over gaplessly: the one after that comes, once.
    rt.playback_sender().send(PlaybackEvent::TrackChanged { to: queued[2] }).unwrap();
    wait_for("the track after it", || handed(&rec).len() == 3);
    assert_eq!(handed(&rec)[2], queued[1]);
    rt.send(Command::Enqueue(vec![ids[0]]));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(handed(&rec).len(), 3);

    // The engine failed to open it: it is handed over again when asked.
    rt.playback_sender().send(PlaybackEvent::Error { id: queued[1], message: "gone".into() }).unwrap();
    rt.send(Command::Enqueue(vec![ids[0]]));
    wait_for("handed over again", || handed(&rec).len() == 4);
    assert_eq!(handed(&rec)[3], queued[1]);
}
