//! `NIKGOB_DEMO=1`: a throwaway library with generated covers, for looking
//! at the interface without importing music. Files do not exist; playback
//! goes to the null backend.
//!
//! For the recommendations (docs/stage4.md) every track gets a synthetic
//! sound vector and the library a listening history of three months: the
//! albums fall into three styles (Russian rock, electronic, dark and
//! dreamy), tracks within a style spread over energy and brightness so
//! every mood has its own, and plays lean to the evening and the night
//! with a few early skips. The home then shows «Твой микс», the daylist,
//! six moods, three style mixes, and (from the fake services) discoveries
//! and new releases. For the bento's tiles the history also has a month of
//! repeats, plays a year ago, two albums loved half a year ago and not
//! since, the last two songs of every album played only once, and fresh
//! likes. Beyond them, for the other kinds of tiles (docs/stage4.md §12.9):
//! a soundtrack album started and left, an artist not heard for two
//! months, likes over four months, songs never played, long and short
//! songs, lyrics on the songs with words, a rested playlist and liked
//! stream songs without files.

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
    ("Kevin Penkin", "Made in Abyss Original Soundtrack", 2017, ReleaseType::Album, [70, 110, 90], &["Made in Abyss", "Hanezeve Caradhina", "Underground River", "The First Layer", "Belaf", "Pathway", "Tomorrow", "Old Stories", "Deep in Abyss", "Endless Embrace"]),
    ("Air", "Moon Safari", 1998, ReleaseType::Album, [240, 210, 160], &["La Femme d'Argent", "Sexy Boy", "All I Need", "Kelly Watch the Stars", "Talisman", "Remember", "You Make It Easy", "Ce matin-là", "New Star in the Sky"]),
];

/// The style of each album (index in [`ALBUMS`]): 0 Russian rock, 1
/// electronic, 2 dark and dreamy.
const STYLES: &[usize] = &[0, 0, 2, 1, 0, 1, 2, 2, 2, 1, 0, 1, 2, 1];
/// How much each style is listened to (a day's session is skipped above it).
const STYLE_WEIGHT: [f32; 3] = [1.0, 0.7, 0.55];
/// Days of history.
pub(crate) const HISTORY_DAYS: i64 = 90;
/// Albums loved half a year ago and not played since («Забытое любимое»).
const FORGOTTEN: &[usize] = &[6, 9];
/// Albums whose first songs are on repeat this month («На повторе»).
const REPEATED: &[usize] = &[0, 1, 4, 10];
/// Albums played around this day a year ago («Год назад»).
const YEAR_AGO: &[usize] = &[2, 3, 11];
/// The last this many songs of every album are played once
/// («Глубже у любимых»).
const DEEP: usize = 2;
/// An artist played a lot and not for the last 45 days («Давно не слышал»).
const COMEBACK: &[usize] = &[2];
/// Albums started three weeks ago and left after two songs («Дослушай»).
const UNFINISHED: &[usize] = &[12];
/// Liked stream songs without files («Ещё не скачано»).
const STREAM_ONLY: &[usize] = &[13];
/// Albums without words (no lyrics).
const NO_LYRICS: &[usize] = &[5, 9, 12];
/// Added this long before the history starts.
pub(crate) const ADDED_DAYS_AGO: i64 = 120;
pub(crate) const DAY_MS: i64 = 24 * 3600 * 1000;

