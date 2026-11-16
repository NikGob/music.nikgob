//! The tools: 18 in four groups of rights, each a thin call (or a few)
//! into the REST API. A session sees only the tools its token can use:
//! «Только чтение» the reading ones, «Управление» also the player's,
//! «Полный» all of them. Answers are compact JSON (no covers, no nulls,
//! lists cut with their totals said).

use nikgob_core::settings::ApiPreset;
use serde_json::{Map, Value, json};

use crate::backend::{ApiRequest, ApiResponse, Backend, Unreachable, encode, with_query};

/// The rights a tool needs, as one group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Contour {
    /// Reads: every token.
    Read,
    /// The player, the queue, the output: «Управление» and «Полный».
    Control,
    /// Changes that add or can be undone: «Полный».
    Library,
    /// Changes of files and playlists, with a confirmation: «Полный».
    Destructive,
}

impl Contour {
    pub fn allowed(self, preset: ApiPreset) -> bool {
        match self {
            Self::Read => true,
            Self::Control => matches!(preset, ApiPreset::Control | ApiPreset::Full),
            Self::Library | Self::Destructive => preset == ApiPreset::Full,
        }
    }
}

/// One tool.
pub struct ToolDef {
    pub name: &'static str,
    pub title: &'static str,
    pub contour: Contour,
    pub description: &'static str,
    pub schema: fn() -> Value,
}

/// Every tool, in the order they are listed.
pub const TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "status_get",
        title: "Now playing",
        contour: Contour::Read,
        description: "What the player is doing now: the track, position (position_ms as of at_ms; it moves at `rate`), volume, mute, shuffle, repeat, duck, output, and the next tracks of the queue. Call it first when you need context.",
        schema: schema_status,
    },
    ToolDef {
        name: "library_search",
        title: "Search the library",
        contour: Contour::Read,
        description: "Search the user's own library (tracks, albums, artists, playlists) by words of a title or name. Use the ids it returns with the other tools. For music the library does not have, use discover_search.",
        schema: schema_search,
    },
    ToolDef {
        name: "library_get",
        title: "Read from the library",
        contour: Contour::Read,
        description: "Read one thing of the library by id: a track (with its lyrics if include_lyrics), an album or an artist with their tracks, a playlist with its tracks (paged with offset/limit), the list of playlists (kind=playlists, no id), or a track's lyrics (kind=lyrics). Playlist id `liked` is «Любимые».",
        schema: schema_get,
    },
    ToolDef {
        name: "discover_search",
        title: "Search the services",
        contour: Contour::Read,
        description: "Look music up on the connected services (Spotify, YouTube Music, Deezer, SoundCloud). mode=search: tracks, albums, artists, playlists with `in_library` and the library id when the user has it. mode=artist_discography: what a library artist released that the library lacks. mode=share_link: a song.link/album.link page for a library track or album. Slow and rate-limited: do not loop over it.",
        schema: schema_discover,
    },
    ToolDef {
        name: "stats_query",
        title: "Listening and library analytics",
        contour: Contour::Read,
        description: "Analytics. report=listening: plays, hours, completions vs early skips, top tracks/artists/albums, hours of day, devices for a period (days, or from_ms/to_ms). report=forgotten_gems: songs liked or played a lot that were not played for before_days. report=library_health: sizes, formats (lossless/opus/mp3), tracks without synced lyrics, fake lossless files, masters without a copy. report=similar_tracks: the library's nearest tracks by sound to track_id (default: the one playing).",
        schema: schema_stats,
    },
    ToolDef {
        name: "downloads_inspect",
        title: "Downloads",
        contour: Contour::Read,
        description: "The downloads: active jobs, the «Хочу скачать» wishlist, and the «Подтверди» queue waiting for the user. With search_track_id or search_album_id: search the download sources for that library track or album without downloading anything (slow).",
        schema: schema_downloads,
    },
    ToolDef {
        name: "outputs_list",
        title: "Audio outputs",
        contour: Contour::Read,
        description: "The audio outputs the player can use, with their ids and which one is in use, then the other devices of the home online (kind node, id node:<id>, whether they play). Pass an id (or part of a name) to output_select or player_play.output; a node's id to target_switch.",
        schema: schema_empty,
    },
    ToolDef {
        name: "player_play",
        title: "Play",
        contour: Contour::Control,
        description: "Start music: by a free query (an artist, album, song, playlist; «моя волна» / «любимые» work too) with optional kind, or exactly by id+kind (from library_search). mode: replace the queue (default), next (after the current track) or enqueue (at the end). shuffle: null = as the app does. output: switch the output first. No query and no id resumes.",
        schema: schema_play,
    },
    ToolDef {
        name: "player_control",
        title: "Control the player",
        contour: Contour::Control,
        description: "Transport and settings in one call; every field is optional and they combine (applied in this order): transport (play, pause, toggle, next, previous), seek_ms or seek_delta_ms, volume (0..1) or volume_delta, muted, shuffle, repeat (off, all, one), duck ({gain, fade_ms, ttl_ms} lowers the sound for a while; null brings it back).",
        schema: schema_control,
    },
    ToolDef {
        name: "queue_modify",
        title: "Change the queue",
        contour: Contour::Control,
        description: "Change the play queue. action=play_next / enqueue: add track_ids after the current track / at the end. remove: take out entry. move: put entry after (or before, after=false) anchor. jump: play entry now. clear: empty the queue. Entries are the `entry` numbers status_get returns.",
        schema: schema_queue,
    },
    ToolDef {
        name: "output_select",
        title: "Switch the output",
        contour: Contour::Control,
        description: "Switch where the sound goes: an id from outputs_list, a part of a device name, or `local:default` for the system default.",
        schema: schema_output,
    },
    ToolDef {
        name: "target_switch",
        title: "Play on another device",
        contour: Contour::Control,
        description: "Play on another device of the home, like Spotify Connect. target: a node id from outputs_list (`node:<id>`), a device's name, or `this` to bring the music back here. mode: transfer (default) moves the queue, the track and its position there and pauses here; attach only drives what that device already plays, moving nothing.",
        schema: schema_target,
    },
    ToolDef {
        name: "library_feedback",
        title: "Like and recommend",
        contour: Contour::Library,
        description: "Reversible feedback on a track, album or artist (a library id, or a service id `service:id` for a track or album): like / unlike, dislike / undislike (a track: «Не нравится»), dont_recommend / recommend (a track or artist: kept out of mixes and radio), add_to_library / remove_from_library (a track or album). The app shows its usual «Отменить».",
        schema: schema_feedback,
    },
    ToolDef {
        name: "playlist_add",
        title: "Create or extend a playlist",
        contour: Contour::Library,
        description: "Add to a playlist (only adds): give playlist_id to append to an existing one (`liked` likes the tracks), or name to create a new one. Add track_ids, an album_id (its tracks in order) or remote_track_ids (`service:id` from discover_search).",
        schema: schema_playlist_add,
    },
    ToolDef {
        name: "downloads_enqueue",
        title: "Download",
        contour: Contour::Library,
        description: "Ask the player to download library tracks or albums without a file, or service tracks/albums (`service:id`). A small request (up to 3 releases, about 1.5 GB) goes to «Хочу скачать» at once; a larger one waits in the app's «Подтверди» queue for the user to click (status queued_in_confirm).",
        schema: schema_enqueue,
    },
    ToolDef {
        name: "playlist_remove",
        title: "Remove from, rename or delete a playlist",
        contour: Contour::Destructive,
        description: "Destructive, two steps: called without confirm_token it changes nothing and answers the exact plan with a confirm_token (valid 5 minutes, for exactly these arguments); show the plan to the user and call again with the same arguments and the token to do it. If the playlist changed meanwhile, the answer is plan_changed with the new plan and a new token. At most 50 tracks per call. action=remove_tracks (track_ids), rename (name) or delete.",
        schema: schema_playlist_remove,
    },
    ToolDef {
        name: "library_retag",
        title: "Change tags",
        contour: Contour::Destructive,
        description: "Destructive, two steps: writes tags (title, artist, album, album_artist, year, track_no, disc_no, genre) into the files of track_ids and the library; fields not given stay. Without confirm_token it answers the plan (before → after per track) and a confirm_token; call again with the same arguments and the token to write. A file changed since the plan is not written (plan_changed or a failure). At most 50 tracks per call; files outside the library are listed apart and never touched.",
        schema: schema_retag,
    },
    ToolDef {
        name: "library_trash",
        title: "Move to the Trash",
        contour: Contour::Destructive,
        description: "Destructive, two steps: moves track_ids and/or album_ids to the library's Trash (kept 30 days, the app can restore them; nothing is ever deleted for good). Without confirm_token it answers the plan and a confirm_token; call again with the same arguments and the token. At most 50 tracks per call (albums count by their tracks); if what would go changed meanwhile, the answer is plan_changed with the new plan and a new token. Files outside the library are never touched.",
        schema: schema_trash,
    },
];

