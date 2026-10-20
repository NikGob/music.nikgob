//! Reading the services' export files (docs/stage3.md §3.6, cold start):
//! a folder or a `.zip` (or a folder of `.zip`s, as Google Takeout splits
//! big exports), recognised by file name and structure rather than by exact
//! folder names, since Takeout localises those.
//!
//! | File | What it gives |
//! |---|---|
//! | `Streaming_History_Audio_*.json`, `endsong_*.json` (Spotify Extended Streaming History) | plays with the track uri |
//! | `StreamingHistory*.json` (Spotify Account data; `_podcast_` / `_video_` ones skipped) | plays by artist + title, used only when no extended history is in the same export |
//! | `YourLibrary.json` (Spotify Account data) | liked tracks (`tracks[]`) |
//! | `Playlist*.json` (Spotify Account data) | playlists with their tracks |
//! | any `.json` under a `YouTube…` folder (or called `watch-history.json` / `история просмотра.json` anywhere) that is a list of `{header: "YouTube Music", titleUrl: …watch?v=…, time}` | YouTube Music plays |
//! | any `.html` that is a My Activity page (`watch-history.html`, Takeout's default format): `outer-cell` entries headed «YouTube Music» | YouTube Music plays |
//! | `.csv` in a playlist folder under a `YouTube…` folder (`playlists/`, `плейлисты/`), or any `.csv` headed `Video ID,Playlist Video Creation Timestamp` | playlists by video id; the liked-music one ([`LIKED_NAMES`]) gives likes, «Watch later» ([`WATCH_LATER_NAMES`]) is left out |
//! | a `.csv` headed `Playlist ID,…,Playlist Title (Original),…` (`playlists.csv`) | the names and ids of the playlists above |
//! | a `.csv` headed `Video ID,Song Title,Album Title,Artist Name 1,…` (`music library songs.csv`) | likes, with their titles, albums and artists |
//!
//! CSV and HTML files are told by their first line / bytes, so loose files
//! (no `Takeout/YouTube…` folders around them) work too, and anything else
//! with those extensions (a game's save, the search history) is skipped.
//! Entries of plain YouTube (header «YouTube») are not plays, but the
//! videos are remembered ([`ExportData::plain_watched`]): the import uses
//! them to tell playlists of music from playlists of videos.
//!
//! Everything here is plain parsing, without the library or the network;
//! the cold-start import ([`super::cold`]) turns the result into tracks,
//! events, likes and playlists.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;

use nikgob_core::model::UnixMs;
use nikgob_core::streaming::Service;
use serde_json::Value;

/// Plays shorter than this count as skipped.
pub(crate) const SKIP_BELOW_MS: u64 = 30_000;
/// Export files bigger than this are not read (an export file is a few MB).
const MAX_FILE: u64 = 512 << 20;

/// Names (folded, as the CSV file is called without `.csv` / `-videos`) of
/// the YouTube Music playlist of liked songs in Takeout, in the languages
/// seen so far. Matching is exact after folding.
pub(crate) const LIKED_NAMES: &[&str] = &[
    "liked music",
    "liked songs",
    "понравившаяся музыка",
    "понравилось",
    "мне понравилось",
    "понравившиеся треки",
    "любимая музыка",
    "понравившееся",
    "вподобана музика",
    "música que me gusta",
    "musique que j'aime",
    "titres likés",
    "musik, die ich mag",
    "mag ich",
];

/// Names (folded) of YouTube's «Watch later» list, which is not music and
/// is not imported (its id is `WL` where a file names it).
pub(crate) const WATCH_LATER_NAMES: &[&str] = &[
    "watch later",
    "смотреть позже",
    "посмотреть позже",
    "переглянути пізніше",
    "später ansehen",
    "à regarder plus tard",
    "ver más tarde",
    "guarda più tardi",
    "assistir mais tarde",
    "do obejrzenia",
    "あとで見る",
    "나중에 볼 동영상",
];

/// Bytes read from a CSV or HTML file to tell what it is.
const HEAD: u64 = 64 << 10;

/// Prefixes Takeout puts before a watched title ("Watched …"), by language.
const WATCHED_PREFIXES: &[&str] = &[
    "Watched ",
    "Посмотрели ",
    "Просмотрено: ",
    "Просмотрено ",
    "Вы посмотрели ",
    "Вы смотрели ",
    "Переглянуто: ",
    "Ви переглянули ",
    "Angesehen: ",
    "Vous avez regardé ",
    "Has visto ",
    "Visto ",
    "Assistiu a ",
    "Hai guardato ",
    "Obejrzano: ",
];
/// …and the languages that put it after the title.
const WATCHED_SUFFIXES: &[&str] = &[" angesehen", " を視聴しました", " 시청함"];

/// A track as an export file names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Item {
    pub(crate) service: Service,
    /// Spotify track id / YouTube video id, when the file has it.
    pub(crate) id: Option<String>,
    /// Empty when the file does not say (a YouTube playlist lists ids only).
    pub(crate) title: String,
    pub(crate) artist: String,
    /// The other artists of the credit, when the file lists them apart.
    pub(crate) featured: Vec<String>,
    pub(crate) album: Option<String>,
}

impl Item {
    fn new(service: Service, id: Option<String>, title: &str, artist: &str, album: Option<&str>) -> Self {
        Self {
            service,
            id: id.filter(|i| !i.is_empty()),
            title: title.trim().to_owned(),
            artist: artist.trim().to_owned(),
            featured: Vec::new(),
            album: album.map(str::trim).filter(|a| !a.is_empty()).map(str::to_owned),
        }
    }
}

