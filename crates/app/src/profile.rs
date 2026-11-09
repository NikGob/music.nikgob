//! `--demo-profile` (or `NIKGOB_DEMO_PROFILE=1`): a separate player for
//! videos and UI tests, beside the real one and apart from it.
//!
//! Everything it keeps is in its own folder, `%LOCALAPPDATA%\nikgob-demo-profile`
//! ([`dir`]): the library, prefs.json, settings.toml, the caches, the log,
//! the services' folders (their sign-ins in the credential store are keyed
//! by that folder, so they start empty and never meet the real ones), and
//! `Music\`, `Inbox\`, `Downloads\`, `Trash\` for files added later. It has
//! its own single-instance slot in that folder ([`slot`]): the real player
//! and the profile run side by side, and a second «Открыть» brings the
//! profile's window up. It never reads or writes the real player's folders.
//!
//! On its first start the library is seeded from a catalogue of popular US
//! releases (`assets/demo-catalog.json`, built from MusicBrainz: release and
//! recording ids, real lengths): artists, albums, an EP and singles, front
//! covers from the Cover Art Archive (a generated one where that fails),
//! three months of listening in three styles with synthetic sound vectors
//! (the demo's generator, [`demo::history`]), likes and playlists, so Главная,
//! the mixes, the artist pages and the lyrics look like a real user's. The
//! tracks have no files: each has a local source where its file would be,
//! which the audio engine plays as silence of the track's length
//! ([`crate::audio::play_missing_files_silent`]); «Включить с YouTube» (key
//! Y) plays the real song instead. A file imported later (Inbox, or the
//! usual import) lands on the seeded track: same release by MBID or name,
//! same number and title, and any file is better than the silent source
//! (quality 0), so it replaces it.
//!
//! The prefs of a fresh profile are the first start's with nothing that
//! downloads, moves, converts or seeds files by itself, and without the
//! lyrics models' automatic download (they would only fail on silent
//! tracks). Its online parts (lyrics, covers, photos, YouTube Music,
//! Deezer, SoundCloud without an account) work as usual.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nikgob_core::model::{
    Analysis, AnalysisSource, Artist, ArtistCredit, Codec, LikeTarget, Release, ReleaseType, Source, SourceKind,
    TrackId, Track,
};
use nikgob_library::Library;
use nikgob_view::Prefs;
use nikgob_view::dev::DevProfile;

use crate::demo::{self, ADDED_DAYS_AGO, DAY_MS, HISTORY_DAYS};

/// Starts the profile.
pub const FLAG: &str = "--demo-profile";
/// With [`FLAG`]: seed the profile and quit, no window (after «Сбросить»).
pub const SEED_ONLY: &str = "--seed-only";
/// The profile's folder name in the user's local app data.
const DIR_NAME: &str = "nikgob-demo-profile";
/// The name of the profile's single-instance slot.
const SLOT_NAME: &str = "music.nikgob-profile";
/// The releases it is seeded with.
const CATALOG: &str = include_str!("../assets/demo-catalog.json");

/// This run is the profile.
pub fn requested() -> bool {
    std::env::args().any(|a| a == FLAG) || std::env::var_os("NIKGOB_DEMO_PROFILE").is_some()
}

/// This run only seeds the profile.
pub fn seed_only() -> bool {
    std::env::args().any(|a| a == SEED_ONLY)
}

/// The profile's folder: `%LOCALAPPDATA%\nikgob-demo-profile` (the local
/// data dir elsewhere).
pub fn dir() -> PathBuf {
    directories::BaseDirs::new()
        .map_or_else(std::env::temp_dir, |b| b.data_local_dir().to_owned())
        .join(DIR_NAME)
}

/// Where the profile keeps the library, its prefs and settings, and the
/// root of its `Music\`, `Inbox\`, `Downloads\` and `Trash\`.
pub(crate) fn paths(dir: &Path) -> crate::Paths {
    crate::Paths {
        db: dir.join("library.sqlite"),
        root: dir.to_owned(),
        prefs: dir.join("prefs.json"),
        settings: Some(dir.join("settings.toml")),
    }
}

/// The profile's single-instance slot, in its own folder: never the real
/// player's.
pub fn slot() -> fastframe_instance::Slot {
    fastframe_instance::Slot::at(dir().join("instance"), SLOT_NAME)
}

