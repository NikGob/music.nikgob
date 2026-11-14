//! Folder layout of the managed library:
//!
//! ```text
//! Music/<AlbumArtist>/<YYYY - Album>/<NN - Title>.opus      (multi-disc: <D-NN - Title>)
//! Music/<Artist>/Singles/<YYYY - Title>/<NN - Title>.opus
//! Music/Various Artists/<YYYY - Compilation>/...
//! ```

use std::path::{Path, PathBuf};

use nikgob_core::model::ReleaseType;

use crate::sanitize::{MAX_PATH, fit_stem, sanitize_component, truncate_chars, unique_path};

/// Folder used for compilations.
pub const VARIOUS_ARTISTS: &str = "Various Artists";
const UNKNOWN_ARTIST: &str = "Unknown Artist";
const SINGLES_DIR: &str = "Singles";
const MAX_DIR_CHARS: usize = 120;
const MAX_STEM_CHARS: usize = 150;

/// What decides where a file goes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayoutInput {
    pub album_artist: String,
    /// Track artist (credit line); used when `album_artist` is empty.
    pub artist: String,
    pub album: Option<String>,
    pub title: String,
    pub year: Option<i32>,
    pub disc_no: u32,
    pub disc_total: Option<u32>,
    /// 0 = unknown.
    pub track_no: u32,
    pub release_type: ReleaseType,
    pub compilation: bool,
}

/// "Various Artists" and its spellings.
pub fn is_various_artists(name: &str) -> bool {
    let n = name.trim().to_lowercase();
    matches!(
        n.as_str(),
        "various artists" | "various" | "va" | "v.a." | "разные исполнители" | "сборник"
    )
}

/// Where a lyrics text kept for a song without a file goes: the artist's
/// folder (named as [`LayoutInput::album_dir`] names it), the song's title
/// as the name, `.ttml`.
pub fn kept_text_path(library_dir: &Path, artist: &str, title: &str) -> PathBuf {
    let input = LayoutInput { artist: artist.to_owned(), title: title.to_owned(), ..Default::default() };
    let title = if title.trim().is_empty() { "Untitled" } else { title.trim() };
    let stem = truncate_chars(&sanitize_component(title), MAX_STEM_CHARS);
    library_dir.join(input.artist_folder()).join(format!("{stem}.ttml"))
}

impl LayoutInput {
    pub(crate) fn is_compilation(&self) -> bool {
        self.compilation
            || self.release_type == ReleaseType::Compilation
            || is_various_artists(&self.album_artist)
    }

    fn is_single(&self) -> bool {
        self.release_type == ReleaseType::Single
            || (self.album.as_deref().is_none_or(|a| a.trim().is_empty()) && !self.is_compilation())
    }

    fn multi_disc(&self) -> bool {
        self.disc_total.is_some_and(|t| t > 1) || self.disc_no > 1
    }

    fn artist_folder(&self) -> String {
        if self.is_compilation() {
            return VARIOUS_ARTISTS.to_owned();
        }
        let name = [&self.album_artist, &self.artist]
            .into_iter()
            .map(|s| s.trim())
            .find(|s| !s.is_empty())
            .unwrap_or(UNKNOWN_ARTIST);
        truncate_chars(&sanitize_component(name), MAX_DIR_CHARS)
    }

    fn release_folder(&self) -> String {
        let title = if self.is_single() {
            self.album
                .as_deref()
                .map(str::trim)
                .filter(|a| !a.is_empty())
                .unwrap_or(self.title.trim())
        } else {
            self.album.as_deref().unwrap_or("").trim()
        };
        let title = if title.is_empty() { "Untitled" } else { title };
        let name = match self.year.filter(|y| (1..=9999).contains(y)) {
            Some(y) => format!("{y:04} - {title}"),
            None => title.to_owned(),
        };
        truncate_chars(&sanitize_component(&name), MAX_DIR_CHARS)
    }

    /// The release folder: `Music/<artist>/<YYYY - Album>` or
    /// `Music/<artist>/Singles/<YYYY - Title>`.
    pub fn album_dir(&self, library_dir: &Path) -> PathBuf {
        let mut dir = library_dir.join(self.artist_folder());
        if self.is_single() && !self.is_compilation() {
            dir.push(SINGLES_DIR);
        }
        dir.push(self.release_folder());
        dir
    }

    /// `01 - Title`, `2-01 - Title` for multi-disc releases, `Title` without a number.
    /// Returns the stem and the length of its numeric prefix (for shortening).
    pub fn file_stem(&self) -> (String, usize) {
        let track_no = if self.track_no == 0 && self.is_single() {
            1
        } else {
            self.track_no
        };
        let title = sanitize_component(if self.title.trim().is_empty() {
            "Untitled"
        } else {
            &self.title
        });
        let title = truncate_chars(&title, MAX_STEM_CHARS);
        if track_no == 0 {
            return (title, 0);
        }
        let prefix = if self.multi_disc() {
            format!("{}-{track_no:02} - ", self.disc_no.max(1))
        } else {
            format!("{track_no:02} - ")
        };
        let len = prefix.chars().count();
        (format!("{prefix}{title}"), len)
    }

    /// Full target path (no collision handling): directory plus the stem
    /// shortened to fit `MAX_PATH`.
    pub fn target_path(&self, library_dir: &Path, ext: &str) -> PathBuf {
        let dir = self.album_dir(library_dir);
        let (stem, prefix) = self.file_stem();
        let stem = fit_stem(&dir, &stem, ext, MAX_PATH, prefix);
        dir.join(format!("{stem}.{ext}"))
    }

