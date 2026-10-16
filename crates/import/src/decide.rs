//! New, upgrade or duplicate? (PLAN section 6, step 4 and "Апгрейд".)
//!
//! The incoming file is the *same track* as an existing one when, in order:
//! 1. it carries our own `NIKGOB_TRACK_ID` of a known track;
//! 2. recording MBID, AcoustID id or ISRC match a track that lives on the same
//!    release, has no release, or has no local file (a stream-only track the
//!    user liked). The same recording on a compilation is a different track;
//! 3. inside the same release: the Chromaprint fingerprints are alike, or, when
//!    a fingerprint is missing on either side, the title and track number agree.
//!
//! Then [`decide`] compares `quality_score`: higher replaces the active file
//! (the track id stays), equal or lower is a duplicate.

use std::collections::HashSet;
use std::path::Path;

use nikgob_core::model::{Source, Track, TrackId};
use nikgob_library::Library;
use nikgob_metadata::text::title_key;

use crate::analysis::{Fingerprint, SAME_RECORDING_SIMILARITY, fingerprint_similarity};
use crate::error::Result;
use crate::identity::Identity;
use crate::store::Store;

/// An existing track together with its live local file, if any.
#[derive(Clone, Debug)]
pub struct Existing {
    pub track: Track,
    pub local: Option<Source>,
}

#[derive(Clone, Debug)]
pub enum Decision {
    /// A track the library has not seen.
    New,
    /// Same track, the incoming file is better: it becomes the active source.
    Upgrade(Existing),
    /// Same track without a local file (stream-only, or its file was trashed).
    AddSource(Existing),
    /// Same track, not better: the incoming file is not needed.
    Duplicate(Existing),
}

impl Decision {
    pub fn track_id(&self) -> Option<TrackId> {
        match self {
            Self::New => None,
            Self::Upgrade(e) | Self::AddSource(e) | Self::Duplicate(e) => Some(e.track.id),
        }
    }

    pub fn is_duplicate(&self) -> bool {
        matches!(self, Self::Duplicate(_))
    }
}

/// The pure part: what to do with an incoming file of `incoming_score`.
pub fn decide(existing: Option<Existing>, incoming_score: i32) -> Decision {
    match existing {
        None => Decision::New,
        Some(e) => match &e.local {
            None => Decision::AddSource(e),
            Some(local) if incoming_score > local.quality_score => Decision::Upgrade(e),
            Some(_) => Decision::Duplicate(e),
        },
    }
}

/// What the matching needs to know about the incoming file.
pub struct Incoming<'a> {
    pub identity: &'a Identity,
    /// `NIKGOB_TRACK_ID` read from the file.
    pub tag_track_id: Option<TrackId>,
    pub fingerprint: Option<&'a [u32]>,
}

fn existing_of(lib: &Library, track: Track) -> Result<Existing> {
    let local = lib
        .active_source(track.id)?
        .filter(|s| s.kind.is_local() && s.available);
    Ok(Existing { track, local })
}

fn stored_fingerprint(source: &Source) -> Option<Vec<u32>> {
    let (tags, _) = crate::tags::read_tags(Path::new(&source.locator)).ok()?;
    Fingerprint::decode(tags.acoustid_fingerprint.as_deref()?)
}