/// Makes the folder ready before the library opens; on the first start
/// writes the prefs of a fresh profile ([`first_prefs`]).
pub fn prepare(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let prefs = dir.join("prefs.json");
    if !prefs.exists() {
        match serde_json::to_vec_pretty(&first_prefs()) {
            Ok(bytes) => {
                if let Err(e) = std::fs::write(&prefs, bytes) {
                    log::warn!("profile: writing {}: {e}", prefs.display());
                }
            }
            Err(e) => log::warn!("profile: prefs: {e}"),
        }
    }
}

/// A fresh profile's prefs: the first start's, without the greeting's
/// question, and with nothing that downloads, replaces, converts or seeds
/// files by itself.
pub(crate) fn first_prefs() -> Prefs {
    let mut p = Prefs::first_start();
    p.user_name = String::new();
    p.name_prompt = false;
    p.autostart = false;
    p.like_downloads = false;
    p.upgrade_every = nikgob_view::UpgradeEvery::Never;
    p.upgrade_auto = false;
    p.no_seed = true;
    p.save_found_lyrics = false;
    p.convert_fake_lossless = false;
    p.fake_flac_auto = false;
    // The models would be fetched for songs that have no sound to time.
    p.lyrics_models_auto = false;
    p
}

/// Does what Настройки → «Для разработчика» asked (in the real player).
pub fn act(action: DevProfile) {
    match action {
        DevProfile::Open => {
            if let Err(e) = spawn(&[]) {
                log::warn!("profile: cannot start it: {e}");
            }
        }
        DevProfile::Reset => {
            let started = std::thread::Builder::new().name("profile-reset".into()).spawn(reset);
            if let Err(e) = started {
                log::warn!("profile: cannot reset it: {e}");
            }
        }
    }
}

/// Starts this exe as the profile, with `extra` arguments.
fn spawn(extra: &[&str]) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    std::process::Command::new(exe)
        .arg(FLAG)
        .args(extra)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(drop)
}