/// How a play ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Complete,
    Skip,
    /// The file does not say (YouTube history).
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Play {
    pub(crate) item: Item,
    /// When it started (unix ms).
    pub(crate) start_ms: UnixMs,
    /// How long it played, when the file says.
    pub(crate) ms_played: Option<u64>,
    pub(crate) outcome: Outcome,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExportPlaylist {
    pub(crate) service: Service,
    pub(crate) name: String,
    /// The playlist's id in the service, when the file has it.
    pub(crate) remote: Option<String>,
    pub(crate) items: Vec<Item>,
}

/// What an export holds.
#[derive(Clone, Debug, Default)]
pub(crate) struct ExportData {
    pub(crate) plays: Vec<Play>,
    pub(crate) likes: Vec<Item>,
    pub(crate) playlists: Vec<ExportPlaylist>,
    /// YouTube videos watched on plain YouTube (not YouTube Music).
    pub(crate) plain_watched: HashSet<String>,
    /// The files that were recognised and read (paths inside the export).
    pub(crate) files: Vec<String>,
    /// Recognised files that could not be read, with the reason.
    pub(crate) problems: Vec<String>,
}

impl ExportData {
    pub(crate) fn is_empty(&self) -> bool {
        self.plays.is_empty() && self.likes.is_empty() && self.playlists.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Extended,
    OldHistory,
    Library,
    SpotifyPlaylists,
    /// A JSON under a YouTube folder: a watch history if its content says so.
    YoutubeJson,
    /// An HTML file: a My Activity page if its content says so.
    YoutubeHtml,
    YoutubePlaylist,
    /// A CSV elsewhere: what it is depends on its header ([`csv_kind`]).
    Csv,
    /// `playlists.csv`: playlist ids and names.
    PlaylistIndex,
    /// `music library songs.csv`.
    MusicLibrary,
}

/// Lower case, `ё` → `е`, trimmed.
fn fold(s: &str) -> String {
    nikgob_library::fold(s.trim())
}

/// What a file is, from its path inside the export (`/` separated).
fn kind_of(path: &str) -> Option<Kind> {
    let lower = fold(path);
    let mut parts: Vec<&str> = lower.split('/').filter(|p| !p.is_empty()).collect();
    let name = parts.pop()?;
    let in_youtube = parts.iter().any(|p| p.contains("youtube"));
    if let Some(stem) = name.strip_suffix(".json") {
        if stem.starts_with("streaming_history_audio") || stem.starts_with("endsong") {
            return Some(Kind::Extended);
        }
        if stem.starts_with("streaming_history_") {
            return None; // video / podcast parts of the extended history
        }
        if stem.starts_with("streaminghistory") {
            return (!stem.contains("podcast") && !stem.contains("video")).then_some(Kind::OldHistory);
        }
        if stem == "yourlibrary" {
            return Some(Kind::Library);
        }
        if in_youtube || stem == "watch-history" || stem == "история просмотра" {
            return Some(Kind::YoutubeJson);
        }
        if stem.starts_with("playlist") {
            return Some(Kind::SpotifyPlaylists);
        }
        return None;
    }
    if name.ends_with(".csv") {
        let dir = parts.last().copied().unwrap_or("");
        let playlist_dir = ["playlist", "плейлист", "списки", "wiedergabeliste", "lista", "liste"]
            .iter()
            .any(|w| dir.contains(w));
        return Some(if in_youtube && playlist_dir { Kind::YoutubePlaylist } else { Kind::Csv });
    }
    if name.ends_with(".html") || name.ends_with(".htm") {
        return Some(Kind::YoutubeHtml);
    }
    None
}

/// What a file of kind `kind` (by its name) is after a look at its first
/// bytes; `None`: not an export file after all.
fn sniff(kind: Kind, head: &[u8]) -> Option<Kind> {
    match kind {
        Kind::Csv | Kind::YoutubePlaylist => {
            let text = String::from_utf8_lossy(strip_bom(head));
            let first = text.lines().next().unwrap_or("");
            // A playlist folder also holds the old layout, whose header is localised.
            csv_kind(first).or((kind == Kind::YoutubePlaylist).then_some(kind))
        }
        // My Activity pages are styled with Material Design Lite.
        Kind::YoutubeHtml => head.windows(4).any(|w| w == b"mdl-").then_some(kind),
        k => Some(k),
    }
}

/// What a Takeout CSV is from its header line (the newer files have the
/// same English headers in every language).
fn csv_kind(header: &str) -> Option<Kind> {
    let cols: Vec<String> = header.split(',').map(|c| fold(c.trim_matches('"'))).collect();
    let col = |i: usize| cols.get(i).map_or("", String::as_str);
    let has = |name: &str| cols.iter().any(|c| c == name);
    match col(0) {
        "video id" if has("song title") => Some(Kind::MusicLibrary),
        "video id" if col(1).starts_with("playlist video") => Some(Kind::YoutubePlaylist),
        "playlist id" if cols.iter().any(|c| c.starts_with("playlist title")) => Some(Kind::PlaylistIndex),
        // The older layout's metadata block.
        "playlist id" if col(1) == "channel id" => Some(Kind::YoutubePlaylist),
        _ => None,
    }
}

/// Reads every export file in `path` (a folder, a `.zip`, or a folder with
/// `.zip`s in it). Files it does not recognise are ignored.
pub(crate) fn read_export(path: &Path) -> Result<ExportData, String> {
    let mut files: Vec<(String, Kind, Vec<u8>)> = Vec::new();
    if path.is_dir() {
        walk(path, path, &mut files)?;
    } else if is_zip(path) {
        read_zip(path, &mut files)?;
    } else {
        // A single export file.
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let not_export = || format!("«{name}» — не файл выгрузки");
        let kind = kind_of(&name).ok_or_else(not_export)?;
        let bytes = read_file(path)?;
        let kind = sniff(kind, &bytes[..bytes.len().min(HEAD as usize)]).ok_or_else(not_export)?;
        files.push((name, kind, bytes));
    }
    let mut out = ExportData::default();
    let has_extended = files.iter().any(|(_, k, _)| *k == Kind::Extended);
    // The extended history repeats the account-data one in finer detail.
    files.retain(|(_, k, _)| !(has_extended && *k == Kind::OldHistory));
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut yt_playlists = Vec::new();
    let mut index = Vec::new();
    for (name, kind, bytes) in files {
        let parsed = match kind {
            Kind::YoutubePlaylist => {
                let text = String::from_utf8_lossy(&bytes);
                yt_playlists.push(youtube_playlist(&name, &text));
                Ok(true)
            }
            Kind::PlaylistIndex => {
                index.extend(playlist_index(&String::from_utf8_lossy(&bytes)));
                Ok(true)
            }
            Kind::MusicLibrary => {
                let songs = library_songs(&String::from_utf8_lossy(&bytes));
                let found = !songs.is_empty();
                out.likes.extend(songs);
                Ok(found)
            }
            Kind::YoutubeHtml => Ok(html_history(&String::from_utf8_lossy(&bytes), &mut out)),
            Kind::Csv => Ok(false),
            _ => match serde_json::from_slice::<Value>(strip_bom(&bytes)) {
                Ok(json) => Ok(apply_json(kind, &json, &mut out)),
                Err(e) => Err(e.to_string()),
            },
        };
        match parsed {
            Ok(true) => out.files.push(name),
            Ok(false) => {}
            Err(e) => {
                log::warn!("export file {name}: {e}");
                out.problems.push(format!("{name}: {e}"));
            }
        }
    }
    // The liked-music playlist is the likes; «Watch later» is not music;
    // empty lists (a list of playlist ids) are nothing.
    for mut p in yt_playlists.into_iter().flatten() {
        name_from_index(&mut p, &index);
        let folded = fold(&p.name);
        if LIKED_NAMES.contains(&folded.as_str()) {
            out.likes.extend(p.items);
        } else if WATCH_LATER_NAMES.contains(&folded.as_str()) || p.remote.as_deref() == Some("WL") {
            log::debug!("export: «{}» left out", p.name);
        } else if !p.items.is_empty() {
            out.playlists.push(p);
        }
    }
    fill_youtube_titles(&mut out);
    // The library and the liked-music playlist name the same songs.
    let mut seen = HashSet::new();
    out.likes
        .retain(|i| i.id.as_ref().is_none_or(|id| seen.insert((i.service, id.clone()))));
    Ok(out)
}

/// A playlist CSV of the newer layout names neither its id nor (but by its
/// file name) its title: `playlists.csv` has both. Takeout makes the file
/// name from the title with the characters a file name cannot hold
/// replaced by `_`.
fn name_from_index(p: &mut ExportPlaylist, index: &[(String, String)]) {
    if p.remote.is_some() {
        return;
    }
    let stem = fold(&p.name);
    let file_safe = |t: &str| -> String {
        t.chars()
            .map(|c| if matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
            .collect()
    };
    if let Some((id, title)) = index
        .iter()
        .find(|(_, t)| fold(t) == stem || fold(&file_safe(t)) == stem)
    {
        p.remote = Some(id.clone());
        p.name.clone_from(title);
    }
}

/// `playlists.csv`: (id, original title) of each playlist.
fn playlist_index(text: &str) -> Vec<(String, String)> {
    let rows = csv_rows(text);
    let Some(header) = rows.first() else { return Vec::new() };
    let header: Vec<String> = header.iter().map(|h| fold(h)).collect();
    let id_col = header.iter().position(|h| h == "playlist id");
    let title_col = header
        .iter()
        .position(|h| h == "playlist title (original)")
        .or_else(|| header.iter().position(|h| h.starts_with("playlist title") && !h.contains("language")));
    let (Some(id_col), Some(title_col)) = (id_col, title_col) else { return Vec::new() };
    rows[1..]
        .iter()
        .filter_map(|r| {
            let id = r.get(id_col)?.trim();
            let title = r.get(title_col)?.trim();
            (!id.is_empty() && !title.is_empty()).then(|| (id.to_owned(), title.to_owned()))
        })
        .collect()
}

/// `music library songs.csv`: the songs of the YouTube Music library (the
/// liked ones among them) with their titles, albums and up to nine artists.
/// Some ids come with a leading space.
fn library_songs(text: &str) -> Vec<Item> {
    let rows = csv_rows(text);
    let Some(header) = rows.first() else { return Vec::new() };
    let header: Vec<String> = header.iter().map(|h| fold(h)).collect();
    let col = |name: &str| header.iter().position(|h| h == name);
    let (Some(id_col), Some(title_col)) = (col("video id"), col("song title")) else {
        return Vec::new();
    };
    let album_col = col("album title");
    let artist_cols: Vec<usize> = (0..header.len()).filter(|&i| header[i].starts_with("artist name")).collect();
    rows[1..]
        .iter()
        .filter_map(|r| {
            let id = r.get(id_col)?.trim();
            if !is_video_id(id) {
                return None;
            }
            let mut artists: Vec<&str> = Vec::new();
            for a in artist_cols.iter().filter_map(|&c| r.get(c)).map(|a| a.trim()) {
                if !a.is_empty() && !artists.contains(&a) {
                    artists.push(a);
                }
            }
            let album = album_col.and_then(|c| r.get(c)).map(String::as_str);
            let mut item = Item::new(
                Service::Youtube,
                Some(id.to_owned()),
                r.get(title_col)?,
                artists.first().copied().unwrap_or_default(),
                album,
            );
            item.featured = artists.iter().skip(1).map(|a| (*a).to_owned()).collect();
            Some(item)
        })
        .collect()
}

fn strip_bom(b: &[u8]) -> &[u8] {
    b.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(b)
}

fn is_zip(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"))
}

fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len > MAX_FILE {
        return Err(format!("{}: слишком большой файл", path.display()));
    }
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn read_head(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut head = Vec::new();
    std::fs::File::open(path)?.take(HEAD).read_to_end(&mut head)?;
    Ok(head)
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Kind, Vec<u8>)>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut entries: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk(root, &p, out)?;
        } else if is_zip(&p) {
            if let Err(e) = read_zip(&p, out) {
                log::warn!("export: {e}");
            }
        } else {
            let rel = p.strip_prefix(root).unwrap_or(&p);
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            // The folder itself may be the YouTube one: its name counts.
            let named = match root.file_name() {
                Some(n) => format!("{}/{rel}", n.to_string_lossy()),
                None => rel,
            };
            let Some(kind) = kind_of(&named) else { continue };
            let kind = match kind {
                // Any CSV or HTML may be one: a look at its start first, and
                // one that cannot be read is not an export file.
                Kind::Csv | Kind::YoutubeHtml | Kind::YoutubePlaylist => {
                    match read_head(&p).map(|head| sniff(kind, &head)) {
                        Ok(Some(k)) => k,
                        Ok(None) => continue,
                        Err(e) => {
                            log::debug!("export: {}: {e}", p.display());
                            continue;
                        }
                    }
                }
                k => k,
            };
            out.push((named, kind, read_file(&p)?));
        }
    }
    Ok(())
}

