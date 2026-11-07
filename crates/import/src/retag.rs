//! Manual tag edits (the tag editor): new values go into the file, the
//! library rows follow, and the file moves to where §5 of the PLAN says a
//! file with these tags belongs. The track id stays the same.

use std::path::PathBuf;
use std::sync::PoisonError;

use nikgob_core::model::{ReleaseId, TrackId};

use crate::error::{ImportError, Result};
use crate::fsutil;
use crate::identity::identity_from_tags;
use crate::importer::{Importer, layout_input, rel_to_library};
use crate::sanitize::long_path;
use crate::tags::{self, Field, TagSet};

/// Fields the editor can change; `None` leaves the value alone, an empty
/// string clears it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TagChanges {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub year: Option<Option<i32>>,
    pub track_no: Option<Option<u32>>,
    /// The album's track count.
    pub track_total: Option<Option<u32>>,
    pub disc_no: Option<Option<u32>>,
    pub genre: Option<String>,
    pub recording_mbid: Option<String>,
    pub release_mbid: Option<String>,
    /// The album the track stays on, whatever album the tags name (a
    /// file put back at its place on its album).
    pub release: Option<ReleaseId>,
    /// The album artist stays as it is even when the artist changes from
    /// the same name (a repair gives a soundtrack's track its composer
    /// back; the editor's renamed artist takes the album along).
    pub keep_album_artist: bool,
}

fn opt(s: String) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

impl TagChanges {
    fn apply(self, t: &mut TagSet) {
        if let Some(v) = self.title {
            t.title = opt(v);
        }
        if let Some(v) = self.artist {
            t.artist = opt(v);
            t.artists = t.artist.iter().cloned().collect();
        }
        if let Some(v) = self.album {
            t.album = opt(v);
        }
        if let Some(v) = self.album_artist {
            t.album_artist = opt(v);
        }
        if let Some(v) = self.year {
            t.year = v;
            t.date = v.map(|y| y.to_string());
        }
        if let Some(v) = self.track_no {
            t.track_no = v;
        }
        if let Some(v) = self.track_total {
            t.track_total = v;
        }
        if let Some(v) = self.disc_no {
            t.disc_no = v;
        }
        if let Some(v) = self.genre {
            t.genre = opt(v);
        }
        if let Some(v) = self.recording_mbid {
            t.recording_mbid = opt(v);
        }
        if let Some(v) = self.release_mbid {
            t.release_mbid = opt(v);
        }
    }
}

fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (std::fs::canonicalize(long_path(a)), std::fs::canonicalize(long_path(b))) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Fields that had a value before the edit and have none after it.
fn cleared_fields(before: &TagSet, after: &TagSet) -> Vec<Field> {
    let mut out = Vec::new();
    let mut check = |was: bool, is: bool, field: Field| {
        if was && !is {
            out.push(field);
        }
    };
    check(before.album.is_some(), after.album.is_some(), Field::Album);
    check(
        before.album_artist.is_some(),
        after.album_artist.is_some(),
        Field::AlbumArtist,
    );
    check(before.genre.is_some(), after.genre.is_some(), Field::Genre);
    check(
        before.year.is_some() || before.date.is_some(),
        after.year.is_some() || after.date.is_some(),
        Field::Date,
    );
    check(
        before.track_no.is_some(),
        after.track_no.is_some(),
        Field::TrackNumber,
    );
    check(
        before.disc_no.is_some(),
        after.disc_no.is_some(),
        Field::DiscNumber,
    );
    check(
        before.recording_mbid.is_some(),
        after.recording_mbid.is_some(),
        Field::RecordingMbid,
    );
    check(
        before.release_mbid.is_some(),
        after.release_mbid.is_some(),
        Field::ReleaseMbid,
    );
    out
}

impl Importer {
    /// Current tags of the track's active local file.
    pub fn read_track_tags(&self, track: TrackId) -> Result<(PathBuf, TagSet)> {
        let src = self
            .library()
            .active_source(track)?
            .filter(|s| s.kind.is_local())
            .ok_or_else(|| ImportError::Other("the track has no local file".into()))?;
        let path = PathBuf::from(&src.locator);
        let (set, _) = tags::read_tags(&path)?;
        Ok((path, set))
    }

