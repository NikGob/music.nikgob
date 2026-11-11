//! The home page, built off the player's loop.
//!
//! The page used to be built on the worker's thread: [`Command::LoadHome`]
//! and every arrival of the recommendations held the loop for most of a
//! second (several on a long history), and no click, playback event or
//! command was answered meanwhile. Now the worker only gathers what a
//! build needs ([`Job`]: the visit, owned copies of the recommendations'
//! picks, the last pass's discoveries) and hands it to one builder thread
//! with a resolver of its own ([`crate::Resolver::fork`]). The page comes
//! back finished ([`Done`]) and the worker only swaps it in.
//!
//! * **Newest wins.** Jobs that wait while one is built are one build:
//!   the last one's inputs, the whole page if any of them asked for it.
//!   Pages come back in the order asked; one is never older than the one
//!   shown before it.
//! * **Nothing lost in between.** The worker changes the shown page itself
//!   at once (a heart, a hidden mix, a larger cover, an artist's photo):
//!   such a change is kept with the input generation it came at while
//!   builds are on their way ([`Worker::patch_home`]) and made again on
//!   every page built of older inputs before it is shown. A like, a
//!   dislike, a play or a library edit moves the generation on too
//!   ([`Worker::home_input_changed`]): a page built of older inputs is
//!   shown, then built again, unless a newer one is on its way already.
//! * **The last page at the start.** The page shown is kept beside the
//!   prefs (`home.json`, at most every [`KEEP_EVERY`] and when the app
//!   closes, written on the builder's thread); the next start shows it at
//!   once while its own first page is built.
//!
//! [`Command::LoadHome`]: nikgob_view::Command::LoadHome

use std::any::Any;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use nikgob_view as v;

use crate::Worker;
use crate::convert::Resolver;
use crate::home_v2::Visit;
use crate::recs::{DiscoverInput, HomeParts, RecsFill};

/// A build of the whole page this long is logged with its parts (shorter
/// ones in the debug log).
const SLOW_BUILD: Duration = Duration::from_millis(1000);
/// The page shown is kept for the next start at most this often.
const KEEP_EVERY: Duration = Duration::from_secs(20);
/// The kept page's format: one of another is not read.
const KEPT_VERSION: u32 = 1;
/// The kept page's file, beside the prefs.
const KEPT_FILE: &str = "home.json";
/// «Слушать снова» gets this many more songs at its row's end.
const AGAIN_MORE: usize = 12;

/// What the rows of the page were asked for past their own length (each
/// scrolled to its end, [`v::Command::HomeMore`]), for this run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct More {
    /// Songs of «Слушать снова».
    pub(crate) again: usize,
    /// Songs of «Новое для тебя».
    pub(crate) new: usize,
}

impl More {
    /// The row `row` was scrolled to its end showing `shown` of the
    /// `source` its source has (`None`: it does not know before it draws):
    /// true when that asks for more of it now. A row that shows less than
    /// it was asked for has no more.
    fn ask(&mut self, row: v::HomeRow, shown: usize, source: Option<usize>) -> bool {
        let (field, base, step) = match row {
            v::HomeRow::ListenAgain => (&mut self.again, crate::home_v2::LISTEN_AGAIN, AGAIN_MORE),
            v::HomeRow::New => (&mut self.new, nikgob_recs::discover::DISCOVERIES, crate::recs::HOME_NEW_MORE),
            v::HomeRow::Mixes => return false,
        };
        let asked = base + *field;
        if shown < asked || source.is_some_and(|n| n <= asked) {
            return false;
        }
        *field += step;
        true
    }
}

/// What a build of the page is asked for, gathered on the worker.
pub(crate) struct Job {
    /// Its number: pages come back in this order.
    seq: u64,
    /// The input generation it was gathered at ([`HomeBuild::generation`]).
    generation: u64,
    /// The whole page; else only the recommendations' parts on the page as
    /// it is.
    full: bool,
    /// The recommendations were built when it was asked (before the run's
    /// first build the page keeps the mixes it shows: the last run's).
    mixes: bool,
    visit: Visit,
    parts: HomeParts,
    discover: DiscoverInput,
}

