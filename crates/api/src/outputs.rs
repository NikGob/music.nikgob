//! The outputs the API names: `local:default` (the system's), and
//! `local:<id>` for each device the player lists, the id made of its name
//! so it stays the same across restarts. A request may name one by its id,
//! its name, a part of its name, or (on Linux) a PulseAudio sink name.
//! The paired devices of the home online are listed too, `node:<id>`: the
//! music moves there.

use serde::Serialize;
use utoipa::ToSchema;

use nikgob_view::MeshNodeView;

pub const DEFAULT_ID: &str = "local:default";
pub const DEFAULT_NAME: &str = "По умолчанию";
/// The id of a device of the home begins with this.
pub const NODE_PREFIX: &str = "node:";

/// The id of the local device named `name`: stable, short.
pub fn local_id(name: &str) -> String {
    let hex = blake3::hash(name.as_bytes()).to_hex();
    format!("local:{}", &hex.as_str()[..10])
}

/// A PulseAudio sink name (`alsa_output.usb-…analog-stereo`).
pub fn is_sink_name(name: &str) -> bool {
    ["alsa_output.", "bluez_output.", "bluez_sink.", "alsa_sink."].iter().any(|p| name.starts_with(p))
}

/// What an output is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum OutputKind {
    /// A sound device of this computer.
    Local,
    /// Another device of the home: the music moves there.
    Node,
}

/// `GET /v1/outputs`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
pub struct Output {
    pub id: String,
    pub name: String,
    /// The system's own name of it when known: the PulseAudio sink on Linux.
    pub system_name: Option<String>,
    pub current: bool,
    pub kind: OutputKind,
    /// A device of the home plays music now (`node` only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playing: Option<bool>,
}

/// The system's output first, then each device. `sinks` pairs a shown name
/// with the system's own id of it (the PulseAudio sink on Linux), where known.
pub fn list(devices: &[String], current: Option<&str>, sinks: &[(String, String)]) -> Vec<Output> {
    let sink_of = |d: &str| sinks.iter().find(|(n, _)| n == d).map(|(_, id)| id.clone()).or_else(|| is_sink_name(d).then(|| d.to_owned()));
    // A current device chosen by its sink name is the listed device with that sink.
    let current_name = current.map(|c| sinks.iter().find(|(_, id)| id == c).map_or(c, |(n, _)| n.as_str()));
    let local = |id: String, name: String, system_name: Option<String>, current: bool| Output { id, name, system_name, current, kind: OutputKind::Local, playing: None };
    let mut out = vec![local(DEFAULT_ID.to_owned(), DEFAULT_NAME.to_owned(), None, current.is_none())];
    let current_listed = current_name.is_some_and(|c| devices.iter().any(|d| d == c));
    for d in devices {
        out.push(local(local_id(d), d.clone(), sink_of(d), current_name == Some(d.as_str())));
    }
    // A sink chosen by its name that the list does not have.
    if let Some(c) = current_name.filter(|_| !current_listed) {
        out.push(local(local_id(c), c.to_owned(), sink_of(c), true));
    }
    out
}

/// The paired devices of the home online (and `target`, the one the
/// window drives, whatever its link), as outputs.
pub fn nodes(nodes: &[MeshNodeView], target: Option<&str>) -> Vec<Output> {
    nodes
        .iter()
        .filter(|n| n.online || target == Some(n.id.as_str()))
        .map(|n| Output {
            id: format!("{NODE_PREFIX}{}", n.id),
            name: n.name.clone(),
            system_name: None,
            current: target == Some(n.id.as_str()),
            kind: OutputKind::Node,
            playing: Some(n.playing.as_ref().is_some_and(|p| !p.paused)),
        })
        .collect()
}

/// What a request's `output` names among the devices of the home.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodePick {
    /// No device of the home: a sound device here, maybe.
    NoNode,
    /// `this` (or this device's own `node:` id): here.
    This,
    /// A paired device online, by its id.
    Node(String),
    /// A paired device that is not online now: its name.
    Offline(String),
    /// `node:` of no paired device.
    Unknown,
}

