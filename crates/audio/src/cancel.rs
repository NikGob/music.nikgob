//! Stream reads that give way to commands.
//!
//! The decoder thread both reads the bytes of a stream and takes the
//! commands, so a read waiting for the network (a stalled download) would
//! hold Stop, Play, Next or Seek up for as long as it waits; an opener
//! thread probing a stream nobody wants any more would wait just as long.
//!
//! Every item with bytes to open (a stream or a file) that is handed to the
//! [`Player`](crate::Player) gets a [`Slot`]. Once the item's
//! [`ByteSource`](crate::ByteSource) exists, its [`Interrupter`] goes into
//! the slot. The player's methods run on the caller's thread and go through
//! [`Slots`], which also keeps the order of the commands they send (see
//! [`Slots::lock`]): a play, a stop or the engine's end cancel every item
//! handed over before, a new next item (or none) cancels the next one that
//! has not started, a seek interrupts every item the decoder may be reading
//! (the playing one, and the next one or both sides of a crossfade, which
//! the decoder reads at a transition) until the decoder takes that seek. A
//! read waiting in a cancelled or interrupted stream returns an error at
//! once, so the decoder comes back to its commands.
//!
//! Which read gave way is told by the source itself: a read that gives up
//! fails with [`std::io::ErrorKind::WouldBlock`], the engine's wrapper of
//! each source notes such a read of it when the engine itself had cancelled
//! or interrupted it ([`Slot::note_given_way`]), and the decoder looks at
//! the very source whose read failed ([`Slot::take_gave_way`]). Any other
//! read error is the stream's own failure and is reported, also while a
//! seek is on its way.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// Makes the reads of a [`ByteSource`](crate::ByteSource) that wait for
/// bytes give up: called with `true`, a read waiting now returns an error
/// at once, and so does every later read that would have to wait, until
/// it is called with `false`. Reads of bytes already there go on as
/// usual. Called from any thread, also while a read waits. A read that
/// gives up this way must fail with [`std::io::ErrorKind::WouldBlock`] and
/// only such one: the engine takes any other error for the stream's own
/// failure.
pub type Interrupter = Arc<dyn Fn(bool) + Send + Sync>;

#[derive(Default)]
struct State {
    /// The source's interrupter, once the item's opener gave the source.
    handle: Option<Interrupter>,
    /// Superseded: the item will never be heard (again).
    cancelled: bool,
    /// Seeks sent that interrupted it and that the decoder has not taken
    /// yet: its reads wait again only when the last one is taken.
    interrupts: u32,
    /// A read of it failed while it was cancelled or interrupted.
    gave_way: bool,
    /// The item plays: it was played, or it took over as the next item. A
    /// new next item no longer supersedes it.
    started: bool,
    /// Its opener is done: the decoder reads it (a seek may interrupt it).
    opened: bool,
}

/// What the engine knows about the reads of one item; see the module docs.
pub(crate) struct Slot {
    state: Mutex<State>,
}

impl Slot {
    /// A slot for an item; `started` for an item that plays at once.
    pub(crate) fn new(started: bool) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                started,
                ..State::default()
            }),
        })
    }

    // The interrupter is called under this lock, so a cancel, an interrupt
    // and its end reach the source in the order they happen here. It only
    // takes the source's own lock, which nothing holds while taking this.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The opened source's interrupter. False when the item was cancelled
    /// meanwhile: its open stops here.
    pub(crate) fn attach(&self, handle: Option<Interrupter>) -> bool {
        let mut st = self.lock();
        if st.cancelled {
            return false;
        }
        st.handle = handle;
        true
    }

    /// The item is open: from now on the decoder reads it.
    pub(crate) fn mark_opened(&self) {
        self.lock().opened = true;
    }

    /// The item is superseded: whatever waits in its reads gives up, now
    /// and later.
    pub(crate) fn cancel(&self) {
        let mut st = self.lock();
        if st.cancelled {
            return;
        }
        st.cancelled = true;
        if let Some(h) = &st.handle {
            h(true);
        }
    }

    /// Cancels the item unless it started; whether it is cancelled now.
    fn cancel_unless_started(&self) -> bool {
        let mut st = self.lock();
        if st.started {
            return false;
        }
        st.cancelled = true;
        if let Some(h) = &st.handle {
            h(true);
        }
        true
    }

    /// A seek on its way: a read waiting in the item gives up until the
    /// decoder takes it ([`Self::end_interrupt`]). Nothing for an item still
    /// opening (its probe must not fail): false then.
    fn interrupt(&self) -> bool {
        let mut st = self.lock();
        if st.cancelled || !st.opened {
            return false;
        }
        st.interrupts += 1;
        if st.interrupts == 1
            && let Some(h) = &st.handle
        {
            h(true);
        }
        true
    }

    /// The decoder took a seek that interrupted the item: when no other
    /// seek is on its way, its reads wait for their bytes again.
    pub(crate) fn end_interrupt(&self) {
        let mut st = self.lock();
        if st.interrupts == 0 {
            return;
        }
        st.interrupts -= 1;
        if st.interrupts == 0
            && !st.cancelled
            && let Some(h) = &st.handle
        {
            h(false);
        }
    }

    /// A seek that interrupted the item is on its way.
    pub(crate) fn interrupted(&self) -> bool {
        self.lock().interrupts > 0
    }

    /// The engine's wrapper of the source saw a read of it give up
    /// ([`std::io::ErrorKind::WouldBlock`]): when the engine had cancelled
    /// or interrupted the item, the read gave way to it.
    pub(crate) fn note_given_way(&self) {
        let mut st = self.lock();
        if st.cancelled || st.interrupts > 0 {
            st.gave_way = true;
        }
    }

    /// Whether a read of the item gave way since the last look.
    pub(crate) fn take_gave_way(&self) -> bool {
        std::mem::take(&mut self.lock().gave_way)
    }

    /// The next item takes over (or its crossfade begins). False when it
    /// was superseded meanwhile: it must not start.
    pub(crate) fn start(&self) -> bool {
        let mut st = self.lock();
        if st.cancelled {
            return false;
        }
        st.started = true;
        true
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.lock().cancelled
    }
}