    /// Like [`Self::target_path`], with `" (2)"`-style suffixes when the name is taken.
    pub fn unique_target_path(&self, library_dir: &Path, ext: &str) -> PathBuf {
        unique_path(&self.target_path(library_dir, ext))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> LayoutInput {
        LayoutInput {
            album_artist: "Кино".into(),
            artist: "Кино".into(),
            album: Some("Группа крови".into()),
            title: "Группа крови".into(),
            year: Some(1988),
            disc_no: 1,
            disc_total: Some(1),
            track_no: 3,
            release_type: ReleaseType::Album,
            compilation: false,
        }
    }

    fn rel(p: &Path, root: &Path) -> String {
        p.strip_prefix(root)
            .unwrap()
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/")
    }

    #[test]
    fn a_kept_text_goes_into_its_artist_s_folder() {
        let root = Path::new("C:\\Music");
        assert_eq!(rel(&kept_text_path(root, "LAZZY2WICE", "Summer 15/16"), root), "LAZZY2WICE/Summer 15-16.ttml");
        assert_eq!(rel(&kept_text_path(root, " ", ""), root), "Unknown Artist/Untitled.ttml");
    }

    #[test]
    fn plain_album_track() {
        let root = Path::new("C:\\Music");
        let p = input().target_path(root, "opus");
        assert_eq!(rel(&p, root), "Кино/1988 - Группа крови/03 - Группа крови.opus");
    }

    #[test]
    fn multi_disc_prefix() {
        let root = Path::new("C:\\Music");
        let mut i = input();
        i.disc_no = 2;
        i.disc_total = Some(2);
        i.track_no = 7;
        i.title = "Song".into();
        assert_eq!(
            rel(&i.target_path(root, "opus"), root),
            "Кино/1988 - Группа крови/2-07 - Song.opus"
        );
        // Disc 1 of a multi-disc release is numbered the same way.
        i.disc_no = 1;
        assert!(rel(&i.target_path(root, "opus"), root).ends_with("/1-07 - Song.opus"));
    }

    #[test]
    fn singles_go_under_singles() {
        let root = Path::new("C:\\Music");
        let i = LayoutInput {
            album_artist: "Artist".into(),
            artist: "Artist".into(),
            album: Some("Hit".into()),
            title: "Hit".into(),
            year: Some(2021),
            release_type: ReleaseType::Single,
            track_no: 0,
            ..Default::default()
        };
        assert_eq!(
            rel(&i.target_path(root, "opus"), root),
            "Artist/Singles/2021 - Hit/01 - Hit.opus"
        );
        // A loose track without album information is a single too.
        let loose = LayoutInput {
            artist: "Somebody".into(),
            title: "Loose: Track".into(),
            ..Default::default()
        };
        assert_eq!(
            rel(&loose.target_path(root, "opus"), root),
            "Somebody/Singles/Loose - Track/01 - Loose - Track.opus"
        );
    }

    #[test]
    fn compilations_go_under_various_artists() {
        let root = Path::new("C:\\Music");
        for (album_artist, compilation, ty) in [
            ("Various Artists", false, ReleaseType::Album),
            ("VA", false, ReleaseType::Album),
            ("Someone", true, ReleaseType::Album),
            ("Someone", false, ReleaseType::Compilation),
        ] {
            let i = LayoutInput {
                album_artist: album_artist.into(),
                artist: "Track Artist".into(),
                album: Some("Сборник 2020".into()),
                title: "T".into(),
                year: Some(2020),
                track_no: 12,
                release_type: ty,
                compilation,
                ..Default::default()
            };
            assert_eq!(
                rel(&i.target_path(root, "opus"), root),
                "Various Artists/2020 - Сборник 2020/12 - T.opus",
                "{album_artist}"
            );
        }
    }

    #[test]
    fn missing_pieces_have_fallbacks() {
        let root = Path::new("C:\\Music");
        let i = LayoutInput {
            title: "Song".into(),
            album: Some("Album".into()),
            track_no: 0,
            ..Default::default()
        };
        assert_eq!(
            rel(&i.target_path(root, "opus"), root),
            "Unknown Artist/Album/Song.opus"
        );
        // Artist falls back from album artist to track artist.
        let j = LayoutInput {
            artist: "Solo".into(),
            ..i
        };
        assert!(rel(&j.target_path(root, "opus"), root).starts_with("Solo/Album/"));
    }

    #[test]
    fn names_are_sanitized_and_shortened() {
        let root = Path::new("C:\\Music");
        let i = LayoutInput {
            album_artist: "AC/DC".into(),
            album: Some("Who Made Who?".into()),
            title: format!("CON {}", "x".repeat(400)),
            year: Some(1986),
            track_no: 1,
            ..Default::default()
        };
        let p = i.target_path(root, "opus");
        let s = rel(&p, root);
        assert!(s.starts_with("AC-DC/1986 - Who Made Who_/01 - CON xxx"), "{s}");
        assert!(crate::sanitize::win_len(&p) < MAX_PATH);
    }

    #[test]
    fn unique_target_path_adds_counter() {
        let dir = tempfile::tempdir().unwrap();
        let i = input();
        let first = i.unique_target_path(dir.path(), "opus");
        std::fs::create_dir_all(first.parent().unwrap()).unwrap();
        std::fs::write(&first, b"x").unwrap();
        let second = i.unique_target_path(dir.path(), "opus");
        assert_ne!(first, second);
        assert!(
            second
                .file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with("(2).opus")
        );
    }

    #[test]
    fn unknown_year_is_omitted() {
        let root = Path::new("C:\\Music");
        let mut i = input();
        i.year = None;
        assert_eq!(
            rel(&i.target_path(root, "opus"), root),
            "Кино/Группа крови/03 - Группа крови.opus"
        );
    }
}
