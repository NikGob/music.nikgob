//! `NIKGOB_DEMO=1`: a throwaway library with generated covers, for looking
//! at the interface without importing music. Files do not exist; playback
//! goes to the null backend.
//!
//! For the recommendations (docs/stage4.md) every track gets a synthetic
//! sound vector and the library a listening history of three months: the
//! albums fall into three styles (Russian rock, electronic, dark and
//! dreamy), tracks within a style spread over energy and brightness so
//! every mood has its own, and plays lean to the evening and the night
//! with a few early skips. The home then shows «Твой микс», six moods,
//! three style mixes, and (from the fake services) discoveries and new
//! releases.

use std::path::Path;

use nikgob_core::model::{
    Analysis, AnalysisSource, Artist, ArtistCredit, Codec, Event, EventContext, EventType, Release, ReleaseType,
    Source, SourceKind, Track, TrackId,
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
    ("RADWIMPS", "すずめの戸締まり", 2022, ReleaseType::Album, [60, 140, 220], &["すずめ feat. 十明", "カナタハルカ", "Tamaki", "ダイジンの歌"]),
    ("Boards of Canada", "Dayvan Cowboy", 2006, ReleaseType::Ep, [240, 120, 60], &["Dayvan Cowboy", "Ataronchronon", "Dayvan Cowboy (Odd Nosdam Remix)"]),
    ("Мумий Тролль", "Морская", 1997, ReleaseType::Album, [60, 180, 200], &["Утекай", "Владивосток 2000", "Девочка", "Кот кота", "Забавы", "Скорость", "Роза Белая", "Сантименты", "Новая луна апреля", "Делай меня точно"]),
    ("The Prodigy", "The Fat of the Land", 1997, ReleaseType::Album, [220, 60, 30], &["Smack My Bitch Up", "Breathe", "Diesel Power", "Funky Shit", "Serial Thrilla", "Mindfields", "Narayan", "Firestarter", "Climbatize", "Fuel My Fire"]),
];

/// The style of each album (index in [`ALBUMS`]): 0 Russian rock, 1
/// electronic, 2 dark and dreamy.
const STYLES: &[usize] = &[0, 0, 2, 1, 0, 1, 2, 2, 2, 1, 0, 1];
/// How much each style is listened to (a day's session is skipped above it).
const STYLE_WEIGHT: [f32; 3] = [1.0, 0.7, 0.55];
/// Days of history.
const HISTORY_DAYS: i64 = 90;
const DAY_MS: i64 = 24 * 3600 * 1000;

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
pub const VERSION: &str = "seed-v11";

/// The intro, instrumental gap and long line exercise both lyrics layouts.
const DEMO_LRC: &str = "[00:03.00]Свет из окна ложится на пол
[00:05.00]Город ещё не проснулся
[00:12.80]и слова, что не сказаны вслух, звучат громче всего остального, пока город считает шаги до утра
[00:19.00]Мы считаем шаги до утра
[00:22.00]Тишина";

