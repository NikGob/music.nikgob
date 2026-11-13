//! Windows-safe file names and long-path handling.
//!
//! Rules for one path component:
//! * `/ \ |` become `-`, `:` becomes `-` (`": "` becomes `" - "`), `"` becomes `'`,
//!   `< > ? *` become `_`; control characters are dropped;
//! * runs of whitespace collapse, leading/trailing spaces, leading dots (a name
//!   starting with `.` is hidden for the library scan) and trailing dots are cut;
//! * reserved device names (`CON`, `NUL`, `COM1`, ... with or without an
//!   extension) get a `_` suffix; an empty result becomes `_`;
//! * Cyrillic and every other letter is kept as is (no transliteration).

use std::path::{Path, PathBuf};

/// `MAX_PATH` on Windows, in UTF-16 code units including the terminating NUL.
pub const MAX_PATH: usize = 260;

const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Is `name` (a file name, with or without extension) a reserved Windows device name?
pub fn is_reserved_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    RESERVED.iter().any(|r| stem.eq_ignore_ascii_case(r))
}

/// Makes `s` usable as one directory or file name on Windows (and everywhere else).
pub fn sanitize_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '/' | '\\' | '|' => out.push('-'),
            ':' => {
                // "Title: Sub" -> "Title - Sub"
                if chars.peek() == Some(&' ') {
                    if !out.ends_with(' ') {
                        out.push(' ');
                    }
                    out.push('-');
                } else {
                    out.push('-');
                }
            }
            '"' => out.push('\''),
            '<' | '>' | '?' | '*' => out.push('_'),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    // Collapse whitespace, trim spaces and dots. Leading dots go too: the scans
    // (library rebuild, Inbox) skip every name that starts with `.`, so
    // `...And Justice for All` would silently vanish from a rebuild.
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_matches(['.', ' ']);
    let mut name = trimmed.to_owned();
    if name.is_empty() {
        name.push('_');
    }
    if is_reserved_name(&name) {
        name.push('_');
    }
    name
}

/// At most `max` characters, cut on a character boundary, trimmed again so the
/// result never ends in a dot or space.
pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let cut: String = s.chars().take(max).collect();
    let cut = cut.trim_end_matches(['.', ' ']).to_owned();
    if cut.is_empty() { "_".to_owned() } else { cut }
}

/// Length of a path as Windows counts it: UTF-16 code units.
pub fn win_len(path: &Path) -> usize {
    path.to_string_lossy().encode_utf16().count()
}

/// Shortens `stem` so that `dir/<stem>.<ext>` fits in `limit` (Windows counts a
/// terminating NUL, hence `limit - 1` usable units). `keep_prefix` characters
/// at the start (the track number) are never cut. Falls back to a minimum
/// length of 8 characters when the directory itself is already too long; the
/// caller then relies on `\\?\` paths.
pub fn fit_stem(dir: &Path, stem: &str, ext: &str, limit: usize, keep_prefix: usize) -> String {
    let fixed = win_len(dir) + 1 /* separator */ + if ext.is_empty() { 0 } else { ext.encode_utf16().count() + 1 };
    let budget = limit.saturating_sub(1).saturating_sub(fixed);
    let units = |s: &str| s.encode_utf16().count();
    if units(stem) <= budget {
        return stem.to_owned();
    }
    let min_len = (keep_prefix + 8).max(8);
    let mut out = String::new();
    for c in stem.chars() {
        let next = units(&out) + c.len_utf16();
        if next > budget.max(min_len) {
            break;
        }
        out.push(c);
    }
    let out = out.trim_end_matches(['.', ' ']).to_owned();
    if out.is_empty() { "_".to_owned() } else { out }
}

/// Extended-length form of `path` (`\\?\C:\...`) on Windows when the path is
/// long enough to need it; unchanged elsewhere and for relative paths.
pub fn long_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let s = path.to_string_lossy();
        if s.starts_with(r"\\?\") || !path.is_absolute() || win_len(path) < MAX_PATH - 12 {
            return path.to_path_buf();
        }
        let norm = s.replace('/', r"\");
        if let Some(unc) = norm.strip_prefix(r"\\") {
            return PathBuf::from(format!(r"\\?\UNC\{unc}"));
        }
        PathBuf::from(format!(r"\\?\{norm}"))
    }
    #[cfg(not(windows))]
    {
        path.to_path_buf()
    }
}

/// `path` with the counter `n` added: `name (n).ext`.
pub fn numbered_path(path: &Path, n: u32) -> PathBuf {
    let dir = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = match path.extension() {
        Some(ext) => format!("{stem} ({n}).{}", ext.to_string_lossy()),
        None => format!("{stem} ({n})"),
    };
    dir.join(name)
}