/// A tool by its name.
pub fn find(name: &str) -> Option<&'static ToolDef> {
    TOOLS.iter().find(|t| t.name == name)
}

/// Whether a session of `preset` sees `tool`; an unknown preset (the
/// player could not be asked yet) sees everything, and the API refuses
/// what the token cannot do.
pub fn visible(tool: &ToolDef, preset: Option<ApiPreset>) -> bool {
    preset.is_none_or(|p| tool.contour.allowed(p))
}

/// The `tools/list` entries for `preset`.
pub fn list(preset: Option<ApiPreset>) -> Vec<Value> {
    TOOLS
        .iter()
        .filter(|t| visible(t, preset))
        .map(|t| {
            let read = t.contour == Contour::Read;
            json!({
                "name": t.name,
                "title": t.title,
                "description": t.description,
                "inputSchema": (t.schema)(),
                "annotations": {
                    "title": t.title,
                    "readOnlyHint": read,
                    "destructiveHint": t.contour == Contour::Destructive,
                    "idempotentHint": read,
                    "openWorldHint": matches!(t.name, "discover_search" | "downloads_inspect" | "downloads_enqueue"),
                }
            })
        })
        .collect()
}

// ---------------------------------------------------------------- schemas

fn obj(props: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": props, "required": required, "additionalProperties": false})
}

fn ids(what: &str) -> Value {
    json!({"type": "array", "items": {"type": "string"}, "description": what})
}

fn schema_empty() -> Value {
    obj(json!({}), &[])
}

fn schema_status() -> Value {
    obj(json!({"queue_limit": {"type": "integer", "minimum": 0, "maximum": 50, "description": "Next tracks of the queue to include (10 by default)."}}), &[])
}

fn schema_search() -> Value {
    obj(
        json!({
            "query": {"type": "string", "description": "Words of a title or a name."},
            "kind": {"type": "string", "enum": ["any", "track", "album", "artist", "playlist"]},
            "limit": {"type": "integer", "minimum": 1, "maximum": 50, "description": "Per kind, 10 by default."},
            "offset": {"type": "integer", "minimum": 0, "description": "Skip this many of each kind (offset + limit at most 250); `track_total` says how many tracks match."}
        }),
        &["query"],
    )
}

fn schema_get() -> Value {
    obj(
        json!({
            "kind": {"type": "string", "enum": ["track", "album", "artist", "playlist", "playlists", "lyrics"]},
            "id": {"type": "string", "description": "Not needed for playlists."},
            "include_lyrics": {"type": "boolean", "description": "For a track."},
            "offset": {"type": "integer", "minimum": 0},
            "limit": {"type": "integer", "minimum": 1, "maximum": 500, "description": "Tracks of an album, artist or playlist, 100 by default."}
        }),
        &["kind"],
    )
}

fn schema_discover() -> Value {
    obj(
        json!({
            "mode": {"type": "string", "enum": ["search", "artist_discography", "share_link"], "description": "search by default."},
            "query": {"type": "string", "description": "For search."},
            "kind": {"type": "string", "enum": ["any", "track", "album", "artist", "playlist"], "description": "For search."},
            "services": {"type": "array", "items": {"type": "string", "enum": ["spotify", "youtube", "deezer", "soundcloud"]}, "description": "For search: all connected ones by default."},
            "limit": {"type": "integer", "minimum": 1, "maximum": 25, "description": "Per kind and service, 5 by default."},
            "artist_id": {"type": "string", "description": "For artist_discography: a library artist id."},
            "refresh": {"type": "boolean", "description": "For artist_discography: look it up again now."},
            "target_kind": {"type": "string", "enum": ["track", "album"], "description": "For share_link."},
            "target_id": {"type": "string", "description": "For share_link: a library track or album id, or `service:id`."}
        }),
        &[],
    )
}

