//! Stage 4.5 runtime flows (docs/stage4.5.md) on fake download sources:
//! a wish found in a source other than torrents downloads its file into
//! the track, the user's order and switches of the sources decide among
//! offers of one quality, and a copy saved from YouTube keeps its item
//! looking until something better replaces it.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_core::indexer::BoxFuture;
use nikgob_core::model::{Codec, LikeTarget, TrackId, WishlistStatus, score};
use nikgob_core::source::{Fetched, Format, Offer, OfferScope, Progress, Source, SourceError, SourceKey, Want};
use nikgob_core::streaming::{FakeService, StreamService};
use nikgob_library::Library;
use nikgob_runtime::{ImportConfig, ImportSetup, PlayRequest, Playback, Runtime, RuntimeConfig};
use nikgob_view::{Command, Prefs, SourceSwitch, ViewState};

fn write_wav(path: &Path, secs: f32) {
    let rate = 44_100;
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let mut seed = 11u32;
    for i in 0..(secs * rate as f32) as u32 {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = ((seed >> 16) as f32 / 65_536.0 - 0.5) * 0.2;
        let tone = (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.3;
        let s = ((tone + noise) * 20_000.0) as i16;
        w.write_sample(s).unwrap();
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
}

struct Silent;

impl Playback for Silent {
    fn play(&mut self, _: PlayRequest) {}
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

/// A source with one offer of the track: its artist, title and length,
/// in `format`; a fetch writes a short WAV.
struct FakeSource {
    key: SourceKey,
    offer: Mutex<Option<Offer>>,
    searched: Mutex<u32>,
    fetched: Mutex<u32>,
}

impl FakeSource {
    fn new(key: SourceKey) -> Arc<Self> {
        Arc::new(Self {
            key,
            offer: Mutex::new(None),
            searched: Mutex::new(0),
            fetched: Mutex::new(0),
        })
    }

    fn offers(&self, artist: &str, title: &str, ms: u32, format: Format) {
        *self.offer.lock().unwrap() = Some(Offer {
            source: self.key,
            provider: self.key.display_name().into(),
            id: format!("{}-1", self.key.as_str()),
            title: format!("{artist} — {title}"),
            artist: Some(artist.into()),
            track_title: Some(title.into()),
            album: None,
            duration_ms: Some(ms),
            isrc: None,
            scope: OfferScope::Track,
            format: Some(format),
            size_bytes: Some(600_000),
            seeders: None,
            info_url: None,
            handle: "h".into(),
        });
    }
}

impl Source for FakeSource {
    fn key(&self) -> SourceKey {
        self.key
    }
    fn name(&self) -> &str {
        self.key.display_name()
    }
    fn search<'a>(&'a self, _: &'a Want) -> BoxFuture<'a, Result<Vec<Offer>, SourceError>> {
        *self.searched.lock().unwrap() += 1;
        let o = self.offer.lock().unwrap().clone();
        Box::pin(async move { Ok(o.into_iter().collect()) })
    }
    fn fetch<'a>(&'a self, _: &'a Offer, _: &'a Want, dest: &'a Path, progress: Progress) -> BoxFuture<'a, Result<Fetched, SourceError>> {
        *self.fetched.lock().unwrap() += 1;
        Box::pin(async move {
            let path = dest.join(format!("{}.wav", self.key.as_str()));
            write_wav(&path, 3.0);
            progress(10, Some(10));
            Ok(Fetched::Files(vec![path]))
        })
    }
}

struct Rig {
    rt: Runtime,
    _dir: tempfile::TempDir,
    cfg: ImportConfig,
}

fn rig(lib: &Library, streams: Vec<Arc<dyn StreamService>>, prefs: Prefs) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let cfg = ImportConfig::with_root(dir.path());
    for d in [&cfg.library_dir, &cfg.inbox_dir, &cfg.downloads_dir, &cfg.trash_dir] {
        std::fs::create_dir_all(d).unwrap();
    }
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
            streams,
            library_dir: cfg.library_dir.clone(),
            prefs,
            prefs_path: None,
            settings_path: None,
            import: Some(ImportSetup {
                config: cfg.clone(),
                metadata: None,
                watch_inbox: false,
                torrent_state_dir: dir.path().join("torrent-state"),
                torrent_offline: true,
            }),
        },
        Box::new(Silent),
        Arc::new(|| {}),
    );
    Rig { rt, _dir: dir, cfg }
}