fn read_zip(path: &Path, out: &mut Vec<(String, Kind, Vec<u8>)>) -> Result<(), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    for i in 0..zip.len() {
        let mut entry = match zip.by_index(i) {
            Ok(e) => e,
            Err(e) => {
                log::warn!("{}: entry {i}: {e}", path.display());
                continue;
            }
        };
        if entry.is_dir() || entry.size() > MAX_FILE {
            continue;
        }
        let name = entry.name().replace('\\', "/");
        let Some(kind) = kind_of(&name) else { continue };
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        if let Err(e) = entry.read_to_end(&mut bytes) {
            log::warn!("{}: {name}: {e}", path.display());
            continue;
        }
        let Some(kind) = sniff(kind, &bytes[..bytes.len().min(HEAD as usize)]) else { continue };
        out.push((name, kind, bytes));
    }
    Ok(())
}

/// Parses one JSON file into `out`; `false` when it turned out not to be
/// an export file after all.
fn apply_json(kind: Kind, json: &Value, out: &mut ExportData) -> bool {
    match kind {
        Kind::Extended => {
            out.plays.extend(json.as_array().into_iter().flatten().filter_map(extended_play));
            true
        }
        Kind::OldHistory => {
            out.plays.extend(json.as_array().into_iter().flatten().filter_map(old_play));
            true
        }
        Kind::Library => {
            out.likes.extend(
                json.get("tracks")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(library_track),
            );
            true
        }
        Kind::SpotifyPlaylists => match json.get("playlists").and_then(Value::as_array) {
            Some(list) => {
                out.playlists.extend(list.iter().filter_map(spotify_playlist));
                true
            }
            None => false,
        },
        Kind::YoutubeJson => {
            let mut found = false;
            for v in json.as_array().into_iter().flatten() {
                if let Some(play) = watched(v) {
                    out.plays.push(play);
                    found = true;
                } else if str_of(v, "header") == Some("YouTube")
                    && let Some(id) = str_of(v, "titleUrl").and_then(video_id)
                {
                    // Plain YouTube: not a play, but a sign of a video.
                    out.plain_watched.insert(id);
                    found = true;
                }
            }
            found
        }
        Kind::YoutubePlaylist | Kind::YoutubeHtml | Kind::Csv | Kind::PlaylistIndex | Kind::MusicLibrary => false,
    }
}

fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

/// `spotify:track:ID` → `ID` (local files and episodes have none).
fn spotify_id(uri: Option<&str>) -> Option<String> {
    uri?.strip_prefix("spotify:track:").map(str::to_owned).filter(|s| !s.is_empty())
}

/// ISO-8601 (`2019-08-12T14:19:43Z`, fractions allowed) or the account
/// data's `2019-08-12 14:19` (UTC), as unix ms.
pub(crate) fn parse_time(s: &str) -> Option<UnixMs> {
    let s = s.trim();
    if let Ok(ts) = s.parse::<jiff::Timestamp>() {
        return Some(ts.as_millisecond());
    }
    let civil = s.replacen(' ', "T", 1);
    let civil = if civil.len() == 16 { format!("{civil}:00") } else { civil };
    let dt: jiff::civil::DateTime = civil.parse().ok()?;
    dt.to_zoned(jiff::tz::TimeZone::UTC).ok().map(|z| z.timestamp().as_millisecond())
}

fn extended_play(v: &Value) -> Option<Play> {
    let title = str_of(v, "master_metadata_track_name")?;
    let artist = str_of(v, "master_metadata_album_artist_name").unwrap_or_default();
    let album = str_of(v, "master_metadata_album_album_name");
    let end = parse_time(str_of(v, "ts")?)?;
    let ms = v.get("ms_played").and_then(Value::as_u64).unwrap_or(0);
    let done = str_of(v, "reason_end") == Some("trackdone");
    let skipped = v.get("skipped").and_then(Value::as_bool).unwrap_or(false);
    let outcome = if done {
        Outcome::Complete
    } else if skipped || ms < SKIP_BELOW_MS {
        Outcome::Skip
    } else {
        Outcome::Complete
    };
    Some(Play {
        item: Item::new(Service::Spotify, spotify_id(str_of(v, "spotify_track_uri")), title, artist, album),
        start_ms: end - ms as i64,
        ms_played: Some(ms),
        outcome,
    })
}