fn schema_stats() -> Value {
    obj(
        json!({
            "report": {"type": "string", "enum": ["listening", "forgotten_gems", "library_health", "similar_tracks"]},
            "days": {"type": "integer", "minimum": 1, "maximum": 3650, "description": "listening: the last N days (30 by default)."},
            "from_ms": {"type": "integer", "description": "listening: period start, unix ms."},
            "to_ms": {"type": "integer", "description": "listening: period end, unix ms."},
            "before_days": {"type": "integer", "minimum": 1, "description": "forgotten_gems: not played for this many days (90 by default)."},
            "min_plays": {"type": "integer", "minimum": 1, "description": "forgotten_gems: played at least this often (5 by default), or liked."},
            "track_id": {"type": "string", "description": "similar_tracks: the seed (the playing track by default)."},
            "limit": {"type": "integer", "minimum": 1, "maximum": 100}
        }),
        &["report"],
    )
}

fn schema_downloads() -> Value {
    obj(
        json!({
            "search_track_id": {"type": "string", "description": "Search the sources for this library track (nothing is downloaded)."},
            "search_album_id": {"type": "string", "description": "Search the sources for this library album (nothing is downloaded)."}
        }),
        &[],
    )
}

fn schema_play() -> Value {
    obj(
        json!({
            "query": {"type": "string"},
            "id": {"type": "string", "description": "Exact id (then kind is needed)."},
            "kind": {"type": "string", "enum": ["any", "track", "album", "artist", "playlist", "liked", "wave"]},
            "mode": {"type": "string", "enum": ["replace", "next", "enqueue"]},
            "shuffle": {"type": ["boolean", "null"]},
            "output": {"type": "string", "description": "An output id or a part of its name."}
        }),
        &[],
    )
}

fn schema_control() -> Value {
    obj(
        json!({
            "transport": {"type": "string", "enum": ["play", "pause", "toggle", "next", "previous"]},
            "seek_ms": {"type": "integer", "minimum": 0},
            "seek_delta_ms": {"type": "integer"},
            "volume": {"type": "number", "minimum": 0, "maximum": 1},
            "volume_delta": {"type": "number", "minimum": -1, "maximum": 1},
            "muted": {"type": "boolean"},
            "shuffle": {"type": "boolean"},
            "repeat": {"type": "string", "enum": ["off", "all", "one"]},
            "duck": {
                "type": ["object", "null"],
                "properties": {
                    "gain": {"type": "number", "minimum": 0, "maximum": 1},
                    "fade_ms": {"type": "integer", "minimum": 0, "maximum": 5000},
                    "ttl_ms": {"type": "integer", "minimum": 100, "maximum": 60000}
                },
                "required": ["gain"]
            }
        }),
        &[],
    )
}

fn schema_queue() -> Value {
    obj(
        json!({
            "action": {"type": "string", "enum": ["play_next", "enqueue", "remove", "move", "jump", "clear"]},
            "track_ids": ids("For play_next and enqueue."),
            "entry": {"type": "integer", "minimum": 0},
            "anchor": {"type": "integer", "minimum": 0},
            "after": {"type": "boolean"}
        }),
        &["action"],
    )
}

fn schema_output() -> Value {
    obj(json!({"output": {"type": "string"}}), &["output"])
}

fn schema_target() -> Value {
    obj(
        json!({
            "target": {"type": "string", "description": "`node:<id>` from outputs_list, a device's name, or `this`."},
            "mode": {"type": "string", "enum": ["transfer", "attach"]}
        }),
        &["target"],
    )
}

fn schema_feedback() -> Value {
    obj(
        json!({
            "action": {"type": "string", "enum": ["like", "unlike", "dislike", "undislike", "dont_recommend", "recommend", "add_to_library", "remove_from_library"]},
            "kind": {"type": "string", "enum": ["track", "album", "artist"]},
            "id": {"type": "string"}
        }),
        &["action", "kind", "id"],
    )
}

fn schema_playlist_add() -> Value {
    obj(
        json!({
            "playlist_id": {"type": "string", "description": "An existing playlist, or `liked`."},
            "name": {"type": "string", "description": "A new playlist's name (when no playlist_id)."},
            "track_ids": ids("Library track ids."),
            "album_id": {"type": "string"},
            "remote_track_ids": ids("Service tracks, `service:id`.")
        }),
        &[],
    )
}

fn schema_enqueue() -> Value {
    obj(
        json!({
            "track_ids": ids("Library tracks."),
            "album_ids": ids("Library albums."),
            "remote_track_ids": ids("Service tracks, `service:id`."),
            "remote_album_ids": ids("Service albums, `service:id`.")
        }),
        &[],
    )
}

fn token_prop() -> Value {
    json!({"type": "string", "description": "From the first call's answer, to do it."})
}

fn schema_playlist_remove() -> Value {
    obj(
        json!({
            "playlist_id": {"type": "string"},
            "action": {"type": "string", "enum": ["remove_tracks", "rename", "delete"]},
            "track_ids": ids("For remove_tracks."),
            "name": {"type": "string", "description": "For rename."},
            "confirm_token": token_prop()
        }),
        &["playlist_id", "action"],
    )
}

fn schema_retag() -> Value {
    let field = json!({"type": "string"});
    obj(
        json!({
            "track_ids": ids("Library tracks with a file."),
            "changes": {
                "type": "object",
                "properties": {
                    "title": field, "artist": field, "album": field, "album_artist": field,
                    "year": field, "track_no": field, "disc_no": field, "genre": field
                },
                "additionalProperties": false
            },
            "confirm_token": token_prop()
        }),
        &["track_ids", "changes"],
    )
}

fn schema_trash() -> Value {
    obj(json!({"track_ids": ids("Library tracks."), "album_ids": ids("Library albums."), "confirm_token": token_prop()}), &[])
}

// ---------------------------------------------------------------- calls

/// Why a tool call failed.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolError {
    /// The arguments do not make a request.
    Bad(String),
    /// The API refused it (its status and error body).
    Api { status: u16, body: Value },
    Unreachable(Unreachable),
}

impl ToolError {
    /// The error as the tool's answer.
    pub fn to_value(&self) -> Value {
        match self {
            Self::Bad(m) => json!({"error": "bad_arguments", "message": m}),
            Self::Api { status, body } => {
                let mut v = if body.is_object() { body.clone() } else { json!({"error": "api", "message": body}) };
                if let Some(m) = v.as_object_mut() {
                    m.insert("status".to_owned(), json!(status));
                }
                v
            }
            Self::Unreachable(u) => json!({"error": "player_unreachable", "message": u.message()}),
        }
    }
}

type Out = Result<Value, ToolError>;

fn bad(m: impl Into<String>) -> ToolError {
    ToolError::Bad(m.into())
}

/// A text argument, trimmed, not empty.
fn text(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
}

fn int(args: &Value, key: &str) -> Option<i64> {
    args.get(key).and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
}