/// Syllable, word and line timing also check the sweep across forced rows, a duet and a
/// background voice which belongs to singing rather than the long gap.
const DEMO_TTML: &str = r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:itunes="http://music.apple.com/lyric-ttml-internal" xmlns:ttm="http://www.w3.org/ns/ttml#metadata" itunes:timing="Word">
<head><metadata><ttm:agent type="person" xml:id="v1"/><ttm:agent type="person" xml:id="v2"/></metadata></head>
<body dur="24.000"><div>
<p begin="3.000" end="4.600" ttm:agent="v1"><span begin="3.000" end="3.250">Свет</span> <span begin="3.260" end="3.500">из</span> <span begin="3.510" end="3.750">окна</span> <span begin="3.760" end="4.000">ложится</span> <span begin="4.010" end="4.250">на</span> <span begin="4.260" end="4.600">пол</span></p>
<p begin="5.000" end="8.600" ttm:agent="v2"><span begin="5.000" end="5.400">Го</span><span begin="5.500" end="7.200">род</span> <span begin="7.250" end="7.600">ещё</span> <span begin="7.650" end="7.950">не</span> <span begin="8.000" end="8.600">проснулся</span><span ttm:role="x-translation" xml:lang="en">The city is not awake yet</span></p>
<p begin="12.800" end="18.600" ttm:agent="v1"><span begin="12.800" end="13.060">и</span> <span begin="13.100" end="13.360">слова,</span> <span begin="13.400" end="13.660">что</span> <span begin="13.700" end="13.960">не</span> <span begin="14.000" end="14.260">сказаны</span> <span begin="14.300" end="14.560">вслух,</span> <span begin="14.600" end="14.860">звучат</span> <span begin="14.900" end="15.160">громче</span> <span begin="15.200" end="15.460">всего</span> <span begin="15.500" end="15.760">остального,</span> <span begin="15.800" end="16.060">пока</span> <span begin="16.100" end="16.360">город</span> <span begin="16.400" end="16.660">считает</span> <span begin="16.700" end="16.960">шаги</span> <span begin="17.000" end="17.260">до</span> <span begin="17.300" end="17.560">утра</span></p>
<p begin="19.000" end="21.600" ttm:agent="v1"><span begin="19.000" end="19.400">Мы</span> <span begin="19.420" end="19.820">считаем</span> <span begin="19.840" end="20.240">шаги</span> <span begin="20.260" end="20.660">до</span> <span begin="20.680" end="21.600">утра</span><span ttm:role="x-bg" begin="20.800" end="21.600"><span begin="20.800" end="21.160">(до</span> <span begin="21.180" end="21.600">утра)</span></span></p>
<p begin="22.000" end="23.900" ttm:agent="v1">Тишина</p>
</div></body></tt>"#;

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
    // Every track with its album's index and number, for the history.
    let mut all: Vec<(TrackId, usize, usize)> = Vec::new();
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
            // "Song feat. Guest": a track of two artists, each with a page.
            let (title, credit) = match title.split_once(" feat. ") {
                Some((song, guest)) => {
                    let g = artists
                        .entry(guest)
                        .or_insert_with(|| lib.upsert_artist(&Artist::new(guest)).expect("artist"))
                        .clone();
                    let mut both = credit.clone();
                    both.push(" feat. ", guest, Some(g.id));
                    (song, both)
                }
                None => (*title, credit.clone()),
            };
            let mut t = Track::new(title, credit);
            t.release_id = Some(r.id);
            t.track_no = n as u32 + 1;
            t.duration_ms = Some(24_000);
            let t = lib.upsert_track(&t).expect("track");
            let codec = Codec::Wav;
            let path = dir.join(format!("{:02} - {title}.wav", n + 1));
            write_tone(&path, 220.0 * 2f32.powf((i * 3 + n) as f32 / 12.0), 24.0);
            let mut s = Source::new(t.id, SourceKind::LocalFile, path.display().to_string(), codec);
            s.bitrate = Some(705);
            s.sample_rate = Some(44_100);
            s.bit_depth = Some(16);
            let _ = lib.add_source(&s);
            let _ = lib.put_lyrics(t.id, Some(DEMO_LRC), None, false);
            let _ = std::fs::write(path.with_extension("ttml"), DEMO_TTML);
            if n == 0 {
                first_tracks.push(t.id);
            }
            if (n + i) % 4 == 0 {
                let _ = lib.like(t.id);
            }
            let _ = lib.put_analysis(&Analysis {
                track_id: t.id,
                bliss_vector: vector(STYLES[i], i, n),
                analyzed_with: nikgob_recs::ANALYZED_WITH.to_owned(),
                source: AnalysisSource::File,
            });
            all.push((t.id, i, n));
        }
    }
    one_song_of_an_album(lib, root);
    history(lib, &all);
    if let Ok(p) = lib.create_playlist("Вечером") {
        let _ = lib.playlist_add_many(p.id, &first_tracks);
    }
    let _ = lib.create_playlist("В дорогу");
}

/// «First Love» with one file (no number) and no track count: its page
/// asks the (fake) YouTube Music for the rest of the album
/// (docs/stage4.md §12.8).
fn one_song_of_an_album(lib: &Library, root: &Path) {
    let artist = "宇多田ヒカル";
    let Ok(a) = lib.upsert_artist(&Artist::new(artist)) else { return };
    let credit = ArtistCredit::single(artist, Some(a.id));
    let dir = root.join(artist).join("1999 - First Love");
    let _ = std::fs::create_dir_all(&dir);
    let cover = dir.join("cover.jpg");
    make_cover(&cover, [210, 120, 150], 99);
    let mut r = Release::new("First Love", credit.clone());
    r.year = Some(1999);
    r.cover_ref = Some(cover.display().to_string());
    let Ok(r) = lib.upsert_release(&r) else { return };
    let mut t = Track::new("First Love", credit);
    t.release_id = Some(r.id);
    t.duration_ms = Some(257_000);
    let Ok(t) = lib.upsert_track(&t) else { return };
    let path = dir.join("First Love.wav");
    write_tone(&path, 196.0, 24.0);
    let mut s = Source::new(t.id, SourceKind::LocalFile, path.display().to_string(), Codec::Wav);
    s.bitrate = Some(705);
    let _ = lib.add_source(&s);
}

/// A number in −1..1 from the inputs (the same on every run).
fn noise(a: usize, b: usize, c: usize, salt: u64) -> f32 {
    let mut h = 0xcbf2_9ce4_8422_2325_u64 ^ salt;
    for x in [a, b, c] {
        h = (h ^ x as u64).wrapping_mul(0x100_0000_01b3);
        h ^= h >> 29;
    }
    (h % 20_001) as f32 / 10_000.0 - 1.0
}