fn old_play(v: &Value) -> Option<Play> {
    let title = str_of(v, "trackName")?;
    let artist = str_of(v, "artistName").unwrap_or_default();
    if title == "Unknown Track" {
        return None;
    }
    let end = parse_time(str_of(v, "endTime")?)?;
    let ms = v.get("msPlayed").and_then(Value::as_u64).unwrap_or(0);
    Some(Play {
        item: Item::new(Service::Spotify, None, title, artist, None),
        start_ms: end - ms as i64,
        ms_played: Some(ms),
        outcome: if ms >= SKIP_BELOW_MS { Outcome::Complete } else { Outcome::Skip },
    })
}

fn library_track(v: &Value) -> Option<Item> {
    let title = str_of(v, "track")?;
    Some(Item::new(
        Service::Spotify,
        spotify_id(str_of(v, "uri")),
        title,
        str_of(v, "artist").unwrap_or_default(),
        str_of(v, "album"),
    ))
}

fn spotify_playlist(v: &Value) -> Option<ExportPlaylist> {
    let name = str_of(v, "name")?;
    let items = v
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|i| {
            let t = i.get("track").filter(|t| t.is_object())?;
            Some(Item::new(
                Service::Spotify,
                spotify_id(str_of(t, "trackUri")),
                str_of(t, "trackName")?,
                str_of(t, "artistName").unwrap_or_default(),
                str_of(t, "albumName"),
            ))
        })
        .collect();
    Some(ExportPlaylist {
        service: Service::Spotify,
        name: name.to_owned(),
        remote: str_of(v, "uri")
            .and_then(|u| u.strip_prefix("spotify:playlist:"))
            .map(str::to_owned),
        items,
    })
}

/// The `v=` of a YouTube watch url.
fn video_id(url: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    query
        .split('&')
        .find_map(|kv| kv.strip_prefix("v="))
        .map(|v| v.split('#').next().unwrap_or(v).to_owned())
        .filter(|v| is_video_id(v))
}

/// Looks like a video id: the YouTube id alphabet, with a letter (a date is
/// not one), shorter than a channel or playlist id. Real ones have 11
/// characters; the bounds are loose for other ids (the fake service's).
fn is_video_id(s: &str) -> bool {
    (6..=20).contains(&s.len())
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        && s.bytes().any(|b| b.is_ascii_alphabetic())
}

/// A playlist id in a Takeout CSV (`PL…`, the liked list `LM`, `VL…`).
fn is_playlist_id(s: &str) -> bool {
    s.len() >= 13
        && ["PL", "VL", "LM", "OL"].iter().any(|p| s.starts_with(p))
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// "Artist - Topic" (the auto-generated channel) → "Artist"; "ArtistVEVO" → "Artist".
pub(crate) fn channel_artist(name: &str) -> String {
    let n = name.trim();
    for suffix in [" - Topic", " – Topic", " - тема", " – тема", " - Тема", " – Тема"] {
        if let Some(a) = n.strip_suffix(suffix) {
            return a.trim().to_owned();
        }
    }
    match n.strip_suffix("VEVO") {
        Some(a) if !a.trim().is_empty() => a.trim().to_owned(),
        _ => n.to_owned(),
    }
}

/// "Watched Song" in any of the known languages → "Song". A title that is
/// the url itself (a removed video) gives "".
fn watched_title(title: &str) -> String {
    let t = title.trim();
    if t.starts_with("http://") || t.starts_with("https://") {
        return String::new();
    }
    for p in WATCHED_PREFIXES {
        if let Some(rest) = t.strip_prefix(p) {
            return rest.trim().to_owned();
        }
    }
    for s in WATCHED_SUFFIXES {
        if let Some(rest) = t.strip_suffix(s) {
            return rest.trim().to_owned();
        }
    }
    t.to_owned()
}

fn watched(v: &Value) -> Option<Play> {
    if str_of(v, "header") != Some("YouTube Music") {
        return None;
    }
    let id = video_id(str_of(v, "titleUrl")?)?;
    let start = parse_time(str_of(v, "time")?)?;
    let artist = v
        .get("subtitles")
        .and_then(Value::as_array)
        .and_then(|s| s.first())
        .and_then(|s| str_of(s, "name"))
        .map(channel_artist)
        .unwrap_or_default();
    let title = watched_title(str_of(v, "title").unwrap_or_default());
    Some(Play {
        item: Item::new(Service::Youtube, Some(id), &title, &artist, None),
        start_ms: start,
        ms_played: None,
        outcome: Outcome::Unknown,
    })
}

/// Splits CSV text into rows (quotes, doubled quotes and newlines in quotes).
pub(crate) fn csv_rows(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '"') if chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            (true, '"') => quoted = false,
            (true, c) => field.push(c),
            (false, '"') => quoted = true,
            (false, ',') => row.push(std::mem::take(&mut field)),
            (false, '\r') => {}
            (false, '\n') => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            (false, c) => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

/// A Takeout playlist CSV: the older layout has a metadata block (`Playlist
/// Id,Channel Id,…,Title,…` and its row) above the `Video Id,Time Added`
/// list; the newer one is the list alone (`Video ID,Playlist Video
/// Creation Timestamp`) and the name comes from the file
/// (`<name>-videos.csv`). Header names are localised, so a row is a video
/// when its first field is a video id; the metadata row is found by its
/// first field being a playlist id.
fn youtube_playlist(path: &str, text: &str) -> Option<ExportPlaylist> {
    let rows = csv_rows(text);
    let mut name = None;
    let mut remote = None;
    let mut items = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let first = row.first().map_or("", |f| f.trim());
        if is_playlist_id(first) {
            if remote.is_some() || i == 0 {
                continue;
            }
            remote = Some(first.to_owned());
            // The title column of the header above.
            let header = &rows[i - 1];
            let col = header.iter().position(|h| {
                let h = fold(h);
                ["title", "название", "titel", "título", "titre", "назва"].iter().any(|w| h.contains(w))
            });
            name = col.and_then(|c| row.get(c)).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
        } else if is_video_id(first) {
            items.push(Item::new(Service::Youtube, Some(first.to_owned()), "", "", None));
        }
    }
    let file = path.rsplit('/').next().unwrap_or(path);
    let stem = file.strip_suffix(".csv").or_else(|| file.strip_suffix(".CSV")).unwrap_or(file);
    let stem = ["-videos", "-видео", " - videos", "-vidéos", "-vídeos"]
        .iter()
        .find_map(|s| stem.strip_suffix(s))
        .unwrap_or(stem);
    Some(ExportPlaylist {
        service: Service::Youtube,
        name: name.unwrap_or_else(|| stem.trim().to_owned()),
        remote,
        items,
    })
}

/// Video ids of playlists (and of the liked-music one) have no title in
/// the CSVs: take the one the music library or the watch history knows.
fn fill_youtube_titles(out: &mut ExportData) {
    let mut known: HashMap<String, Item> = HashMap::new();
    let titled = |i: &&Item| i.service == Service::Youtube && !i.title.is_empty();
    for i in out.plays.iter().map(|p| &p.item).filter(titled) {
        if let Some(id) = &i.id {
            known.insert(id.clone(), i.clone());
        }
    }
    // The library has albums and every artist: it wins.
    for i in out.likes.iter().filter(titled) {
        if let Some(id) = &i.id {
            known.insert(id.clone(), i.clone());
        }
    }
    let fill = |i: &mut Item| {
        if i.title.is_empty()
            && let Some(k) = i.id.as_ref().and_then(|id| known.get(id))
        {
            i.title.clone_from(&k.title);
            i.artist.clone_from(&k.artist);
            i.featured.clone_from(&k.featured);
            i.album.clone_from(&k.album);
        }
    };
    out.likes.iter_mut().for_each(fill);
    for p in &mut out.playlists {
        p.items.iter_mut().for_each(fill);
    }
}

