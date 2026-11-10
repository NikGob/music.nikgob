//! Files that hold another song's audio (docs/TODO.md 32).
//!
//! * The background check: one thread at the lowest priority hears each
//!   local file that has no stored print yet (its first 120 s only), one at
//!   a time, paced by [`crate::quiet`] (a rest five times its work, none
//!   while a game runs) with a rest between files on top (a longer one
//!   while music plays), and
//!   compares it with the files of its album and artist. Two files that
//!   sound the same under different titles become a pair for «Подтверди».
//!   Nothing is moved by it. It goes on where it stopped after a restart
//!   (what has a print is done).
//! * «Подтверди»: one card per pair, «В каком файле другая песня?» with the
//!   two songs; the chosen one is handled as the menu action below, «Всё
//!   верно» keeps both and the pair is not asked again. A pair on two
//!   albums is asked «Одна и та же песня?» instead: «Одна песня» links the
//!   two tracks into one recording, «Разные песни» keeps them apart for
//!   good.
//! * One song on two albums (see `nikgob_library` `recordings`): the same
//!   thread links the tracks of one recording, rows only — the same ISRC,
//!   recording MBID or AcoustID at its start and after each import, and the
//!   stored prints not yet compared (near bit-exact audio under one title
//!   on another album), resumable like the rest
//!   ([`nikgob_import::same_audio::same_recording`]).
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
use nikgob_import::same_audio::{Heard, Judgement, Known, Recording, Take, judge, print_file_paced, same_recording};
use nikgob_import::{WrongFile, mark_wrong_file, undo_wrong_file};
use nikgob_library::{AudioPrint, Library, PrintToLink, PrintTodo, RecordingSide, SameAudioOffer, SameSide};
use nikgob_view::{self as v, Command};
use parking_lot::{Condvar, Mutex};

use crate::Worker;
use crate::import::Internal;
use crate::quiet::Pacer;

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
    /// Tracks of one song on two albums were linked (how many links).
    Linked(usize),
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
    /// The ids of the library are looked at for one recording on two
    /// albums (at the start, after an import).
    ids_due: AtomicBool,
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

/// A track as the rule of one recording sees it.
fn take_of<'a>(s: &'a RecordingSide, print: Option<&'a [u32]>) -> Take<'a> {
    Take {
        title: &s.title,
        release: s.release_id,
        duration_ms: s.duration_ms,
        isrc: s.isrc.as_deref(),
        recording_mbid: s.recording_mbid.as_deref(),
        acoustid: s.acoustid.as_deref(),
        print,
    }
}

/// Links the tracks on two albums with the same ISRC, recording MBID or
/// AcoustID that the rule finds one recording: `track`'s only, or all.
/// Returns how many links were made.
pub(crate) fn link_by_ids(lib: &Library, track: Option<TrackId>) -> usize {
    let pairs = match lib.recording_id_pairs(track) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("one recording: reading the ids: {e}");
            return 0;
        }
    };
    let mut linked = 0;
    for (a, b) in &pairs {
        let print = |t: TrackId| lib.prints_of_track(t).ok().and_then(|v| v.into_iter().next());
        let (pa, pb) = (print(a.track_id), print(b.track_id));
        let Recording::Same(how) = same_recording(&take_of(a, pa.as_deref()), &take_of(b, pb.as_deref())) else { continue };
        match lib.link_recording(a.track_id, b.track_id, how.key()) {
            Ok(true) => {
                log::info!("one recording: «{}» and «{}» on two albums ({})", a.title, b.title, how.key());
                linked += 1;
            }
            Ok(false) => {}
            Err(e) => log::warn!("one recording: linking «{}»: {e}", a.title),
        }
    }
    linked
}