fn wait_for(rt: &Runtime, what: &str, f: impl Fn(&ViewState) -> bool) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(60) {
        if f(&rt.state()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    let s = rt.state();
    panic!(
        "timed out waiting for {what}: toasts {:?}, wishlist {:?}",
        s.toasts.iter().map(|t| &t.text).collect::<Vec<_>>(),
        s.downloads.wishlist.items
    );
}

fn wait_until(what: &str, f: impl Fn() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(60), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(30));
    }
}

fn wish_status(lib: &Library, track: TrackId) -> Option<WishlistStatus> {
    lib.wishlist_for_track(track).unwrap().last().map(|i| i.status)
}

/// A stream track of the fake Spotify, 3 s long like the files the fake
/// sources write.
fn gruppa(lib: &Library) -> (TrackId, String, String) {
    let mut t = FakeService::spotify().tracks().into_iter().find(|t| t.id.id == "sp-gruppa-1").unwrap();
    t.duration_ms = Some(3_000);
    let id = lib.upsert_stream_track(&t).unwrap();
    let row = lib.get_track(id).unwrap().unwrap();
    (id, row.artist_credit.display(), row.title)
}

fn flac() -> Format {
    Format { bit_depth: Some(16), sample_rate: Some(44_100), ..Format::new(Codec::Flac) }
}

#[test]
fn a_wish_found_in_a_catalogue_lands_on_the_same_track() {
    let lib = Library::open_in_memory().unwrap();
    let (track, artist, title) = gruppa(&lib);
    let r = rig(&lib, vec![Arc::new(FakeService::spotify().logged_out())], Prefs::default());
    let qobuz = FakeSource::new(SourceKey::Qobuz);
    qobuz.offers(&artist, &title, 3_000, flac());
    // Another artist's version: never taken.
    let tidal = FakeSource::new(SourceKey::Tidal);
    tidal.offers("Ария", &title, 3_000, flac());
    r.rt.add_source(qobuz.clone());
    r.rt.add_source(tidal.clone());
    wait_for(&r.rt, "the sources", |s| s.downloads.sources.iter().filter(|v| v.available).count() == 2);

    r.rt.send(Command::Like(LikeTarget::Track(track), true));
    wait_until("the file", || lib.active_source(track).unwrap().is_some_and(|s| s.kind.is_local()));
    wait_until("the item done", || wish_status(&lib, track) == Some(WishlistStatus::Done));
    assert_eq!(*qobuz.fetched.lock().unwrap(), 1);
    assert_eq!(*tidal.fetched.lock().unwrap(), 0, "another artist");
    assert!(*tidal.searched.lock().unwrap() >= 1, "every source is asked");
    let src = lib.active_source(track).unwrap().unwrap();
    assert!(src.quality_score > score::SAVED_STREAM);
    // The download's own folder went once the import took the file.
    let sources = r.cfg.downloads_dir.join(".sources");
    wait_until("the folder gone", || std::fs::read_dir(&sources).map_or(true, |d| d.count() == 0));
    assert!(r.rt.state().downloads.wishlist.can_search);
}