/// «Сбросить»: the running profile quits, its folder (that one only) is
/// deleted, and a run without a window seeds it again.
fn reset() {
    let slot = slot();
    if slot.send("quit").is_ok() {
        let until = Instant::now() + Duration::from_secs(20);
        while Instant::now() < until && slot.send("ping").is_ok() {
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    let dir = dir();
    if !deletable(&dir, &crate::paths()) {
        log::error!("profile: {} is not the profile's own folder, nothing deleted", dir.display());
        return;
    }
    // The profile that just quit may hold its files a moment longer.
    let mut gone = false;
    for _ in 0..20 {
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => {
                gone = true;
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                gone = true;
                break;
            }
            Err(e) => {
                log::info!("profile: deleting {}: {e}", dir.display());
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
    if !gone {
        log::warn!("profile: {} could not be deleted", dir.display());
        return;
    }
    log::info!("profile: {} deleted, seeding it again", dir.display());
    if let Err(e) = spawn(&[SEED_ONLY]) {
        log::warn!("profile: cannot seed it: {e}");
    }
}

/// Only the profile's own folder is ever deleted: by its name, and holding
/// none of the real player's places nor inside them.
fn deletable(dir: &Path, real: &crate::Paths) -> bool {
    let places = [Some(real.db.as_path()), Some(real.prefs.as_path()), Some(real.root.as_path()), real.settings.as_deref()];
    dir.file_name().is_some_and(|n| n == DIR_NAME)
        && dir.is_absolute()
        && places.into_iter().flatten().all(|p| {
            let home = p.parent().unwrap_or(p);
            !p.starts_with(dir) && !dir.starts_with(home)
        })
}

// ------------------------------------------------------------- catalogue

/// One name of an artist credit.
#[derive(Clone, Debug)]
pub(crate) struct Credit {
    pub name: String,
    pub mbid: Option<String>,
    /// What comes after the name (" feat. ", " & ").
    pub join: String,
}

#[derive(Clone, Debug)]
pub(crate) struct CatTrack {
    pub title: String,
    pub ms: u32,
    pub mbid: Option<String>,
    /// The track's own credit, when it is not the release's.
    pub artist: Option<Vec<Credit>>,
}

#[derive(Clone, Debug)]
pub(crate) struct CatRelease {
    pub artist: Vec<Credit>,
    pub title: String,
    /// `YYYY`, `YYYY-MM` or `YYYY-MM-DD`: the first release.
    pub date: String,
    pub kind: ReleaseType,
    /// 0 rock, 1 hip-hop, 2 pop (the sound vectors' styles).
    pub style: usize,
    pub mbid: String,
    pub group_mbid: Option<String>,
    pub tracks: Vec<CatTrack>,
}

impl CatRelease {
    fn year(&self) -> Option<i32> {
        self.date.get(..4)?.parse().ok()
    }
}

fn text(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key)?.as_str().map(str::to_owned)
}

fn credits(v: Option<&serde_json::Value>) -> Result<Vec<Credit>, String> {
    let list = v.and_then(|v| v.as_array()).ok_or("an artist credit is missing")?;
    let out: Vec<Credit> = list
        .iter()
        .map(|c| {
            Ok(Credit {
                name: text(c, "name").ok_or("an artist without a name")?,
                mbid: text(c, "mbid"),
                join: text(c, "join").unwrap_or_default(),
            })
        })
        .collect::<Result<_, String>>()?;
    if out.is_empty() {
        return Err("an empty artist credit".into());
    }
    Ok(out)
}

/// The catalogue the profile is seeded with.
pub(crate) fn catalog() -> Result<Vec<CatRelease>, String> {
    let doc: serde_json::Value = serde_json::from_str(CATALOG).map_err(|e| e.to_string())?;
    let releases = doc.get("releases").and_then(|r| r.as_array()).ok_or("no releases")?;
    releases
        .iter()
        .map(|r| {
            let title = text(r, "title").ok_or("a release without a title")?;
            let kind = match r.get("type").and_then(|t| t.as_str()) {
                Some("album") => ReleaseType::Album,
                Some("ep") => ReleaseType::Ep,
                Some("single") => ReleaseType::Single,
                other => return Err(format!("{title}: type {other:?}")),
            };
            let style = match r.get("style").and_then(|t| t.as_str()) {
                Some("rock") => 0,
                Some("hiphop") => 1,
                Some("pop") => 2,
                other => return Err(format!("{title}: style {other:?}")),
            };
            let tracks = r
                .get("tracks")
                .and_then(|t| t.as_array())
                .ok_or_else(|| format!("{title}: no tracks"))?
                .iter()
                .map(|t| {
                    let name = text(t, "title").ok_or_else(|| format!("{title}: a track without a title"))?;
                    let ms = t
                        .get("ms")
                        .and_then(serde_json::Value::as_u64)
                        .and_then(|ms| u32::try_from(ms).ok())
                        .filter(|ms| *ms > 0)
                        .ok_or_else(|| format!("{title}: «{name}» has no length"))?;
                    let artist = match t.get("artist") {
                        Some(a) => Some(credits(Some(a))?),
                        None => None,
                    };
                    Ok(CatTrack { title: name, ms, mbid: text(t, "mbid"), artist })
                })
                .collect::<Result<Vec<_>, String>>()?;
            Ok(CatRelease {
                artist: credits(r.get("artist")).map_err(|e| format!("{title}: {e}"))?,
                date: text(r, "date").ok_or_else(|| format!("{title}: no date"))?,
                kind,
                style,
                mbid: text(r, "mbid").ok_or_else(|| format!("{title}: no MBID"))?,
                group_mbid: text(r, "group_mbid"),
                title,
                tracks,
            })
        })
        .collect()
}

// ------------------------------------------------------------------ seed

/// How much each style is heard: hip-hop most, then pop, then rock.
const STYLE_WEIGHT: [f32; 3] = [0.75, 1.0, 0.85];
/// The tiles' stories (release titles), as the demo's.
const FORGOTTEN: &[&str] = &["A Rush of Blood to the Head", "Evolve"];
const REPEATED: &[&str] = &["After Hours", "SOUR", "DAMN.", "Not Like Us", "Houdini"];
const YEAR_AGO: &[&str] = &["The Eminem Show", "1989", "Night Visions"];
const COMEBACK: &[&str] = &["Nothing Was the Same"];
const UNFINISHED: &[&str] = &["24K Magic"];
/// Albums liked as albums («Любимые альбомы»).
const LIKED_ALBUMS: &[&str] = &["After Hours", "Hybrid Theory", "WHEN WE ALL FALL ASLEEP, WHERE DO WE GO?"];
/// When the releases were added, days ago (the last ones of the list last).
const ADDED_DAYS: [i64; 17] = [400, 330, 300, 240, 200, 150, 120, 90, 60, 45, 30, 20, 12, 7, 4, 2, 0];

/// Seeds an empty profile library (see the module's notes); a library
/// that has releases stays as it is.
pub fn seed_if_empty(lib: &Library, root: &Path) {
    match lib.count_releases() {
        Ok(0) => {}
        Ok(_) => return,
        Err(e) => {
            log::error!("profile: reading the library: {e}");
            return;
        }
    }
    match catalog() {
        Ok(releases) => {
            let started = Instant::now();
            seed(lib, root, &releases, true);
            log::info!("profile: seeded {} releases in {:?}", releases.len(), started.elapsed());
        }
        Err(e) => log::error!("profile: the catalogue does not read: {e}"),
    }
}

/// A file or folder name of `name`: what Windows refuses becomes `_`.
fn file_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() { '_' } else { c })
        .collect();
    clean.trim().trim_end_matches('.').to_owned()
}

/// A colour of its own for a release's generated cover.
fn hue_of(mbid: &str) -> [u8; 3] {
    let h = mbid.bytes().fold(0x811c_9dc5_u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193));
    let hue = (h % 360) as f32 / 60.0;
    let (s, v) = (0.55, 0.8);
    let c = v * s;
    let x = c * (1.0 - (hue % 2.0 - 1.0).abs());
    let (r, g, b) = match hue as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r, g, b].map(|ch| ((ch + m) * 255.0) as u8)
}

