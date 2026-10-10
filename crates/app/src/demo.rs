//! `NIKGOB_DEMO=1`: a throwaway library with generated covers, for looking
//! at the interface without importing music. Files do not exist; playback
//! goes to the null backend.

use std::path::Path;

use nikgob_core::model::{
    Artist, ArtistCredit, Codec, EventContext, EventType, Release, ReleaseType, Source, SourceKind,
    Track,
};
use nikgob_library::Library;

const ALBUMS: &[(&str, &str, i32, ReleaseType, [u8; 3], &[&str])] = &[
    ("Кино", "Группа крови", 1988, ReleaseType::Album, [196, 40, 40], &["Группа крови", "Закрой за мной дверь", "Война", "Спокойная ночь", "Мама, мы все тяжело больны", "Бошетунмай", "Легенда", "Попробуй спеть вместе со мной"]),
    ("Кино", "Звезда по имени Солнце", 1989, ReleaseType::Album, [235, 170, 40], &["Звезда по имени Солнце", "Невеселая песня", "Место для шага вперёд", "Печаль", "Пачка сигарет", "Апрель"]),
    ("Radiohead", "In Rainbows", 2007, ReleaseType::Album, [40, 120, 220], &["15 Step", "Bodysnatchers", "Nude", "Weird Fishes/Arpeggi", "All I Need", "Faust Arp", "Reckoner", "House of Cards", "Jigsaw Falling Into Place", "Videotape"]),
    ("Daft Punk", "Discovery", 2001, ReleaseType::Album, [150, 60, 200], &["One More Time", "Aerodynamic", "Digital Love", "Harder, Better, Faster, Stronger", "Crescendolls", "Nightvision", "Something About Us", "Voyager"]),
    ("Земфира", "Земфира", 1999, ReleaseType::Album, [40, 170, 120], &["Снег", "Ромашки", "Скандал", "Почему", "Ариведерчи", "СПИД", "Синоптик", "Румба"]),
    ("Aphex Twin", "Selected Ambient Works 85-92", 1992, ReleaseType::Album, [120, 120, 120], &["Xtal", "Tha", "Pulsewidth", "Ageispolis", "i", "Green Calx", "Heliosphan"]),
    ("Molchat Doma", "Этажи", 2018, ReleaseType::Album, [30, 30, 60], &["Тоска", "Клетка", "Судно (Борис Рыжий)", "Танцевать", "Discoteque", "Звезды"]),
    ("Massive Attack", "Mezzanine", 1998, ReleaseType::Album, [200, 200, 210], &["Angel", "Risingson", "Teardrop", "Inertia Creeps", "Exchange", "Dissolved Girl", "Man Next Door"]),
    ("Boards of Canada", "Dayvan Cowboy", 2006, ReleaseType::Ep, [240, 120, 60], &["Dayvan Cowboy", "Ataronchronon", "Dayvan Cowboy (Odd Nosdam Remix)"]),
];

/// A cover: abstract art from the hue (sun over stripes, diagonal bands,
/// soft blobs or a dot grid, by `seed`) with a little grain, 600×600 JPEG.
fn make_cover(path: &Path, rgb: [u8; 3], seed: u32) {
    let size = 600u32;
    let base = rgb.map(f32::from);
    let mix = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] {
        [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
    };
    let light = mix(base, [255.0; 3], 0.55);
    let dark = mix(base, [0.0; 3], 0.65);
    let other = mix([255.0 - base[0], 255.0 - base[2], base[1]], base, 0.35);
    let img = image::RgbImage::from_fn(size, size, |x, y| {
        let (u, v) = (x as f32 / size as f32, y as f32 / size as f32);
        let c = match seed % 4 {
            // Sun over a striped horizon.
            0 => {
                let sky = mix(light, base, v);
                let d = ((u - 0.5).powi(2) + (v - 0.55).powi(2)).sqrt();
                let sun = (d < 0.26) && !(v > 0.55 && ((v - 0.55) * 40.0).fract() < 0.45);
                if v > 0.72 { mix(base, dark, (v - 0.72) / 0.28) } else if sun { other } else { sky }
            }
            // Diagonal bands.
            1 => {
                let band = ((u * 0.8 + v) * 5.0).floor() as i32 % 3;
                [base, light, dark][band as usize]
            }
            // Soft blobs on a dark ground.
            2 => {
                let blob = |cx: f32, cy: f32, r: f32| (-((u - cx).powi(2) + (v - cy).powi(2)) / (r * r)).exp();
                let c = mix(dark, base, blob(0.3, 0.35, 0.3));
                let c = mix(c, other, blob(0.72, 0.6, 0.22) * 0.85);
                mix(c, light, blob(0.45, 0.8, 0.14) * 0.7)
            }
            // Dot grid fading into the hue.
            _ => {
                let (gx, gy) = ((u * 10.0).fract() - 0.5, (v * 10.0).fract() - 0.5);
                let r = 0.12 + 0.28 * u * v;
                let ground = mix(dark, base, (u + v) * 0.5);
                if gx * gx + gy * gy < r * r { light } else { ground }
            }
        };
        // Grain.
        let h = (x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663) ^ seed.wrapping_mul(83_492_791)) % 1000;
        let n = (h as f32 / 1000.0 - 0.5) * 14.0;
        image::Rgb(c.map(|ch| (ch + n).clamp(0.0, 255.0) as u8))
    });
    let _ = img.save(path);
    // Ambient: 16×16 downscale next to it.
    let small = image::imageops::resize(&img, 16, 16, image::imageops::FilterType::Triangle);
    let _ = small.save(path.with_file_name("ambient.png"));
}

