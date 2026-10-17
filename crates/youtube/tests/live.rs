//! Against the real YouTube Music, no login: `cargo test -p nikgob-youtube
//! --test live -- --ignored --nocapture`.

use std::io::{Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_audio::{ByteSource, OfflineOptions, PlayItem, Player};
use nikgob_core::streaming::{AudioStream, StreamService, TrackKind};
use nikgob_youtube::{Youtube, YoutubeConfig};

/// An [`AudioStream`] as the engine's [`ByteSource`], noting when the first
/// byte arrived.
struct Bytes {
    inner: Box<dyn AudioStream>,
    opened: Instant,
    first_byte: Arc<Mutex<Option<Duration>>>,
}

impl Read for Bytes {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        let mut first = self.first_byte.lock().unwrap();
        if first.is_none() && n > 0 {
            *first = Some(self.opened.elapsed());
        }
        Ok(n)
    }
}

impl Seek for Bytes {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

impl ByteSource for Bytes {
    fn byte_len(&self) -> Option<u64> {
        self.inner.byte_len()
    }
    fn format_hint(&self) -> &str {
        self.inner.format_hint()
    }
    fn gain_db(&self) -> Option<f32> {
        self.inner.gain_db()
    }
    fn duration(&self) -> Option<Duration> {
        self.inner.duration()
    }
}

fn rms(s: &[f32]) -> f32 {
    (s.iter().map(|x| x * x).sum::<f32>() / s.len().max(1) as f32).sqrt()
}

#[test]
#[ignore = "network"]
fn live_smoke() {
    let dir = std::env::temp_dir().join(format!("nikgob-yt-live-{}", std::process::id()));
    let yt = Youtube::new(YoutubeConfig::new(dir.join("data"))).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let omori = rt.block_on(yt.search("OMORI soundtrack")).unwrap();
    println!(
        "search OMORI: {} tracks, {} albums, {} artists, {} playlists; first track {:?} by {:?} ({:?})",
        omori.tracks.len(),
        omori.albums.len(),
        omori.artists.len(),
        omori.playlists.len(),
        omori.tracks.first().map(|t| &t.title),
        omori.tracks.first().map(|t| t.artist_line()),
        omori.tracks.first().map(|t| t.kind),
    );
    assert!(!omori.tracks.is_empty() && !omori.albums.is_empty());

    let kino = rt.block_on(yt.search("Кино Группа крови")).unwrap();
    println!(
        "search Кино: {} tracks ({} songs), {} albums, {} artists, {} playlists",
        kino.tracks.len(),
        kino.tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Song)
            .count(),
        kino.albums.len(),
        kino.artists.len(),
        kino.playlists.len()
    );
    let song = kino
        .tracks
        .iter()
        .find(|t| t.kind == TrackKind::Song)
        .expect("a song")
        .clone();
    println!(
        "  song {} — {} [{:?}] album {:?} {:?} ms",
        song.id.id,
        song.title,
        song.artist_line(),
        song.album.as_ref().map(|a| &a.title),
        song.duration_ms
    );

    let album_id = kino.albums[0].id.id.clone();
    let album = rt.block_on(yt.album(&album_id)).unwrap();
    println!(
        "album {} ({:?}, {:?}): {} tracks, first {:?} #{:?}",
        album.title,
        album.year,
        album.release_type,
        album.tracks.len(),
        album.tracks.first().map(|t| &t.title),
        album.tracks.first().and_then(|t| t.track_no)
    );
    assert!(!album.tracks.is_empty());

    let artist_id = kino.artists[0].id.id.clone();
    let artist = rt.block_on(yt.artist(&artist_id)).unwrap();
    println!(
        "artist {}: {} releases, newest {:?} ({:?})",
        artist.name,
        artist.albums.len(),
        artist.albums.first().map(|a| &a.title),
        artist.albums.first().and_then(|a| a.year)
    );
    assert!(!artist.albums.is_empty());