/// Looks for the track the incoming file belongs to. Creates nothing.
pub(crate) fn find_existing(store: &mut Store, inc: &Incoming<'_>) -> Result<Option<Existing>> {
    let lib = store.library().clone();
    let id = inc.identity;

    // 1. Our own id.
    if let Some(tid) = inc.tag_track_id
        && let Some(track) = lib.get_track(tid)?
    {
        return Ok(Some(existing_of(&lib, track)?));
    }

    let release = store.find_release(id)?;

    // 2. External ids.
    let mut seen = HashSet::new();
    let mut candidates: Vec<Track> = Vec::new();
    if let Some(m) = &id.recording_mbid {
        candidates.extend(lib.tracks_by_recording_mbid(m)?);
    }
    if let Some(m) = &id.acoustid_id {
        candidates.extend(lib.tracks_by_acoustid(m)?);
    }
    if let Some(m) = &id.isrc {
        candidates.extend(lib.tracks_by_isrc(m)?);
    }
    for track in candidates {
        if !seen.insert(track.id) {
            continue;
        }
        let same_release = track.release_id.is_some() && track.release_id == release;
        let ex = existing_of(&lib, track)?;
        if same_release || ex.track.release_id.is_none() || ex.local.is_none() {
            return Ok(Some(ex));
        }
    }

    // 3. Inside the same release.
    let Some(release) = release else {
        return Ok(None);
    };
    let want = title_key(&id.title);
    for row in lib.tracks_of_release_with_deleted(release)? {
        if seen.contains(&row.id) {
            continue;
        }
        let have = title_key(&row.title);
        let title_same = have.base == want.base && have.variant == want.variant;
        let num_same = id.track_no != 0 && row.track_no == id.track_no && row.disc_no == id.disc_no;
        if !(title_same || num_same) {
            continue;
        }
        let Some(track) = lib.get_track(row.id)? else {
            continue;
        };
        let ex = existing_of(&lib, track)?;
        let their_fp = ex.local.as_ref().and_then(stored_fingerprint);
        let matches = match (inc.fingerprint, their_fp) {
            (Some(mine), Some(theirs)) => {
                fingerprint_similarity(mine, &theirs) >= SAME_RECORDING_SIMILARITY
            }
            _ => title_same && (num_same || id.track_no == 0 || row.track_no == 0),
        };
        if matches {
            return Ok(Some(ex));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::identity_from_tags;
    use crate::tags::TagSet;
    use nikgob_core::model::{Codec, SourceKind, quality_score};

    fn identity(title: &str, no: u32, album: &str) -> Identity {
        identity_from_tags(
            &TagSet {
                title: Some(title.into()),
                artist: Some("Artist".into()),
                album: Some(album.into()),
                track_no: Some(no),
                year: Some(2001),
                ..TagSet::default()
            },
            Path::new("x.flac"),
        )
    }

    /// Imports a row the way the pipeline does: returns the track id.
    fn add(store: &mut Store, id: &Identity, codec: Codec, score: i32, local: bool) -> TrackId {
        let release = store.ensure_release(id, None).unwrap();
        let credit = store.track_credit(id).unwrap();
        let mut t = Track::new(id.title.clone(), credit);
        t.release_id = Some(release.id);
        t.track_no = id.track_no;
        t.recording_mbid = id.recording_mbid.clone();
        let t = store.library().upsert_track(&t).unwrap();
        let (kind, locator) = if local {
            (SourceKind::LocalFile, format!("C:/Music/{}.opus", t.id))
        } else {
            (SourceKind::Spotify, format!("spotify:track:{}", t.id))
        };
        let mut s = Source::new(t.id, kind, locator, codec);
        s.quality_score = score;
        store.library().add_source(&s).unwrap();
        t.id
    }

    fn score_of(codec: Codec, kbps: Option<u32>, rate: u32, depth: Option<u8>, fake: bool) -> i32 {
        quality_score(&codec, kbps, Some(rate), depth, fake)
    }

    #[test]
    fn the_pure_decision() {
        let lib = Library::open_in_memory().unwrap();
        let mut store = Store::new(lib);
        let id = identity("Song", 1, "Album");
        let tid = add(&mut store, &id, Codec::Mp3, 775, true);
        let track = store.library().get_track(tid).unwrap().unwrap();
        let local = store.library().active_source(tid).unwrap();
        let ex = Existing { track, local };
        assert!(matches!(decide(None, 100), Decision::New));
        assert!(matches!(decide(Some(ex.clone()), 900), Decision::Upgrade(_)));
        assert!(matches!(decide(Some(ex.clone()), 775), Decision::Duplicate(_)), "equal is a duplicate");
        assert!(matches!(decide(Some(ex.clone()), 500), Decision::Duplicate(_)));
        let no_local = Existing { local: None, ..ex };
        assert!(matches!(decide(Some(no_local), 10), Decision::AddSource(_)));
    }

    #[test]
    fn finds_by_title_and_number_in_the_same_release() {
        let mut store = Store::new(Library::open_in_memory().unwrap());
        let id = identity("Song", 1, "Album");
        let tid = add(&mut store, &id, Codec::Mp3, 775, true);
        let inc = Incoming {
            identity: &identity("Song (Remastered 2011)", 1, "Album"),
            tag_track_id: None,
            fingerprint: None,
        };
        let found = find_existing(&mut store, &inc).unwrap().expect("same track");
        assert_eq!(found.track.id, tid);
        assert_eq!(found.local.unwrap().quality_score, 775);

        // Another song of the album, or the same title on another album, is new.
        for other in [identity("Other", 2, "Album"), identity("Song", 1, "Different Album")] {
            let inc = Incoming {
                identity: &other,
                tag_track_id: None,
                fingerprint: None,
            };
            assert!(find_existing(&mut store, &inc).unwrap().is_none(), "{}", other.title);
        }
        // A live version on the same release is a different recording.
        let live = identity("Song (Live)", 1, "Album");
        let inc = Incoming {
            identity: &live,
            tag_track_id: None,
            fingerprint: None,
        };
        assert!(find_existing(&mut store, &inc).unwrap().is_none());
    }

    #[test]
    fn recording_mbid_matches_per_release_and_stream_only_tracks() {
        let mut store = Store::new(Library::open_in_memory().unwrap());
        let mut a = identity("Song", 1, "Album");
        a.recording_mbid = Some("rec-1".into());
        let on_album = add(&mut store, &a, Codec::Flac, 900, true);

        // The same recording on a compilation is another track.
        let mut comp = identity("Song", 5, "Hits 2001");
        comp.recording_mbid = Some("rec-1".into());
        let inc = Incoming {
            identity: &comp,
            tag_track_id: None,
            fingerprint: None,
        };
        assert!(find_existing(&mut store, &inc).unwrap().is_none());

        // ...unless the existing track has no local file (a liked stream track).
        let mut s = identity("Streamed", 1, "Streaming Album");
        s.recording_mbid = Some("rec-2".into());
        let streamed = add(&mut store, &s, Codec::Opus, 0, false);
        let mut file = identity("Streamed", 3, "Other Release");
        file.recording_mbid = Some("rec-2".into());
        let inc = Incoming {
            identity: &file,
            tag_track_id: None,
            fingerprint: None,
        };
        let found = find_existing(&mut store, &inc).unwrap().expect("claims the stream track");
        assert_eq!(found.track.id, streamed);
        assert!(found.local.is_none());
        assert!(matches!(decide(Some(found), 1), Decision::AddSource(_)));

        // Same release + same recording: found as the album track.
        let inc = Incoming {
            identity: &a,
            tag_track_id: None,
            fingerprint: None,
        };
        assert_eq!(find_existing(&mut store, &inc).unwrap().unwrap().track.id, on_album);
    }

    #[test]
    fn own_track_id_wins() {
        let mut store = Store::new(Library::open_in_memory().unwrap());
        let tid = add(&mut store, &identity("Song", 1, "Album"), Codec::Flac, 900, true);
        let unrelated = identity("Totally Different", 9, "Elsewhere");
        let inc = Incoming {
            identity: &unrelated,
            tag_track_id: Some(tid),
            fingerprint: None,
        };
        assert_eq!(find_existing(&mut store, &inc).unwrap().unwrap().track.id, tid);
        let unknown = Incoming {
            identity: &unrelated,
            tag_track_id: Some(TrackId::new()),
            fingerprint: None,
        };
        assert!(find_existing(&mut store, &unknown).unwrap().is_none());
    }

    #[test]
    fn quality_ladder_gives_new_upgrade_duplicate() {
        let mp3_320 = score_of(Codec::Mp3, Some(320), 44_100, None, false);
        let mp3_128 = score_of(Codec::Mp3, Some(128), 44_100, None, false);
        let flac = score_of(Codec::Flac, None, 44_100, Some(16), false);
        let hires = score_of(Codec::Flac, None, 96_000, Some(24), false);
        let fake = score_of(Codec::Flac, None, 44_100, Some(16), true);
        let mut store = Store::new(Library::open_in_memory().unwrap());
        let id = identity("Song", 1, "Album");
        let tid = add(&mut store, &id, Codec::Mp3, mp3_320, true);
        let existing = || {
            let track = store.library().get_track(tid).unwrap().unwrap();
            Some(Existing {
                local: store.library().active_source(tid).unwrap(),
                track,
            })
        };
        assert!(matches!(decide(existing(), flac), Decision::Upgrade(_)));
        assert!(matches!(decide(existing(), hires), Decision::Upgrade(_)));
        assert!(matches!(decide(existing(), mp3_128), Decision::Duplicate(_)));
        assert!(matches!(decide(existing(), fake), Decision::Duplicate(_)), "fake FLAC is worse than MP3 320");
    }
}
