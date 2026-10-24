//! Where a slow frame spent its time: named scopes timed only while the app
//! logs frame times (`NIKGOB_FRAMES`), otherwise one relaxed load each.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

static ON: AtomicBool = AtomicBool::new(false);

thread_local! {
    static SPENT: RefCell<Vec<(&'static str, f32)>> = const { RefCell::new(Vec::new()) };
}

/// Starts timing scopes.
pub fn enable() {
    ON.store(true, Ordering::Relaxed);
}

/// Times until dropped, under `name` (scopes inside it count in both).
pub struct Scope(Option<(&'static str, Instant)>);

pub fn scope(name: &'static str) -> Scope {
    Scope(ON.load(Ordering::Relaxed).then(|| (name, Instant::now())))
}

impl Drop for Scope {
    fn drop(&mut self) {
        if let Some((name, at)) = self.0.take() {
            let ms = at.elapsed().as_secs_f32() * 1000.0;
            SPENT.with_borrow_mut(|v| match v.iter_mut().find(|(n, _)| *n == name) {
                Some(e) => e.1 += ms,
                None => v.push((name, ms)),
            });
        }
    }
}

/// The time of every scope since the last call, in milliseconds.
pub fn take() -> Vec<(&'static str, f32)> {
    SPENT.with_borrow_mut(std::mem::take)
}