    if let Some(p) = omori.playlists.first() {
        let pl = rt.block_on(yt.playlist(&p.id.id)).unwrap();
        println!(
            "playlist {} by {:?}: {} tracks (total {:?})",
            pl.name,
            pl.owner,
            pl.tracks.len(),
            pl.total_tracks
        );
    }

    let isrc = rt.block_on(yt.find_isrc("GBAYE0601498")).unwrap();
    println!("isrc GBAYE0601498 -> {:?}", isrc.map(|t| t.title));

    // Stream: decode 3 s through the engine's stream path.
    let (player, events, mut device) = Player::offline(OfflineOptions::default()).unwrap();
    let first_byte = Arc::new(Mutex::new(None));
    let (yt2, id, fb) = (Arc::clone(&yt), song.id.id.clone(), Arc::clone(&first_byte));
    let started = Instant::now();
    player.play(PlayItem::stream(
        1,
        Box::new(move || {
            let opened = Instant::now();
            let inner = yt2.open_audio(&id).map_err(|e| e.to_string())?;
            Ok(Box::new(Bytes {
                inner,
                opened,
                first_byte: fb,
            }) as Box<dyn ByteSource>)
        }),
    ));
    // The engine renders silence until the stream is open.
    let mut waited = 0;
    while !events
        .try_iter()
        .any(|e| matches!(e, nikgob_audio::Event::Started { .. }))
    {
        assert!(waited < 48_000 * 30, "the stream did not start");
        device.render(4800);
        waited += 4800;
        std::thread::sleep(Duration::from_millis(20));
    }
    let open_time = started.elapsed();
    let audio = device.render(48_000 * 6);
    let elapsed = started.elapsed();
    let blocks: Vec<String> = audio
        .chunks(48_000)
        .map(|b| format!("{:.3}", rms(b)))
        .collect();
    println!("  RMS per 0.5 s: {}", blocks.join(" "));
    let level = rms(&audio[audio.len() / 2..]);
    println!(
        "stream {}: first byte after {:?}, playing after {:?}, 6 s rendered by {:?}, RMS {:.3}",
        song.id.id,
        first_byte.lock().unwrap(),
        open_time,
        elapsed,
        level
    );
    let peak = audio.iter().fold(0f32, |m, x| m.max(x.abs()));
    assert!(peak > 0.05, "the stream is silent (peak {peak})");
    // Far past the first megabyte (where PO-token-less URLs used to stop).
    let t = Instant::now();
    player.seek(Duration::from_secs(200));
    device.render(48_000 / 2);
    let later = device.render(48_000 * 2);
    let later_rms = rms(&later);
    println!(
        "  seek to 200 s: RMS {later_rms:.3} after {:?}",
        t.elapsed()
    );
    assert!(later_rms > 0.02, "silent after the seek");
    drop(player);

    // Save a short track and decode the file.
    let short = album
        .tracks
        .iter()
        .chain(&kino.tracks)
        .filter(|t| t.kind == TrackKind::Song && t.duration_ms.is_some())
        .min_by_key(|t| t.duration_ms)
        .unwrap()
        .clone();
    println!("saving {} ({:?} ms)", short.id.id, short.duration_ms);
    let t = Instant::now();
    let path = rt
        .block_on(yt.save_audio(&short.id.id, &dir.join("saved")))
        .unwrap();
    let size = std::fs::metadata(&path).unwrap().len();
    let (player, _events, mut device) = Player::offline(OfflineOptions::default()).unwrap();
    player.play(PlayItem::file(1, &path));
    let frames = device.drain(48_000 * 900).len() / 2;
    let secs = frames as f64 / 48_000.0;
    println!(
        "saved {} ({} bytes) in {:?}: decodes to {secs:.2} s, listed {:?} ms",
        path.display(),
        size,
        t.elapsed(),
        short.duration_ms
    );
    // The remux keeps the length of the WebM it came from.
    let mut raw = Vec::new();
    yt.open_audio(&short.id.id)
        .unwrap()
        .read_to_end(&mut raw)
        .unwrap();
    let raw_path = dir.join("raw.webm");
    std::fs::write(&raw_path, &raw).unwrap();
    let (player2, _events2, mut device2) = Player::offline(OfflineOptions::default()).unwrap();
    player2.play(PlayItem::file(1, &raw_path));
    let raw_frames = device2.drain(48_000 * 900).len() / 2;
    drop(player2);
    println!(
        "  the WebM itself decodes to {:.2} s",
        raw_frames as f64 / 48_000.0
    );
    assert!(
        raw_frames.abs_diff(frames) <= 960,
        "{raw_frames} vs {frames} frames"
    );
    let listed = short.duration_ms.unwrap() as f64 / 1000.0;
    assert!((secs - listed).abs() < 2.0, "{secs} s vs {listed} s");

