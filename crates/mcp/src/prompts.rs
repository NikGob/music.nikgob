//! Three ready scenarios a user can start from the client's prompt menu.

use serde_json::{Value, json};

struct PromptDef {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    /// (name, description, required)
    arguments: &'static [(&'static str, &'static str, bool)],
}

const PROMPTS: &[PromptDef] = &[
    PromptDef {
        name: "curate-playlist",
        title: "Curate a playlist",
        description: "Build a playlist on a theme from the user's library and listening history, with a few finds from the services.",
        arguments: &[("theme", "What the playlist is about: a mood, an occasion, a style.", true), ("duration_min", "About how long, in minutes.", false)],
    },
    PromptDef {
        name: "library-audit",
        title: "Audit the library",
        description: "Check the library's health (fake lossless files, missing lyrics, masters without a copy, gaps in albums) and propose a plan of fixes.",
        arguments: &[],
    },
    PromptDef {
        name: "explain-track",
        title: "Explain a track",
        description: "Explain the playing track (or a given one): its lyrics, its place in the artist's discography, what is similar in the library.",
        arguments: &[("track_id", "A library track id; the playing track by default.", false)],
    },
];

/// The `prompts/list` entries.
pub fn list() -> Vec<Value> {
    PROMPTS
        .iter()
        .map(|p| {
            let args: Vec<Value> = p.arguments.iter().map(|(n, d, r)| json!({"name": n, "description": d, "required": r})).collect();
            json!({"name": p.name, "title": p.title, "description": p.description, "arguments": args})
        })
        .collect()
}

fn arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

/// The `prompts/get` result of `name`; an error for an unknown prompt or a
/// missing required argument (the message says which).
pub fn get(name: &str, args: &Value) -> Result<Value, String> {
    let p = PROMPTS.iter().find(|p| p.name == name).ok_or_else(|| format!("No prompt `{name}`"))?;
    for (n, _, required) in p.arguments {
        if *required && arg(args, n).is_none() {
            return Err(format!("`{n}` is required"));
        }
    }
    let text = match name {
        "curate-playlist" => {
            let theme = arg(args, "theme").unwrap_or_default();
            let length = arg(args, "duration_min").map_or_else(|| "about an hour".to_owned(), |d| format!("about {d} minutes"));
            format!(
                "Curate a playlist for: «{theme}», {length} long.\n\
                 1. Learn the taste: stats_query report=listening (days=90) and report=forgotten_gems.\n\
                 2. Find candidates in the library with library_search and library_get; use stats_query report=similar_tracks on good seeds.\n\
                 3. Add at most a few songs the library lacks from discover_search (mark them as new).\n\
                 4. Order the songs so energy and mood flow; sum the durations to fit the length.\n\
                 5. Show the list (artist — title, why it fits). Then, if the playlist_add tool is available, create it with name and the track ids; otherwise offer to play it with player_play and queue_modify."
            )
        }
        "library-audit" => "Audit the music library.\n\
             1. stats_query report=library_health: formats and sizes, tracks without synced lyrics, fake lossless files, masters without a replica, missing files.\n\
             2. For albums that look incomplete, use library_get kind=album and compare tracks with discover_search mode=artist_discography where useful.\n\
             3. Report the findings in order of importance with numbers.\n\
             4. Propose a plan of fixes. Do not change anything without the user's yes: retag and trash are two-step tools that answer a plan first; downloads over the limits wait in the app's «Подтверди»."
            .to_owned(),
        "explain-track" => {
            let which = arg(args, "track_id").map_or_else(
                || "the playing track (read the music://now-playing resource or call status_get)".to_owned(),
                |id| format!("the track {id} (library_get kind=track include_lyrics=true)"),
            );
            format!(
                "Explain {which}.\n\
                 1. Read its lyrics and say what the song is about; quote at most a short line.\n\
                 2. Place it in the artist's work: library_get kind=artist and discover_search mode=artist_discography.\n\
                 3. Name a few similar songs from the library (stats_query report=similar_tracks) and say how they relate.\n\
                 Keep it concise."
            )
        }
        _ => unreachable!("every prompt has its text"),
    };
    Ok(json!({"description": p.description, "messages": [{"role": "user", "content": {"type": "text", "text": text}}]}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_prompts_with_their_arguments() {
        let names: Vec<String> = list().iter().map(|p| p["name"].as_str().unwrap().to_owned()).collect();
        assert_eq!(names, ["curate-playlist", "library-audit", "explain-track"]);
        assert!(get("curate-playlist", &json!({})).is_err(), "theme is required");
        let p = get("curate-playlist", &json!({"theme": "дождь", "duration_min": "40"})).unwrap();
        let text = p["messages"][0]["content"]["text"].as_str().unwrap();
        assert!(text.contains("дождь") && text.contains("40 minutes"));
        assert!(get("explain-track", &json!({})).unwrap()["messages"][0]["content"]["text"].as_str().unwrap().contains("now-playing"));
        assert!(get("nope", &json!({})).is_err());
    }
}