/// `path`, or `name (2).ext`, `name (3).ext`, ... for the first variant that
/// does not exist yet.
pub fn unique_path(path: &Path) -> PathBuf {
    if !long_path(path).exists() {
        return path.to_path_buf();
    }
    for n in 2.. {
        let candidate = numbered_path(path, n);
        if !long_path(&candidate).exists() {
            return candidate;
        }
    }
    unreachable!("an unbounded range always yields a free name")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forbidden_characters_are_replaced() {
        assert_eq!(sanitize_component("AC/DC"), "AC-DC");
        assert_eq!(sanitize_component("a\\b|c"), "a-b-c");
        assert_eq!(sanitize_component("Title: Sub"), "Title - Sub");
        assert_eq!(sanitize_component("Re:Zero"), "Re-Zero");
        assert_eq!(sanitize_component("Say \"Hi\""), "Say 'Hi'");
        assert_eq!(sanitize_component("What? <Yes> *"), "What_ _Yes_ _");
        assert_eq!(sanitize_component("tab\there\u{7}"), "tabhere");
        for forbidden in ['<', '>', ':', '"', '/', '\\', '|', '?', '*'] {
            let s = sanitize_component(&format!("a{forbidden}b"));
            assert!(!s.contains(forbidden), "{forbidden} left in {s:?}");
        }
    }

    #[test]
    fn trailing_dots_and_spaces_are_trimmed() {
        assert_eq!(sanitize_component("Album..."), "Album");
        assert_eq!(sanitize_component("  Name  . "), "Name");
        assert_eq!(sanitize_component("a   b"), "a b");
        assert_eq!(sanitize_component("..."), "_");
        assert_eq!(sanitize_component(""), "_");
        assert_eq!(sanitize_component(" . "), "_");
        // A leading dot would make the folder "hidden" for the library scan.
        assert_eq!(sanitize_component("...And Justice for All"), "And Justice for All");
        assert_eq!(sanitize_component(".38 Special"), "38 Special");
    }

    #[test]
    fn reserved_names_are_avoided() {
        assert_eq!(sanitize_component("CON"), "CON_");
        assert_eq!(sanitize_component("nul"), "nul_");
        assert_eq!(sanitize_component("Com1"), "Com1_");
        assert_eq!(sanitize_component("LPT9"), "LPT9_");
        assert_eq!(sanitize_component("CON.txt"), "CON.txt_");
        assert_eq!(sanitize_component("Console"), "Console");
        assert_eq!(sanitize_component("COM10"), "COM10");
        assert!(is_reserved_name("aux.opus"));
        assert!(!is_reserved_name("auxiliary"));
    }

    #[test]
    fn cyrillic_is_kept() {
        assert_eq!(sanitize_component("Кино — Группа крови"), "Кино — Группа крови");
        assert_eq!(sanitize_component("Земфира/Zемфира?"), "Земфира-Zемфира_");
        assert_eq!(truncate_chars("Привет мир", 6), "Привет");
    }

    #[test]
    fn truncation_never_leaves_trailing_dot_or_space() {
        assert_eq!(truncate_chars("ab cd", 3), "ab");
        assert_eq!(truncate_chars("ab.cd", 3), "ab");
        assert_eq!(truncate_chars("short", 10), "short");
    }

    #[test]
    fn long_names_are_shortened_to_fit() {
        let dir = Path::new("C:\\Music\\Artist\\2019 - Album");
        let stem = format!("01 - {}", "я".repeat(300));
        let fitted = fit_stem(dir, &stem, "opus", MAX_PATH, 5);
        let full = dir.join(format!("{fitted}.opus"));
        assert!(win_len(&full) < MAX_PATH, "{}", win_len(&full));
        assert!(fitted.starts_with("01 - "), "track number kept: {fitted}");
        assert!(fitted.chars().count() > 20, "not shortened too much");
        // Short names are untouched.
        assert_eq!(fit_stem(dir, "01 - Short", "opus", MAX_PATH, 5), "01 - Short");
    }

    #[test]
    fn stem_has_a_floor_when_the_directory_is_too_long() {
        let dir = PathBuf::from(format!("C:\\{}", "d".repeat(300)));
        let fitted = fit_stem(&dir, &"x".repeat(100), "opus", MAX_PATH, 0);
        assert_eq!(fitted.chars().count(), 8);
    }

    #[cfg(windows)]
    #[test]
    fn long_paths_get_the_extended_prefix() {
        let long = PathBuf::from(format!("C:\\{}\\file.opus", "d".repeat(260)));
        assert!(long_path(&long).to_string_lossy().starts_with(r"\\?\C:\"));
        let short = PathBuf::from("C:\\Music\\a.opus");
        assert_eq!(long_path(&short), short);
        assert_eq!(long_path(Path::new("rel/a.opus")), PathBuf::from("rel/a.opus"));
    }

    #[test]
    fn collisions_get_a_counter() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("01 - Song.opus");
        assert_eq!(unique_path(&p), p);
        std::fs::write(&p, b"x").unwrap();
        assert_eq!(unique_path(&p), dir.path().join("01 - Song (2).opus"));
        std::fs::write(dir.path().join("01 - Song (2).opus"), b"x").unwrap();
        assert_eq!(unique_path(&p), dir.path().join("01 - Song (3).opus"));
    }
}
