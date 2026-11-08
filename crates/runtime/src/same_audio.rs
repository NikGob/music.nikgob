//! Files that hold another song's audio (docs/TODO.md 32).
//!
//! * The background check: one thread at the lowest priority hears each
//!   local file that has no stored print yet (its first 120 s only), one at
//!   a time with a rest between files (a longer one while music plays), and
//!   compares it with the files of its album and artist. Two files that
//!   sound the same under different titles become a pair for «Подтверди».
//!   Nothing is moved by it. It goes on where it stopped after a restart
//!   (what has a print is done).
//! * «Подтверди»: one card per pair, «В каком файле другая песня?» with the
//!   two songs; the chosen one is handled as the menu action below, «Всё
//!   верно» keeps both and the pair is not asked again.
//! * «В файле другая песня» on a local track ([`Command::WrongFile`]): the
//!   file goes to the Trash (see `nikgob_import::wrong_file`), the track is
//!   looked for again in «Хочу скачать», and the toast's «Вернуть» takes it
//!   all back.
//!
//! [`Command::WrongFile`]: nikgob_view::Command::WrongFile

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossbeam_channel::Sender;
use nikgob_core::model::{SourceId, TrackId};
use nikgob_import::quality::CheckError;
use nikgob_import::same_audio::{Heard, Judgement, Known, judge, print_file};
use nikgob_import::{WrongFile, mark_wrong_file, undo_wrong_file};
use nikgob_library::{AudioPrint, Library, PrintTodo, SameAudioOffer, SameSide};
use nikgob_view::{self as v, Command};
use parking_lot::{Condvar, Mutex};

use crate::Worker;
use crate::import::Internal;

/// Bit of the «Подтверди» ids of the pairs.
pub(crate) const SAME_AUDIO_ID: u64 = 1 << 44;
/// Rest between two files.
const REST: Duration = Duration::from_secs(1);
/// Rest between two files while music plays.
const REST_PLAYING: Duration = Duration::from_secs(5);
/// Files listed at a time.
const BATCH: usize = 64;
/// A shutdown waits this long for the file being heard.
const STOP_WAIT: Duration = Duration::from_secs(5);

/// From the check's thread and the actions to the worker.
pub(crate) enum Msg {
    /// New pairs were found.
    Found,
    /// «В файле другая песня» of `track` is done.
    Marked { track: TrackId, title: String, result: Result<WrongFile, String> },
    /// Its undo is done: whether the file came back.
    Undone { track: TrackId, result: Result<bool, String> },
}

#[derive(Default)]
struct Wake {
    kicked: bool,
}

struct Shared {
    lib: Library,
    root: PathBuf,
    tx: Sender<Internal>,
    stop: AtomicBool,
    playing: AtomicBool,
    wake: Mutex<Wake>,
    cond: Condvar,
    /// Without work, the library is looked at again this often.
    idle: Duration,
}

/// Absolute path of a source locator (relative ones are under the library).
fn file_path(root: &Path, locator: &str) -> PathBuf {
    let p = Path::new(locator);
    if p.is_absolute() { p.to_owned() } else { root.join(p) }
}

/// The pair `t`'s file makes with a file of its album or artist, if it
/// sounds like another song: `(that file, its track, how alike)`.
pub(crate) fn pair_of(lib: &Library, t: &PrintTodo, print: &[u32], duration_ms: Option<u32>) -> Option<(SourceId, TrackId, f64)> {
    let track = lib.get_track(t.track_id).ok().flatten()?;
    let near = lib.prints_near(t.release_id, Some(t.track_id), None).ok()?;
    let known: Vec<Known<'_>> = near
        .iter()
        .filter(|n| n.source_id != t.source_id)
        .map(|n| Known {
            source: n.source_id,
            track: n.track_id,
            title: &n.title,
            print: &n.print,
            duration_ms: n.duration_ms,
            // Which of two library files is wrong is the user's call.
            settled: false,
            same_release: t.release_id.is_some() && n.release_id == t.release_id,
        })
        .collect();
    let heard = Heard { title: &track.title, track: Some(track.id), print, duration_ms, expected_ms: None, own: &[] };
    match judge(&heard, &known) {
        Judgement::Fine => None,
        Judgement::Flag { source, track, similarity } | Judgement::Refuse { source, track, similarity } => {
            Some((source, track, similarity))
        }
    }
}

