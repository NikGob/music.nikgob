//! The settings sweep: every setting at every value, shot on every screen
//! it changes, for review agents that can only look at pictures and read
//! code.
//!
//! # Commands
//!
//! Always on a copy of a library (`NIKGOB_DATA=<copy>`) or the demo
//! (`NIKGOB_DEMO=1`): the sweep refuses to run on the real profile, and it
//! writes the prefs of the data dir it runs on (they are put back at the
//! end). It runs offline, with the window off the screen, in one process
//! for the whole shard.
//!
//! - `nikgob --sweep-list out.json` — every settings row: tab, group, label,
//!   ⓘ text, kind, the values to try (every choice; a slider's ends, middle
//!   and two between), what each value writes, the defaults (`default`,
//!   `first_start` and the data dir's), what makes the row shown and
//!   active, which rows a value shows, hides or fades, what the row depends
//!   on, the screens it changes, and the built-in combos. No window.
//! - `nikgob --sweep plan.json --out dir` — shoots what the plan asks.
//! - `nikgob --sweep-baseline --out dir` — the baseline alone: every screen
//!   and every group of every settings tab with the data dir's prefs (with
//!   `--sweep plan.json` too, the baseline first, then the plan).
//! - `--sweep-overwrite` shoots again what is already in `dir` (by default
//!   an entry whose `meta.json` says `complete` is skipped, so a run that
//!   stopped goes on where it was).
//!
//! # Plan
//!
//! A JSON object, every field optional:
//!
//! ```json
//! { "tabs": ["Внешний вид"], "groups": ["Обложка"], "settings": ["CoverShape"],
//!   "exclude": ["OutputDevice"], "values": {"ShapePick": ["Cookie9", "Sunny"]},
//!   "screens": ["np", "np_paused"], "combos": true, "baseline": false,
//!   "shard": [0, 4], "sizes": [[1600, 1000], [1000, 700]],
//!   "track": "<track id>", "album": "Title", "fakes": ["lyrics", "torrents", "align-models"],
//!   "frames": 3, "frame_gap_ms": 400, "glow": false, "format": "png", "jpeg_quality": 90 }
//! ```
//!
//! `tabs` takes names or numbers (0 «Основные» … 3 «Внешний вид»); `combos`
//! takes `true` (the built-in ones, the default when nothing is filtered),
//! `false`, or a list of built-in names and objects
//! `{"name": "…", "set": [["CoverShape", "0"], ["Ring", "on"]], "screens": ["np"]}`;
//! `shard: [k, n]` keeps every n-th entry from the k-th; `screens` keeps
//! only those (the settings page is always shot; `[]` shoots it alone).
//!
//! # Output
//!
//! `dir/<setting>/<value>/<screen>[-fN][@WxH].png` and `meta.json` beside
//! them (the setting, the value, the prefs it changed, the screens, each
//! shot's time, the checks and warnings: the row not drawn, faded, off the
//! page or overlapping, the menu's label cut, the option lit not the one
//! set, the pref not taken, rows shown, hidden or changed in height,
//! slow frames, warnings and errors in the log); `dir/_combos/<name>/…`,
//! `dir/_baseline/…`; `dir/index.json` lists every entry with its warnings
//! and the rows whose height changes between the values of their setting.
//! Size `@WxH` is added for every size after the first.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use egui::{Context, Rect};
use nikgob_md3e::screens::sweep::{self as s, DrawnRow, Kind, Screen, SettingId};
use nikgob_md3e::screens::{Md3eUi, PaneTab};
use nikgob_view::{Command, NpSlot, Prefs, Route, ViewState};
use serde_json::{Map, Value as J, json};

/// What the command line asked.
struct Args {
    plan: J,
    out: PathBuf,
    baseline: bool,
    /// The plan came from a file (else only the baseline is shot).
    planned: bool,
    overwrite: bool,
    sizes: Vec<[f32; 2]>,
}

static ARGS: OnceLock<Option<Args>> = OnceLock::new();

fn arg_value(name: &str) -> Option<String> {
    let all: Vec<String> = std::env::args().collect();
    let at = all.iter().position(|a| a == name)?;
    all.get(at + 1).cloned()
}

fn flag(name: &str) -> bool {
    std::env::args().any(|a| a == name)
}

fn args() -> Option<&'static Args> {
    ARGS.get_or_init(|| {
        let plan_path = arg_value("--sweep");
        let baseline = flag("--sweep-baseline");
        if plan_path.is_none() && !baseline {
            return None;
        }
        let plan = match &plan_path {
            Some(p) => match std::fs::read(p).map_err(|e| e.to_string()).and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string())) {
                Ok(v) => v,
                Err(e) => {
                    log::error!("sweep: cannot read the plan {p}: {e}");
                    json!({})
                }
            },
            None => json!({}),
        };
        let sizes: Vec<[f32; 2]> = plan
            .get("sizes")
            .and_then(J::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|s| Some([s.get(0)?.as_f64()? as f32, s.get(1)?.as_f64()? as f32]))
                    .filter(|[w, h]| *w >= 360.0 && *h >= 560.0)
                    .collect()
            })
            .filter(|v: &Vec<[f32; 2]>| !v.is_empty())
            .unwrap_or_else(|| vec![[1600.0, 1000.0]]);
        Some(Args {
            overwrite: flag("--sweep-overwrite") || plan.get("overwrite").and_then(J::as_bool) == Some(true),
            out: PathBuf::from(arg_value("--out").unwrap_or_else(|| "sweep-out".to_owned())),
            baseline: baseline || plan.get("baseline").and_then(J::as_bool) == Some(true),
            planned: plan_path.is_some(),
            plan,
            sizes,
        })
    })
    .as_ref()
}

/// A sweep run was asked for (`--sweep` or `--sweep-baseline`).
pub fn active() -> bool {
    args().is_some()
}

/// The sweep's first window size.
pub fn window_size() -> Option<[f32; 2]> {
    args().map(|a| a.sizes[0])
}

/// `--sweep-list out.json`.
pub fn list_path() -> Option<PathBuf> {
    arg_value("--sweep-list").map(PathBuf::from)
}

fn prefs_map(p: &Prefs) -> Map<String, J> {
    match serde_json::to_value(p) {
        Ok(J::Object(m)) => m,
        _ => Map::new(),
    }
}

/// The fields `b` has otherwise than `a`, as `[from, to]`.
fn diff(a: &Prefs, b: &Prefs) -> Map<String, J> {
    let (a, b) = (prefs_map(a), prefs_map(b));
    b.into_iter().filter(|(k, v)| a.get(k) != Some(v)).map(|(k, v)| (k.clone(), json!([a.get(&k).cloned().unwrap_or(J::Null), v]))).collect()
}