/// The credit of `parts`, one artist row per MusicBrainz artist.
fn credit_of(lib: &Library, artists: &mut HashMap<String, Artist>, parts: &[Credit]) -> ArtistCredit {
    let mut credit = ArtistCredit::default();
    let mut join = String::new();
    for p in parts {
        let key = p.mbid.clone().unwrap_or_else(|| p.name.to_lowercase());
        let id = match artists.get(&key) {
            Some(a) => Some(a.id),
            None => {
                let mut a = Artist::new(p.name.clone());
                a.mbid = p.mbid.clone();
                match lib.upsert_artist(&a) {
                    Ok(stored) => {
                        let id = stored.id;
                        artists.insert(key, stored);
                        Some(id)
                    }
                    Err(e) => {
                        log::warn!("profile: artist {}: {e}", p.name);
                        None
                    }
                }
            }
        };
        credit.push(&join, p.name.clone(), id);
        join.clone_from(&p.join);
    }
    credit
}

/// The indices of the releases titled `titles`.
fn indices(releases: &[CatRelease], titles: &[&str]) -> Vec<usize> {
    releases.iter().enumerate().filter(|(_, r)| titles.contains(&r.title.as_str())).map(|(i, _)| i).collect()
}

/// Seeds the profile library with `releases`; `files` fetches the covers
/// and writes them under `root\Music` (tests seed without them).
pub(crate) fn seed(lib: &Library, root: &Path, releases: &[CatRelease], files: bool) {
    let now = nikgob_library::now_ms();
    let music = root.join("Music");
    let mut artists: HashMap<String, Artist> = HashMap::new();
    // Where each release keeps its cover, like an imported album.
    let dirs: Vec<PathBuf> = releases
        .iter()
        .map(|r| {
            let artist = r.artist.first().map_or("Unknown", |a| a.name.as_str());
            let album = format!("{} - {}", r.year().unwrap_or_default(), r.title);
            music.join(file_name(artist)).join(file_name(&album))
        })
        .collect();
    let fetched = if files {
        let jobs: Vec<(String, PathBuf)> =
            releases.iter().zip(&dirs).map(|(r, d)| (r.mbid.clone(), d.join("cover.jpg"))).collect();
        fetch_covers(&jobs)
    } else {
        vec![false; releases.len()]
    };
    let mut all: Vec<(TrackId, usize, usize)> = Vec::new();
    for (i, r) in releases.iter().enumerate() {
        let credit = credit_of(lib, &mut artists, &r.artist);
        let dir = &dirs[i];
        let cover = dir.join("cover.jpg");
        if files && !fetched[i] {
            let _ = std::fs::create_dir_all(dir);
            demo::make_cover(&cover, hue_of(&r.mbid), i as u32);
        }
        let mut rel = Release::new(r.title.clone(), credit.clone());
        rel.mbid = Some(r.mbid.clone());
        rel.release_group_mbid.clone_from(&r.group_mbid);
        rel.year = r.year();
        rel.release_type = r.kind;
        rel.cover_ref = Some(cover.display().to_string());
        rel.total_tracks = Some(r.tracks.len() as u32);
        rel.added_at = now - ADDED_DAYS[i * ADDED_DAYS.len() / releases.len().max(1)] * DAY_MS - i as i64 * 60_000;
        let rel = match lib.upsert_release(&rel) {
            Ok(rel) => rel,
            Err(e) => {
                log::warn!("profile: release {}: {e}", r.title);
                continue;
            }
        };
        if let Some(img) = files.then(|| image::open(&cover).ok()).flatten() {
            let rgba = img.to_rgba8();
            if let Some(seed) = nikgob_md3e::seed_from_image(rgba.as_raw(), rgba.width() as usize, rgba.height() as usize) {
                let _ = lib.put_palette(&nikgob_core::model::Palette::new(rel.id, seed));
            }
        }
        for (n, t) in r.tracks.iter().enumerate() {
            let credit = match &t.artist {
                Some(parts) => credit_of(lib, &mut artists, parts),
                None => credit.clone(),
            };
            let mut tr = Track::new(t.title.clone(), credit);
            tr.recording_mbid.clone_from(&t.mbid);
            tr.release_id = Some(rel.id);
            tr.track_no = n as u32 + 1;
            tr.duration_ms = Some(t.ms);
            tr.added_at = now - (ADDED_DAYS_AGO + HISTORY_DAYS) * DAY_MS;
            let tr = match lib.upsert_track(&tr) {
                Ok(tr) => tr,
                Err(e) => {
                    log::warn!("profile: track {}: {e}", t.title);
                    continue;
                }
            };
            // Where its file would be; the engine plays it as silence. Any
            // real file is better (quality 0) and replaces it on import.
            let path = dir.join(format!("{:02} - {}.opus", n + 1, file_name(&t.title)));
            let mut s = Source::new(tr.id, SourceKind::LocalFile, path.display().to_string(), Codec::Opus);
            s.bitrate = Some(160);
            s.sample_rate = Some(48_000);
            s.quality_score = 0;
            s.size_bytes = Some(u64::from(t.ms) * 20);
            if let Err(e) = lib.add_source(&s) {
                log::warn!("profile: source of {}: {e}", t.title);
            }
            // Likes over four months; every single.
            if (n + i) % 4 == 0 || r.kind == ReleaseType::Single {
                let _ = lib.like_at(tr.id, now - ((i * 11 + n * 7) % 120) as i64 * DAY_MS);
            }
            let _ = lib.put_analysis(&Analysis {
                track_id: tr.id,
                bliss_vector: demo::vector(r.style, i, n),
                analyzed_with: nikgob_recs::ANALYZED_WITH.to_owned(),
                source: AnalysisSource::File,
            });
            all.push((tr.id, i, n));
        }
        if LIKED_ALBUMS.contains(&r.title.as_str()) {
            let _ = lib.like_at(LikeTarget::Release(rel.id), now - (10 + i as i64) * DAY_MS);
        }
    }
    let styles: Vec<usize> = releases.iter().map(|r| r.style).collect();
    let (forgotten, repeated, year_ago, comeback, unfinished) = (
        indices(releases, FORGOTTEN),
        indices(releases, REPEATED),
        indices(releases, YEAR_AGO),
        indices(releases, COMEBACK),
        indices(releases, UNFINISHED),
    );
    let plan = demo::Plan {
        lens: releases.iter().map(|r| r.tracks.len()).collect(),
        styles: &styles,
        weights: STYLE_WEIGHT,
        forgotten: &forgotten,
        repeated: &repeated,
        year_ago: &year_ago,
        comeback: &comeback,
        unfinished: &unfinished,
        stream_only: &[],
        lengths: Some(releases.iter().map(|r| r.tracks.iter().map(|t| t.ms).collect()).collect()),
    };
    demo::history(lib, &all, &plan);
    playlists(lib, releases, &all, &forgotten);
}

