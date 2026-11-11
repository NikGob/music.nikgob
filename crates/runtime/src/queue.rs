//! The play queue: what plays now, what comes next, shuffle and repeat.
//!
//! Pure data, no I/O. Every item has a stable entry id so the UI can remove
//! or drag a specific occurrence of a track that appears twice.

use nikgob_core::model::TrackId;
use nikgob_view::Repeat;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: u64,
    pub track: TrackId,
}

#[derive(Debug, Default)]
pub struct Queue {
    /// Items in play order (shuffle reorders this list itself, keeping
    /// the original order to restore when shuffle goes off).
    items: Vec<Entry>,
    original: Vec<Entry>,
    /// Index of the current item.
    pos: Option<usize>,
    pub shuffle: bool,
    pub repeat: Repeat,
    pub context: Option<String>,
    next_id: u64,
    /// Bumped whenever the whole queue is replaced ([`Self::set`],
    /// [`Self::restore`]): a radio knows its queue is gone.
    generation: u64,
}

/// A queue as saved between runs (entry ids are not kept: they are only
/// meaningful within one run).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    /// Tracks in play order.
    pub items: Vec<TrackId>,
    /// The order to go back to when shuffle goes off, as indices into `items`.
    pub original: Vec<usize>,
    pub pos: Option<usize>,
    pub shuffle: bool,
    /// 0 off, 1 all, 2 one.
    pub repeat: u8,
    pub context: Option<String>,
}

impl Queue {
    pub fn snapshot(&self) -> Snapshot {
        let original = self
            .original
            .iter()
            .filter_map(|o| self.items.iter().position(|e| e.id == o.id))
            .collect();
        Snapshot {
            items: self.items.iter().map(|e| e.track).collect(),
            original,
            pos: self.pos,
            shuffle: self.shuffle,
            repeat: match self.repeat {
                Repeat::Off => 0,
                Repeat::All => 1,
                Repeat::One => 2,
            },
            context: self.context.clone(),
        }
    }

    /// Replaces the queue with a saved one (fresh entry ids).
    pub fn restore(&mut self, s: &Snapshot) {
        self.items = s.items.iter().map(|&t| self.entry(t)).collect();
        let original: Vec<Entry> = s.original.iter().filter_map(|&i| self.items.get(i).copied()).collect();
        // A damaged file: fall back to the play order.
        self.original = if original.len() == self.items.len() { original } else { self.items.clone() };
        self.pos = s.pos.filter(|&p| p < self.items.len());
        self.restore_modes(s);
        self.context = s.context.clone();
        self.generation += 1;
    }

    /// Only shuffle and repeat of a saved queue (they are the player's,
    /// kept even when the saved queue was empty).
    pub fn restore_modes(&mut self, s: &Snapshot) {
        self.shuffle = s.shuffle;
        self.repeat = match s.repeat {
            1 => Repeat::All,
            2 => Repeat::One,
            _ => Repeat::Off,
        };
    }

