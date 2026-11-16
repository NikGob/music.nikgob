//! The allocator's memory a worker kept after a burst.
//!
//! mimalloc keeps each thread's heap to itself: the pages a burst emptied
//! stay with the thread that did it until that thread collects, and no
//! other thread can collect them for it. So the workers that do bursts
//! (the analysis after a batch, the home and recommendation builds, the
//! radio's picks) call [`release_thread_memory`] when they are done and
//! nothing more waits; never the player loop, never per frame.

/// Gives back what this thread's heap holds free (and decommits the freed
/// memory of every arena). A few ms at most; harmless where mimalloc is not
/// the allocator (a test's), as its own heap is then empty.
#[cfg(feature = "mimalloc")]
#[allow(unsafe_code)]
pub fn release_thread_memory() {
    // SAFETY: a plain call into mimalloc; it touches only this thread's
    // heap and the arenas' own locks.
    unsafe { libmimalloc_sys::mi_collect(true) };
}

/// The system allocator (the phone's build) gives back on its own.
#[cfg(not(feature = "mimalloc"))]
pub fn release_thread_memory() {}

#[cfg(test)]
mod tests {
    #[test]
    fn a_release_after_a_burst_is_fine() {
        let burst: Vec<Vec<u8>> = (0..2000).map(|i| vec![7u8; 16 + i * 13]).collect();
        let sum: usize = burst.iter().map(Vec::len).sum();
        drop(burst);
        super::release_thread_memory();
        super::release_thread_memory();
        let again: Vec<u8> = vec![1; sum];
        assert_eq!(again.len(), sum);
    }
}
