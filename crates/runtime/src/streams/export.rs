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
//! | any `.json` under a `YouTube…` folder that is a list of `{header: "YouTube Music", titleUrl: …watch?v=…, time}` (`watch-history.json`, `история просмотра.json`) | YouTube Music plays |
//! | `.csv` in a playlist folder under a `YouTube…` folder (`playlists/`, `плейлисты/`) | playlists by video id; the liked-music one ([`LIKED_NAMES`]) gives likes |
//!
//! Everything here is plain parsing, without the library or the network;
//! the cold-start import ([`super::cold`]) turns the result into tracks,
//! events, likes and playlists.

use std::collections::HashMap;
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
    pub(crate) album: Option<String>,
}

impl Item {
    fn new(service: Service, id: Option<String>, title: &str, artist: &str, album: Option<&str>) -> Self {
        Self {
            service,
            id: id.filter(|i| !i.is_empty()),
            title: title.trim().to_owned(),
            artist: artist.trim().to_owned(),
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
    YoutubePlaylist,
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
        if in_youtube {
            return Some(Kind::YoutubeJson);
        }
        if stem.starts_with("playlist") {
            return Some(Kind::SpotifyPlaylists);
        }
        return None;
    }
    if name.ends_with(".csv") && in_youtube {
        let dir = parts.last().copied().unwrap_or("");
        let playlist_dir = ["playlist", "плейлист", "списки", "wiedergabeliste", "lista", "liste"]
            .iter()
            .any(|w| dir.contains(w));
        return playlist_dir.then_some(Kind::YoutubePlaylist);
    }
    None
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
        let kind = kind_of(&name).ok_or_else(|| format!("«{name}» — не файл выгрузки"))?;
        files.push((name, kind, read_file(path)?));
    }
    let mut out = ExportData::default();
    let has_extended = files.iter().any(|(_, k, _)| *k == Kind::Extended);
    // The extended history repeats the account-data one in finer detail.
    files.retain(|(_, k, _)| !(has_extended && *k == Kind::OldHistory));
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut yt_playlists = Vec::new();
    for (name, kind, bytes) in files {
        let parsed = match kind {
            Kind::YoutubePlaylist => {
                let text = String::from_utf8_lossy(&bytes);
                yt_playlists.push(youtube_playlist(&name, &text));
                Ok(true)
            }
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
    // The liked-music playlist is the likes; empty lists (the playlist
    // index file, a list of playlist ids) are nothing.
    for p in yt_playlists.into_iter().flatten() {
        if LIKED_NAMES.contains(&fold(&p.name).as_str()) {
            out.likes.extend(p.items);
        } else if !p.items.is_empty() {
            out.playlists.push(p);
        }
    }
    fill_youtube_titles(&mut out);
    Ok(out)
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
            if let Some(kind) = kind_of(&named) {
                out.push((named, kind, read_file(&p)?));
            }
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
            let plays: Vec<Play> = json.as_array().into_iter().flatten().filter_map(watched).collect();
            let found = !plays.is_empty();
            out.plays.extend(plays);
            found
        }
        Kind::YoutubePlaylist => false,
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

/// Video ids of likes and playlists have no title in the CSVs: take the one
/// the watch history knows.
fn fill_youtube_titles(out: &mut ExportData) {
    let known: HashMap<String, (String, String)> = out
        .plays
        .iter()
        .filter(|p| p.item.service == Service::Youtube && !p.item.title.is_empty())
        .filter_map(|p| Some((p.item.id.clone()?, (p.item.title.clone(), p.item.artist.clone()))))
        .collect();
    let fill = |i: &mut Item| {
        if i.title.is_empty()
            && let Some((t, a)) = i.id.as_ref().and_then(|id| known.get(id))
        {
            i.title.clone_from(t);
            i.artist.clone_from(a);
        }
    };
    out.likes.iter_mut().for_each(fill);
    for p in &mut out.playlists {
        p.items.iter_mut().for_each(fill);
    }
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
        assert_eq!(k("Takeout/YouTube and YouTube Music/subscriptions/subscriptions.csv"), None);
        assert_eq!(k("Takeout/YouTube and YouTube Music/music (library and uploads)/music library songs.csv"), None);
        assert_eq!(k("notes/readme.txt"), None);
        assert_eq!(k("other/data.json"), None);
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