    /// See the field: changes when the queue is replaced.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn entry(&mut self, track: TrackId) -> Entry {
        self.next_id += 1;
        Entry {
            id: self.next_id,
            track,
        }
    }

    /// Replaces the queue and starts at `start`. With shuffle on, the
    /// starting track plays first and all the others follow shuffled
    /// (shuffle stays as it was: a new queue never turns it off).
    pub fn set(&mut self, tracks: &[TrackId], start: usize, context: Option<String>) {
        self.set_ordered(tracks, start, context);
        if self.shuffle {
            self.shuffle_all_but_current();
        }
    }

    /// [`Self::set`] for a whole list the user did not start at a track:
    /// with shuffle on a random track starts, else the first one.
    pub fn set_from(&mut self, tracks: &[TrackId], start: Option<usize>, context: Option<String>) {
        let start = match start {
            Some(s) => s,
            None if self.shuffle && !tracks.is_empty() => fastrand::usize(..tracks.len()),
            None => 0,
        };
        self.set(tracks, start, context);
    }

    /// Replaces the queue keeping the given order even with shuffle on (a
    /// radio's picks are a path, not a list to shuffle). Shuffle itself
    /// stays on: turning it off and on again shuffles what is left.
    pub fn set_ordered(&mut self, tracks: &[TrackId], start: usize, context: Option<String>) {
        let items: Vec<Entry> = tracks.iter().map(|&t| self.entry(t)).collect();
        self.original = items.clone();
        self.items = items;
        self.context = context;
        self.generation += 1;
        self.pos = (!self.items.is_empty()).then(|| start.min(self.items.len() - 1));
    }

    /// A new list is about to replace the queue (an album, a playlist, a
    /// mix, a radio, a search result): shuffle stays as the user set it
    /// when `keep` («Перемешивание остаётся включённым»), else it goes off.
    pub fn new_list(&mut self, keep: bool) {
        if !keep {
            self.shuffle = false;
        }
    }

    /// Replaces the queue with a radio's picks (a radio, the daylist):
    /// in their order ([`Self::set_ordered`]), or with `shuffled`
    /// («Перемешивать радио и автовоспроизведение») and shuffle on as a
    /// whole list under shuffle ([`Self::set_from`]).
    pub fn set_path(&mut self, tracks: &[TrackId], context: Option<String>, shuffled: bool) {
        if shuffled && self.shuffle {
            self.set_from(tracks, None, context);
        } else {
            self.set_ordered(tracks, 0, context);
        }
    }

    /// More picks of a radio or autoplay: at the end in their order, or
    /// with `shuffled` and shuffle on among the upcoming ones
    /// ([`Self::enqueue_shuffled`]). The new entries, in the given order.
    pub fn enqueue_path(&mut self, tracks: &[TrackId], shuffled: bool) -> Vec<Entry> {
        if shuffled {
            self.enqueue_shuffled(tracks)
        } else {
            self.enqueue(tracks)
        }
    }

    /// Every entry in the order the queue was started with (what shuffle
    /// off goes back to), whatever the play order is now.
    pub fn context_order(&self) -> &[Entry] {
        &self.original
    }

    /// Every entry of `from` plays `to` instead (the track was merged into
    /// another one). Returns whether any did.
    pub fn retarget(&mut self, from: TrackId, to: TrackId) -> bool {
        let mut any = false;
        for e in self.items.iter_mut().chain(self.original.iter_mut()) {
            if e.track == from {
                e.track = to;
                any = true;
            }
        }
        any
    }

    pub fn current(&self) -> Option<Entry> {
        self.pos.and_then(|p| self.items.get(p).copied())
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Items after the current one, in play order.
    pub fn upcoming(&self) -> &[Entry] {
        match self.pos {
            Some(p) => &self.items[(p + 1).min(self.items.len())..],
            None => &self.items,
        }
    }

    /// Items before the current one, in play order: what has played (or
    /// was skipped past) and what "previous" goes back to.
    pub fn history(&self) -> &[Entry] {
        match self.pos {
            Some(p) => &self.items[..p.min(self.items.len())],
            None => &[],
        }
    }

    /// What plays after the current item when it ends by itself.
    pub fn peek_auto_next(&self) -> Option<Entry> {
        let p = self.pos?;
        match self.repeat {
            Repeat::One => self.items.get(p).copied(),
            Repeat::All if p + 1 >= self.items.len() => self.items.first().copied(),
            _ => self.items.get(p + 1).copied(),
        }
    }

    /// Moves on when the current item ends by itself (respects repeat one).
    pub fn advance_auto(&mut self) -> Option<Entry> {
        let next = self.peek_auto_next();
        if let Some(n) = next {
            self.pos = self.items.iter().position(|e| e.id == n.id);
        } else {
            self.pos = None;
        }
        next
    }

    /// The user pressed "next": repeat-one does not hold it back.
    pub fn skip_forward(&mut self) -> Option<Entry> {
        let p = self.pos?;
        if p + 1 < self.items.len() {
            self.pos = Some(p + 1);
        } else if self.repeat != Repeat::Off && !self.items.is_empty() {
            self.pos = Some(0);
        } else {
            return None;
        }
        self.current()
    }

    pub fn previous(&mut self) -> Option<Entry> {
        let p = self.pos?;
        if p > 0 {
            self.pos = Some(p - 1);
        } else if self.repeat == Repeat::All && !self.items.is_empty() {
            self.pos = Some(self.items.len() - 1);
        }
        self.current()
    }

    /// Jumps to an entry anywhere in the queue.
    pub fn jump(&mut self, entry: u64) -> Option<Entry> {
        self.pos = Some(self.items.iter().position(|e| e.id == entry)?);
        self.current()
    }

    /// Adds the tracks at the end. The new entries, in the given order.
    pub fn enqueue(&mut self, tracks: &[TrackId]) -> Vec<Entry> {
        let first_new = self.items.len();
        let entries: Vec<Entry> = tracks.iter().map(|&t| self.entry(t)).collect();
        self.items.extend(entries.iter().copied());
        self.original.extend(entries.iter().copied());
        // Nothing is current (empty queue, or it has played out): the first
        // added track becomes current, not a track that already played.
        if self.pos.is_none() && self.items.len() > first_new {
            self.pos = Some(first_new);
        }
        entries
    }

    /// More of the list the queue plays (the rest of an album's tracklist):
    /// at the end with shuffle off, each at a random place among the
    /// upcoming ones with shuffle on. Shuffle off brings them back at the
    /// end in the given order. The new entries, in the given order.
    pub fn enqueue_shuffled(&mut self, tracks: &[TrackId]) -> Vec<Entry> {
        if !self.shuffle {
            return self.enqueue(tracks);
        }
        let entries: Vec<Entry> = tracks.iter().map(|&t| self.entry(t)).collect();
        self.original.extend(entries.iter().copied());
        match self.pos {
            Some(p) => {
                for &e in &entries {
                    let at = fastrand::usize(p + 1..=self.items.len());
                    self.items.insert(at, e);
                }
            }
            None => {
                let first_new = self.items.len();
                let mut shuffled = entries.clone();
                fastrand::shuffle(&mut shuffled);
                self.items.extend(shuffled);
                if self.items.len() > first_new {
                    self.pos = Some(first_new);
                }
            }
        }
        entries
    }

    pub fn play_next(&mut self, tracks: &[TrackId]) {
        let at = self.pos.map_or(0, |p| p + 1);
        let entries: Vec<Entry> = tracks.iter().map(|&t| self.entry(t)).collect();
        let oat = self
            .current()
            .and_then(|c| self.original.iter().position(|e| e.id == c.id))
            .map_or(0, |p| p + 1);
        self.items.splice(at..at, entries.iter().copied());
        self.original.splice(oat..oat, entries);
        if self.pos.is_none() && !self.items.is_empty() {
            self.pos = Some(0);
        }
    }

    /// Removes an upcoming (or any non-current) entry.
    pub fn remove(&mut self, entry: u64) -> bool {
        let Some(i) = self.items.iter().position(|e| e.id == entry) else {
            return false;
        };
        if Some(i) == self.pos {
            return false;
        }
        self.items.remove(i);
        self.original.retain(|e| e.id != entry);
        if let Some(p) = self.pos
            && i < p
        {
            self.pos = Some(p - 1);
        }
        true
    }

    /// Moves `entry` right after (or before) `anchor`, anywhere in the queue:
    /// a played track can go back among the upcoming ones and the other way
    /// round. The current item itself never moves and stays current.
    pub fn move_next_to(&mut self, entry: u64, anchor: u64, after: bool) -> bool {
        let current = self.current().map(|c| c.id);
        if entry == anchor || current == Some(entry) {
            return false;
        }
        let Some(i) = self.items.iter().position(|e| e.id == entry) else {
            return false;
        };
        if !self.items.iter().any(|e| e.id == anchor) {
            return false;
        }
        let e = self.items.remove(i);
        let j = self.items.iter().position(|e| e.id == anchor).unwrap_or(self.items.len());
        self.items.insert(if after { j + 1 } else { j }, e);
        self.pos = current.and_then(|c| self.items.iter().position(|e| e.id == c));
        if !self.shuffle {
            // `original` is the order shuffle-off restores: keep the edit.
            self.original.clone_from(&self.items);
        } else if let Some(i) = self.original.iter().position(|e| e.id == entry) {
            // Shuffled: the order shuffle-off restores keeps the entry next
            // to the same anchor (a track put right after the current one
            // still comes right after it).
            let e = self.original.remove(i);
            let at = match self.original.iter().position(|o| o.id == anchor) {
                Some(j) if after => j + 1,
                Some(j) => j,
                None => i.min(self.original.len()),
            };
            self.original.insert(at, e);
        }
        true
    }

    /// Drops everything except the current item.
    pub fn clear_upcoming(&mut self) {
        match self.current() {
            Some(c) => {
                self.items = vec![c];
                self.original = vec![c];
                self.pos = Some(0);
            }
            None => {
                self.items.clear();
                self.original.clear();
            }
        }
    }

    /// Puts back what [`Self::clear_upcoming`] took out (fresh entry ids):
    /// `before` ahead of the current item, `after` behind it, each in its
    /// order; the current item keeps its entry. With nothing current they
    /// are the whole queue again.
    pub fn restore_around(&mut self, before: &[TrackId], after: &[TrackId]) {
        let b: Vec<Entry> = before.iter().map(|&t| self.entry(t)).collect();
        let a: Vec<Entry> = after.iter().map(|&t| self.entry(t)).collect();
        match self.current() {
            Some(c) => {
                let p = self.pos.unwrap_or(0);
                self.items.splice(p..p, b.iter().copied());
                let p = p + b.len();
                self.pos = Some(p);
                self.items.splice(p + 1..p + 1, a.iter().copied());
                let o = self.original.iter().position(|e| e.id == c.id).unwrap_or(0);
                self.original.splice(o..o, b.iter().copied());
                let o = (o + b.len() + 1).min(self.original.len());
                self.original.splice(o..o, a);
            }
            None => {
                self.items.extend(b.iter().chain(&a).copied());
                self.original.extend(b.into_iter().chain(a));
            }
        }
    }

    /// Shuffle on: what plays now stays current, what has played stays
    /// behind it, the rest is shuffled. Off: the order the queue was
    /// started with comes back, the current track stays current and "next"
    /// goes on from its place in that order.
    pub fn set_shuffle(&mut self, on: bool) {
        if on == self.shuffle {
            return;
        }
        self.shuffle = on;
        if on {
            let from = self.pos.map_or(0, |p| (p + 1).min(self.items.len()));
            fastrand::shuffle(&mut self.items[from..]);
        } else {
            let cur = self.current();
            self.items = self.original.clone();
            self.pos = cur.and_then(|c| self.items.iter().position(|e| e.id == c.id));
        }
    }

    /// A new queue under shuffle: the starting item moves to the front so
    /// it plays first, all the others follow shuffled.
    fn shuffle_all_but_current(&mut self) {
        let cur = self.current();
        let mut rest: Vec<Entry> = self
            .items
            .iter()
            .copied()
            .filter(|e| Some(e.id) != cur.map(|c| c.id))
            .collect();
        fastrand::shuffle(&mut rest);
        self.items = cur.into_iter().chain(rest).collect();
        self.pos = cur.map(|_| 0);
    }

    pub fn tracks(&self) -> Vec<TrackId> {
        self.items.iter().map(|e| e.track).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<TrackId> {
        (0..n).map(|_| TrackId::new()).collect()
    }

    #[test]
    fn plays_through_and_stops() {
        let t = ids(3);
        let mut q = Queue::default();
        q.set(&t, 0, None);
        assert_eq!(q.current().map(|e| e.track), Some(t[0]));
        assert_eq!(q.advance_auto().map(|e| e.track), Some(t[1]));
        assert_eq!(q.advance_auto().map(|e| e.track), Some(t[2]));
        assert_eq!(q.advance_auto(), None);
    }

    #[test]
    fn repeat_modes() {
        let t = ids(2);
        let mut q = Queue::default();
        q.set(&t, 1, None);
        q.repeat = Repeat::All;
        assert_eq!(q.advance_auto().map(|e| e.track), Some(t[0]));
        q.repeat = Repeat::One;
        assert_eq!(q.advance_auto().map(|e| e.track), Some(t[0]));
        // Pressing next skips even with repeat one.
        assert_eq!(q.skip_forward().map(|e| e.track), Some(t[1]));
    }

    #[test]
    fn shuffle_keeps_current_and_restores_order() {
        let t = ids(20);
        let mut q = Queue::default();
        q.set(&t, 5, None);
        q.set_shuffle(true);
        assert_eq!(q.current().map(|e| e.track), Some(t[5]));
        assert_eq!(q.history().len(), 5);
        assert_eq!(q.upcoming().len(), 14);
        q.set_shuffle(false);
        assert_eq!(q.current().map(|e| e.track), Some(t[5]));
        assert_eq!(q.tracks(), t);
    }

    #[test]
    fn manual_reorder_survives_shuffle_toggle() {
        let t = ids(4);
        let mut q = Queue::default();
        q.set(&t, 0, None);
        let up: Vec<u64> = q.upcoming().iter().map(|e| e.id).collect();
        assert!(q.move_next_to(up[2], up[0], false)); // t3 first
        q.set_shuffle(true);
        q.set_shuffle(false);
        assert_eq!(q.tracks(), vec![t[0], t[3], t[1], t[2]]);
    }

    #[test]
    fn enqueue_after_the_queue_played_out_starts_with_the_new_track() {
        let t = ids(2);
        let mut q = Queue::default();
        q.set(&t, 0, None);
        assert!(q.advance_auto().is_some());
        assert_eq!(q.advance_auto(), None);
        let extra = ids(1);
        q.enqueue(&extra);
        assert_eq!(q.current().map(|e| e.track), Some(extra[0]));
    }

    #[test]
    fn history_is_what_came_before_and_jumping_back_works() {
        let t = ids(4);
        let mut q = Queue::default();
        q.set(&t, 0, None);
        assert!(q.history().is_empty());
        q.advance_auto();
        q.skip_forward();
        let back: Vec<TrackId> = q.history().iter().map(|e| e.track).collect();
        assert_eq!(back, vec![t[0], t[1]]);
        let first = q.history()[0].id;
        assert_eq!(q.jump(first).map(|e| e.track), Some(t[0]));
        assert!(q.history().is_empty());
        assert_eq!(q.upcoming().len(), 3);
        // Played out: nothing is current, so nothing is "before" it.
        q.set(&t, 3, None);
        assert_eq!(q.history().len(), 3);
        assert_eq!(q.advance_auto(), None);
        assert!(q.history().is_empty());
    }

    #[test]
    fn played_tracks_can_be_moved_and_removed_like_upcoming_ones() {
        let t = ids(5);
        let mut q = Queue::default();
        q.set(&t, 2, None);
        let all: Vec<u64> = q.history().iter().chain(q.current().iter()).chain(q.upcoming()).map(|e| e.id).collect();
        let cur = all[2];
        // A played track back after the current one: it plays next.
        assert!(q.move_next_to(all[0], cur, true));
        assert_eq!(q.current().map(|e| e.id), Some(cur));
        assert_eq!(q.history().iter().map(|e| e.track).collect::<Vec<_>>(), vec![t[1]]);
        assert_eq!(q.upcoming()[0].track, t[0]);
        // An upcoming one into the history, before the current.
        assert!(q.move_next_to(all[4], cur, false));
        assert_eq!(q.history().iter().map(|e| e.track).collect::<Vec<_>>(), vec![t[1], t[4]]);
        assert_eq!(q.current().map(|e| e.track), Some(t[2]));
        // Reorder inside the history, then drop one of it.
        assert!(q.move_next_to(all[4], all[1], false));
        assert_eq!(q.history().iter().map(|e| e.track).collect::<Vec<_>>(), vec![t[4], t[1]]);
        assert!(q.remove(all[1]));
        assert_eq!(q.history().iter().map(|e| e.track).collect::<Vec<_>>(), vec![t[4]]);
        assert_eq!(q.current().map(|e| e.id), Some(cur));
        // The current item never moves.
        assert!(!q.move_next_to(cur, all[4], false));
        assert!(!q.move_next_to(all[4], all[4], true));
    }

    #[test]
    fn a_saved_queue_comes_back_the_same() {
        let t = ids(5);
        let mut q = Queue::default();
        q.set(&t, 2, Some("Альбом".into()));
        q.set_shuffle(true);
        q.repeat = Repeat::All;
        let saved = q.snapshot();
        let json = serde_json::to_string(&saved).unwrap();
        let mut back = Queue::default();
        back.restore(&serde_json::from_str(&json).unwrap());
        assert_eq!(back.snapshot(), saved);
        assert_eq!(back.current().map(|e| e.track), q.current().map(|e| e.track));
        back.set_shuffle(false);
        q.set_shuffle(false);
        assert_eq!(back.tracks(), q.tracks());
    }

    fn tracks_of(list: &[Entry]) -> Vec<TrackId> {
        list.iter().map(|e| e.track).collect()
    }

    /// The same tracks, in any order.
    fn same_tracks(a: &[TrackId], b: &[TrackId]) -> bool {
        a.len() == b.len() && a.iter().all(|x| b.contains(x))
    }

    fn entry_of(q: &Queue, t: TrackId) -> u64 {
        q.history().iter().chain(q.current().iter()).chain(q.upcoming()).find(|e| e.track == t).unwrap().id
    }

    #[test]
    fn a_new_queue_keeps_shuffle_on_and_plays_shuffled_from_the_picked_track() {
        let t = ids(30);
        let mut q = Queue::default();
        q.set_shuffle(true);
        q.set(&t, 7, Some("Альбом".into()));
        assert!(q.shuffle);
        assert_eq!(q.current().map(|e| e.track), Some(t[7]));
        assert!(q.history().is_empty());
        let others: Vec<TrackId> = t.iter().copied().filter(|&x| x != t[7]).collect();
        let up = tracks_of(q.upcoming());
        assert!(same_tracks(&up, &others));
        assert_ne!(up, others, "the rest is shuffled");
        // Another list after it: shuffle is still on.
        let u = ids(5);
        q.set(&u, 0, Some("Плейлист".into()));
        assert!(q.shuffle);
        assert_eq!(q.current().map(|e| e.track), Some(u[0]));
        assert!(same_tracks(&q.tracks(), &u));
    }

    #[test]
    fn a_whole_list_starts_anywhere_under_shuffle_and_at_the_first_track_without() {
        let t = ids(12);
        let mut q = Queue::default();
        q.set_shuffle(true);
        q.set_from(&t, None, None);
        let cur = q.current().map(|e| e.track).unwrap();
        assert!(t.contains(&cur));
        assert!(q.history().is_empty());
        assert!(same_tracks(&q.tracks(), &t));
        // A picked track starts even under shuffle.
        q.set_from(&t, Some(4), None);
        assert_eq!(q.current().map(|e| e.track), Some(t[4]));
        q.set_shuffle(false);
        q.set_from(&t, None, None);
        assert_eq!(q.current().map(|e| e.track), Some(t[0]));
        assert_eq!(q.tracks(), t);
    }

    #[test]
    fn shuffle_off_goes_on_from_the_current_tracks_place_in_the_list() {
        let t = ids(10);
        let mut q = Queue::default();
        q.set_shuffle(true);
        q.set(&t, 0, None);
        let seventh = entry_of(&q, t[6]);
        q.jump(seventh);
        q.set_shuffle(false);
        assert!(!q.shuffle);
        assert_eq!(q.tracks(), t);
        assert_eq!(q.current().map(|e| e.id), Some(seventh));
        assert_eq!(q.skip_forward().map(|e| e.track), Some(t[7]));
        assert_eq!(q.history().len(), 7);
    }

    #[test]
    fn hand_added_tracks_keep_their_place_when_shuffle_goes_off() {
        let t = ids(10);
        let (next, last) = (ids(1), ids(1));
        let mut q = Queue::default();
        q.set_shuffle(true);
        q.set(&t, 0, None);
        q.jump(entry_of(&q, t[4]));
        q.play_next(&next);
        q.enqueue(&last);
        assert_eq!(q.upcoming().first().map(|e| e.track), Some(next[0]));
        assert_eq!(q.upcoming().last().map(|e| e.track), Some(last[0]));
        q.set_shuffle(false);
        let mut want = t[..5].to_vec();
        want.push(next[0]);
        want.extend_from_slice(&t[5..]);
        want.push(last[0]);
        assert_eq!(q.tracks(), want);
        assert_eq!(q.current().map(|e| e.track), Some(t[4]));
        assert_eq!(q.skip_forward().map(|e| e.track), Some(next[0]));
    }

    #[test]
    fn shuffle_on_mid_list_keeps_what_played_and_shuffles_the_rest() {
        let t = ids(30);
        let mut q = Queue::default();
        q.set(&t, 0, None);
        for _ in 0..5 {
            q.skip_forward();
        }
        q.set_shuffle(true);
        assert_eq!(tracks_of(q.history()), t[..5].to_vec());
        assert_eq!(q.current().map(|e| e.track), Some(t[5]));
        let up = tracks_of(q.upcoming());
        assert!(same_tracks(&up, &t[6..]));
        assert_ne!(up, t[6..].to_vec(), "the rest is shuffled");
        q.set_shuffle(false);
        assert_eq!(q.tracks(), t);
        assert_eq!(q.current().map(|e| e.track), Some(t[5]));
    }

    #[test]
    fn a_continuation_after_the_end_keeps_its_order_through_shuffle_off() {
        let t = ids(10);
        let more = ids(5);
        let mut q = Queue::default();
        q.set_shuffle(true);
        q.set(&t, 0, None);
        // Autoplay / a radio goes on after the list.
        q.enqueue(&more);
        let up = tracks_of(q.upcoming());
        assert_eq!(up[up.len() - 5..].to_vec(), more);
        q.jump(entry_of(&q, more[2]));
        q.set_shuffle(false);
        let mut want = t.clone();
        want.extend_from_slice(&more);
        assert_eq!(q.tracks(), want);
        assert_eq!(q.current().map(|e| e.track), Some(more[2]));
        assert_eq!(q.skip_forward().map(|e| e.track), Some(more[3]));
    }

    #[test]
    fn a_radio_keeps_its_order_under_shuffle_and_so_does_what_joins_it() {
        let r = ids(6);
        let new = ids(1);
        let mut q = Queue::default();
        q.set_shuffle(true);
        q.set_ordered(&r, 0, Some("радио".into()));
        assert!(q.shuffle);
        assert_eq!(q.tracks(), r);
        // New music spliced in after the third pick.
        q.enqueue(&new);
        let entry = q.upcoming().last().unwrap().id;
        let anchor = entry_of(&q, r[2]);
        assert!(q.move_next_to(entry, anchor, true));
        let want = vec![r[0], r[1], r[2], new[0], r[3], r[4], r[5]];
        assert_eq!(q.tracks(), want);
        q.set_shuffle(false);
        assert_eq!(q.tracks(), want);
        // The order the queue started with, the splice in it.
        assert_eq!(tracks_of(q.context_order()), want);
    }

    #[test]
    fn the_rest_of_an_album_joins_the_shuffled_rest() {
        let t = ids(10);
        let more = ids(5);
        let mut q = Queue::default();
        q.set_shuffle(true);
        q.set(&t, 3, None);
        q.enqueue_shuffled(&more);
        assert_eq!(q.len(), 15);
        assert_eq!(q.current().map(|e| e.track), Some(t[3]));
        assert!(more.iter().all(|m| q.upcoming().iter().any(|e| e.track == *m)));
        q.set_shuffle(false);
        let mut want = t.clone();
        want.extend_from_slice(&more);
        assert_eq!(q.tracks(), want);
        assert_eq!(q.current().map(|e| e.track), Some(t[3]));
        // Without shuffle they simply come at the end.
        let extra = ids(2);
        q.enqueue_shuffled(&extra);
        want.extend_from_slice(&extra);
        assert_eq!(q.tracks(), want);
    }

    #[test]
    fn shuffle_and_repeat_come_back_without_a_saved_queue() {
        let mut q = Queue::default();
        q.set_shuffle(true);
        q.repeat = Repeat::All;
        let saved = q.snapshot();
        assert!(saved.items.is_empty());
        let mut back = Queue::default();
        back.restore_modes(&saved);
        assert!(back.shuffle);
        assert_eq!(back.repeat, Repeat::All);
    }

    #[test]
    fn a_new_list_keeps_shuffle_only_with_the_setting() {
        let t = ids(30);
        // «Перемешивание остаётся включённым» (the default): on stays on.
        let mut q = Queue::default();
        q.set_shuffle(true);
        q.new_list(true);
        q.set_from(&t, None, None);
        assert!(q.shuffle);
        assert!(same_tracks(&q.tracks(), &t));
        // Off: a new list turns shuffle off and plays from the first track.
        q.new_list(false);
        q.set_from(&t, None, None);
        assert!(!q.shuffle);
        assert_eq!(q.tracks(), t);
        assert_eq!(q.current().map(|e| e.track), Some(t[0]));
        // «Перемешать» still turns it on (as `play_list` does).
        q.new_list(false);
        q.shuffle = true;
        q.set_from(&t, None, None);
        assert!(q.shuffle);
        assert!(same_tracks(&q.tracks(), &t));
        assert_ne!(q.tracks(), t, "the list is shuffled");
        // Turned off by hand, the list's order comes back in both modes.
        let cur = q.current().unwrap();
        q.set_shuffle(false);
        assert_eq!(q.tracks(), t);
        assert_eq!(q.current(), Some(cur));
        q.new_list(true);
        q.set_shuffle(true);
        q.set(&t, 3, None);
        q.set_shuffle(false);
        assert_eq!(q.tracks(), t);
        assert_eq!(q.current().map(|e| e.track), Some(t[3]));
    }

    #[test]
    fn radio_picks_are_shuffled_only_with_the_setting() {
        let r = ids(30);
        let more = ids(5);
        let mut q = Queue::default();
        q.set_shuffle(true);
        // Off (the default): the picks and what joins them keep their order.
        q.set_path(&r, Some("радио".into()), false);
        assert!(q.shuffle);
        assert_eq!(q.tracks(), r);
        let added = q.enqueue_path(&more, false);
        assert_eq!(tracks_of(&added), more);
        let up = tracks_of(q.upcoming());
        assert_eq!(up[up.len() - 5..].to_vec(), more);
        // On: the picks play shuffled, the order they came in is kept for
        // shuffle off.
        q.set_path(&r, Some("радио".into()), true);
        assert!(q.shuffle);
        assert!(q.history().is_empty());
        assert!(same_tracks(&q.tracks(), &r));
        assert_ne!(q.tracks(), r, "the picks are shuffled");
        assert_eq!(tracks_of(q.context_order()), r);
        let added = q.enqueue_path(&more, true);
        assert_eq!(tracks_of(&added), more, "the new entries in the given order");
        assert!(added.iter().all(|a| q.upcoming().iter().any(|e| e.id == a.id)));
        let cur = q.current().unwrap();
        q.set_shuffle(false);
        let mut want = r.clone();
        want.extend_from_slice(&more);
        assert_eq!(q.tracks(), want);
        assert_eq!(q.current(), Some(cur));
        // Shuffle off: the setting alone shuffles nothing.
        q.set_path(&r, Some("радио".into()), true);
        assert_eq!(q.tracks(), r);
        let added = q.enqueue_path(&more, true);
        assert_eq!(tracks_of(&added), more);
        assert_eq!(q.tracks()[r.len()..].to_vec(), more);
    }

    #[test]
    fn edit_upcoming() {
        let t = ids(4);
        let mut q = Queue::default();
        q.set(&t, 0, None);
        let up: Vec<u64> = q.upcoming().iter().map(|e| e.id).collect();
        assert!(q.remove(up[0]));
        assert_eq!(q.upcoming().len(), 2);
        assert!(q.move_next_to(up[2], up[1], false));
        assert_eq!(q.upcoming()[0].track, t[3]);
        let extra = ids(1);
        q.play_next(&extra);
        assert_eq!(q.upcoming()[0].track, extra[0]);
        q.clear_upcoming();
        assert!(q.upcoming().is_empty());
        assert_eq!(q.current().map(|e| e.track), Some(t[0]));
    }

    /// «Отменить» after «Очистить»: what was before and after the current
    /// track comes back in its order, the current one stays current.
    #[test]
    fn a_cleared_queue_comes_back() {
        let t = ids(5);
        let mut q = Queue::default();
        q.set(&t, 2, None);
        let cur = q.current().map(|e| e.id);
        let before: Vec<TrackId> = q.history().iter().map(|e| e.track).collect();
        let after: Vec<TrackId> = q.upcoming().iter().map(|e| e.track).collect();
        q.clear_upcoming();
        q.restore_around(&before, &after);
        assert_eq!(q.current().map(|e| e.id), cur);
        assert_eq!(q.tracks(), t);
        assert_eq!(q.upcoming().iter().map(|e| e.track).collect::<Vec<_>>(), &t[3..]);
        // Shuffle off later keeps them in their places too.
        q.set_shuffle(true);
        q.set_shuffle(false);
        assert_eq!(q.tracks(), t);
    }
}