// ------------------------------------------------------------ My Activity

/// Takeout's `watch-history.html` (My Activity as a page, its default
/// format): an `outer-cell` per entry, headed by the product («YouTube
/// Music» or «YouTube»), then `Watched <a href="…watch?v=ID">Title</a><br>
/// <a href="…/channel/…">Channel</a><br>Oct 15, 2026, 4:55:46 PM IDT<br>`
/// (no channel link for a removed video, whose title is its url). Read in
/// one pass over the text, a cell at a time; `false` when no entry was a
/// watched video.
fn html_history(text: &str, out: &mut ExportData) -> bool {
    let mut found = false;
    for cell in text.split("class=\"outer-cell").skip(1) {
        let Some(header) = between(cell, "mdl-typography--title\">", "<") else { continue };
        let music = match unescape(header).trim() {
            "YouTube Music" => true,
            "YouTube" => false,
            _ => continue,
        };
        let Some((_, body)) = cell.split_once("content-cell") else { continue };
        let Some((_, body)) = body.split_once('>') else { continue };
        let body = body.split("</div>").next().unwrap_or(body);
        let mut links = Anchors(body);
        let Some((href, title)) = links.next() else { continue };
        let Some(id) = video_id(&unescape(href)) else { continue };
        found = true;
        if !music {
            out.plain_watched.insert(id);
            continue;
        }
        // The date is the last line that is not a link.
        let Some(start) = body
            .rsplit("<br>")
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.contains('<'))
            .and_then(|l| parse_activity_time(&unescape(l)))
        else {
            continue;
        };
        let title = unescape(title);
        let title = if title.starts_with("http://") || title.starts_with("https://") { "" } else { title.trim() };
        let artist = links.next().map(|(_, a)| channel_artist(&unescape(a))).unwrap_or_default();
        out.plays.push(Play {
            item: Item::new(Service::Youtube, Some(id), title, &artist, None),
            start_ms: start,
            ms_played: None,
            outcome: Outcome::Unknown,
        });
    }
    found
}

/// The text between `start` and the next `end`.
fn between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let rest = &s[s.find(start)? + start.len()..];
    Some(&rest[..rest.find(end)?])
}

/// The links of a piece of HTML: (href, text) in order.
struct Anchors<'a>(&'a str);

impl<'a> Iterator for Anchors<'a> {
    type Item = (&'a str, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        let tag = &self.0[self.0.find("<a ")?..];
        let href = between(tag, "href=\"", "\"").unwrap_or("");
        let open = tag.find('>')? + 1;
        let close = tag[open..].find("</a>")? + open;
        self.0 = &tag[close + 4..];
        Some((href, &tag[open..close]))
    }
}

