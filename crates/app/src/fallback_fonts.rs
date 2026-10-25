//! Installed faces for the scripts Roboto Flex does not draw, mapped from
//! their files.
//!
//! `fastframe_fonts::system::fallbacks` reads every face it chooses whole
//! and keeps it for the life of the process: 58 MB of private memory on a
//! stock Windows, most of it the CJK collections (msyh.ttc, YuGothM.ttc,
//! malgun.ttf), of which a library draws a few hundred glyphs. This chooses
//! the same faces the same way (its ranking, kept in step by
//! `the_same_faces_as_fastframe`) and maps the files instead, so only the
//! pages egui reads are resident, clean and shared with every other program
//! drawing those fonts. macOS asks CoreText, which is left to fastframe.

use std::sync::OnceLock;

/// A face to register behind every family.
pub struct Face {
    /// The font data key, `fallback-<script>` after the first script that
    /// chose the face.
    pub name: String,
    pub bytes: &'static [u8],
    pub index: u32,
    pub scale: f32,
    pub y_offset_factor: f32,
}

/// The faces, chosen once per process.
pub fn faces() -> &'static [Face] {
    static FACES: OnceLock<Vec<Face>> = OnceLock::new();
    FACES.get_or_init(|| {
        #[cfg(not(target_os = "macos"))]
        let faces = scan::faces();
        #[cfg(target_os = "macos")]
        let faces = fastframe_fonts::system::fallbacks()
            .iter()
            .map(|f| Face {
                name: f.name.clone(),
                bytes: &f.bytes,
                index: f.index,
                scale: f.scale,
                y_offset_factor: f.y_offset_factor,
            })
            .collect();
        faces
    })
}

#[cfg(not(target_os = "macos"))]
mod scan {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use skrifa::MetadataProvider as _;
    use skrifa::raw::TableProvider as _;

    use super::Face;

    /// fastframe's scripts, probes and family hints, in its order.
    const SCRIPTS: &[(&str, char, &str)] = &[
        ("han", '\u{4e2d}', "cjk"),
        ("kana", '\u{3042}', "cjk"),
        ("hangul", '\u{d55c}', "cjk"),
        ("arabic", '\u{0627}', "arabic"),
        ("hebrew", '\u{05d0}', "hebrew"),
        ("thai", '\u{0e01}', "thai"),
        ("lao", '\u{0e81}', "lao"),
        ("khmer", '\u{1780}', "khmer"),
        ("myanmar", '\u{1000}', "myanmar"),
        ("devanagari", '\u{0915}', "devanagari"),
        ("bengali", '\u{0995}', "bengali"),
        ("gurmukhi", '\u{0a15}', "gurmukhi"),
        ("gujarati", '\u{0a95}', "gujarati"),
        ("tamil", '\u{0ba4}', "tamil"),
        ("telugu", '\u{0c15}', "telugu"),
        ("kannada", '\u{0c95}', "kannada"),
        ("malayalam", '\u{0d15}', "malayalam"),
        ("sinhala", '\u{0d9a}', "sinhala"),
        ("armenian", '\u{0531}', "armenian"),
        ("georgian", '\u{10d0}', "georgian"),
        ("ethiopic", '\u{1200}', "ethiopic"),
        ("cherokee", '\u{13a0}', "cherokee"),
        ("yi", '\u{a248}', "yi"),
        ("javanese", '\u{a9c1}', "javanese"),
        ("math", '\u{1d4d0}', "math"),
        ("enclosed", '\u{24b6}', "symbol"),
        ("symbols", '\u{2605}', "symbol"),
        ("suits", '\u{2661}', "symbol"),
    ];
    const SCAN_DEPTH: usize = 4;
    const MAX_FACES: u32 = 64;
    const HAN_REGIONS: &[(&str, &str)] = &[
        ("zh_tw", "tc"),
        ("zh_hant", "tc"),
        ("zh_hk", "hk"),
        ("zh_mo", "hk"),
        ("zh", "sc"),
        ("ja", "jp"),
        ("ko", "kr"),
    ];
    /// Inter's baseline from the centre of its line box and its x-height,
    /// in ems.
    const INTER_BASELINE_CENTER: f32 = (1984.0 / 2048.0) - 0.5 * ((1984.0 + 494.0) / 2048.0);
    const INTER_X_HEIGHT: f32 = 1118.0 / 2048.0;

    struct Candidate {
        foreign: bool,
        score: u32,
        path: PathBuf,
        index: u32,
    }