/// Marker file of the current seed; bump to reseed old demo libraries.
pub const VERSION: &str = "seed-v2";

pub fn seed(lib: &Library, root: &Path) {
    if lib.count_releases().unwrap_or(0) > 0 {
        // Existing demo library: refresh the generated covers only.
        for (i, (artist, album, year, _, rgb, _)) in ALBUMS.iter().enumerate() {
            let dir = root.join(artist).join(format!("{year} - {album}"));
            if dir.is_dir() {
                make_cover(&dir.join("cover.jpg"), *rgb, i as u32);
            }
        }
        return;
    }
    let mut first_tracks = Vec::new();
    // One row per artist: upsert matches by id / MBID, not by name.
    let mut artists = std::collections::HashMap::new();
    for (i, (artist, album, year, kind, rgb, tracks)) in ALBUMS.iter().enumerate() {
        let a = artists
            .entry(*artist)
            .or_insert_with(|| lib.upsert_artist(&Artist::new(*artist)).expect("artist"))
            .clone();
        let credit = ArtistCredit::single(*artist, Some(a.id));
        let dir = root.join(artist).join(format!("{year} - {album}"));
        let _ = std::fs::create_dir_all(&dir);
        let cover = dir.join("cover.jpg");
        make_cover(&cover, *rgb, i as u32);
        let mut r = Release::new(*album, credit.clone());
        r.year = Some(*year);
        r.release_type = *kind;
        r.cover_ref = Some(cover.display().to_string());
        r.total_tracks = Some(tracks.len() as u32 + u32::from(i % 3 == 0) * 2);
        let r = lib.upsert_release(&r).expect("release");
        if let Ok(img) = image::open(&cover) {
            let rgba = img.to_rgba8();
            if let Some(seed) = nikgob_md3e::seed_from_image(rgba.as_raw(), rgba.width() as usize, rgba.height() as usize) {
                let _ = lib.put_palette(&nikgob_core::model::Palette::new(r.id, seed));
            }
        }
        for (n, title) in tracks.iter().enumerate() {
            let mut t = Track::new(*title, credit.clone());
            t.release_id = Some(r.id);
            t.track_no = n as u32 + 1;
            t.duration_ms = Some(6_000);
            let t = lib.upsert_track(&t).expect("track");
            let codec = Codec::Wav;
            let path = dir.join(format!("{:02} - {title}.wav", n + 1));
            write_tone(&path, 220.0 * 2f32.powf((i * 3 + n) as f32 / 12.0), 6.0);
            let mut s = Source::new(t.id, SourceKind::LocalFile, path.display().to_string(), codec);
            s.bitrate = Some(705);
            s.sample_rate = Some(44_100);
            s.bit_depth = Some(16);
            let _ = lib.add_source(&s);
            if n == 0 {
                first_tracks.push(t.id);
            }
            if (n + i) % 4 == 0 {
                let _ = lib.like(t.id);
            }
            for _ in 0..((n * 7 + i * 3) % 5) {
                let _ = lib.record_event(t.id, EventType::Start, None, Some(0), EventContext::Album);
                let _ = lib.record_event(t.id, EventType::Complete, None, None, EventContext::Album);
            }
        }
    }
    if let Ok(p) = lib.create_playlist("Вечером") {
        let _ = lib.playlist_add_many(p.id, &first_tracks);
    }
    let _ = lib.create_playlist("В дорогу");
}

/// A short mono sine WAV so demo tracks really play (gapless joins audible).
fn write_tone(path: &Path, freq: f32, seconds: f32) {
    let rate = 44_100u32;
    let n = (rate as f32 * seconds) as u32;
    let mut data = Vec::with_capacity(44 + n as usize * 2);
    let push32 = |d: &mut Vec<u8>, v: u32| d.extend_from_slice(&v.to_le_bytes());
    let push16 = |d: &mut Vec<u8>, v: u16| d.extend_from_slice(&v.to_le_bytes());
    data.extend_from_slice(b"RIFF");
    push32(&mut data, 36 + n * 2);
    data.extend_from_slice(b"WAVEfmt ");
    push32(&mut data, 16);
    push16(&mut data, 1);
    push16(&mut data, 1);
    push32(&mut data, rate);
    push32(&mut data, rate * 2);
    push16(&mut data, 2);
    push16(&mut data, 16);
    data.extend_from_slice(b"data");
    push32(&mut data, n * 2);
    for i in 0..n {
        let t = i as f32 / rate as f32;
        // A soft chord pulsing at 2 Hz, so the bass glow has something to follow.
        let env = 0.55 + 0.45 * (std::f32::consts::TAU * 2.0 * t).sin().max(0.0);
        let s = 0.18 * env
            * ((std::f32::consts::TAU * freq * t).sin()
                + 0.6 * (std::f32::consts::TAU * freq * 1.5 * t).sin()
                + 0.8 * (std::f32::consts::TAU * 55.0 * t).sin());
        push16(&mut data, (s.clamp(-1.0, 1.0) * 32767.0) as i16 as u16);
    }
    let _ = std::fs::write(path, data);
}
