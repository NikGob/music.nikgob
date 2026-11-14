//! The threads that open items off the decoder thread.
//!
//! At most [`MAX_OPENERS`] opens of items still wanted run at once; more
//! wait in line, and one whose item was cancelled while it waited never
//! runs. A cancelled open that runs ends as soon as its source gives up
//! (see [`crate::cancel`]), and a service lookup before the stream exists
//! gives up too where the service lets it ([`crate::open_wanted`]); one
//! that cannot keeps its thread until it returns, but no longer counts:
//! the newest open gets a thread at once. Threads in all (such leftovers
//! included) stay within [`MAX_THREADS`]. A thread that is done takes the
//! next open in line instead of ending.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use crate::cancel::{Slot, cancelled};

/// Opens of wanted items running at once.
pub(crate) const MAX_OPENERS: usize = 3;
/// Opener threads in all, the ones finishing a superseded open included.
pub(crate) const MAX_THREADS: usize = 6;

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
    threads: usize,
    /// Threads started that have not taken an open yet.
    starting: usize,
    /// The slots of the opens running now, by job number.
    running: Vec<(u64, Option<Arc<Slot>>)>,
    queue: VecDeque<Job>,
    jobs: u64,
}

impl Pool {
    /// Running opens of items still wanted.
    fn wanted(&self) -> usize {
        self.running.iter().filter(|(_, s)| !cancelled(s.as_ref())).count()
    }

    /// The opens in line whose items were cancelled.
    fn take_cancelled(&mut self) -> Vec<Job> {
        let mut skipped = Vec::new();
        let mut i = 0;
        while i < self.queue.len() {
            if cancelled(self.queue[i].slot.as_ref()) {
                skipped.extend(self.queue.remove(i));
            } else {
                i += 1;
            }
        }
        skipped
    }
}

#[derive(Clone, Default)]
pub(crate) struct Openers(Arc<Mutex<Pool>>);

impl Openers {
    /// Opens an item: `work` runs with [`Start::Run`] on an opener thread,
    /// or with another [`Start`] where it does not open.
    pub(crate) fn submit(&self, slot: Option<Arc<Slot>>, work: Work) {
        let (skipped, spawn) = {
            let mut pool = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            let skipped = pool.take_cancelled();
            pool.queue.push_back(Job { slot, work });
            // A thread for each open in line that may run now and that no
            // thread starting already will take.
            let spawn = pool
                .queue
                .len()
                .saturating_sub(pool.starting)
                .min(MAX_OPENERS.saturating_sub(pool.wanted() + pool.starting))
                .min(MAX_THREADS - pool.threads);
            pool.threads += spawn;
            pool.starting += spawn;
            (skipped, spawn)
        };
        for job in skipped {
            (job.work)(Start::Skip);
        }
        for _ in 0..spawn {
            self.spawn();
        }
    }

    fn spawn(&self) {
        let pool = Arc::clone(&self.0);
        let spawned = std::thread::Builder::new()
            .name("nikgob-audio-open".into())
            .spawn(move || work_through(&pool));
        if let Err(e) = spawned {
            let failed = {
                let mut pool = self.0.lock().unwrap_or_else(PoisonError::into_inner);
                pool.threads -= 1;
                pool.starting -= 1;
                // Nobody else takes the line: it fails.
                if pool.threads == 0 {
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

    /// Opener threads alive now.
    #[cfg(test)]
    fn threads(&self) -> usize {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).threads
    }
}

/// An opener thread: opens in line while there are some and fewer than
/// [`MAX_OPENERS`] wanted opens run.
fn work_through(pool: &Mutex<Pool>) {
    let mut done = None;
    let mut first = true;
    loop {
        let (skipped, job) = {
            let mut pool = pool.lock().unwrap_or_else(PoisonError::into_inner);
            if std::mem::take(&mut first) {
                pool.starting -= 1;
            }
            if let Some(n) = done.take() {
                pool.running.retain(|(m, _)| *m != n);
            }
            let skipped = pool.take_cancelled();
            let job = if pool.wanted() < MAX_OPENERS {
                pool.queue.pop_front()
            } else {
                None
            };
            match &job {
                Some(job) => {
                    pool.jobs += 1;
                    let n = pool.jobs;
                    pool.running.push((n, job.slot.clone()));
                    done = Some(n);
                }
                None => pool.threads -= 1,
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

    /// An open that hangs, uninterruptibly, until `gate`.
    fn hanging(
        n: u64,
        gate: &Arc<AtomicBool>,
        busy: &Arc<AtomicUsize>,
        tx: &mpsc::Sender<Option<u64>>,
    ) -> Work {
        let (gate, busy, tx) = (Arc::clone(gate), Arc::clone(busy), tx.clone());
        Box::new(move |start| {
            if let Start::Run = start {
                busy.fetch_add(1, Ordering::SeqCst);
                while !gate.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                busy.fetch_sub(1, Ordering::SeqCst);
                tx.send(Some(n)).unwrap();
            } else {
                tx.send(None).unwrap();
            }
        })
    }

    #[test]
    fn wanted_opens_are_capped_and_the_newest_is_not_held_up_by_superseded_ones() {
        let openers = Openers::default();
        let gate = Arc::new(AtomicBool::new(false));
        let busy = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = mpsc::channel();
        let slots: Vec<_> = (0..5).map(|_| Slot::new(true)).collect();
        for (n, s) in slots.iter().enumerate() {
            openers.submit(Some(Arc::clone(s)), hanging(n as u64, &gate, &busy, &tx));
        }
        wait_until("three opens run", || busy.load(Ordering::SeqCst) == MAX_OPENERS);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(busy.load(Ordering::SeqCst), MAX_OPENERS, "the cap holds");

        // The three that hang are superseded, and so is one in line: the one
        // left in line gets a thread at once, without waiting for them.
        for s in &slots[..4] {
            s.cancel();
        }
        let newest = Slot::new(true);
        openers.submit(Some(Arc::clone(&newest)), hanging(9, &gate, &busy, &tx));
        wait_until("the wanted ones run", || busy.load(Ordering::SeqCst) == 5);
        assert!(openers.threads() <= MAX_THREADS);

        // Superseded leftovers never pass the thread ceiling.
        for s in &slots {
            s.cancel();
        }
        newest.cancel();
        for n in 10..20 {
            let s = Slot::new(true);
            s.cancel();
            openers.submit(Some(s), hanging(n, &gate, &busy, &tx));
            assert!(openers.threads() <= MAX_THREADS, "{}", openers.threads());
        }

        gate.store(true, Ordering::SeqCst);
        let mut ran: Vec<u64> = (0..16)
            .filter_map(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap())
            .collect();
        ran.sort_unstable();
        assert_eq!(ran, [0, 1, 2, 4, 9]);
        wait_until("the threads end", || openers.threads() == 0);
    }
}