    drop(player);
    drop(yt);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// How long a search takes, cold and warm: `cargo test -p nikgob-youtube
/// --test live live_search_timing -- --ignored --nocapture`.
#[test]
#[ignore = "network"]
fn live_search_timing() {
    let _ = env_logger::builder().is_test(true).filter_level(log::LevelFilter::Debug).try_init();
    let dir = std::env::temp_dir().join(format!("nikgob-yt-timing-{}", std::process::id()));
    let yt = Youtube::new(YoutubeConfig::new(dir.join("data"))).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    if std::env::var_os("WARM").is_some() {
        yt.warm_up();
        std::thread::sleep(Duration::from_secs(3));
    }
    for q in ["лёд", "Кино Группа крови", "OMORI soundtrack", "лёд"] {
        let t = Instant::now();
        let first = Arc::new(Mutex::new(None));
        let sink = {
            let first = first.clone();
            Arc::new(move |_: nikgob_core::streaming::StreamSearch| {
                first.lock().unwrap().get_or_insert(t.elapsed());
            })
        };
        let r = rt.block_on(yt.search_in_parts(q, sink)).unwrap();
        println!(
            "search «{q}»: first part {:?}, all {} ms ({} tracks, {} albums, {} artists, {} playlists)",
            first.lock().unwrap().map(|d| d.as_millis()),
            t.elapsed().as_millis(),
            r.tracks.len(),
            r.albums.len(),
            r.artists.len(),
            r.playlists.len()
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// Opens the first songs of a search and reports what fails:
/// `QUERY="MORGENSHTERN ARISTOCRAT" cargo test -p nikgob-youtube --test live
/// live_open_search -- --ignored --nocapture`.
#[test]
#[ignore = "network"]
fn live_open_search() {
    let _ = env_logger::builder().is_test(true).filter_module("nikgob_youtube", log::LevelFilter::Debug).try_init();
    let q = std::env::var("QUERY").unwrap_or_else(|_| "MORGENSHTERN ARISTOCRAT".into());
    let dir = std::env::temp_dir().join(format!("nikgob-yt-open-{}", std::process::id()));
    let yt = Youtube::new(YoutubeConfig::new(dir.join("data"))).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let found = rt.block_on(yt.search(&q)).unwrap();
    let tracks = if std::env::var_os("ALBUM").is_some() {
        let a = &found.albums[0];
        println!("album {} — {}", a.title, a.id.id);
        rt.block_on(yt.album(&a.id.id)).unwrap().tracks
    } else {
        found.tracks.iter().take(6).cloned().collect()
    };
    for t in &tracks {
        let mut s = yt.open_audio(&t.id.id).unwrap();
        let mut head = [0u8; 16];
        let started = Instant::now();
        let r = s.read_exact(&mut head);
        println!(
            "{} {:?} «{}» — {} ({:?} ms): {:?}",
            t.id.id,
            t.kind,
            t.title,
            t.artist_line(),
            t.duration_ms,
            r.map(|()| format!("ok in {} ms, {:?}", started.elapsed().as_millis(), s.quality()))
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}
