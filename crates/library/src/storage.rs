//! How much the library's own files take («Загрузки»' header).

use crate::db::Library;
use crate::error::Result;

/// The library's files as the database knows them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalFileSizes {
    /// The bytes of the files whose size is recorded.
    pub known: u64,
    /// The locators of those without one (to be looked at on disk).
    pub unknown: Vec<String>,
}

impl Library {
    /// The library's own audio files (not in the Trash, not missing): the
    /// sizes recorded at import summed, and the files without one.
    pub fn local_file_sizes(&self) -> Result<LocalFileSizes> {
        self.read(|c| {
            let known: i64 = c.query_row(
                "SELECT coalesce(sum(size_bytes), 0) FROM source
                 WHERE kind = 'local_file' AND trashed_at IS NULL AND available = 1 AND size_bytes IS NOT NULL",
                [],
                |r| r.get(0),
            )?;
            let mut stmt = c.prepare(
                "SELECT locator FROM source
                 WHERE kind = 'local_file' AND trashed_at IS NULL AND available = 1 AND size_bytes IS NULL",
            )?;
            let unknown = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(LocalFileSizes { known: u64::try_from(known).unwrap_or(0), unknown })
        })
    }
}

#[cfg(test)]
mod tests {
    use nikgob_core::model::{Artist, ArtistCredit, Codec, Source, SourceKind, Track};

    use crate::Library;

    #[test]
    fn sizes_of_the_library_files_skip_the_trash_and_streams() {
        let lib = Library::open_in_memory().unwrap();
        let a = lib.upsert_artist(&Artist::new("A")).unwrap();
        let credit = ArtistCredit::single(a.name.clone(), Some(a.id));
        let source = |title: &str, kind: SourceKind, locator: &str, size: Option<u64>| {
            let t = lib.upsert_track(&Track::new(title, credit.clone())).unwrap();
            let mut s = Source::new(t.id, kind, locator, Codec::Flac);
            s.size_bytes = size;
            lib.add_source(&s).unwrap()
        };
        source("1", SourceKind::LocalFile, "a/1.flac", Some(1_000));
        source("2", SourceKind::LocalFile, "a/2.flac", Some(2_500));
        source("3", SourceKind::LocalFile, "a/3.flac", None);
        source("4", SourceKind::Youtube, "vid", Some(9_999));
        let gone = source("5", SourceKind::LocalFile, "a/5.flac", Some(7_000));
        lib.trash_source(gone.id, Some("Trash/5.flac")).unwrap();
        let missing = source("6", SourceKind::LocalFile, "a/6.flac", Some(4_000));
        lib.set_source_available(missing.id, false).unwrap();
        let sizes = lib.local_file_sizes().unwrap();
        assert_eq!(sizes.known, 3_500);
        assert_eq!(sizes.unknown, vec!["a/3.flac".to_owned()]);
    }
}
