//! A better copy the user let replace a file, taken back («Отменить» of
//! «Найти качество лучше» of an album): the old file comes back from the
//! library's Trash to its place and plays again under the same track (its
//! likes, plays, playlists and lyrics never left), the new file goes to the
//! Trash, and the track's tags and master are what they were.

use std::path::{Path, PathBuf};

use nikgob_core::model::{Master, SourceId, Track, TrackId};
use nikgob_library::Library;

use crate::error::{ImportError, Result};
use crate::fsutil;
use crate::sanitize::long_path;
use crate::trash::trash_file;

/// One file a better copy replaced, as it was before.
#[derive(Clone, Debug)]
pub struct Replacement {
    pub track: TrackId,
    /// The track as it was (the new file's import may have retagged it).
    pub before: Track,
    /// The replaced file's source, in the Trash since.
    pub old: SourceId,
    /// The new file's source.
    pub new: SourceId,
    /// The track's master before (`None`: it had none).
    pub master_before: Option<Master>,
}

/// Takes `r` back. The new file goes to the Trash first: whatever fails
/// after it is undone step by step, so the track never plays two live
/// files, never loses both, and a failed undo can be tried again. Done
/// already (the old file plays): nothing to do.
pub fn undo_replacement(lib: &Library, trash_dir: &Path, r: &Replacement) -> Result<()> {
    let old = lib
        .get_source(r.old)?
        .ok_or_else(|| ImportError::Other(format!("the replaced file of {} is not known any more", r.track)))?;
    if old.trashed_at.is_none() {
        return Ok(());
    }
    let in_trash = lib
        .trash_path_of(r.old)?
        .map(PathBuf::from)
        .ok_or_else(|| ImportError::Other(format!("{} has no place in the Trash", old.locator)))?;
    if !long_path(&in_trash).exists() {
        return Err(ImportError::Other(format!("{} is not in the Trash any more", in_trash.display())));
    }
    let original = PathBuf::from(&old.locator);
    // The new file, while it plays.
    let new = lib.get_source(r.new)?.filter(|s| s.trashed_at.is_none());
    let new_path = new.as_ref().map(|s| PathBuf::from(&s.locator));
    if long_path(&original).exists() && new_path.as_deref() != Some(original.as_path()) {
        return Err(ImportError::Other(format!("{} is taken", original.display())));
    }
    let new_trash = match &new_path {
        Some(p) if long_path(p).exists() => Some(trash_file(trash_dir, p)?),
        _ => None,
    };
    let new_files_back = |e: ImportError| {
        if let (Some(t), Some(p)) = (&new_trash, &new_path)
            && let Err(m) = fsutil::move_file(t, p)
        {
            log::warn!("cannot put {} back from the Trash: {m}", p.display());
        }
        e
    };
    if let Some(n) = &new {
        let at = new_trash.as_ref().map(|t| t.to_string_lossy().into_owned());
        lib.trash_source(n.id, at.as_deref()).map_err(|e| new_files_back(e.into()))?;
    }
    let new_back = |e: ImportError| {
        if new.is_some()
            && let Err(m) = lib.restore_source(r.new)
        {
            log::warn!("the row of the new file of {} stays in the Trash: {m}", r.track);
        }
        new_files_back(e)
    };
    if let Some(dir) = original.parent() {
        fsutil::create_dir_all(dir).map_err(|e| new_back(e))?;
    }
    fsutil::move_file(&in_trash, &original).map_err(|e| new_back(e))?;
    let put_back = |e: ImportError| {
        if let Err(m) = fsutil::move_file(&original, &in_trash) {
            log::warn!("cannot put {} back into the Trash: {m}", original.display());
        }
        new_back(e)
    };
    lib.restore_source(r.old).map_err(|e| put_back(e.into()))?;
    // The track's tags and place as they were.
    if let Err(e) = lib.upsert_track(&r.before) {
        log::warn!("{} keeps the tags of the new file: {e}", r.before.title);
    }
    // Its master as it was: one the new file brought goes from the library
    // (its file stays where it is, it may be another track's too).
    match &r.master_before {
        Some(m) => {
            if let Err(e) = lib.put_master(m) {
                log::warn!("the master of {} is not put back: {e}", r.track);
            }
        }
        None => match lib.delete_master(r.track) {
            Ok(true) => log::info!("the master the better copy of {} brought is no longer the track's", r.track),
            Ok(false) => {}
            Err(e) => log::warn!("the master of {} stays: {e}", r.track),
        },
    }
    // Its lyrics file: the old one from the Trash, else the new one's.
    let back = original.with_extension("lrc");
    if !long_path(&back).exists() {
        let old_lrc = in_trash.with_extension("lrc");
        let new_lrc = new_path.as_ref().map(|p| p.with_extension("lrc")).filter(|l| *l != back);
        if long_path(&old_lrc).exists() {
            let _ = fsutil::move_file(&old_lrc, &back);
        } else if let Some(l) = new_lrc.filter(|l| long_path(l).exists()) {
            let _ = fsutil::move_file(&l, &back);
        }
    }
    log::info!(
        "better copy taken back: {} is back from the Trash, {} went there",
        original.display(),
        new_trash.as_ref().map_or_else(|| "nothing".to_owned(), |t| t.display().to_string())
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use nikgob_core::model::{ArtistCredit, Codec, Source, SourceKind};

    use super::*;

    /// An MP3 replaced by a better copy the way the import does it: the
    /// MP3 in the Trash, the new file live in its place.
    fn replaced(same_place: bool) -> (tempfile::TempDir, Library, Replacement, PathBuf, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::open_in_memory().unwrap();
        let before = lib.upsert_track(&Track::new("Молли", ArtistCredit::single("Пошлая Молли", None))).unwrap();
        let music = dir.path().join("Music").join("Пошлая Молли");
        std::fs::create_dir_all(&music).unwrap();
        let trash = dir.path().join("Trash");
        let mp3_path = music.join("07 - Молли.mp3");
        std::fs::write(&mp3_path, b"mp3").unwrap();
        std::fs::write(mp3_path.with_extension("lrc"), b"[00:01]a").unwrap();
        let mut mp3 = Source::new(before.id, SourceKind::LocalFile, mp3_path.to_string_lossy(), Codec::Mp3);
        mp3.quality_score = 700;
        let mp3 = lib.add_source(&mp3).unwrap();
        let in_trash = trash_file(&trash, &mp3_path).unwrap();
        trash_file(&trash, &mp3_path.with_extension("lrc")).unwrap();
        lib.trash_source(mp3.id, Some(&in_trash.to_string_lossy())).unwrap();
        let new_path = if same_place { mp3_path.clone() } else { music.join("07 - Молли.opus") };
        std::fs::write(&new_path, b"opus").unwrap();
        let mut opus = Source::new(before.id, SourceKind::LocalFile, new_path.to_string_lossy(), Codec::Opus);
        opus.quality_score = 900;
        let opus = lib.add_source(&opus).unwrap();
        let mut retagged = before.clone();
        retagged.title = "Молли (Remastered)".into();
        lib.upsert_track(&retagged).unwrap();
        let r = Replacement { track: before.id, before, old: mp3.id, new: opus.id, master_before: None };
        (dir, lib, r, trash, mp3_path, new_path)
    }

    #[test]
    fn an_undo_brings_the_old_file_back_and_trashes_the_new_one() {
        for same_place in [false, true] {
            let (_dir, lib, r, trash, mp3_path, new_path) = replaced(same_place);
            assert_eq!(lib.active_source(r.track).unwrap().unwrap().id, r.new);
            undo_replacement(&lib, &trash, &r).unwrap();
            assert_eq!(std::fs::read(&mp3_path).unwrap(), b"mp3", "{same_place}");
            assert!(mp3_path.with_extension("lrc").exists(), "its lyrics too");
            if !same_place {
                assert!(!new_path.exists());
            }
            assert_eq!(lib.active_source(r.track).unwrap().unwrap().id, r.old, "the old file plays");
            assert!(lib.get_source(r.new).unwrap().unwrap().trashed_at.is_some());
            let in_trash = lib.trash_path_of(r.new).unwrap().unwrap();
            assert_eq!(std::fs::read(&in_trash).unwrap(), b"opus", "the new file waits in the Trash");
            assert_eq!(lib.get_track(r.track).unwrap().unwrap().title, "Молли", "the tags as they were");
            // Again: nothing more to do.
            undo_replacement(&lib, &trash, &r).unwrap();
            assert_eq!(lib.active_source(r.track).unwrap().unwrap().id, r.old);
        }
    }

    /// The old file cannot come back (its folder cannot be made): the new
    /// file stays live and in its place, and the undo can be tried again.
    #[test]
    fn a_failed_undo_leaves_the_new_file_playing() {
        let (dir, lib, r, trash, _mp3_path, new_path) = replaced(false);
        let blocker = dir.path().join("a file, not a folder");
        std::fs::write(&blocker, b"x").unwrap();
        let mut moved = lib.get_source(r.old).unwrap().unwrap();
        moved.locator = blocker.join("07 - Молли.mp3").to_string_lossy().into_owned();
        moved.trashed_at = None;
        // The old row points where nothing can be made.
        lib.forget_source(r.old).unwrap();
        let old = lib.add_source(&moved).unwrap();
        let in_trash = dir.path().join("Trash").join("kept.mp3");
        std::fs::write(&in_trash, b"mp3").unwrap();
        lib.trash_source(old.id, Some(&in_trash.to_string_lossy())).unwrap();
        let r = Replacement { old: old.id, ..r };
        assert!(undo_replacement(&lib, &trash, &r).is_err());
        assert_eq!(std::fs::read(&new_path).unwrap(), b"opus", "the new file is back in its place");
        assert_eq!(lib.active_source(r.track).unwrap().unwrap().id, r.new, "and plays");
        assert!(in_trash.exists(), "the old one waits in the Trash");
        assert!(lib.get_source(r.old).unwrap().unwrap().trashed_at.is_some());
    }
}