fn num(args: &Value, key: &str) -> Option<f64> {
    args.get(key).and_then(Value::as_f64)
}

fn flag(args: &Value, key: &str) -> Option<bool> {
    args.get(key).and_then(Value::as_bool)
}

fn list_of(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect())
        .unwrap_or_default()
}

fn need(args: &Value, key: &str) -> Result<String, ToolError> {
    text(args, key).ok_or_else(|| bad(format!("`{key}` is required")))
}

/// Takes the API's answer: its JSON, or the error it said.
fn answer(r: Result<ApiResponse, Unreachable>) -> Out {
    let r = r.map_err(ToolError::Unreachable)?;
    if r.ok() { Ok(r.body) } else { Err(ToolError::Api { status: r.status, body: r.body }) }
}

async fn send<B: Backend>(b: &B, req: ApiRequest) -> Out {
    answer(b.call(req).await)
}

/// The JSON without nulls and without cover links (an agent has no use for
/// them), recursively.
pub fn compact(v: Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.into_iter().filter(|(k, v)| !v.is_null() && k != "cover").map(|(k, v)| (k, compact(v))).collect::<Map<String, Value>>(),
        ),
        Value::Array(a) => Value::Array(a.into_iter().map(compact).collect()),
        other => other,
    }
}

/// `list[key]` cut to `offset..offset+limit`, with its total and the
/// window said beside it.
fn page(mut v: Value, key: &str, offset: usize, limit: usize) -> Value {
    if let Some(m) = v.as_object_mut()
        && let Some(Value::Array(all)) = m.remove(key)
    {
        let total = all.len();
        let part: Vec<Value> = all.into_iter().skip(offset).take(limit).collect();
        let shown = part.len();
        m.insert(key.to_owned(), Value::Array(part));
        m.insert(format!("{key}_total"), json!(total));
        if shown < total {
            m.insert(format!("{key}_window"), json!({"offset": offset, "count": shown}));
        }
    }
    v
}

/// A page the API cut already: `tracks_total` (of `total`) and the window
/// beside it when it is not all of them.
fn served_page(mut v: Value, total: usize, offset: usize) -> Value {
    if let Some(m) = v.as_object_mut() {
        let shown = m.get("tracks").and_then(Value::as_array).map_or(0, Vec::len);
        m.remove("tracks_offset");
        m.insert("tracks_total".to_owned(), json!(total));
        if shown < total {
            m.insert("tracks_window".to_owned(), json!({"offset": offset, "count": shown}));
        }
    }
    v
}

fn usize_of(v: Option<i64>, default: usize, max: usize) -> usize {
    v.map_or(default, |n| usize::try_from(n.max(0)).unwrap_or(default)).min(max)
}

/// Runs `name` with `args` against the API.
pub async fn call<B: Backend>(b: &B, name: &str, args: &Value) -> Out {
    let empty = json!({});
    let args = if args.is_object() { args } else { &empty };
    let out = match name {
        "status_get" => status_get(b, args).await,
        "library_search" => {
            let q = need(args, "query")?;
            let path = with_query(
                "/v1/search",
                &[
                    ("q", Some(q)),
                    ("kind", text(args, "kind")),
                    ("limit", int(args, "limit").map(|n| n.to_string())),
                    ("offset", int(args, "offset").map(|n| n.to_string())),
                ],
            );
            send(b, ApiRequest::get(path)).await
        }
        "library_get" => library_get(b, args).await,
        "discover_search" => discover(b, args).await,
        "stats_query" => stats(b, args).await,
        "downloads_inspect" => {
            let path = match (text(args, "search_track_id"), text(args, "search_album_id")) {
                (Some(t), _) => with_query("/v1/downloads/sources", &[("track_id", Some(t))]),
                (None, Some(a)) => with_query("/v1/downloads/sources", &[("album_id", Some(a))]),
                (None, None) => "/v1/downloads".to_owned(),
            };
            send(b, ApiRequest::get(path)).await
        }
        "outputs_list" => send(b, ApiRequest::get("/v1/outputs")).await,
        "player_play" => {
            let mut body = Map::new();
            for k in ["query", "id", "kind", "mode", "output"] {
                if let Some(v) = text(args, k) {
                    body.insert(k.to_owned(), Value::String(v));
                }
            }
            if let Some(s) = flag(args, "shuffle") {
                body.insert("shuffle".to_owned(), Value::Bool(s));
            }
            send(b, ApiRequest::post("/v1/player/play", Value::Object(body))).await
        }
        "player_control" => control(b, args).await,
        "queue_modify" => queue(b, args).await,
        "output_select" => send(b, ApiRequest::put("/v1/player/output", json!({"output": need(args, "output")?}))).await,
        "target_switch" => {
            let attach = text(args, "mode").as_deref() == Some("attach");
            send(b, ApiRequest::put("/v1/player/output", json!({"output": need(args, "target")?, "attach": attach}))).await
        }
        "library_feedback" => {
            let body = json!({"action": need(args, "action")?, "kind": need(args, "kind")?, "id": need(args, "id")?});
            send(b, ApiRequest::post("/v1/library/feedback", body)).await
        }
        "playlist_add" => playlist_add(b, args).await,
        "downloads_enqueue" => {
            let body = json!({
                "track_ids": list_of(args, "track_ids"),
                "album_ids": list_of(args, "album_ids"),
                "remote_track_ids": list_of(args, "remote_track_ids"),
                "remote_album_ids": list_of(args, "remote_album_ids"),
            });
            send(b, ApiRequest::post("/v1/downloads", body)).await
        }
        "playlist_remove" => playlist_remove(b, args).await,
        "library_retag" => {
            let mut body = json!({"track_ids": list_of(args, "track_ids"), "changes": args.get("changes").cloned().unwrap_or_else(|| json!({}))});
            if let Some(t) = text(args, "confirm_token") {
                body["confirm_token"] = Value::String(t);
            }
            send(b, ApiRequest::post("/v1/library/retag", body)).await
        }
        "library_trash" => {
            let mut body = json!({"track_ids": list_of(args, "track_ids"), "album_ids": list_of(args, "album_ids")});
            if let Some(t) = text(args, "confirm_token") {
                body["confirm_token"] = Value::String(t);
            }
            send(b, ApiRequest::post("/v1/library/trash", body)).await
        }
        other => Err(bad(format!("No tool `{other}`"))),
    };
    out.map(compact)
}

