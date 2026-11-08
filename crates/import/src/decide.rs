//! New, upgrade or duplicate? (PLAN section 6, step 4 and "Апгрейд".)
//!
//! The incoming file is the *same track* as an existing one when, in order:
//! 1. it carries our own `NIKGOB_TRACK_ID` of a known track (or the job was
//!    told which track it is: [`ImportTargets::force`]);
//! 2. recording MBID, AcoustID id or ISRC match a track that lives on the same
//!    release, has no release, or has no local file (a stream-only track the
//!    user liked). The same recording on a compilation is a different track,
//!    and a stream-only track whose length differs by more than
//!    [`DURATION_SLACK_MS`] is not matched silently (see [`stream_doubt`]);
//! 3. one of the job's [`ImportTargets`] (the wishlist item it was downloaded
//!    for) has this title (or this number and a similar title) and length;
//! 4. a stream-only track (liked from Spotify / YouTube, no file yet) has the
//!    same artist and title (or the title with and without its romanization,
//!    «夏子 - Natsuko» and «夏子») and a length within [`DURATION_SLACK_MS`]:
//!    the file attaches to that track id, so playlists, likes and history stay;
//! 5. inside the same release: the Chromaprint fingerprints are alike, or, when
//!    a fingerprint is missing on either side, the title and track number agree.
//!
//! Then [`decide`] compares `quality_score`: higher replaces the active file
//! (the track id stays), equal or lower is a duplicate.

use std::collections::HashSet;
use std::path::Path;

use nikgob_core::clip_title::same_dual_title;
use nikgob_core::model::{ReleaseId, Source, Track, TrackId, WishlistId};
use nikgob_library::Library;
use nikgob_metadata::text::{album_key, artist_key, similarity, title_key};

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

/// How far (ms) the length of a file may be from a stream track's to be the
/// same recording (docs/stage3.md §1).
pub const DURATION_SLACK_MS: u32 = 3_000;

/// Title similarity (0..=1) a file needs to match one of its job's targets.
const TARGET_TITLE_SIMILARITY: f64 = 0.85;
/// The same, when the track number agrees too.
const TARGET_NUMBERED_SIMILARITY: f64 = 0.5;
/// Artist similarity for matching a stream-only track by name.
const STREAM_ARTIST_SIMILARITY: f64 = 0.8;

/// The library tracks the files of one import job are meant for: the
/// wishlist item a torrent was downloaded for, or the item the user attached
/// a file to. A file that fits one of them (title and length) becomes that
/// track's local source; the other files of the item's album land on its
/// release (see [`crate::album`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImportTargets {
    pub tracks: Vec<TrackId>,
    /// The user said which track the file is: with exactly one target and
    /// one file, the file attaches to it whatever its tags say.
    pub force: bool,
    /// The album the item asked for (its tracks may be fewer than the
    /// album has).
    #[serde(default)]
    pub release: Option<ReleaseId>,
    /// The «Хочу скачать» item, for files that wait in «Подтверди».
    #[serde(default)]
    pub wishlist: Option<WishlistId>,
    /// Better copies of these tracks (a «Качество лучше» item): a file that
    /// is none of them (another album, a stranger's song) is left out,
    /// never imported as something new.
    #[serde(default)]
    pub only_targets: bool,
    /// Where the files came from (`soulseek:<user\path>`, `qobuz:<id>`,
    /// `torrents:<id>`): a file that holds another song's audio makes this
    /// place one that is not taken for its track again.
    #[serde(default)]
    pub from: Option<String>,
}

impl ImportTargets {
    pub fn new(tracks: Vec<TrackId>) -> Self {
        Self {
            tracks,
            ..Self::default()
        }
    }

    /// The one track every file of the job is (see [`Self::force`]).
    pub fn forced(&self) -> Option<TrackId> {
        match self.tracks[..] {
            [one] if self.force => Some(one),
            _ => None,
        }
    }
}

/// What the matching needs to know about the incoming file.
pub struct Incoming<'a> {
    pub identity: &'a Identity,
    /// `NIKGOB_TRACK_ID` read from the file, or the track the user chose.
    pub tag_track_id: Option<TrackId>,
    pub fingerprint: Option<&'a [u32]>,
    /// The job's [`ImportTargets::tracks`].
    pub targets: &'a [TrackId],
    /// Length of the file.
    pub duration_ms: Option<u32>,
}

fn existing_of(lib: &Library, track: Track) -> Result<Existing> {
    let local = lib
        .active_source(track.id)?
        .filter(|s| s.kind.is_local() && s.available);
    Ok(Existing { track, local })
}

/// `Some(true)` when both lengths are known and within the slack.
fn duration_close(a: Option<u32>, b: Option<u32>) -> Option<bool> {
    Some(a?.abs_diff(b?) <= DURATION_SLACK_MS)
}