/// The fields `b` has otherwise than `a`, as they are in `b`.
fn patch(a: &Prefs, b: &Prefs) -> Map<String, J> {
    let (a, b) = (prefs_map(a), prefs_map(b));
    b.into_iter().filter(|(k, v)| a.get(k) != Some(v)).collect()
}

fn rect_json(r: Rect) -> J {
    let n = |v: f32| (v * 10.0).round() / 10.0;
    json!([n(r.left()), n(r.top()), n(r.width()), n(r.height())])
}

fn visible(r: &DrawnRow) -> bool {
    r.clip.intersects(r.rect)
}

/// Which rows `after` draws otherwise than `before`, besides `own`.
fn row_effects(before: &[DrawnRow], after: &[DrawnRow], own: SettingId) -> Map<String, J> {
    let find = |rows: &[DrawnRow], id| rows.iter().find(|r| r.id == id).cloned();
    let ids: BTreeSet<String> = before.iter().chain(after).filter(|r| r.id != own).map(|r| s::key(r.id)).collect();
    let mut shows = Vec::new();
    let mut hides = Vec::new();
    let mut dims = Vec::new();
    let mut undims = Vec::new();
    for key in ids {
        let Some(id) = s::parse(&key) else { continue };
        match (find(before, id), find(after, id)) {
            (None, Some(_)) => shows.push(key),
            (Some(_), None) => hides.push(key),
            (Some(b), Some(a)) => {
                let (was, now) = (b.opacity < 0.99 || !b.enabled, a.opacity < 0.99 || !a.enabled);
                if now && !was {
                    dims.push(key);
                } else if was && !now {
                    undims.push(key);
                }
            }
            (None, None) => {}
        }
    }
    let mut m = Map::new();
    for (name, list) in [("shows", shows), ("hides", hides), ("dims", dims), ("undims", undims)] {
        if !list.is_empty() {
            m.insert(name.to_owned(), json!(list));
        }
    }
    m
}

/// Combinations of values known to be risky, shot together.
const COMBOS: &[(&str, &[(&str, &str)], &[&str])] = &[
    ("square-ring-circle-paused", &[("CoverShape", "0"), ("Ring", "on"), ("RingStyle", "0"), ("RingSpin", "on")], &["np", "np_paused"]),
    ("square-ring-contour", &[("CoverShape", "0"), ("Ring", "on"), ("RingStyle", "1")], &["np", "np_paused"]),
    ("random-ring-bars-pulse", &[("CoverShape", "2"), ("Ring", "on"), ("RingStyle", "2"), ("CoverPulse", "on")], &["np", "np_paused"]),
    (
        "shape-spin-ccw-ring-contour-spin",
        &[("CoverShape", "1"), ("CoverSpin", "on"), ("CoverDir", "1"), ("Ring", "on"), ("RingStyle", "1"), ("RingSpin", "on"), ("RingDir", "0")],
        &["np", "np_paused"],
    ),
    (
        "reduced-motion-everything-turning",
        &[
            ("ReducedMotion", "on"),
            ("CoverShape", "1"),
            ("CoverSpin", "on"),
            ("CoverPulse", "on"),
            ("Ring", "on"),
            ("RingStyle", "0"),
            ("RingSpin", "on"),
            ("RingBeat", "on"),
        ],
        &["np", "np_paused", "np_opening"],
    ),
    (
        "ambient-reactive-max-bg-morph-max",
        &[("Ambient", "2"), ("AmbientStrength", "2"), ("BgShapes", "3"), ("BgCount", "32"), ("BgSize", "2"), ("BgOpacity", "2")],
        &["np", "np_paused", "np_lyrics"],
    ),
    (
        "light-theme-bg-bounce-custom-hue-glow",
        &[("Theme", "0"), ("BgShapes", "1"), ("BgColors", "3"), ("BgHue", "5"), ("Ambient", "1")],
        &["np", "np_paused"],
    ),
    ("dark-high-contrast-mono", &[("Theme", "1"), ("Contrast", "2"), ("Scheme", "5")], &["home", "np", "np_lyrics"]),
    (
        "lyrics-wave-words-jump-max-soft-edges-max",
        &[
            ("LyricsStyle", "2"),
            ("LyricsUnit", "2"),
            ("WaveJump", "2"),
            ("LyricsSoftEdges", "on"),
            ("LyricsSoftStrength", "1.5"),
            ("LyricsSoftDepth", "0.4"),
            ("LyricsSoftFade", "1"),
        ],
        &["np_lyrics"],
    ),
    (
        "lyrics-karaoke-letters-glow-big-dim",
        &[("LyricsStyle", "0"), ("LyricsUnit", "0"), ("LyricsGlow", "on"), ("LyricsScale", "1.4"), ("LyricsDim", "1")],
        &["np_lyrics"],
    ),
    ("lyrics-focus-letters", &[("LyricsStyle", "1"), ("LyricsUnit", "0")], &["np_lyrics"]),
    ("lyrics-lead-early-smooth-instant-light", &[("LyricsLead", "-0.5"), ("LyricsSmooth", "1"), ("LightSpeed", "1")], &["np_lyrics"]),
    (
        "lyrics-gap-thick-wave-max-countdown",
        &[("LyricsGap", "3"), ("LyricsGapWaves", "16"), ("LyricsGapHeight", "2"), ("LyricsGapSize", "1.5"), ("LyricsCountdown", "on")],
        &["np_lyrics_gap", "np_lyrics_countdown"],
    ),
    (
        "lyrics-gap-small-line-no-countdown",
        &[("LyricsGap", "0"), ("LyricsGapSize", "0.8"), ("LyricsCountdown", "off")],
        &["np_lyrics_gap", "np_lyrics_countdown"],
    ),
    (
        "lyrics-big-soft-edges-pane-compact-rail",
        &[("LyricsScale", "1.4"), ("LyricsSoftEdges", "on"), ("LyricsSoftDepth", "0.4"), ("PaneLyrics", "on"), ("RailCompact", "on")],
        &["np_lyrics", "lyrics_pane"],
    ),
];

/// `sets` as (row, value) from keys, each value looked up after the ones
/// before it are set (as the run sets them); the keys that name nothing.
fn resolve(base: &Prefs, sets: &[(String, String)]) -> (Vec<(SettingId, usize)>, Vec<String>) {
    let mut p = base.clone();
    let mut out = Vec::new();
    let mut bad = Vec::new();
    for (setting, value) in sets {
        let Some(id) = s::parse(setting) else {
            bad.push(format!("{setting}={value}"));
            continue;
        };
        s::enable(id, &mut p);
        match s::values(id, &p).iter().position(|v| v.key == *value) {
            Some(i) => {
                s::apply(id, i, &mut p);
                out.push((id, i));
            }
            None => bad.push(format!("{setting}={value}")),
        }
    }
    (out, bad)
}