/// Four playlists with English names: calm songs, loud ones, the early
/// 2000s, and one not opened since its albums were loved («Из плейлиста»).
fn playlists(lib: &Library, releases: &[CatRelease], all: &[(TrackId, usize, usize)], forgotten: &[usize]) {
    // The energy the sound vector was made with (its first feature).
    let energy = |i: usize, n: usize| demo::vector(releases[i].style, i, n)[0] / 0.8;
    let early = |i: usize| releases[i].year().is_some_and(|y| y < 2005);
    let lists: [(&str, Vec<TrackId>); 4] = [
        ("Late Night Drive", all.iter().filter(|(_, i, n)| energy(*i, *n) < -0.2).map(|t| t.0).take(25).collect()),
        ("Gym", all.iter().filter(|(_, i, n)| energy(*i, *n) > 0.45).map(|t| t.0).take(30).collect()),
        ("Throwbacks", all.iter().filter(|(_, i, n)| early(*i) && *n < 4).map(|t| t.0).collect()),
        ("Road Trip", all.iter().filter(|(_, i, _)| forgotten.contains(i)).map(|t| t.0).collect()),
    ];
    for (name, tracks) in lists {
        if tracks.is_empty() {
            continue;
        }
        match lib.create_playlist(name) {
            Ok(p) => {
                let _ = lib.playlist_add_many(p.id, &tracks);
            }
            Err(e) => log::warn!("profile: playlist {name}: {e}"),
        }
    }
}

