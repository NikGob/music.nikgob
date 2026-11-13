//! The radio's picks (a wave's pool with them) and the guests of «✦ С
//! дополнением», on one thread that lives as long as the worker instead of
//! a thread per request.
//!
//! Each kind of request has a lane, and in a lane the newer request wins
//! (but in the niches' lane, whose requests all run in order):
//! one still waiting when a newer one comes is dropped unrun. That is safe
//! because each kind has at most one request on its way per run (the
//! radio's `extending`, the guests' `ranking`): a newer one in the same
//! lane is always of a newer run, and the older run's answer would be
//! thrown away anyway. A request being worked on is never cut off. The
//! loop only queues ([`Picker::send`] never waits), and when nothing more
//! waits the thread gives back what the work left
//! ([`crate::release_thread_memory`]).

use std::collections::VecDeque;
use std::sync::OnceLock;

use crossbeam_channel::{Receiver, Sender};

/// Which kind of request: a newer one replaces a waiting one of its kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lane {
    /// A radio's (or a wave's) next picks.
    Radio,
    /// The guests of the playlist playing.
    Plus,
    /// The niches' top-ups: their plans and the library's writes of their
    /// budgets and finds — every one runs, in the order sent (none is
    /// replaced).
    Niche,
}

struct Job {
    lane: Lane,
    work: Box<dyn FnOnce() + Send>,
}

/// The thread, started with the first request.
#[derive(Default)]
pub(crate) struct Picker {
    jobs: OnceLock<Result<Sender<Job>, String>>,
}

impl Picker {
    /// Queues `work` in `lane` (a waiting request of that lane goes).
    /// Never waits; an error only when the thread could not start.
    pub(crate) fn send(&self, lane: Lane, work: impl FnOnce() + Send + 'static) -> Result<(), String> {
        let jobs = self.jobs.get_or_init(|| {
            let (jobs, inbox) = crossbeam_channel::unbounded::<Job>();
            std::thread::Builder::new()
                .name("nikgob-picks".into())
                .spawn(move || run(&inbox))
                .map(|_| jobs)
                .map_err(|e| e.to_string())
        });
        let jobs = jobs.as_ref().map_err(Clone::clone)?;
        jobs.send(Job { lane, work: Box::new(work) }).map_err(|_| "the picks' thread is gone".to_string())
    }
}

/// The thread's loop: the requests waiting, oldest first, the newest of
/// each lane only.
fn run(inbox: &Receiver<Job>) {
    let mut waiting: VecDeque<Job> = VecDeque::new();
    loop {
        if waiting.is_empty() {
            match inbox.recv() {
                Ok(job) => waiting.push_back(job),
                Err(_) => return,
            }
        }
        while let Ok(job) = inbox.try_recv() {
            queue(&mut waiting, job);
        }
        let Some(job) = waiting.pop_front() else { continue };
        let lane = job.lane;
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(job.work)).is_err() {
            log::error!("picks: a {lane:?} request failed");
        }
        if waiting.is_empty() && inbox.is_empty() {
            crate::release_thread_memory();
        }
    }
}

/// `job` joins the requests waiting; an older one of its lane goes (but
/// for [`Lane::Niche`]'s).
fn queue(waiting: &mut VecDeque<Job>, job: Job) {
    if job.lane != Lane::Niche {
        waiting.retain(|w| w.lane != job.lane);
    }
    waiting.push_back(job);
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    /// While a request is worked on, newer ones queue without waiting; of
    /// those, only the newest of each lane is run, in the order they came.
    #[test]
    fn the_newest_request_of_a_lane_wins() {
        let picker = Picker::default();
        let (ran, done) = crossbeam_channel::unbounded::<&'static str>();
        let (started, has_started) = crossbeam_channel::bounded::<()>(1);
        let (open, gate) = crossbeam_channel::bounded::<()>(1);
        let r = ran.clone();
        picker
            .send(Lane::Radio, move || {
                let _ = started.send(());
                let _ = gate.recv();
                let _ = r.send("first");
            })
            .unwrap();
        has_started.recv_timeout(Duration::from_secs(10)).expect("the first request runs");
        let t = Instant::now();
        for (lane, name) in [(Lane::Radio, "radio a"), (Lane::Niche, "niche 1"), (Lane::Radio, "radio b"), (Lane::Plus, "plus c"), (Lane::Niche, "niche 2"), (Lane::Radio, "radio d")] {
            let r = ran.clone();
            picker.send(lane, move || { let _ = r.send(name); }).unwrap();
        }
        assert!(t.elapsed() < Duration::from_millis(500), "sending never waits for the work");
        open.send(()).unwrap();
        let got: Vec<_> = (0..5).map(|_| done.recv_timeout(Duration::from_secs(10)).expect("a request ran")).collect();
        assert_eq!(got, ["first", "niche 1", "plus c", "niche 2", "radio d"], "the niches' requests all run, in order");
        assert!(done.recv_timeout(Duration::from_millis(200)).is_err(), "the older ones never run");
    }

    /// A request that panics does not take the thread with it.
    #[test]
    fn a_failed_request_leaves_the_thread() {
        let picker = Picker::default();
        picker.send(Lane::Plus, || panic!("a broken request")).unwrap();
        let (ran, done) = crossbeam_channel::unbounded();
        picker.send(Lane::Plus, move || { let _ = ran.send(()); }).unwrap();
        // (The first may have been replaced before it ran; either way the
        // second runs.)
        done.recv_timeout(Duration::from_secs(10)).expect("the thread goes on");
    }
}