/// The track is on another album than `release`, the file's album the
/// library has.
fn elsewhere(track: &Track, release: Option<ReleaseId>) -> bool {
    release.is_some() && track.release_id.is_some() && track.release_id != release
}

/// A track without a local file that streams (a liked Spotify / YouTube track).
fn is_stream_only(lib: &Library, ex: &Existing) -> Result<bool> {
    Ok(ex.local.is_none()
        && lib
            .sources_of_track(ex.track.id)?
            .iter()
            .any(|s| !s.kind.is_local() && s.trashed_at.is_none()))
}

/// Same title (also the same variant: a live take is another recording; a
/// dual title is its halves, see [`same_dual_title`]) and a similar artist.
fn same_song(track: &Track, id: &Identity) -> bool {
    let (have, want) = (title_key(&track.title), title_key(&id.title));
    let same_title = !have.base.is_empty() && have == want
        || same_dual_title(&track.title, &id.title, title_key);
    if !same_title {
        return false;
    }
    let (a, b) = (artist_key(&track.artist_credit.display()), artist_key(&id.artist_line));
    a.is_empty() || b.is_empty() || similarity(&a, &b) >= STREAM_ARTIST_SIMILARITY
}

/// The track's album has the file's album title.
fn same_album(lib: &Library, track: &Track, id: &Identity) -> Result<bool> {
    let (Some(rid), Some(album)) = (track.release_id, id.album.as_deref()) else {
        return Ok(false);
    };
    Ok(lib
        .get_release(rid)?
        .is_some_and(|r| album_key(&r.title) == album_key(album)))
}

/// Step 3: the job's target that this file is.
fn target_match(lib: &Library, inc: &Incoming<'_>) -> Result<Option<Track>> {
    let id = inc.identity;
    let want = title_key(&id.title);
    let mut best: Option<(f64, Track)> = None;
    for &tid in inc.targets {
        let Some(track) = lib.get_track(tid)? else { continue };
        let have = title_key(&track.title);
        if have.variant != want.variant {
            continue;
        }
        let sim = similarity(&have.base, &want.base);
        let numbered = id.track_no != 0
            && track.track_no == id.track_no
            && track.disc_no == id.disc_no.max(1);
        let fits = sim >= TARGET_TITLE_SIMILARITY || (numbered && sim >= TARGET_NUMBERED_SIMILARITY);
        if !fits || duration_close(track.duration_ms, inc.duration_ms) == Some(false) {
            continue;
        }
        if best.as_ref().is_none_or(|(s, _)| sim > *s) {
            best = Some((sim, track));
        }
    }
    Ok(best.map(|(_, t)| t))
}

/// Step 4: a stream-only track with this artist, title and length. Without
/// a length on either side the album title has to agree instead. Never one
/// of another album when the file's own album is in the library: a track
/// is its place there (docs/stage4.md §12.13).
fn stream_match(lib: &Library, inc: &Incoming<'_>, release: Option<ReleaseId>) -> Result<Option<Track>> {
    for track in lib.stream_only_tracks()? {
        if !same_song(&track, inc.identity) || elsewhere(&track, release) {
            continue;
        }
        let fits = match duration_close(track.duration_ms, inc.duration_ms) {
            Some(close) => close,
            None => same_album(lib, &track, inc.identity)?,
        };
        if fits {
            return Ok(Some(track));
        }
    }
    Ok(None)
}