/// Resolves `query` among the paired `nodes` (`this` is this device's id):
/// `node:<id>` (or the start of an id, 8 characters at least, of one
/// device only), a device's whole name, or `this`.
pub fn node_pick(query: &str, this: &str, nodes: &[MeshNodeView]) -> NodePick {
    let q = query.trim();
    let qw = words(q);
    if ["this", "here", "здесь", "сюда"].contains(&qw.as_str()) {
        return NodePick::This;
    }
    let pick = |n: &MeshNodeView| if n.online { NodePick::Node(n.id.clone()) } else { NodePick::Offline(n.name.clone()) };
    let Some(id) = q.strip_prefix(NODE_PREFIX).map(str::trim) else {
        // A device by its whole name («Raspberry Pi»).
        return nodes
            .iter()
            .find(|n| !qw.is_empty() && (n.name.trim().eq_ignore_ascii_case(q) || words(&n.name) == qw))
            .map_or(NodePick::NoNode, pick);
    };
    if id.is_empty() {
        return NodePick::Unknown;
    }
    if id == this {
        return NodePick::This;
    }
    if let Some(n) = nodes.iter().find(|n| n.id == id) {
        return pick(n);
    }
    let mut starting = nodes.iter().filter(|n| id.len() >= 8 && n.id.starts_with(id));
    match (starting.next(), starting.next()) {
        (Some(n), None) => pick(n),
        _ => NodePick::Unknown,
    }
}

/// What a request's `output` means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    /// The system's output.
    Default,
    /// A device by the name the player knows it by (or a sink name).
    Device(String),
    NotFound,
}

