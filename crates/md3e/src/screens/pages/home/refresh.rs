//! Cards of Главная that change when the page is built anew: the page of
//! the last run giving way to this run's, the mixes and the tiles of the
//! day rebuilt, more cards at the end of a row.
//!
//! Each row (and the shortcuts) remembers what its cards showed ([`track`],
//! by a key of what a card looks like: its cover, its name). A card whose
//! key changed swaps: the old card fades out where it stands while the
//! new one fades in and grows from [`FROM_SCALE`] on the default spatial
//! spring (its small overshoot is the bounce), the cards along the row one
//! after another, [`STAGGER`] apart. A card that did not change stays
//! still, and nothing moves the layout: the new card takes the old one's
//! place. With less motion it is the crossfade alone.

use egui::{Context, Id, Rect, Ui, UiBuilder};

use super::super::cards;
use crate::{Theme, components::anim};

/// The changed cards start this far apart along the row…
pub const STAGGER: f64 = 0.04;
/// …at most this many places apart (a long row does not wait).
pub const STAGGER_PLACES: usize = 8;
/// A new card grows in from this scale.
pub const FROM_SCALE: f32 = 0.92;
/// A swap is over this long after its turn came.
const SWAP_LONG: f64 = 1.0;

/// A card that changed: its place in the row, when it changed, the card it
/// had before (none: a card that was not there).
#[derive(Clone, Debug)]
struct Swap<T> {
    slot: usize,
    at: f64,
    old: Option<T>,
}

/// What a row showed, by its cards' keys, and the swaps going on.
#[derive(Clone, Debug)]
struct Seen<T> {
    /// Drawn before: a row seen the first time comes in with the page.
    init: bool,
    keys: Vec<u64>,
    items: Vec<T>,
    swaps: Vec<Swap<T>>,
}

impl<T> Default for Seen<T> {
    fn default() -> Self {
        Self { init: false, keys: Vec::new(), items: Vec::new(), swaps: Vec::new() }
    }
}

impl<T: Clone> Seen<T> {
    /// The row shows `keys` at `now` (`items` the cards themselves, made
    /// only when something changed); the swaps still going on.
    fn update(&mut self, keys: Vec<u64>, items: impl FnOnce() -> Vec<T>, now: f64) -> Vec<Swap<T>> {
        if !self.init {
            self.init = true;
            self.keys = keys;
            self.items = items();
        } else if self.keys != keys {
            for (slot, key) in keys.iter().enumerate() {
                if self.keys.get(slot) != Some(key) {
                    let old = self.items.get(slot).cloned();
                    self.swaps.retain(|s| s.slot != slot);
                    self.swaps.push(Swap { slot, at: now, old });
                }
            }
            let n = keys.len();
            self.swaps.retain(|s| s.slot < n);
            self.keys = keys;
            self.items = items();
        }
        let long = SWAP_LONG + STAGGER * STAGGER_PLACES as f64;
        self.swaps.retain(|s| now - s.at < long);
        self.swaps.clone()
    }
}

/// The swaps of a row in a frame ([`track`]).
pub struct Changes<T> {
    swaps: Vec<Swap<T>>,
    now: f64,
}

/// The row `id` shows the cards of `keys` (see the module docs); `items`
/// makes the cards to keep for a later swap, only when something changed.
/// A row that is not `live` (the settings' copy of the page) never swaps.
pub fn track<T: Clone + Send + Sync + 'static>(ctx: &Context, id: Id, live: bool, keys: Vec<u64>, items: impl FnOnce() -> Vec<T>) -> Changes<T> {
    let now = anim::now(ctx);
    if !live {
        return Changes { swaps: Vec::new(), now };
    }
    let swaps = ctx.data_mut(|d| d.get_temp_mut_or_default::<Seen<T>>(id.with("home-refresh")).update(keys, items, now));
    if !swaps.is_empty() {
        ctx.request_repaint();
    }
    Changes { swaps, now }
}

/// The key of what a card shows.
pub fn key(what: impl std::hash::Hash) -> u64 {
    use std::hash::Hasher as _;
    let mut h = std::hash::DefaultHasher::new();
    what.hash(&mut h);
    h.finish()
}

/// The place of a card in `cell` along a row whose cards stand `pitch`
/// apart from `left` (the order the changed cards come in).
pub fn place(cell: Rect, left: f32, pitch: f32) -> usize {
    if pitch <= 0.0 {
        return 0;
    }
    ((cell.left() - left) / pitch).round().max(0.0) as usize
}

/// How far a swap is `t` seconds after its turn came: the new card's
/// opacity and scale.
pub fn look(t: f32, reduced: bool, effects: nikgob_motion::Spring, spatial: nikgob_motion::Spring) -> (f32, f32) {
    if t <= 0.0 {
        return (0.0, if reduced { 1.0 } else { FROM_SCALE });
    }
    let alpha = (1.0 - effects.solve(1.0, 0.0, t).0).clamp(0.0, 1.0);
    let scale = if reduced { 1.0 } else { 1.0 - (1.0 - FROM_SCALE) * spatial.solve(1.0, 0.0, t).0 };
    (alpha, scale)
}