enum ToBuilder {
    Job(Box<Job>),
    /// Something to let go of off the loop ([`Worker::drop_off_loop`]).
    Drop(Box<dyn Any + Send>),
    /// The page to keep for the next start, and its file.
    Keep(Box<v::HomeView>, PathBuf),
}

/// The kept page as written…
#[derive(serde::Serialize)]
struct KeptRef<'a> {
    version: u32,
    page: &'a v::HomeView,
}

/// …and as read.
#[derive(serde::Deserialize)]
struct Kept {
    version: u32,
    page: v::HomeView,
}

/// The page the last run kept in `path`, if it reads.
fn read_kept(path: &Path) -> Option<v::HomeView> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice::<Kept>(&bytes) {
        Ok(k) if k.version == KEPT_VERSION => Some(k.page),
        Ok(_) => None,
        Err(e) => {
            log::debug!("home: the kept page does not read: {e}");
            None
        }
    }
}

/// Keeps `page` in `path` (written aside and renamed: never half a file).
fn write_kept(page: &v::HomeView, path: &Path) {
    let json = match serde_json::to_vec(&KeptRef { version: KEPT_VERSION, page }) {
        Ok(j) => j,
        Err(e) => {
            log::warn!("home: the page cannot be kept: {e}");
            return;
        }
    };
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, json).and_then(|()| std::fs::rename(&tmp, path)) {
        log::warn!("home: the page cannot be kept: {e}");
        let _ = std::fs::remove_file(&tmp);
    }
}

/// A finished build.
pub(crate) struct Done {
    seq: u64,
    generation: u64,
    full: bool,
    mixes: bool,
    /// `None` when the build failed (logged).
    page: Option<Page>,
    /// The discoveries on it ([`Worker::discover_shown`]).
    shown: Vec<String>,
}

enum Page {
    Whole(v::HomeView),
    Parts(RecsFill),
}

/// A change of the shown page made by the worker itself.
type Patch = Box<dyn Fn(&mut v::HomeView)>;

/// The worker's end of the builder.
pub(crate) struct HomeBuild {
    jobs: Sender<ToBuilder>,
    /// Where pages come; the worker's loop listens.
    pub(crate) done: Receiver<Done>,
    /// The builder's thread did not start: pages are built on the loop.
    here: Option<Resolver>,
    /// The last job asked for and the last one whose page came.
    asked: u64,
    came: u64,
    /// The input generation (see the module docs).
    generation: u64,
    /// More of a row asked for.
    pub(crate) more: More,
    /// The changes of the shown page made while builds were on their way,
    /// each with the generation it came at.
    patches: Vec<(u64, Patch)>,
    /// Where the page is kept for the next start (`None`: a run that keeps
    /// nothing, a test's), when it was last, and whether a newer one waits.
    keep: Option<PathBuf>,
    kept_at: Option<Instant>,
    keep_due: bool,
}

impl HomeBuild {
    /// Starts the builder's thread with a fork of the worker's resolver;
    /// the page is kept beside `prefs` (the prefs' file) when there is one.
    pub(crate) fn start(res: &Resolver, prefs: Option<&Path>) -> Self {
        let (jobs, inbox) = crossbeam_channel::unbounded::<ToBuilder>();
        let (out, done) = crossbeam_channel::unbounded::<Done>();
        let fork = res.fork();
        let spawned = std::thread::Builder::new().name("nikgob-home".into()).spawn(move || run(&inbox, &out, fork));
        let here = match spawned {
            Ok(_) => None,
            Err(e) => {
                log::error!("home: its builder did not start, the page is built on the loop: {e}");
                Some(res.fork())
            }
        };
        Self {
            jobs,
            done,
            here,
            asked: 0,
            came: 0,
            generation: 0,
            more: More::default(),
            patches: Vec::new(),
            keep: prefs.map(|p| p.with_file_name(KEPT_FILE)),
            kept_at: None,
            keep_due: false,
        }
    }

    /// A page is on its way.
    pub(crate) fn busy(&self) -> bool {
        self.came < self.asked
    }
}