    /// Writes `changes` into the file, updates the library and moves the
    /// file (with its `.lrc`) to its place under the new tags. Returns the
    /// new path.
    pub fn retag(&self, track: TrackId, changes: TagChanges) -> Result<PathBuf> {
        let lib = self.library().clone();
        let mut src = lib
            .active_source(track)?
            .filter(|s| s.kind.is_local())
            .ok_or_else(|| ImportError::Other("the track has no local file".into()))?;
        let path = PathBuf::from(&src.locator);
        let (mut set, _) = tags::read_tags(&path)?;
        let (old_artist, old_aa) = (set.artist.clone(), set.album_artist.clone());
        let before = set.clone();
        let keep_aa = changes.keep_album_artist;
        let stays_on = changes.release;
        changes.apply(&mut set);
        // The album artist follows a renamed artist when it was the same
        // person (or a placeholder) and the editor left it alone.
        let aa_untouched = set.album_artist == old_aa && !keep_aa;
        let aa_follows = old_aa.is_none()
            || old_aa == old_artist
            || old_aa.as_deref().is_some_and(|a| a.eq_ignore_ascii_case("Unknown Artist"));
        if aa_untouched && aa_follows && set.artist != old_artist {
            set.album_artist = set.artist.clone();
        }
        set.track_id = Some(track);
        let mut identity = identity_from_tags(&set, &path);
        identity.release_id = stays_on;

        let mut store = self
            .inner
            .store
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut row = lib
            .get_track(track)?
            .ok_or_else(|| ImportError::Other(format!("no track {track}")))?;
        let old_release = row.release_id.and_then(|r| lib.get_release(r).ok().flatten());
        let cover_ref = old_release.as_ref().and_then(|r| r.cover_ref.clone());
        let release = store.ensure_release(&identity, cover_ref)?;
        row.title = identity.title.clone();
        row.artist_credit = store.track_credit(&identity)?;
        row.release_id = Some(release.id);
        row.disc_no = identity.disc_no.max(1);
        row.track_no = identity.track_no;
        if identity.recording_mbid.is_some() {
            row.recording_mbid = identity.recording_mbid.clone();
        }

        tags::write_tags(&path, &set)?;
        // `write_tags` only sets values; a field the editor emptied has to be
        // deleted from the file too, or it comes back on the next read/rebuild.
        tags::remove_fields(&path, &cleared_fields(&before, &set))?;

        // Move into place if the tags imply another folder or name.
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().into_owned())
            .unwrap_or_default();
        let layout = layout_input(&identity);
        let lib_dir = &self.config().library_dir;
        let target = layout.target_path(lib_dir, &ext);
        let mut new_path = path.clone();
        if path.starts_with(lib_dir) && target != path {
            // A change of letter case only names the same file on a
            // case-insensitive volume: rename it, do not dodge to "(2)".
            let target = if same_file(&path, &target) {
                target
            } else {
                crate::sanitize::unique_path(&target)
            };
            if let Some(dir) = target.parent() {
                fsutil::create_dir_all(dir)?;
            }
            fsutil::move_file(&path, &target)?;
            let lrc = path.with_extension("lrc");
            if long_path(&lrc).exists() {
                let _ = fsutil::move_file(&lrc, &target.with_extension("lrc"));
            }
            // The cover follows the first track that lands in a new folder.
            if let (Some(old_dir), Some(new_dir)) = (path.parent(), target.parent()) {
                for name in ["cover.jpg", "ambient.png"] {
                    let (from, to) = (old_dir.join(name), new_dir.join(name));
                    if long_path(&from).exists() && !long_path(&to).exists() {
                        let _ = fsutil::copy_file(&from, &to);
                    }
                }
                if release.cover_ref.is_some() && new_dir != old_dir {
                    let mut r = release.clone();
                    r.cover_ref = Some(rel_to_library(self.config(), &new_dir.join("cover.jpg")));
                    if long_path(&new_dir.join("cover.jpg")).exists() {
                        lib.upsert_release(&r)?;
                    }
                }
                fsutil::prune_empty_dirs(old_dir, lib_dir);
            }
            src.locator = target.to_string_lossy().into_owned();
            lib.add_source(&src)?;
            new_path = target;
        }
        lib.upsert_track(&row)?;
        Ok(new_path)
    }
}
