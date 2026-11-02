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
    /// What its downloads answer instead of a file.
    refuse: Mutex<Option<SourceError>>,
}

impl FakeSource {
    fn new(key: SourceKey) -> Arc<Self> {
        Arc::new(Self {
            key,
            offer: Mutex::new(None),
            searched: Mutex::new(0),
            fetched: Mutex::new(0),
            refuse: Mutex::new(None),
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
        let refused = self.refuse.lock().unwrap().clone();
        Box::pin(async move {
            if let Some(e) = refused {
                return Err(e);
            }
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
                models_dir: None,
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

/// Tidal found the song but its only instance refused the file: «Источники»
/// says so on its row, and a download that works again clears it.
#[test]
fn a_source_whose_downloads_fail_says_so_on_its_row() {
    let lib = Library::open_in_memory().unwrap();
    let (track, artist, title) = gruppa(&lib);
    let r = rig(&lib, vec![Arc::new(FakeService::spotify().logged_out())], Prefs::default());
    let tidal = FakeSource::new(SourceKey::Tidal);
    tidal.offers(&artist, &title, 3_000, flac());
    *tidal.refuse.lock().unwrap() = Some(SourceError::Http { status: 403 });
    r.rt.add_source(tidal.clone());
    let status = |s: &ViewState| s.downloads.sources.iter().find(|v| v.key == SourceKey::Tidal).and_then(|v| v.status.clone());
    wait_for(&r.rt, "the source", |s| s.downloads.sources.iter().any(|v| v.key == SourceKey::Tidal && v.available));
    assert_eq!(status(&r.rt.state()), None);

    r.rt.send(Command::Like(LikeTarget::Track(track), true));
    wait_for(&r.rt, "the row says it", |s| status(s).as_deref() == Some("Находит, но не скачивает"));
    assert!(*tidal.fetched.lock().unwrap() >= 1);

    // It gives files again: the row is quiet once one comes.
    *tidal.refuse.lock().unwrap() = None;
    let item = lib.wishlist_for_track(track).unwrap().last().unwrap().id;
    r.rt.send(Command::WishlistRetry(item));
    wait_for(&r.rt, "the row quiet", |s| status(s).is_none());
    wait_until("the item done", || wish_status(&lib, track) == Some(WishlistStatus::Done));
}

/// Nothing had the song; a source added later has it, and «Искать заново»
/// finds it there without waiting for the weekly search.
#[test]
fn searching_all_again_finds_what_a_new_source_has() {
    let lib = Library::open_in_memory().unwrap();
    let (track, artist, title) = gruppa(&lib);
    let r = rig(&lib, vec![Arc::new(FakeService::spotify().logged_out())], Prefs::default());
    let tidal = FakeSource::new(SourceKey::Tidal);
    r.rt.add_source(tidal.clone());
    wait_for(&r.rt, "the source", |s| s.downloads.sources.iter().any(|v| v.key == SourceKey::Tidal && v.available));
    r.rt.send(Command::Like(LikeTarget::Track(track), true));
    wait_until("not found", || wish_status(&lib, track) == Some(WishlistStatus::NoTorrent));

    let qobuz = FakeSource::new(SourceKey::Qobuz);
    qobuz.offers(&artist, &title, 3_000, flac());
    r.rt.add_source(qobuz.clone());
    wait_for(&r.rt, "the new source", |s| s.downloads.sources.iter().filter(|v| v.available).count() == 2);
    r.rt.send(Command::WishlistRetryAll);
    wait_until("the item done", || wish_status(&lib, track) == Some(WishlistStatus::Done));
    assert_eq!(*qobuz.fetched.lock().unwrap(), 1);
}

/// An uploader's «Performer - Song» found as the performer's: the file
/// carries the performer and the song, not the uploader.
#[test]
fn an_uploaders_song_lands_under_its_performer() {
    let lib = Library::open_in_memory().unwrap();
    let mut t = FakeService::spotify().tracks().into_iter().find(|t| t.id.id == "sp-gruppa-1").unwrap();
    t.duration_ms = Some(3_000);
    t.title = "Комсомольск - Мозги & деньги".into();
    t.artists = vec![nikgob_core::streaming::ArtistRef::new(None, "Gg2")];
    t.album = None;
    let track = lib.upsert_stream_track(&t).unwrap();
    let r = rig(&lib, vec![Arc::new(FakeService::spotify().logged_out())], Prefs::default());
    let qobuz = FakeSource::new(SourceKey::Qobuz);
    qobuz.offers("Комсомольск", "Мозги & деньги", 3_000, flac());
    r.rt.add_source(qobuz.clone());
    wait_for(&r.rt, "the source", |s| s.downloads.sources.iter().any(|v| v.key == SourceKey::Qobuz && v.available));

    r.rt.send(Command::Like(LikeTarget::Track(track), true));
    // Found through the title, not the uploader: the user says it is it.
    wait_for(&r.rt, "«Подтверди»", |s| !s.downloads.confirm.is_empty());
    let id = r.rt.state().downloads.confirm[0].id;
    r.rt.send(Command::ConfirmChoice { id, option: Some(0) });
    wait_until("the item done", || wish_status(&lib, track) == Some(WishlistStatus::Done));
    wait_until("the performer's names", || {
        lib.get_track(track).unwrap().is_some_and(|t| t.artist_credit.display() == "Комсомольск" && t.title == "Мозги & деньги")
    });
    let row = lib.get_track(track).unwrap().unwrap();
    let release = lib.get_release(row.release_id.unwrap()).unwrap().unwrap();
    assert_eq!((release.title.as_str(), release.artist_credit.display().as_str()), ("Мозги & деньги", "Комсомольск"));
    let file = lib.active_source(track).unwrap().unwrap().locator;
    assert!(file.contains("Комсомольск") && !file.contains("Gg2"), "{file}");
}

/// Turning a source on searches again by itself what was not found.
#[test]
fn a_source_turned_on_searches_again_what_was_not_found() {
    let lib = Library::open_in_memory().unwrap();
    let (track, artist, title) = gruppa(&lib);
    let prefs = Prefs {
        download_sources: vec![
            SourceSwitch { key: SourceKey::Tidal, on: true },
            SourceSwitch { key: SourceKey::Qobuz, on: false },
        ],
        ..Prefs::default()
    };
    let r = rig(&lib, vec![Arc::new(FakeService::spotify().logged_out())], prefs);
    let (tidal, qobuz) = (FakeSource::new(SourceKey::Tidal), FakeSource::new(SourceKey::Qobuz));
    qobuz.offers(&artist, &title, 3_000, flac());
    r.rt.add_source(tidal.clone());
    r.rt.add_source(qobuz.clone());
    wait_for(&r.rt, "the sources", |s| s.downloads.sources.iter().filter(|v| v.available).count() == 2);
    r.rt.send(Command::Like(LikeTarget::Track(track), true));
    wait_until("not found", || wish_status(&lib, track) == Some(WishlistStatus::NoTorrent));
    assert_eq!(*qobuz.searched.lock().unwrap(), 0, "off: not asked");

    let mut prefs = r.rt.state().prefs.clone();
    prefs.download_sources = vec![
        SourceSwitch { key: SourceKey::Tidal, on: true },
        SourceSwitch { key: SourceKey::Qobuz, on: true },
    ];
    r.rt.send(Command::SetPrefs(Box::new(prefs)));
    wait_until("the item done", || wish_status(&lib, track) == Some(WishlistStatus::Done));
    assert_eq!(*qobuz.fetched.lock().unwrap(), 1);
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

#[test]
fn soulseek_is_off_by_default_and_its_account_goes_to_the_settings_file() {
    let lib = Library::open_in_memory().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("settings.toml");
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib,
            streams: Vec::new(),
            library_dir: dir.path().join("lib"),
            prefs: Prefs::default(),
            prefs_path: None,
            settings_path: Some(settings.clone()),
            import: None,
        },
        Box::new(Silent),
        Arc::new(|| {}),
    );
    assert!(!Prefs::default().source_on(SourceKey::Soulseek), "off until the user turns it on");
    rt.send(Command::SetSoulseek(nikgob_view::SoulseekForm {
        username: "me".into(),
        password: "secret".into(),
        ..Default::default()
    }));
    wait_for(&rt, "the saved account", |s| s.downloads.soulseek.configured());
    let view = rt.state().downloads.soulseek.clone();
    assert!(!view.running, "its switch is off: not started");
    assert_eq!(view.username, "me");
    let saved = nikgob_core::settings::Settings::load(&settings).unwrap().soulseek;
    assert!(saved.enabled);
    assert_eq!((saved.username.as_str(), saved.password.as_str()), ("me", "secret"));
    // An empty password keeps the saved one; sharing changes on its own.
    rt.send(Command::SetSoulseek(nikgob_view::SoulseekForm { username: "me2".into(), ..Default::default() }));
    rt.send(Command::SetSoulseekShare { share: false, upload_limit_kbps: 256 });
    wait_for(&rt, "the new settings", |s| s.downloads.soulseek.username == "me2" && !s.downloads.soulseek.share);
    let saved = nikgob_core::settings::Settings::load(&settings).unwrap().soulseek;
    assert_eq!((saved.password.as_str(), saved.share, saved.upload_limit_kbps), ("secret", false, 256));
    assert!(!format!("{:?}", rt.state().downloads.soulseek).contains("secret"));
}