/// The builder's loop: the jobs that wait are one build (newest wins).
fn run(inbox: &Receiver<ToBuilder>, out: &Sender<Done>, mut res: Resolver) {
    while let Ok(first) = inbox.recv() {
        let mut job = None;
        take(first, &mut job);
        while let Ok(more) = inbox.try_recv() {
            take(more, &mut job);
        }
        let Some(job) = job else { continue };
        let job = *job;
        let (seq, generation, full, mixes) = (job.seq, job.generation, job.full, job.mixes);
        let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build(&mut res, job))).unwrap_or_else(|_| {
            log::error!("home: the build of page {seq} failed");
            Done { seq, generation, full, mixes, page: None, shown: Vec::new() }
        });
        if out.send(done).is_err() {
            return;
        }
    }
}

/// One message into the job waiting: a later job replaces it (the whole
/// page if either asked for it); garbage is dropped here.
fn take(m: ToBuilder, job: &mut Option<Box<Job>>) {
    match m {
        ToBuilder::Drop(garbage) => drop(garbage),
        ToBuilder::Keep(page, path) => write_kept(&page, &path),
        ToBuilder::Job(mut next) => {
            next.full |= job.as_ref().is_some_and(|j| j.full);
            *job = Some(next);
        }
    }
}

/// The page of `job`: its new-music rows, the base rows when it is the
/// whole page, the recommendations' parts resolved.
fn build(res: &mut Resolver, job: Job) -> Done {
    let started = Instant::now();
    let Job { seq, generation, full, mixes, visit, parts, discover } = job;
    let lib = res.library().clone();
    let (rows, albums, shown) = discover.rows(&lib);
    let parts = parts.with_discover(rows, albums);
    let new_music = started.elapsed();
    let page = if full {
        let home = crate::home::build_with(&lib, &visit);
        let base = started.elapsed();
        let mut view = res.home(&home);
        parts.apply(&mut view, res);
        let took = started.elapsed();
        let line = format!(
            "home: page {seq} built in {} ms (new music {} ms, base rows {} ms, resolved {} ms)",
            took.as_millis(),
            new_music.as_millis(),
            (base - new_music).as_millis(),
            (took - base).as_millis()
        );
        if took >= SLOW_BUILD {
            log::info!("{line}");
        } else {
            log::debug!("{line}");
        }
        Page::Whole(view)
    } else {
        let fill = parts.resolve(res);
        log::debug!("home: the mixes of page {seq} built in {} ms", started.elapsed().as_millis());
        Page::Parts(fill)
    };
    Done { seq, generation, full, mixes, page: Some(page), shown }
}

/// Before the run's first recommendations: the mixes `shown` has (the last
/// run's, kept) stay on the page built without them, `into`, until the
/// build brings this run's.
fn carry_mixes(shown: &mut v::HomeView, into: &mut v::HomeView) {
    if shown.hero.is_none() && shown.lanes.is_empty() && shown.tiles.is_empty() {
        return;
    }
    into.hero = shown.hero.take();
    into.daylist = shown.daylist.take();
    into.tiles = std::mem::take(&mut shown.tiles);
    into.moods = std::mem::take(&mut shown.moods);
    into.style_mixes = std::mem::take(&mut shown.style_mixes);
    into.lanes = std::mem::take(&mut shown.lanes);
    into.blend_profiles = std::mem::take(&mut shown.blend_profiles);
    into.mix = std::mem::take(&mut shown.mix);
}

/// Makes again on `home` every change of `patches` that came after `generation`,
/// in their order.
fn replay(patches: &[(u64, Patch)], generation: u64, home: &mut v::HomeView) {
    for (at, patch) in patches {
        if *at > generation {
            patch(home);
        }
    }
}

impl Worker {
    /// Builds the whole page anew (off the loop).
    pub(crate) fn load_home(&mut self) {
        self.ask_home(true);
    }