#[test]
fn the_users_order_and_switches_pick_among_equals() {
    let lib = Library::open_in_memory().unwrap();
    let (track, artist, title) = gruppa(&lib);
    // Tidal before Qobuz, khinsider off.
    let prefs = Prefs {
        download_sources: vec![
            SourceSwitch { key: SourceKey::Tidal, on: true },
            SourceSwitch { key: SourceKey::Qobuz, on: true },
            SourceSwitch { key: SourceKey::Khinsider, on: false },
        ],
        ..Prefs::default()
    };
    let r = rig(&lib, vec![Arc::new(FakeService::spotify().logged_out())], prefs);
    let (qobuz, tidal, kh) = (FakeSource::new(SourceKey::Qobuz), FakeSource::new(SourceKey::Tidal), FakeSource::new(SourceKey::Khinsider));
    qobuz.offers(&artist, &title, 3_000, flac());
    tidal.offers(&artist, &title, 3_000, flac());
    kh.offers(&artist, &title, 3_000, flac());
    for s in [qobuz.clone(), tidal.clone(), kh.clone()] {
        r.rt.add_source(s);
    }
    wait_for(&r.rt, "the sources", |s| s.downloads.sources.iter().filter(|v| v.available).count() == 3);
    // Torrents, not listed, keep their default place: first.
    let order: Vec<SourceKey> = r.rt.state().downloads.sources.iter().map(|v| v.key).collect();
    assert_eq!(order, [SourceKey::Torrents, SourceKey::Tidal, SourceKey::Qobuz, SourceKey::Khinsider, SourceKey::Soulseek]);
    r.rt.send(Command::Like(LikeTarget::Track(track), true));
    wait_until("the item done", || wish_status(&lib, track) == Some(WishlistStatus::Done));
    assert_eq!((*tidal.fetched.lock().unwrap(), *qobuz.fetched.lock().unwrap()), (1, 0));
    assert_eq!(*kh.searched.lock().unwrap(), 0, "switched off: not asked");
}

#[test]
fn a_youtube_copy_keeps_looking_and_a_better_file_replaces_it() {
    let lib = Library::open_in_memory().unwrap();
    let yt = FakeService::youtube().logged_out();
    let t = yt.tracks().into_iter().find(|t| t.id.id == "yt-gruppa-2").unwrap();
    let track = lib.upsert_stream_track(&t).unwrap();
    let row = lib.get_track(track).unwrap().unwrap();
    let r = rig(&lib, vec![Arc::new(yt)], Prefs::default());
    // Nothing to search with yet: the item waits; the copy comes.
    r.rt.send(Command::Like(LikeTarget::Track(track), true));
    wait_until("the item", || wish_status(&lib, track).is_some());
    let item = lib.wishlist_for_track(track).unwrap().pop().unwrap();
    r.rt.send(Command::WishlistSaveFromYoutube(item.id));
    wait_until("the copy", || lib.active_source(track).unwrap().is_some_and(|s| s.kind.is_local()));
    assert_eq!(lib.active_source(track).unwrap().unwrap().quality_score, score::SAVED_STREAM);
    wait_for(&r.rt, "the row with its copy", |s| {
        s.downloads.wishlist.items.iter().any(|i| i.id == item.id && i.has_copy && i.status == WishlistStatus::NoTorrent)
    });
    let it = lib.get_wishlist_item(item.id).unwrap().unwrap();
    assert!(it.next_try_at.unwrap() > nikgob_library::now_ms() + 6 * 86_400_000, "weekly");

    // A lossy file better than the copy: it downloads at once (no
    // «Подтверди») and replaces the copy.
    let qobuz = FakeSource::new(SourceKey::Qobuz);
    let mp3 = Format { bitrate_kbps: Some(320), ..Format::new(Codec::Mp3) };
    // As long as the track is now (the copy's length).
    let ms = lib.get_track(track).unwrap().unwrap().duration_ms.unwrap_or(3_000);
    qobuz.offers(&row.artist_credit.display(), &row.title, ms, mp3);
    r.rt.add_source(qobuz.clone());
    r.rt.send(Command::WishlistRetry(item.id));
    wait_for(&r.rt, "the better file", |_| {
        lib.active_source(track).unwrap().is_some_and(|s| s.kind.is_local() && s.quality_score > score::SAVED_STREAM)
    });
    wait_until("the item done", || wish_status(&lib, track) == Some(WishlistStatus::Done));
    assert_eq!(*qobuz.fetched.lock().unwrap(), 1);
}