/// Writes `--sweep-list`: every row and what can be known of it without
/// the runtime (`base` is the data dir's prefs, or the defaults).
pub fn write_list(path: &std::path::Path, base: &Prefs) -> std::io::Result<usize> {
    let size = [1600.0, 1000.0];
    let mut survey = s::Survey::new(s::survey_state(), size);
    let base_rows: Vec<Vec<DrawnRow>> = (0..s::TABS.len()).map(|t| survey.tab(base, t)).collect();
    let (defaults, first) = (prefs_map(&Prefs::default()), prefs_map(&Prefs::first_start()));
    let mine = prefs_map(base);
    let mut settings = Vec::new();
    // Row → the settings whose values hide or fade it.
    let mut depends: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for &id in s::ALL {
        let tab = id.tab();
        let mut en = base.clone();
        s::enable(id, &mut en);
        let en_rows = if en == *base { base_rows[tab].clone() } else { survey.tab(&en, tab) };
        let row = en_rows.iter().chain(&base_rows[tab]).find(|r| r.id == id).cloned();
        let mut fields: BTreeSet<String> = BTreeSet::new();
        let mut values = Vec::new();
        for (i, v) in s::values(id, &en).into_iter().enumerate() {
            let mut p = en.clone();
            s::apply(id, i, &mut p);
            fields.extend(patch(&en, &p).into_iter().map(|(k, _)| k));
            let rows = survey.tab(&p, tab);
            let effects = row_effects(&en_rows, &rows, id);
            for list in ["hides", "dims"] {
                for r in effects.get(list).and_then(J::as_array).into_iter().flatten().filter_map(J::as_str) {
                    depends.entry(r.to_owned()).or_default().insert(s::key(id));
                }
            }
            values.push(json!({
                "key": v.key, "label": v.label, "help": v.help,
                "prefs": patch(base, &p),
                "same_as_baseline": p == *base,
                "rows": effects,
            }));
        }
        let pick = |m: &Map<String, J>| fields.iter().map(|f| (f.clone(), m.get(f).cloned().unwrap_or(J::Null))).collect::<Map<_, _>>();
        let key_of = |p: &Prefs| s::shown(id, p).and_then(|i| s::values(id, p).get(i).map(|v| v.key.clone()));
        settings.push(json!({
            "id": s::key(id),
            "tab": s::tab_name(id),
            "tab_index": tab,
            "group": row.as_ref().map(|r| r.group.clone()),
            "label": row.as_ref().map(|r| r.label.clone()),
            "drawn": row.is_some(),
            "help": s::help(id),
            "beta": s::beta(id),
            "kind": s::kind(id).name(),
            "values": values,
            "fields": fields,
            "default": pick(&defaults),
            "first_start": pick(&first),
            "baseline": pick(&mine),
            "default_value": key_of(&Prefs::default()),
            "first_start_value": key_of(&Prefs::first_start()),
            "baseline_value": key_of(base),
            "enable": patch(base, &en),
            "screens": s::screens(id).iter().map(|x| x.key()).collect::<Vec<_>>(),
            "animated": s::animated(id),
            "needs": s::needs(id),
        }));
    }
    for item in &mut settings {
        let key = item["id"].as_str().unwrap_or_default().to_owned();
        let deps: Vec<String> = depends.get(&key).map(|d| d.iter().cloned().collect()).unwrap_or_default();
        item["depends_on"] = json!(deps);
    }
    let combos: Vec<J> = COMBOS
        .iter()
        .map(|(name, sets, screens)| {
            let set: Vec<J> = sets.iter().map(|(k, v)| json!([k, v])).collect();
            json!({"name": name, "set": set, "screens": screens})
        })
        .collect();
    let screens: Vec<J> = Screen::ALL.iter().map(|x| json!({"key": x.key(), "about": x.about()})).collect();
    let n = settings.len();
    let doc = json!({
        "tabs": s::TABS,
        "screens": screens,
        "settings": settings,
        "combos": combos,
        "note": "Values are tried from the baseline prefs with `enable` applied first. `rows` says which other rows a value shows, hides, fades (dims) or brings back on the settings page as drawn without the runtime (rows that need an account or the runtime's state may be missing: `drawn` false).",
    });
    std::fs::write(path, serde_json::to_vec_pretty(&doc).unwrap_or_default())?;
    Ok(n)
}

/// One thing the run shoots.
#[derive(Clone, Debug)]
struct Entry {
    /// Under the output dir: `CoverShape/0`, `_combos/<name>`, `_baseline`.
    dir: String,
    setting: Option<SettingId>,
    value: Option<(String, String)>,
    combo: Option<String>,
    sets: Vec<(SettingId, usize)>,
    screens: Vec<Screen>,
    animated: bool,
    baseline: bool,
}

impl Entry {
    /// The row the settings page is opened at.
    fn row(&self) -> Option<SettingId> {
        self.setting.or_else(|| self.sets.first().map(|(id, _)| *id))
    }
}

fn strings(v: Option<&J>) -> Option<Vec<String>> {
    v?.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned).or_else(|| x.as_u64().map(|n| n.to_string()))).collect())
}

/// The track the sweep plays.
#[derive(Clone, Debug)]
enum Pick {
    Track(nikgob_core::model::TrackId),
    Album(String),
    First,
}

