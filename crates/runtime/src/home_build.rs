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
//!
//! [`Command::LoadHome`]: nikgob_view::Command::LoadHome

use std::any::Any;
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

/// What a build of the page is asked for, gathered on the worker.
pub(crate) struct Job {
    /// Its number: pages come back in this order.
    seq: u64,
    /// The input generation it was gathered at ([`HomeBuild::gen`]).
    gen: u64,
    /// The whole page; else only the recommendations' parts on the page as
    /// it is.
    full: bool,
    visit: Visit,
    parts: HomeParts,
    discover: DiscoverInput,
}

enum ToBuilder {
    Job(Box<Job>),
    /// Something to let go of off the loop ([`Worker::drop_off_loop`]).
    Drop(Box<dyn Any + Send>),
}

/// A finished build.
pub(crate) struct Done {
    seq: u64,
    gen: u64,
    full: bool,
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
    gen: u64,
    /// The changes of the shown page made while builds were on their way,
    /// each with the generation it came at.
    patches: Vec<(u64, Patch)>,
}

impl HomeBuild {
    /// Starts the builder's thread with a fork of the worker's resolver.
    pub(crate) fn start(res: &Resolver) -> Self {
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
        Self { jobs, done, here, asked: 0, came: 0, gen: 0, patches: Vec::new() }
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
        let (seq, gen, full) = (job.seq, job.gen, job.full);
        let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build(&mut res, *job))).unwrap_or_else(|_| {
            log::error!("home: the build of page {seq} failed");
            Done { seq, gen, full, page: None, shown: Vec::new() }
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
    let Job { seq, gen, full, visit, parts, discover } = job;
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
    Done { seq, gen, full, page: Some(page), shown }
}

/// Makes again on `home` every change of `patches` that came after `gen`,
/// in their order.
fn replay(patches: &[(u64, Patch)], gen: u64, home: &mut v::HomeView) {
    for (at, patch) in patches {
        if *at > gen {
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
        let job = Job {
            seq: self.home_build.asked + 1,
            gen: self.home_build.gen,
            full,
            visit: self.visit(),
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
                        ToBuilder::Drop(_) => return,
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
        let stale = done.gen < hb.gen;
        let patches = std::mem::take(&mut hb.patches);
        self.discover_shown(&done.shown);
        self.turn.lap("shown");
        match done.page {
            Some(Page::Whole(mut view)) => {
                replay(&patches, done.gen, &mut view);
                let stages: Vec<_> = view.worlds.iter().filter_map(|w| w.release).collect();
                self.publish(|s| s.home = view);
                self.turn.lap("swap");
                self.kick_hd_covers(stages);
                self.turn.lap("hd covers");
            }
            Some(Page::Parts(fill)) => {
                self.publish(|s| {
                    fill.apply(&mut s.home);
                    replay(&patches, done.gen, &mut s.home);
                });
                self.turn.lap("swap");
            }
            None => {}
        }
        // The pages still on their way may be older than these changes;
        // with none on its way, every change is in what comes next.
        if waiting {
            self.home_build.patches = patches.into_iter().filter(|(at, _)| *at > done.gen).collect();
        }
        if stale && !waiting {
            log::debug!("home: page {} was built of older inputs, building it again", done.seq);
            self.ask_home(done.full);
        }
    }

    /// Changes the shown page now (a heart, a hidden mix, a larger cover, a
    /// photo) and keeps the change for the pages on their way, built of
    /// what was before it.
    pub(crate) fn patch_home(&mut self, patch: impl Fn(&mut v::HomeView) + 'static) {
        self.publish(|s| patch(&mut s.home));
        let hb = &mut self.home_build;
        hb.gen += 1;
        if hb.busy() {
            hb.patches.push((hb.gen, Box::new(patch)));
        }
    }

    /// What the page is built of changed (a like, a dislike, a play, a
    /// library edit): a page on its way, built of what was before, is built
    /// again once shown.
    pub(crate) fn home_input_changed(&mut self) {
        self.home_build.gen += 1;
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
            gen: seq,
            full,
            visit: Visit::default(),
            parts: HomeParts::of(None, Vec::new(), Vec::new(), false, false),
            discover: DiscoverInput::default(),
        })
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

    #[test]
    fn a_page_of_older_inputs_gets_the_changes_made_since_in_their_order() {
        let key = v::MixKey::OnRepeat;
        let mut patches: Vec<(u64, Patch)> = Vec::new();
        patches.push((2, Box::new(|h: &mut v::HomeView| h.history = true)));
        patches.push((3, Box::new(move |h: &mut v::HomeView| h.lanes.retain(|l| l.key != key))));
        patches.push((4, Box::new(|h: &mut v::HomeView| h.history = false)));
        let lane = v::LaneCard {
            key,
            title: String::new(),
            short_titles: Vec::new(),
            artists: Vec::new(),
            new_artists: Vec::new(),
            covers: Vec::new(),
            new_cover: None,
            tracks: Vec::new(),
            blend_base: None,
            queue: None,
            why: None,
        };
        // Built after the first change: the later two are made again.
        let mut home = v::HomeView { lanes: vec![lane.clone()], ..Default::default() };
        replay(&patches, 2, &mut home);
        assert!(home.lanes.is_empty());
        assert!(!home.history, "the last change wins");
        // Built after all of them: nothing to make again.
        let mut home = v::HomeView { lanes: vec![lane], history: true, ..Default::default() };
        replay(&patches, 4, &mut home);
        assert_eq!(home.lanes.len(), 1);
        assert!(home.history);
    }
}