#[test]
fn the_users_own_file_replaces_a_youtube_copy() {
    let lib = Library::open_in_memory().unwrap();
    let yt = FakeService::youtube().logged_out();
    let t = yt.tracks().into_iter().find(|t| t.id.id == "yt-gruppa-2").unwrap();
    let track = lib.upsert_stream_track(&t).unwrap();
    let r = rig(&lib, vec![Arc::new(yt)], Prefs::default());
    r.rt.send(Command::Like(LikeTarget::Track(track), true));
    wait_until("the item", || wish_status(&lib, track).is_some());
    let item = lib.wishlist_for_track(track).unwrap().pop().unwrap();
    r.rt.send(Command::WishlistSaveFromYoutube(item.id));
    wait_until("the copy", || lib.active_source(track).unwrap().is_some_and(|s| s.kind.is_local()));
    let copy = lib.active_source(track).unwrap().unwrap();
    assert_eq!(copy.quality_score, score::SAVED_STREAM);
    wait_until("the item keeps looking", || wish_status(&lib, track) == Some(WishlistStatus::NoTorrent));

    // «Свой файл…» on the row: a lossless file of the user's.
    let own = r._dir.path().join("own.wav");
    let secs = lib.get_track(track).unwrap().unwrap().duration_ms.unwrap_or(3_000) as f32 / 1000.0;
    write_wav(&own, secs);
    r.rt.send(Command::WishlistAttach {
        item: item.id,
        what: nikgob_view::WishlistAttach::Files(vec![own]),
    });
    wait_for(&r.rt, "the own file", |_| {
        lib.active_source(track).unwrap().is_some_and(|s| s.kind.is_local() && s.quality_score > score::SAVED_STREAM)
    });
    wait_until("the item done", || wish_status(&lib, track) == Some(WishlistStatus::Done));
    // The copy is no longer what plays, and its file left the library.
    let now = lib.active_source(track).unwrap().unwrap();
    assert_ne!(now.id, copy.id);
    assert!(!Path::new(&copy.locator).starts_with(&r.cfg.library_dir) || !Path::new(&copy.locator).exists(), "{}", copy.locator);
}

#[test]
fn the_weekly_search_of_a_youtube_copy_replaces_it_by_itself() {
    let lib = Library::open_in_memory().unwrap();
    let yt = FakeService::youtube().logged_out();
    let t = yt.tracks().into_iter().find(|t| t.id.id == "yt-gruppa-2").unwrap();
    let track = lib.upsert_stream_track(&t).unwrap();
    let row = lib.get_track(track).unwrap().unwrap();
    let r = rig(&lib, vec![Arc::new(yt)], Prefs::default());
    r.rt.send(Command::Like(LikeTarget::Track(track), true));
    wait_until("the item", || wish_status(&lib, track).is_some());
    let item = lib.wishlist_for_track(track).unwrap().pop().unwrap();
    r.rt.send(Command::WishlistSaveFromYoutube(item.id));
    wait_until("the copy", || lib.active_source(track).unwrap().is_some_and(|s| s.kind.is_local()));
    wait_until("the item keeps looking", || {
        lib.get_wishlist_item(item.id).unwrap().is_some_and(|i| i.status == WishlistStatus::NoTorrent && i.next_try_at.is_some())
    });
    // A week went by: the item is due, and a source has a FLAC now. No
    // click: the scheduler finds it.
    let mut due = lib.get_wishlist_item(item.id).unwrap().unwrap();
    due.next_try_at = Some(nikgob_library::now_ms() - 1);
    lib.put_wishlist_item(&due).unwrap();
    let tidal = FakeSource::new(SourceKey::Tidal);
    // As long as the track is now (the copy's length).
    let ms = lib.get_track(track).unwrap().unwrap().duration_ms.unwrap_or(3_000);
    tidal.offers(&row.artist_credit.display(), &row.title, ms, flac());
    r.rt.add_source(tidal.clone());
    wait_for(&r.rt, "the FLAC", |_| {
        lib.active_source(track).unwrap().is_some_and(|s| s.kind.is_local() && s.quality_score > score::SAVED_STREAM)
    });
    wait_until("the item done", || wish_status(&lib, track) == Some(WishlistStatus::Done));
    assert_eq!(*tidal.fetched.lock().unwrap(), 1);
}