/// The sweep's run as the screenshot driver's steps, or None without
/// `--sweep` / `--sweep-baseline`. `base` is the prefs the app starts with.
pub fn shot(base: &Prefs) -> Option<crate::shot::Shot> {
    let a = args()?;
    let plan = &a.plan;
    let mut survey = s::Survey::new(s::survey_state(), a.sizes[0]);
    let mut groups: std::collections::HashMap<SettingId, String> = std::collections::HashMap::new();
    for t in 0..s::TABS.len() {
        for r in survey.tab(base, t) {
            groups.insert(r.id, r.group);
        }
    }
    let tabs: Option<Vec<usize>> = strings(plan.get("tabs"))
        .map(|l| l.iter().filter_map(|t| t.parse().ok().or_else(|| s::TABS.iter().position(|n| n == t))).collect());
    let group_filter = strings(plan.get("groups"));
    let only = strings(plan.get("settings"));
    let exclude = strings(plan.get("exclude")).unwrap_or_default();
    let value_filter = plan.get("values").and_then(J::as_object);
    let screen_filter: Option<Vec<Screen>> = strings(plan.get("screens")).map(|l| l.iter().filter_map(|k| Screen::parse(k)).collect());
    let keep_screens = |list: Vec<Screen>| match &screen_filter {
        Some(f) => list.into_iter().filter(|x| f.contains(x)).collect(),
        None => list,
    };
    let filtered = tabs.is_some() || group_filter.is_some() || only.is_some();
    let mut entries = Vec::new();
    let mut problems = Vec::new();
    if a.baseline {
        entries.push(Entry {
            dir: "_baseline".to_owned(),
            setting: None,
            value: None,
            combo: None,
            sets: Vec::new(),
            screens: keep_screens(Screen::ALL.to_vec()),
            animated: true,
            baseline: true,
        });
    }
    if a.planned {
        for &id in s::ALL {
            let key = s::key(id);
            if tabs.as_ref().is_some_and(|t| !t.contains(&id.tab()))
                || group_filter.as_ref().is_some_and(|g| !groups.get(&id).is_some_and(|x| g.contains(x)))
                || only.as_ref().is_some_and(|o| !o.contains(&key))
                || exclude.contains(&key)
            {
                continue;
            }
            let mut en = base.clone();
            s::enable(id, &mut en);
            let wanted = value_filter.and_then(|m| strings(m.get(&key)));
            for (i, v) in s::values(id, &en).into_iter().enumerate() {
                if wanted.as_ref().is_some_and(|w| !w.contains(&v.key)) {
                    continue;
                }
                entries.push(Entry {
                    dir: format!("{key}/{}", v.key),
                    setting: Some(id),
                    value: Some((v.key, v.label)),
                    combo: None,
                    sets: vec![(id, i)],
                    screens: keep_screens(s::screens(id)),
                    animated: s::animated(id),
                    baseline: false,
                });
            }
        }
        let combos: Vec<(String, Vec<(String, String)>, Vec<Screen>)> = match plan.get("combos") {
            Some(J::Bool(false)) => Vec::new(),
            Some(J::Array(list)) => list
                .iter()
                .filter_map(|c| match c {
                    J::String(name) => COMBOS.iter().find(|c| c.0 == name.as_str()).map(builtin),
                    J::Object(o) => {
                        let set = o.get("set")?.as_array()?.iter().filter_map(|p| Some((p.get(0)?.as_str()?.to_owned(), p.get(1)?.as_str()?.to_owned()))).collect();
                        let screens = strings(o.get("screens")).unwrap_or_default().iter().filter_map(|k| Screen::parse(k)).collect();
                        Some((o.get("name")?.as_str()?.to_owned(), set, screens))
                    }
                    _ => None,
                })
                .collect(),
            Some(J::Bool(true)) => COMBOS.iter().map(builtin).collect(),
            _ if !filtered => COMBOS.iter().map(builtin).collect(),
            _ => Vec::new(),
        };
        for (name, sets, screens) in combos {
            let (sets, bad) = resolve(base, &sets);
            problems.extend(bad.into_iter().map(|b| format!("combo {name}: no value {b}")));
            entries.push(Entry {
                dir: format!("_combos/{name}"),
                setting: None,
                value: None,
                combo: Some(name),
                sets,
                screens: keep_screens(screens),
                animated: true,
                baseline: false,
            });
        }
    }
    if let Some((k, n)) = plan.get("shard").and_then(|v| Some((v.get(0)?.as_u64()? as usize, v.get(1)?.as_u64()? as usize))).filter(|(k, n)| *n > 0 && k < n) {
        entries = entries.into_iter().enumerate().filter(|(i, _)| i % n == k).map(|(_, e)| e).collect();
    }
    if !a.overwrite {
        entries.retain(|e| {
            let meta = a.out.join(&e.dir).join("meta.json");
            let done = std::fs::read(&meta).ok().and_then(|b| serde_json::from_slice::<J>(&b).ok()).and_then(|m| m.get("complete")?.as_bool());
            done != Some(true)
        });
    }
    let pick = match (plan.get("track").and_then(J::as_str), plan.get("album").and_then(J::as_str)) {
        (Some(t), _) => t.parse().map(Pick::Track).unwrap_or(Pick::First),
        (None, Some(a)) => Pick::Album(a.to_owned()),
        _ => Pick::First,
    };
    let fakes = strings(plan.get("fakes")).unwrap_or_else(|| vec!["lyrics".into(), "torrents".into(), "align-models".into(), "analysis".into()]);
    let format = if plan.get("format").and_then(J::as_str) == Some("jpg") { "jpg" } else { "png" };
    let quality = plan.get("jpeg_quality").and_then(J::as_u64).map_or(90, |q| q.clamp(40, 100) as u8);
    let run = Run {
        out: a.out.clone(),
        sizes: a.sizes.clone(),
        size: 0,
        ext: format,
        frames: plan.get("frames").and_then(J::as_u64).map_or(3, |f| f.clamp(1, 6) as usize),
        gap_ms: plan.get("frame_gap_ms").and_then(J::as_u64).unwrap_or(400),
        glow: plan.get("glow").and_then(J::as_bool) == Some(true),
        fakes,
        pick,
        track: None,
        start_tries: 0,
        base: None,
        expected: None,
        cur: None,
        metas: vec![Map::new(); entries.len()],
        survey: BTreeMap::new(),
        heights: BTreeMap::new(),
        index: vec![J::Null; entries.len()],
        problems,
        plan: plan.clone(),
        started: Instant::now(),
        entries,
    };
    let steps = run.steps();
    log::info!("sweep: {} entries, {} steps, into {}", run.entries.len(), steps.len(), run.out.display());
    Some(crate::shot::Shot::sweep(steps, run, quality))
}

fn builtin(c: &(&str, &[(&str, &str)], &[&str])) -> (String, Vec<(String, String)>, Vec<Screen>) {
    let (name, sets, screens) = c;
    (
        (*name).to_owned(),
        sets.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect(),
        screens.iter().filter_map(|k| Screen::parse(k)).collect(),
    )
}

/// What one entry gathers while it is shot (at one size).
struct Cur {
    n: usize,
    t0: Instant,
    synced_from: Instant,
    /// The step running, for slow frames.
    label: String,
    shots: Vec<J>,
    warnings: Vec<J>,
    checks: Map<String, J>,
    frames: u32,
    max_ms: f32,
    spikes: Vec<J>,
}

/// A frame slower than this (ms) is noted.
const SPIKE_MS: f32 = 150.0;

/// The sweep while it runs.
pub struct Run {
    out: PathBuf,
    entries: Vec<Entry>,
    sizes: Vec<[f32; 2]>,
    size: usize,
    ext: &'static str,
    frames: usize,
    gap_ms: u64,
    glow: bool,
    fakes: Vec<String>,
    pick: Pick,
    track: Option<nikgob_core::model::TrackId>,
    start_tries: u32,
    /// The data dir's prefs, as the run found them.
    base: Option<Prefs>,
    expected: Option<Prefs>,
    cur: Option<Cur>,
    metas: Vec<Map<String, J>>,
    /// Each tab's rows with the baseline prefs, by size.
    survey: BTreeMap<(usize, usize), Vec<DrawnRow>>,
    /// Setting → size → value → the height of its row.
    heights: BTreeMap<String, BTreeMap<String, BTreeMap<String, f32>>>,
    index: Vec<J>,
    problems: Vec<String>,
    plan: J,
    started: Instant,
}