/// Lowercase letters and digits, words split by one space.
fn words(s: &str) -> String {
    let mapped: String = s
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| if c == 'ё' { 'е' } else if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    mapped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Resolves `query` among `devices`. `linux`: a sink name the list does
/// not have is taken as it is (the engine finds it in PulseAudio).
pub fn resolve(query: &str, devices: &[String], linux: bool) -> Pick {
    let q = query.trim();
    if q.is_empty() {
        return Pick::NotFound;
    }
    let qw = words(q);
    if q == DEFAULT_ID || ["default", "по умолчанию", "системное", "системное по умолчанию", "system"].contains(&qw.as_str()) {
        return Pick::Default;
    }
    if q.starts_with("local:") {
        return devices.iter().find(|d| local_id(d) == q).map_or(Pick::NotFound, |d| Pick::Device(d.clone()));
    }
    if q.contains(':') && !is_sink_name(q) {
        // `node:…` and anything else of another kind.
        return Pick::NotFound;
    }
    if let Some(d) = devices.iter().find(|d| d.eq_ignore_ascii_case(q) || words(d) == qw) {
        return Pick::Device(d.clone());
    }
    if linux && is_sink_name(q) {
        return Pick::Device(q.to_owned());
    }
    if qw.is_empty() {
        return Pick::NotFound;
    }
    // Every word of the query in the name; the shortest such name is the
    // most specific.
    let terms: Vec<&str> = qw.split(' ').collect();
    devices
        .iter()
        .filter(|d| {
            let dw = words(d);
            terms.iter().all(|t| dw.contains(t))
        })
        .min_by_key(|d| d.chars().count())
        .map_or(Pick::NotFound, |d| Pick::Device(d.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn devices() -> Vec<String> {
        vec![
            "Динамики (Realtek(R) Audio)".to_owned(),
            "Наушники (USB Audio Device)".to_owned(),
            "Наушники (USB Audio Device) 2".to_owned(),
            "alsa_output.usb-3142_fifine_Headset-00.analog-stereo".to_owned(),
        ]
    }

    #[test]
    fn ids_are_stable_and_distinct() {
        let d = devices();
        let a: Vec<String> = d.iter().map(|n| local_id(n)).collect();
        let b: Vec<String> = d.iter().map(|n| local_id(n)).collect();
        assert_eq!(a, b, "the same name, the same id (a restart changes nothing)");
        assert_eq!(a.iter().collect::<std::collections::HashSet<_>>().len(), a.len());
        for id in &a {
            assert!(id.starts_with("local:") && id.len() == "local:".len() + 10, "{id}");
        }
        // A known value: the id is part of the API.
        assert_eq!(local_id(""), "local:af1349b9f5");
    }

    #[test]
    fn the_list_has_the_default_first_and_the_current_marked() {
        let d = devices();
        let l = list(&d, Some(d[1].as_str()), &[]);
        assert_eq!(l[0].id, DEFAULT_ID);
        assert!(!l[0].current);
        assert!(l[2].current && l.iter().filter(|o| o.current).count() == 1);
        assert_eq!(l[4].system_name.as_deref(), Some(d[3].as_str()));
        assert_eq!(l[1].system_name, None);
        assert!(list(&d, None, &[])[0].current);
        let sink = "alsa_output.pci-0000_00_1f.3.analog-stereo";
        let l = list(&d, Some(sink), &[]);
        assert_eq!(l.last().map(|o| (o.name.as_str(), o.current)), Some((sink, true)));
    }

    #[test]
    fn a_device_shows_its_sink_and_is_current_when_chosen_by_it() {
        let d = vec!["fifine Headset Analog Stereo".to_owned()];
        let sink = "alsa_output.usb-3142_fifine_Headset-00.analog-stereo";
        let sinks = vec![(d[0].clone(), sink.to_owned())];
        let l = list(&d, Some(sink), &sinks);
        assert_eq!(l.len(), 2);
        assert_eq!(l[1].system_name.as_deref(), Some(sink));
        assert!(l[1].current && !l[0].current);
    }

    #[test]
    fn a_device_is_found_by_id_name_or_part_of_it() {
        let d = devices();
        assert_eq!(resolve("local:default", &d, false), Pick::Default);
        assert_eq!(resolve("По умолчанию", &d, false), Pick::Default);
        assert_eq!(resolve(&local_id(&d[0]), &d, false), Pick::Device(d[0].clone()));
        assert_eq!(resolve("local:0000000000", &d, false), Pick::NotFound);
        assert_eq!(resolve("node:pi", &d, false), Pick::NotFound);
        assert_eq!(resolve("динамики (realtek(r) audio)", &d, false), Pick::Device(d[0].clone()));
        assert_eq!(resolve("realtek", &d, false), Pick::Device(d[0].clone()));
        assert_eq!(resolve("наушники", &d, false), Pick::Device(d[1].clone()), "the shortest of two");
        assert_eq!(resolve("наушники 2", &d, false), Pick::Device(d[2].clone()));
        assert_eq!(resolve("fifine", &d, false), Pick::Device(d[3].clone()));
        assert_eq!(resolve("hdmi", &d, false), Pick::NotFound);
        assert_eq!(resolve("  ", &d, false), Pick::NotFound);
    }

    /// A device of the home is found by its `node:` id, the start of one,
    /// or its whole name; one offline says so; `this` is here.
    #[test]
    fn a_device_of_the_home_is_found() {
        let node = |id: &str, name: &str, online: bool| MeshNodeView { id: id.into(), name: name.into(), online, ..MeshNodeView::default() };
        let nodes = vec![node("aaaa1111bbbb2222", "Raspberry Pi", true), node("cccc3333dddd4444", "Ноутбук", false), node("aaaa1111eeee5555", "TV", true)];
        assert_eq!(node_pick("node:aaaa1111bbbb2222", "this-id", &nodes), NodePick::Node("aaaa1111bbbb2222".into()));
        assert_eq!(node_pick("node:cccc3333", "this-id", &nodes), NodePick::Offline("Ноутбук".into()));
        assert_eq!(node_pick("node:aaaa1111", "this-id", &nodes), NodePick::Unknown, "the start of two");
        assert_eq!(node_pick("node:aaaa", "this-id", &nodes), NodePick::Unknown, "too short");
        assert_eq!(node_pick("node:this-id", "this-id", &nodes), NodePick::This);
        assert_eq!(node_pick("this", "this-id", &nodes), NodePick::This);
        assert_eq!(node_pick("raspberry pi", "this-id", &nodes), NodePick::Node("aaaa1111bbbb2222".into()));
        assert_eq!(node_pick("node:", "this-id", &nodes), NodePick::Unknown);
        assert_eq!(node_pick("наушники", "this-id", &nodes), NodePick::NoNode);
        assert_eq!(node_pick("local:default", "this-id", &nodes), NodePick::NoNode);

        let mut playing = nodes.clone();
        playing[2].playing = Some(nikgob_view::MeshPlaying { title: "T".into(), ..Default::default() });
        let listed = super::nodes(&playing, Some("cccc3333dddd4444"));
        let ids: Vec<&str> = listed.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(ids, vec!["node:aaaa1111bbbb2222", "node:cccc3333dddd4444", "node:aaaa1111eeee5555"], "the target though offline");
        assert_eq!(listed.iter().map(|o| (o.current, o.playing)).collect::<Vec<_>>(), vec![(false, Some(false)), (true, Some(false)), (false, Some(true))]);
        assert!(listed.iter().all(|o| o.kind == OutputKind::Node));
        assert!(super::nodes(&nodes, None).iter().all(|o| o.id != "node:cccc3333dddd4444"), "offline, not listed");
        assert_eq!(serde_json::to_value(&listed[0]).unwrap()["kind"], "node");
        assert!(serde_json::to_value(&list(&[], None, &[])[0]).unwrap().get("playing").is_none());
    }

    #[test]
    fn a_sink_name_goes_as_it_is_on_linux() {
        let d = devices();
        let other = "alsa_output.usb-Other-00.analog-stereo";
        assert_eq!(resolve(other, &d, true), Pick::Device(other.to_owned()));
        assert_eq!(resolve(other, &d, false), Pick::NotFound);
        assert_eq!(resolve(&d[3], &d, false), Pick::Device(d[3].clone()));
    }
}