impl Shared {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Sleeps `d` unless stopped or kicked meanwhile.
    fn pause(&self, d: Duration) {
        let mut w = self.wake.lock();
        if w.kicked || self.stopped() {
            return;
        }
        let _ = self.cond.wait_for(&mut w, d);
    }

    fn rest(&self) {
        let d = if self.playing.load(Ordering::Relaxed) { REST_PLAYING } else { REST };
        let until = std::time::Instant::now() + d;
        let mut w = self.wake.lock();
        while !self.stopped() {
            if self.cond.wait_until(&mut w, until).timed_out() {
                break;
            }
        }
    }

    /// One look over the files without a print; true when one was heard.
    fn pass(&self, skip: &mut HashSet<SourceId>) -> bool {
        let todo = match self.lib.prints_todo(None, BATCH + skip.len()) {
            Ok(t) => t,
            Err(e) => {
                log::warn!("same audio: listing the files: {e}");
                return false;
            }
        };
        let (mut worked, mut found) = (false, false);
        for t in todo {
            if self.stopped() {
                break;
            }
            if skip.contains(&t.source_id) {
                continue;
            }
            worked = true;
            let path = file_path(&self.root, &t.locator);
            let row = match print_file(&path, &self.stop) {
                Ok((print, duration_ms)) => {
                    if let Some((source, track, similarity)) = pair_of(&self.lib, &t, &print, duration_ms) {
                        log::warn!(
                            "same audio: {} sounds like the file of {track} ({similarity:.2}), asked about",
                            path.display()
                        );
                        match self.lib.put_same_audio((t.source_id, t.track_id), (source, track), similarity) {
                            Ok(()) => found = true,
                            Err(e) => log::warn!("same audio: storing the pair of {}: {e}", path.display()),
                        }
                    }
                    AudioPrint {
                        source_id: t.source_id,
                        track_id: t.track_id,
                        print: Some(print),
                        duration_ms,
                        came_hash: None,
                        came_from: Vec::new(),
                        failed: None,
                    }
                }
                Err(CheckError::Stopped) => break,
                Err(CheckError::Unavailable(e)) => {
                    log::info!("same audio: skipping {}: {e}", path.display());
                    skip.insert(t.source_id);
                    continue;
                }
                Err(CheckError::Undecodable(e)) => {
                    log::info!("same audio: cannot hear {}: {e}", path.display());
                    AudioPrint {
                        source_id: t.source_id,
                        track_id: t.track_id,
                        print: None,
                        duration_ms: None,
                        came_hash: None,
                        came_from: Vec::new(),
                        failed: Some(e),
                    }
                }
            };
            if let Err(e) = self.lib.put_audio_print(&row) {
                log::warn!("same audio: storing the print of {}: {e}", path.display());
                skip.insert(t.source_id);
            }
            self.rest();
        }
        if found {
            let _ = self.tx.send(Internal::SameAudio(Msg::Found));
        }
        worked
    }

    fn run(self: &Arc<Self>, start: Duration, alive: Sender<()>) {
        crate::analysis::lower_priority();
        self.pause(start);
        match self.lib.drop_orphan_prints() {
            Ok(0) => {}
            Ok(n) => log::info!("same audio: {n} rows of files that are gone dropped"),
            Err(e) => log::warn!("same audio: dropping old prints: {e}"),
        }
        // Not tried again in this run (could not be opened).
        let mut skip = HashSet::new();
        while !self.stopped() {
            self.wake.lock().kicked = false;
            if !self.pass(&mut skip) {
                self.pause(self.idle);
            }
        }
        drop(alive);
    }
}

