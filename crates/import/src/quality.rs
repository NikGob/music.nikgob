//! The spectrum check of a file in the library (the import runs it on its
//! own way in), and what happens to a "lossless" file found transcoded:
//! with `convert_fake_lossless` on it becomes an MP3 that keeps everything
//! its content really has (MP3 320 CBR, V0 for a lower origin), with its
//! tags, cover and lyrics, under the same track; the old file goes to the
//! Trash like any replaced file.
//!
//! MP3 is written by `ffmpeg` (libmp3lame) found on `PATH`; without it a
//! fake stays as it is, honestly labelled. A conversion is undone by
//! [`restore_conversion`]: the original comes back from the Trash.

use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use nikgob_core::model::{Codec, Source, SourceKind, TrackId, UnixMs};
use nikgob_core::quality::{Mp3Target, QualityCheck, SoundOrigin, Verdict};
use nikgob_library::{Conversion, Library};

use crate::decode::{PcmSpec, decode_file};
use crate::error::{ImportError, Result};
use crate::fsutil;
use crate::sanitize::{long_path, unique_path};
use crate::spectrum::{SpectrumCollector, SpectrumReport};
use crate::tags::{self, R128_REFERENCE_LUFS, REPLAYGAIN_REFERENCE_LUFS};
use crate::trash::trash_file;

/// `(size, mtime in Unix ms)` of a file, what a stored check is matched
/// against: another file at the place is checked again.
pub fn file_stamp(path: &Path) -> Option<(u64, UnixMs)> {
    let m = std::fs::metadata(long_path(path)).ok()?;
    let mtime = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64);
    Some((m.len(), mtime))
}

/// The stored form of a spectrum report. `made_from`: the report is of
/// the lossless file the source's file was made from; `stamp` is the
/// source's own file.
pub fn check_of(
    report: &SpectrumReport,
    spectrum_db: Option<Vec<f32>>,
    sample_rate: u32,
    made_from: Option<Codec>,
    stamp: Option<(u64, UnixMs)>,
) -> QualityCheck {
    let (file_size, file_mtime) = stamp.unwrap_or_default();
    QualityCheck {
        verdict: report.verdict,
        cutoff_hz: report.cutoff_hz,
        steepness_db: report.steepness_db,
        origin: SoundOrigin::estimate(report.verdict, report.cutoff_hz),
        sample_rate,
        spectrum_db: spectrum_db.unwrap_or_default(),
        made_from,
        file_size,
        file_mtime,
        checked_at: nikgob_library::now_ms(),
    }
}

/// Why a file has no check.
#[derive(Debug)]
pub enum CheckError {
    /// `stop` was set.
    Stopped,
    /// The file could not be opened (missing, locked, a drive not there):
    /// maybe later.
    Unavailable(String),
    /// The file is read but cannot be decoded: not until it changes.
    Undecodable(String),
}

/// Decodes the whole file into the spectrum check. `stop` aborts the decode.
pub fn check_file(path: &Path, stop: &AtomicBool) -> std::result::Result<QualityCheck, CheckError> {
    check_file_paced(path, stop, &mut || {})
}

/// [`check_file`] with `pace` called between the decoded chunks (it may
/// block: a background pass rests there).
pub fn check_file_paced(path: &Path, stop: &AtomicBool, pace: &mut dyn FnMut()) -> std::result::Result<QualityCheck, CheckError> {
    let stamp = file_stamp(path).ok_or_else(|| CheckError::Unavailable(format!("{}: not found", path.display())))?;
    let hint = tags::probe_info(path).map(|i| i.duration).unwrap_or_default();
    let mut collector: Option<SpectrumCollector> = None;
    let mut sink = |chunk: &[f32], spec: PcmSpec| -> ControlFlow<()> {
        pace();
        if stop.load(Ordering::Relaxed) {
            return ControlFlow::Break(());
        }
        collector
            .get_or_insert_with(|| SpectrumCollector::new(spec.sample_rate, spec.channels, hint.as_secs_f64()))
            .push(chunk);
        ControlFlow::Continue(())
    };
    let decoded = decode_file(path, &mut sink);
    if stop.load(Ordering::Relaxed) {
        return Err(CheckError::Stopped);
    }
    let spec = match decoded {
        Ok(spec) => spec,
        Err(e @ ImportError::Io { .. }) => return Err(CheckError::Unavailable(e.to_string())),
        Err(e) => return Err(CheckError::Undecodable(e.to_string())),
    };
    let collector = collector.ok_or_else(|| CheckError::Undecodable("no audio".into()))?;
    let (report, levels) = collector.finish_with_levels();
    Ok(check_of(&report, levels, spec.sample_rate, None, Some(stamp)))
}

