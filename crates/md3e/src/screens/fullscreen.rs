//! Who wants the window full screen. F11 is the user's own switch; the
//! visualizer brings its remembered choice («во весь экран» or the
//! window) while it is up, and F11 inside it changes that choice. Closing
//! the visualizer puts the window back as F11 left it. A change the
//! window manager makes by itself is taken as the user's.

/// Seconds a sent change may take before the window's own state counts.
const SETTLE: f64 = 1.0;

pub(crate) struct Fullscreen {
    /// F11's choice: the window with the visualizer closed.
    user: bool,
    /// The visualizer's choice for this opening.
    viz: bool,
    /// What the window was last told, and when.
    sent: bool,
    sent_at: f64,
}

impl Default for Fullscreen {
    fn default() -> Self {
        // The window never starts full screen.
        Self { user: false, viz: false, sent: false, sent_at: f64::NEG_INFINITY }
    }
}

impl Fullscreen {
    /// Whether the window should be full screen now.
    pub fn want(&self, viz_open: bool) -> bool {
        if viz_open { self.viz || self.user } else { self.user }
    }

    /// The visualizer opened with its remembered choice.
    pub fn open_viz(&mut self, remembered: bool) {
        self.viz = remembered;
    }

    /// F11 or the visualizer's button. Inside the visualizer returns its
    /// new choice to remember.
    pub fn toggle(&mut self, viz_open: bool) -> Option<bool> {
        let next = !self.want(viz_open);
        if viz_open {
            self.viz = next;
            if !next {
                self.user = false;
            }
            Some(next)
        } else {
            self.user = next;
            None
        }
    }

    /// The window's real state (`None`: unknown) at `now`.
    pub fn observe(&mut self, actual: Option<bool>, viz_open: bool, now: f64) {
        let Some(actual) = actual else { return };
        if actual == self.sent || now - self.sent_at < SETTLE {
            return;
        }
        if viz_open {
            self.viz = actual;
            if !actual {
                self.user = false;
            }
        } else {
            self.user = actual;
        }
        self.sent = actual;
    }

    /// The state to send the window this frame, if it changed.
    pub fn command(&mut self, viz_open: bool, now: f64) -> Option<bool> {
        let want = self.want(viz_open);
        (want != self.sent).then(|| {
            self.sent = want;
            self.sent_at = now;
            want
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f11_works_alone_and_inside_the_visualizer() {
        let mut f = Fullscreen::default();
        assert_eq!(f.command(false, 0.0), None);
        // F11 alone.
        f.toggle(false);
        assert_eq!(f.command(false, 1.0), Some(true));
        f.toggle(false);
        assert_eq!(f.command(false, 2.0), Some(false));
        // F12 with the full-screen choice, then F11 inside: the window
        // leaves full screen and the choice is remembered.
        f.open_viz(true);
        assert_eq!(f.command(true, 3.0), Some(true));
        assert_eq!(f.toggle(true), Some(false));
        assert_eq!(f.command(true, 4.0), Some(false));
        assert_eq!(f.toggle(true), Some(true));
        assert_eq!(f.command(true, 5.0), Some(true));
        // Closing goes back to F11's window.
        assert_eq!(f.command(false, 6.0), Some(false));
    }

    #[test]
    fn the_visualizer_in_the_window_keeps_f11s_full_screen() {
        let mut f = Fullscreen::default();
        f.toggle(false);
        f.command(false, 0.0);
        f.open_viz(false);
        assert_eq!(f.command(true, 1.0), None, "already full screen");
        // F11 inside leaves full screen for good.
        assert_eq!(f.toggle(true), Some(false));
        assert_eq!(f.command(true, 2.0), Some(false));
        assert_eq!(f.command(false, 3.0), None);
        // Opened in the window, closed: nothing moves.
        f.open_viz(false);
        assert_eq!(f.command(true, 4.0), None);
        assert_eq!(f.command(false, 5.0), None);
    }

    #[test]
    fn a_change_from_outside_is_the_users() {
        let mut f = Fullscreen::default();
        f.toggle(false);
        f.command(false, 0.0);
        // The window has not got there yet: no change.
        f.observe(Some(false), false, 0.5);
        assert!(f.want(false));
        // The window manager took it out of full screen.
        f.observe(Some(false), false, 2.0);
        assert!(!f.want(false));
        assert_eq!(f.command(false, 2.0), None);
        // F11 then goes full screen again at once.
        f.toggle(false);
        assert_eq!(f.command(false, 3.0), Some(true));
    }
}