/// A stream-only track the file seems to be (same ids, or same artist and
/// title) whose length is off by more than [`DURATION_SLACK_MS`]: a TV size,
/// a radio edit, another version. Not attached silently; the importer asks
/// ("Подтверди"). Call it only when [`find_existing`] found nothing.
pub(crate) fn stream_doubt(store: &mut Store, inc: &Incoming<'_>) -> Result<Option<Track>> {
    let lib = store.library().clone();
    let id = inc.identity;
    let off = |t: &Track| duration_close(t.duration_ms, inc.duration_ms) == Some(false);
    let same_ids = |t: &Track| {
        let eq = |a: &Option<String>, b: &Option<String>| a.is_some() && a == b;
        eq(&t.isrc, &id.isrc) || eq(&t.recording_mbid, &id.recording_mbid)
    };
    let mut candidates = lib.stream_only_tracks()?;
    for &tid in inc.targets {
        if !candidates.iter().any(|t| t.id == tid)
            && let Some(t) = lib.get_track(tid)?
        {
            candidates.push(t);
        }
    }
    Ok(candidates
        .into_iter()
        .find(|t| off(t) && (same_ids(t) || same_song(t, id))))
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
        if is_stream_only(&lib, &ex)?
            && duration_close(ex.track.duration_ms, inc.duration_ms) == Some(false)
        {
            // Another version of a liked stream track: asked about instead.
            continue;
        }
        // A track without a file takes it, unless it has an album of its
        // own and the file's album is another one the library has.
        if same_release || ex.track.release_id.is_none() || (ex.local.is_none() && !elsewhere(&ex.track, release)) {
            return Ok(Some(ex));
        }
    }

    // 3. The track the job was started for.
    if let Some(track) = target_match(&lib, inc)? {
        return Ok(Some(existing_of(&lib, track)?));
    }

    // 4. A liked stream track, by name and length.
    if let Some(track) = stream_match(&lib, inc, release)? {
        return Ok(Some(existing_of(&lib, track)?));
    }

    // 5. Inside the same release.
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
            targets: &[],
            duration_ms: None,
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
                targets: &[],
                duration_ms: None,
            };
            assert!(find_existing(&mut store, &inc).unwrap().is_none(), "{}", other.title);
        }
        // A live version on the same release is a different recording.
        let live = identity("Song (Live)", 1, "Album");
        let inc = Incoming {
            identity: &live,
            tag_track_id: None,
            fingerprint: None,
            targets: &[],
            duration_ms: None,
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
            targets: &[],
            duration_ms: None,
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
            targets: &[],
            duration_ms: None,
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
            targets: &[],
            duration_ms: None,
        };
        assert_eq!(find_existing(&mut store, &inc).unwrap().unwrap().track.id, on_album);
    }

    #[test]
    fn a_file_with_its_album_does_not_join_a_fileless_track_of_another_album() {
        let mut store = Store::new(Library::open_in_memory().unwrap());
        // The standard edition's row from the service, without a file.
        let mut standard = identity("Alien Boy", 7, "Ugly Is Beautiful");
        standard.isrc = Some("USUM72000001".into());
        let row = add(&mut store, &standard, Codec::Opus, 0, false);
        let mut t = store.library().get_track(row).unwrap().unwrap();
        t.duration_ms = Some(160_000);
        t.isrc = standard.isrc.clone();
        store.library().upsert_track(&t).unwrap();
        // The deluxe edition is in the library; its file of the song comes.
        add(&mut store, &identity("Me, Myself & I", 1, "Ugly Is Beautiful: Shorter, Thicker & Uglier (Deluxe)"), Codec::Opus, 500, true);
        let mut deluxe = identity("Alien Boy", 14, "Ugly Is Beautiful: Shorter, Thicker & Uglier (Deluxe)");
        deluxe.isrc = standard.isrc.clone();
        let inc = Incoming { identity: &deluxe, tag_track_id: None, fingerprint: None, targets: &[], duration_ms: Some(160_000) };
        assert!(find_existing(&mut store, &inc).unwrap().is_none(), "a track of its own album");
        // Its own album's row takes it.
        let inc = Incoming { identity: &standard, tag_track_id: None, fingerprint: None, targets: &[], duration_ms: Some(160_000) };
        assert_eq!(find_existing(&mut store, &inc).unwrap().map(|e| e.track.id), Some(row));
        // The job's target takes it wherever it is.
        let inc = Incoming { identity: &deluxe, tag_track_id: None, fingerprint: None, targets: &[row], duration_ms: Some(160_000) };
        assert_eq!(find_existing(&mut store, &inc).unwrap().map(|e| e.track.id), Some(row));
    }

    #[test]
    fn a_file_takes_the_stream_track_of_its_song_under_a_dual_title() {
        let mut store = Store::new(Library::open_in_memory().unwrap());
        let streamed = add(&mut store, &identity("夏子 - Natsuko", 1, "Natsuko"), Codec::Opus, 0, false);
        let mut t = store.library().get_track(streamed).unwrap().unwrap();
        t.duration_ms = Some(213_000);
        store.library().upsert_track(&t).unwrap();
        let find = |store: &mut Store, title: &str| {
            let id = identity(title, 4, "夏子");
            let inc = Incoming {
                identity: &id,
                tag_track_id: None,
                fingerprint: None,
                targets: &[],
                duration_ms: Some(212_800),
            };
            find_existing(store, &inc).unwrap().map(|e| e.track.id)
        };
        assert_eq!(find(&mut store, "夏子"), Some(streamed), "another album, the title without its romanization");
        assert_eq!(find(&mut store, "Natsuko"), Some(streamed));
        assert_eq!(find(&mut store, "夏子 (Live)"), None, "a version");
        assert_eq!(find(&mut store, "夏子 - Live"), None, "a version");
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
            targets: &[],
            duration_ms: None,
        };
        assert_eq!(find_existing(&mut store, &inc).unwrap().unwrap().track.id, tid);
        let unknown = Incoming {
            identity: &unrelated,
            tag_track_id: Some(TrackId::new()),
            fingerprint: None,
            targets: &[],
            duration_ms: None,
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