/// `ffmpeg -c:a libmp3lame`.
#[derive(Clone, Debug)]
pub struct Mp3Encoder {
    exe: PathBuf,
}

impl Mp3Encoder {
    /// Looks for `ffmpeg` on `PATH`.
    pub fn find() -> Option<Self> {
        crate::encode::FfmpegEncoder::find_exe().map(|exe| Self { exe })
    }

    pub fn with_exe(exe: impl Into<PathBuf>) -> Self {
        Self { exe: exe.into() }
    }

    /// Writes the audio of `src` as an untagged MP3 at `dst`.
    pub fn encode(&self, src: &Path, dst: &Path, target: Mp3Target) -> Result<()> {
        let quality: &[&str] = match target {
            Mp3Target::Cbr320 => &["-b:a", "320k"],
            Mp3Target::V0 => &["-q:a", "0"],
        };
        let mut cmd = Command::new(&self.exe);
        cmd.args(["-nostdin", "-v", "error", "-y", "-i"])
            .arg(long_path(src))
            .args(["-map", "0:a:0", "-vn", "-map_metadata", "-1", "-c:a", "libmp3lame"])
            .args(quality)
            .args(["-f", "mp3"])
            .arg(long_path(dst))
            .stdin(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            // No console window flashing up from a windowed app.
            cmd.creation_flags(0x0800_0000);
        }
        let output = cmd
            .output()
            .map_err(|e| ImportError::Encode(format!("cannot run ffmpeg: {e}")))?;
        if !output.status.success() {
            let _ = fsutil::remove_file(dst);
            return Err(ImportError::Encode(format!(
                "ffmpeg failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(())
    }
}

/// Loudness the tags of a file carry, for writing it into another format.
fn loudness_of(t: &tags::TagSet) -> Option<f64> {
    t.loudness_lufs
        .or_else(|| t.r128_track_gain.map(|q8| R128_REFERENCE_LUFS - f64::from(q8) / 256.0))
        .or_else(|| t.replaygain_track_gain_db.map(|db| REPLAYGAIN_REFERENCE_LUFS - f64::from(db)))
}

/// Whether a check calls for converting the source: a lossless file whose
/// content is lossy.
pub fn is_fake(codec: &Codec, check: &QualityCheck) -> bool {
    codec.is_lossless() && check.made_from.is_none() && check.verdict == Verdict::Transcoded
}

/// Converts the fake lossless file of `src` to MP3 next to it (same name,
/// `.mp3`), with its tags, cover and lyrics, as a new source of the same
/// track keeping its score; the old file and source go to the Trash, the
/// `.lrc` follows, and the check moves to the new source marked as made
/// from the old codec. Returns the new source.
pub fn convert_fake(
    lib: &Library,
    trash_dir: &Path,
    encoder: &Mp3Encoder,
    src: &Source,
    check: &QualityCheck,
) -> Result<Source> {
    if src.kind != SourceKind::LocalFile || !is_fake(&src.codec, check) {
        return Err(ImportError::Other(format!("{} is not a fake lossless file", src.locator)));
    }
    let path = PathBuf::from(&src.locator);
    let dir = path
        .parent()
        .ok_or_else(|| ImportError::Other(format!("{} has no folder", path.display())))?;
    let (mut set, _) = tags::read_tags(&path)?;
    set.track_id = Some(src.track_id);
    set.loudness_lufs = loudness_of(&set);
    let tmp = dir.join(format!(".{}.nikgob-tmp.mp3", TrackId::new()));
    let made = encoder
        .encode(&path, &tmp, Mp3Target::for_origin(check.origin))
        .and_then(|()| tags::write_tags(&tmp, &set));
    if let Err(e) = made {
        let _ = fsutil::remove_file(&tmp);
        return Err(e);
    }
    let dest = unique_path(&path.with_extension("mp3"));
    if let Err(e) = fsutil::move_file(&tmp, &dest) {
        let _ = fsutil::remove_file(&tmp);
        return Err(e);
    }
    let undo_dest = |e: ImportError| {
        let _ = fsutil::remove_file(&dest);
        e
    };
    let info = tags::probe_info(&dest).map_err(undo_dest)?;
    let hash = fsutil::hash_file(&dest).map_err(undo_dest)?;
    let size = fsutil::file_size(&dest).map_err(undo_dest)?;
    let trashed = trash_file(trash_dir, &path).map_err(undo_dest)?;
    let trashed_at = trashed.to_string_lossy().into_owned();

    let mut new = Source::new(src.track_id, SourceKind::LocalFile, dest.to_string_lossy().into_owned(), info.codec);
    new.bitrate = info.bitrate_kbps;
    new.sample_rate = info.sample_rate;
    new.bit_depth = None;
    new.quality_score = src.quality_score;
    new.file_hash = Some(hash);
    new.size_bytes = Some(size);
    new.origin = src.origin.clone();
    let stored = lib
        .add_source(&new)
        .and_then(|n| lib.trash_source(src.id, Some(&trashed_at)).map(|()| n));
    let new = match stored {
        Ok(n) => n,
        Err(e) => {
            // Put the old file back; the new one goes.
            if let Err(e) = fsutil::move_file(&trashed, &path) {
                log::warn!("cannot restore {} from Trash: {e}", path.display());
            }
            let _ = fsutil::remove_file(&dest);
            return Err(e.into());
        }
    };
    let lrc = path.with_extension("lrc");
    let new_lrc = dest.with_extension("lrc");
    if lrc != new_lrc && long_path(&lrc).exists() {
        let _ = fsutil::move_file(&lrc, &new_lrc);
    }
    let moved = QualityCheck {
        made_from: Some(src.codec.clone()),
        ..check.clone()
    };
    let moved = match file_stamp(&dest) {
        Some((file_size, file_mtime)) => QualityCheck { file_size, file_mtime, ..moved },
        None => moved,
    };
    if let Err(e) = lib.put_quality_check(new.id, &moved) {
        log::warn!("the check of {} is not stored: {e}", dest.display());
    }
    log::info!("fake lossless {} converted to {}", path.display(), dest.display());
    Ok(new)
}

/// Undoes a conversion: the original lossless file comes back from the
/// Trash to its place and plays again (the same track, so its likes, plays,
/// playlists and lyrics stay), the MP3 goes to the Trash, and the original
/// is never offered as an MP3 again. Its rank is what its check says now
/// (a cut no encoder makes is no verdict, see
/// [`nikgob_core::quality::plausible`]).
pub fn restore_conversion(lib: &Library, trash_dir: &Path, c: &Conversion) -> Result<()> {
    // Restored already (by hand, or an earlier run): nothing to do.
    match lib.get_source(c.original)? {
        Some(s) if s.trashed_at.is_none() => return Ok(()),
        None => return Ok(()),
        Some(_) => {}
    }
    let original = PathBuf::from(&c.original_locator);
    let in_trash = PathBuf::from(&c.trash_path);
    if !long_path(&in_trash).exists() {
        return Err(ImportError::Other(format!("{} is not in the Trash any more", in_trash.display())));
    }
    if long_path(&original).exists() {
        return Err(ImportError::Other(format!("{} is taken", original.display())));
    }
    if let Some(dir) = original.parent() {
        fsutil::create_dir_all(dir)?;
    }
    fsutil::move_file(&in_trash, &original)?;
    let put_back = |e: ImportError| {
        if let Err(m) = fsutil::move_file(&original, &in_trash) {
            log::warn!("cannot put {} back into the Trash: {m}", original.display());
        }
        e
    };
    let src = lib.restore_source(c.original).map_err(|e| put_back(e.into()))?;
    let check = lib.quality_check(c.original).ok().flatten();
    let rank = match &check {
        Some(q) if nikgob_core::quality::plausible(q.verdict, q.cutoff_hz) == Verdict::Transcoded => {
            nikgob_core::quality::fake_score(q.origin, q.cutoff_hz)
        }
        _ => nikgob_core::model::quality_score(&src.codec, src.bitrate, src.sample_rate, src.bit_depth, false),
    };
    if let Err(e) = lib.set_source_score(src.id, rank) {
        log::warn!("ranking {}: {e}", original.display());
    }
    if check.is_some() && let Err(e) = lib.set_quality_keep(src.id, true) {
        log::warn!("keeping {}: {e}", original.display());
    }
    // The MP3 goes where the original was.
    let mp3 = PathBuf::from(&c.mp3_locator);
    let mp3_trash = if long_path(&mp3).exists() {
        Some(trash_file(trash_dir, &mp3)?.to_string_lossy().into_owned())
    } else {
        None
    };
    lib.trash_source(c.mp3, mp3_trash.as_deref())?;
    // The lyrics file went along with the MP3 when its name differed.
    let lrc = mp3.with_extension("lrc");
    let back = original.with_extension("lrc");
    if lrc != back && long_path(&lrc).exists() && !long_path(&back).exists() {
        let _ = fsutil::move_file(&lrc, &back);
    }
    log::info!("{} is back from the Trash, {} went there", original.display(), mp3.display());
    Ok(())
}