/// The background check and what «Подтверди» and the action need.
pub(crate) struct SameAudioJob {
    tx: Sender<Internal>,
    shared: Option<Arc<Shared>>,
    gone: Option<crossbeam_channel::Receiver<()>>,
    /// Stable ids of the cards, by their two files.
    card_ids: HashMap<(SourceId, SourceId), u64>,
    next_card: u64,
    /// The pairs shown, by card id.
    shown: HashMap<u64, SameAudioOffer>,
    /// Tracks whose file is being handled.
    busy: HashSet<TrackId>,
    /// What each action did, for its «Вернуть».
    undo: HashMap<TrackId, WrongFile>,
}

/// `m:ss` of a length.
fn clock(ms: u32) -> String {
    let s = ms / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

/// A song's row on a pair's card: its title, its length and format.
pub(crate) fn side_option(s: &SameSide) -> v::ConfirmOption {
    let mut detail: Vec<String> = Vec::new();
    if let Some(ms) = s.duration_ms {
        detail.push(clock(ms));
    }
    detail.push(nikgob_core::quality::label(&s.codec, s.bitrate, None));
    v::ConfirmOption { label: s.title.clone(), detail: detail.join(" · "), recommended: false }
}

/// The two files of a pair in the order the card shows them: the one that
/// came later first (more likely the wrong one), never preselected as
/// recommended.
pub(crate) fn card_sides(o: &SameAudioOffer) -> (&SameSide, &SameSide) {
    if o.b.added_at > o.a.added_at { (&o.b, &o.a) } else { (&o.a, &o.b) }
}

/// The «Подтверди» card of a pair.
pub(crate) fn pair_card(id: u64, o: &SameAudioOffer) -> v::Confirm {
    let (first, second) = card_sides(o);
    v::Confirm {
        id,
        title: o.album.clone(),
        origin: "Одинаковый звук".into(),
        question: "В каком файле другая песня?".into(),
        options: vec![side_option(first), side_option(second)],
        skip_label: "Всё верно".into(),
        accept_label: "В файле другая песня".into(),
        pick: None,
    }
}

impl SameAudioJob {
    pub(crate) fn new(tx: Sender<Internal>) -> Self {
        Self {
            tx,
            shared: None,
            gone: None,
            card_ids: HashMap::new(),
            next_card: 0,
            shown: HashMap::new(),
            busy: HashSet::new(),
            undo: HashMap::new(),
        }
    }

    /// Starts hearing the files after `start`; `idle` is how often the
    /// library is looked at again when there is nothing to do.
    pub(crate) fn start(&mut self, lib: Library, root: PathBuf, start: Duration, idle: Duration) {
        if self.shared.is_some() {
            return;
        }
        let shared = Arc::new(Shared {
            lib,
            root,
            tx: self.tx.clone(),
            stop: AtomicBool::new(false),
            playing: AtomicBool::new(false),
            wake: Mutex::new(Wake::default()),
            cond: Condvar::new(),
            idle,
        });
        let (alive, gone) = crossbeam_channel::bounded::<()>(0);
        let s = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("nikgob-same-audio".into())
            .spawn(move || s.run(start, alive));
        match spawned {
            Ok(_) => {
                self.shared = Some(shared);
                self.gone = Some(gone);
            }
            Err(e) => log::warn!("same audio: cannot start: {e}"),
        }
    }

    pub(crate) fn set_playing(&self, playing: bool) {
        if let Some(s) = &self.shared {
            s.playing.store(playing, Ordering::Relaxed);
        }
    }

    /// New files may be waiting (an import landed).
    pub(crate) fn kick(&self) {
        if let Some(s) = &self.shared {
            s.wake.lock().kicked = true;
            s.cond.notify_all();
        }
    }

    /// Stops the thread: a decode stops at its next chunk. Waits up to
    /// [`STOP_WAIT`].
    pub(crate) fn shutdown(&mut self) {
        let Some(s) = self.shared.take() else { return };
        s.stop.store(true, Ordering::Relaxed);
        {
            let _w = s.wake.lock();
            s.cond.notify_all();
        }
        if let Some(gone) = self.gone.take()
            && let Err(crossbeam_channel::RecvTimeoutError::Timeout) = gone.recv_timeout(STOP_WAIT)
        {
            log::warn!("same audio: the check is still busy at shutdown");
        }
    }
}

impl Drop for SameAudioJob {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Worker {
    pub(crate) fn start_same_audio(&mut self) {
        let root = self.res.root().to_owned();
        let (start, idle) = (self.streaming.schedule.analysis_start, self.streaming.schedule.analysis_idle);
        self.same_audio.start(self.lib.clone(), root, start, idle);
        self.publish_same_audio();
    }

    /// «Подтверди»: a card per pair of playing files that sound the same
    /// under different titles (none while a file of theirs is handled).
    pub(crate) fn publish_same_audio(&mut self) {
        let offers = if self.import.is_some() { self.lib.same_audio_offers().unwrap_or_default() } else { Vec::new() };
        let j = &mut self.same_audio;
        j.shown.clear();
        let mut confirms = Vec::new();
        for o in offers {
            if j.busy.contains(&o.a.track_id) || j.busy.contains(&o.b.track_id) {
                continue;
            }
            let key = (o.a.source_id, o.b.source_id);
            let id = match j.card_ids.get(&key) {
                Some(id) => *id,
                None => {
                    j.next_card += 1;
                    let id = SAME_AUDIO_ID | j.next_card;
                    j.card_ids.insert(key, id);
                    id
                }
            };
            confirms.push(pair_card(id, &o));
            j.shown.insert(id, o);
        }
        self.publish(|s| {
            s.downloads.confirm.retain(|c| c.id & SAME_AUDIO_ID == 0);
            s.downloads.confirm.extend(confirms);
        });
    }

    /// The answer on a pair's card: option `i` holds another song (the
    /// action below, with the other one as the song it holds); none: both
    /// are right, and the pair is not asked again.
    pub(crate) fn same_audio_choice(&mut self, id: u64, option: Option<usize>) {
        let Some(o) = self.same_audio.shown.get(&id).cloned() else {
            log::info!("same audio: card {id} is no longer shown");
            self.publish_same_audio();
            return;
        };
        let (first, second) = card_sides(&o);
        match option {
            None => {
                if let Err(e) = self.lib.keep_same_audio(o.a.source_id, o.b.source_id) {
                    log::warn!("same audio: keeping the pair: {e}");
                }
                log::info!("same audio: «{}» and «{}» are both right", o.a.title, o.b.title);
                self.publish_same_audio();
            }
            Some(i) => {
                let (wrong, right) = if i == 0 { (first, second) } else { (second, first) };
                let (wrong, right) = (wrong.track_id, right.track_id);
                self.wrong_file(wrong, Some(right));
            }
        }
    }

    /// «В файле другая песня» of `track` (`sounds_like`: the song its file
    /// holds, when known).
    pub(crate) fn wrong_file(&mut self, track: TrackId, sounds_like: Option<TrackId>) {
        if self.same_audio.busy.contains(&track) {
            return;
        }
        let Some(svc) = &self.import else {
            self.toast("Импорт недоступен", None);
            return;
        };
        let title = self.lib.get_track(track).ok().flatten().map(|t| t.title).unwrap_or_default();
        let (lib, cfg, tx) = (self.lib.clone(), svc.config().clone(), svc.internal());
        let t = title.clone();
        svc.spawn_blocking(move || {
            let result = mark_wrong_file(&lib, &cfg, track, sounds_like).map_err(|e| e.to_string());
            let _ = tx.send(Internal::SameAudio(Msg::Marked { track, title: t, result }));
        });
        self.same_audio.busy.insert(track);
        self.publish_same_audio();
    }

    /// «Вернуть» on the toast of [`Self::wrong_file`].
    pub(crate) fn undo_wrong_file(&mut self, track: TrackId) {
        let Some(w) = self.same_audio.undo.remove(&track) else { return };
        let Some(svc) = &self.import else { return };
        let (lib, tx) = (self.lib.clone(), svc.internal());
        svc.spawn_blocking(move || {
            let result = undo_wrong_file(&lib, &w).map_err(|e| e.to_string());
            let _ = tx.send(Internal::SameAudio(Msg::Undone { track, result }));
        });
        self.same_audio.busy.insert(track);
    }

    pub(crate) fn same_audio_msg(&mut self, m: Msg) {
        match m {
            Msg::Found => self.publish_same_audio(),
            Msg::Marked { track, title, result } => {
                self.same_audio.busy.remove(&track);
                match result {
                    Ok(w) => {
                        self.same_audio.undo.insert(track, w);
                        self.reload_after_import();
                        // The track is looked for again, without that file.
                        self.wish_again(track);
                        self.toast(
                            format!("«{title}»: файл в Корзине, ищу заново"),
                            Some(("Вернуть".into(), Command::UndoWrongFile(track))),
                        );
                    }
                    Err(e) => {
                        log::warn!("same audio: «{title}» stays: {e}");
                        self.toast(format!("Не получилось убрать файл «{title}»"), None);
                    }
                }
                self.publish_same_audio();
            }
            Msg::Undone { track, result } => {
                self.same_audio.busy.remove(&track);
                match result {
                    Ok(true) => {
                        self.reload_after_import();
                        // The file is back: what looks for it is done.
                        self.wish_imported(track, Path::new(""));
                        self.toast("Файл вернулся", None);
                    }
                    Ok(false) => {
                        self.reload_after_import();
                        self.toast("Файл не вернулся: его место занято", None);
                    }
                    Err(e) => {
                        log::warn!("same audio: undo of {track}: {e}");
                        self.toast("Не получилось вернуть файл", None);
                    }
                }
                self.publish_same_audio();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use nikgob_core::model::Codec;

    use super::*;

    fn side(title: &str, added_at: i64) -> SameSide {
        SameSide {
            source_id: SourceId::new(),
            track_id: TrackId::new(),
            title: title.into(),
            duration_ms: Some(160_480),
            codec: Codec::Opus,
            bitrate: Some(160),
            added_at,
        }
    }

    /// A card per pair: the file that came later first, its length and
    /// format, nothing recommended; the buttons say what they do.
    #[test]
    fn a_pair_is_asked_about_with_the_later_file_first() {
        let o = SameAudioOffer {
            a: side("Тмстс", 1_000),
            b: side("Сфотографирую", 2_000),
            similarity: 0.97,
            release: None,
            album: "8 способов как бросить".into(),
        };
        let card = pair_card(SAME_AUDIO_ID | 1, &o);
        assert_eq!(card.title, "8 способов как бросить");
        assert_eq!(card.options.len(), 2);
        assert_eq!(card.options[0].label, "Сфотографирую");
        assert_eq!(card.options[1].label, "Тмстс");
        assert_eq!(card.options[0].detail, "2:40 · Opus 160");
        assert!(card.options.iter().all(|o| !o.recommended), "never accepted with the recommended ones");
        assert_eq!((card.accept_label.as_str(), card.skip_label.as_str()), ("В файле другая песня", "Всё верно"));
        assert!(card.pick.is_none());
        let (first, _) = card_sides(&o);
        assert_eq!(first.title, "Сфотографирую");
    }

    /// «Загрузки → История» says which file held another song, by title.
    #[test]
    fn a_refused_file_is_named_in_the_history() {
        use crate::import::failed_words;
        assert_eq!(failed_words(0, &[]), None);
        assert_eq!(failed_words(2, &[]).as_deref(), Some("Не удалось: 2"));
        let wrong = vec!["Сфотографирую".to_owned()];
        assert_eq!(failed_words(1, &wrong).as_deref(), Some("В файле другая песня: «Сфотографирую»"));
        assert_eq!(
            failed_words(3, &wrong).as_deref(),
            Some("В файле другая песня: «Сфотографирую» · не удалось ещё: 2")
        );
    }
}
