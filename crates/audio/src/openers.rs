//! The threads that open items off the decoder thread.
//!
//! At most [`MAX_OPENERS`] run at once; more opens wait in line, and one
//! whose item was cancelled while it waited never runs. A cancelled open
//! that runs ends as soon as its source gives up (see [`crate::cancel`]);
//! only a service's own call that cannot be interrupted (a lookup before
//! the stream exists) keeps its thread until the call returns, and the cap
//! keeps such threads from piling up. A thread that is done takes the next
//! open in line instead of ending.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use crate::cancel::{Slot, cancelled};

/// Opener threads running at once.
pub(crate) const MAX_OPENERS: usize = 3;

/// How an open in line comes to an end.
pub(crate) enum Start {
    /// Open it now (on an opener thread).
    Run,
    /// Its item was cancelled before it ran: nothing to do.
    Skip,
    /// No thread could be started for it.
    Failed(String),
}

type Work = Box<dyn FnOnce(Start) + Send>;

struct Job {
    slot: Option<Arc<Slot>>,
    work: Work,
}

#[derive(Default)]
struct Pool {
    running: usize,
    queue: VecDeque<Job>,
}

#[derive(Clone, Default)]
pub(crate) struct Openers(Arc<Mutex<Pool>>);

impl Openers {
    /// Opens an item: `work` runs with [`Start::Run`] on an opener thread,
    /// or with another [`Start`] where it does not open.
    pub(crate) fn submit(&self, slot: Option<Arc<Slot>>, work: Work) {
        let (skipped, spawn) = {
            let mut pool = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            let skipped = take_cancelled(&mut pool.queue);
            pool.queue.push_back(Job { slot, work });
            let spawn = pool.running < MAX_OPENERS;
            if spawn {
                pool.running += 1;
            }
            (skipped, spawn)
        };
        for job in skipped {
            (job.work)(Start::Skip);
        }
        if !spawn {
            return;
        }
        let pool = Arc::clone(&self.0);
        let spawned = std::thread::Builder::new()
            .name("nikgob-audio-open".into())
            .spawn(move || work_through(&pool));
        if let Err(e) = spawned {
            let failed = {
                let mut pool = self.0.lock().unwrap_or_else(PoisonError::into_inner);
                pool.running -= 1;
                // Nobody else takes the line: it fails.
                if pool.running == 0 {
                    pool.queue.drain(..).collect()
                } else {
                    Vec::new()
                }
            };
            for job in failed {
                (job.work)(Start::Failed(e.to_string()));
            }
        }
    }

    /// Opener threads running now.
    #[cfg(test)]
    fn running(&self) -> usize {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).running
    }
}

/// The opens in line whose items were cancelled.
fn take_cancelled(queue: &mut VecDeque<Job>) -> Vec<Job> {
    let mut skipped = Vec::new();
    let mut i = 0;
    while i < queue.len() {
        if cancelled(queue[i].slot.as_ref()) {
            skipped.extend(queue.remove(i));
        } else {
            i += 1;
        }
    }
    skipped
}

/// An opener thread: opens in line until there are none.
fn work_through(pool: &Mutex<Pool>) {
    loop {
        let (skipped, job) = {
            let mut pool = pool.lock().unwrap_or_else(PoisonError::into_inner);
            let skipped = take_cancelled(&mut pool.queue);
            let job = pool.queue.pop_front();
            if job.is_none() {
                pool.running -= 1;
            }
            (skipped, job)
        };
        for job in skipped {
            (job.work)(Start::Skip);
        }
        let Some(job) = job else { return };
        (job.work)(Start::Run);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use super::*;

    fn wait_until(what: &str, f: impl Fn() -> bool) {
        let t = Instant::now();
        while !f() {
            assert!(t.elapsed() < Duration::from_secs(5), "timed out: {what}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn opens_are_capped_and_a_cancelled_one_in_line_never_runs() {
        let openers = Openers::default();
        let gate = Arc::new(AtomicBool::new(false));
        let busy = Arc::new(AtomicUsize::new(0));
        let most = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = mpsc::channel();
        let mut slots = Vec::new();
        for n in 0..8u64 {
            let slot = Slot::new(n, true);
            slots.push(Arc::clone(&slot));
            let (gate, busy, most, tx) = (
                Arc::clone(&gate),
                Arc::clone(&busy),
                Arc::clone(&most),
                tx.clone(),
            );
            openers.submit(
                Some(slot),
                Box::new(move |start| {
                    if let Start::Run = start {
                        let now = busy.fetch_add(1, Ordering::SeqCst) + 1;
                        most.fetch_max(now, Ordering::SeqCst);
                        while !gate.load(Ordering::SeqCst) {
                            std::thread::sleep(Duration::from_millis(1));
                        }
                        busy.fetch_sub(1, Ordering::SeqCst);
                        tx.send(Some(n)).unwrap();
                    } else {
                        tx.send(None).unwrap();
                    }
                }),
            );
        }
        wait_until("three opens run", || busy.load(Ordering::SeqCst) == MAX_OPENERS);
        // Everything but the newest is superseded while the first three hang.
        for s in &slots[..7] {
            s.cancel();
        }
        gate.store(true, Ordering::SeqCst);
        let mut ran: Vec<u64> = (0..8)
            .filter_map(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap())
            .collect();
        ran.sort_unstable();
        assert_eq!(ran, [0, 1, 2, 7], "the ones that hung, then the newest");
        assert_eq!(most.load(Ordering::SeqCst), MAX_OPENERS);
        wait_until("the threads end", || openers.running() == 0);
    }
}
