//! `sources.toml`, next to `settings.toml`: the Qobuz and Tidal instance
//! lists the user can edit. Written once, with the built-in addresses and
//! a short header, when it is missing; never rewritten, so a later edit
//! and any key this version does not know stay as the user left them.
//! A missing list, or an empty one, is the built-in list.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use nikgob_core::settings::SourcesSettings;

/// What a service is asked, after the file is merged with the built-in list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lists {
    pub qobuz: Vec<String>,
    pub tidal: Vec<String>,
    /// The file names a Qobuz list that is not the built-in one.
    pub qobuz_own: bool,
    /// The file names a Tidal list that is not the built-in one.
    pub tidal_own: bool,
}

/// The addresses shipped in code, for a list the file does not set.
pub struct Defaults<'a> {
    pub qobuz: &'a [&'a str],
    pub tidal: &'a [&'a str],
}

impl Lists {
    /// Both services on their built-in lists (not the user's own).
    pub fn from_defaults(defaults: Defaults<'_>) -> Self {
        Self {
            qobuz: owned(defaults.qobuz),
            tidal: owned(defaults.tidal),
            qobuz_own: false,
            tidal_own: false,
        }
    }
}

/// `settings.toml`'s path → `sources.toml` beside it.
pub fn beside(settings: &Path) -> PathBuf {
    settings.with_file_name("sources.toml")
}

/// Reads `path`, creating it from `seed` (a list already in settings.toml)
/// or the built-in addresses when it is missing. A file that does not
/// parse is the built-in lists: the player's start does not depend on a
/// typo, and the file is left for the user to fix.
pub fn load(path: &Path, defaults: Defaults<'_>, seed: &SourcesSettings) -> Lists {
    if let Err(e) = ensure(path, defaults, seed) {
        log::warn!("sources.toml: {e}");
    }
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text, defaults).unwrap_or_else(|e| {
            log::warn!("sources.toml: {e}");
            Lists::from_defaults(defaults)
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Lists::from_defaults(defaults),
        Err(e) => {
            log::warn!("sources.toml: {e}");
            Lists::from_defaults(defaults)
        }
    }
}

/// Writes the file only when it is not there. An existing file, including
/// one with keys this version does not read, is left byte for byte.
pub fn ensure(path: &Path, defaults: Defaults<'_>, seed: &SourcesSettings) -> std::io::Result<()> {
    if path.is_file() {
        return Ok(());
    }
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir)?;
    }
    let text = template(defaults, seed);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)?;
    log::info!("sources.toml: wrote {}", path.display());
    Ok(())
}

/// The lists in `text`, missing and empty ones filled from `defaults`.
/// A list that differs from the built-in one is the user's own (nothing
/// else, such as an uptime list, replaces it). Unknown keys are ignored
/// here and stay in the file, which is not rewritten.
pub fn parse(text: &str, defaults: Defaults<'_>) -> Result<Lists, String> {
    let doc: toml::Value = toml::from_str(text).map_err(|e| e.to_string())?;
    let qobuz = field(&doc, "qobuz")?;
    let tidal = field(&doc, "tidal")?;
    Ok(merge(qobuz, tidal, defaults))
}

fn merge(qobuz: Option<Vec<String>>, tidal: Option<Vec<String>>, defaults: Defaults<'_>) -> Lists {
    let (qobuz, qobuz_own) = one(qobuz, defaults.qobuz);
    let (tidal, tidal_own) = one(tidal, defaults.tidal);
    Lists { qobuz, tidal, qobuz_own, tidal_own }
}

/// `None` or empty: the built-in list, not the user's. A non-empty list
/// that is not the built-in one is theirs.
fn one(list: Option<Vec<String>>, defaults: &[&str]) -> (Vec<String>, bool) {
    let Some(list) = list.filter(|l| !l.is_empty()) else {
        return (owned(defaults), false);
    };
    let own = !same(&list, defaults);
    (list, own)
}

fn field(doc: &toml::Value, section: &str) -> Result<Option<Vec<String>>, String> {
    let Some(table) = doc.as_table() else {
        return Err("sources.toml is not a table".into());
    };
    let Some(sec) = table.get(section) else { return Ok(None) };
    let Some(sec) = sec.as_table() else {
        return Err(format!("[{section}] is not a table"));
    };
    let Some(v) = sec.get("instances") else { return Ok(None) };
    let Some(arr) = v.as_array() else {
        return Err(format!("[{section}].instances is not a list"));
    };
    Ok(Some(
        arr.iter().filter_map(|i| i.as_str()).map(|s| s.trim().trim_end_matches('/').to_owned()).filter(|s| !s.is_empty()).collect(),
    ))
}