/// A synthetic bliss vector (features version 2 order: tempo, ZCR,
/// spectral centroid mean and deviation, rolloff mean and deviation,
/// flatness mean and deviation, loudness mean and deviation, 13 chroma
/// features): the style sets the timbre and harmony features, each track
/// its own energy, brightness and steadiness, so moods cut across styles.
fn vector(style: usize, album: usize, n: usize) -> Vec<f32> {
    let lean = [0.35, 0.15, -0.45][style];
    let energy = (noise(album, n, 1, 11) * 0.85 + lean).clamp(-1.0, 1.0);
    let bright = noise(album, n, 2, 13) * 0.9;
    let steady = noise(album, n, 3, 17) * 0.9;
    let mut v: Vec<f32> = (0..nikgob_recs::DIM)
        .map(|d| ((style * 5 + d * 3) % 7) as f32 / 3.0 - 1.0 + noise(album, n, d, 19) * 0.12)
        .collect();
    v[0] = energy * 0.8;
    v[1] = energy * 0.5 + bright * 0.2;
    v[2] = energy * 0.4 + bright * 0.4;
    v[3] = -steady * 0.5;
    v[4] = bright * 0.6;
    v[8] = energy * 0.7;
    v[9] = -steady * 0.6;
    v[16] += bright * 0.5;
    v[17] -= bright * 0.5;
    v
}

/// Three months of listening: every track completed once long ago, then
/// sessions of one style a day (the first style most), mostly in the
/// evening and at night, with a few early skips.
fn history(lib: &Library, tracks: &[(TrackId, usize, usize)]) {
    let now = nikgob_library::now_ms();
    let today = now - now.rem_euclid(DAY_MS);
    let device = lib.device_id();
    let play = |track: TrackId, ts: i64, skip: bool| {
        let mut start = Event::new(track, EventType::Start, ts, device.clone());
        start.position_ms = Some(0);
        start.context = EventContext::Album;
        let _ = lib.record_event_raw(&start);
        let mut end = if skip {
            let mut e = Event::new(track, EventType::Skip, ts + 6_000, device.clone());
            e.position_ms = Some(6_000);
            e
        } else {
            Event::new(track, EventType::Complete, ts + 24_000, device.clone())
        };
        end.context = EventContext::Album;
        let _ = lib.record_event_raw(&end);
    };
    for (k, (t, i, n)) in tracks.iter().enumerate() {
        let day = HISTORY_DAYS - 1 - (noise(*i, *n, 4, 23).abs() * 30.0) as i64;
        play(*t, today - day * DAY_MS + 20 * 3_600_000 + k as i64 * 60_000, false);
    }
    // Hours of the sessions (UTC), evening and night first.
    const HOURS: [i64; 8] = [19, 20, 21, 22, 23, 1, 9, 14];
    for day in 0..HISTORY_DAYS {
        let d = day as usize;
        if noise(d, 0, 5, 29) < -0.3 {
            continue;
        }
        let hour = HOURS[(noise(d, 1, 6, 31).abs() * 7.99) as usize];
        let style = (noise(d, 2, 7, 37).abs() * 2.99) as usize;
        if noise(d, 3, 8, 41).abs() > STYLE_WEIGHT[style] {
            continue;
        }
        let pool: Vec<&(TrackId, usize, usize)> = tracks.iter().filter(|(_, i, _)| STYLES[*i] == style).collect();
        let mut ts = today - day * DAY_MS + hour * 3_600_000;
        for p in 0..(6 + (noise(d, 4, 9, 43).abs() * 6.0) as usize) {
            let (t, i, n) = pool[(noise(d, p, 10, 47).abs() * (pool.len() as f32 - 0.01)) as usize];
            let skip = noise(*i, *n, d, 53) > 0.8;
            play(*t, ts, skip);
            ts += 30_000;
        }
    }
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

#[cfg(test)]
mod tests {
    use nikgob_core::model::{ArtistId, ReleaseId, Uuid};
    use nikgob_recs::{Catalog, Mood, Play, Recs, Taste, TasteInput, TrackInfo, VectorIndex};

    use super::*;

    /// The demo's vectors make three style mixes and every mood its own
    /// tracks (what the home screenshots show).
    #[test]
    fn the_demo_sounds_make_three_styles_and_six_moods() {
        let mut infos = Vec::new();
        let mut index = VectorIndex::new();
        let mut events = Vec::new();
        for (i, (_, _, _, _, _, tracks)) in ALBUMS.iter().enumerate() {
            for n in 0..tracks.len() {
                let id = TrackId(Uuid::from_u128((i * 100 + n + 1) as u128));
                let artist = ArtistId(Uuid::from_u128(1_000 + i as u128));
                infos.push(TrackInfo::new(id, vec![artist], Some(ReleaseId(Uuid::from_u128(2_000 + i as u128)))));
                let v = vector(STYLES[i], i, n);
                index.insert(id, <[f32; nikgob_recs::DIM]>::try_from(v.as_slice()).unwrap());
                events.push(Play {
                    track: id,
                    ts: 1_000_000,
                    kind: EventType::Complete,
                    position_ms: None,
                });
            }
        }
        let catalog = Catalog::new(infos);
        let taste = Taste::build(&catalog, &TasteInput { events, ..TasteInput::default() }, 2_000_000);
        let recs = Recs::new(&catalog, &taste, &index);
        let styles = recs.style_mixes(&[], 1);
        assert_eq!(styles.len(), 3, "{:?}", styles.iter().map(|s| s.size).collect::<Vec<_>>());
        for m in Mood::ALL {
            assert!(recs.mood_pool(m).len() >= 20, "{m:?}");
        }
    }
}