/// Whether the item behind `slot` was cancelled (no slot: never).
pub(crate) fn cancelled(slot: Option<&Arc<Slot>>) -> bool {
    slot.is_some_and(|s| s.is_cancelled())
}

/// The slots of the items handed to the engine and still held by it (an
/// item the engine let go holds no reference to its slot any more, which
/// is how it is forgotten here). A handful at most: what plays, what is
/// next, what is still opening.
#[derive(Default)]
pub(crate) struct Slots(Mutex<Vec<Arc<Slot>>>);

impl Slots {
    /// The slots, held while a command that changes them is sent: the
    /// commands of two threads reach the decoder in the order their
    /// cancellations happened.
    pub(crate) fn lock(&self) -> Held<'_> {
        let mut all = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        all.retain(|s| Arc::strong_count(s) > 1);
        Held(all)
    }
}

/// [`Slots`] while held.
pub(crate) struct Held<'a>(MutexGuard<'a, Vec<Arc<Slot>>>);

impl Held<'_> {
    /// A play, a stop, the engine's end: everything handed over before is
    /// superseded. `then` is the new item's slot.
    pub(crate) fn cancel_all(&mut self, then: Option<Arc<Slot>>) {
        for s in self.0.drain(..) {
            s.cancel();
        }
        self.0.extend(then);
    }

    /// A new next item (`then`), or none: the next item handed over before
    /// is superseded unless it already took over.
    pub(crate) fn cancel_next(&mut self, then: Option<Arc<Slot>>) {
        self.0.retain(|s| !s.cancel_unless_started());
        self.0.extend(then);
    }

    /// A seek: every opened item may be the one the decoder waits in. The
    /// slots interrupted, for the decoder to end it when it takes the seek.
    pub(crate) fn interrupt(&mut self) -> Vec<Arc<Slot>> {
        self.0
            .iter()
            .filter(|s| s.interrupt())
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU8, Ordering};

    use super::*;

    /// An interrupter that records the last call (0 none, 1 off, 2 on).
    fn probe() -> (Interrupter, Arc<AtomicU8>) {
        let last = Arc::new(AtomicU8::new(0));
        let l = Arc::clone(&last);
        (
            Arc::new(move |on| l.store(if on { 2 } else { 1 }, Ordering::SeqCst)),
            last,
        )
    }

    #[test]
    fn a_cancel_reaches_the_source_also_when_it_comes_before_the_source() {
        let early = Slot::new(true);
        early.cancel();
        let (h, _) = probe();
        assert!(!early.attach(Some(h)), "the open stops");

        let late = Slot::new(true);
        let (h, last) = probe();
        assert!(late.attach(Some(h)));
        late.cancel();
        assert_eq!(last.load(Ordering::SeqCst), 2);
        assert!(!late.start());
    }

    #[test]
    fn a_new_next_item_cancels_only_a_next_item_that_has_not_started() {
        let slots = Slots::default();
        let playing = Slot::new(true);
        let next = Slot::new(false);
        let held = [Arc::clone(&playing), Arc::clone(&next)];
        slots.lock().cancel_all(Some(Arc::clone(&playing)));
        slots.lock().cancel_next(Some(Arc::clone(&next)));
        slots.lock().cancel_next(Some(Slot::new(false)));
        assert!(next.is_cancelled());
        assert!(!playing.is_cancelled());

        // A next item that took over is not superseded by the one after it.
        let took = Slot::new(false);
        slots.lock().cancel_next(Some(Arc::clone(&took)));
        assert!(took.start());
        slots.lock().cancel_next(None);
        assert!(!took.is_cancelled());
        slots.lock().cancel_all(None);
        assert!(took.is_cancelled() && playing.is_cancelled());
        drop(held);
    }

    #[test]
    fn a_second_seek_keeps_the_reads_interrupted_until_it_is_taken_too() {
        let slots = Slots::default();
        let s = Slot::new(true);
        slots.lock().cancel_all(Some(Arc::clone(&s)));
        let (h, last) = probe();
        assert!(s.attach(Some(h)));
        assert!(slots.lock().interrupt().is_empty(), "still opening");
        s.mark_opened();
        let first = slots.lock().interrupt();
        let second = slots.lock().interrupt();
        assert_eq!((first.len(), second.len()), (1, 1));
        assert_eq!(last.load(Ordering::SeqCst), 2);
        first[0].end_interrupt();
        assert_eq!(last.load(Ordering::SeqCst), 2, "the second seek is still on its way");
        assert!(s.interrupted());
        second[0].end_interrupt();
        assert_eq!(last.load(Ordering::SeqCst), 1);
        assert!(!s.interrupted());
    }

    #[test]
    fn only_a_read_failing_while_cancelled_or_interrupted_gave_way() {
        let s = Slot::new(true);
        s.mark_opened();
        s.note_given_way();
        assert!(!s.take_gave_way(), "not raised by the engine");
        assert!(s.interrupt());
        s.note_given_way();
        assert!(s.take_gave_way());
        assert!(!s.take_gave_way());
    }

    #[test]
    fn items_the_engine_let_go_are_forgotten() {
        let slots = Slots::default();
        slots.lock().cancel_all(Some(Slot::new(true)));
        slots.lock().cancel_next(Some(Slot::new(false)));
        assert!(slots.lock().0.is_empty());
    }
}
