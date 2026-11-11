//! How many cards (or columns) a width takes and how wide each one is.
//!
//! Every page that lays cards along its width asks here: the library's
//! grids keep a card at least as wide as their own (and stretch the cards
//! to fill the line), Главная's rows keep a card at most as wide as theirs
//! and show more of them on a wide window instead of growing them.

/// Items laid along a width, the same gap between two: how many and how
/// wide each one is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    pub count: usize,
    pub size: f32,
}

/// A width this close to an exact number of items takes that number (no
/// extra item for a rounding error).
const EPS: f32 = 1e-3;

impl Fit {
    /// `count` items sharing `width`, `gap` apart.
    pub fn even(width: f32, gap: f32, count: usize) -> Self {
        let count = count.max(1);
        Self { count, size: ((width - gap * (count - 1) as f32) / count as f32).max(0.0) }
    }

    /// As many items as fit `width` at least `min` wide each, stretched to
    /// fill it; one at least.
    pub fn at_least(width: f32, gap: f32, min: f32) -> Self {
        let count = (((width + gap) / (min + gap).max(1.0) + EPS).floor() as usize).max(1);
        Self::even(width, gap, count)
    }

    /// The fewest items, `least` at least, of which none is wider than
    /// `max`: a wider `width` takes more of them, not wider ones.
    pub fn at_most(width: f32, gap: f32, max: f32, least: usize) -> Self {
        let count = (((width + gap) / (max + gap).max(1.0) - EPS).ceil().max(1.0) as usize).max(least);
        Self::even(width, gap, count)
    }

    /// From one item's left edge to the next one's.
    pub fn pitch(&self, gap: f32) -> f32 {
        self.size + gap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn even_items_share_the_width() {
        let f = Fit::even(1000.0, 16.0, 4);
        assert_eq!(f.count, 4);
        assert_eq!(f.size, 238.0);
        assert_eq!(f.pitch(16.0), 254.0);
        assert_eq!(Fit::even(100.0, 16.0, 0).count, 1, "one at least");
        assert_eq!(Fit::even(10.0, 16.0, 4).size, 0.0, "never below nothing");
    }

    #[test]
    fn at_least_keeps_its_smallest_size_and_stretches() {
        // The library's grid: cards of 160 at least, 12 apart.
        for (width, count) in [(150.0, 1), (331.0, 1), (332.0, 2), (1000.0, 5), (1792.0, 10), (2432.0, 14)] {
            let f = Fit::at_least(width, 12.0, 160.0);
            assert_eq!(f.count, count, "{width}");
            assert!(f.size >= 160.0 || count == 1, "{width}: {}", f.size);
            assert!((f.size * count as f32 + 12.0 * (count - 1) as f32 - width).abs() < 0.01, "fills {width}");
        }
    }

    #[test]
    fn at_most_adds_items_instead_of_growing_them() {
        // Exactly six of 200: six, not seven.
        let f = Fit::at_most(6.0 * 200.0 + 5.0 * 16.0, 16.0, 200.0, 1);
        assert_eq!((f.count, f.size), (6, 200.0));
        for width in [712.0, 999.0, 1280.0, 1281.0, 1600.0, 1792.0, 2432.0, 3000.0] {
            let f = Fit::at_most(width, 16.0, 200.0, 3);
            assert!(f.size <= 200.0 + 0.01, "{width}: {}", f.size);
            // One fewer would be wider than the most.
            assert!(Fit::even(width, 16.0, f.count - 1).size > 200.0 || f.count == 3, "{width}");
        }
        // Never fewer than asked, however wide each one gets.
        assert_eq!(Fit::at_most(500.0, 16.0, 200.0, 3).count, 3);
        assert_eq!(Fit::at_most(0.0, 16.0, 200.0, 0).count, 1);
    }
}