/// The releases' front covers from the Cover Art Archive (by release MBID,
/// 1200 px), saved at each job's path as JPEG with the 16 px `ambient.png`
/// beside it; true where that worked. A few at a time, all within a minute.
fn fetch_covers(jobs: &[(String, PathBuf)]) -> Vec<bool> {
    let none = vec![false; jobs.len()];
    let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(4).thread_name("profile-covers").enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            log::warn!("profile: no runtime for the covers: {e}");
            return none;
        }
    };
    let http = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("nikgob-music/", env!("CARGO_PKG_VERSION")))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            log::warn!("profile: no http client for the covers: {e}");
            return none;
        }
    };
    rt.block_on(async {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        let mut out = Vec::with_capacity(jobs.len());
        for batch in jobs.chunks(4) {
            let handles: Vec<_> = batch
                .iter()
                .map(|(mbid, path)| {
                    let (http, path) = (http.clone(), path.clone());
                    let url = format!("https://coverartarchive.org/release/{mbid}/front-1200");
                    tokio::spawn(async move { fetch_cover(&http, &url, &path).await })
                })
                .collect();
            for h in handles {
                out.push(matches!(tokio::time::timeout_at(deadline, h).await, Ok(Ok(true))));
            }
        }
        out
    })
}

async fn fetch_cover(http: &reqwest::Client, url: &str, path: &Path) -> bool {
    let bytes = match http.get(url).header("Accept", "image/jpeg,image/png;q=0.9").send().await {
        Ok(r) if r.status().is_success() => match r.bytes().await {
            Ok(b) => b,
            Err(e) => {
                log::info!("profile: cover {url}: {e}");
                return false;
            }
        },
        Ok(r) => {
            log::info!("profile: cover {url}: HTTP {}", r.status().as_u16());
            return false;
        }
        Err(e) => {
            log::info!("profile: cover {url}: {e}");
            return false;
        }
    };
    let img = match image::load_from_memory(&bytes) {
        Ok(img) => img.to_rgb8(),
        Err(e) => {
            log::info!("profile: cover {url} does not decode: {e}");
            return false;
        }
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = img.save(path) {
        log::warn!("profile: saving {}: {e}", path.display());
        return false;
    }
    let small = image::imageops::resize(&img, 16, 16, image::imageops::FilterType::Triangle);
    let _ = small.save(path.with_file_name("ambient.png"));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The catalogue reads, every track has a length, the releases are
    /// unique, and the tiles' stories name releases it has.
    #[test]
    fn the_catalogue_reads_and_every_track_has_a_length() {
        let releases = catalog().unwrap();
        assert!(releases.len() >= 15, "{} releases", releases.len());
        let mut mbids = std::collections::HashSet::new();
        for r in &releases {
            assert!(mbids.insert(r.mbid.clone()), "{} twice", r.title);
            assert!(r.year().is_some_and(|y| (1990..=2026).contains(&y)), "{}: {}", r.title, r.date);
            assert!(!r.tracks.is_empty(), "{}", r.title);
            for t in &r.tracks {
                assert!(t.ms >= 5_000 && t.ms < 20 * 60_000, "{} – {}: {} ms", r.title, t.title, t.ms);
                assert!(t.mbid.is_some(), "{} – {}", r.title, t.title);
            }
        }
        for kind in [ReleaseType::Album, ReleaseType::Ep, ReleaseType::Single] {
            assert!(releases.iter().any(|r| r.kind == kind), "{kind:?}");
        }
        for style in 0..3 {
            assert!(releases.iter().filter(|r| r.style == style).count() >= 3, "style {style}");
        }
        for titles in [FORGOTTEN, REPEATED, YEAR_AGO, COMEBACK, UNFINISHED, LIKED_ALBUMS] {
            assert_eq!(indices(&releases, titles).len(), titles.len(), "{titles:?}");
        }
        // Artists with two albums.
        let mut albums: HashMap<&str, usize> = HashMap::new();
        for r in releases.iter().filter(|r| r.kind == ReleaseType::Album) {
            *albums.entry(r.artist[0].name.as_str()).or_default() += 1;
        }
        assert!(albums.values().filter(|n| **n >= 2).count() >= 2, "{albums:?}");
    }

    /// The profile's folder and everything in it are apart from the real
    /// player's: not the same, not inside one another.
    #[test]
    fn the_profile_never_uses_the_real_places() {
        let real = crate::paths();
        let ours = paths(&dir());
        let theirs = [Some(&real.db), Some(&real.prefs), Some(&real.root), real.settings.as_ref()];
        for mine in [Some(&ours.db), Some(&ours.prefs), Some(&ours.root), ours.settings.as_ref()].into_iter().flatten() {
            for other in theirs.into_iter().flatten() {
                assert_ne!(mine, other);
                assert!(!mine.starts_with(other) && !other.starts_with(mine), "{} / {}", mine.display(), other.display());
            }
        }
        for other in theirs.into_iter().flatten() {
            let home = other.parent().unwrap();
            assert!(!dir().starts_with(home), "{} inside {}", dir().display(), home.display());
        }
        assert!(deletable(&dir(), &real));
        // Nothing else is ever deleted.
        for other in theirs.into_iter().flatten() {
            assert!(!deletable(other, &real));
            assert!(!deletable(other.parent().unwrap(), &real));
        }
        assert!(!deletable(Path::new(DIR_NAME), &real), "a relative path");
        assert_ne!(slot().dir(), fastframe_instance::Slot::new("music.nikgob").dir());
        assert!(slot().dir().starts_with(dir()));
    }

    /// A fresh profile downloads, replaces, converts and seeds nothing by
    /// itself and asks no name.
    #[test]
    fn a_fresh_profile_touches_no_files_by_itself() {
        let p = first_prefs();
        assert!(!p.like_downloads && !p.upgrade_auto && p.no_seed && !p.save_found_lyrics);
        assert!(!p.convert_fake_lossless && !p.fake_flac_auto && !p.autostart && !p.lyrics_models_auto);
        assert_eq!(p.upgrade_every, nikgob_view::UpgradeEvery::Never);
        assert!(p.user_name.is_empty() && !p.name_prompt);
        // Written and read as the app reads it, it stays so.
        let read = crate::prefs_of(Some(&serde_json::to_vec_pretty(&p).unwrap()));
        assert!(!read.like_downloads && read.no_seed && !read.name_prompt && !read.lyrics_models_auto);
    }

    /// The seed fills the library: every release, every track with its
    /// length, a silent local source of quality 0 and a sound vector, a
    /// history, likes and the playlists.
    #[test]
    fn the_seed_fills_a_library() {
        let lib = Library::open_in_memory().unwrap();
        let root = tempfile::tempdir().unwrap();
        let releases = catalog().unwrap();
        seed(&lib, root.path(), &releases, false);
        assert_eq!(lib.count_releases().unwrap() as usize, releases.len());
        let tracks: usize = releases.iter().map(|r| r.tracks.len()).sum();
        let vectors = lib.analysis_vectors(nikgob_recs::ANALYZED_WITH).unwrap();
        assert_eq!(vectors.len(), tracks);
        for row in vectors.iter().take(20) {
            let t = lib.get_track(row.track_id).unwrap().unwrap();
            assert!(t.duration_ms.is_some_and(|ms| ms > 0));
            let sources = lib.sources_of_track(t.id).unwrap();
            assert_eq!(sources.len(), 1);
            assert!(sources[0].kind == SourceKind::LocalFile && sources[0].quality_score == 0);
            assert!(!Path::new(&sources[0].locator).exists());
        }
        // Nothing was written but the library (no covers without `files`).
        assert!(!root.path().join("Music").exists());
        // Seeding again (a library with releases) changes nothing.
        seed_if_empty(&lib, root.path());
        assert_eq!(lib.count_releases().unwrap() as usize, releases.len());
    }
}
