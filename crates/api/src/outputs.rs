//! The outputs the API names: `local:default` (the system's), and
//! `local:<id>` for each device the player lists, the id made of its name
//! so it stays the same across restarts. A request may name one by its id,
//! its name, a part of its name, or (on Linux) a PulseAudio sink name.
//! Other devices of the home (`node:<device>`) come with the devices wave.

use serde::Serialize;
use utoipa::ToSchema;

pub const DEFAULT_ID: &str = "local:default";
pub const DEFAULT_NAME: &str = "По умолчанию";

/// The id of the local device named `name`: stable, short.
pub fn local_id(name: &str) -> String {
    let hex = blake3::hash(name.as_bytes()).to_hex();
    format!("local:{}", &hex.as_str()[..10])
}

/// A PulseAudio sink name (`alsa_output.usb-…analog-stereo`).
pub fn is_sink_name(name: &str) -> bool {
    ["alsa_output.", "bluez_output.", "bluez_sink.", "alsa_sink."].iter().any(|p| name.starts_with(p))
}

/// `GET /v1/outputs`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
pub struct Output {
    pub id: String,
    pub name: String,
    /// The system's own name of it when known: the PulseAudio sink on Linux.
    pub system_name: Option<String>,
    pub current: bool,
}

/// The system's output first, then each device.
pub fn list(devices: &[String], current: Option<&str>) -> Vec<Output> {
    let mut out = vec![Output { id: DEFAULT_ID.to_owned(), name: DEFAULT_NAME.to_owned(), system_name: None, current: current.is_none() }];
    let current_listed = current.is_some_and(|c| devices.iter().any(|d| d == c));
    for d in devices {
        out.push(Output {
            id: local_id(d),
            name: d.clone(),
            system_name: is_sink_name(d).then(|| d.clone()),
            current: current == Some(d.as_str()),
        });
    }
    // A sink chosen by its name that the list does not have.
    if let Some(c) = current.filter(|_| !current_listed) {
        out.push(Output { id: local_id(c), name: c.to_owned(), system_name: is_sink_name(c).then(|| c.to_owned()), current: true });
    }
    out
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
        let l = list(&d, Some(d[1].as_str()));
        assert_eq!(l[0].id, DEFAULT_ID);
        assert!(!l[0].current);
        assert!(l[2].current && l.iter().filter(|o| o.current).count() == 1);
        assert_eq!(l[4].system_name.as_deref(), Some(d[3].as_str()));
        assert_eq!(l[1].system_name, None);
        assert!(list(&d, None)[0].current);
        let sink = "alsa_output.pci-0000_00_1f.3.analog-stereo";
        let l = list(&d, Some(sink));
        assert_eq!(l.last().map(|o| (o.name.as_str(), o.current)), Some((sink, true)));
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

    #[test]
    fn a_sink_name_goes_as_it_is_on_linux() {
        let d = devices();
        let other = "alsa_output.usb-Other-00.analog-stereo";
        assert_eq!(resolve(other, &d, true), Pick::Device(other.to_owned()));
        assert_eq!(resolve(other, &d, false), Pick::NotFound);
        assert_eq!(resolve(&d[3], &d, false), Pick::Device(d[3].clone()));
    }
}
