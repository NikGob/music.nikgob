//! Importer settings.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where things live and how the pipeline behaves. Nothing here is read from the
/// environment; the app fills it in from its own settings.
#[derive(Clone, Debug)]
pub struct ImportConfig {
    /// `Music/`: the managed library (PLAN section 5).
    pub library_dir: PathBuf,
    /// `Inbox/`: files and `.torrent` dropped by the user.
    pub inbox_dir: PathBuf,
    /// `Downloads/`: torrent payloads (not part of the library).
    pub downloads_dir: PathBuf,
    /// `Trash/`: deleted and replaced files, kept for 30 days.
    pub trash_dir: PathBuf,
    /// Opus VBR target for lossless inputs. Default 160.
    pub opus_bitrate_kbps: u32,
    /// Run the spectrum check on lossless inputs.
    pub check_fake_lossless: bool,
    /// Longest edge of `cover.jpg` and of the embedded cover. Default 1200.
    pub cover_max_edge: u32,
    /// Convert lossless files that look transcoded from a lossy source anyway.
    /// Off by default: the PLAN forbids lossy-to-lossy conversion, so such a
    /// file is kept as a FLAC/WAV copy and scored as lossy.
    pub convert_fake_lossless: bool,
    /// Look up synchronized lyrics on LRCLIB after the import (never blocks it).
    pub fetch_lyrics: bool,
    /// Fetch a cover online when the file has none.
    pub fetch_cover_online: bool,
    /// Files processed at the same time. Default 2.
    pub max_parallel: usize,
    /// Palette seed for colourless covers.
    pub fallback_seed_argb: u32,
    /// Give up on one online call after this long (offline never blocks import).
    pub online_timeout: Duration,
}

impl ImportConfig {
    pub fn new(
        library_dir: impl Into<PathBuf>,
        inbox_dir: impl Into<PathBuf>,
        downloads_dir: impl Into<PathBuf>,
        trash_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            library_dir: library_dir.into(),
            inbox_dir: inbox_dir.into(),
            downloads_dir: downloads_dir.into(),
            trash_dir: trash_dir.into(),
            opus_bitrate_kbps: 160,
            check_fake_lossless: true,
            cover_max_edge: 1200,
            convert_fake_lossless: false,
            fetch_lyrics: true,
            fetch_cover_online: true,
            max_parallel: 2,
            fallback_seed_argb: 0xFF67_50A4,
            online_timeout: Duration::from_secs(15),
        }
    }

    /// `root/Music`, `root/Inbox`, `root/Downloads`, `root/Trash`.
    pub fn with_root(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref();
        Self::new(
            root.join("Music"),
            root.join("Inbox"),
            root.join("Downloads"),
            root.join("Trash"),
        )
    }

    /// `<library>/.musiclib`: private bookkeeping (masters, userdata).
    pub fn musiclib_dir(&self) -> PathBuf {
        self.library_dir.join(".musiclib")
    }

    /// Where lossless masters wait for their backup (PLAN section 6, step 9).
    pub fn masters_dir(&self) -> PathBuf {
        self.musiclib_dir().join("masters")
    }
}