fn owned(urls: &[&str]) -> Vec<String> {
    urls.iter().map(|u| (*u).trim_end_matches('/').to_owned()).filter(|u| !u.is_empty()).collect()
}

fn same(list: &[String], defaults: &[&str]) -> bool {
    list == owned(defaults).as_slice()
}

fn seed_or<'a>(saved: &'a [String], defaults: &'a [&'a str]) -> Vec<String> {
    let saved: Vec<String> = saved.iter().map(|u| u.trim().trim_end_matches('/').to_owned()).filter(|u| !u.is_empty()).collect();
    if saved.is_empty() { owned(defaults) } else { saved }
}

fn toml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn list_block(urls: &[String]) -> String {
    if urls.is_empty() {
        return "instances = []\n".into();
    }
    let mut out = String::from("instances = [\n");
    for u in urls {
        out.push_str("  ");
        out.push_str(&toml_string(u));
        out.push_str(",\n");
    }
    out.push_str("]\n");
    out
}

fn template(defaults: Defaults<'_>, seed: &SourcesSettings) -> String {
    let qobuz = seed_or(&seed.qobuz_instances, defaults.qobuz);
    let tidal = seed_or(&seed.tidal_instances, defaults.tidal);
    format!(
        "\
# Инстансы бесплатных сервисов скачивания. Свой адрес или публичный — в список.
# Пустой список — встроенные адреса. Плеер читает файл, когда он меняется,
# и не перезаписывает правки.
#
# [qobuz] — Qobuz-DL (https://github.com/QobuzDL/Qobuz-DL), без /api на конце.
# [tidal] — hifi-api (https://github.com/binimum/hifi-api). Не класть сюда
# tracks.monochrome.st: у него другой API.

[qobuz]
# Qobuz-DL, без /api на конце.
{qobuz}
[tidal]
# hifi-api. Не tracks.monochrome.st.
{tidal}",
        qobuz = list_block(&qobuz),
        tidal = list_block(&tidal),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const QOBUZ: &[&str] = &["https://qobuz.example"];
    const TIDAL: &[&str] = &["https://tidal.example"];

    fn defaults() -> Defaults<'static> {
        Defaults { qobuz: QOBUZ, tidal: TIDAL }
    }

    #[test]
    fn a_missing_list_is_the_built_in_one_and_an_added_address_is_the_users() {
        let text = r#"
[qobuz]
instances = ["https://qobuz.example", "https://mine.example/"]

[notes]
left = "keep me"
"#;
        let lists = parse(text, defaults()).unwrap();
        assert_eq!(lists.qobuz, ["https://qobuz.example", "https://mine.example"]);
        assert!(lists.qobuz_own);
        assert_eq!(lists.tidal, ["https://tidal.example"]);
        assert!(!lists.tidal_own, "a missing list is not the user's own");
        let empty = parse("[qobuz]\ninstances = []\n", defaults()).unwrap();
        assert_eq!(empty.qobuz, ["https://qobuz.example"]);
        assert!(!empty.qobuz_own);
        let same = parse("[tidal]\ninstances = [\"https://tidal.example/\"]\n", defaults()).unwrap();
        assert!(!same.tidal_own, "the built-in list, even written out, is not the user's own");
    }

    #[test]
    fn an_existing_file_is_not_rewritten_and_unknown_keys_stay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sources.toml");
        let text = "# mine\n[qobuz]\ninstances = [\"https://mine.example\"]\n\n[extra]\nnote = \"leave me\"\n";
        std::fs::write(&path, text).unwrap();
        ensure(&path, defaults(), &SourcesSettings::default()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text, "an existing file is not touched");
        let lists = parse(&std::fs::read_to_string(&path).unwrap(), defaults()).unwrap();
        assert_eq!(lists.qobuz, ["https://mine.example"]);
        assert!(lists.qobuz_own);
        let created = dir.path().join("fresh.toml");
        ensure(&created, defaults(), &SourcesSettings {
            qobuz_instances: vec!["https://from-settings.example".into()],
            ..SourcesSettings::default()
        })
        .unwrap();
        let fresh = std::fs::read_to_string(&created).unwrap();
        assert!(fresh.contains("https://from-settings.example"), "{fresh}");
        assert!(fresh.contains("https://tidal.example"), "{fresh}");
        assert!(fresh.starts_with('#'));
        ensure(&created, defaults(), &SourcesSettings::default()).unwrap();
        assert_eq!(std::fs::read_to_string(&created).unwrap(), fresh, "a second ensure does not overwrite");
    }

    #[test]
    fn a_broken_file_is_told_and_not_taken_for_the_lists() {
        assert!(parse("instances = [", defaults()).is_err());
        assert!(parse("[qobuz]\ninstances = \"no\"\n", defaults()).is_err());
    }
}