/// The tracks on other albums whose file is the same audio as `p`'s under
/// one title (or a version of it) and as long: one recording. The same
/// audio under two titles is the check's pair for «Подтверди», not this.
pub(crate) fn audio_twins(lib: &Library, p: &PrintToLink) -> Vec<(TrackId, nikgob_import::same_audio::How)> {
    use nikgob_import::same_audio::{same_title, versions_of_one_song};
    let Some(me) = lib.recording_side(p.track_id).ok().flatten() else { return Vec::new() };
    let Ok(near) = lib.prints_near(None, Some(p.track_id), None) else { return Vec::new() };
    let mine = Take { duration_ms: p.duration_ms.or(me.duration_ms), ..take_of(&me, Some(&p.print)) };
    let mut out: Vec<(TrackId, nikgob_import::same_audio::How)> = Vec::new();
    for n in &near {
        if n.track_id == p.track_id || out.iter().any(|(t, _)| *t == n.track_id) {
            continue;
        }
        if me.release_id.is_some() && n.release_id == me.release_id {
            continue;
        }
        if !same_title(&me.title, &n.title) && !versions_of_one_song(&me.title, &n.title) {
            continue;
        }
        let theirs = Take { title: &n.title, release: n.release_id, duration_ms: n.duration_ms, print: Some(&n.print), ..Take::default() };
        if let Recording::Same(how) = same_recording(&mine, &theirs) {
            out.push((n.track_id, how));
        }
    }
    out
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

    /// The open pairs judged again by today's rule from their stored prints:
    /// those it no longer finds (a soundtrack's leitmotif, two versions of
    /// one song) are forgotten.
    fn recheck_offers(&self) {
        let Ok(offers) = self.lib.same_audio_offers() else { return };
        let mut dropped = 0;
        for o in &offers {
            let print = |s: SourceId| self.lib.audio_print(s).ok().flatten().and_then(|p| p.print);
            let still = match (print(o.a.source_id), print(o.b.source_id)) {
                (Some(a), Some(b)) => {
                    !nikgob_import::same_audio::versions_of_one_song(&o.a.title, &o.b.title)
                        && nikgob_import::same_audio::overlap(&a, &b) >= nikgob_import::same_audio::SAME_SONG
                }
                _ => true,
            };
            if !still && self.lib.drop_same_audio(o.a.source_id, o.b.source_id).is_ok() {
                dropped += 1;
            }
        }
        if dropped > 0 {
            log::info!("same audio: {dropped} of {} open pairs are not the same audio by the stricter rule", offers.len());
            let _ = self.tx.send(Internal::SameAudio(Msg::Found));
        }
    }

    /// One look over the files without a print; true when one was heard.
    fn pass(&self, skip: &mut HashSet<SourceId>, pacer: &mut Pacer<'_>) -> bool {
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
            if !pacer.begin() {
                break;
            }
            let row = match print_file_paced(&path, &self.stop, &mut || {
                pacer.step();
            }) {
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
            pacer.rest();
            self.rest();
        }
        if found {
            let _ = self.tx.send(Internal::SameAudio(Msg::Found));
        }
        worked
    }

    /// One look for one song on two albums: the ids when due, then the
    /// stored prints not compared yet (a batch, paced). True when prints
    /// were compared.
    fn link_pass(&self, pacer: &mut Pacer<'_>) -> bool {
        let mut linked = 0;
        if self.ids_due.swap(false, Ordering::Relaxed) {
            linked += link_by_ids(&self.lib, None);
        }
        let todo = match self.lib.prints_to_link(BATCH) {
            Ok(t) => t,
            Err(e) => {
                log::warn!("one recording: listing the prints: {e}");
                Vec::new()
            }
        };
        let worked = !todo.is_empty();
        for p in &todo {
            if self.stopped() || !pacer.begin() {
                break;
            }
            for (other, how) in audio_twins(&self.lib, p) {
                match self.lib.link_recording(p.track_id, other, how.key()) {
                    Ok(true) => {
                        log::info!("one recording: the file of {} is the song of {other} on another album", p.track_id);
                        linked += 1;
                    }
                    Ok(false) => {}
                    Err(e) => log::warn!("one recording: linking {}: {e}", p.track_id),
                }
            }
            if let Err(e) = self.lib.mark_print_linked(p.source_id) {
                log::warn!("one recording: marking the print of {}: {e}", p.track_id);
                break;
            }
            pacer.rest();
        }
        if linked > 0 {
            let _ = self.tx.send(Internal::SameAudio(Msg::Linked(linked)));
        }
        worked
    }

    fn run(self: &Arc<Self>, start: Duration, alive: Sender<()>) {
        crate::quiet::lower_priority();
        self.pause(start);
        match self.lib.drop_orphan_prints() {
            Ok(0) => {}
            Ok(n) => log::info!("same audio: {n} rows of files that are gone dropped"),
            Err(e) => log::warn!("same audio: dropping old prints: {e}"),
        }
        self.recheck_offers();
        let mut pacer = Pacer::new(&self.stop);
        // Not tried again in this run (could not be opened).
        let mut skip = HashSet::new();
        while !self.stopped() {
            self.wake.lock().kicked = false;
            let heard = self.pass(&mut skip, &mut pacer);
            let compared = !self.stopped() && self.link_pass(&mut pacer);
            if !heard && !compared {
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

/// A song's row on the card of a pair on two albums: its title, its album,
/// length and format.
pub(crate) fn album_option(s: &SameSide) -> v::ConfirmOption {
    let mut o = side_option(s);
    if !s.album.is_empty() {
        o.detail = format!("{} · {}", s.album, o.detail);
    }
    o
}

/// The two files of the pair are on two albums (or loose): one recording
/// or two songs, not a wrong file.
pub(crate) fn on_two_albums(o: &SameAudioOffer) -> bool {
    !o.a.same_release(&o.b)
}

/// The «Подтверди» card of a pair: on one album, which file holds the
/// other song; on two albums, whether the two are one song.
pub(crate) fn pair_card(id: u64, o: &SameAudioOffer) -> v::Confirm {
    let (first, second) = card_sides(o);
    if on_two_albums(o) {
        return v::Confirm {
            id,
            title: o.artist.clone(),
            origin: String::new(),
            question: "Одна и та же песня?".into(),
            options: vec![album_option(first), album_option(second)],
            skip_label: "Разные песни".into(),
            accept_label: "Одна песня".into(),
            pick: None,
            yes_no: true,
        };
    }
    v::Confirm {
        id,
        title: o.album.clone(),
        origin: String::new(),
        question: "В каком файле другая песня?".into(),
        options: vec![side_option(first), side_option(second)],
        skip_label: "Всё верно".into(),
        accept_label: "В файле другая песня".into(),
        pick: None,
        yes_no: false,
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
            ids_due: AtomicBool::new(true),
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
            s.ids_due.store(true, Ordering::Relaxed);
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
    /// are right, and the pair is not asked again. On two albums: an
    /// option is «Одна песня» (the tracks become one recording), none is
    /// «Разные песни» (never linked).
    pub(crate) fn same_audio_choice(&mut self, id: u64, option: Option<usize>) {
        let Some(o) = self.same_audio.shown.get(&id).cloned() else {
            log::info!("same audio: card {id} is no longer shown");
            self.publish_same_audio();
            return;
        };
        let (first, second) = card_sides(&o);
        if on_two_albums(&o) {
            if let Err(e) = self.lib.keep_same_audio(o.a.source_id, o.b.source_id) {
                log::warn!("same audio: keeping the pair: {e}");
            }
            let (a, b) = (o.a.track_id, o.b.track_id);
            if option.is_some() {
                match self.lib.link_recording(a, b, "user") {
                    Ok(_) => log::info!("one recording: «{}» and «{}» are one song", o.a.title, o.b.title),
                    Err(e) => log::warn!("one recording: linking «{}»: {e}", o.a.title),
                }
                self.recordings_changed();
            } else {
                if let Err(e) = self.lib.set_recording_apart(a, b) {
                    log::warn!("one recording: «{}» apart: {e}", o.a.title);
                }
                log::info!("one recording: «{}» and «{}» are different songs", o.a.title, o.b.title);
            }
            self.publish_same_audio();
            return;
        }
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
        let Some(svc) = &self.import else { return };
        let Some(w) = self.same_audio.undo.remove(&track) else { return };
        let (lib, tx) = (self.lib.clone(), svc.internal());
        svc.spawn_blocking(move || {
            let result = undo_wrong_file(&lib, &w).map_err(|e| e.to_string());
            let _ = tx.send(Internal::SameAudio(Msg::Undone { track, result }));
        });
        self.same_audio.busy.insert(track);
    }

    /// Tracks were linked into one recording, or taken out of one: the
    /// menus' «Есть и на альбоме», the hearts, «Любимые» and the
    /// recommendations read the library again.
    pub(crate) fn recordings_changed(&mut self) {
        self.res.invalidate();
        self.publish_now_playing();
        self.load_library();
        self.load_home();
        let (release, artist, playlist) = {
            let s = self.state.read();
            (s.release.as_ref().map(|r| r.card.id), s.artist.as_ref().map(|a| a.card.id), s.playlist.as_ref().map(|p| p.card.id))
        };
        if let Some(id) = release {
            self.load_release(id);
        }
        if let Some(id) = artist {
            self.load_artist(id);
        }
        if let Some(id) = playlist {
            self.load_playlist(id);
        }
        self.publish_same_audio();
    }

    /// The other album of each track of one song on two albums, for its
    /// menu («Есть и на альбоме «…»»).
    pub(crate) fn publish_recordings(&mut self) {
        let albums = self.lib.recording_albums().unwrap_or_default();
        if *self.state.read().also_on == albums {
            return;
        }
        let albums = Arc::new(albums);
        self.publish(|s| s.also_on = albums);
    }

    /// «Разные песни» in a track's menu: it leaves its recording, and is
    /// never linked with those tracks again.
    pub(crate) fn recording_apart(&mut self, track: TrackId) {
        match self.lib.unlink_recording(track) {
            Ok(others) if !others.is_empty() => {
                log::info!("one recording: {track} is another song than {} tracks", others.len());
                self.recordings_changed();
            }
            Ok(_) => {}
            Err(e) => log::warn!("one recording: taking {track} out: {e}"),
        }
    }

    pub(crate) fn same_audio_msg(&mut self, m: Msg) {
        match m {
            Msg::Found => self.publish_same_audio(),
            Msg::Linked(n) => {
                log::info!("one recording: {n} links");
                self.recordings_changed();
            }
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
                        // The file is back: what looks for it stops.
                        self.wish_file_back(track);
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
            release_id: None,
            album: String::new(),
        }
    }

    /// A card per pair: the file that came later first, its length and
    /// format, nothing recommended; the buttons say what they do.
    #[test]
    fn a_pair_is_asked_about_with_the_later_file_first() {
        let album = Some(nikgob_core::model::ReleaseId::new());
        let o = SameAudioOffer {
            a: SameSide { release_id: album, ..side("Тмстс", 1_000) },
            b: SameSide { release_id: album, ..side("Сфотографирую", 2_000) },
            similarity: 0.97,
            release: album,
            album: "8 способов как бросить".into(),
            artist: "Пошлая Молли".into(),
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
        assert!(!card.yes_no);
        let (first, _) = card_sides(&o);
        assert_eq!(first.title, "Сфотографирую");
    }

    /// The same audio on two albums under two titles: «Одна и та же
    /// песня?», each song with its album, «Одна песня» / «Разные песни».
    #[test]
    fn a_pair_on_two_albums_asks_whether_it_is_one_song() {
        let (ost, special) = (nikgob_core::model::ReleaseId::new(), nikgob_core::model::ReleaseId::new());
        let o = SameAudioOffer {
            a: SameSide { release_id: Some(ost), album: "OST".into(), duration_ms: Some(276_900), ..side("Oración", 1_000) },
            b: SameSide { release_id: Some(special), album: "Special Edition".into(), duration_ms: Some(277_600), ..side("オラシオン", 2_000) },
            similarity: 1.0,
            release: Some(ost),
            album: "OST".into(),
            artist: "Joe Hisaishi".into(),
        };
        assert!(on_two_albums(&o));
        let card = pair_card(SAME_AUDIO_ID | 2, &o);
        assert_eq!(card.question, "Одна и та же песня?");
        assert_eq!(card.title, "Joe Hisaishi");
        assert!(card.yes_no);
        assert_eq!((card.accept_label.as_str(), card.skip_label.as_str()), ("Одна песня", "Разные песни"));
        assert_eq!(card.options[0].label, "オラシオン");
        assert_eq!(card.options[0].detail, "Special Edition · 4:37 · Opus 160");
        assert_eq!(card.options[1].detail, "OST · 4:36 · Opus 160");
        assert!(card.options.iter().all(|o| !o.recommended));
        // Two loose tracks are on no one album either.
        let loose = SameAudioOffer { a: side("A", 1), b: side("B", 2), ..o };
        assert!(on_two_albums(&loose));
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
