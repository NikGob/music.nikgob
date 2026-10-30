//! The graph of similar music on a copy of a real library
//! (docs/recs-graph.md §8): with `NIKGOB_GRAPH_FETCH=minutes` the runtime
//! runs its background pass against the real YouTube Music, Deezer,
//! MusicBrainz and ListenBrainz for that long (their state in a folder next
//! to the copy), then the copy is measured with the graph and without it:
//! the prequential replay of its history (hit@10), a track's radio from the
//! most played songs (its artists, its largest world, how much of it is
//! known), the taste profiles and «Миксы для тебя» (their largest world).
//!
//! `NIKGOB_GRAPH_DB=<copy>/library.sqlite [NIKGOB_GRAPH_FETCH=40]
//! [NIKGOB_GRAPH_OUT=<dir>] cargo test -p nikgob --test real_graph --
//! --ignored --nocapture`
//!
//! Never point it at the library in use: it writes to the database it is
//! given.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_core::model::{EventContext, EventOrigin, EventType, TrackId};
use nikgob_core::streaming::StreamService;
use nikgob_library::Library;
use nikgob_recs::mixes::{Builder, Kept, Settings};
use nikgob_recs::{DayPart, Ended, Item, Live, RadioParams, RadioSeed, RadioState, Recs, Session};
use nikgob_runtime::{Metadata, MetadataConfig, ModelPreview, NullPlayback, Runtime, RuntimeConfig, preview_replay_with};
use nikgob_view::Prefs;
use serde_json::{Value, json};

fn fetch(lib: &Library, dir: &std::path::Path, minutes: u64) {
    let mut streams: Vec<Arc<dyn StreamService>> = Vec::new();
    streams.push(nikgob_youtube::Youtube::new(nikgob_youtube::YoutubeConfig::new(dir.join("youtube"))).unwrap());
    streams.push(nikgob_deezer::Deezer::new(nikgob_deezer::DeezerConfig::default()).unwrap());
    let empty = dir.join("root");
    std::fs::create_dir_all(&empty).unwrap();
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
            streams,
            library_dir: empty,
            prefs: Prefs::default(),
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    let meta = || Metadata::new(&MetadataConfig::new("music.nikgob graph check")).unwrap();
    rt.set_graph_metadata(meta());
    let start = Instant::now();
    let mut last = Instant::now();
    while start.elapsed() < Duration::from_secs(60 * minutes) {
        std::thread::sleep(Duration::from_secs(20));
        let asked = lib.graph_fetches().unwrap().len();
        println!("graph: {asked} answers after {} s", start.elapsed().as_secs());
        // A pass is over: the next one now.
        if last.elapsed() > Duration::from_secs(300) {
            rt.set_graph_metadata(meta());
            last = Instant::now();
        }
    }
    drop(rt);
}

fn round(x: f32) -> f32 {
    (x * 1000.0).round() / 1000.0
}

/// A track's radio from the most played songs of the history.
fn radio(lib: &Library, p: &ModelPreview) -> Value {
    let m = p.model();
    let (cat, idx) = p.catalog();
    let recs = Recs::new(cat, &m.taste, idx).with_model(m);
    let events = lib.listening_events().unwrap();
    let mut starts: HashMap<TrackId, u32> = HashMap::new();
    for e in events.iter().filter(|e| e.event_type == EventType::Start) {
        *starts.entry(e.track_id).or_default() += 1;
    }
    let mut top: Vec<(TrackId, u32)> = starts.into_iter().filter(|(t, _)| m.position(*t).is_some()).collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let now = nikgob_library::now_ms();
    let (mut artists, mut world, mut known, mut same_profile, mut n, mut runs) = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0usize, 0usize);
    for (k, (seed, _)) in top.iter().take(12).enumerate() {
        let mut s = Session::default();
        s.start(*seed, now - 200_000, EventContext::Radio, Some(EventOrigin::Manual));
        s.end(*seed, Ended::Complete);
        let mut r = RadioState::new(RadioSeed::Track(*seed));
        r.attune(s, None);
        let picks: Vec<usize> = r
            .extend(&recs, 15, &RadioParams { new_share: 0.0, seed: k as u64 })
            .into_iter()
            .filter_map(|p| match p.item {
                Item::Track(t) => m.position(t),
                Item::Stream(_) => None,
            })
            .collect();
        if picks.is_empty() {
            continue;
        }
        let len = picks.len() as f32;
        artists += picks.iter().filter_map(|&i| m.main_artist_at(i)).collect::<HashSet<_>>().len() as f32 / len;
        let mut per: HashMap<u64, usize> = HashMap::new();
        for &i in &picks {
            if let Some(w) = m.world_at(i) {
                *per.entry(w).or_default() += 1;
            }
        }
        world += per.values().copied().max().unwrap_or(0) as f32 / len;
        known += picks.iter().filter(|&&i| m.tracks[i].completes > 0).count() as f32 / len;
        let sp = m.position(*seed).and_then(|i| m.profile_at(i));
        same_profile += picks.iter().filter(|&&i| sp.is_some() && m.profile_at(i) == sp).count() as f32 / len;
        n += picks.len();
        runs += 1;
    }
    let r = runs.max(1) as f32;
    json!({ "seeds": runs, "picks": n, "distinct_artists": round(artists / r), "top_world_share": round(world / r),
            "heard_out_before": round(known / r), "same_profile": round(same_profile / r) })
}

