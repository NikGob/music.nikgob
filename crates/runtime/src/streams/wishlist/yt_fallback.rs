//! The «Копия с YouTube» preference: a wished track that no source has is
//! saved from YouTube Music by itself, quietly. The pipeline calls
//! [`Worker::wish_nothing_found`] only where it concludes «nothing
//! anywhere» — every source that was asked answered, and none had a sure
//! offer; a source that was silent, paused, rate-limited or turned us away
//! leads to [`Worker::wish_later`] (or «источники молчат») instead, never
//! to a save. The save is the manual one's
//! ([`Worker::wish_save_from_youtube`]) without its toasts: a failure just
//! leaves the item «не нашёл», and its «История» line is the same
//! ([`nikgob_core::i18n::status::StoredLine::SavingFromYoutube`]). A music
//! video or a short is never saved: only a source scored at least
//! [`score::STREAM_YOUTUBE`], or a clip's song counterpart when that is all
//! there is. At most one such save runs at a time; the rest wait for a
//! later wishlist tick.

use super::*;
use nikgob_core::streaming::{RemoteId, StreamService, StreamTrack};

/// Run tokens of the saves this module started, for [`Worker::wish_saved`]
/// to tell them from the manual ones (quiet, no toasts). A token is never
/// reused ([`crate::running::Runs::reserve`]).
static FALLBACK_RUNS: parking_lot::Mutex<Vec<u64>> = parking_lot::Mutex::new(Vec::new());

/// Remembers `run` as a save started by the fallback.
fn mark_fallback_run(run: u64) {
    FALLBACK_RUNS.lock().push(run);
}

/// Whether `run` was started by the fallback (and forgets it).
pub(super) fn take_fallback_run(run: u64) -> bool {
    let mut runs = FALLBACK_RUNS.lock();
    runs.iter().position(|r| *r == run).is_some_and(|at| {
        runs.remove(at);
        true
    })
}

/// Under this length a video is a short, unless the wished track is about
/// as long (a genuinely short song).
const SHORT_MS: u64 = 60_000;

/// How close to the wished track's length a short video's must be to count
/// as the same song.
const CLOSE_MS: u64 = 10_000;

/// Whether a raw title says the video is a short or a preview («#shorts»,
/// «short», «TV Size», «teaser», «preview», «【MV short】»): the titles the
/// searching strips such words from ([`nikgob_core::clip_title`]), so the
/// raw one is what is checked.
fn marks_short(title: &str) -> bool {
    let low = title.to_lowercase();
    if ["#shorts", "tv size", "teaser", "preview"].iter().any(|m| low.contains(m)) {
        return true;
    }
    low.split(|c: char| !c.is_alphanumeric()).any(|w| w == "short" || w == "shorts")
}

/// Whether a video of the known length `video_ms` may be saved for a track
/// of `track_ms`: under [`SHORT_MS`] only when the track is about as long.
fn duration_fits(video_ms: Option<u64>, track_ms: Option<u64>) -> bool {
    match video_ms {
        Some(v) if v < SHORT_MS => track_ms.is_some_and(|t| t.abs_diff(v) <= CLOSE_MS),
        _ => true,
    }
}

/// Whether the fallback may save for `item` now: the preference is on, it
/// is a track item that is not a better-copy lookup and has no copy yet,
/// the YouTube service is on, and no such save runs already.
fn gate(item: &WishlistItem, pref: bool, has_copy: bool, yt_on: bool, busy: bool) -> bool {
    pref
        && item.scope == WishlistScope::Track
        && item.origin != WishlistOrigin::Upgrade
        && item.track_id.is_some()
        && !has_copy
        && yt_on
        && !busy
}

/// The track's best YouTube source, as the manual save takes it: the
/// highest scored one that is available and not trashed. `None` when there
/// is none; the bool tells the best is only a music video (scored below
/// [`score::STREAM_YOUTUBE`]), whose song counterpart is tried instead.
fn best_youtube_source(sources: &[nikgob_core::model::Source]) -> Option<(String, bool)> {
    sources
        .iter()
        .filter(|s| s.kind == SourceKind::Youtube && s.available && s.trashed_at.is_none())
        .max_by_key(|s| s.quality_score)
        .map(|s| (s.locator.clone(), s.quality_score < score::STREAM_YOUTUBE))
}

