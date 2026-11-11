//! A better copy the user let replace a file, taken back («Отменить» of
//! «Найти качество лучше» of an album): the old file comes back from the
//! library's Trash to its place and plays again under the same track (its
//! likes, plays, playlists and lyrics never left), the new file goes to the
//! Trash, and the track's tags and master are what they were.

use std::path::{Path, PathBuf};

use nikgob_core::model::{Master, Source, SourceId, Track, TrackId};
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
    /// Where the replaced file's lyrics went in the Trash (`None`: it had
    /// none), as the import put them there ([`replaced_lyrics`]).
    pub old_lrc: Option<PathBuf>,
}

fn lyrics_key(old: SourceId) -> String {
    format!("replaced_lrc.{old}")
}

/// An import of confirmed files ([`crate::ImportTargets::pinned`]) moved
/// the lyrics of the file of source `old` it replaced to `in_trash`: kept
/// for [`replaced_lyrics`].
pub(crate) fn keep_replaced_lyrics(lib: &Library, old: SourceId, in_trash: &Path) {
    if let Err(e) = lib.cache_put(&lyrics_key(old), nikgob_library::now_ms(), &in_trash.to_string_lossy()) {
        log::warn!("where the lyrics of {} went is not kept: {e}", in_trash.display());
    }
}

/// Where the import put the lyrics of the replaced file of source `old`
/// (an import of confirmed files only); asked once, then forgotten.
pub fn replaced_lyrics(lib: &Library, old: SourceId) -> Option<PathBuf> {
    let (_, path) = lib.cache_get(&lyrics_key(old)).ok().flatten()?;
    let _ = lib.cache_forget(&lyrics_key(old), &path);
    Some(PathBuf::from(path))
}

/// Takes `r` back. The new file goes to the Trash first: whatever fails
/// after it is undone step by step, so the track never plays two live
/// files, never loses both, and a failed undo can be tried again. The
/// track's tags, master and lyrics are then put back as they were; any of
/// them that cannot be is an error too (tried again with the undo, the
/// files staying as they are by then).
pub fn undo_replacement(lib: &Library, trash_dir: &Path, r: &Replacement) -> Result<()> {
    let old = lib
        .get_source(r.old)?
        .ok_or_else(|| ImportError::Other(format!("the replaced file of {} is not known any more", r.track)))?;
    if old.trashed_at.is_some() {
        files_back(lib, trash_dir, r, &old)?;
    }
    rest_back(lib, r, &PathBuf::from(&old.locator))
}

/// The old file back from the Trash, the new one there.
fn files_back(lib: &Library, trash_dir: &Path, r: &Replacement, old: &Source) -> Result<()> {
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
    log::info!(
        "better copy taken back: {} is back from the Trash, {} went there",
        original.display(),
        new_trash.as_ref().map_or_else(|| "nothing".to_owned(), |t| t.display().to_string())
    );
    Ok(())
}

/// The track's tags, master and lyrics as they were, once its old file
/// plays again. Each is tried; what fails is said in the error.
fn rest_back(lib: &Library, r: &Replacement, original: &Path) -> Result<()> {
    let mut failed: Vec<String> = Vec::new();
    // The track's tags and place as they were, exactly.
    if let Err(e) = lib.put_track_back(&r.before) {
        failed.push(format!("tags: {e}"));
    }
    // Its master as it was: one the new file brought goes from the library
    // (its file stays where it is, it may be another track's too).
    let master = match &r.master_before {
        Some(m) => lib.put_master(m),
        None => lib.delete_master(r.track).map(|gone| {
            if gone {
                log::info!("the master the better copy of {} brought is no longer the track's", r.track);
            }
        }),
    };
    if let Err(e) = master {
        failed.push(format!("master: {e}"));
    }
    // Its lyrics file: the old one from the very place in the Trash the
    // import put it, else the new file's.
    let back = original.with_extension("lrc");
    if !long_path(&back).exists() {
        let from = match &r.old_lrc {
            Some(l) if long_path(l).exists() => Some(l.clone()),
            Some(l) => {
                failed.push(format!("lyrics: {} is not in the Trash any more", l.display()));
                None
            }
            None => lib
                .get_source(r.new)
                .ok()
                .flatten()
                .map(|s| PathBuf::from(&s.locator).with_extension("lrc"))
                .filter(|l| *l != back && long_path(l).exists()),
        };
        if let Some(l) = from
            && let Err(e) = fsutil::move_file(&l, &back)
        {
            failed.push(format!("lyrics: {e}"));
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(ImportError::Other(format!("{} is back, but not all of it: {}", original.display(), failed.join("; "))))
    }
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
        // A lyrics file of the same name was in the Trash already: this
        // one's name there is not the audio's.
        std::fs::create_dir_all(&trash).unwrap();
        std::fs::write(in_trash.with_extension("lrc"), b"[00:01]stranger").unwrap();
        let old_lrc = trash_file(&trash, &mp3_path.with_extension("lrc")).unwrap();
        assert_ne!(old_lrc, in_trash.with_extension("lrc"));
        lib.trash_source(mp3.id, Some(&in_trash.to_string_lossy())).unwrap();
        let new_path = if same_place { mp3_path.clone() } else { music.join("07 - Молли.opus") };
        std::fs::write(&new_path, b"opus").unwrap();
        let mut opus = Source::new(before.id, SourceKind::LocalFile, new_path.to_string_lossy(), Codec::Opus);
        opus.quality_score = 900;
        let opus = lib.add_source(&opus).unwrap();
        let mut retagged = before.clone();
        retagged.title = "Молли (Remastered)".into();
        retagged.isrc = Some("RUA1D0700001".into());
        lib.upsert_track(&retagged).unwrap();
        let r = Replacement { track: before.id, before, old: mp3.id, new: opus.id, master_before: None, old_lrc: Some(old_lrc) };
        (dir, lib, r, trash, mp3_path, new_path)
    }

    #[test]
    fn an_undo_brings_the_old_file_back_and_trashes_the_new_one() {
        for same_place in [false, true] {
            let (_dir, lib, r, trash, mp3_path, new_path) = replaced(same_place);
            assert_eq!(lib.active_source(r.track).unwrap().unwrap().id, r.new);
            undo_replacement(&lib, &trash, &r).unwrap();
            assert_eq!(std::fs::read(&mp3_path).unwrap(), b"mp3", "{same_place}");
            assert_eq!(std::fs::read(mp3_path.with_extension("lrc")).unwrap(), b"[00:01]a", "its own lyrics too");
            if !same_place {
                assert!(!new_path.exists());
            }
            assert_eq!(lib.active_source(r.track).unwrap().unwrap().id, r.old, "the old file plays");
            assert!(lib.get_source(r.new).unwrap().unwrap().trashed_at.is_some());
            let in_trash = lib.trash_path_of(r.new).unwrap().unwrap();
            assert_eq!(std::fs::read(&in_trash).unwrap(), b"opus", "the new file waits in the Trash");
            let track = lib.get_track(r.track).unwrap().unwrap();
            assert_eq!(track.title, "Молли", "the tags as they were");
            assert_eq!(track.isrc, None, "an ISRC the new file brought goes too");
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