/// The lyrics moment each Now Playing screen is shot at (s into the song):
/// mid-line of `fake:lyrics`, in the pause of `fake:lyrics-gap:words`, in
/// its last seconds.
fn moment(screen: Screen) -> f32 {
    match screen {
        Screen::NpLyricsGap => 9.0,
        Screen::NpLyricsCountdown => 12.6,
        _ => 9.9,
    }
}

impl Run {
    fn size_key(&self) -> String {
        let [w, h] = self.sizes[self.size.min(self.sizes.len() - 1)];
        format!("{w:.0}x{h:.0}")
    }

    fn suffix(&self, size: usize) -> String {
        if size == 0 {
            String::new()
        } else {
            let [w, h] = self.sizes[size];
            format!("@{w:.0}x{h:.0}")
        }
    }

    /// Every step of the run.
    fn steps(&self) -> Vec<String> {
        let mut v = vec!["sweep_load".to_owned(), "waitms:2500".to_owned(), "sweep_start".to_owned(), "waitms:2000".to_owned()];
        for size in 0..self.sizes.len() {
            v.push(format!("sweep_size:{size}"));
            v.push("waitms:1500".to_owned());
            for tab in 0..s::TABS.len() {
                v.push(format!("sweep_survey:{tab}"));
                v.push("waitms:1800".to_owned());
                v.push(format!("sweep_survey_take:{tab}"));
            }
            for n in 0..self.entries.len() {
                v.extend(self.entry_steps(n, size));
            }
        }
        v.push("sweep_finish".to_owned());
        v
    }

    fn file(&self, n: usize, size: usize, name: &str) -> Vec<String> {
        let file = format!("{name}{}.{}", self.suffix(size), self.ext);
        let path = self.out.join(&self.entries[n].dir).join(&file);
        vec![format!("sweep_mark:{n}:{file}"), format!("snap:{}", path.display())]
    }

    fn entry_steps(&self, n: usize, size: usize) -> Vec<String> {
        let e = &self.entries[n];
        let mut v = vec![format!("sweep_begin:{n}"), format!("sweep_sync:{n}"), "waitms:400".to_owned()];
        if e.baseline {
            v.push(format!("sweep_groups:{n}"));
        } else if e.row().is_some() {
            v.push(format!("sweep_settings:{n}"));
            v.push(format!("waitms:{}", if self.glow { 700 } else { 1800 }));
            v.push(format!("sweep_check:{n}"));
            v.extend(self.file(n, size, "settings"));
        }
        for &screen in &e.screens {
            let key = screen.key();
            v.push(format!("sweep_screen:{n}:{key}"));
            match screen {
                Screen::LyricsUi => {
                    v.push("waitms:1200".to_owned());
                    v.push(format!("sweep_lyrics_ui:{n}"));
                    v.push("waitms:1000".to_owned());
                    v.extend(self.file(n, size, key));
                }
                Screen::NpOpening => {
                    v.push("waitms:800".to_owned());
                    v.push(format!("sweep_np_open:{n}"));
                    for (f, wait) in [100u64, 150, 200, 900].into_iter().enumerate() {
                        v.push(format!("waitms:{wait}"));
                        v.extend(self.file(n, size, &format!("{key}-f{}", f + 1)));
                    }
                }
                _ => {
                    v.push(format!("waitms:{}", if screen.now_playing() { 1500 } else { 1000 }));
                    let frames = if e.animated && screen != Screen::NpPaused && screen.now_playing() { self.frames } else { 1 };
                    for f in 0..frames {
                        if f > 0 {
                            v.push(format!("waitms:{}", self.gap_ms));
                        }
                        let name = if frames == 1 { key.to_owned() } else { format!("{key}-f{}", f + 1) };
                        v.extend(self.file(n, size, &name));
                    }
                    if screen == Screen::NpPaused {
                        v.push("sweep_play".to_owned());
                    }
                }
            }
        }
        v.push(format!("sweep_end:{n}"));
        v
    }

    /// One frame took `dt` (`saved`: it saved a picture, which is slow on purpose).
    pub fn tick(&mut self, dt: Duration, saved: bool) {
        let Some(c) = self.cur.as_mut() else { return };
        if saved {
            return;
        }
        let ms = dt.as_secs_f32() * 1000.0;
        c.frames += 1;
        c.max_ms = c.max_ms.max(ms);
        if ms > SPIKE_MS && c.spikes.len() < 20 {
            c.spikes.push(json!({"during": c.label, "ms": ms.round(), "at_ms": c.t0.elapsed().as_millis() as u64}));
        }
    }

    fn warn(&mut self, check: &str, detail: J) {
        if let Some(c) = self.cur.as_mut() {
            c.warnings.push(json!({"check": check, "detail": detail}));
        }
    }

    fn set_fakes(&self, fakes: &mut crate::shot_fakes::Fakes, gap: bool) {
        fakes.clear();
        for f in &self.fakes {
            if gap && f.starts_with("lyrics") {
                continue;
            }
            fakes.add(f);
        }
        if gap {
            fakes.add("lyrics-gap:words");
        }
    }

    /// The sweep's track plays (again), not paused.
    fn ensure_playing(&mut self, state: &ViewState, out: &mut Vec<Command>) {
        let current = state.player.current.as_ref().map(|t| t.id);
        if self.track.is_none() {
            self.track = current;
        }
        if let Some(t) = self.track
            && current != Some(t)
        {
            out.push(Command::PlayTracks { tracks: vec![t], start: 0, context: "Скриншот".into() });
        } else if state.player.status == nikgob_view::Status::Paused {
            out.push(Command::Play);
        }
    }