/// A cover: abstract art from the hue (sun over stripes, diagonal bands,
/// soft blobs or a dot grid, by `seed`) with a little grain, 600×600 JPEG.
pub(crate) fn make_cover(path: &Path, rgb: [u8; 3], seed: u32) {
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

/// Artists with a photo (the rest show their initials), and the photo's hue.
const PHOTOS: &[(&str, [u8; 3])] = &[
    ("Кино", [150, 60, 50]),
    ("Radiohead", [50, 90, 140]),
    ("Daft Punk", [120, 70, 160]),
    ("Земфира", [60, 130, 110]),
    ("Massive Attack", [110, 110, 120]),
    ("The Prodigy", [170, 80, 40]),
];

/// An artist photo: a concert from the floor, the stage lights in the hue
/// as soft bokeh and beams through haze over the crowd's silhouette, a
/// vignette and grain, 400×400 JPEG.
fn make_photo(path: &Path, rgb: [u8; 3], seed: u32) {
    let size = 400u32;
    let base = rgb.map(f32::from);
    let mix = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] {
        let t = t.clamp(0.0, 1.0);
        [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
    };
    let smooth = |e0: f32, e1: f32, x: f32| {
        let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let glow = mix(base, [255.0, 240.0, 220.0], 0.55);
    let deep = mix(base, [0.0; 3], 0.85);
    // Stage lights: a few soft discs, placed by the seed.
    let lights: Vec<(f32, f32, f32)> = (0..7)
        .map(|k| {
            let r = |salt: u32| ((seed.wrapping_mul(2_654_435_761) ^ (k * 40_503 + salt).wrapping_mul(97_531)) % 1000) as f32 / 1000.0;
            (r(1), r(2) * 0.6, 0.03 + r(3) * 0.06)
        })
        .collect();
    let lean = ((seed % 5) as f32 - 2.0) * 0.03;
    let img = image::RgbImage::from_fn(size, size, |x, y| {
        let (u, v) = (x as f32 / size as f32, y as f32 / size as f32);
        // The stage: dark below, a spot from above, haze in the hue.
        let spot = (-((u - 0.5 - lean).powi(2)) / 0.06).exp() * (1.0 - v);
        let mut c = mix(deep, base, 0.35 + spot * 0.9);
        for (lx, ly, lr) in &lights {
            let d = ((u - lx).powi(2) + (v - ly).powi(2)).sqrt();
            c = mix(c, glow, (1.0 - smooth(lr * 0.7, *lr, d)) * 0.55);
        }
        // Beams from the rig, fanning down from above.
        for (bx, tilt) in [(0.2 + lean, 0.35), (0.55 - lean, -0.1), (0.85, -0.4)] {
            let along = (u - bx - tilt * v).abs() / (0.03 + v * 0.12);
            c = mix(c, glow, (1.0 - smooth(0.4, 1.0, along)) * 0.28 * (1.0 - v * 0.6));
        }
        // The crowd: heads and raised hands against the light.
        let k = seed as f32;
        let crowd = 0.8
            + 0.035 * ((u * 31.0 + k).sin() + 0.6 * (u * 57.0 + k * 1.7).sin())
            - 0.09 * (1.0 - smooth(0.0, 0.025, ((u * 7.0 + k * 0.3).fract() - 0.5).abs())) * (u * 3.0 + k).sin().max(0.0);
        let dark = mix([6.0; 3], base, 0.08);
        c = mix(c, dark, smooth(crowd - 0.004, crowd + 0.004, v));
        // Vignette and grain.
        let vig = ((u - 0.5).powi(2) + (v - 0.45).powi(2)).sqrt();
        c = mix(c, [0.0; 3], smooth(0.4, 0.8, vig) * 0.55);
        let h = (x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663) ^ seed.wrapping_mul(83_492_791)) % 1000;
        let n = (h as f32 / 1000.0 - 0.5) * 16.0;
        image::Rgb(c.map(|ch| (ch + n).clamp(0.0, 255.0) as u8))
    });
    let _ = img.save(path);
}

/// Marker file of the current seed; bump to reseed old demo libraries.
pub const VERSION: &str = "seed-v16";

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
    seed_with(lib, root, true);
}

/// The length of a song as the library says (the files are 24 s tones):
/// Aphex Twin and half of Mezzanine are long, Земфира and the first songs
/// of «Звезда по имени Солнце» short.
fn duration_of(album: usize, n: usize) -> u32 {
    match album {
        5 => 440_000 + n as u32 * 20_000,
        7 if n % 2 == 0 => 425_000,
        4 => 130_000 + n as u32 * 2_000,
        1 if n < 3 => 140_000,
        _ => 210_000 + n as u32 * 7_000,
    }
}

