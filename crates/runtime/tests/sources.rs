//! Stage 4.5 runtime flows on fake download sources:
//! a wish found in a source other than torrents downloads its file into
//! the track, the user's order and switches of the sources decide among
//! offers of one quality, and a copy saved from YouTube keeps its item
//! looking until something better replaces it. A source that turns
//! downloads away hands over to the next one, is waited for, and is set
//! aside when it keeps doing so; an album that came in part keeps its
//! files.

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
    /// What its next search answers instead of the offer (once).
    search_err: Mutex<Option<SourceError>>,
    /// The album's track titles, when its offer is an album.
    album: Mutex<Vec<String>>,
    /// Tracks of the album it gives before `refuse` (all when `None`).
    give: Mutex<Option<usize>>,
    /// The titles its album fetches were asked for.
    asked: Mutex<Vec<String>>,
}

impl FakeSource {
    fn new(key: SourceKey) -> Arc<Self> {
        Arc::new(Self {
            key,
            offer: Mutex::new(None),
            searched: Mutex::new(0),
            fetched: Mutex::new(0),
            refuse: Mutex::new(None),
            search_err: Mutex::new(None),
            album: Mutex::new(Vec::new()),
            give: Mutex::new(None),
            asked: Mutex::new(Vec::new()),
        })
    }