/// Whether `t` — the video, or a clip's song — may be saved as the copy:
/// not a video the user marked wrong before, not titled a short or a
/// preview, and of a length that fits the wished track.
fn ok_to_save(t: &StreamTrack, track_ms: Option<u64>, refused: &[String]) -> bool {
    !refused.iter().any(|r| *r == t.id.id) && !marks_short(&t.title) && duration_fits(t.duration_ms, track_ms)
}

impl Worker {
    /// What the pipeline does when a search of `id` found nothing
    /// anywhere: «не нашёл», told as usual, and with the «Копия с YouTube»
    /// preference on, the YouTube copy saved by itself. Only the two
    /// conclusions of a search call this — the ones where every source that
    /// was asked answered and none turned the item away.
    pub(super) fn wish_nothing_found(&mut self, id: WishlistId) {
        self.no_torrent(id, true);
        self.yt_fallback_try(id);
    }

    /// Saves the item's track from YouTube Music by itself, quietly: the
    /// save runs like the manual one's (its run, its «История» line), only
    /// without toasts. Nothing is started when the preference is off, the
    /// item is not a plain track wish, it has a copy already, YouTube is
    /// off, another such save runs, or the track has no YouTube source.
    pub(super) fn yt_fallback_try(&mut self, id: WishlistId) {
        let pref = self.state.read().prefs.youtube_fallback;
        let Some(item) = self.lib.get_wishlist_item(id).ok().flatten() else { return };
        let has_copy = self.copy_quality(&item).is_some();
        let yt = self.service(Service::Youtube);
        if !gate(&item, pref, has_copy, yt.is_some(), self.yt_save_running()) {
            return;
        }
        let Some(yt) = yt else { return };
        let Some(track) = item.track_id else { return };
        let sources = self.lib.sources_of_track(track).unwrap_or_default();
        let Some((locator, clip)) = best_youtube_source(&sources) else { return };
        let Some(t) = self.lib.get_track(track).ok().flatten() else { return };
        // The raw title of the track: the title the searching uses has the
        // clip words stripped, the raw one does not.
        if marks_short(&t.title) {
            return;
        }
        let track_ms = t.duration_ms.map(u64::from);
        let refused = self.refused_videos();
        if refused.iter().any(|r| *r == locator) {
            return;
        }
        let known = self.streaming.known.get(&RemoteId::new(Service::Youtube, locator.as_str())).cloned();
        let dir = self.save_dir();
        let run = self.streaming.wish.runs.reserve();
        mark_fallback_run(run);
        let task = self.streaming.spawn(async move {
            let result = save_yt_copy(&yt, &locator, clip, known, track_ms, &refused, &dir).await;
            Msg::Wish(WishMsg::Saved { item: id, track, run, result })
        });
        self.set_item(id, |i| {
            i.status = WishlistStatus::Downloading;
            i.note = Some(note_code(WishNote::SavingYoutube));
        });
        self.streaming.wish.from.insert(id, "YouTube".into());
        let title = self.item_title(id);
        self.streaming.wish.runs.start(id, run, title, "YouTube".into(), None, None, Some(task));
        self.publish_wishlist();
        self.publish_running();
    }

    /// A save from YouTube runs already (its run, or its file the importer
    /// is taking): the fallback starts at most one at a time.
    fn yt_save_running(&self) -> bool {
        if !self.streaming.wish.saving.is_empty() {
            return true;
        }
        self.lib
            .list_wishlist_by_status(WishlistStatus::Downloading)
            .unwrap_or_default()
            .iter()
            .any(|i| {
                self.streaming.wish.runs.runs(i.id)
                    && self.streaming.wish.from.get(&i.id).is_some_and(|f| f == "YouTube")
            })
    }
}