    /// Runs one `sweep_…` step; returns the frames to wait after it.
    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        verb: &str,
        arg: &str,
        ctx: &Context,
        ui: &mut Md3eUi,
        state: &ViewState,
        out: &mut Vec<Command>,
        steps: &mut Vec<String>,
        fakes: &mut crate::shot_fakes::Fakes,
    ) -> u32 {
        let (num, rest) = arg.split_once(':').unwrap_or((arg, ""));
        let n: usize = num.parse().unwrap_or(0);
        match verb {
            // The library loads with its page.
            "sweep_load" => {
                ui.sweep_page(ctx, Route::Library(nikgob_view::LibraryTab::All), None, out);
                2
            }
            "sweep_start" => self.start(state, out, steps, fakes),
            "sweep_size" => {
                self.size = n.min(self.sizes.len() - 1);
                let [w, h] = self.sizes[self.size];
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(w, h)));
                if let Some(base) = &self.base {
                    out.push(Command::SetPrefs(Box::new(base.clone())));
                }
                ui.sweep_reset();
                2
            }
            "sweep_survey" => {
                ui.sweep_open_setting(s::tab_first(n));
                2
            }
            "sweep_survey_take" => {
                let rows: Vec<DrawnRow> = s::rows_now(ctx).into_iter().filter(|r| r.id.tab() == n).collect();
                if rows.is_empty() {
                    self.problems.push(format!("survey of tab {n} at {}: no rows drawn", self.size_key()));
                }
                self.survey.insert((self.size, n), rows);
                0
            }
            "sweep_begin" => self.begin(n, ui, out, fakes),
            "sweep_sync" => self.sync(n, state, steps),
            "sweep_settings" => {
                if let Some(id) = self.entries.get(n).and_then(Entry::row) {
                    ui.sweep_open_setting(id);
                }
                2
            }
            "sweep_open" => {
                if let Some(id) = s::parse(rest) {
                    ui.sweep_open_setting(id);
                }
                2
            }
            "sweep_check" => {
                self.check(n, ctx, state);
                0
            }
            "sweep_groups" => {
                self.groups(n, steps);
                0
            }
            "sweep_mark" => {
                if let Some(c) = self.cur.as_mut() {
                    let [w, h] = self.sizes[self.size];
                    c.shots.push(json!({"file": rest, "size": [w, h], "t_ms": c.t0.elapsed().as_millis() as u64, "during": c.label}));
                }
                0
            }
            "sweep_screen" => {
                self.screen(rest, ctx, ui, state, out, fakes);
                2
            }
            "sweep_np_open" => {
                ui.sweep_now_playing(ctx, NpSlot::None, true);
                0
            }
            "sweep_lyrics_ui" => {
                nikgob_md3e::screens::lyrics_ui::open(ctx);
                2
            }
            "sweep_play" => {
                out.push(Command::Play);
                1
            }
            "sweep_end" => {
                self.end(n);
                0
            }
            "sweep_finish" => {
                self.finish(ui, out, steps);
                0
            }
            _ => {
                log::warn!("sweep: unknown step {verb}");
                0
            }
        }
    }

    fn start(&mut self, state: &ViewState, out: &mut Vec<Command>, steps: &mut Vec<String>, fakes: &mut crate::shot_fakes::Fakes) -> u32 {
        let releases = &state.library.releases;
        if releases.is_empty() && self.start_tries < 20 {
            self.start_tries += 1;
            steps.insert(0, "sweep_start".to_owned());
            steps.insert(0, "waitms:500".to_owned());
            return 0;
        }
        self.base = Some(state.prefs.clone());
        crate::logfile::capture_start();
        let _ = std::fs::create_dir_all(&self.out);
        out.push(Command::SetVolume(0.0));
        self.set_fakes(fakes, false);
        match &self.pick {
            Pick::Track(t) => {
                self.track = Some(*t);
                out.push(Command::PlayTracks { tracks: vec![*t], start: 0, context: "Скриншот".into() });
            }
            Pick::Album(title) => match releases.iter().find(|r| r.title == *title).or(releases.first()) {
                Some(r) => out.push(Command::PlayRelease { id: r.id, start: None, shuffle: false }),
                None => self.problems.push("no album to play".to_owned()),
            },
            Pick::First => match releases.first() {
                Some(r) => out.push(Command::PlayRelease { id: r.id, start: None, shuffle: false }),
                None => self.problems.push("the library has no album to play".to_owned()),
            },
        }
        2
    }

    fn begin(&mut self, n: usize, ui: &mut Md3eUi, out: &mut Vec<Command>, fakes: &mut crate::shot_fakes::Fakes) -> u32 {
        let Some(base) = self.base.clone() else { return 0 };
        let mut expected = base;
        for &(id, i) in &self.entries[n].sets {
            s::enable(id, &mut expected);
            s::apply(id, i, &mut expected);
        }
        out.push(Command::SetPrefs(Box::new(expected.clone())));
        self.expected = Some(expected);
        ui.sweep_reset();
        self.set_fakes(fakes, false);
        let _ = std::fs::create_dir_all(self.out.join(&self.entries[n].dir));
        let _ = crate::logfile::capture_take();
        let now = Instant::now();
        self.cur = Some(Cur {
            n,
            t0: now,
            synced_from: now,
            label: "begin".to_owned(),
            shots: Vec::new(),
            warnings: Vec::new(),
            checks: Map::new(),
            frames: 0,
            max_ms: 0.0,
            spikes: Vec::new(),
        });
        2
    }

    /// Waits (up to 3 s) until the runtime holds the prefs sent; says when
    /// it does not.
    fn sync(&mut self, n: usize, state: &ViewState, steps: &mut Vec<String>) -> u32 {
        let (Some(expected), Some(base)) = (self.expected.clone(), self.base.clone()) else { return 0 };
        let waited = self.cur.as_ref().map_or(Duration::ZERO, |c| c.synced_from.elapsed());
        if state.prefs == expected {
            if let Some(c) = self.cur.as_mut() {
                c.checks.insert("prefs_applied_ms".into(), json!(waited.as_millis() as u64));
            }
            return 0;
        }
        if waited < Duration::from_secs(3) {
            steps.insert(0, format!("sweep_sync:{n}"));
            return 1;
        }
        if state.prefs == base && expected != base {
            self.warn("pref_unchanged", json!(diff(&base, &expected)));
        } else {
            self.warn("pref_not_as_set", json!({"set": patch(&state.prefs, &expected), "runtime_has": patch(&expected, &state.prefs)}));
        }
        0
    }

    /// The checks of the settings page as it is drawn now.
    fn check(&mut self, n: usize, ctx: &Context, state: &ViewState) {
        let rows = s::rows_now(ctx);
        let entry = self.entries[n].clone();
        let size = self.size_key();
        if rows.is_empty() {
            self.warn("settings_page_not_drawn", J::Null);
            return;
        }
        let Some(id) = entry.row() else { return };
        let mut checks = Map::new();
        match rows.iter().find(|r| r.id == id) {
            None => self.warn("row_not_drawn", json!(s::key(id))),
            Some(r) => {
                checks.insert(
                    "row".into(),
                    json!({"label": r.label, "group": r.group, "rect": rect_json(r.rect), "opacity": r.opacity, "enabled": r.enabled}),
                );
                let shown = r.clip.intersect(r.rect).height();
                if shown < r.rect.height() * 0.9 {
                    self.warn("row_not_fully_on_screen", json!({"rect": rect_json(r.rect), "visible_height": shown}));
                }
                if r.opacity < 0.99 {
                    self.warn("row_dimmed", json!(r.opacity));
                }
                if !r.enabled {
                    self.warn("row_disabled", J::Null);
                }
                let right = r.control.map_or(r.rect.right(), |c| c.right().max(r.rect.right()));
                if right > r.clip.right() + 1.0 {
                    self.warn("row_overflows_page", json!({"right": right, "page_right": r.clip.right()}));
                }
                if let Some(c) = r.control
                    && c.intersect(r.button).area() > 4.0
                    && c.intersects(r.button)
                {
                    self.warn("help_button_overlaps_control", json!({"button": rect_json(r.button), "control": rect_json(c)}));
                }
                for (i, a) in r.choices.iter().enumerate() {
                    for b in &r.choices[i + 1..] {
                        if a.intersects(*b) && a.intersect(*b).area() > 4.0 {
                            self.warn("choices_overlap", json!([rect_json(*a), rect_json(*b)]));
                        }
                    }
                    if a.right() > r.clip.right() + 1.0 {
                        self.warn("choice_off_page", json!(rect_json(*a)));
                    }
                }
                // A menu's chip shows at most 30 characters.
                if s::kind(id) == Kind::Menu
                    && let Some(i) = s::shown(id, &state.prefs)
                    && let Some(v) = s::values(id, &state.prefs).get(i)
                    && v.label.chars().count() > 30
                {
                    self.warn("menu_label_elided", json!(v.label));
                }
                if let Some((value, _)) = &entry.value {
                    self.heights
                        .entry(s::key(id))
                        .or_default()
                        .entry(size.clone())
                        .or_default()
                        .insert(value.clone(), r.rect.height());
                }
            }
        }
        // The option lit is the one set.
        for &(sid, i) in &entry.sets {
            if matches!(s::kind(sid), Kind::Segments | Kind::Menu)
                && let Some(lit) = s::shown(sid, &state.prefs)
                && lit != i
            {
                let label = |k: usize| s::values(sid, &state.prefs).get(k).map(|v| v.label.clone());
                self.warn("ui_shows_other_value", json!({"setting": s::key(sid), "set": label(i), "shows": label(lit)}));
            }
        }
        // Rows over each other on screen.
        let on: Vec<&DrawnRow> = rows.iter().filter(|r| visible(r)).collect();
        let mut overlaps = Vec::new();
        for (i, a) in on.iter().enumerate() {
            for b in &on[i + 1..] {
                if a.id != b.id && a.rect.intersects(b.rect) && a.rect.intersect(b.rect).area() > 16.0 {
                    overlaps.push(json!([s::key(a.id), s::key(b.id)]));
                }
            }
        }
        if !overlaps.is_empty() {
            overlaps.truncate(10);
            self.warn("rows_overlap", json!(overlaps));
        }
        // Against the baseline of this tab at this size.
        if let Some(base) = self.survey.get(&(self.size, id.tab())) {
            let now: Vec<&DrawnRow> = rows.iter().filter(|r| r.id.tab() == id.tab()).collect();
            let shown: Vec<String> = now.iter().filter(|r| !base.iter().any(|b| b.id == r.id)).map(|r| s::key(r.id)).collect();
            let hidden: Vec<String> = base.iter().filter(|b| !now.iter().any(|r| r.id == b.id)).map(|b| s::key(b.id)).collect();
            let changed: Vec<J> = now
                .iter()
                .filter(|r| r.id != id)
                .filter_map(|r| {
                    let b = base.iter().find(|b| b.id == r.id)?;
                    ((r.rect.height() - b.rect.height()).abs() > 1.0)
                        .then(|| json!({"row": s::key(r.id), "baseline": b.rect.height(), "now": r.rect.height()}))
                })
                .collect();
            checks.insert("rows_shown".into(), json!(shown));
            checks.insert("rows_hidden".into(), json!(hidden));
            if !changed.is_empty() {
                self.warn("rows_height_changed", json!(changed));
            }
        }
        if let Some(c) = self.cur.as_mut() {
            c.checks.extend(checks);
        }
    }

    /// The baseline: each group of each tab, from its first row.
    fn groups(&mut self, n: usize, steps: &mut Vec<String>) {
        let mut add = Vec::new();
        let mut list = Vec::new();
        for tab in 0..s::TABS.len() {
            let Some(rows) = self.survey.get(&(self.size, tab)) else { continue };
            let mut rows = rows.clone();
            rows.sort_by(|a, b| a.rect.top().total_cmp(&b.rect.top()));
            let mut seen: Vec<String> = Vec::new();
            for r in rows {
                if seen.contains(&r.group) {
                    continue;
                }
                let name = format!("settings-t{tab}-g{:02}", seen.len());
                seen.push(r.group.clone());
                list.push(json!({"file": name, "tab": s::TABS[tab], "group": r.group, "first_row": s::key(r.id)}));
                add.push(format!("sweep_open:{n}:{}", s::key(r.id)));
                add.push(format!("waitms:{}", if self.glow { 700 } else { 1800 }));
                add.extend(self.file(n, self.size, &name));
            }
        }
        if let Some(c) = self.cur.as_mut() {
            c.checks.insert("groups".into(), json!(list));
        }
        for (i, step) in add.into_iter().enumerate() {
            steps.insert(i, step);
        }
    }

    fn screen(&mut self, rest: &str, ctx: &Context, ui: &mut Md3eUi, state: &ViewState, out: &mut Vec<Command>, fakes: &mut crate::shot_fakes::Fakes) {
        let key = rest.rsplit(':').next().unwrap_or(rest);
        let Some(screen) = Screen::parse(key) else {
            log::warn!("sweep: unknown screen {key}");
            return;
        };
        if let Some(c) = self.cur.as_mut() {
            c.label = key.to_owned();
        }
        self.ensure_playing(state, out);
        let gap = matches!(screen, Screen::NpLyricsGap | Screen::NpLyricsCountdown);
        self.set_fakes(fakes, gap);
        let seek = |out: &mut Vec<Command>| out.push(Command::Seek(Duration::from_secs_f32(moment(screen))));
        match screen {
            Screen::Np | Screen::NpPaused | Screen::NpLyrics | Screen::NpLyricsGap | Screen::NpLyricsCountdown | Screen::NpQueue => {
                let slot = match screen {
                    Screen::Np | Screen::NpPaused => NpSlot::None,
                    Screen::NpQueue => NpSlot::Queue,
                    _ => NpSlot::Lyrics,
                };
                ui.sweep_now_playing(ctx, slot, false);
                seek(out);
                if screen == Screen::NpPaused {
                    out.push(Command::Pause);
                }
            }
            Screen::NpOpening => {
                ui.sweep_page(ctx, Route::Home, None, out);
                seek(out);
            }
            Screen::LyricsPane => {
                ui.sweep_page(ctx, Route::Home, Some(PaneTab::Lyrics), out);
                seek(out);
            }
            Screen::Queue => ui.sweep_page(ctx, Route::Home, Some(PaneTab::Queue), out),
            // From the lyrics beside the cover: their control opens it
            // (`sweep_lyrics_ui`, once the control is drawn).
            Screen::LyricsUi => {
                ui.sweep_now_playing(ctx, NpSlot::Lyrics, false);
                seek(out);
            }
            Screen::Home => ui.sweep_page(ctx, Route::Home, None, out),
            Screen::Library => ui.sweep_page(ctx, Route::Library(nikgob_view::LibraryTab::All), None, out),
            Screen::Album => {
                if let Some(r) = state.library.releases.first() {
                    ui.sweep_page(ctx, Route::Release(r.id), None, out);
                }
            }
        }
    }

    fn end(&mut self, n: usize) {
        let Some(c) = self.cur.take() else { return };
        let log = crate::logfile::capture_take();
        let size = self.size_key();
        let complete = self.size + 1 >= self.sizes.len();
        let e = self.entries[n].clone();
        let (base, expected) = (self.base.clone().unwrap_or_default(), self.expected.clone().unwrap_or_default());
        let meta = &mut self.metas[n];
        if meta.is_empty() {
            let sets: Vec<J> = e
                .sets
                .iter()
                .map(|&(id, i)| {
                    let v = s::values(id, &base).get(i).cloned();
                    json!({"setting": s::key(id), "value": v.as_ref().map(|v| v.key.clone()), "value_label": v.map(|v| v.label)})
                })
                .collect();
            let id = e.setting;
            meta.insert("setting".into(), json!(id.map(s::key)));
            meta.insert("value".into(), json!(e.value.as_ref().map(|v| v.0.clone())));
            meta.insert("value_label".into(), json!(e.value.as_ref().map(|v| v.1.clone())));
            meta.insert("combo".into(), json!(e.combo));
            meta.insert("baseline".into(), json!(e.baseline));
            meta.insert("sets".into(), json!(sets));
            if let Some(id) = id {
                meta.insert("kind".into(), json!(s::kind(id).name()));
                meta.insert("tab".into(), json!(s::tab_name(id)));
                meta.insert("help".into(), json!(s::help(id)));
                meta.insert("needs".into(), json!(s::needs(id)));
            }
            meta.insert("prefs_diff".into(), json!(diff(&base, &expected)));
            meta.insert("same_as_baseline".into(), json!(base == expected));
            let screens: Vec<J> = e.screens.iter().map(|x| json!({"key": x.key(), "about": x.about()})).collect();
            meta.insert("screens".into(), json!(screens));
            meta.insert("sizes".into(), json!({}));
        }
        let warnings = c.warnings.len() + log.iter().filter(|l| l.starts_with("ERROR")).count();
        let at_size = json!({
            "shots": c.shots,
            "checks": c.checks,
            "warnings": c.warnings,
            "frames": {"count": c.frames, "max_ms": c.max_ms.round(), "slow": c.spikes},
            "log": log,
            "took_ms": c.t0.elapsed().as_millis() as u64,
        });
        if let Some(J::Object(sizes)) = meta.get_mut("sizes") {
            sizes.insert(size, at_size);
        }
        meta.insert("complete".into(), json!(complete));
        let path = self.out.join(&e.dir).join("meta.json");
        if let Err(err) = std::fs::write(&path, serde_json::to_vec_pretty(&J::Object(meta.clone())).unwrap_or_default()) {
            log::warn!("sweep: cannot write {}: {err}", path.display());
        }
        let before = self.index[n].get("warnings").and_then(J::as_u64).unwrap_or(0) as usize;
        self.index[n] = json!({
            "dir": e.dir,
            "setting": e.setting.map(s::key),
            "value": e.value.as_ref().map(|v| v.0.clone()),
            "label": e.value.as_ref().map(|v| v.1.clone()),
            "combo": e.combo,
            "screens": e.screens.iter().map(|x| x.key()).collect::<Vec<_>>(),
            "warnings": before + warnings,
            "complete": complete,
        });
        if c.n % 10 == 0 {
            self.write_index(false);
        }
    }

    fn write_index(&self, done: bool) {
        let mut jumps = Vec::new();
        for (setting, sizes) in &self.heights {
            for (size, values) in sizes {
                let (lo, hi) = values.values().fold((f32::MAX, f32::MIN), |(lo, hi), h| (lo.min(*h), hi.max(*h)));
                if values.len() > 1 && hi - lo > 1.0 {
                    jumps.push(json!({"setting": setting, "size": size, "heights": values}));
                }
            }
        }
        let doc = json!({
            "done": done,
            "took_s": self.started.elapsed().as_secs(),
            "sizes": self.sizes,
            "format": self.ext,
            "plan": self.plan,
            "problems": self.problems,
            "screens": Screen::ALL.iter().map(|x| json!({"key": x.key(), "about": x.about()})).collect::<Vec<_>>(),
            "layout_jumps": jumps,
            "entries": self.index.iter().filter(|e| !e.is_null()).collect::<Vec<_>>(),
        });
        let path = self.out.join("index.json");
        if let Err(err) = std::fs::write(&path, serde_json::to_vec_pretty(&doc).unwrap_or_default()) {
            log::warn!("sweep: cannot write {}: {err}", path.display());
        }
    }

    /// The index, and the data dir's prefs back as they were.
    fn finish(&mut self, ui: &mut Md3eUi, out: &mut Vec<Command>, steps: &mut Vec<String>) {
        self.write_index(true);
        if let Some(base) = &self.base {
            out.push(Command::SetPrefs(Box::new(base.clone())));
        }
        ui.sweep_reset();
        out.push(Command::Pause);
        // Time for the runtime to write the prefs before the window closes.
        steps.insert(0, "waitms:2500".to_owned());
        log::info!("sweep: done in {} s, {}", self.started.elapsed().as_secs(), self.out.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every built-in combination names rows and values that exist.
    #[test]
    fn the_combos_name_real_values() {
        for base in [Prefs::default(), Prefs::first_start()] {
            for c in COMBOS {
                let (name, sets, screens) = builtin(c);
                let (found, bad) = resolve(&base, &sets);
                assert!(bad.is_empty(), "{name}: {bad:?}");
                assert_eq!(found.len(), sets.len(), "{name}");
                assert_eq!(screens.len(), c.2.len(), "{name}: a screen key is wrong");
            }
        }
    }

    #[test]
    fn diffs_name_the_fields_changed() {
        let a = Prefs::default();
        let b = Prefs { ring: !a.ring, ..a.clone() };
        let d = diff(&a, &b);
        assert_eq!(d.len(), 1);
        assert_eq!(d["ring"], json!([a.ring, b.ring]));
        assert_eq!(patch(&a, &b)["ring"], json!(b.ring));
    }
}