    /// One offer of the whole album `title` of `artist`, its `tracks`.
    fn offers_album(&self, artist: &str, title: &str, tracks: &[&str]) {
        *self.album.lock().unwrap() = tracks.iter().map(|t| (*t).to_owned()).collect();
        *self.offer.lock().unwrap() = Some(Offer {
            source: self.key,
            provider: self.key.display_name().into(),
            id: format!("{}-album", self.key.as_str()),
            title: format!("{artist} — {title}"),
            artist: Some(artist.into()),
            track_title: None,
            album: Some(title.into()),
            duration_ms: None,
            isrc: None,
            scope: OfferScope::Album,
            format: Some(flac()),
            size_bytes: None,
            seeders: None,
            info_url: None,
            handle: "album:1".into(),
        });
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
        let err = self.search_err.lock().unwrap().take();
        Box::pin(async move {
            match err {
                Some(e) => Err(e),
                None => Ok(o.into_iter().collect()),
            }
        })
    }
    fn fetch<'a>(&'a self, offer: &'a Offer, want: &'a Want, dest: &'a Path, progress: Progress) -> BoxFuture<'a, Result<Fetched, SourceError>> {
        *self.fetched.lock().unwrap() += 1;
        let refused = self.refuse.lock().unwrap().clone();
        let album = self.album.lock().unwrap().clone();
        let give = *self.give.lock().unwrap();
        Box::pin(async move {
            if offer.scope == OfferScope::Album {
                // Each track not here yet, as «NN - Title.wav»; past `give`
                // the source refuses.
                let (mut files, mut missing) = (Vec::new(), Vec::new());
                for (i, t) in album.iter().enumerate() {
                    if want.got.iter().any(|g| g.eq_ignore_ascii_case(t)) {
                        continue;
                    }
                    self.asked.lock().unwrap().push(t.clone());
                    if give.is_some_and(|g| files.len() >= g) || (give.is_none() && refused.is_some()) {
                        missing.push(t.clone());
                        continue;
                    }
                    let path = dest.join(format!("{:02} - {t}.wav", i + 1));
                    write_wav(&path, 3.0);
                    files.push(path);
                }
                let error = refused.unwrap_or(SourceError::Http { status: 403 });
                return match (files.is_empty(), missing.is_empty()) {
                    (_, true) => Ok(Fetched::Files(files)),
                    (true, false) => Err(error),
                    (false, false) => Ok(Fetched::Partial { files, missing, error }),
                };
            }
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
    assert_eq!(
        order,
        [
            SourceKey::Torrents,
            SourceKey::Tidal,
            SourceKey::Qobuz,
            SourceKey::Khinsider,
            SourceKey::Bandcamp,
            SourceKey::Soulseek,
            SourceKey::Squidify,
            SourceKey::Archive,
            SourceKey::Soundcloud,
            SourceKey::Dogmazic
        ]
    );
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

fn tidal_first() -> Prefs {
    Prefs {
        download_sources: vec![
            SourceSwitch { key: SourceKey::Tidal, on: true },
            SourceSwitch { key: SourceKey::Qobuz, on: true },
            SourceSwitch { key: SourceKey::Khinsider, on: true },
        ],
        ..Prefs::default()
    }
}

/// A note as it was kept, in the words it stands for.
fn words(note: Option<&str>) -> String {
    note.map_or_else(String::new, |n| nikgob_core::i18n::status::line_words_in(nikgob_core::i18n::Locale::Ru, n))
}

fn history(rt: &Runtime) -> Vec<String> {
    rt.state()
        .downloads
        .history
        .iter()
        .map(|j| format!("{} | {} | {}", j.title, j.subtitle, j.error.clone().unwrap_or_default()))
        .collect()
}

/// Stream tracks of one album `album` of «Тестер», 3 s long like the
/// files the fake sources write; the album's id.
fn album_of(lib: &Library, album: &str, titles: &[&str]) -> (nikgob_core::model::ReleaseId, Vec<TrackId>) {
    use nikgob_core::streaming::{AlbumRef, ArtistRef, RemoteId, Service, StreamTrack};
    let mut tracks = Vec::new();
    for (i, t) in titles.iter().enumerate() {
        let mut s = StreamTrack::new(RemoteId::new(Service::Spotify, format!("sp-{album}-{i}")), *t);
        s.artists = vec![ArtistRef::new(None, "Тестер")];
        s.album = Some(AlbumRef::new(Some(RemoteId::new(Service::Spotify, format!("sp-alb-{album}"))), album));
        s.duration_ms = Some(3_000);
        s.track_no = Some(i as u32 + 1);
        s.disc_no = Some(1);
        tracks.push(lib.upsert_stream_track(&s).unwrap());
    }
    let release = lib.get_track(tracks[0]).unwrap().unwrap().release_id.unwrap();
    (release, tracks)
}

fn album_item(lib: &Library, release: nikgob_core::model::ReleaseId) -> nikgob_core::model::WishlistItem {
    lib.wishlist_for_release(release)
        .unwrap()
        .into_iter()
        .find(|i| i.scope == nikgob_core::model::WishlistScope::Album)
        .unwrap()
}

/// Tidal is first and finds the song, but its download answers 403: Qobuz,
/// next, gives it; once it came, «История» keeps no line of the refusal
/// (one line per wish: the import's).
#[test]
fn a_refused_download_falls_back_to_the_next_source() {
    let lib = Library::open_in_memory().unwrap();
    let (track, artist, title) = gruppa(&lib);
    let r = rig(&lib, vec![Arc::new(FakeService::spotify().logged_out())], tidal_first());
    let (tidal, qobuz) = (FakeSource::new(SourceKey::Tidal), FakeSource::new(SourceKey::Qobuz));
    tidal.offers(&artist, &title, 3_000, flac());
    qobuz.offers(&artist, &title, 3_000, flac());
    *tidal.refuse.lock().unwrap() = Some(SourceError::Http { status: 403 });
    r.rt.add_source(tidal.clone());
    r.rt.add_source(qobuz.clone());
    wait_for(&r.rt, "the sources", |s| s.downloads.sources.iter().filter(|v| v.available).count() == 2);

    r.rt.send(Command::Like(LikeTarget::Track(track), true));
    wait_until("the item done", || wish_status(&lib, track) == Some(WishlistStatus::Done));
    assert_eq!((*tidal.fetched.lock().unwrap(), *qobuz.fetched.lock().unwrap()), (1, 1));
    wait_for(&r.rt, "no refusal line left", |s| !s.downloads.history.iter().any(|j| j.subtitle.contains("не отдаёт")));
    let item = lib.wishlist_for_track(track).unwrap().pop().unwrap();
    assert!(lib.job_log_with("import", "wish:", 50).unwrap().iter().all(|e| e.kind != format!("wish:{}", item.id)));
}

/// Qobuz asks to wait at the search and nothing else has the song: not
/// «не нашёл» for a week, but a search again after the wait, of Qobuz only.
#[test]
fn a_source_asking_to_wait_is_searched_again_after_the_wait() {
    let lib = Library::open_in_memory().unwrap();
    let (track, artist, title) = gruppa(&lib);
    let r = rig(&lib, vec![Arc::new(FakeService::spotify().logged_out())], tidal_first());
    let (tidal, qobuz) = (FakeSource::new(SourceKey::Tidal), FakeSource::new(SourceKey::Qobuz));
    qobuz.offers(&artist, &title, 3_000, flac());
    *qobuz.search_err.lock().unwrap() = Some(SourceError::RateLimited { retry_after: Some(Duration::from_secs(90)) });
    r.rt.add_source(tidal.clone());
    r.rt.add_source(qobuz.clone());
    wait_for(&r.rt, "the sources", |s| s.downloads.sources.iter().filter(|v| v.available).count() == 2);

    let before = nikgob_library::now_ms();
    r.rt.send(Command::Like(LikeTarget::Track(track), true));
    wait_until("the retry planned", || {
        lib.wishlist_for_track(track).unwrap().last().is_some_and(|i| i.status == WishlistStatus::New && i.next_try_at.is_some())
    });
    let item = lib.wishlist_for_track(track).unwrap().pop().unwrap();
    let at = item.next_try_at.unwrap();
    assert!(at >= before + 4 * 60_000 && at < before + 3_600_000, "minutes, not a week: {}", at - before);
    let note = words(item.note.as_deref());
    assert!(note.starts_with("Qobuz просит подождать · в "), "{note}");
    assert!(note.ends_with(" спрошу Qobuz"), "{note}");
    wait_for(&r.rt, "its line", |s| s.downloads.history.iter().any(|j| j.subtitle.starts_with("Qobuz просит подождать")));
    assert_eq!(*qobuz.fetched.lock().unwrap(), 0);

    // The wait is over: the scheduler asks Qobuz again (and only it). The
    // source's own pause (a minute at least, by the clock) is ended the way
    // the user ends it, Qobuz turned off and on; the item is made due by hand.
    let set_qobuz = |on: bool| {
        let mut prefs = r.rt.state().prefs.clone();
        prefs.download_sources = vec![SourceSwitch { key: SourceKey::Tidal, on: true }, SourceSwitch { key: SourceKey::Qobuz, on }];
        r.rt.send(Command::SetPrefs(Box::new(prefs)));
        wait_for(&r.rt, "the switch", |s| s.prefs.source_on(SourceKey::Qobuz) == on);
    };
    set_qobuz(false);
    let mut due = item.clone();
    due.next_try_at = Some(nikgob_library::now_ms() - 1);
    lib.put_wishlist_item(&due).unwrap();
    set_qobuz(true);
    let kh = FakeSource::new(SourceKey::Khinsider);
    r.rt.add_source(kh.clone());
    wait_until("the item done", || wish_status(&lib, track) == Some(WishlistStatus::Done));
    assert_eq!(*qobuz.searched.lock().unwrap(), 2);
    assert_eq!(*tidal.searched.lock().unwrap(), 1, "it had nothing: not asked again");
    assert_eq!(*kh.searched.lock().unwrap(), 0);
}

/// An album: Tidal gives one track and then answers 403; that track stays,
/// and Qobuz is asked only for the other two.
#[test]
fn an_album_that_came_in_part_keeps_its_tracks_and_gets_the_rest_elsewhere() {
    let lib = Library::open_in_memory().unwrap();
    let titles = ["Первая", "Вторая", "Третья"];
    let (release, tracks) = album_of(&lib, "Пыль", &titles);
    let r = rig(&lib, vec![Arc::new(FakeService::spotify().logged_out())], tidal_first());
    let (tidal, qobuz) = (FakeSource::new(SourceKey::Tidal), FakeSource::new(SourceKey::Qobuz));
    tidal.offers_album("Тестер", "Пыль", &titles);
    qobuz.offers_album("Тестер", "Пыль", &titles);
    *tidal.give.lock().unwrap() = Some(1);
    *tidal.refuse.lock().unwrap() = Some(SourceError::Http { status: 403 });
    r.rt.add_source(tidal.clone());
    r.rt.add_source(qobuz.clone());
    wait_for(&r.rt, "the sources", |s| s.downloads.sources.iter().filter(|v| v.available).count() == 2);

    r.rt.send(Command::DownloadRelease(release));
    let local = |t: TrackId| lib.active_source(t).unwrap().is_some_and(|s| s.kind.is_local());
    wait_until("every track", || tracks.iter().all(|t| local(*t)));
    wait_until("the album done", || album_item(&lib, release).status == WishlistStatus::Done);
    assert_eq!(album_item(&lib, release).note.as_deref(), Some("Скачано"));
    assert_eq!(*tidal.asked.lock().unwrap(), titles, "Tidal was asked for all");
    assert_eq!(*qobuz.asked.lock().unwrap(), ["Вторая", "Третья"], "Qobuz only for what was missing");
    wait_for(&r.rt, "its line gone once all came", |s| !s.downloads.history.iter().any(|j| j.subtitle.contains("Скачано 1 из 3")));
}

/// An album only Tidal has, and it asks to wait after one track: the track
/// stays, the item waits for later (not «Скачано», not a failure).
#[test]
fn an_album_cut_short_by_a_refusal_waits_for_its_rest() {
    let lib = Library::open_in_memory().unwrap();
    let titles = ["Раз", "Два"];
    let (release, tracks) = album_of(&lib, "Обрыв", &titles);
    let r = rig(&lib, vec![Arc::new(FakeService::spotify().logged_out())], tidal_first());
    let tidal = FakeSource::new(SourceKey::Tidal);
    tidal.offers_album("Тестер", "Обрыв", &titles);
    *tidal.give.lock().unwrap() = Some(1);
    *tidal.refuse.lock().unwrap() = Some(SourceError::RateLimited { retry_after: Some(Duration::from_secs(600)) });
    r.rt.add_source(tidal.clone());
    wait_for(&r.rt, "the source", |s| s.downloads.sources.iter().any(|v| v.key == SourceKey::Tidal && v.available));

    r.rt.send(Command::DownloadRelease(release));
    wait_until("the later", || {
        lib.wishlist_for_release(release)
            .unwrap()
            .iter()
            .any(|i| i.status == WishlistStatus::New && i.next_try_at.is_some())
    });
    assert!(lib.active_source(tracks[0]).unwrap().is_some_and(|s| s.kind.is_local()), "the track that came stays");
    assert!(!lib.active_source(tracks[1]).unwrap().is_some_and(|s| s.kind.is_local()));
    let it = album_item(&lib, release);
    assert!(words(it.note.as_deref()).starts_with("Есть 1 из 2 · Tidal просит подождать · в "), "{:?}", it.note);
    assert!(it.next_try_at.unwrap() >= nikgob_library::now_ms() + 9 * 60_000, "the wait it asked for");
    wait_for(&r.rt, "one line of the album", |s| {
        s.downloads.history.iter().filter(|j| j.title == "Тестер — Обрыв").count() == 1
            && s.downloads.history.iter().any(|j| j.subtitle.starts_with("Есть 1 из 2 · Tidal просит подождать"))
    });
}

/// Tidal finds but every download answers 403: after the second it is set
/// aside, and the next search does not try it at all.
#[test]
fn a_source_refusing_again_and_again_is_set_aside() {
    let lib = Library::open_in_memory().unwrap();
    let (track, artist, title) = gruppa(&lib);
    let r = rig(&lib, vec![Arc::new(FakeService::spotify().logged_out())], tidal_first());
    let tidal = FakeSource::new(SourceKey::Tidal);
    tidal.offers(&artist, &title, 3_000, flac());
    *tidal.refuse.lock().unwrap() = Some(SourceError::Http { status: 403 });
    r.rt.add_source(tidal.clone());
    wait_for(&r.rt, "the source", |s| s.downloads.sources.iter().any(|v| v.key == SourceKey::Tidal && v.available));
    let later = || {
        lib.wishlist_for_track(track)
            .unwrap()
            .last()
            .is_some_and(|i| i.status == WishlistStatus::New && i.next_try_at.is_some())
    };

    r.rt.send(Command::Like(LikeTarget::Track(track), true));
    wait_until("the first refusal", later);
    assert_eq!(*tidal.fetched.lock().unwrap(), 1);
    let id = lib.wishlist_for_track(track).unwrap().last().unwrap().id;
    r.rt.send(Command::WishlistRetry(id));
    wait_until("the second", || *tidal.fetched.lock().unwrap() == 2 && later());
    let status = |s: &ViewState| s.downloads.sources.iter().find(|v| v.key == SourceKey::Tidal).and_then(|v| v.status.clone());
    wait_for(&r.rt, "set aside", |s| status(s).is_some_and(|t| t.starts_with("Находит, но не скачивает — пропускаю")));

    r.rt.send(Command::WishlistRetry(id));
    wait_until("searched again", || *tidal.searched.lock().unwrap() == 3 && later());
    assert_eq!(*tidal.fetched.lock().unwrap(), 2, "set aside: its offer is not tried");
    let note = words(lib.get_wishlist_item(id).unwrap().unwrap().note.as_deref());
    assert!(note.starts_with("Tidal сейчас не отдаёт этот трек · в "), "{note}");
    // Three refusals, one line: the latest.
    let lines = history(&r.rt);
    assert_eq!(lines.iter().filter(|l| l.contains("Tidal сейчас не отдаёт этот трек")).count(), 1, "{lines:?}");
}

/// A source that has no file of the song (404) is not a refusal: one try,
/// then the weekly search, no quick retries.
#[test]
fn a_miss_is_not_retried_soon() {
    let lib = Library::open_in_memory().unwrap();
    let (track, artist, title) = gruppa(&lib);
    let r = rig(&lib, vec![Arc::new(FakeService::spotify().logged_out())], tidal_first());
    let tidal = FakeSource::new(SourceKey::Tidal);
    tidal.offers(&artist, &title, 3_000, flac());
    *tidal.refuse.lock().unwrap() = Some(SourceError::Http { status: 404 });
    r.rt.add_source(tidal.clone());
    wait_for(&r.rt, "the source", |s| s.downloads.sources.iter().any(|v| v.key == SourceKey::Tidal && v.available));

    r.rt.send(Command::Like(LikeTarget::Track(track), true));
    wait_until("not found", || wish_status(&lib, track) == Some(WishlistStatus::NoTorrent));
    let item = lib.wishlist_for_track(track).unwrap().pop().unwrap();
    assert!(item.next_try_at.unwrap() > nikgob_library::now_ms() + 6 * 86_400_000, "weekly");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!((*tidal.searched.lock().unwrap(), *tidal.fetched.lock().unwrap()), (1, 1));
    let status = r.rt.state().downloads.sources.iter().find(|v| v.key == SourceKey::Tidal).and_then(|v| v.status.clone());
    assert_eq!(status.as_deref(), Some("Находит, но не скачивает"));
    assert!(!history(&r.rt).iter().any(|l| l.contains("не отдаёт")));
}

/// A file of `track` in `dir` the library plays: `codec`, ranked `quality`.
fn own_file(lib: &Library, dir: &Path, track: TrackId, codec: Codec, quality: i32) -> nikgob_core::model::Source {
    let path = dir.join(format!("own.{}", codec.as_str()));
    write_wav(&path, 3.0);
    let mut s = nikgob_core::model::Source::new(track, nikgob_core::model::SourceKind::LocalFile, path.to_string_lossy(), codec);
    s.quality_score = quality;
    let s = lib.add_source(&s).unwrap();
    lib.set_active_source(track).unwrap();
    s
}

/// «Найти качество лучше…» on an MP3 128: the sources find a FLAC, and it
/// waits in «Подтверди» even though found copies may replace files by
/// themselves; nothing downloads and the file stays.
#[test]
fn a_better_copy_asked_for_by_hand_waits_for_the_users_word() {
    let lib = Library::open_in_memory().unwrap();
    let (track, artist, title) = gruppa(&lib);
    let prefs = Prefs { upgrade_auto: true, ..Prefs::default() };
    let r = rig(&lib, vec![Arc::new(FakeService::spotify().logged_out())], prefs);
    let mp3 = own_file(&lib, r._dir.path(), track, Codec::Mp3, 500);
    let qobuz = FakeSource::new(SourceKey::Qobuz);
    qobuz.offers(&artist, &title, 3_000, flac());
    r.rt.add_source(qobuz.clone());
    wait_for(&r.rt, "the source", |s| s.downloads.sources.iter().any(|v| v.available));

    r.rt.send(Command::FindBetter(track));
    wait_for(&r.rt, "the toast", |s| s.toasts.iter().any(|t| t.text.starts_with("Ищу качество лучше")));
    wait_until("the copy in «Подтверди»", || wish_status(&lib, track) == Some(WishlistStatus::Confirm));
    let item = lib.wishlist_for_track(track).unwrap().pop().unwrap();
    assert_eq!(item.origin, nikgob_core::model::WishlistOrigin::Upgrade);
    assert_eq!(item.better_than, Some(500));
    assert!(item.candidates_json.as_deref().is_some_and(|j| j.contains("qobuz-1")));
    assert_eq!(*qobuz.fetched.lock().unwrap(), 0, "nothing downloads before the user's word");
    assert_eq!(lib.active_source(track).unwrap().unwrap().id, mp3.id, "the file stays");
}

/// Nothing better than a CD FLAC: the click is answered, and the item
/// looks on as any other (here: never, the file is good enough).
#[test]
fn a_better_copy_not_found_says_so() {
    let lib = Library::open_in_memory().unwrap();
    let (track, artist, title) = gruppa(&lib);
    let r = rig(&lib, vec![Arc::new(FakeService::spotify().logged_out())], Prefs::default());
    let cd = nikgob_core::model::quality_score(&Codec::Flac, None, Some(44_100), Some(16), false);
    own_file(&lib, r._dir.path(), track, Codec::Flac, cd);
    let tidal = FakeSource::new(SourceKey::Tidal);
    tidal.offers(&artist, &title, 3_000, flac());
    r.rt.add_source(tidal.clone());
    wait_for(&r.rt, "the source", |s| s.downloads.sources.iter().any(|v| v.available));

    r.rt.send(Command::FindBetter(track));
    wait_for(&r.rt, "the answer", |s| s.toasts.iter().any(|t| t.text.starts_with("Лучше не нашёл")));
    let item = lib.wishlist_for_track(track).unwrap().pop().unwrap();
    assert_eq!(item.status, WishlistStatus::NoTorrent);
    assert_eq!(item.better_than, None, "asked once");
    assert!(*tidal.searched.lock().unwrap() >= 1);
    assert_eq!(*tidal.fetched.lock().unwrap(), 0);
}

/// The «Подтверди» card of a better copy, once it is shown.
fn upgrade_card(rt: &Runtime) -> u64 {
    wait_for(rt, "the card", |s| s.downloads.confirm.iter().any(|c| c.origin == "Качество лучше · трек"));
    rt.state().downloads.confirm.iter().find(|c| c.origin == "Качество лучше · трек").unwrap().id
}

/// A track with an MP3 128 whose better copy, a FLAC from Qobuz, waits in
/// «Подтверди» after «Найти качество лучше…».
fn asked_and_found(lib: &Library) -> (Rig, TrackId, Arc<FakeSource>) {
    let (track, artist, title) = gruppa(lib);
    let r = rig(lib, vec![Arc::new(FakeService::spotify().logged_out())], Prefs::default());
    own_file(lib, r._dir.path(), track, Codec::Mp3, 500);
    let qobuz = FakeSource::new(SourceKey::Qobuz);
    qobuz.offers(&artist, &title, 3_000, flac());
    r.rt.add_source(qobuz.clone());
    wait_for(&r.rt, "the source", |s| s.downloads.sources.iter().any(|v| v.available));
    r.rt.send(Command::FindBetter(track));
    wait_until("the copy in «Подтверди»", || wish_status(lib, track) == Some(WishlistStatus::Confirm));
    (r, track, qobuz)
}

/// The copy the user picked does not come: no «nothing better» is said,
/// and the item keeps asking before it replaces anything.
#[test]
fn a_picked_copy_that_fails_to_come_is_no_nothing_found() {
    let lib = Library::open_in_memory().unwrap();
    let (r, track, qobuz) = asked_and_found(&lib);
    *qobuz.refuse.lock().unwrap() = Some(SourceError::Http { status: 404 });
    let id = upgrade_card(&r.rt);
    r.rt.send(Command::ConfirmChoice { id, option: Some(0) });
    wait_until("the download tried", || *qobuz.fetched.lock().unwrap() == 1);
    wait_until("the item waits", || wish_status(&lib, track) == Some(WishlistStatus::NoTorrent));
    let item = lib.wishlist_for_track(track).unwrap().pop().unwrap();
    assert_eq!(item.better_than, Some(500), "still asks");
    assert!(!r.rt.state().toasts.iter().any(|t| t.text.starts_with("Лучше не нашёл")));
}

/// «Оставить как есть» on a copy asked for by hand: turned down, never
/// handed to the schedule.
#[test]
fn keeping_the_file_turns_a_hand_asked_copy_down() {
    let lib = Library::open_in_memory().unwrap();
    let (r, track, qobuz) = asked_and_found(&lib);
    let id = upgrade_card(&r.rt);
    r.rt.send(Command::ConfirmChoice { id, option: None });
    wait_until("turned down", || wish_status(&lib, track) == Some(WishlistStatus::Dismissed));
    let item = lib.wishlist_for_track(track).unwrap().pop().unwrap();
    assert_eq!(item.better_than, Some(500));
    assert_eq!(item.next_try_at, None);
    assert_eq!(*qobuz.fetched.lock().unwrap(), 0);
}

/// Anything of the track already on its way (here a plain download of
/// another kind): no second search, whatever the file is.
#[test]
fn a_track_on_its_way_is_not_looked_for_twice() {
    for quality in [500, score::SAVED_STREAM] {
        let lib = Library::open_in_memory().unwrap();
        let (track, artist, title) = gruppa(&lib);
        let r = rig(&lib, vec![Arc::new(FakeService::spotify().logged_out())], Prefs::default());
        own_file(&lib, r._dir.path(), track, Codec::Mp3, quality);
        let qobuz = FakeSource::new(SourceKey::Qobuz);
        qobuz.offers(&artist, &title, 3_000, flac());
        r.rt.add_source(qobuz.clone());
        wait_for(&r.rt, "the source", |s| s.downloads.sources.iter().any(|v| v.available));
        let mut busy = nikgob_core::model::WishlistItem::new(WishlistStatus::Downloading, nikgob_library::now_ms());
        busy.track_id = Some(track);
        busy.title = Some(title.clone());
        busy.origin = nikgob_core::model::WishlistOrigin::Like;
        lib.put_wishlist_item(&busy).unwrap();

        r.rt.send(Command::FindBetter(track));
        wait_for(&r.rt, "the answer", |s| s.toasts.iter().any(|t| t.text == "Уже в «Хочу скачать»"));
        let items = lib.wishlist_for_track(track).unwrap();
        assert_eq!(items.len(), 1, "{quality}: no item of its own");
        assert_eq!(items[0].status, WishlistStatus::Downloading, "{quality}: not restarted");
        assert_eq!(*qobuz.searched.lock().unwrap(), 0, "{quality}");
    }
}

/// Our Opus of a genuine FLAC (ranked as the FLAC): nothing is better.
/// The click is answered at once, no source is asked, and the track
/// leaves the search for better copies (an item the schedule made goes).
#[test]
fn our_opus_of_a_flac_gets_nothing_better() {
    let lib = Library::open_in_memory().unwrap();
    let (track, artist, title) = gruppa(&lib);
    let r = rig(&lib, vec![Arc::new(FakeService::spotify().logged_out())], Prefs::default());
    own_file(&lib, r._dir.path(), track, Codec::Opus, score::CD_LOSSLESS);
    let qobuz = FakeSource::new(SourceKey::Qobuz);
    qobuz.offers(&artist, &title, 3_000, flac());
    r.rt.add_source(qobuz.clone());
    wait_for(&r.rt, "the source", |s| s.downloads.sources.iter().any(|v| v.available));
    let mut old = nikgob_core::model::WishlistItem::new(WishlistStatus::NoTorrent, nikgob_library::now_ms());
    old.track_id = Some(track);
    old.title = Some(title.clone());
    old.scope = nikgob_core::model::WishlistScope::Track;
    old.origin = nikgob_core::model::WishlistOrigin::Upgrade;
    lib.put_wishlist_item(&old).unwrap();

    r.rt.send(Command::FindBetter(track));
    wait_for(&r.rt, "the answer", |s| s.toasts.iter().any(|t| t.text.starts_with("Лучше не нашёл")));
    assert!(lib.wishlist_for_track(track).unwrap().is_empty(), "out of the search for better copies");
    assert_eq!(*qobuz.searched.lock().unwrap(), 0);
    assert_eq!(*qobuz.fetched.lock().unwrap(), 0);
}

/// «Скачать» on a track not in the library, then cancelled («Не качать»
/// on its row, as a cancelled torrent or import does): it leaves «Хочу
/// скачать» and the library has no track of it; «Отменить» asks again.
#[test]
fn a_cancelled_download_of_a_click_leaves_the_wishlist() {
    let lib = Library::open_in_memory().unwrap();
    let (track, ..) = gruppa(&lib);
    let r = rig(&lib, vec![Arc::new(FakeService::spotify().logged_out())], Prefs::default());
    assert_eq!(lib.track_in_library(track).unwrap(), Some(false));
    r.rt.send(Command::DownloadTracks(vec![track]));
    wait_until("the item", || wish_status(&lib, track).is_some());
    let id = lib.wishlist_for_track(track).unwrap().pop().unwrap().id;

    r.rt.send(Command::WishlistDismiss(vec![id]));
    wait_until("the item gone", || lib.wishlist_for_track(track).unwrap().is_empty());
    wait_for(&r.rt, "out of the list", |s| !s.downloads.wishlist.items.iter().any(|i| i.id == id));
    assert_eq!(lib.track_in_library(track).unwrap(), Some(false), "no track of it in the library");
    let undo = r.rt.state().toasts.iter().find(|t| t.text.starts_with("Не буду качать")).and_then(|t| t.action.clone());
    assert!(matches!(undo, Some((_, Command::DownloadTracks(ref t))) if t == &vec![track]), "{undo:?}");
}

/// «Не качать ничего отсюда» on a group of a «Скачать» click and a like's
/// item: one «Отменить» brings both back (the like's item as it was, the
/// click asked again).
#[test]
fn a_mixed_dismissal_is_taken_back_at_once() {
    use nikgob_core::model::{WishlistItem, WishlistOrigin, WishlistScope};
    let lib = Library::open_in_memory().unwrap();
    let (track, ..) = gruppa(&lib);
    let r = rig(&lib, vec![Arc::new(FakeService::spotify().logged_out())], Prefs::default());
    r.rt.send(Command::DownloadTracks(vec![track]));
    wait_until("the item", || wish_status(&lib, track).is_some());
    let click = lib.wishlist_for_track(track).unwrap().pop().unwrap().id;
    let mut liked = WishlistItem::new(WishlistStatus::Waiting, nikgob_library::now_ms());
    liked.scope = WishlistScope::Track;
    liked.origin = WishlistOrigin::Like;
    liked.title = Some("Звезда по имени Солнце".into());
    lib.put_wishlist_item(&liked).unwrap();

    r.rt.send(Command::WishlistDismiss(vec![click, liked.id]));
    wait_until("the click gone", || lib.wishlist_for_track(track).unwrap().is_empty());
    wait_until("the like turned down", || {
        lib.get_wishlist_item(liked.id).unwrap().is_some_and(|i| i.status == WishlistStatus::Dismissed)
    });
    wait_for(&r.rt, "the toast", |s| s.toasts.iter().any(|t| t.text == "Не буду качать: 2"));
    let undo = r.rt.state().toasts.iter().find(|t| t.text == "Не буду качать: 2").and_then(|t| t.action.clone());
    let Some((_, cmd)) = undo else { panic!("no «Отменить»") };
    assert_eq!(cmd, Command::WishlistUndoDismiss { restore: vec![liked.id], tracks: vec![track], albums: Vec::new() });
    r.rt.send(cmd);
    wait_until("the click asked again", || wish_status(&lib, track).is_some());
    wait_until("the like back", || {
        lib.get_wishlist_item(liked.id).unwrap().is_some_and(|i| i.status != WishlistStatus::Dismissed)
    });
}