    /// Asks the builder for the whole page (`full`) or for the
    /// recommendations' parts on the page as it is.
    pub(crate) fn ask_home(&mut self, full: bool) {
        let mut visit = self.visit();
        visit.again_more = self.home_build.more.again;
        let job = Job {
            seq: self.home_build.asked + 1,
            generation: self.home_build.generation,
            full,
            mixes: self.snapshot().is_some(),
            visit,
            parts: self.recs_home_parts(),
            discover: self.discover_input(),
        };
        self.home_build.asked = job.seq;
        let job = if self.home_build.here.is_none() {
            match self.home_build.jobs.send(ToBuilder::Job(Box::new(job))) {
                Ok(()) => return,
                Err(e) => {
                    log::error!("home: its builder stopped, the page is built on the loop");
                    self.home_build.here = Some(self.res.fork());
                    match e.0 {
                        ToBuilder::Job(job) => *job,
                        _ => return,
                    }
                }
            }
        } else {
            job
        };
        let Some(mut res) = self.home_build.here.take() else { return };
        let done = build(&mut res, job);
        self.home_build.here = Some(res);
        self.home_built(done);
    }

    /// A page came from the builder: swapped in, with the changes made to
    /// the shown page since its inputs were taken made again; built again
    /// when what it is built of changed meanwhile and no newer page is on
    /// its way.
    pub(crate) fn home_built(&mut self, done: Done) {
        let hb = &mut self.home_build;
        hb.came = hb.came.max(done.seq);
        let waiting = hb.busy();
        let stale = done.generation < hb.generation;
        let patches = std::mem::take(&mut hb.patches);
        self.discover_shown(&done.shown);
        self.turn.lap("shown");
        match done.page {
            Some(Page::Whole(mut view)) => {
                replay(&patches, done.generation, &mut view);
                let stages: Vec<_> = view.worlds.iter().filter_map(|w| w.release).collect();
                let mixes = done.mixes;
                self.publish(|s| {
                    if !mixes {
                        carry_mixes(&mut s.home, &mut view);
                    }
                    s.home = view;
                });
                self.turn.lap("swap");
                self.kick_hd_covers(stages);
                self.turn.lap("hd covers");
            }
            Some(Page::Parts(fill)) => {
                self.publish(|s| {
                    fill.apply(&mut s.home);
                    replay(&patches, done.generation, &mut s.home);
                });
                self.turn.lap("swap");
            }
            None => {}
        }
        // The pages still on their way may be older than these changes;
        // with none on its way, every change is in what comes next.
        if waiting {
            self.home_build.patches = patches.into_iter().filter(|(at, _)| *at > done.generation).collect();
        }
        if stale && !waiting {
            log::debug!("home: page {} was built of older inputs, building it again", done.seq);
            self.ask_home(done.full);
        } else if !waiting {
            self.keep_home(false);
        }
    }

    /// A row of the page was scrolled near its end: more of it where its
    /// source has more («Слушать снова» draws on, «Новое для тебя» shows
    /// more of the pass's discoveries, «Миксы для тебя» builds the cards
    /// its ten left out), built off the loop as any page.
    pub(crate) fn home_more(&mut self, row: v::HomeRow) {
        let (again, new) = {
            let s = self.state.read();
            (s.home.listen_again.len(), s.home.discover.len())
        };
        match row {
            v::HomeRow::Mixes => self.recs_more_mixes(),
            v::HomeRow::ListenAgain => {
                if self.home_build.more.ask(row, again, None) {
                    log::debug!("home: more of «Слушать снова» ({} songs)", crate::home_v2::LISTEN_AGAIN + self.home_build.more.again);
                    self.ask_home(true);
                }
            }
            v::HomeRow::New => {
                let kept = self.discoveries_kept();
                if self.home_build.more.ask(row, new, Some(kept)) {
                    log::debug!("home: more of «Новое для тебя» ({kept} kept)");
                    self.ask_home(false);
                }
            }
        }
    }

    /// At the start: the page the last run kept, shown while this run's
    /// first page is built.
    pub(crate) fn show_kept_home(&mut self) {
        let Some(path) = self.home_build.keep.clone() else { return };
        let Some(mut page) = read_kept(&path) else { return };
        page.loading = false;
        page.greeting_hour = jiff::Zoned::now().hour() as u8;
        log::debug!("home: the last run's page is shown while this run's is built");
        self.publish(|s| {
            if s.home.loading {
                s.home = page;
            }
        });
    }