impl<T> Changes<T> {
    /// Draws the card of `slot` in `cell`, `place` in the row's order:
    /// `draw` with `None` draws the card as it is now, with the old card
    /// while it fades out under the new one (its clicks go nowhere: the
    /// caller drops what it says).
    pub fn card(&self, ui: &mut Ui, slot: usize, cell: Rect, place: usize, mut draw: impl FnMut(&mut Ui, Option<&T>)) {
        let Some(swap) = self.swaps.iter().find(|s| s.slot == slot) else {
            draw(ui, None);
            return;
        };
        let theme = Theme::get(ui.ctx());
        let wait = STAGGER * place.min(STAGGER_PLACES) as f64;
        let t = (self.now - swap.at - wait) as f32;
        let (alpha, scale) = look(t, theme.reduced_motion, theme.motion.default_effects, theme.motion.default_spatial);
        if alpha < 1.0
            && let Some(old) = &swap.old
        {
            let mut child = ui.new_child(UiBuilder::new().max_rect(ui.max_rect()).id_salt(("home-refresh-old", slot)));
            child.set_clip_rect(ui.clip_rect());
            child.set_opacity(ui.opacity() * (1.0 - alpha));
            draw(&mut child, Some(old));
        }
        if alpha > 0.0 {
            let before = ui.opacity();
            ui.set_opacity(before * alpha);
            cards::scaled(ui, cell.center(), scale, |ui| draw(ui, None));
            ui.set_opacity(before);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_cards_that_changed_swap() {
        let mut seen: Seen<&str> = Seen::default();
        // The first sight of a row: it comes in with the page.
        assert!(seen.update(vec![1, 2, 3], || vec!["a", "b", "c"], 0.0).is_empty());
        // The same cards: nothing.
        assert!(seen.update(vec![1, 2, 3], || unreachable!("nothing changed"), 0.1).is_empty());
        // The second card changed and one more came at the end.
        let swaps = seen.update(vec![1, 5, 3, 4], || vec!["a", "e", "c", "d"], 0.2);
        let got: Vec<(usize, Option<&str>)> = swaps.iter().map(|s| (s.slot, s.old)).collect();
        assert_eq!(got, [(1, Some("b")), (3, None)]);
        // Over after a while.
        assert_eq!(seen.update(vec![1, 5, 3, 4], || unreachable!(), 0.5).len(), 2);
        assert!(seen.update(vec![1, 5, 3, 4], || unreachable!(), 3.0).is_empty());
        // A card changed twice: one swap, from the card it had last.
        seen.update(vec![1, 6, 3, 4], || vec!["a", "f", "c", "d"], 4.0);
        let swaps = seen.update(vec![1, 7, 3, 4], || vec!["a", "g", "c", "d"], 4.1);
        let got: Vec<(usize, Option<&str>)> = swaps.iter().map(|s| (s.slot, s.old)).collect();
        assert_eq!(got, [(1, Some("f"))]);
        // A shorter row: the swaps past its end go.
        assert!(seen.update(vec![1], || vec!["a"], 4.2).is_empty());
    }

    #[test]
    fn a_new_card_fades_and_springs_in() {
        let effects = nikgob_motion::Spring::new(1.0, 1600.0);
        let spatial = nikgob_motion::Spring::new(0.8, 380.0);
        assert_eq!(look(-0.1, false, effects, spatial), (0.0, FROM_SCALE), "its turn has not come");
        let mut last = 0.0;
        let mut top: f32 = 0.0;
        for i in 1..=120 {
            let (alpha, scale) = look(i as f32 / 60.0, false, effects, spatial);
            assert!(alpha + 1e-6 >= last, "the fade never goes back");
            assert!(scale >= FROM_SCALE - 1e-4, "{scale}");
            top = top.max(scale);
            last = alpha;
        }
        assert!(last > 0.999, "{last}");
        let (_, end) = look(2.0, false, effects, spatial);
        assert!((end - 1.0).abs() < 0.002, "{end}");
        assert!(top < 1.03, "a small spring, not a jump: {top}");
        // Less motion: the crossfade alone.
        assert_eq!(look(0.05, true, effects, spatial).1, 1.0);
        assert_eq!(look(-1.0, true, effects, spatial), (0.0, 1.0));
    }

    #[test]
    fn the_cards_come_in_along_the_row() {
        let cell = |x: f32| Rect::from_min_size(egui::pos2(x, 0.0), egui::vec2(200.0, 200.0));
        assert_eq!(place(cell(100.0), 100.0, 216.0), 0);
        assert_eq!(place(cell(100.0 + 3.0 * 216.0), 100.0, 216.0), 3);
        // Half out at the left: the first.
        assert_eq!(place(cell(-50.0), 100.0, 216.0), 0);
        assert_eq!(place(cell(500.0), 100.0, 0.0), 0);
    }
}