/// Saves the audio of the track's best YouTube source into `dir`: the
/// video's own data is asked of the service when the app has not seen it
/// yet; when the best source is only a clip, its song counterpart is used,
/// and only when found. A video that is a short or a preview, or one the
/// user marked wrong before, is not saved: `Err` then, a quiet skip.
async fn save_yt_copy(
    yt: &Arc<dyn StreamService>,
    locator: &str,
    clip: bool,
    known: Option<StreamTrack>,
    track_ms: Option<u64>,
    refused: &[String],
    dir: &Path,
) -> Result<PathBuf, StreamError> {
    let video = match known {
        Some(v) => v,
        None => yt.track(locator).await?,
    };
    if !ok_to_save(&video, track_ms, refused) {
        return Err(StreamError::Unavailable);
    }
    let save_id = if clip {
        match yt.song_for_video(&video).await? {
            Some(song) if ok_to_save(&song, track_ms, refused) => song.id.id,
            _ => return Err(StreamError::Unavailable),
        }
    } else {
        video.id.id.clone()
    };
    yt.save_audio(&save_id, dir).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_core::model::Codec;
    use nikgob_core::streaming::FakeService;

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("nikgob-yt-fallback-{name}-{}", nikgob_library::now_ms()))
    }

    fn item(scope: WishlistScope, origin: WishlistOrigin) -> WishlistItem {
        let mut i = WishlistItem::new(WishlistStatus::NoTorrent, 1);
        i.scope = scope;
        i.origin = origin;
        i.track_id = Some(TrackId::new());
        i
    }

    /// The fallback runs only for a plain track wish on a genuine miss:
    /// the preference on, no copy yet, YouTube on, nothing saving already.
    #[test]
    fn the_fallback_runs_only_for_a_plain_track_miss() {
        let ok = item(WishlistScope::Track, WishlistOrigin::Like);
        assert!(gate(&ok, true, false, true, false));
        assert!(!gate(&ok, false, false, true, false), "the preference is off");
        assert!(!gate(&item(WishlistScope::Track, WishlistOrigin::Upgrade), true, false, true, false), "a better copy");
        assert!(!gate(&item(WishlistScope::Album, WishlistOrigin::Like), true, false, true, false), "an album item");
        let mut no_track = ok.clone();
        no_track.track_id = None;
        assert!(!gate(&no_track, true, false, true, false), "no track");
        assert!(!gate(&ok, true, true, true, false), "it has a copy already");
        assert!(!gate(&ok, true, false, false, false), "the YouTube service is off");
        assert!(!gate(&ok, true, false, true, true), "a save runs already");
    }

    /// A raw title that says a short or a preview is not saved; a word only
    /// inside another word is not one.
    #[test]
    fn a_short_or_preview_title_marks_the_video() {
        assert!(marks_short("Song #shorts"));
        assert!(marks_short("Song (Short)"));
        assert!(marks_short("Song 【MV short】"));
        assert!(marks_short("Song TV Size"));
        assert!(marks_short("Song - teaser"));
        assert!(marks_short("Preview of Song"));
        assert!(!marks_short("Shortage"));
        assert!(!marks_short("Группа крови"));
        assert!(!marks_short("Song (Official Video)"));
    }

    /// A video under a minute is saved only for a track about as short.
    #[test]
    fn a_short_video_fits_only_a_short_track() {
        assert!(duration_fits(Some(240_000), Some(245_000)));
        assert!(duration_fits(Some(45_000), Some(50_000)), "the track is as short");
        assert!(!duration_fits(Some(45_000), Some(240_000)));
        assert!(!duration_fits(Some(45_000), None), "the track's length unknown");
        assert!(duration_fits(None, Some(240_000)), "the video's length unknown");
    }

    fn yt_source(track: TrackId, score: i32, locator: &str) -> nikgob_core::model::Source {
        let mut s = nikgob_core::model::Source::new(track, SourceKind::Youtube, locator, Codec::Opus);
        s.quality_score = score;
        s
    }

    /// The best available YouTube source is the one taken; a clip only tells
    /// its song is to be asked.
    #[test]
    fn the_best_available_youtube_source_is_taken() {
        let t = TrackId::new();
        let mut trashed = yt_source(t, score::STREAM_YOUTUBE, "gone");
        trashed.trashed_at = Some(1);
        let mut down = yt_source(t, score::STREAM_YOUTUBE, "down");
        down.available = false;
        let local = nikgob_core::model::Source::new(t, SourceKind::LocalFile, "/x.flac", Codec::Flac);
        let clip = yt_source(t, score::STREAM_YOUTUBE_CLIP, "clip");
        let song = yt_source(t, score::STREAM_YOUTUBE, "song");
        assert_eq!(best_youtube_source(&[trashed, down, local.clone(), clip.clone(), song]), Some(("song".to_string(), false)));
        assert_eq!(best_youtube_source(&[clip]), Some(("clip".to_string(), true)), "a clip only: its song is asked");
        assert_eq!(best_youtube_source(&[local]), None, "no YouTube source");
        assert_eq!(best_youtube_source(&[]), None);
    }

    #[tokio::test]
    async fn a_song_source_is_saved_by_itself() {
        let yt: Arc<dyn StreamService> = Arc::new(FakeService::youtube().with_song("s", "Song", "Artist", "Album", 240));
        let dir = temp_dir("song");
        let path = save_yt_copy(&yt, "yt-x-s", false, None, Some(240_000), &[], &dir).await.unwrap();
        assert!(path.ends_with("yt-x-s.wav"), "{path:?}");
        assert!(std::fs::metadata(&path).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A clip is saved only as its song counterpart; a clip without one is
    /// skipped.
    #[tokio::test]
    async fn a_clip_is_saved_only_as_its_song() {
        let yt: Arc<dyn StreamService> = Arc::new(
            FakeService::youtube()
                .with_song("s", "Song", "Artist", "Album", 240)
                .with_video("v", "Artist - Song (Official Video)", "Artist", 245, Some("yt-x-s"))
                .with_video("v2", "Artist - Other (Official Video)", "Artist", 200, None),
        );
        let dir = temp_dir("clip");
        let path = save_yt_copy(&yt, "yt-video-v", true, None, Some(240_000), &[], &dir).await.unwrap();
        assert!(path.ends_with("yt-x-s.wav"), "the song, not the video: {path:?}");
        assert!(save_yt_copy(&yt, "yt-video-v2", true, None, Some(200_000), &[], &dir).await.is_err(), "no counterpart");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A short or preview video, a too-short one for a full-length track,
    /// and a video the user marked wrong before are not saved.
    #[tokio::test]
    async fn a_short_preview_or_refused_video_is_not_saved() {
        let yt: Arc<dyn StreamService> = Arc::new(
            FakeService::youtube()
                .with_song("s", "Song", "Artist", "Album", 240)
                .with_video("sh", "Artist - Song #shorts", "Artist", 30, Some("yt-x-s")),
        );
        let yt2: Arc<dyn StreamService> = Arc::new(FakeService::youtube().with_song("t", "Tiny", "Artist", "Album", 45));
        let dir = temp_dir("skip");
        assert!(save_yt_copy(&yt, "yt-video-sh", true, None, Some(240_000), &[], &dir).await.is_err(), "#shorts in the title");
        assert!(save_yt_copy(&yt2, "yt-x-t", false, None, Some(240_000), &[], &dir).await.is_err(), "under a minute for a full-length track");
        let path = save_yt_copy(&yt2, "yt-x-t", false, None, Some(50_000), &[], &dir).await.unwrap();
        assert!(path.ends_with("yt-x-t.wav"), "the track is as short: {path:?}");
        assert!(save_yt_copy(&yt2, "yt-x-t", false, None, Some(240_000), &["yt-x-t".to_owned()], &dir).await.is_err(), "marked wrong before");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