fn mixes(p: &ModelPreview) -> Value {
    let m = p.model();
    let (cat, idx) = p.catalog();
    let recs = Recs::new(cat, &m.taste, idx).with_model(m);
    let ok = |_: usize| true;
    let suits = |_: usize| true;
    let b = Builder::new(&recs, m, &ok, &suits, 7, DayPart::Evening, Live::default(), Settings::default());
    let (lineup, _) = b.lineup(&Kept::default());
    let mut worst = 0.0f32;
    let mut artists = 0.0f32;
    for mix in &lineup {
        let mut per: HashMap<u64, usize> = HashMap::new();
        for &i in &mix.tracks {
            if let Some(w) = m.world_at(i) {
                *per.entry(w).or_default() += 1;
            }
        }
        worst = worst.max(per.values().copied().max().unwrap_or(0) as f32 / mix.tracks.len().max(1) as f32);
        artists += mix.tracks.iter().filter_map(|&i| m.main_artist_at(i)).collect::<HashSet<_>>().len() as f32 / mix.tracks.len().max(1) as f32;
    }
    json!({
        "profiles": m.profiles.len(),
        "profile_shares": m.profile_share.iter().map(|s| round(*s)).collect::<Vec<_>>(),
        "mixes": lineup.iter().map(|x| x.title.clone()).collect::<Vec<_>>(),
        "mix_worst_world_share": round(worst),
        "mix_distinct_artists": round(artists / lineup.len().max(1) as f32),
    })
}

#[test]
#[ignore = "needs NIKGOB_GRAPH_DB (a copy), and the network with NIKGOB_GRAPH_FETCH"]
fn the_graph_on_a_copy_of_a_real_library() {
    let Some(db) = std::env::var_os("NIKGOB_GRAPH_DB") else { return };
    let db = PathBuf::from(db);
    let dir = db.parent().unwrap().to_owned();
    let _ = env_logger::builder().is_test(true).filter_level(log::LevelFilter::Info).try_init();
    let lib = Library::open(&db).unwrap();
    if let Some(minutes) = std::env::var("NIKGOB_GRAPH_FETCH").ok().and_then(|m| m.parse().ok()) {
        fetch(&lib, &dir, minutes);
    }
    let mut by_source: HashMap<String, usize> = HashMap::new();
    for r in lib.graph_edges().unwrap() {
        *by_source.entry(r.source).or_default() += 1;
    }
    let mut out = serde_json::Map::new();
    out.insert("kept_entries".into(), json!(by_source));
    out.insert("answers".into(), json!(lib.graph_fetches().unwrap().len()));
    let variants = [
        ("today", Prefs { graph_use: false, graph_personal: false, library_as_taste: false, ..Prefs::default() }),
        ("graph", Prefs { graph_personal: false, ..Prefs::default() }),
        ("graph_personal", Prefs::default()),
    ];
    for (name, prefs) in variants {
        let r = preview_replay_with(&lib, &prefs).unwrap();
        let h = |s: &str, k: &str| {
            let x = r.get(s, k);
            json!({ "n": x.n, "hit1": round(x.hit1), "hit10": round(x.hit10), "mrr": round(x.mrr) })
        };
        let p = ModelPreview::of_with(&lib, &prefs).unwrap();
        let (te, ae) = p.graph_size();
        let row = json!({
            "graph_edges": { "tracks": te, "artists": ae },
            "replay": { "all_learned": h("all", "learned"), "all_hand": h("all", "hand"), "free_learned": h("free", "learned"),
                        "free_second_half": h("free:second_half", "learned"), "coverage": round(r.coverage) },
            "radio": radio(&lib, &p),
            "mixes": mixes(&p),
        });
        out.insert(name.into(), row);
    }
    let v = Value::Object(out);
    println!("{}", serde_json::to_string_pretty(&v).unwrap());
    if let Some(o) = std::env::var_os("NIKGOB_GRAPH_OUT") {
        std::fs::create_dir_all(&o).unwrap();
        std::fs::write(PathBuf::from(o).join("owner_copy.json"), serde_json::to_string_pretty(&v).unwrap()).unwrap();
    }
}