/// A track as the lists show it: what an agent needs to name and play it.
fn brief(t: &Value) -> Value {
    let mut out = Map::new();
    for k in ["id", "title", "artists", "album", "album_id", "duration_ms", "liked", "local"] {
        if let Some(v) = t.get(k) {
            out.insert(k.to_owned(), v.clone());
        }
    }
    Value::Object(out)
}

/// The queue: the current entry and the first `limit` next ones.
fn queue_brief(q: &Value, limit: usize) -> Value {
    let item = |e: &Value| json!({"entry": e.get("entry"), "track": e.get("track").map(brief)});
    let next: Vec<Value> = q.get("next").and_then(Value::as_array).map(|a| a.iter().take(limit).map(item).collect()).unwrap_or_default();
    json!({
        "current": q.get("current").filter(|c| !c.is_null()).map(item),
        "next": next,
        "next_total": q.get("next").and_then(Value::as_array).map_or(0, Vec::len),
        "prev_total": q.get("prev").and_then(Value::as_array).map_or(0, Vec::len),
    })
}

async fn status_get<B: Backend>(b: &B, args: &Value) -> Out {
    let limit = usize_of(int(args, "queue_limit"), 10, 50);
    let mut state = send(b, ApiRequest::get("/v1/state")).await?;
    let q = send(b, ApiRequest::get("/v1/queue")).await?;
    if let Some(m) = state.as_object_mut() {
        m.insert("queue".to_owned(), queue_brief(&q, limit));
    }
    Ok(state)
}

async fn library_get<B: Backend>(b: &B, args: &Value) -> Out {
    let kind = need(args, "kind")?;
    let offset = usize_of(int(args, "offset"), 0, usize::MAX);
    let limit = usize_of(int(args, "limit"), 100, 500);
    let id = || need(args, "id").map(|i| encode(&i));
    match kind.as_str() {
        "playlists" => send(b, ApiRequest::get("/v1/playlists")).await,
        "track" => {
            let id = id()?;
            let mut t = send(b, ApiRequest::get(format!("/v1/tracks/{id}"))).await?;
            if flag(args, "include_lyrics") == Some(true) {
                let lyrics = match send(b, ApiRequest::get(format!("/v1/tracks/{id}/lyrics"))).await {
                    Ok(l) => lyrics_brief(&l),
                    Err(ToolError::Api { status: 404, .. }) => Value::Null,
                    Err(e) => return Err(e),
                };
                t["lyrics"] = lyrics;
            }
            Ok(t)
        }
        "lyrics" => send(b, ApiRequest::get(format!("/v1/tracks/{}/lyrics", id()?))).await.map(|l| lyrics_brief(&l)),
        "album" => {
            let a = send(b, ApiRequest::get(format!("/v1/albums/{}", id()?))).await?;
            Ok(page(brief_tracks(a), "tracks", offset, limit))
        }
        // The API pages these itself: the whole list is never fetched.
        "artist" => {
            let path = with_query(&format!("/v1/artists/{}", id()?), &[("offset", Some(offset.to_string())), ("limit", Some(limit.to_string()))]);
            let mut a = send(b, ApiRequest::get(path)).await?;
            let total = a.get("tracks_total").and_then(Value::as_u64).unwrap_or(0) as usize;
            if let Some(m) = a.as_object_mut() {
                m.remove("tracks_total");
            }
            Ok(served_page(brief_tracks(a), total, offset))
        }
        "playlist" => {
            let path = with_query(&format!("/v1/playlists/{}", id()?), &[("offset", Some(offset.to_string())), ("limit", Some(limit.to_string()))]);
            let p = send(b, ApiRequest::get(path)).await?;
            let total = p.pointer("/playlist/track_count").and_then(Value::as_u64).unwrap_or(0) as usize;
            Ok(served_page(brief_tracks(p), total, offset))
        }
        other => Err(bad(format!("Unknown kind `{other}`"))),
    }
}

/// `tracks` of an answer as [`brief`] tracks.
fn brief_tracks(mut v: Value) -> Value {
    if let Some(Value::Array(ts)) = v.get_mut("tracks") {
        for t in ts.iter_mut() {
            *t = brief(t);
        }
    }
    v
}

/// The lyrics as text an agent reads: each line with its start, no word
/// timing.
pub fn lyrics_brief(l: &Value) -> Value {
    let lines: Vec<Value> = l
        .get("lines")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|x| json!({"start_ms": x.get("start_ms"), "text": x.get("text")})).collect())
        .unwrap_or_default();
    json!({
        "source": l.get("source"),
        "timing": l.get("timing"),
        "lines": lines,
        "plain": l.get("plain"),
        "instrumental": l.get("instrumental"),
    })
}

async fn discover<B: Backend>(b: &B, args: &Value) -> Out {
    match text(args, "mode").as_deref().unwrap_or("search") {
        "search" => {
            let services = list_of(args, "services");
            let path = with_query(
                "/v1/discover/search",
                &[
                    ("q", Some(need(args, "query")?)),
                    ("kind", text(args, "kind")),
                    ("services", (!services.is_empty()).then(|| services.join(","))),
                    ("limit", int(args, "limit").map(|n| n.to_string())),
                ],
            );
            send(b, ApiRequest::get(path)).await
        }
        "artist_discography" => {
            let id = encode(&need(args, "artist_id")?);
            let path = with_query(&format!("/v1/discover/artists/{id}/discography"), &[("refresh", flag(args, "refresh").filter(|r| *r).map(|_| "true".to_owned()))]);
            send(b, ApiRequest::get(path)).await
        }
        "share_link" => {
            let path = with_query(
                "/v1/discover/share",
                &[("kind", Some(text(args, "target_kind").unwrap_or_else(|| "track".to_owned()))), ("id", Some(need(args, "target_id")?))],
            );
            send(b, ApiRequest::get(path)).await
        }
        other => Err(bad(format!("Unknown mode `{other}`"))),
    }
}

async fn stats<B: Backend>(b: &B, args: &Value) -> Out {
    let n = |k: &str| int(args, k).map(|v| v.to_string());
    let path = match need(args, "report")?.as_str() {
        "listening" => with_query("/v1/stats/listening", &[("days", n("days")), ("from_ms", n("from_ms")), ("to_ms", n("to_ms")), ("limit", n("limit"))]),
        "forgotten_gems" => with_query("/v1/stats/forgotten", &[("days", n("before_days")), ("min_plays", n("min_plays")), ("limit", n("limit"))]),
        "library_health" => "/v1/stats/health".to_owned(),
        "similar_tracks" => with_query("/v1/stats/similar", &[("track_id", text(args, "track_id")), ("limit", n("limit"))]),
        other => return Err(bad(format!("Unknown report `{other}`"))),
    };
    send(b, ApiRequest::get(path)).await
}