    /// Keeps the page shown for the next start (on the builder's thread):
    /// at most every [`KEEP_EVERY`], else once the time is over; `now`
    /// writes it here at once (the app closes).
    pub(crate) fn keep_home(&mut self, now: bool) {
        let hb = &mut self.home_build;
        let Some(path) = hb.keep.clone() else { return };
        if !now && hb.kept_at.is_some_and(|t| t.elapsed() < KEEP_EVERY) {
            hb.keep_due = true;
            return;
        }
        if now && hb.kept_at.is_none() && !hb.keep_due {
            // Nothing of this run was built: the kept page stays as it is.
            return;
        }
        hb.kept_at = Some(Instant::now());
        hb.keep_due = false;
        let page = self.state.read().home.clone();
        if page.loading {
            return;
        }
        if now || self.home_build.here.is_some() {
            write_kept(&page, &path);
            return;
        }
        if let Err(e) = self.home_build.jobs.send(ToBuilder::Keep(Box::new(page), path))
            && let ToBuilder::Keep(page, path) = e.0
        {
            write_kept(&page, &path);
        }
    }

    /// A newer page waits to be kept and its time came (the tick).
    pub(crate) fn keep_home_due(&mut self) {
        let hb = &self.home_build;
        if hb.keep_due && !hb.busy() && hb.kept_at.is_none_or(|t| t.elapsed() >= KEEP_EVERY) {
            self.keep_home(false);
        }
    }

    /// Changes the shown page now (a heart, a hidden mix, a larger cover, a
    /// photo) and keeps the change for the pages on their way, built of
    /// what was before it.
    pub(crate) fn patch_home(&mut self, patch: impl Fn(&mut v::HomeView) + 'static) {
        self.publish(|s| patch(&mut s.home));
        let hb = &mut self.home_build;
        hb.generation += 1;
        if hb.busy() {
            hb.patches.push((hb.generation, Box::new(patch)));
        }
    }

    /// What the page is built of changed (a like, a dislike, a play, a
    /// library edit): a page on its way, built of what was before, is built
    /// again once shown.
    pub(crate) fn home_input_changed(&mut self) {
        self.home_build.generation += 1;
    }