/// The demo library; `files` writes the covers, tones and lyrics files
/// next to it (tests seed without them).
fn seed_with(lib: &Library, root: &Path, files: bool) {
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
    let now = nikgob_library::now_ms();
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
        let cover = dir.join("cover.jpg");
        if files {
            let _ = std::fs::create_dir_all(&dir);
            make_cover(&cover, *rgb, i as u32);
        }
        let mut r = Release::new(*album, credit.clone());
        r.year = Some(*year);
        r.release_type = *kind;
        r.cover_ref = Some(cover.display().to_string());
        r.total_tracks = Some(tracks.len() as u32 + u32::from(i % 3 == 0) * 2);
        // Added over the last year, the last albums of the list first, so
        // «Недавно добавлено» and its headers have something to tell.
        const ADDED_DAYS: [i64; 12] = [300, 200, 120, 90, 45, 40, 20, 12, 5, 3, 0, 0];
        r.added_at = nikgob_library::now_ms() - ADDED_DAYS[i % ADDED_DAYS.len()] * DAY_MS - i as i64 * 60_000;
        let r = lib.upsert_release(&r).expect("release");
        if let Some(img) = files.then(|| image::open(&cover).ok()).flatten() {
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
            t.duration_ms = Some(duration_of(i, n));
            t.added_at = now - (ADDED_DAYS_AGO + HISTORY_DAYS) * DAY_MS;
            let t = lib.upsert_track(&t).expect("track");
            if STREAM_ONLY.contains(&i) {
                // A liked song of YouTube Music, played from the stream.
                let s = Source::new(t.id, SourceKind::Youtube, format!("demo-{i}-{n}"), Codec::Opus);
                let _ = lib.add_source(&s);
                let _ = lib.like_at(t.id, now - n as i64 * 2 * DAY_MS);
            } else {
                let codec = Codec::Wav;
                let path = dir.join(format!("{:02} - {title}.wav", n + 1));
                if files {
                    write_tone(&path, 220.0 * 2f32.powf((i * 3 + n) as f32 / 12.0), 24.0);
                }
                let mut s = Source::new(t.id, SourceKind::LocalFile, path.display().to_string(), codec);
                s.bitrate = Some(705);
                s.sample_rate = Some(44_100);
                s.bit_depth = Some(16);
                let _ = lib.add_source(&s);
                if !NO_LYRICS.contains(&i) {
                    let _ = lib.put_lyrics(t.id, Some(DEMO_LRC), None, false);
                    if files {
                        let _ = std::fs::write(path.with_extension("ttml"), DEMO_TTML);
                    }
                }
                if n == 0 {
                    first_tracks.push(t.id);
                }
                // Likes over four months: the first ones long ago, some fresh.
                if (n + i) % 4 == 0 {
                    let _ = lib.like_at(t.id, now - ((i * 11 + n * 7) % 120) as i64 * DAY_MS);
                }
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
    one_song_of_an_album(lib, root, files);
    history(lib, &all, &Plan::demo());
    // Photos for some artists; the rest are marked as looked up, so the
    // demo never asks a service for them.
    let photos = root.join("..").join("photos");
    let _ = std::fs::create_dir_all(&photos);
    for (n, (name, a)) in artists.iter().enumerate() {
        match PHOTOS.iter().find(|(p, _)| p == name) {
            Some((_, rgb)) => {
                let path = photos.join(format!("artist-{n}.jpg"));
                make_photo(&path, *rgb, n as u32);
                let _ = lib.set_artist_photo(a.id, Some(&path.display().to_string()));
            }
            None => {
                let _ = lib.set_artist_photo(a.id, None);
            }
        }
    }
    if let Ok(p) = lib.create_playlist("Вечером") {
        let _ = lib.playlist_add_many(p.id, &first_tracks);
    }
    // Not opened since the forgotten albums: «Из плейлиста».
    if let Ok(p) = lib.create_playlist("В дорогу") {
        let rested: Vec<TrackId> = all.iter().filter(|(_, i, _)| FORGOTTEN.contains(i)).map(|(t, _, _)| *t).collect();
        let _ = lib.playlist_add_many(p.id, &rested);
    }
}

/// «First Love» with one file (no number) and no track count: its page
/// asks the (fake) YouTube Music for the rest of the album
/// (docs/stage4.md §12.8).
fn one_song_of_an_album(lib: &Library, root: &Path, files: bool) {
    let artist = "宇多田ヒカル";
    let Ok(a) = lib.upsert_artist(&Artist::new(artist)) else { return };
    let credit = ArtistCredit::single(artist, Some(a.id));
    let dir = root.join(artist).join("1999 - First Love");
    let cover = dir.join("cover.jpg");
    if files {
        let _ = std::fs::create_dir_all(&dir);
        make_cover(&cover, [210, 120, 150], 99);
    }
    let mut r = Release::new("First Love", credit.clone());
    r.year = Some(1999);
    r.cover_ref = Some(cover.display().to_string());
    let Ok(r) = lib.upsert_release(&r) else { return };
    let mut t = Track::new("First Love", credit);
    t.release_id = Some(r.id);
    t.duration_ms = Some(257_000);
    let Ok(t) = lib.upsert_track(&t) else { return };
    let path = dir.join("First Love.wav");
    if files {
        write_tone(&path, 196.0, 24.0);
    }
    let mut s = Source::new(t.id, SourceKind::LocalFile, path.display().to_string(), Codec::Wav);
    s.bitrate = Some(705);
    let _ = lib.add_source(&s);
}

/// A number in −1..1 from the inputs (the same on every run).
pub(crate) fn noise(a: usize, b: usize, c: usize, salt: u64) -> f32 {
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
pub(crate) fn vector(style: usize, album: usize, n: usize) -> Vec<f32> {
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

/// What a listening history is made of: each album's songs and style, how
/// much each style is heard, and the albums of each tile's story (indices
/// into the albums). The demo's songs are 24 s tones; a library of real
/// songs gives their lengths, and its plays last as long.
pub(crate) struct Plan<'a> {
    /// Songs per album.
    pub lens: Vec<usize>,
    /// The style (0..3) of each album.
    pub styles: &'a [usize],
    /// How much each style is listened to (a day's session is skipped above it).
    pub weights: [f32; 3],
    pub forgotten: &'a [usize],
    pub repeated: &'a [usize],
    pub year_ago: &'a [usize],
    pub comeback: &'a [usize],
    pub unfinished: &'a [usize],
    pub stream_only: &'a [usize],
    /// Every song's length in ms by album and number; `None`: the demo's tones.
    pub lengths: Option<Vec<Vec<u32>>>,
}

impl Plan<'static> {
    /// The demo library's.
    fn demo() -> Self {
        Self {
            lens: ALBUMS.iter().map(|a| a.5.len()).collect(),
            styles: STYLES,
            weights: STYLE_WEIGHT,
            forgotten: FORGOTTEN,
            repeated: REPEATED,
            year_ago: YEAR_AGO,
            comeback: COMEBACK,
            unfinished: UNFINISHED,
            stream_only: STREAM_ONLY,
            lengths: None,
        }
    }
}

impl Plan<'_> {
    /// How long a complete play of the song lasts.
    fn length(&self, album: usize, n: usize) -> i64 {
        self.lengths.as_ref().and_then(|l| l.get(album)?.get(n).copied()).map_or(24_000, i64::from)
    }

    /// The last songs of an album are played once («Глубже у любимых»): not
    /// in a single, which is its one song.
    fn deep(&self, album: usize, n: usize) -> bool {
        let len = self.lens[album];
        len > DEEP && n + DEEP >= len
    }

    /// The last song of an album is never played: not a single's.
    fn last(&self, album: usize, n: usize) -> bool {
        let len = self.lens[album];
        len > 1 && n + 1 == len
    }
}

/// Three months of listening: every track completed once long ago, then
/// sessions of one style a day (the first style most), mostly in the
/// evening and at night, with a few early skips; and the tiles' own
/// history (the plan's albums for [`FORGOTTEN`], [`REPEATED`],
/// [`YEAR_AGO`], [`COMEBACK`], [`UNFINISHED`], and [`DEEP`]); the last song
/// of every album is never played, the stream songs neither.
pub(crate) fn history(lib: &Library, tracks: &[(TrackId, usize, usize)], plan: &Plan<'_>) {
    let now = nikgob_library::now_ms();
    let today = now - now.rem_euclid(DAY_MS);
    let device = lib.device_id();
    // Real songs: a session that would end after now is not made up (the
    // demo's tones keep their old history).
    let until = if plan.lengths.is_some() { now } else { i64::MAX };
    let play = |track: TrackId, ts: i64, skip: bool, length: i64| {
        if ts + length > until {
            return;
        }
        let mut start = Event::new(track, EventType::Start, ts, device.clone());
        start.position_ms = Some(0);
        start.context = EventContext::Album;
        let _ = lib.record_event_raw(&start);
        let mut end = if skip {
            let mut e = Event::new(track, EventType::Skip, ts + 6_000, device.clone());
            e.position_ms = Some(6_000);
            e
        } else {
            Event::new(track, EventType::Complete, ts + length, device.clone())
        };
        end.context = EventContext::Album;
        let _ = lib.record_event_raw(&end);
    };
    for (k, (t, i, n)) in tracks.iter().enumerate() {
        let length = plan.length(*i, *n);
        if plan.stream_only.contains(i) {
            continue;
        }
        if plan.unfinished.contains(i) {
            if *n < 2 {
                play(*t, today - 20 * DAY_MS + 21 * 3_600_000 + k as i64 * 60_000, false, length);
            }
            continue;
        }
        if plan.forgotten.contains(i) {
            // Loved half a year ago: four completes each.
            for r in 0..4 {
                play(*t, today - (150 + r * 9) * DAY_MS + 21 * 3_600_000 + k as i64 * 60_000, false, length);
            }
            continue;
        }
        if plan.last(*i, *n) {
            continue;
        }
        let day = HISTORY_DAYS - 1 - (noise(*i, *n, 4, 23).abs() * 30.0) as i64;
        play(*t, today - day * DAY_MS + 20 * 3_600_000 + k as i64 * 60_000, false, length);
        if plan.year_ago.contains(i) && !plan.deep(*i, *n) {
            let day = 365 + (noise(*i, *n, 11, 59) * 10.0) as i64;
            play(*t, today - day * DAY_MS + 19 * 3_600_000 + k as i64 * 60_000, false, length);
        }
        if plan.repeated.contains(i) && *n < 3 {
            for day in (1..28).step_by(3) {
                play(*t, today - day * DAY_MS + 18 * 3_600_000 + k as i64 * 60_000, false, length);
            }
        }
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
        if noise(d, 3, 8, 41).abs() > plan.weights[style] {
            continue;
        }
        let pool: Vec<&(TrackId, usize, usize)> = tracks
            .iter()
            .filter(|(_, i, n)| {
                plan.styles[*i] == style
                    && !plan.forgotten.contains(i)
                    && !plan.unfinished.contains(i)
                    && !plan.stream_only.contains(i)
                    && !(plan.comeback.contains(i) && day < 45)
                    && !plan.deep(*i, *n)
            })
            .collect();
        if pool.is_empty() {
            continue;
        }
        let mut ts = today - day * DAY_MS + hour * 3_600_000;
        for p in 0..(6 + (noise(d, 4, 9, 43).abs() * 6.0) as usize) {
            let (t, i, n) = pool[(noise(d, p, 10, 47).abs() * (pool.len() as f32 - 0.01)) as usize];
            let skip = noise(*i, *n, d, 53) > 0.8;
            let length = plan.length(*i, *n);
            play(*t, ts, skip, length);
            // The tones follow each other every 30 s; real songs as they end.
            ts += match (&plan.lengths, skip) {
                (None, _) => 30_000,
                (Some(_), true) => 8_000,
                (Some(_), false) => length + 2_000,
            };
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
                    context: Default::default(),
                    origin: None,
                    end_reason: None,
                    attention: None,
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

#[cfg(test)]
mod tile_tests {
    use std::collections::HashSet;

    use nikgob_view::TileKind;

    use super::*;

    /// A week of the demo's boards: most kinds of tiles show up, and none
    /// but «На повторе» three days in a row (docs/stage4.md §12.9).
    #[test]
    fn a_week_of_the_demo_shows_most_tiles() {
        let lib = Library::open_in_memory().unwrap();
        // The seed writes artist photos next to the library: a temporary
        // folder, not the crate.
        let dir = tempfile::tempdir().unwrap();
        seed_with(&lib, &dir.path().join("demo"), false);
        let days = nikgob_runtime::preview_tile_days(&lib, 7).unwrap();
        let kinds: Vec<Vec<TileKind>> = days.iter().map(|d| d.iter().map(|t| t.0).collect()).collect();
        for (d, k) in kinds.iter().enumerate() {
            assert_eq!(k.len(), nikgob_recs::TILES_PER_DAY, "day {d}: {k:?}");
            assert_eq!(k[0], TileKind::OnRepeat, "day {d}: {k:?}");
        }
        let shown: HashSet<TileKind> = kinds.iter().flatten().copied().collect();
        assert!(shown.len() >= 15, "{} kinds: {kinds:?}", shown.len());
        for w in kinds.windows(3) {
            for k in &w[2] {
                assert!(*k == TileKind::OnRepeat || !(w[0].contains(k) && w[1].contains(k)), "{k:?} three days running: {kinds:?}");
            }
        }
        // The week as picked (with --nocapture).
        for (d, tiles) in days.iter().enumerate() {
            eprintln!("day {d}: {:?}", tiles.iter().map(|t| format!("{} «{}» ({})", t.0.key(), t.1, t.3)).collect::<Vec<_>>());
        }
    }
}
