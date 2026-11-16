//! The runtime's `wake`, shared: the runtime calls it after every change of
//! its state, and each part of the host that follows the state (the API's
//! event stream, later the mesh) adds a callback of its own, before or
//! after the runtime starts.

use std::sync::{Arc, PoisonError, RwLock};

type Callback = Arc<dyn Fn() + Send + Sync>;

/// A list of callbacks called together. Cheap to clone (one list).
#[derive(Clone, Default)]
pub struct Wake(Arc<RwLock<Vec<Callback>>>);

impl Wake {
    /// Calls `f` at every change from now on. It must be cheap and must not
    /// block (it runs on the player's loop): notify, do not work.
    #[allow(dead_code)] // for the server next to the player
    pub fn add(&self, f: impl Fn() + Send + Sync + 'static) {
        self.0.write().unwrap_or_else(PoisonError::into_inner).push(Arc::new(f));
    }

    /// Calls every callback.
    pub fn wake(&self) {
        let callbacks = self.0.read().unwrap_or_else(PoisonError::into_inner);
        for f in callbacks.iter() {
            f();
        }
    }

    /// What the runtime is given as its `wake`.
    pub fn callback(&self) -> Arc<dyn Fn() + Send + Sync> {
        let me = self.clone();
        Arc::new(move || me.wake())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn every_callback_hears_every_wake_even_one_added_later() {
        let wake = Wake::default();
        let runtime_side = wake.callback();
        runtime_side();
        let a = Arc::new(AtomicUsize::new(0));
        let b = Arc::new(AtomicUsize::new(0));
        let a2 = a.clone();
        wake.add(move || {
            a2.fetch_add(1, Ordering::Relaxed);
        });
        runtime_side();
        // Added after the runtime got its callback.
        let b2 = b.clone();
        wake.clone().add(move || {
            b2.fetch_add(1, Ordering::Relaxed);
        });
        runtime_side();
        runtime_side();
        assert_eq!(a.load(Ordering::Relaxed), 3);
        assert_eq!(b.load(Ordering::Relaxed), 2);
    }
}
