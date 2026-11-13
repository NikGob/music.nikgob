//! Importer settings.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where things live and how the pipeline behaves. Nothing here is read from the
/// environment; the app fills it in from its own settings.
#[derive(Clone, Debug)]
pub struct ImportConfig {
    /// `Music/`: the managed library.
    pub library_dir: PathBuf,
    /// `Inbox/`: files and `.torrent` dropped by the user.
    pub inbox_dir: PathBuf,
    /// `Downloads/`: torrent payloads (not part of the library).
    pub downloads_dir: PathBuf,
    /// `Trash/`: deleted and replaced files, kept for 30 days.
    pub trash_dir: PathBuf,
    /// Optional custom directory (e.g. on another drive) for lossless FLAC/WAV
    /// masters and their `.json` sidecars. When empty, `<library_dir>/.musiclib/masters`
    /// is used.
    pub masters_dir: PathBuf,
    /// Opus VBR target for lossless inputs. Default 160.
    pub opus_bitrate_kbps: u32,
    /// Run the spectrum check on lossless inputs.
    pub check_fake_lossless: bool,
    /// Longest edge of `cover.jpg` and of the embedded cover. Default 1200.
    pub cover_max_edge: u32,
    /// A lossless file plainly transcoded from a lossy source
    /// ([`nikgob_core::quality::clear_fake`]) becomes an MP3 that keeps all
    /// its content has (320 CBR, V0 for a lower origin; `ffmpeg` needed),
    /// instead of a FLAC/WAV copy scored as lossy. Off by default: the
    /// player asks first; either way it is never registered as a lossless
    /// master.
    pub convert_fake_lossless: bool,
    /// Enable background online lyrics enrichment in the runtime.
    pub fetch_lyrics: bool,
    /// Fetch a cover online when the file has none.
    pub fetch_cover_online: bool,
    /// Files processed at the same time. Default 2.
    pub max_parallel: usize,
    /// Palette seed for colourless covers.
    pub fallback_seed_argb: u32,
    /// Give up on one online call after this long (offline never blocks import).
    pub online_timeout: Duration,
    /// Where an import cuts single-file rips into tracks for the while it
    /// encodes them. Empty: `nikgob-cue` in the system's temp folder.
    pub work_dir: PathBuf,
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
            masters_dir: PathBuf::new(),
            opus_bitrate_kbps: 160,
            check_fake_lossless: true,
            cover_max_edge: 1200,
            convert_fake_lossless: false,
            fetch_lyrics: true,
            fetch_cover_online: true,
            max_parallel: 2,
            fallback_seed_argb: 0xFF67_50A4,
            online_timeout: Duration::from_secs(15),
            work_dir: PathBuf::new(),
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

    /// Where lossless masters wait for their backup,
    /// or the custom archive folder/drive chosen by the user.
    pub fn masters_dir(&self) -> PathBuf {
        if self.masters_dir.as_os_str().is_empty() {
            self.musiclib_dir().join("masters")
        } else {
            self.masters_dir.clone()
        }
    }

    /// Where single-file rips are cut ([`Self::work_dir`]).
    pub fn work_dir(&self) -> PathBuf {
        if self.work_dir.as_os_str().is_empty() {
            std::env::temp_dir().join("nikgob-cue")
        } else {
            self.work_dir.clone()
        }
    }

    pub fn with_masters_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.masters_dir = dir.into();
        self
    }
}

/// Normalizes a user-chosen path for the lossless masters folder:
/// - Empty or equal to `library_dir` -> `<library_dir>/.musiclib/masters`.
/// - Drive root (`D:\`, `/`) with no folder name -> `<chosen>/FLAC`.
/// - Any specific folder -> `<chosen>`.
pub fn normalize_masters_dir(chosen: &Path, library_dir: &Path) -> PathBuf {
    if chosen.as_os_str().is_empty() || chosen == library_dir {
        library_dir.join(".musiclib").join("masters")
    } else if chosen.file_name().is_none() {
        chosen.join("FLAC")
    } else {
        chosen.to_path_buf()
    }
}