    /// Lets go of `value` on the builder's thread, not on the loop (a large
    /// snapshot of the recommendations takes a while to free).
    pub(crate) fn drop_off_loop<T: Send + 'static>(&self, value: T) {
        if self.home_build.here.is_none() {
            let _ = self.home_build.jobs.send(ToBuilder::Drop(Box::new(value)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(seq: u64, full: bool) -> Box<Job> {
        Box::new(Job {
            seq,
            generation: seq,
            full,
            mixes: true,
            visit: Visit::default(),
            parts: HomeParts::of(None, Vec::new(), Vec::new(), false, false),
            discover: DiscoverInput::default(),
        })
    }

    #[test]
    fn a_row_gets_more_while_its_source_has_more() {
        let again = crate::home_v2::LISTEN_AGAIN;
        let new = nikgob_recs::discover::DISCOVERIES;
        let mut more = More::default();
        // «Слушать снова» showing all it was asked for: more.
        assert!(more.ask(v::HomeRow::ListenAgain, again, None));
        assert_eq!(more.again, AGAIN_MORE);
        // Asked again before the page came, or the draw had no more: no.
        assert!(!more.ask(v::HomeRow::ListenAgain, again, None));
        assert!(!more.ask(v::HomeRow::ListenAgain, again + 3, None));
        assert!(more.ask(v::HomeRow::ListenAgain, again + AGAIN_MORE, None));
        // «Новое для тебя»: only while the pass kept more than is shown.
        assert!(!more.ask(v::HomeRow::New, new, Some(new)), "the pass kept no more");
        assert!(more.ask(v::HomeRow::New, new, Some(new + 1)));
        assert_eq!(more.new, crate::recs::HOME_NEW_MORE);
        // Disliked songs make the row shorter than it was asked for: no.
        assert!(!more.ask(v::HomeRow::New, new + 2, Some(3 * new)));
        // The mixes are the recommendations' to count.
        assert!(!more.ask(v::HomeRow::Mixes, 100, Some(1000)));
        assert_eq!(more, More { again: 2 * AGAIN_MORE, new: crate::recs::HOME_NEW_MORE });
    }

    #[test]
    fn the_jobs_that_wait_are_one_build_of_the_newest_inputs() {
        let mut waiting = None;
        take(ToBuilder::Job(job(1, true)), &mut waiting);
        take(ToBuilder::Drop(Box::new(vec![1u8; 16])), &mut waiting);
        take(ToBuilder::Job(job(2, false)), &mut waiting);
        let j = waiting.expect("a job");
        // The newest inputs, the whole page since the first asked for it.
        assert_eq!((j.seq, j.full), (2, true));
        let mut waiting = None;
        take(ToBuilder::Job(job(3, false)), &mut waiting);
        take(ToBuilder::Job(job(4, false)), &mut waiting);
        assert_eq!(waiting.map(|j| (j.seq, j.full)), Some((4, false)));
    }

    fn lane(key: v::MixKey) -> v::LaneCard {
        v::LaneCard {
            key,
            title: "Любимые".into(),
            short_titles: Vec::new(),
            artists: vec!["Кино".into()],
            new_artists: Vec::new(),
            covers: vec![v::Cover::web("https://fake.invalid/a.jpg")],
            new_cover: None,
            tracks: Vec::new(),
            new_tracks: Vec::new(),
            blend_base: Some(1),
            queue: None,
            why: None,
        }
    }

    #[test]
    fn a_page_of_older_inputs_gets_the_changes_made_since_in_their_order() {
        let key = v::MixKey::OnRepeat;
        let mut patches: Vec<(u64, Patch)> = Vec::new();
        patches.push((2, Box::new(|h: &mut v::HomeView| h.history = true)));
        patches.push((3, Box::new(move |h: &mut v::HomeView| h.lanes.retain(|l| l.key != key))));
        patches.push((4, Box::new(|h: &mut v::HomeView| h.history = false)));
        // Built after the first change: the later two are made again.
        let mut home = v::HomeView { lanes: vec![lane(key)], ..Default::default() };
        replay(&patches, 2, &mut home);
        assert!(home.lanes.is_empty());
        assert!(!home.history, "the last change wins");
        // Built after all of them: nothing to make again.
        let mut home = v::HomeView { lanes: vec![lane(key)], history: true, ..Default::default() };
        replay(&patches, 4, &mut home);
        assert_eq!(home.lanes.len(), 1);
        assert!(home.history);
    }

    #[test]
    fn the_kept_page_reads_back_and_one_of_another_format_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(KEPT_FILE);
        let page = v::HomeView { greeting_hour: 7, history: true, lanes: vec![lane(v::MixKey::Liked)], ..Default::default() };
        write_kept(&page, &path);
        assert_eq!(read_kept(&path), Some(page));
        std::fs::write(&path, br#"{"version":0,"page":{}}"#).unwrap();
        assert_eq!(read_kept(&path), None);
        assert_eq!(read_kept(&dir.path().join("none.json")), None);
    }

    #[test]
    fn a_page_built_before_the_first_mixes_keeps_the_ones_shown() {
        let mut shown = v::HomeView { lanes: vec![lane(v::MixKey::Liked)], ..Default::default() };
        let mut built = v::HomeView { greeting_hour: 9, ..Default::default() };
        carry_mixes(&mut shown, &mut built);
        assert_eq!((built.lanes.len(), built.greeting_hour), (1, 9));
        // Nothing of the kind shown: the page keeps what it was built with.
        let mut built = v::HomeView { lanes: vec![lane(v::MixKey::Everything)], ..Default::default() };
        carry_mixes(&mut v::HomeView::default(), &mut built);
        assert_eq!(built.lanes.len(), 1);
    }
}