/// HTML character references → text (the named ones Takeout writes, and
/// numeric ones).
fn unescape(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains('&') {
        return s.into();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let entity = rest.find(';').filter(|&e| e <= 10).map(|e| &rest[1..e]);
        let c = entity.and_then(|e| match e {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            "emsp" => Some('\u{2003}'),
            _ => {
                let n = e.strip_prefix('#')?;
                let code = match n.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                    None => n.parse().ok()?,
                };
                char::from_u32(code)
            }
        });
        match (c, entity) {
            (Some(c), Some(e)) => {
                out.push(c);
                rest = &rest[e.len() + 2..];
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out.into()
}

/// UTC offsets (minutes) of the time zone names My Activity writes. It
/// writes every date in the zone of the moment of the export; a name not
/// here is taken as UTC. `IST` is Israel's, the pair of `IDT` (Takeout
/// writes India's as `GMT+05:30` in English).
const ZONES: &[(&str, i32)] = &[
    ("UTC", 0),
    ("GMT", 0),
    ("Z", 0),
    ("WET", 0),
    ("WEST", 60),
    ("BST", 60),
    ("CET", 60),
    ("CEST", 120),
    ("MEZ", 60),
    ("MESZ", 120),
    ("EET", 120),
    ("EEST", 180),
    ("IST", 120),
    ("IDT", 180),
    ("MSK", 180),
    ("МСК", 180),
    ("SAMT", 240),
    ("GST", 240),
    ("YEKT", 300),
    ("PKT", 300),
    ("OMST", 360),
    ("NOVT", 420),
    ("KRAT", 420),
    ("ICT", 420),
    ("IRKT", 480),
    ("HKT", 480),
    ("SGT", 480),
    ("AWST", 480),
    ("YAKT", 540),
    ("JST", 540),
    ("KST", 540),
    ("ACST", 570),
    ("VLAT", 600),
    ("AEST", 600),
    ("AEDT", 660),
    ("MAGT", 660),
    ("PETT", 720),
    ("NZST", 720),
    ("NZDT", 780),
    ("NST", -210),
    ("NDT", -150),
    ("BRT", -180),
    ("ART", -180),
    ("ADT", -180),
    ("AST", -240),
    ("EDT", -240),
    ("EST", -300),
    ("CDT", -300),
    ("CST", -360),
    ("MDT", -360),
    ("MST", -420),
    ("PDT", -420),
    ("PST", -480),
    ("AKDT", -480),
    ("AKST", -540),
    ("HST", -600),
];

/// `GMT+03:00`, `UTC-4`, `GMT+0530`, or a name from [`ZONES`]: the offset
/// in minutes.
fn zone_offset(token: &str) -> Option<i32> {
    let upper = token.to_uppercase();
    if let Some(rest) = upper.strip_prefix("GMT").or_else(|| upper.strip_prefix("UTC")) {
        if rest.is_empty() {
            return Some(0);
        }
        let (sign, rest) = if let Some(r) = rest.strip_prefix('+') {
            (1, r)
        } else {
            (-1, rest.strip_prefix(['-', '\u{2212}'])?)
        };
        let (h, m) = match rest.split_once(':') {
            Some((h, m)) => (h.parse::<i32>().ok()?, m.parse::<i32>().ok()?),
            None if rest.len() > 2 => (
                rest[..rest.len() - 2].parse().ok()?,
                rest[rest.len() - 2..].parse().ok()?,
            ),
            None => (rest.parse().ok()?, 0),
        };
        return Some(sign * (h * 60 + m));
    }
    ZONES.iter().find(|(name, _)| *name == upper).map(|(_, off)| *off)
}

/// A month by its English or Russian name, abbreviated or not, in any case
/// (`Oct`, `окт.`, `мая`).
fn month(token: &str) -> Option<i8> {
    const NAMES: &[(&str, i8)] = &[
        ("jan", 1),
        ("feb", 2),
        ("mar", 3),
        ("apr", 4),
        ("may", 5),
        ("jun", 6),
        ("jul", 7),
        ("aug", 8),
        ("sep", 9),
        ("oct", 10),
        ("nov", 11),
        ("dec", 12),
        ("янв", 1),
        ("фев", 2),
        ("мар", 3),
        ("апр", 4),
        ("ма", 5),
        ("июн", 6),
        ("июл", 7),
        ("авг", 8),
        ("сен", 9),
        ("окт", 10),
        ("ноя", 11),
        ("дек", 12),
    ];
    let t = token.to_lowercase();
    NAMES.iter().find(|(p, _)| t.starts_with(p)).map(|(_, m)| *m)
}

/// A My Activity date as unix ms: `Oct 15, 2026, 4:55:46 PM IDT` (with a
/// narrow no-break space before `PM` at times), `15 окт. 2026 г., 16:55:46
/// MSK`, `15.10.2026, 16:55:46 GMT+03:00`, or ISO-8601.
pub(crate) fn parse_activity_time(s: &str) -> Option<UnixMs> {
    if let Some(t) = parse_time(s) {
        return Some(t);
    }
    let s = s.replace(['\u{202f}', '\u{a0}', ','], " ");
    let (mut year, mut mon, mut day) = (None, None, None);
    let mut hms = None;
    let mut pm = None;
    let mut offset = 0;
    for token in s.split_whitespace() {
        let lower = token.to_lowercase();
        if token.contains(':') && token.starts_with(|c: char| c.is_ascii_digit()) {
            let mut parts = token.split(':').map(|p| p.parse::<i8>().ok());
            hms = Some((parts.next()??, parts.next()??, parts.next().flatten().unwrap_or(0)));
        } else if matches!(lower.as_str(), "am" | "a.m." | "pm" | "p.m.") {
            pm = Some(lower.starts_with('p'));
        } else if token.bytes().all(|b| b.is_ascii_digit()) {
            match token.len() {
                4 => year = token.parse::<i16>().ok(),
                1 | 2 if day.is_none() => day = token.parse::<i8>().ok(),
                _ => {}
            }
        } else if let [d, m, y] = token.split('.').collect::<Vec<_>>()[..]
            && let (Ok(d), Ok(m), Ok(y)) = (d.parse(), m.parse(), y.parse())
        {
            (day, mon, year) = (Some(d), Some(m), Some(y));
        } else if let Some(m) = month(token) {
            mon = Some(m);
        } else if let Some(o) = zone_offset(token) {
            offset = o;
        }
    }
    let (mut h, mi, sec) = hms?;
    match pm {
        Some(true) if h < 12 => h += 12,
        Some(false) if h == 12 => h = 0,
        _ => {}
    }
    let dt = jiff::civil::DateTime::new(year?, mon?, day?, h, mi, sec, 0).ok()?;
    let utc = dt.to_zoned(jiff::tz::TimeZone::UTC).ok()?.timestamp().as_millisecond();
    Some(utc - i64::from(offset) * 60_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_recognised_by_name_and_place() {
        let k = |p| kind_of(p);
        assert_eq!(k("Spotify Extended Streaming History/Streaming_History_Audio_2019-2020_0.json"), Some(Kind::Extended));
        assert_eq!(k("MyData/endsong_3.json"), Some(Kind::Extended));
        assert_eq!(k("Spotify Extended Streaming History/Streaming_History_Video_2020.json"), None);
        assert_eq!(k("MyData/StreamingHistory_music_0.json"), Some(Kind::OldHistory));
        assert_eq!(k("MyData/StreamingHistory0.json"), Some(Kind::OldHistory));
        assert_eq!(k("MyData/StreamingHistory_podcast_0.json"), None);
        assert_eq!(k("MyData/YourLibrary.json"), Some(Kind::Library));
        assert_eq!(k("MyData/Playlist1.json"), Some(Kind::SpotifyPlaylists));
        assert_eq!(k("Takeout/YouTube and YouTube Music/history/watch-history.json"), Some(Kind::YoutubeJson));
        assert_eq!(k("Takeout/YouTube и YouTube Music/история/история просмотра.json"), Some(Kind::YoutubeJson));
        assert_eq!(k("Takeout/YouTube и YouTube Music/плейлисты/Вечер-видео.csv"), Some(Kind::YoutubePlaylist));
        assert_eq!(k("Takeout/YouTube and YouTube Music/playlists/Liked music.csv"), Some(Kind::YoutubePlaylist));
        // Other CSVs and HTML pages are told by what they hold.
        assert_eq!(k("Takeout/YouTube and YouTube Music/subscriptions/subscriptions.csv"), Some(Kind::Csv));
        assert_eq!(k("Takeout/YouTube and YouTube Music/music (library and uploads)/music library songs.csv"), Some(Kind::Csv));
        assert_eq!(k("Temp/Made in Abyss OST 1-videos.csv"), Some(Kind::Csv));
        assert_eq!(k("Temp/watch-history.html"), Some(Kind::YoutubeHtml));
        assert_eq!(k("Temp/watch-history.json"), Some(Kind::YoutubeJson));
        assert_eq!(k("Temp/Clean It_ Restaurant Cleanup"), None);
        assert_eq!(k("notes/readme.txt"), None);
        assert_eq!(k("other/data.json"), None);
    }

    #[test]
    fn csv_and_html_are_told_by_their_start() {
        let s = |kind, head: &str| sniff(kind, head.as_bytes());
        assert_eq!(s(Kind::Csv, "Video ID,Playlist Video Creation Timestamp\nabc,1\n"), Some(Kind::YoutubePlaylist));
        assert_eq!(
            s(Kind::Csv, "\u{feff}Video ID,Song Title,Album Title,Artist Name 1,Artist Name 2\n"),
            Some(Kind::MusicLibrary)
        );
        assert_eq!(
            s(Kind::YoutubePlaylist, "Playlist ID,Add new videos to top,Playlist Title (Original)\n"),
            Some(Kind::PlaylistIndex)
        );
        assert_eq!(s(Kind::Csv, "Playlist Id,Channel Id,Time Created,Title\n"), Some(Kind::YoutubePlaylist));
        // The old layout's header is localised: in a playlist folder it still is one.
        assert_eq!(s(Kind::YoutubePlaylist, "Идентификатор плейлиста,Канал\n"), Some(Kind::YoutubePlaylist));
        assert_eq!(s(Kind::Csv, "Идентификатор плейлиста,Канал\n"), None);
        assert_eq!(s(Kind::Csv, "Title,Playable ID,Timestamp\nClean It,Ugkx,2025\n"), None);
        assert_eq!(s(Kind::Csv, "Channel Id,Channel Url,Channel Title\n"), None);
        assert_eq!(s(Kind::YoutubeHtml, "<html><style>.mdl-grid{}</style>"), Some(Kind::YoutubeHtml));
        assert_eq!(s(Kind::YoutubeHtml, "<html><body>hello</body></html>"), None);
    }

    /// An entry of a My Activity page.
    fn cell(header: &str, body: &str) -> String {
        format!(
            "<div class=\"outer-cell mdl-cell mdl-cell--12-col mdl-shadow--2dp\"><div class=\"mdl-grid\">\
             <div class=\"header-cell mdl-cell mdl-cell--12-col\"><p class=\"mdl-typography--title\">{header}<br></p></div>\
             <div class=\"content-cell mdl-cell mdl-cell--6-col mdl-typography--body-1\">{body}</div>\
             <div class=\"content-cell mdl-cell mdl-cell--6-col mdl-typography--body-1 mdl-typography--text-right\"></div>\
             <div class=\"content-cell mdl-cell mdl-cell--12-col mdl-typography--caption\"><b>Products:</b><br>&emsp;YouTube<br>\
             <a href=\"https://myaccount.google.com/activitycontrols\">here</a>.</div></div></div>"
        )
    }

    fn page(cells: &[String]) -> String {
        format!(
            "<html><head><style>.outer-cell{{}} .mdl-grid{{}}</style></head><body><div class=\"mdl-grid\">{}</div></body></html>",
            cells.concat()
        )
    }

    #[test]
    fn watch_history_page() {
        let html = page(&[
            cell(
                "YouTube Music",
                "Watched\u{a0}<a href=\"https://music.youtube.com/watch?v=fm7INtTR5Pk\">lazy cat - Eve (Live ver)</a><br>\
                 <a href=\"https://www.youtube.com/channel/UCxxxxxxxxxxxxxxxxxxxxxx\">Eve - Topic</a><br>\
                 Oct 15, 2026, 4:55:46\u{202f}PM IDT<br>",
            ),
            cell(
                "YouTube",
                "Watched\u{a0}<a href=\"https://www.youtube.com/watch?v=plainVideo1\">How to cook</a><br>\
                 <a href=\"https://www.youtube.com/channel/UCyyyyyyyyyyyyyyyyyyyyyy\">Blogger</a><br>\
                 Oct 15, 2026, 3:00:00\u{202f}PM IDT<br>",
            ),
            // A removed video: its url as the title, no channel.
            cell(
                "YouTube Music",
                "Watched\u{a0}<a href=\"https://music.youtube.com/watch?v=removedVid1\">https://music.youtube.com/watch?v=removedVid1</a><br>\
                 Jan 6, 2026, 12:46:24\u{202f}AM IDT<br>",
            ),
            cell(
                "YouTube Music",
                "Вы смотрели <a href=\"https://music.youtube.com/watch?v=gruppaKrovi&amp;t=3\">Rock &amp; Roll &#39;n&#39; &quot;Кино&quot;</a><br>\
                 <a href=\"https://www.youtube.com/channel/UCzzzzzzzzzzzzzzzzzzzzzz\">Кино</a><br>\
                 15 окт. 2026 г., 16:55:46 MSK<br>",
            ),
            cell("YouTube Music", "Searched for <a href=\"https://music.youtube.com/search?q=eve\">eve</a><br>Oct 15, 2026, 4:00:00 PM IDT<br>"),
            cell("Google Play", "Used <a href=\"https://play.google.com/watch?v=notyoutube1\">x</a><br>Oct 15, 2026, 4:00:00 PM IDT<br>"),
        ]);
        let mut out = ExportData::default();
        assert!(html_history(&html, &mut out));
        assert_eq!(out.plays.len(), 3);
        let p = &out.plays[0];
        assert_eq!(p.item.id.as_deref(), Some("fm7INtTR5Pk"));
        assert_eq!(p.item.title, "lazy cat - Eve (Live ver)");
        assert_eq!(p.item.artist, "Eve");
        assert_eq!(p.outcome, Outcome::Unknown);
        assert_eq!(p.start_ms, parse_time("2026-10-15T13:55:46Z").unwrap());
        assert_eq!(out.plays[1].item.title, "");
        assert_eq!(out.plays[1].item.artist, "");
        assert_eq!(out.plays[1].start_ms, parse_time("2026-01-05T21:46:24Z").unwrap());
        let kino = &out.plays[2];
        assert_eq!(kino.item.id.as_deref(), Some("gruppaKrovi"));
        assert_eq!(kino.item.title, "Rock & Roll 'n' \"Кино\"");
        assert_eq!(kino.start_ms, parse_time("2026-10-15T13:55:46Z").unwrap());
        assert_eq!(out.plain_watched, HashSet::from(["plainVideo1".to_owned()]));

        let mut out = ExportData::default();
        assert!(!html_history("<html><style>.mdl-grid{}</style><body>Search history</body></html>", &mut out));
    }

    #[test]
    fn activity_dates() {
        let t = |s: &str| parse_activity_time(s);
        let at = |s: &str| parse_time(s).unwrap();
        assert_eq!(t("Oct 15, 2026, 4:55:46\u{202f}PM IDT"), Some(at("2026-10-15T13:55:46Z")));
        assert_eq!(t("Oct 15, 2026, 4:55:46 PM IDT"), Some(at("2026-10-15T13:55:46Z")));
        assert_eq!(t("Jan 1, 2026, 12:05:00 AM EST"), Some(at("2026-01-01T05:05:00Z")));
        assert_eq!(t("Jan 1, 2026, 12:05:00 PM PST"), Some(at("2026-01-01T20:05:00Z")));
        assert_eq!(t("15 окт. 2026 г., 16:55:46 MSK"), Some(at("2026-10-15T13:55:46Z")));
        assert_eq!(t("3 мая 2025 г., 09:00:00 CEST"), Some(at("2025-05-03T07:00:00Z")));
        assert_eq!(t("3 мар. 2025 г., 09:00:00 CET"), Some(at("2025-03-03T08:00:00Z")));
        assert_eq!(t("15.10.2026, 16:55:46 GMT+03:00"), Some(at("2026-10-15T13:55:46Z")));
        assert_eq!(t("Oct 15, 2026, 4:55:46 PM GMT-04:30"), Some(at("2026-10-15T21:25:46Z")));
        assert_eq!(t("Oct 15, 2026, 4:55:46 PM UTC"), Some(at("2026-10-15T16:55:46Z")));
        // An unknown zone is UTC.
        assert_eq!(t("Oct 15, 2026, 4:55:46 PM XYZT"), Some(at("2026-10-15T16:55:46Z")));
        assert_eq!(t("2026-10-15T16:55:46Z"), Some(at("2026-10-15T16:55:46Z")));
        assert_eq!(t("вчера"), None);
        assert_eq!(t("Oct 2026"), None);
    }

    #[test]
    fn library_songs_are_likes_with_their_artists() {
        let csv = "Video ID,Song Title,Album Title,Artist Name 1,Artist Name 2,Artist Name 3\n\
                   abcdefghijk,Song,Album,Main,Guest,\n \
                   -Jn0EGU5S-g,\"Title, with comma\",,Solo,Solo,\n\
                   not an id,X,Y,Z,,\n";
        let songs = library_songs(csv);
        assert_eq!(songs.len(), 2);
        assert_eq!(songs[0].id.as_deref(), Some("abcdefghijk"));
        assert_eq!((songs[0].title.as_str(), songs[0].artist.as_str()), ("Song", "Main"));
        assert_eq!(songs[0].featured, ["Guest"]);
        assert_eq!(songs[0].album.as_deref(), Some("Album"));
        assert_eq!(songs[1].id.as_deref(), Some("-Jn0EGU5S-g"));
        assert_eq!(songs[1].title, "Title, with comma");
        assert!(songs[1].featured.is_empty());
        assert_eq!(songs[1].album, None);
    }

    #[test]
    fn loose_takeout_files() {
        let dir = tempfile::tempdir().unwrap();
        let put = |name: &str, text: &str| std::fs::write(dir.path().join(name), text).unwrap();
        put(
            "playlists.csv",
            "Playlist ID,Add new videos to top,Playlist Description (Original),Playlist Title (Original),Playlist Title (Original) Language,Playlist Visibility\n\
             PLaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa,False,,Made in Abyss OST 1,,Unlisted\n\
             PLbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb,False,,Re: Zero / Best,,Private\n\
             PLcccccccccccccccccccccccccccccccc,False,,Watch later,,Private\n",
        );
        put("Made in Abyss OST 1-videos.csv", "Video ID,Playlist Video Creation Timestamp\nsongAAAAAA1,2025-05-11T00:34:07+00:00\nsongBBBBBB2,2025-05-11T00:34:07+00:00\n");
        put("Re_ Zero _ Best-videos.csv", "Video ID,Playlist Video Creation Timestamp\nsongAAAAAA1,2025-05-11T00:34:07+00:00\n");
        put("Watch later-videos.csv", "Video ID,Playlist Video Creation Timestamp\nlaterVideo1,2026-04-03T03:58:22+00:00\n");
        // Takeout's older naming: `<title>.csv`, no index entry.
        put("Old one.csv", "Video ID,Playlist Video Creation Timestamp\nsongBBBBBB2,2020-01-01T00:00:00+00:00\n");
        put(
            "music library songs.csv",
            "Video ID,Song Title,Album Title,Artist Name 1,Artist Name 2\n songAAAAAA1,Hanezeve Caradhina,Made in Abyss OST,Kevin Penkin,\n",
        );
        put(
            "watch-history.html",
            &page(&[cell(
                "YouTube Music",
                "Watched\u{a0}<a href=\"https://music.youtube.com/watch?v=songBBBBBB2\">Underground River</a><br>\
                 <a href=\"https://www.youtube.com/channel/UCxxxxxxxxxxxxxxxxxxxxxx\">Kevin Penkin - Topic</a><br>\
                 Oct 15, 2026, 4:55:46\u{202f}PM IDT<br>",
            )]),
        );
        // Not export files: a game's save, a search history, notes.
        put("saves data.csv", "Title,Playable ID,Timestamp\nClean It: Restaurant Cleanup,Ugkxg96n,2025-11-09T00:16:16+00:00\n");
        put("Clean It_ Restaurant Cleanup", "{\"wallet\":{\"coins\":0}}");
        put("search-history.html", &page(&[cell("YouTube", "Searched for <a href=\"https://www.youtube.com/results?search_query=x\">x</a><br>Oct 15, 2026, 4:00:00 PM IDT<br>")]));
        put("notes.html", "<html><body>hi</body></html>");

        let data = read_export(dir.path()).unwrap();
        assert!(data.problems.is_empty(), "{:?}", data.problems);
        assert_eq!(data.plays.len(), 1);
        assert_eq!(data.likes.len(), 1);
        assert_eq!(data.likes[0].title, "Hanezeve Caradhina");
        assert_eq!(data.likes[0].album.as_deref(), Some("Made in Abyss OST"));
        let names: Vec<(&str, Option<&str>)> =
            data.playlists.iter().map(|p| (p.name.as_str(), p.remote.as_deref())).collect();
        assert_eq!(
            names,
            [
                ("Made in Abyss OST 1", Some("PLaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")),
                ("Old one", None),
                ("Re: Zero / Best", Some("PLbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")),
            ]
        );
        // Titles from the library (it wins) and from the history.
        let abyss = &data.playlists[0].items;
        assert_eq!((abyss[0].title.as_str(), abyss[0].artist.as_str()), ("Hanezeve Caradhina", "Kevin Penkin"));
        assert_eq!((abyss[1].title.as_str(), abyss[1].artist.as_str()), ("Underground River", "Kevin Penkin"));
        let mut files = data.files.clone();
        files.sort();
        assert_eq!(files.len(), 7, "{files:?}");
        assert!(files.iter().all(|f| !f.contains("saves") && !f.contains("search") && !f.contains("notes")));
    }

    #[test]
    fn times_titles_and_channels() {
        assert_eq!(parse_time("1970-01-01T00:00:01Z"), Some(1000));
        assert_eq!(parse_time("1970-01-01T00:00:01.5Z"), Some(1500));
        assert_eq!(parse_time("1970-01-01 00:01"), Some(60_000));
        assert_eq!(parse_time("вчера"), None);
        assert_eq!(watched_title("Watched One More Time"), "One More Time");
        assert_eq!(watched_title("Просмотрено: Группа крови"), "Группа крови");
        assert_eq!(watched_title("https://music.youtube.com/watch?v=abcdefghijk"), "");
        assert_eq!(channel_artist("Daft Punk - Topic"), "Daft Punk");
        assert_eq!(channel_artist("DaftPunkVEVO"), "DaftPunk");
        assert_eq!(channel_artist("Кино"), "Кино");
        assert_eq!(video_id("https://music.youtube.com/watch?v=yt-gruppa-1"), Some("yt-gruppa-1".into()));
        assert_eq!(video_id("https://www.youtube.com/watch?feature=x&v=abcdefghijk#t"), Some("abcdefghijk".into()));
        assert_eq!(video_id("https://music.youtube.com/watch?v=short"), None);
    }

    #[test]
    fn csv_handles_quotes_and_newlines() {
        let rows = csv_rows("a,\"b, c\",\"say \"\"hi\"\"\"\r\n\"multi\nline\",x\nlast");
        assert_eq!(rows[0], vec!["a", "b, c", "say \"hi\""]);
        assert_eq!(rows[1], vec!["multi\nline", "x"]);
        assert_eq!(rows[2], vec!["last"]);
    }

    #[test]
    fn extended_play_outcomes() {
        let play = |ms: u64, reason: &str, skipped: bool| {
            extended_play(&serde_json::json!({
                "ts": "2020-01-01T00:10:00Z", "ms_played": ms,
                "master_metadata_track_name": "T", "master_metadata_album_artist_name": "A",
                "master_metadata_album_album_name": "B", "spotify_track_uri": "spotify:track:X",
                "reason_end": reason, "skipped": skipped
            }))
            .unwrap()
        };
        let p = play(200_000, "trackdone", false);
        assert_eq!(p.outcome, Outcome::Complete);
        assert_eq!(p.item.id.as_deref(), Some("X"));
        assert_eq!(p.start_ms, parse_time("2020-01-01T00:10:00Z").unwrap() - 200_000);
        assert_eq!(play(10_000, "fwdbtn", false).outcome, Outcome::Skip);
        assert_eq!(play(90_000, "fwdbtn", true).outcome, Outcome::Skip);
        assert_eq!(play(90_000, "endplay", false).outcome, Outcome::Complete);
        assert_eq!(play(5_000, "trackdone", false).outcome, Outcome::Complete);
        // Podcast episodes have no track name.
        assert!(extended_play(&serde_json::json!({"ts": "2020-01-01T00:00:00Z", "episode_name": "E"})).is_none());
    }

    #[test]
    fn youtube_playlist_layouts() {
        let old = "Playlist Id,Channel Id,Time Created,Time Updated,Title,Description,Visibility\n\
                   PLabcdefghijklmnop,UCxxxxxxxxxxxxxxxxxxxxxx,2020-01-01,2020-01-02,Вечер,,Private\n\
                   \n\
                   Video Id,Time Added\n\
                   yt-gruppa-1,2020-01-01 10:00:00 UTC\n\
                   yt-michi-1 ,2020-01-01 10:00:00 UTC\n";
        let p = youtube_playlist("x/playlists/whatever.csv", old).unwrap();
        assert_eq!(p.name, "Вечер");
        assert_eq!(p.remote.as_deref(), Some("PLabcdefghijklmnop"));
        assert_eq!(p.items.len(), 2);
        let new = "Video ID,Playlist Video Creation Timestamp\nyt-gruppa-2,2023-01-01T00:00:00+00:00\n";
        let p = youtube_playlist("x/playlists/Liked music-videos.csv", new).unwrap();
        assert_eq!(p.name, "Liked music");
        assert_eq!(p.remote, None);
        assert_eq!(p.items[0].id.as_deref(), Some("yt-gruppa-2"));
        let index = "Playlist ID,Add new videos to top,Playlist Title (Original)\nPLabcdefghijklmnop,False,Вечер\n";
        assert!(youtube_playlist("x/playlists/playlists.csv", index).unwrap().items.is_empty());
    }
}