/// The requests `player_control` makes for `args`, in order.
pub fn control_requests(args: &Value) -> Result<Vec<ApiRequest>, ToolError> {
    let mut out = Vec::new();
    if let Some(t) = text(args, "transport") {
        let path = match t.as_str() {
            "play" => "/v1/player/resume",
            "pause" => "/v1/player/pause",
            "toggle" => "/v1/player/toggle",
            "next" => "/v1/player/next",
            "previous" => "/v1/player/previous",
            other => return Err(bad(format!("Unknown transport `{other}`"))),
        };
        out.push(ApiRequest::post(path, json!({})));
    }
    match (int(args, "seek_ms"), int(args, "seek_delta_ms")) {
        (Some(p), _) => out.push(ApiRequest::post("/v1/player/seek", json!({"position_ms": p.max(0)}))),
        (None, Some(d)) => out.push(ApiRequest::post("/v1/player/seek", json!({"delta_ms": d}))),
        (None, None) => {}
    }
    match (num(args, "volume"), num(args, "volume_delta")) {
        (Some(v), _) => out.push(ApiRequest::put("/v1/player/volume", json!({"volume": v}))),
        (None, Some(d)) => out.push(ApiRequest::put("/v1/player/volume", json!({"delta": d}))),
        (None, None) => {}
    }
    if let Some(m) = flag(args, "muted") {
        out.push(ApiRequest::put("/v1/player/volume", json!({"muted": m})));
    }
    let mut mode = Map::new();
    if let Some(s) = flag(args, "shuffle") {
        mode.insert("shuffle".to_owned(), Value::Bool(s));
    }
    if let Some(r) = text(args, "repeat") {
        mode.insert("repeat".to_owned(), Value::String(r));
    }
    if !mode.is_empty() {
        out.push(ApiRequest::put("/v1/player/mode", Value::Object(mode)));
    }
    match args.get("duck") {
        Some(Value::Null) => out.push(ApiRequest::delete("/v1/player/duck")),
        Some(d @ Value::Object(_)) => out.push(ApiRequest::put("/v1/player/duck", d.clone())),
        Some(_) => return Err(bad("`duck` is an object or null")),
        None => {}
    }
    if out.is_empty() {
        return Err(bad("Nothing to do: give at least one field"));
    }
    Ok(out)
}

async fn control<B: Backend>(b: &B, args: &Value) -> Out {
    let mut done = Vec::new();
    let mut last = Value::Null;
    for req in control_requests(args)? {
        let what = format!("{} {}", req.method.as_str(), req.path);
        match send(b, req).await {
            Ok(v) => {
                done.push(what);
                last = v;
            }
            Err(e) => {
                // What was done before the step that failed is said too.
                let status = match &e {
                    ToolError::Api { status, .. } => *status,
                    _ => 0,
                };
                let mut err = e.to_value();
                err["done"] = json!(done);
                err["failed"] = json!(what);
                return Err(ToolError::Api { status, body: err });
            }
        }
    }
    // The state after the last step (the duck's answer has only the duck).
    let state = match last.get("state") {
        Some(s) => s.clone(),
        None => send(b, ApiRequest::get("/v1/state")).await.unwrap_or(last),
    };
    Ok(json!({"done": done, "state": state}))
}

/// The request `queue_modify` makes for `args`.
pub fn queue_request(args: &Value) -> Result<ApiRequest, ToolError> {
    let entry = || int(args, "entry").ok_or_else(|| bad("`entry` is required"));
    Ok(match need(args, "action")?.as_str() {
        a @ ("play_next" | "enqueue") => {
            let ids = list_of(args, "track_ids");
            if ids.is_empty() {
                return Err(bad("`track_ids` is required"));
            }
            ApiRequest::post("/v1/queue", json!({"track_ids": ids, "mode": if a == "play_next" { "next" } else { "enqueue" }}))
        }
        "remove" => ApiRequest::delete(format!("/v1/queue/{}", entry()?)),
        "jump" => ApiRequest::post(format!("/v1/queue/{}/play", entry()?), json!({})),
        "move" => {
            let anchor = int(args, "anchor").ok_or_else(|| bad("`anchor` is required"))?;
            ApiRequest::post("/v1/queue/move", json!({"entry": entry()?, "anchor": anchor, "after": flag(args, "after").unwrap_or(true)}))
        }
        "clear" => ApiRequest::delete("/v1/queue"),
        other => return Err(bad(format!("Unknown action `{other}`"))),
    })
}

async fn queue<B: Backend>(b: &B, args: &Value) -> Out {
    let jump = text(args, "action").as_deref() == Some("jump");
    let v = send(b, queue_request(args)?).await?;
    if jump {
        return Ok(v);
    }
    Ok(queue_brief(&v, 20))
}

async fn playlist_add<B: Backend>(b: &B, args: &Value) -> Out {
    let mut body = Map::new();
    let tracks = list_of(args, "track_ids");
    let remote = list_of(args, "remote_track_ids");
    if !tracks.is_empty() {
        body.insert("track_ids".to_owned(), json!(tracks));
    }
    if !remote.is_empty() {
        body.insert("remote_track_ids".to_owned(), json!(remote));
    }
    if let Some(a) = text(args, "album_id") {
        body.insert("album_id".to_owned(), Value::String(a));
    }
    match (text(args, "playlist_id"), text(args, "name")) {
        (Some(id), _) => {
            if body.is_empty() {
                return Err(bad("Nothing to add: track_ids, album_id or remote_track_ids"));
            }
            send(b, ApiRequest::post(format!("/v1/playlists/{}/tracks", encode(&id)), Value::Object(body))).await
        }
        (None, Some(name)) => {
            body.insert("name".to_owned(), Value::String(name));
            send(b, ApiRequest::post("/v1/playlists", Value::Object(body))).await
        }
        (None, None) => Err(bad("`playlist_id` or `name` is required")),
    }
}

/// The request `playlist_remove` makes for `args`.
pub fn playlist_remove_request(args: &Value) -> Result<ApiRequest, ToolError> {
    let id = encode(&need(args, "playlist_id")?);
    let token = text(args, "confirm_token");
    Ok(match need(args, "action")?.as_str() {
        "remove_tracks" => {
            let mut body = json!({"track_ids": list_of(args, "track_ids")});
            if let Some(t) = token {
                body["confirm_token"] = Value::String(t);
            }
            ApiRequest::post(format!("/v1/playlists/{id}/remove"), body)
        }
        "rename" => {
            let mut body = json!({"name": need(args, "name")?});
            if let Some(t) = token {
                body["confirm_token"] = Value::String(t);
            }
            ApiRequest::put(format!("/v1/playlists/{id}"), body)
        }
        "delete" => ApiRequest::delete(with_query(&format!("/v1/playlists/{id}"), &[("confirm_token", token)])),
        other => return Err(bad(format!("Unknown action `{other}`"))),
    })
}