    pub(super) fn faces() -> Vec<Face> {
        let han = han_region(&locale());
        let mut best: BTreeMap<&'static str, Candidate> = BTreeMap::new();
        for dir in fastframe_fonts::system::font_directories() {
            probe_dir(&dir, 0, han, &mut best);
        }
        let mut mapped: Vec<(PathBuf, &'static [u8])> = Vec::new();
        let mut taken: Vec<(PathBuf, u32)> = Vec::new();
        let mut faces = Vec::new();
        for (script, _, _) in SCRIPTS {
            let Some(candidate) = best.get(script) else {
                continue;
            };
            if taken.contains(&(candidate.path.clone(), candidate.index)) {
                continue;
            }
            let bytes = match mapped.iter().find(|(p, _)| *p == candidate.path) {
                Some((_, bytes)) => *bytes,
                None => {
                    let Some(map) = map(&candidate.path) else {
                        log::warn!("cannot map {}", candidate.path.display());
                        continue;
                    };
                    // Mapped for as long as egui may draw from it.
                    let bytes: &'static [u8] = Box::leak(Box::new(map));
                    mapped.push((candidate.path.clone(), bytes));
                    bytes
                }
            };
            taken.push((candidate.path.clone(), candidate.index));
            let scale = if *script == "arabic" { arabic_scale(bytes, candidate.index) } else { 1.0 };
            let y_offset_factor = baseline_center(bytes, candidate.index)
                .map(|center| INTER_BASELINE_CENTER - center)
                .filter(|offset| offset.abs() > 0.001)
                .unwrap_or(0.0);
            log::info!(
                "{script} fallback: {} ({}, face {}), scale {scale:.2}, baseline {y_offset_factor:+.3} em",
                family_name(bytes, candidate.index).unwrap_or_else(|| "unnamed".into()),
                candidate.path.display(),
                candidate.index,
            );
            faces.push(Face {
                name: format!("fallback-{script}"),
                bytes,
                index: candidate.index,
                scale,
                y_offset_factor,
            });
        }
        faces
    }

    /// A read-only map of a font file. Windows refuses to replace a file
    /// while it is mapped, so a font update waits for the next start.
    #[allow(unsafe_code)]
    fn map(path: &Path) -> Option<memmap2::Mmap> {
        let file = std::fs::File::open(path).ok()?;
        // SAFETY: the map is read-only; the file is a system font nothing
        // writes in place.
        unsafe { memmap2::Mmap::map(&file) }.ok()
    }

    fn probe_dir(dir: &Path, depth: usize, han: &str, best: &mut BTreeMap<&'static str, Candidate>) {
        if depth >= SCAN_DEPTH {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() || (kind.is_symlink() && path.is_dir()) {
                probe_dir(&path, depth + 1, han, best);
            } else if is_font_file(&path) {
                probe_file(&path, han, best);
            }
        }
    }

    fn is_font_file(path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc" | "otc"))
    }

    fn file_faces(data: &[u8]) -> Vec<(u32, skrifa::FontRef<'_>)> {
        match skrifa::raw::FileRef::new(data) {
            Ok(skrifa::raw::FileRef::Font(font)) => vec![(0, font)],
            Ok(skrifa::raw::FileRef::Collection(collection)) => (0..collection.len().min(MAX_FACES))
                .filter_map(|index| collection.get(index).ok().map(|font| (index, font)))
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    fn draws(font: &skrifa::FontRef<'_>, character: char) -> bool {
        let outlines = font.outline_glyphs();
        font.charmap().map(character).is_some_and(|glyph| outlines.get(glyph).is_some())
    }

    fn probe_file(path: &Path, han: &str, best: &mut BTreeMap<&'static str, Candidate>) {
        let Some(data) = map(path) else {
            return;
        };
        for (index, font) in file_faces(&data) {
            let attributes = font.attributes();
            if attributes.style != skrifa::attribute::Style::Normal {
                continue;
            }
            let family = font
                .localized_strings(skrifa::string::StringId::FAMILY_NAME)
                .english_or_first()
                .map(|name| name.to_string())
                .unwrap_or_default()
                .to_lowercase();
            for (script, probe, hint) in SCRIPTS {
                if !draws(&font, *probe) {
                    continue;
                }
                let score = face_score(&family, attributes.weight.value(), han, hint);
                let foreign = *script == "han" && {
                    let code_pages = font.os2().ok().and_then(|os2| os2.ul_code_page_range_1());
                    !covers_han_region(code_pages, han)
                };
                if best
                    .get(script)
                    .is_none_or(|held| (foreign, score, path) < (held.foreign, held.score, held.path.as_path()))
                {
                    best.insert(script, Candidate { foreign, score, path: path.to_path_buf(), index });
                }
            }
        }
    }

    fn face_score(family: &str, weight: f32, han: &str, hint: &str) -> u32 {
        let mut score = ((weight - 400.0).abs() / 25.0) as u32;
        if !family.contains(hint) {
            score += 25;
        }
        let interface = family.contains("sans") || family.ends_with(" ui");
        if !interface && hint != "emoji" && hint != "symbol" {
            score += 50;
        }
        for (fragment, penalty) in [
            ("serif", 200),
            ("mono", 120),
            ("kufi", 80),
            ("naskh", 80),
            ("looped", 80),
            ("display", 80),
            ("typesetting", 80),
            ("traditional", 80),
            ("simplified", 80),
            ("fixed", 80),
            ("uighur", 80),
            ("condensed", 60),
            ("caption", 40),
        ] {
            if family.contains(fragment) {
                score += penalty;
            }
        }
        if let Some(region) = family.rsplit(' ').next().filter(|region| han_code_page(region).is_some())
            && region != han
        {
            score += 40;
        }
        score
    }

    fn han_code_page(region: &str) -> Option<u32> {
        match region {
            "jp" => Some(17),
            "sc" => Some(18),
            "kr" => Some(19),
            "tc" | "hk" => Some(20),
            _ => None,
        }
    }

    fn covers_han_region(code_pages: Option<u32>, han: &str) -> bool {
        han_code_page(han).zip(code_pages).is_some_and(|(bit, pages)| pages & (1 << bit) != 0)
    }

    fn han_region(locale: &str) -> &'static str {
        HAN_REGIONS
            .iter()
            .find(|(prefix, _)| locale.starts_with(prefix))
            .map_or("sc", |(_, region)| *region)
    }

    fn locale() -> String {
        let named = ["LC_ALL", "LC_CTYPE", "LANG"]
            .iter()
            .find_map(|key| std::env::var(key).ok().filter(|value| !value.is_empty()));
        #[cfg(windows)]
        let named = named.or_else(windows_language);
        named.unwrap_or_default().to_lowercase().replace('-', "_")
    }

    /// The display language, then the user locale.
    #[cfg(windows)]
    #[allow(unsafe_code)]
    fn windows_language() -> Option<String> {
        use windows_sys::Win32::Globalization::{
            GetUserDefaultLocaleName, GetUserPreferredUILanguages, MUI_LANGUAGE_NAME,
        };
        /// `LOCALE_NAME_MAX_LENGTH`.
        const NAME_LENGTH: usize = 85;
        let first = |names: &[u16]| {
            let end = names.iter().position(|unit| *unit == 0)?;
            (end > 0).then(|| String::from_utf16_lossy(&names[..end]))
        };
        let mut names = [0u16; NAME_LENGTH * 8];
        let mut count = 0u32;
        let mut length = names.len() as u32;
        // SAFETY: the call writes at most `length` units into `names`, its
        // own length.
        let preferred =
            unsafe { GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, names.as_mut_ptr(), &mut length) };
        if preferred != 0
            && count > 0
            && let Some(name) = first(&names)
        {
            return Some(name);
        }
        let mut name = [0u16; NAME_LENGTH];
        // SAFETY: the call writes at most `name.len()` units into `name`.
        let read = unsafe { GetUserDefaultLocaleName(name.as_mut_ptr(), name.len() as i32) };
        (read > 0).then(|| first(&name)).flatten()
    }

    fn family_name(bytes: &[u8], index: u32) -> Option<String> {
        skrifa::FontRef::from_index(bytes, index)
            .ok()?
            .localized_strings(skrifa::string::StringId::FAMILY_NAME)
            .english_or_first()
            .map(|name| name.to_string())
    }

    fn baseline_center(bytes: &[u8], index: u32) -> Option<f32> {
        let font = skrifa::FontRef::from_index(bytes, index).ok()?;
        let metrics = font.metrics(skrifa::instance::Size::unscaled(), skrifa::instance::LocationRef::default());
        let units = f32::from(metrics.units_per_em);
        let height = metrics.ascent - metrics.descent + metrics.leading;
        if units <= 0.0 || height <= 0.0 {
            return None;
        }
        Some((metrics.ascent - 0.5 * height) / units)
    }

    /// Enlarges an Arabic face whose heh (ه) sits below Inter's x-height:
    /// never smaller, at most 25% larger, and not for a difference too
    /// small to see.
    fn arabic_scale(bytes: &[u8], index: u32) -> f32 {
        let height = || -> Option<f32> {
            let font = skrifa::FontRef::from_index(bytes, index).ok()?;
            let glyph = font.charmap().map('\u{0647}')?;
            let size = skrifa::instance::Size::unscaled();
            let bounds = font.glyph_metrics(size, skrifa::instance::LocationRef::default()).bounds(glyph)?;
            let units = f32::from(font.metrics(size, skrifa::instance::LocationRef::default()).units_per_em);
            Some((bounds.y_max - bounds.y_min.max(0.0)) / units)
        };
        match height() {
            Some(arabic) if arabic > 0.0 => {
                let scale = (INTER_X_HEIGHT / arabic).clamp(1.0, 1.25);
                if scale < 1.04 { 1.0 } else { (scale * 100.0).round() / 100.0 }
            }
            _ => 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_faces_as_fastframe() {
        let ours = faces();
        let theirs = fastframe_fonts::system::fallbacks();
        assert_eq!(ours.len(), theirs.len());
        for (a, b) in ours.iter().zip(theirs) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.index, b.index);
            assert_eq!(a.scale, b.scale, "{}", a.name);
            assert_eq!(a.y_offset_factor, b.y_offset_factor, "{}", a.name);
            assert!(a.bytes == b.bytes.as_slice(), "{}", a.name);
        }
    }
}