async fn playlist_remove<B: Backend>(b: &B, args: &Value) -> Out {
    send(b, playlist_remove_request(args)?).await
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Records the requests and answers each with `200 {"ok": true, …}`
    /// (the state route with a state, the queue with a queue).
    #[derive(Default)]
    struct Fake {
        seen: Mutex<Vec<ApiRequest>>,
        status: Mutex<Option<u16>>,
    }

    impl Backend for Fake {
        async fn call(&self, request: ApiRequest) -> Result<ApiResponse, Unreachable> {
            let path = request.path.clone();
            self.seen.lock().unwrap().push(request);
            if let Some(s) = *self.status.lock().unwrap() {
                return Ok(ApiResponse { status: s, body: json!({"error": "forbidden", "message": "no"}) });
            }
            let body = if path == "/v1/state" {
                json!({"status": "playing", "volume": 0.5, "track": {"id": "t", "title": "T", "cover": "/x"}})
            } else if path == "/v1/queue" {
                json!({"prev": [], "current": {"entry": 1, "track": {"id": "a", "title": "A", "cover": "/c"}},
                       "next": [{"entry": 2, "track": {"id": "b", "title": "B"}}, {"entry": 3, "track": {"id": "c", "title": "C"}}]})
            } else {
                json!({"ok": true, "nothing": null})
            };
            Ok(ApiResponse { status: 200, body })
        }
    }

    impl Fake {
        fn paths(&self) -> Vec<String> {
            self.seen.lock().unwrap().iter().map(|r| format!("{} {}", r.method.as_str(), r.path)).collect()
        }
    }

    #[test]
    fn the_eighteen_tools_in_their_groups() {
        assert_eq!(TOOLS.len(), 18);
        let names: Vec<&str> = TOOLS.iter().map(|t| t.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 18, "names are unique");
        let count = |c: Contour| TOOLS.iter().filter(|t| t.contour == c).count();
        assert_eq!((count(Contour::Read), count(Contour::Control), count(Contour::Library), count(Contour::Destructive)), (7, 5, 3, 3));
        for t in TOOLS {
            let s = (t.schema)();
            assert_eq!(s["type"], "object", "{}", t.name);
            assert!(s["properties"].is_object(), "{}", t.name);
            assert!(t.description.len() > 40, "{}", t.name);
        }
    }

    #[test]
    fn a_token_sees_only_the_tools_it_can_use() {
        let names = |p| list(p).iter().map(|t| t["name"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
        let read = names(Some(ApiPreset::Read));
        assert_eq!(read.len(), 7);
        assert!(read.contains(&"status_get".to_owned()) && !read.contains(&"player_play".to_owned()));
        let control = names(Some(ApiPreset::Control));
        assert_eq!(control.len(), 12);
        assert!(control.contains(&"target_switch".to_owned()));
        assert!(!read.contains(&"target_switch".to_owned()));
        assert!(control.contains(&"player_control".to_owned()) && !control.contains(&"library_feedback".to_owned()));
        assert!(!control.contains(&"library_trash".to_owned()));
        assert_eq!(names(Some(ApiPreset::Full)).len(), 18);
        assert_eq!(names(None).len(), 18, "unknown: everything, the API decides");
        let trash = list(Some(ApiPreset::Full)).into_iter().find(|t| t["name"] == "library_trash").unwrap();
        assert_eq!(trash["annotations"]["destructiveHint"], true);
        assert_eq!(trash["annotations"]["readOnlyHint"], false);
    }

    #[tokio::test]
    async fn each_tool_calls_its_endpoint() {
        let cases: Vec<(&str, Value, Vec<&str>)> = vec![
            ("library_search", json!({"query": "кино", "kind": "album"}), vec!["GET /v1/search?q=%D0%BA%D0%B8%D0%BD%D0%BE&kind=album"]),
            ("library_get", json!({"kind": "playlists"}), vec!["GET /v1/playlists"]),
            ("library_get", json!({"kind": "track", "id": "x", "include_lyrics": true}), vec!["GET /v1/tracks/x", "GET /v1/tracks/x/lyrics"]),
            ("library_get", json!({"kind": "playlist", "id": "liked"}), vec!["GET /v1/playlists/liked?offset=0&limit=100"]),
            ("library_get", json!({"kind": "artist", "id": "a", "offset": 200, "limit": 50}), vec!["GET /v1/artists/a?offset=200&limit=50"]),
            ("library_search", json!({"query": "x", "offset": 10}), vec!["GET /v1/search?q=x&offset=10"]),
            ("discover_search", json!({"query": "radiohead", "services": ["deezer", "youtube"]}), vec!["GET /v1/discover/search?q=radiohead&services=deezer%2Cyoutube"]),
            ("discover_search", json!({"mode": "artist_discography", "artist_id": "a1", "refresh": true}), vec!["GET /v1/discover/artists/a1/discography?refresh=true"]),
            ("discover_search", json!({"mode": "share_link", "target_kind": "album", "target_id": "r1"}), vec!["GET /v1/discover/share?kind=album&id=r1"]),
            ("stats_query", json!({"report": "listening", "days": 7}), vec!["GET /v1/stats/listening?days=7"]),
            ("stats_query", json!({"report": "forgotten_gems", "before_days": 120}), vec!["GET /v1/stats/forgotten?days=120"]),
            ("stats_query", json!({"report": "library_health"}), vec!["GET /v1/stats/health"]),
            ("stats_query", json!({"report": "similar_tracks", "track_id": "t", "limit": 5}), vec!["GET /v1/stats/similar?track_id=t&limit=5"]),
            ("downloads_inspect", json!({}), vec!["GET /v1/downloads"]),
            ("downloads_inspect", json!({"search_album_id": "r"}), vec!["GET /v1/downloads/sources?album_id=r"]),
            ("outputs_list", json!({}), vec!["GET /v1/outputs"]),
            ("player_play", json!({"query": "Любимые", "output": "pi"}), vec!["POST /v1/player/play"]),
            ("output_select", json!({"output": "local:default"}), vec!["PUT /v1/player/output"]),
            ("target_switch", json!({"target": "node:abc"}), vec!["PUT /v1/player/output"]),
            ("queue_modify", json!({"action": "enqueue", "track_ids": ["a"]}), vec!["POST /v1/queue"]),
            ("queue_modify", json!({"action": "remove", "entry": 4}), vec!["DELETE /v1/queue/4"]),
            ("queue_modify", json!({"action": "jump", "entry": 4}), vec!["POST /v1/queue/4/play"]),
            ("queue_modify", json!({"action": "clear"}), vec!["DELETE /v1/queue"]),
            ("library_feedback", json!({"action": "like", "kind": "track", "id": "t"}), vec!["POST /v1/library/feedback"]),
            ("playlist_add", json!({"playlist_id": "p", "track_ids": ["a"]}), vec!["POST /v1/playlists/p/tracks"]),
            ("playlist_add", json!({"name": "Ночь", "album_id": "r"}), vec!["POST /v1/playlists"]),
            ("downloads_enqueue", json!({"album_ids": ["r"]}), vec!["POST /v1/downloads"]),
            ("playlist_remove", json!({"playlist_id": "p", "action": "delete"}), vec!["DELETE /v1/playlists/p"]),
            ("playlist_remove", json!({"playlist_id": "p", "action": "delete", "confirm_token": "c1"}), vec!["DELETE /v1/playlists/p?confirm_token=c1"]),
            ("playlist_remove", json!({"playlist_id": "p", "action": "rename", "name": "N"}), vec!["PUT /v1/playlists/p"]),
            ("playlist_remove", json!({"playlist_id": "p", "action": "remove_tracks", "track_ids": ["a"]}), vec!["POST /v1/playlists/p/remove"]),
            ("library_retag", json!({"track_ids": ["a"], "changes": {"year": "1988"}}), vec!["POST /v1/library/retag"]),
            ("library_trash", json!({"album_ids": ["r"]}), vec!["POST /v1/library/trash"]),
            ("status_get", json!({}), vec!["GET /v1/state", "GET /v1/queue"]),
        ];
        for (name, args, want) in cases {
            let fake = Fake::default();
            call(&fake, name, &args).await.unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(fake.paths(), want, "{name} {args}");
        }
    }

    #[tokio::test]
    async fn bodies_carry_the_arguments() {
        let fake = Fake::default();
        call(&fake, "player_play", &json!({"query": "radiohead", "kind": "artist", "shuffle": false, "output": "pi"})).await.unwrap();
        let body = fake.seen.lock().unwrap()[0].body.clone().unwrap();
        assert_eq!(body, json!({"query": "radiohead", "kind": "artist", "shuffle": false, "output": "pi"}));
        let fake = Fake::default();
        call(&fake, "target_switch", &json!({"target": "node:abc", "mode": "attach"})).await.unwrap();
        call(&fake, "target_switch", &json!({"target": "this"})).await.unwrap();
        let seen = fake.seen.lock().unwrap();
        assert_eq!(seen[0].body.clone().unwrap(), json!({"output": "node:abc", "attach": true}));
        assert_eq!(seen[1].body.clone().unwrap(), json!({"output": "this", "attach": false}));
        let fake = Fake::default();
        call(&fake, "library_trash", &json!({"track_ids": ["a", " "], "confirm_token": "tok"})).await.unwrap();
        let body = fake.seen.lock().unwrap()[0].body.clone().unwrap();
        assert_eq!(body, json!({"track_ids": ["a"], "album_ids": [], "confirm_token": "tok"}));
    }

    #[test]
    fn the_player_control_steps_in_order() {
        let r = control_requests(&json!({"transport": "next", "volume_delta": -0.1, "muted": false, "repeat": "one", "duck": null})).unwrap();
        let got: Vec<String> = r.iter().map(|r| format!("{} {}", r.method.as_str(), r.path)).collect();
        assert_eq!(got, ["POST /v1/player/next", "PUT /v1/player/volume", "PUT /v1/player/volume", "PUT /v1/player/mode", "DELETE /v1/player/duck"]);
        assert_eq!(r[1].body, Some(json!({"delta": -0.1})));
        let r = control_requests(&json!({"transport": "play", "duck": {"gain": 0.2}})).unwrap();
        assert_eq!(r[0].path, "/v1/player/resume");
        assert_eq!(r[1].body, Some(json!({"gain": 0.2})));
        assert!(control_requests(&json!({})).is_err());
        assert!(control_requests(&json!({"duck": 3})).is_err());
    }

    #[tokio::test]
    async fn answers_are_compact_and_errors_are_said() {
        let fake = Fake::default();
        let v = call(&fake, "status_get", &json!({"queue_limit": 1})).await.unwrap();
        assert!(v["track"].get("cover").is_none(), "{v}");
        assert_eq!(v["queue"]["next"].as_array().unwrap().len(), 1);
        assert_eq!(v["queue"]["next_total"], 2);
        let v = call(&fake, "outputs_list", &json!({})).await.unwrap();
        assert!(v.get("nothing").is_none());
        let fake = Fake::default();
        *fake.status.lock().unwrap() = Some(403);
        let e = call(&fake, "library_trash", &json!({"track_ids": ["a"]})).await.unwrap_err();
        assert_eq!(e.to_value()["status"], 403);
        assert_eq!(e.to_value()["error"], "forbidden");
        assert!(matches!(call(&fake, "nope", &json!({})).await, Err(ToolError::Bad(_))));
        assert!(matches!(call(&fake, "library_search", &json!({})).await, Err(ToolError::Bad(_))));
        let un = ToolError::Unreachable(Unreachable::NotRunning).to_value();
        assert_eq!(un["message"], crate::backend::NOT_RUNNING);
    }

    #[test]
    fn a_list_is_paged_with_its_total() {
        let v = page(json!({"tracks": [1, 2, 3, 4, 5]}), "tracks", 1, 2);
        assert_eq!(v["tracks"], json!([2, 3]));
        assert_eq!(v["tracks_total"], 5);
        assert_eq!(v["tracks_window"], json!({"offset": 1, "count": 2}));
        let v = page(json!({"tracks": [1]}), "tracks", 0, 10);
        assert!(v.get("tracks_window").is_none());
        // A page the API cut: its total is the API's, not the page's.
        let v = served_page(json!({"tracks": [1, 2], "tracks_offset": 200}), 300, 200);
        assert_eq!(v["tracks_total"], 300);
        assert_eq!(v["tracks_window"], json!({"offset": 200, "count": 2}));
        assert!(v.get("tracks_offset").is_none());
    }
}
